//! SftpView — 左右双栏 SFTP 文件管理（Sprint 7）
//!
//! - 左：本地目录；右：远端目录
//! - 双击目录进入；双击远端文件下载 / 本地文件上传
//! - 列头排序、右键菜单（上传/下载/重命名/删除/权限/属性）
//! - 底部传输面板：进度、暂停/继续、取消
//! - 每个 SFTP Tab 独立 SSH 连接（不占用终端 Tab 的 PTY 会话）

use std::cell::Cell;
use std::path::{Path as StdPath, PathBuf};
use std::rc::Rc;
use std::time::{Duration, Instant, UNIX_EPOCH};

use futures::channel::mpsc::unbounded;
use futures::StreamExt;
use gpui::prelude::*;
use gpui::{
    div, point, px, transparent_black, AnyElement, Bounds, ClickEvent, Context, Entity,
    FocusHandle, Focusable, IntoElement, KeyDownEvent, MouseButton, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, Pixels, Point, Render, ScrollWheelEvent, SharedString, Window,
};
use uuid::Uuid;

use rterm_session::Session as SessionConfig;
use rterm_sftp::{
    format_mode, format_size, remote_join, remote_parent, sort_entries, FileEntry, SftpService,
    SortKey, TransferDirection, TransferEvent, TransferQueue, TransferSnapshot, TransferState,
};
use rterm_ssh::SshSession;

use super::{
    CONTENT_BORDER_W, MENU_ITEM_H, MENU_PADDING_Y, SFTP_MENU_W, SIDEBAR_W, STATUS_BAR_H, TAB_BAR_H,
    TOOLBAR_H,
};
use crate::event::{AppEvent, EventBusHost};
use crate::state::{lock_or_recover, AppState};
use crate::ui::theme;
use crate::views::text_input::TextInput;

const PANE_HEADER_H: f32 = 34.0;
const BREADCRUMB_H: f32 = 22.0;
const COLUMN_HEADER_H: f32 = 22.0;
const ROW_H: f32 = 24.0;
const TRANSFER_ROW_H: f32 = 30.0;
const MAX_VISIBLE_TRANSFERS: usize = 4;
const DOUBLE_CLICK_WINDOW: Duration = Duration::from_millis(400);
const REMOTE_PLACEHOLDER: &str = "远端未连接";

// 文件列表列宽：表头与数据行共用同一组常量，保证任何容器宽度下都左右对齐。
// 名称列不设固定宽度（`flex_1` 吃掉剩余空间），定宽列禁止收缩。
const COL_ICON_W: f32 = 20.0;
const COL_SIZE_W: f32 = 70.0;
const COL_MODE_W: f32 = 80.0;
const COL_MTIME_W: f32 = 110.0;

/// 窄面板时按可用宽度裁剪列：先隐藏「修改时间」，再隐藏「权限」。
#[derive(Clone, Copy, Debug)]
struct SftpColumns {
    mode: bool,
    mtime: bool,
}

impl SftpColumns {
    fn for_width(width: f32) -> Self {
        Self {
            mode: width >= 340.0,
            mtime: width >= 420.0,
        }
    }
}

// ---------------------------------------------------------------------------
// 内部状态
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PaneSide {
    Local,
    Remote,
}

impl PaneSide {
    fn label(self) -> &'static str {
        match self {
            Self::Local => "本地",
            Self::Remote => "远端",
        }
    }

    fn is_remote(self) -> bool {
        matches!(self, Self::Remote)
    }
}

struct Pane {
    path: String,
    rows: Vec<FileEntry>,
    selected: Option<usize>,
    scroll: usize,
    address: Entity<TextInput>,
    error: Option<String>,
}

#[derive(Clone, Copy)]
enum DialogKind {
    Mkdir(PaneSide),
    Rename(PaneSide, usize),
    Chmod(PaneSide, usize),
}

struct DialogState {
    kind: DialogKind,
    title: &'static str,
    input: Entity<TextInput>,
}

struct MenuState {
    side: PaneSide,
    row: Option<usize>,
    x: f32,
    y: f32,
}

#[derive(Clone, Copy)]
enum MenuAction {
    Download(usize),
    Upload(usize),
    Rename(usize),
    Delete(usize),
    Chmod(usize),
    Properties(usize),
    NewFolder,
    Refresh,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SftpStatus {
    Connecting,
    Connected,
    Failed,
}

/// SFTP 视图展示模式（S12 12.1）：独立 Tab 或终端右侧 Dock。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SftpDisplayMode {
    /// 独立 SFTP Tab：显示顶部工具条（标题 / 状态 / 新建文件夹 / 刷新）。
    Tab,
    /// 终端右侧 Dock：隐藏工具条，由 TerminalTabView 提供开关与分隔条。
    Dock,
}

/// 后台连接结果：SSH + SFTP 子系统 + 远端 home 目录一次性完成。
struct ConnectedSftp {
    service: SftpService,
    home: String,
}

/// 后台远端目录读取结果。
struct RemoteNavResult {
    generation: u64,
    path: String,
    result: Result<Vec<FileEntry>, String>,
}

/// 后台本地目录读取结果（9.18：慢盘 / 网络盘不再卡 UI 线程）。
struct LocalNavResult {
    generation: u64,
    path: String,
    result: Result<Vec<FileEntry>, String>,
}

/// Pane 间拖拽状态（9.18）：从某行按下，移动超过阈值才算拖拽。
struct DragState {
    side: PaneSide,
    index: usize,
    start_x: f32,
    start_y: f32,
    active: bool,
}

/// 等待用户确认“目标已存在，是否覆盖”的传输（S7.5.3.10）。
struct PendingTransferConfirm {
    direction: TransferDirection,
    entry: FileEntry,
    /// 下载时是本地目标目录，上传时是远端目标目录。
    dir: String,
}

/// 内部确认卡片的动作类型。
#[derive(Clone, Copy)]
enum ConfirmKind {
    DeleteRow,
    Transfer,
}

/// 内部确认卡片的文案与按钮配置。
struct ConfirmCard {
    message: String,
    detail: Option<String>,
    ok_label: &'static str,
    ok_id: &'static str,
    cancel_id: &'static str,
    kind: ConfirmKind,
    /// 确认按钮是否使用危险色。
    danger: bool,
}

// ---------------------------------------------------------------------------
// SftpView
// ---------------------------------------------------------------------------

pub struct SftpView {
    focus_handle: FocusHandle,
    tab_id: Uuid,
    display_mode: SftpDisplayMode,
    session_id: Option<Uuid>,
    session: Option<SessionConfig>,
    status: SftpStatus,
    status_text: String,
    service: Option<std::sync::Arc<SftpService>>,
    queue: Option<std::sync::Arc<TransferQueue>>,
    /// 远端 home 目录：连接成功后由 `ConnectedSftp.home` 填充，用于 `~` 展开。
    remote_home: Option<String>,
    transfers: Vec<TransferSnapshot>,
    local: Pane,
    remote: Pane,
    active_side: PaneSide,
    sort_key: SortKey,
    sort_ascending: bool,
    menu: Option<MenuState>,
    dialog: Option<DialogState>,
    /// 等待确认的删除（pane + 文件名）。存名字而不是行号，避免确认期间列表刷新后删错行。
    pending_delete: Option<(PaneSide, String)>,
    /// 等待确认的覆盖传输。
    pending_transfer: Option<PendingTransferConfirm>,
    last_click: Option<(PaneSide, usize, Instant)>,
    generation: u64,
    /// 远端导航代际：只应用最新一次 listdir 的结果。
    remote_nav_generation: u64,
    /// 本地导航代际：慢盘读目录放到后台线程后，只应用最新结果。
    local_nav_generation: u64,
    /// 拖拽中的行（None = 没有拖拽）。
    drag: Option<DragState>,
    /// 文件列表可视区高度（每帧 render 时按窗口大小更新，用于虚拟化）。
    rows_view_h: f32,
    /// SftpView 根元素在窗口中的实测矩形（prepaint 更新）：
    /// Dock 模式下视图不在内容区左上角，右键菜单需要它把窗口坐标换算为局部坐标。
    root_bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
}

impl SftpView {
    pub fn new(cx: &mut Context<Self>) -> Self {
        Self::new_with_mode(cx, SftpDisplayMode::Tab)
    }

    /// 指定展示模式构造（S12 12.1）；Dock 模式隐藏顶部工具条。
    pub fn new_with_mode(cx: &mut Context<Self>, display_mode: SftpDisplayMode) -> Self {
        let local_home = std::env::var("USERPROFILE")
            .or_else(|_| std::env::var("HOME"))
            .unwrap_or_else(|_| "C:\\".to_string());
        let mut local_rows = list_local(&local_home).unwrap_or_default();
        sort_entries(&mut local_rows, SortKey::Name, true);
        let local_error = list_local(&local_home).err();

        let local_address = cx.new(|cx| {
            let mut input = TextInput::new(cx).with_placeholder("本地路径");
            input.content = SharedString::from(local_home.clone());
            input
        });
        let remote_address = cx.new(|cx| TextInput::new(cx).with_placeholder("/home/user"));

        Self {
            focus_handle: cx.focus_handle(),
            tab_id: Uuid::new_v4(),
            display_mode,
            session_id: None,
            session: None,
            status: SftpStatus::Connecting,
            status_text: "等待连接…".to_string(),
            service: None,
            queue: None,
            remote_home: None,
            transfers: Vec::new(),
            local: Pane {
                path: local_home,
                rows: local_rows,
                selected: None,
                scroll: 0,
                address: local_address,
                error: local_error,
            },
            remote: Pane {
                path: String::new(),
                rows: Vec::new(),
                selected: None,
                scroll: 0,
                address: remote_address,
                error: None,
            },
            active_side: PaneSide::Local,
            sort_key: SortKey::Name,
            sort_ascending: true,
            menu: None,
            dialog: None,
            pending_delete: None,
            pending_transfer: None,
            last_click: None,
            generation: 0,
            remote_nav_generation: 0,
            local_nav_generation: 0,
            drag: None,
            rows_view_h: ROW_H * 12.0,
            root_bounds: Rc::new(Cell::new(None)),
        }
    }

    pub fn tab_id(&self) -> Uuid {
        self.tab_id
    }

    pub fn focus_handle(&self) -> FocusHandle {
        self.focus_handle.clone()
    }

    /// 窗口坐标 → SftpView 局部坐标（Tab / Dock 模式通用）。
    fn to_local(&self, position: Point<Pixels>) -> Point<Pixels> {
        let origin = self
            .root_bounds
            .get()
            .map(|bounds| bounds.origin)
            .unwrap_or(point(px(0.), px(0.)));
        point(position.x - origin.x, position.y - origin.y)
    }

    /// 是否有内部弹窗（输入框或确认卡片）打开；用于屏蔽全局快捷键。
    pub fn has_modal_dialog(&self) -> bool {
        self.dialog.is_some() || self.pending_delete.is_some() || self.pending_transfer.is_some()
    }

    /// 打开 SFTP Tab：独立 SSH 连接 + SFTP 子系统。
    pub fn connect(&mut self, session_id: Uuid, cx: &mut Context<Self>) {
        let stored = {
            let state = cx.global::<AppState>();
            let store = lock_or_recover(&state.session_store);
            store.get(session_id).cloned()
        };
        let Some(stored) = stored else {
            self.fail(format!("会话不存在: {session_id}"), cx);
            return;
        };
        let session = match rterm_session::crypto_bridge::decrypt_session_dpapi(&stored) {
            Ok(session) => session,
            Err(error) => {
                self.fail(format!("解密会话失败: {error:#}"), cx);
                return;
            }
        };
        if session.host.trim().is_empty() {
            self.fail("会话主机地址为空".into(), cx);
            return;
        }

        log::info!(
            "[sftp] connect requested: {}@{}:{}",
            session.username,
            session.host,
            session.port
        );

        self.session = Some(session.clone());
        self.session_id = Some(session_id);
        self.status = SftpStatus::Connecting;
        self.status_text = format!("连接 {}…", session.display_label());
        self.generation = self.generation.wrapping_add(1);
        // 失效所有在途的远端导航结果。
        self.remote_nav_generation = self.remote_nav_generation.wrapping_add(1);
        let generation = self.generation;

        // SSH 连接、SFTP 子系统与 home 解析全部在后台线程完成，避免阻塞 UI。
        let (tx, mut rx) = unbounded::<Result<ConnectedSftp, String>>();
        let spawned = std::thread::Builder::new()
            .name("rterm-sftp-connect".into())
            .spawn(move || {
                let result = (|| -> Result<ConnectedSftp, String> {
                    let mut ssh = SshSession::new();
                    ssh.connect(&session)
                        .map_err(|error| rterm_ssh::errors::describe(&error))?;
                    let raw = ssh
                        .session_clone()
                        .ok_or_else(|| "SSH 会话不可用".to_string())?;
                    let service = SftpService::new(raw).map_err(|error| format!("{error:#}"))?;
                    let home = match service.home_dir() {
                        Ok(home) => home,
                        Err(error) => {
                            log::warn!("[sftp] 解析远端 home 失败，回退到 /: {error:#}");
                            "/".to_string()
                        }
                    };
                    Ok(ConnectedSftp { service, home })
                })();
                let _ = tx.unbounded_send(result);
            });
        if let Err(error) = spawned {
            self.fail(format!("无法启动 SFTP 连接线程: {error}"), cx);
            return;
        }

        cx.spawn(async move |this, cx| {
            if let Some(result) = rx.next().await {
                let _ = this.update(cx, |this, cx| this.handle_connected(generation, result, cx));
            }
        })
        .detach();

        cx.notify();
    }

    /// 关闭 Tab 时调用。
    pub fn shutdown(&mut self, cx: &mut Context<Self>) {
        self.generation = self.generation.wrapping_add(1);
        self.remote_nav_generation = self.remote_nav_generation.wrapping_add(1);
        if let Some(queue) = self.queue.take() {
            // shutdown 会 join worker；worker 退出后 events sender 全部释放，
            // 转发任务随之结束，不会形成 Arc 循环。
            queue.shutdown();
            drop(queue);
        }
        self.service = None;
        self.remote_home = None;
        self.pending_delete = None;
        self.pending_transfer = None;
        self.status = SftpStatus::Failed;
        self.status_text = "已断开".to_string();
        self.transfers.clear();
        self.emit(
            cx,
            AppEvent::SftpClosed {
                tab_id: self.tab_id,
            },
        );
        cx.notify();
    }

    fn handle_connected(
        &mut self,
        generation: u64,
        result: Result<ConnectedSftp, String>,
        cx: &mut Context<Self>,
    ) {
        if generation != self.generation {
            return;
        }
        match result {
            Ok(ConnectedSftp { service, home }) => {
                let service = std::sync::Arc::new(service);
                self.status = SftpStatus::Connected;
                self.status_text = format!("已连接 · {home}");
                self.service = Some(service.clone());
                self.remote_home = Some(home.clone());

                let (events_tx, mut events_rx) = unbounded::<TransferEvent>();
                let queue = std::sync::Arc::new(TransferQueue::new(service.clone(), events_tx, 2));
                self.queue = Some(queue);

                let generation = self.generation;
                // 只捕获 events_rx：队列由 SftpView 持有，关闭 Tab 后 sender 释放、任务退出。
                cx.spawn(async move |this, cx| {
                    while let Some(event) = events_rx.next().await {
                        let alive = this
                            .update(cx, |this, cx| {
                                this.handle_transfer_event(generation, event, cx)
                            })
                            .is_ok();
                        if !alive {
                            break;
                        }
                    }
                })
                .detach();

                // 首屏目录读取同样走后台导航。
                self.navigate(PaneSide::Remote, home, cx);
                if let Some(session_id) = self.session_id {
                    self.emit(
                        cx,
                        AppEvent::SftpOpened {
                            tab_id: self.tab_id,
                            session_id,
                        },
                    );
                }
                log::info!("[sftp] ready (tab {})", self.tab_id);
            }
            Err(reason) => {
                self.fail(reason, cx);
            }
        }
        cx.notify();
    }

    fn fail(&mut self, reason: String, cx: &mut Context<Self>) {
        log::warn!("[sftp] connection failed: {reason}");
        self.status = SftpStatus::Failed;
        self.status_text = reason.clone();
        self.remote.error = Some(reason.clone());
        self.emit(
            cx,
            AppEvent::StatusBarMessage {
                text: format!("SFTP 连接失败：{reason}"),
                duration_ms: 3000,
            },
        );
        cx.notify();
    }

    fn emit(&self, cx: &mut Context<Self>, event: AppEvent) {
        let host = cx.global::<AppState>().event_bus_host.clone();
        cx.update_entity(&host, |_host, cx: &mut Context<EventBusHost>| {
            cx.emit(event);
        });
    }

    fn pane(&self, side: PaneSide) -> &Pane {
        if side.is_remote() {
            &self.remote
        } else {
            &self.local
        }
    }

    fn pane_mut(&mut self, side: PaneSide) -> &mut Pane {
        if side.is_remote() {
            &mut self.remote
        } else {
            &mut self.local
        }
    }

    // -----------------------------------------------------------------------
    // 导航
    // -----------------------------------------------------------------------

    /// 远端 home：MainWindow 用它把终端的 `~` 解析成绝对路径。
    pub fn remote_home(&self) -> Option<String> {
        self.remote_home.clone()
    }

    /// 路径同步入口：MainWindow 收到 TerminalCwdHint 后调用。
    pub fn navigate_to_remote(&mut self, path: &str, cx: &mut Context<Self>) {
        self.navigate(PaneSide::Remote, path.to_string(), cx);
    }

    fn navigate(&mut self, side: PaneSide, raw_path: String, cx: &mut Context<Self>) {
        if side.is_remote() {
            let Some(service) = self.service.clone() else {
                self.remote.error = Some(REMOTE_PLACEHOLDER.to_string());
                cx.notify();
                return;
            };
            let path = normalize_remote(&raw_path, &self.remote.path, self.remote_home.as_deref());
            // 远端 listdir 走后台线程，仅应用最新一次导航结果。
            self.remote_nav_generation = self.remote_nav_generation.wrapping_add(1);
            let nav_generation = self.remote_nav_generation;
            let (tx, mut rx) = unbounded::<RemoteNavResult>();
            let spawned = std::thread::Builder::new()
                .name("rterm-sftp-nav".into())
                .spawn(move || {
                    let result = service.listdir(&path).map_err(|error| format!("{error:#}"));
                    let _ = tx.unbounded_send(RemoteNavResult {
                        generation: nav_generation,
                        path,
                        result,
                    });
                });
            if let Err(error) = spawned {
                self.remote.error = Some(format!("无法启动目录读取线程: {error}"));
                self.menu = None;
                cx.notify();
                return;
            }
            cx.spawn(async move |this, cx| {
                if let Some(nav) = rx.next().await {
                    let _ = this.update(cx, |this, cx| this.apply_remote_nav(nav, cx));
                }
            })
            .detach();
        } else {
            // 9.18：本地 listdir 也放后台线程（慢盘 / 网络盘不阻塞 UI）
            let path = normalize_local(&raw_path, &self.local.path);
            self.local_nav_generation = self.local_nav_generation.wrapping_add(1);
            let nav_generation = self.local_nav_generation;
            let (tx, mut rx) = unbounded::<LocalNavResult>();
            let spawned = std::thread::Builder::new()
                .name("rterm-sftp-local-nav".into())
                .spawn(move || {
                    let result = list_local(&path);
                    let _ = tx.unbounded_send(LocalNavResult {
                        generation: nav_generation,
                        path,
                        result,
                    });
                });
            if let Err(error) = spawned {
                self.local.error = Some(format!("无法启动本地目录读取线程: {error}"));
                self.menu = None;
                cx.notify();
                return;
            }
            cx.spawn(async move |this, cx| {
                if let Some(nav) = rx.next().await {
                    let _ = this.update(cx, |this, cx| this.apply_local_nav(nav, cx));
                }
            })
            .detach();
        }
        self.menu = None;
        cx.notify();
    }

    /// 应用后台本地 listdir 结果；过期导航直接丢弃。
    fn apply_local_nav(&mut self, nav: LocalNavResult, cx: &mut Context<Self>) {
        if nav.generation != self.local_nav_generation {
            return;
        }
        match nav.result {
            Ok(mut rows) => {
                sort_entries(&mut rows, self.sort_key, self.sort_ascending);
                self.local.rows = rows;
                self.local.path = nav.path.clone();
                self.local.selected = None;
                self.local.scroll = 0;
                self.local.error = None;
                self.set_address(PaneSide::Local, &nav.path, cx);
            }
            Err(error) => self.local.error = Some(error),
        }
        cx.notify();
    }

    /// 应用后台 listdir 结果；过期导航（或已断开）直接丢弃。
    fn apply_remote_nav(&mut self, nav: RemoteNavResult, cx: &mut Context<Self>) {
        if nav.generation != self.remote_nav_generation || self.service.is_none() {
            return;
        }
        match nav.result {
            Ok(mut rows) => {
                sort_entries(&mut rows, self.sort_key, self.sort_ascending);
                self.remote.rows = rows;
                self.remote.path = nav.path.clone();
                self.remote.selected = None;
                self.remote.scroll = 0;
                self.remote.error = None;
                // 顶部状态标签跟随导航刷新，而不是停留在连接时的 home。
                self.status_text = format!("已连接 · {}", self.remote.path);
                self.set_address(PaneSide::Remote, &nav.path, cx);
                if let Some(session_id) = self.session_id {
                    self.emit(
                        cx,
                        AppEvent::SftpNavigated {
                            tab_id: self.tab_id,
                            session_id,
                            path: nav.path,
                        },
                    );
                }
            }
            Err(error) => {
                self.remote.error = Some(error);
            }
        }
        cx.notify();
    }

    fn set_address(&mut self, side: PaneSide, path: &str, cx: &mut Context<Self>) {
        let address = self.pane(side).address.clone();
        let path = path.to_string();
        address.update(cx, |input, cx| {
            input.set_content(path, cx);
        });
    }

    /// 在后台线程执行远端变更操作，完成后回到 UI 线程提示并异步刷新当前远端目录。
    fn run_remote_mutation(
        &mut self,
        success_message: String,
        failure_prefix: &'static str,
        cx: &mut Context<Self>,
        op: impl FnOnce(&SftpService) -> anyhow::Result<()> + Send + 'static,
    ) {
        let Some(service) = self.service.clone() else {
            self.set_status("远端未连接".into(), cx);
            return;
        };
        let generation = self.generation;
        let (tx, mut rx) = unbounded::<Result<(), String>>();
        let spawned = std::thread::Builder::new()
            .name("rterm-sftp-op".into())
            .spawn(move || {
                let result = op(&service).map_err(|error| format!("{error:#}"));
                let _ = tx.unbounded_send(result);
            });
        if let Err(error) = spawned {
            self.set_status(format!("{failure_prefix}: 无法启动后台任务 ({error})"), cx);
            return;
        }
        cx.spawn(async move |this, cx| {
            if let Some(result) = rx.next().await {
                let _ = this.update(cx, |this, cx| {
                    if generation != this.generation {
                        return;
                    }
                    match result {
                        Ok(()) => this.set_status(success_message, cx),
                        Err(error) => this.set_status(format!("{failure_prefix}: {error}"), cx),
                    }
                    let current = this.remote.path.clone();
                    this.navigate(PaneSide::Remote, current, cx);
                });
            }
        })
        .detach();
    }

    fn refresh_both(&mut self, cx: &mut Context<Self>) {
        let local = self.local.path.clone();
        self.navigate(PaneSide::Local, local, cx);
        if self.service.is_some() {
            let remote = self.remote.path.clone();
            self.navigate(PaneSide::Remote, remote, cx);
        }
    }

    fn sort_by(&mut self, key: SortKey, cx: &mut Context<Self>) {
        if self.sort_key == key {
            self.sort_ascending = !self.sort_ascending;
        } else {
            self.sort_key = key;
            self.sort_ascending = true;
        }
        sort_entries(&mut self.local.rows, self.sort_key, self.sort_ascending);
        sort_entries(&mut self.remote.rows, self.sort_key, self.sort_ascending);
        cx.notify();
    }

    fn on_pane_scroll(&mut self, side: PaneSide, event: &ScrollWheelEvent, cx: &mut Context<Self>) {
        let pixel = event.delta.pixel_delta(px(ROW_H));
        let dy = f32::from(pixel.y);
        let lines = if event.delta.precise() {
            (dy / ROW_H).round() as i32
        } else if dy.abs() > 0.1 {
            3 * dy.signum() as i32
        } else {
            0
        };
        if lines == 0 {
            return;
        }
        let visible = visible_row_count(self.rows_view_h);
        let pane = self.pane_mut(side);
        let max = max_scroll_offset(pane.rows.len(), visible);
        pane.scroll = (pane.scroll as i32 - lines).clamp(0, max as i32) as usize;
        cx.notify();
    }

    // -----------------------------------------------------------------------
    // Pane 间拖拽（9.18）
    // -----------------------------------------------------------------------

    /// 在行上按下左键：记录拖拽起点，移动超过阈值后才算拖拽。
    fn begin_drag(
        &mut self,
        side: PaneSide,
        index: usize,
        event: &MouseDownEvent,
        _cx: &mut Context<Self>,
    ) {
        self.drag = Some(DragState {
            side,
            index,
            start_x: f32::from(event.position.x),
            start_y: f32::from(event.position.y),
            active: false,
        });
    }

    fn on_drag_move(&mut self, event: &MouseMoveEvent, cx: &mut Context<Self>) {
        let Some(drag) = &mut self.drag else {
            return;
        };
        if drag.active {
            return;
        }
        let dx = f32::from(event.position.x) - drag.start_x;
        let dy = f32::from(event.position.y) - drag.start_y;
        if dx * dx + dy * dy <= 16.0 {
            return;
        }
        drag.active = true;
        let (side, index) = (drag.side, drag.index);
        // Dock 模式只有远端栏，没有「另一侧面板」可拖放
        if self.display_mode == SftpDisplayMode::Dock {
            self.set_status("侧栏模式不支持拖拽传输，请右键文件选择下载".into(), cx);
            return;
        }
        let name = self
            .pane(side)
            .rows
            .get(index)
            .map(|entry| entry.name.clone())
            .unwrap_or_else(|| "项目".to_string());
        self.set_status(format!("拖动 {name}：松开到另一侧面板开始传输"), cx);
    }

    /// 松开左键：拖拽跨越两侧面板时触发上传 / 下载。
    fn finish_drag(&mut self, event: &MouseUpEvent, window: &mut Window, cx: &mut Context<Self>) {
        let Some(drag) = self.drag.take() else {
            return;
        };
        if !drag.active || self.display_mode == SftpDisplayMode::Dock {
            return;
        }
        let x = f32::from(event.position.x) - SIDEBAR_W - CONTENT_BORDER_W;
        let split = (f32::from(window.viewport_size().width) - SIDEBAR_W - CONTENT_BORDER_W) / 2.0;
        let target = if x < split {
            PaneSide::Local
        } else {
            PaneSide::Remote
        };
        if target == drag.side {
            self.set_status("请拖到另一侧面板完成传输".into(), cx);
            return;
        }
        let Some(entry) = self.pane(drag.side).rows.get(drag.index).cloned() else {
            return;
        };
        if entry.is_dir {
            self.set_status("暂不支持拖拽目录（请双击进入后拖拽文件）".into(), cx);
            return;
        }
        match drag.side {
            PaneSide::Local => {
                let remote_dir = self.remote.path.clone();
                self.request_upload(&entry, &remote_dir, cx);
            }
            PaneSide::Remote => {
                let local_dir = PathBuf::from(&self.local.path);
                self.request_download(&entry, &local_dir, cx);
            }
        }
    }

    // -----------------------------------------------------------------------
    // 点击 / 双击 / 右键
    // -----------------------------------------------------------------------

    fn click_row(&mut self, side: PaneSide, index: usize, cx: &mut Context<Self>) {
        self.active_side = side;
        self.pane_mut(side).selected = Some(index);
        self.menu = None;

        let now = Instant::now();
        let is_double = matches!(
            self.last_click,
            Some((last_side, last_index, at))
                if last_side == side && last_index == index
                    && now.duration_since(at) <= DOUBLE_CLICK_WINDOW
        );
        self.last_click = Some((side, index, now));

        if is_double {
            self.activate_row(side, index, cx);
        }
        cx.notify();
    }

    fn activate_row(&mut self, side: PaneSide, index: usize, cx: &mut Context<Self>) {
        let Some(entry) = self.pane(side).rows.get(index).cloned() else {
            return;
        };
        if entry.is_dir {
            self.navigate(side, entry.path, cx);
            return;
        }
        match side {
            PaneSide::Remote => {
                let local_dir = PathBuf::from(&self.local.path);
                self.request_download(&entry, &local_dir, cx);
            }
            PaneSide::Local => {
                let remote_dir = self.remote.path.clone();
                self.request_upload(&entry, &remote_dir, cx);
            }
        }
    }

    /// 下载入口：本地目标已存在时先确认覆盖，否则直接入队。
    fn request_download(&mut self, entry: &FileEntry, local_dir: &StdPath, cx: &mut Context<Self>) {
        let local_path = local_dir.join(&entry.name);
        if local_path.exists() {
            self.pending_transfer = Some(PendingTransferConfirm {
                direction: TransferDirection::Download,
                entry: entry.clone(),
                dir: local_dir.to_string_lossy().into_owned(),
            });
            cx.notify();
            return;
        }
        self.enqueue_download(entry, local_dir, cx);
    }

    /// 上传入口：后台 stat 远端目标，已存在时先确认覆盖，否则直接入队。
    fn request_upload(&mut self, entry: &FileEntry, remote_dir: &str, cx: &mut Context<Self>) {
        if remote_dir.is_empty() {
            self.set_status("远端目录未就绪".into(), cx);
            return;
        }
        let Some(service) = self.service.clone() else {
            self.set_status("远端未连接，无法上传".into(), cx);
            return;
        };
        let remote_path = remote_join(remote_dir, &entry.name);
        let entry = entry.clone();
        let dir = remote_dir.to_string();
        let generation = self.generation;
        let (tx, mut rx) = unbounded::<bool>();
        let spawned = std::thread::Builder::new()
            .name("rterm-sftp-stat".into())
            .spawn(move || {
                // stat 失败（含目标不存在）按“不存在”处理，直接上传。
                let exists = service.stat(&remote_path).is_ok();
                let _ = tx.unbounded_send(exists);
            });
        if let Err(error) = spawned {
            self.set_status(format!("上传失败: 无法启动后台任务 ({error})"), cx);
            return;
        }
        cx.spawn(async move |this, cx| {
            if let Some(exists) = rx.next().await {
                let _ = this.update(cx, |this, cx| {
                    if generation != this.generation {
                        return;
                    }
                    if exists {
                        this.pending_transfer = Some(PendingTransferConfirm {
                            direction: TransferDirection::Upload,
                            entry: entry.clone(),
                            dir,
                        });
                        cx.notify();
                    } else {
                        this.enqueue_upload(&entry, &dir, cx);
                    }
                });
            }
        })
        .detach();
    }

    /// 确认覆盖：执行真正的入队。
    fn confirm_pending_transfer(&mut self, cx: &mut Context<Self>) {
        let Some(pending) = self.pending_transfer.take() else {
            return;
        };
        match pending.direction {
            TransferDirection::Download => {
                let dir = PathBuf::from(&pending.dir);
                self.enqueue_download(&pending.entry, &dir, cx);
            }
            TransferDirection::Upload => {
                self.enqueue_upload(&pending.entry, &pending.dir, cx);
            }
        }
    }

    fn enqueue_download(&mut self, entry: &FileEntry, local_dir: &StdPath, cx: &mut Context<Self>) {
        let Some(queue) = self.queue.clone() else {
            self.set_status("远端未连接，无法下载".into(), cx);
            return;
        };
        let local_path = local_dir.join(&entry.name);
        let id = queue.enqueue_download(entry.path.clone(), local_path, entry.size);
        self.set_status(format!("开始下载 {}（{id}）", entry.name), cx);
        self.transfers = queue.snapshots();
        cx.notify();
    }

    fn enqueue_upload(&mut self, entry: &FileEntry, remote_dir: &str, cx: &mut Context<Self>) {
        let Some(queue) = self.queue.clone() else {
            self.set_status("远端未连接，无法上传".into(), cx);
            return;
        };
        if remote_dir.is_empty() {
            self.set_status("远端目录未就绪".into(), cx);
            return;
        }
        let remote_path = remote_join(remote_dir, &entry.name);
        match queue.enqueue_upload(&remote_path, PathBuf::from(&entry.path)) {
            Ok(id) => {
                self.set_status(format!("开始上传 {}（{id}）", entry.name), cx);
                self.transfers = queue.snapshots();
            }
            Err(error) => self.set_status(format!("上传失败: {error}"), cx),
        }
        cx.notify();
    }

    fn open_menu(
        &mut self,
        side: PaneSide,
        row: Option<usize>,
        x: f32,
        y: f32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // 把焦点收到 SFTP 视图，Esc 才能冒泡到 handle_key 关闭菜单
        window.focus(&self.focus_handle, cx);
        self.active_side = side;
        if let Some(index) = row {
            self.pane_mut(side).selected = Some(index);
        }
        self.menu = Some(MenuState { side, row, x, y });
        cx.notify();
    }

    fn handle_menu_action(&mut self, action: MenuAction, cx: &mut Context<Self>) {
        self.menu = None;
        match action {
            MenuAction::Download(index) => {
                let Some(entry) = self.remote.rows.get(index).cloned() else {
                    return;
                };
                let local_dir = PathBuf::from(&self.local.path);
                self.request_download(&entry, &local_dir, cx);
            }
            MenuAction::Upload(index) => {
                let Some(entry) = self.local.rows.get(index).cloned() else {
                    return;
                };
                let remote_dir = self.remote.path.clone();
                self.request_upload(&entry, &remote_dir, cx);
            }
            MenuAction::Rename(index) => {
                self.open_dialog_for_row(DialogKind::Rename(self.active_side, index), cx)
            }
            MenuAction::Delete(index) => {
                // 不立即删除：先弹出确认卡片。记录文件名而不是行号，避免列表刷新后删错。
                if let Some(entry) = self.pane(self.active_side).rows.get(index) {
                    self.pending_delete = Some((self.active_side, entry.name.clone()));
                }
            }
            MenuAction::Chmod(index) => {
                self.open_dialog_for_row(DialogKind::Chmod(self.active_side, index), cx)
            }
            MenuAction::Properties(index) => self.show_properties(self.active_side, index, cx),
            MenuAction::NewFolder => {
                let side = self.active_side;
                self.open_dialog(DialogKind::Mkdir(side), "新建目录", "新建目录", cx);
            }
            MenuAction::Refresh => self.refresh_both(cx),
        }
        cx.notify();
    }

    fn open_dialog_for_row(&mut self, kind: DialogKind, cx: &mut Context<Self>) {
        let (side, index) = match kind {
            DialogKind::Rename(side, index) | DialogKind::Chmod(side, index) => (side, index),
            DialogKind::Mkdir(side) => (side, 0),
        };
        let Some(entry) = self.pane(side).rows.get(index).cloned() else {
            return;
        };
        match kind {
            DialogKind::Rename(..) => {
                self.open_dialog(kind, "重命名", &entry.name, cx);
            }
            DialogKind::Chmod(..) => {
                let mode = format!("{:o}", entry.mode & 0o7777);
                self.open_dialog(kind, "修改权限（八进制）", &mode, cx);
            }
            DialogKind::Mkdir(..) => {}
        }
    }

    fn open_dialog(
        &mut self,
        kind: DialogKind,
        title: &'static str,
        initial: &str,
        cx: &mut Context<Self>,
    ) {
        if title.starts_with("修改权限") && !self.active_side.is_remote() {
            self.set_status("本地文件权限修改暂不支持".into(), cx);
            return;
        }
        let content = initial.to_string();
        let input = cx.new(|cx| TextInput::new(cx).with_content(content.clone()));
        input.update(cx, |input, _cx| {
            input.selected_range = 0..input.content.len();
        });
        self.dialog = Some(DialogState { kind, title, input });
        cx.notify();
    }

    fn confirm_dialog(&mut self, cx: &mut Context<Self>) {
        let Some(dialog) = self.dialog.take() else {
            return;
        };
        let value = dialog.input.read(cx).content.to_string();
        let value = value.trim().to_string();
        if value.is_empty() {
            self.set_status("名称不能为空".into(), cx);
            return;
        }
        if value.contains('/') || value.contains('\\') || value == "." || value == ".." {
            self.set_status("名称包含非法字符".into(), cx);
            cx.notify();
            return;
        }

        match dialog.kind {
            DialogKind::Mkdir(side) => {
                if side.is_remote() {
                    let path = remote_join(&self.remote.path, &value);
                    let message = format!("已创建目录 {value}");
                    self.run_remote_mutation(message, "创建失败", cx, move |service| {
                        service.mkdir(&path)
                    });
                } else {
                    let path = PathBuf::from(&self.local.path).join(&value);
                    match std::fs::create_dir(&path) {
                        Ok(()) => self.set_status(format!("已创建目录 {value}"), cx),
                        Err(error) => self.set_status(format!("创建失败: {error}"), cx),
                    }
                    let current = self.pane(side).path.clone();
                    self.navigate(side, current, cx);
                }
            }
            DialogKind::Rename(side, index) => {
                let Some(entry) = self.pane(side).rows.get(index).cloned() else {
                    return;
                };
                if side.is_remote() {
                    let new_path = remote_join(&self.remote.path, &value);
                    let old_path = entry.path.clone();
                    let message = format!("已重命名为 {value}");
                    self.run_remote_mutation(message, "重命名失败", cx, move |service| {
                        service.rename(&old_path, &new_path)
                    });
                } else {
                    let new_path = PathBuf::from(&self.local.path)
                        .join(&value)
                        .to_string_lossy()
                        .into_owned();
                    match std::fs::rename(&entry.path, &new_path) {
                        Ok(()) => self.set_status(format!("已重命名为 {value}"), cx),
                        Err(error) => self.set_status(format!("重命名失败: {error}"), cx),
                    }
                    let current = self.pane(side).path.clone();
                    self.navigate(side, current, cx);
                }
            }
            DialogKind::Chmod(side, index) => {
                let Some(entry) = self.pane(side).rows.get(index).cloned() else {
                    return;
                };
                let mode = u32::from_str_radix(&value, 8).unwrap_or(0);
                if mode == 0 || mode > 0o7777 {
                    self.set_status("非法权限值（示例：644 / 755）".into(), cx);
                    return;
                }
                let path = entry.path.clone();
                let name = entry.name.clone();
                let message = format!("{name} 权限已改为 {mode:o}");
                self.run_remote_mutation(message, "修改权限失败", cx, move |service| {
                    service.chmod(&path, mode)
                });
            }
        }
        cx.notify();
    }

    fn perform_delete_row(&mut self, side: PaneSide, index: usize, cx: &mut Context<Self>) {
        let Some(entry) = self.pane(side).rows.get(index).cloned() else {
            return;
        };
        if side.is_remote() {
            let path = entry.path.clone();
            let name = entry.name.clone();
            let is_dir = entry.is_dir;
            let message = format!("已删除 {name}");
            self.run_remote_mutation(message, "删除失败", cx, move |service| {
                // 判空与删除放在同一个后台任务里，避免 UI 线程两次网络往返。
                if is_dir {
                    if !service.is_dir_empty(&path)? {
                        return Err(anyhow::anyhow!("目录非空，拒绝删除"));
                    }
                    service.remove_dir(&path)
                } else {
                    service.remove_file(&path)
                }
            });
        } else {
            let result = if entry.is_dir {
                std::fs::remove_dir(&entry.path).map_err(|error| anyhow::anyhow!(error))
            } else {
                std::fs::remove_file(&entry.path).map_err(|error| anyhow::anyhow!(error))
            };
            match result {
                Ok(()) => self.set_status(format!("已删除 {}", entry.name), cx),
                Err(error) => self.set_status(format!("删除失败: {error:#}"), cx),
            }
            let current = self.pane(side).path.clone();
            self.navigate(side, current, cx);
        }
        cx.notify();
    }

    /// 确认删除：执行真正的删除实现。
    fn confirm_pending_delete(&mut self, cx: &mut Context<Self>) {
        let Some((side, name)) = self.pending_delete.take() else {
            return;
        };
        // 列表可能在确认期间刷新；按文件名重新定位，找不到就放弃删除。
        if let Some(index) = self
            .pane(side)
            .rows
            .iter()
            .position(|entry| entry.name == name)
        {
            self.perform_delete_row(side, index, cx);
        } else {
            cx.notify();
        }
    }

    /// 确认卡片「继续 / 删除」。
    fn confirm_pending(&mut self, kind: ConfirmKind, cx: &mut Context<Self>) {
        match kind {
            ConfirmKind::DeleteRow => self.confirm_pending_delete(cx),
            ConfirmKind::Transfer => self.confirm_pending_transfer(cx),
        }
    }

    /// 确认卡片「取消」：清空待确认动作，不执行任何操作。
    fn cancel_pending(&mut self, kind: ConfirmKind, cx: &mut Context<Self>) {
        match kind {
            ConfirmKind::DeleteRow => self.pending_delete = None,
            ConfirmKind::Transfer => self.pending_transfer = None,
        }
        cx.notify();
    }

    fn show_properties(&mut self, side: PaneSide, index: usize, cx: &mut Context<Self>) {
        let Some(entry) = self.pane(side).rows.get(index).cloned() else {
            return;
        };
        let text = format!(
            "{} · {} · {} · {}",
            entry.path,
            format_size(entry.size),
            format_mode(&entry),
            format_time(entry.mtime)
        );
        self.set_status(text, cx);
        cx.notify();
    }

    fn set_status(&mut self, text: String, cx: &mut Context<Self>) {
        self.status_text = text.clone();
        self.emit(
            cx,
            AppEvent::StatusBarMessage {
                text,
                duration_ms: 3000,
            },
        );
    }

    fn handle_transfer_event(
        &mut self,
        generation: u64,
        event: TransferEvent,
        cx: &mut Context<Self>,
    ) {
        if generation != self.generation {
            return;
        }
        match &event {
            TransferEvent::Started {
                id,
                direction,
                remote_path,
                local_path,
            } => {
                self.emit(
                    cx,
                    AppEvent::FileTransferStarted {
                        id: *id,
                        remote_path: remote_path.clone(),
                        local_path: local_path.to_string_lossy().into_owned(),
                    },
                );
                self.status_text = format!("{}开始", direction.label());
            }
            TransferEvent::Progress {
                id,
                transferred,
                total,
            } => {
                self.emit(
                    cx,
                    AppEvent::FileTransferProgress {
                        id: *id,
                        transferred: *transferred,
                        total: *total,
                    },
                );
            }
            TransferEvent::Completed { id } => {
                self.emit(cx, AppEvent::FileTransferCompleted { id: *id });
                self.status_text = "传输完成".to_string();
                // 传输完成后刷新两侧目录（文件大小 / 列表变化）
                self.refresh_both(cx);
            }
            TransferEvent::Failed { id, reason } => {
                self.emit(
                    cx,
                    AppEvent::FileTransferFailed {
                        id: *id,
                        reason: reason.clone(),
                    },
                );
                self.status_text = format!("传输失败: {reason}");
            }
            TransferEvent::Cancelled { .. } => {
                self.status_text = "传输已取消".to_string();
            }
            TransferEvent::Paused { .. } => {
                self.status_text = "传输已暂停".to_string();
            }
            TransferEvent::Resumed { .. } => {
                self.status_text = "传输已恢复".to_string();
            }
        }
        if let Some(queue) = self.queue.as_ref() {
            self.transfers = queue.snapshots();
        }
        cx.notify();
    }

    fn handle_key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        match event.keystroke.key.as_str() {
            "enter" => {
                for side in [PaneSide::Local, PaneSide::Remote] {
                    let address = self.pane(side).address.clone();
                    let focused = address.read(cx).focus_handle(cx).is_focused(window);
                    if focused {
                        let text = address.read(cx).content.to_string();
                        self.navigate(side, text, cx);
                        break;
                    }
                }
            }
            "escape" => {
                let mut cleared = self.dialog.take().is_some();
                cleared |= self.pending_delete.take().is_some();
                cleared |= self.pending_transfer.take().is_some();
                cleared |= self.menu.take().is_some();
                if cleared {
                    cx.notify();
                }
            }
            _ => {}
        }
    }
}

impl Focusable for SftpView {
    fn focus_handle(&self, _cx: &gpui::App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

// ---------------------------------------------------------------------------
// 路径 / 本地文件工具
// ---------------------------------------------------------------------------

/// 规范化远端路径：
/// - `~` → 远端 home；`~/sub` → home/sub
/// - 无 home 时回退到当前目录（当前目录也为空则用 `/`）
/// - 绝对路径保持不变，相对路径基于当前目录
fn normalize_remote(raw: &str, current: &str, home: Option<&str>) -> String {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return current.to_string();
    }
    let home = home
        .filter(|home| !home.trim().is_empty())
        .unwrap_or_else(|| {
            if current.trim().is_empty() {
                "/"
            } else {
                current
            }
        });
    let expanded = if trimmed == "~" {
        home.to_string()
    } else if let Some(rest) = trimmed.strip_prefix("~/") {
        remote_join(home, rest)
    } else {
        trimmed.to_string()
    };
    if expanded.starts_with('/') {
        let mut path = expanded;
        while path.len() > 1 && path.ends_with('/') {
            path.pop();
        }
        path
    } else {
        remote_join(current, &expanded)
    }
}

fn normalize_local(raw: &str, current: &str) -> String {
    let trimmed = raw.trim();
    let path = if trimmed.is_empty() {
        PathBuf::from(current)
    } else {
        PathBuf::from(trimmed)
    };
    let path = if path.is_absolute() {
        path
    } else {
        PathBuf::from(current).join(path)
    };
    path.to_string_lossy().into_owned()
}

fn list_local(path: &str) -> Result<Vec<FileEntry>, String> {
    let dir = PathBuf::from(path);
    let read =
        std::fs::read_dir(&dir).map_err(|error| format!("无法读取 {}: {error}", dir.display()))?;
    let mut rows = Vec::new();
    for entry in read.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let file_type = entry.file_type().ok();
        let metadata = entry.metadata().ok();
        let is_symlink = file_type.map(|t| t.is_symlink()).unwrap_or(false);
        let is_dir = metadata.as_ref().map(|m| m.is_dir()).unwrap_or(false);
        let size = metadata.as_ref().map(|m| m.len()).unwrap_or(0);
        let mtime = metadata
            .as_ref()
            .and_then(|m| m.modified().ok())
            .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
            .map(|duration| duration.as_secs() as i64)
            .unwrap_or(0);
        let mode = local_mode(&metadata);
        rows.push(FileEntry {
            name,
            path: entry.path().to_string_lossy().into_owned(),
            size,
            mode,
            mtime,
            is_dir,
            is_symlink,
            symlink_target: None,
        });
    }
    Ok(rows)
}

#[cfg(unix)]
fn local_mode(metadata: &Option<std::fs::Metadata>) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    metadata
        .as_ref()
        .map(|m| m.permissions().mode() & 0o7777)
        .unwrap_or(0o644)
}

#[cfg(not(unix))]
fn local_mode(metadata: &Option<std::fs::Metadata>) -> u32 {
    metadata
        .as_ref()
        .map(|m| if m.is_dir() { 0o755 } else { 0o644 })
        .unwrap_or(0o644)
}

fn format_time(timestamp: i64) -> String {
    if timestamp <= 0 {
        return "-".to_string();
    }
    chrono::DateTime::from_timestamp(timestamp, 0)
        .map(|dt| {
            dt.with_timezone(&chrono::Local)
                .format("%Y-%m-%d %H:%M")
                .to_string()
        })
        .unwrap_or_else(|| "-".to_string())
}

fn local_breadcrumbs(path: &str) -> Vec<(String, String)> {
    let mut result = Vec::new();
    let mut current = PathBuf::new();
    for component in PathBuf::from(path).components() {
        current.push(component.as_os_str());
        let label = match component {
            std::path::Component::Prefix(prefix) => {
                prefix.as_os_str().to_string_lossy().into_owned()
            }
            std::path::Component::RootDir => "\\".to_string(),
            std::path::Component::CurDir => ".".to_string(),
            std::path::Component::ParentDir => "..".to_string(),
            std::path::Component::Normal(name) => name.to_string_lossy().into_owned(),
        };
        result.push((label, current.to_string_lossy().into_owned()));
    }
    result
}

fn remote_breadcrumbs(path: &str) -> Vec<(String, String)> {
    if path.is_empty() {
        return Vec::new();
    }
    let mut result = vec![("/".to_string(), "/".to_string())];
    let mut current = String::new();
    for segment in path.split('/').filter(|segment| !segment.is_empty()) {
        current.push('/');
        current.push_str(segment);
        result.push((segment.to_string(), current.clone()));
    }
    result
}

// ---------------------------------------------------------------------------
// Render
// ---------------------------------------------------------------------------

impl Render for SftpView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);

        // 地址栏获得焦点时，回车导航
        let focus_handle = self.focus_handle.clone();
        let root = div()
            .size_full()
            .bg(t.app_bg())
            .flex()
            .flex_col()
            .track_focus(&focus_handle)
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                this.handle_key(event, window, cx);
            }));

        // 顶部工具栏（Dock 模式由 TerminalTabView 提供开关，不重复占高度）
        let status_color = match self.status {
            SftpStatus::Connected => t.success(),
            SftpStatus::Failed => t.danger(),
            SftpStatus::Connecting => t.text_subtle(),
        };
        let header: Option<AnyElement> = (self.display_mode == SftpDisplayMode::Tab).then(|| {
            div()
                .w_full()
                .h(px(32.))
                .bg(t.toolbar_bg())
                .border_b_1()
                .border_color(t.border_subtle())
                .flex()
                .items_center()
                .px_3()
                .gap_3()
                .child(
                    div()
                        .text_size(px(12.))
                        .text_color(t.text_on_toolbar_bg())
                        .child("📁 SFTP 文件管理"),
                )
                .child(
                    div()
                        .text_size(px(11.))
                        .text_color(status_color)
                        .child(self.status_text.clone()),
                )
                .child(div().flex_1())
                .child(
                    div()
                        .h(px(22.))
                        .px_2()
                        .rounded_md()
                        .bg(t.secondary_btn_bg())
                        .text_size(px(11.))
                        .text_color(t.text_on_secondary_btn())
                        .cursor_pointer()
                        .id("sftp-mkdir")
                        .hover(|s| s.bg(t.secondary_btn_hover()))
                        .on_click(cx.listener(|this, _ev: &ClickEvent, _w, cx| {
                            let side = this.active_side;
                            this.open_dialog(DialogKind::Mkdir(side), "新建目录", "新建目录", cx);
                        }))
                        .child("＋ 目录"),
                )
                .child(
                    div()
                        .h(px(22.))
                        .px_2()
                        .rounded_md()
                        .bg(t.secondary_btn_bg())
                        .text_size(px(11.))
                        .text_color(t.text_on_secondary_btn())
                        .cursor_pointer()
                        .id("sftp-refresh-all")
                        .hover(|s| s.bg(t.secondary_btn_hover()))
                        .on_click(cx.listener(|this, _ev: &ClickEvent, _w, cx| {
                            this.refresh_both(cx);
                        }))
                        .child("⟳ 刷新"),
                )
                .into_any_element()
        });

        // 9.18：虚拟化所需可视区高度（只渲染屏幕内的行，1000 项目录不卡）
        let transfers_h = self.transfers.len().min(MAX_VISIBLE_TRANSFERS) as f32 * TRANSFER_ROW_H;
        let is_dock = self.display_mode == SftpDisplayMode::Dock;
        let error_h = if is_dock {
            if self.remote.error.is_some() {
                24.0
            } else {
                0.0
            }
        } else if self.local.error.is_some() || self.remote.error.is_some() {
            24.0
        } else {
            0.0
        };
        // Dock 模式没有 SftpView 自己的顶部工具条
        let toolbar_h = if is_dock { 0.0 } else { 32.0 };
        let viewport_h = f32::from(window.viewport_size().height);
        self.rows_view_h = (viewport_h
            - TOOLBAR_H
            - TAB_BAR_H
            - STATUS_BAR_H
            - toolbar_h
            - transfers_h
            - PANE_HEADER_H
            - 26.0
            - BREADCRUMB_H
            - COLUMN_HEADER_H
            - error_h
            - 2.0)
            .max(ROW_H);

        // S12 12.1：Dock 模式只显示远端栏；Tab 模式保持双栏
        let mut body = div()
            .w_full()
            .flex_1()
            .flex()
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _w, cx| {
                this.on_drag_move(event, cx);
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, event: &MouseUpEvent, window, cx| {
                    this.finish_drag(event, window, cx);
                }),
            );
        if self.display_mode == SftpDisplayMode::Dock {
            body = body.child(self.render_pane(PaneSide::Remote, cx));
        } else {
            body = body
                .child(self.render_pane(PaneSide::Local, cx))
                .child(self.render_pane(PaneSide::Remote, cx));
        }

        let mut content = root;
        if let Some(header) = header {
            content = content.child(header);
        }
        content = content.child(body).child(self.render_transfers(cx));

        if let Some(menu) = &self.menu {
            content = content.child(self.render_menu(menu, window, cx));
        }
        if let Some(dialog) = &self.dialog {
            let input_handle = dialog.input.read(cx).focus_handle(cx);
            if !input_handle.is_focused(window) {
                window.focus(&input_handle, cx);
            }
            content = content.child(self.render_dialog(cx));
        }

        // 删除 / 覆盖二次确认（S7.5.3.10）。
        if let Some((_, name)) = self.pending_delete.clone() {
            content = content.child(self.render_confirm_card(
                ConfirmCard {
                    message: format!("确定删除「{name}」？"),
                    detail: Some("删除后无法恢复".to_string()),
                    ok_label: "删除",
                    ok_id: "sftp-confirm-delete",
                    cancel_id: "sftp-confirm-delete-cancel",
                    kind: ConfirmKind::DeleteRow,
                    danger: true,
                },
                cx,
            ));
        }
        if let Some(pending) = &self.pending_transfer {
            let name = pending.entry.name.clone();
            content = content.child(self.render_confirm_card(
                ConfirmCard {
                    message: "目标已存在，是否覆盖？".to_string(),
                    detail: Some(name),
                    ok_label: "覆盖",
                    ok_id: "sftp-confirm-overwrite",
                    cancel_id: "sftp-confirm-overwrite-cancel",
                    kind: ConfirmKind::Transfer,
                    danger: false,
                },
                cx,
            ));
        }

        // 测量根元素在窗口中的位置：Dock 模式的右键菜单坐标依赖它换算
        let origin = self.root_bounds.clone();
        div()
            .size_full()
            .relative()
            .on_children_prepainted(move |bounds, _window, _cx| {
                if let Some(first) = bounds.first() {
                    origin.set(Some(*first));
                }
            })
            .child(content)
            .into_any_element()
    }
}

impl SftpView {
    fn render_pane(&self, side: PaneSide, cx: &mut Context<Self>) -> AnyElement {
        let t = theme(cx);
        let pane = self.pane(side);
        let active = self.active_side == side;
        let pane_bg = t.card_bg();
        let header_bg = t.card_header_bg();
        let is_dock = self.display_mode == SftpDisplayMode::Dock;
        // 面板可用宽度：优先用根元素实测值（Dock 模式宽度由父容器决定）
        let pane_width = match self.root_bounds.get() {
            Some(bounds) => {
                let width = f32::from(bounds.size.width);
                if is_dock {
                    width
                } else {
                    width / 2.0
                }
            }
            None => 640.0,
        };
        let columns = SftpColumns::for_width(pane_width);

        // 列头：与数据行使用同一组列宽 —— 图标定宽 + 名称填充 + 定宽列
        let mut header = div()
            .w_full()
            .h(px(COLUMN_HEADER_H))
            .bg(header_bg)
            .border_b_1()
            .border_color(t.border_subtle())
            .flex()
            .items_center()
            .px_2()
            .text_size(px(10.))
            .text_color(t.text_subtle())
            .child(div().w(px(COL_ICON_W)).flex_none())
            .child(self.render_sort_button("名称", SortKey::Name, None, side, cx))
            .child(self.render_sort_button("大小", SortKey::Size, Some(COL_SIZE_W), side, cx));
        if columns.mode {
            header = header.child(self.render_sort_button(
                "权限",
                SortKey::Mode,
                Some(COL_MODE_W),
                side,
                cx,
            ));
        }
        if columns.mtime {
            header = header.child(self.render_sort_button(
                "修改时间",
                SortKey::Modified,
                Some(COL_MTIME_W),
                side,
                cx,
            ));
        }

        // 9.18：虚拟化 —— 只渲染可视区内的行，并把 scroll 收进合法范围
        let visible = visible_row_count(self.rows_view_h);
        let start = pane.scroll.min(max_scroll_offset(pane.rows.len(), visible));
        let rows: Vec<AnyElement> = pane
            .rows
            .iter()
            .enumerate()
            .skip(start)
            .take(visible)
            .map(|(index, entry)| self.render_row(side, index, entry, columns, cx))
            .collect();

        let error = pane.error.clone().map(|error| {
            div()
                .w_full()
                .px_2()
                .py_1()
                .text_size(px(10.))
                .text_color(t.danger())
                .child(error)
        });

        div()
            // Dock 模式单栏占满侧栏，Tab 模式左右各半
            .w(gpui::relative(if is_dock { 1.0 } else { 0.5 }))
            .h_full()
            .bg(pane_bg)
            .border_r_1()
            .border_color(t.border_subtle())
            .flex()
            .flex_col()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _ev: &MouseDownEvent, _w, cx| {
                    this.active_side = side;
                    cx.notify();
                }),
            )
            .child(
                // 面板头：标题 + 项目数 + 上一级 + 刷新
                div()
                    .w_full()
                    .h(px(PANE_HEADER_H))
                    .bg(t.card_header_bg())
                    .border_b_1()
                    .border_color(t.border_subtle())
                    .flex()
                    .items_center()
                    .px_2()
                    .gap_2()
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(if active {
                                t.primary()
                            } else {
                                t.text_on_card_header_bg()
                            })
                            .child(side.label()),
                    )
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(t.text_subtle())
                            .child(format!("{} 项", pane.rows.len())),
                    )
                    .child(div().flex_1())
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(t.text_subtle())
                            .cursor_pointer()
                            .id(shared_id("sftp-up", side))
                            .hover(|s| s.text_color(t.primary()))
                            .on_click(cx.listener(move |this, _ev: &ClickEvent, _w, cx| {
                                let parent = if side.is_remote() {
                                    remote_parent(&this.remote.path)
                                } else {
                                    PathBuf::from(&this.local.path)
                                        .parent()
                                        .map(|p| p.to_string_lossy().into_owned())
                                        .unwrap_or_else(|| this.local.path.clone())
                                };
                                this.navigate(side, parent, cx);
                            }))
                            .child("↑"),
                    )
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(t.text_subtle())
                            .cursor_pointer()
                            .id(shared_id("sftp-refresh", side))
                            .hover(|s| s.text_color(t.primary()))
                            .on_click(cx.listener(move |this, _ev: &ClickEvent, _w, cx| {
                                let current = this.pane(side).path.clone();
                                this.navigate(side, current, cx);
                            }))
                            .child("⟳"),
                    ),
            )
            .child(
                // 地址栏：输入框 + 转到
                div()
                    .w_full()
                    .h(px(32.))
                    .bg(t.card_bg())
                    .flex()
                    .items_center()
                    .px_2()
                    .gap_1()
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .h_full()
                            .overflow_hidden()
                            .child(pane.address.clone()),
                    )
                    .child(
                        div()
                            .h(px(20.))
                            .px_2()
                            .rounded_md()
                            .bg(t.secondary_btn_bg())
                            .text_size(px(10.))
                            .text_color(t.text_on_secondary_btn())
                            .flex()
                            .items_center()
                            .cursor_pointer()
                            .id(shared_id("sftp-go", side))
                            .hover(|s| s.bg(t.secondary_btn_hover()))
                            .on_click(cx.listener(move |this, _ev: &ClickEvent, _w, cx| {
                                let path = this.pane(side).address.read(cx).content.to_string();
                                this.navigate(side, path, cx);
                            }))
                            .child("转到"),
                    ),
            )
            .child(self.render_breadcrumbs(side, cx))
            .child(header)
            .child(
                div()
                    .w_full()
                    .flex_1()
                    .overflow_hidden()
                    .flex()
                    .flex_col()
                    .on_scroll_wheel(cx.listener(move |this, event: &ScrollWheelEvent, _w, cx| {
                        this.on_pane_scroll(side, event, cx);
                    }))
                    // 空白区右键菜单（9.18）：新建目录 / 刷新
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                            let local = this.to_local(event.position);
                            this.open_menu(
                                side,
                                None,
                                f32::from(local.x).max(0.),
                                f32::from(local.y).max(0.),
                                window,
                                cx,
                            );
                        }),
                    )
                    .children(rows),
            )
            .children(error)
            .into_any_element()
    }

    fn render_breadcrumbs(&self, side: PaneSide, cx: &mut Context<Self>) -> AnyElement {
        let t = theme(cx);
        let crumbs = if side.is_remote() {
            remote_breadcrumbs(&self.remote.path)
        } else {
            local_breadcrumbs(&self.local.path)
        };
        let mut row = div()
            .w_full()
            .h(px(BREADCRUMB_H))
            .bg(t.card_bg())
            .border_b_1()
            .border_color(t.border_subtle())
            .flex()
            .items_center()
            .px_2()
            .gap_1()
            .text_size(px(10.));
        for (index, (label, path)) in crumbs.into_iter().enumerate() {
            if index > 0 {
                row = row.child(div().text_color(t.text_placeholder()).child("›"));
            }
            let id = format!("sftp-crumb-{}-{}-{index}", side_key(side), label);
            row = row.child(
                div()
                    .text_color(t.text_subtle())
                    .cursor_pointer()
                    .id(id)
                    .hover(|s| s.text_color(t.primary()))
                    .on_click(cx.listener(move |this, _ev: &ClickEvent, _w, cx| {
                        this.navigate(side, path.clone(), cx);
                    }))
                    .child(label),
            );
        }
        row.into_any_element()
    }

    /// 列头排序按钮。`width = None` 表示名称列：填充剩余宽度且禁止撑破容器。
    fn render_sort_button(
        &self,
        label: &'static str,
        key: SortKey,
        width: Option<f32>,
        side: PaneSide,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = theme(cx);
        let active = self.sort_key == key;
        let arrow = if active {
            if self.sort_ascending {
                " ↑"
            } else {
                " ↓"
            }
        } else {
            ""
        };
        let mut button = div()
            .h_full()
            .flex()
            .items_center()
            .overflow_hidden()
            .cursor_pointer()
            .id(format!("sftp-sort-{}-{}", side_key(side), label))
            .on_click(cx.listener(move |this, _ev: &ClickEvent, _w, cx| {
                this.sort_by(key, cx);
            }))
            .text_color(if active { t.primary() } else { t.text_subtle() });
        button = match width {
            Some(width) => button.w(px(width)).flex_none(),
            None => button.flex_1().min_w(px(0.)),
        };
        button.child(format!("{label}{arrow}")).into_any_element()
    }

    fn render_row(
        &self,
        side: PaneSide,
        index: usize,
        entry: &FileEntry,
        columns: SftpColumns,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = theme(cx);
        let selected = self.pane(side).selected == Some(index);
        let row_bg = if selected {
            t.row_selected_bg()
        } else {
            transparent_black().to_rgb()
        };
        let hover_bg = t.row_hover_bg();
        let text_color = if selected {
            t.text_on_row_selected_bg()
        } else {
            t.text_on_card_bg()
        };
        let icon = if entry.is_dir {
            "📁"
        } else if entry.is_symlink {
            "🔗"
        } else {
            "📄"
        };
        let name = if entry.is_symlink {
            match &entry.symlink_target {
                Some(target) => format!("{} → {target}", entry.name),
                None => entry.name.clone(),
            }
        } else {
            entry.name.clone()
        };

        let mut row = div()
            .w_full()
            .h(px(ROW_H))
            .flex()
            .items_center()
            .px_2()
            .bg(row_bg)
            .text_size(px(11.))
            .text_color(text_color)
            .cursor_pointer()
            .id(format!("sftp-row-{}-{index}", side_key(side)))
            .hover(move |s| s.bg(hover_bg))
            .on_click(cx.listener(move |this, _ev: &ClickEvent, _w, cx| {
                this.click_row(side, index, cx);
            }))
            // 9.18：左键按下记录拖拽起点，拖到另一侧面板即上传 / 下载
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event: &MouseDownEvent, _w, cx| {
                    this.begin_drag(side, index, event, cx);
                }),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, ev: &MouseDownEvent, window, cx| {
                    cx.stop_propagation();
                    let local = this.to_local(ev.position);
                    this.open_menu(
                        side,
                        Some(index),
                        f32::from(local.x).max(0.),
                        f32::from(local.y).max(0.),
                        window,
                        cx,
                    );
                }),
            )
            .child(div().w(px(COL_ICON_W)).flex_none().child(icon))
            // 名称列填充剩余宽度；min_w(0) 允许 truncate，不把定宽列挤出容器
            .child(div().flex_1().min_w(px(0.)).truncate().child(name))
            .child(
                div()
                    .w(px(COL_SIZE_W))
                    .flex_none()
                    .text_color(if selected {
                        text_color
                    } else {
                        t.text_subtle()
                    })
                    .child(if entry.is_dir {
                        "-".to_string()
                    } else {
                        format_size(entry.size)
                    }),
            );
        if columns.mode {
            row = row.child(
                div()
                    .w(px(COL_MODE_W))
                    .flex_none()
                    .text_color(if selected {
                        text_color
                    } else {
                        t.text_subtle()
                    })
                    .child(format_mode(entry)),
            );
        }
        if columns.mtime {
            row = row.child(
                div()
                    .w(px(COL_MTIME_W))
                    .flex_none()
                    .text_color(if selected {
                        text_color
                    } else {
                        t.text_subtle()
                    })
                    .child(format_time(entry.mtime)),
            );
        }
        row.into_any_element()
    }

    fn render_menu(
        &self,
        menu: &MenuState,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = theme(cx);
        let width = SFTP_MENU_W;
        let item_h = MENU_ITEM_H;
        let items = menu_items(menu);
        let height = items.len() as f32 * item_h + MENU_PADDING_Y;
        // 菜单限制在 SftpView 自身范围内（Dock 模式宽度远小于窗口）
        let (view_w, view_h) = match self.root_bounds.get() {
            Some(bounds) => (f32::from(bounds.size.width), f32::from(bounds.size.height)),
            None => {
                let viewport = window.viewport_size();
                (
                    f32::from(viewport.width) - SIDEBAR_W - CONTENT_BORDER_W,
                    f32::from(viewport.height) - TOOLBAR_H - TAB_BAR_H - STATUS_BAR_H,
                )
            }
        };
        let max_x = (view_w - width - 4.0).max(0.0);
        let max_y = (view_h - height - 4.0).max(0.0);
        let x = menu.x.min(max_x).max(0.0);
        let y = menu.y.min(max_y).max(0.0);

        let mut card = div()
            .absolute()
            .left(px(x))
            .top(px(y))
            .w(px(width))
            .py_1()
            .bg(t.card_bg())
            .border_1()
            .border_color(t.border_medium())
            .rounded_md()
            .flex()
            .flex_col()
            .id("sftp-context-menu")
            .on_mouse_down_out(
                cx.listener(|this: &mut Self, _ev: &MouseDownEvent, _w, cx| {
                    this.menu = None;
                    cx.notify();
                }),
            );

        for (index, (label, danger, action)) in items.into_iter().enumerate() {
            let color = if danger {
                t.danger()
            } else {
                t.text_on_card_bg()
            };
            let hover = t.row_hover_bg();
            card = card.child(
                div()
                    .w_full()
                    .h(px(item_h))
                    .px_3()
                    .flex()
                    .items_center()
                    .text_size(px(11.))
                    .text_color(color)
                    .cursor_pointer()
                    .id(format!("sftp-menu-{index}"))
                    .hover(move |s| s.bg(hover))
                    .on_click(cx.listener(move |this, _ev: &ClickEvent, _w, cx| {
                        this.handle_menu_action(action, cx);
                    }))
                    .child(label),
            );
        }
        card.into_any_element()
    }

    fn render_transfers(&self, cx: &mut Context<Self>) -> AnyElement {
        let t = theme(cx);
        if self.transfers.is_empty() {
            return div().into_any_element();
        }
        let recent: Vec<&TransferSnapshot> = self
            .transfers
            .iter()
            .rev()
            .take(MAX_VISIBLE_TRANSFERS)
            .collect();
        let mut panel = div()
            .w_full()
            .bg(t.card_bg())
            .border_t_1()
            .border_color(t.border_subtle())
            .flex()
            .flex_col();
        for snapshot in recent {
            let id = snapshot.id;
            let t = theme(cx);
            let name = PathBuf::from(&snapshot.local_path)
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| snapshot.remote_path.clone());
            let arrow = match snapshot.direction {
                TransferDirection::Upload => "⬆",
                TransferDirection::Download => "⬇",
            };
            let state_text = match snapshot.state {
                TransferState::Failed => {
                    if let Some(error) = &snapshot.error {
                        format!("失败: {error}")
                    } else {
                        "失败".to_string()
                    }
                }
                state => state.label().to_string(),
            };
            let state_color = match snapshot.state {
                TransferState::Completed => t.success(),
                TransferState::Failed | TransferState::Cancelled => t.danger(),
                TransferState::Paused => t.text_subtle(),
                _ => t.text_on_card_bg(),
            };
            let progress = snapshot.progress();
            let bar_bg = t.border_subtle();
            let bar_fg = if snapshot.state == TransferState::Failed {
                t.danger()
            } else {
                t.primary()
            };
            let active = snapshot.state.is_active();

            let mut row = div()
                .w_full()
                .h(px(TRANSFER_ROW_H))
                .px_2()
                .flex()
                .items_center()
                .gap_2()
                .text_size(px(10.))
                .child(
                    div()
                        .w(px(14.))
                        .flex_none()
                        .text_color(t.text_subtle())
                        .child(arrow),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.))
                        .max_w(px(160.))
                        .truncate()
                        .text_color(t.text_on_card_bg())
                        .child(name),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w(px(8.))
                        .h(px(6.))
                        .rounded_full()
                        .bg(bar_bg)
                        .child(
                            div()
                                .h_full()
                                .w(gpui::relative(progress))
                                .rounded_full()
                                .bg(bar_fg),
                        ),
                )
                .child(
                    div()
                        .w(px(90.))
                        .flex_none()
                        .truncate()
                        .text_color(state_color)
                        .child(state_text),
                );
            if active {
                row = row
                    .child(
                        div()
                            .flex_none()
                            .px_2()
                            .h(px(18.))
                            .rounded_md()
                            .bg(t.secondary_btn_bg())
                            .text_color(t.text_on_secondary_btn())
                            .cursor_pointer()
                            .id(format!("sftp-transfer-toggle-{id}"))
                            .hover(|s| s.bg(t.secondary_btn_hover()))
                            .on_click(cx.listener(move |this, _ev: &ClickEvent, _w, cx| {
                                if let Some(queue) = this.queue.clone() {
                                    let paused = this
                                        .transfers
                                        .iter()
                                        .find(|snapshot| snapshot.id == id)
                                        .map(|snapshot| snapshot.state == TransferState::Paused)
                                        .unwrap_or(false);
                                    if paused {
                                        queue.resume(id);
                                    } else {
                                        queue.pause(id);
                                    }
                                    this.transfers = queue.snapshots();
                                    cx.notify();
                                }
                            }))
                            .child(if snapshot.state == TransferState::Paused {
                                "继续"
                            } else {
                                "暂停"
                            }),
                    )
                    .child(
                        div()
                            .flex_none()
                            .px_2()
                            .h(px(18.))
                            .rounded_md()
                            .bg(t.secondary_btn_bg())
                            .text_color(t.danger())
                            .cursor_pointer()
                            .id(format!("sftp-transfer-cancel-{id}"))
                            .hover(|s| s.bg(t.secondary_btn_hover()))
                            .on_click(cx.listener(move |this, _ev: &ClickEvent, _w, cx| {
                                if let Some(queue) = this.queue.clone() {
                                    queue.cancel(id);
                                    this.transfers = queue.snapshots();
                                    cx.notify();
                                }
                            }))
                            .child("取消"),
                    );
            }
            panel = panel.child(row);
        }
        panel.into_any_element()
    }

    /// 弹窗宽度：不超过 SftpView 实测宽度（Dock 模式下远小于窗口）。
    fn overlay_width(&self, preferred: f32) -> f32 {
        let available = self
            .root_bounds
            .get()
            .map(|bounds| f32::from(bounds.size.width))
            .unwrap_or(preferred + 24.0);
        (available - 24.0).clamp(200.0, preferred)
    }

    fn render_dialog(&self, cx: &mut Context<Self>) -> AnyElement {
        let t = theme(cx);
        let Some(dialog) = &self.dialog else {
            return div().into_any_element();
        };
        let title = dialog.title;
        let input = dialog.input.clone();
        let width = self.overlay_width(420.0);
        div()
            .absolute()
            .inset_0()
            .bg(t.modal_overlay())
            .flex()
            .items_center()
            .justify_center()
            .child(
                div()
                    .w(px(width))
                    .bg(t.card_bg())
                    .border_1()
                    .border_color(t.border_medium())
                    .rounded_lg()
                    .p_4()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .child(
                        div()
                            .text_size(px(13.))
                            .text_color(t.text_on_card_bg())
                            .child(title),
                    )
                    .child(div().h(px(30.)).child(input))
                    .child(
                        div()
                            .flex()
                            .justify_end()
                            .gap_2()
                            .child(
                                div()
                                    .px_3()
                                    .h(px(26.))
                                    .rounded_md()
                                    .bg(t.secondary_btn_bg())
                                    .text_size(px(11.))
                                    .text_color(t.text_on_secondary_btn())
                                    .flex()
                                    .items_center()
                                    .cursor_pointer()
                                    .id("sftp-dialog-cancel")
                                    .hover(|s| s.bg(t.secondary_btn_hover()))
                                    .on_click(cx.listener(
                                        |this: &mut Self, _ev: &ClickEvent, _w, cx| {
                                            this.dialog = None;
                                            cx.notify();
                                        },
                                    ))
                                    .child("取消"),
                            )
                            .child(
                                div()
                                    .px_3()
                                    .h(px(26.))
                                    .rounded_md()
                                    .bg(t.primary())
                                    .text_size(px(11.))
                                    .text_color(t.text_on_primary_btn())
                                    .flex()
                                    .items_center()
                                    .cursor_pointer()
                                    .id("sftp-dialog-ok")
                                    .hover(|s| s.bg(t.primary_hover()))
                                    .on_click(cx.listener(
                                        |this: &mut Self, _ev: &ClickEvent, _w, cx| {
                                            this.confirm_dialog(cx);
                                        },
                                    ))
                                    .child("确定"),
                            ),
                    ),
            )
            .into_any_element()
    }

    /// 通用确认卡片：遮罩 + 居中卡片 + 取消/确认按钮。
    fn render_confirm_card(&self, card: ConfirmCard, cx: &mut Context<Self>) -> AnyElement {
        let t = theme(cx);
        let cancel_hover = t.secondary_btn_hover();
        let ok_bg = if card.danger { t.danger() } else { t.primary() };

        let mut body = div()
            .w(px(self.overlay_width(380.)))
            .bg(t.card_bg())
            .border_1()
            .border_color(t.border_medium())
            .rounded_lg()
            .p_4()
            .flex()
            .flex_col()
            .gap_3()
            .child(
                div()
                    .w_full()
                    .text_center()
                    .text_size(px(13.))
                    .text_color(t.text_on_card_bg())
                    .child(card.message),
            );
        if let Some(detail) = card.detail {
            body = body.child(
                div()
                    .w_full()
                    .text_center()
                    .text_size(px(11.))
                    .text_color(t.text_subtle())
                    .child(detail),
            );
        }
        body = body.child(
            div()
                .flex()
                .justify_center()
                .gap_2()
                .child(
                    div()
                        .px_3()
                        .h(px(26.))
                        .rounded_md()
                        .bg(t.secondary_btn_bg())
                        .text_size(px(11.))
                        .text_color(t.text_on_secondary_btn())
                        .flex()
                        .items_center()
                        .cursor_pointer()
                        .id(card.cancel_id)
                        .hover(move |s| s.bg(cancel_hover))
                        .on_click(cx.listener(move |this, _ev: &ClickEvent, _w, cx| {
                            this.cancel_pending(card.kind, cx);
                        }))
                        .child("取消"),
                )
                .child(
                    div()
                        .px_3()
                        .h(px(26.))
                        .rounded_md()
                        .bg(ok_bg)
                        .text_size(px(11.))
                        .text_color(t.text_on_primary_btn())
                        .flex()
                        .items_center()
                        .cursor_pointer()
                        .id(card.ok_id)
                        .on_click(cx.listener(move |this, _ev: &ClickEvent, _w, cx| {
                            this.confirm_pending(card.kind, cx);
                        }))
                        .child(card.ok_label),
                ),
        );

        div()
            .absolute()
            .inset_0()
            .bg(t.modal_overlay())
            .flex()
            .items_center()
            .justify_center()
            .child(body)
            .into_any_element()
    }
}

fn side_key(side: PaneSide) -> &'static str {
    match side {
        PaneSide::Local => "local",
        PaneSide::Remote => "remote",
    }
}

fn shared_id(prefix: &str, side: PaneSide) -> String {
    format!("{prefix}-{}", side_key(side))
}

/// 可视区内能完整显示的行数（至少 1 行，避免除零 / 空列表）。
fn visible_row_count(view_h: f32) -> usize {
    ((view_h / ROW_H).floor() as usize).max(1)
}

/// 列表最大滚动起点：最后一行恰好贴底。
fn max_scroll_offset(row_count: usize, visible: usize) -> usize {
    row_count.saturating_sub(visible)
}

fn menu_items(menu: &MenuState) -> Vec<(String, bool, MenuAction)> {
    if let Some(index) = menu.row {
        let mut items: Vec<(String, bool, MenuAction)> = Vec::new();
        if menu.side.is_remote() {
            items.push(("⬇ 下载到本地".into(), false, MenuAction::Download(index)));
        } else {
            items.push(("⬆ 上传到远端".into(), false, MenuAction::Upload(index)));
        }
        items.push(("✏ 重命名…".into(), false, MenuAction::Rename(index)));
        if menu.side.is_remote() {
            items.push(("🔒 修改权限…".into(), false, MenuAction::Chmod(index)));
        }
        items.push(("ℹ 属性".into(), false, MenuAction::Properties(index)));
        items.push(("🗑 删除".into(), true, MenuAction::Delete(index)));
        items
    } else {
        vec![
            ("📁 新建目录…".into(), false, MenuAction::NewFolder),
            ("⟳ 刷新".into(), false, MenuAction::Refresh),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::{max_scroll_offset, normalize_remote, visible_row_count, ROW_H};

    const HOME: Option<&str> = Some("/home/alice");

    #[test]
    fn virtualization_counts_visible_rows() {
        assert_eq!(visible_row_count(ROW_H * 10.0), 10);
        assert_eq!(visible_row_count(ROW_H * 10.5), 10);
        // 高度异常时也至少渲染一行
        assert_eq!(visible_row_count(0.0), 1);
        assert_eq!(visible_row_count(-5.0), 1);
    }

    #[test]
    fn scroll_offset_stops_at_last_page() {
        assert_eq!(max_scroll_offset(1000, 30), 970);
        assert_eq!(max_scroll_offset(10, 30), 0);
        assert_eq!(max_scroll_offset(0, 30), 0);
    }

    #[test]
    fn tilde_expands_to_home() {
        assert_eq!(normalize_remote("~", "/srv", HOME), "/home/alice");
        assert_eq!(normalize_remote(" ~ ", "/srv", HOME), "/home/alice");
    }

    #[test]
    fn tilde_prefix_expands_below_home() {
        assert_eq!(normalize_remote("~/tmp", "/srv", HOME), "/home/alice/tmp");
        assert_eq!(normalize_remote("~/a/b/", "/srv", HOME), "/home/alice/a/b");
    }

    #[test]
    fn absolute_path_is_unchanged() {
        assert_eq!(normalize_remote("/var/log", "/srv", HOME), "/var/log");
        // 仅去掉多余的尾部斜杠。
        assert_eq!(normalize_remote("/var/log/", "/srv", HOME), "/var/log");
    }

    #[test]
    fn relative_path_is_based_on_current_dir() {
        assert_eq!(normalize_remote("tmp", "/srv", HOME), "/srv/tmp");
        assert_eq!(normalize_remote("a/b", "/srv/", HOME), "/srv/a/b");
    }

    #[test]
    fn empty_input_keeps_current_dir() {
        assert_eq!(normalize_remote("   ", "/srv", HOME), "/srv");
    }

    #[test]
    fn missing_home_falls_back_to_current_dir() {
        assert_eq!(normalize_remote("~", "/srv", None), "/srv");
        assert_eq!(normalize_remote("~/tmp", "/srv", None), "/srv/tmp");
    }
}

//! SftpView — 左右双栏 SFTP 文件管理（Sprint 7）
//!
//! - 左：本地目录；右：远端目录
//! - 双击目录进入；双击远端文件下载 / 本地文件上传
//! - 列头排序、右键菜单（上传/下载/重命名/删除/权限/属性）
//! - 底部传输面板：进度、暂停/继续、取消
//! - 每个 SFTP Tab 独立 SSH 连接（不占用终端 Tab 的 PTY 会话）

use std::path::{Path as StdPath, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use futures::channel::mpsc::unbounded;
use futures::StreamExt;
use gpui::prelude::*;
use gpui::{
    div, px, rgba, AnyElement, ClickEvent, Context, Entity, FocusHandle, Focusable, IntoElement,
    KeyDownEvent, MouseButton, MouseDownEvent, Render, ScrollWheelEvent, SharedString, Window,
};
use uuid::Uuid;

use rterm_session::Session as SessionConfig;
use rterm_sftp::{
    format_mode, format_size, remote_join, remote_parent, sort_entries, FileEntry, SftpService,
    SortKey, TransferDirection, TransferEvent, TransferQueue, TransferSnapshot, TransferState,
};
use rterm_ssh::SshSession;

use crate::event::{AppEvent, EventBusHost};
use crate::state::AppState;
use crate::ui::theme;
use crate::views::text_input::TextInput;
use super::{CONTENT_BORDER_W, SIDEBAR_W, TAB_BAR_H, TOOLBAR_H};

const PANE_HEADER_H: f32 = 34.0;
const BREADCRUMB_H: f32 = 22.0;
const COLUMN_HEADER_H: f32 = 22.0;
const ROW_H: f32 = 24.0;
const TRANSFER_ROW_H: f32 = 30.0;
const MAX_VISIBLE_TRANSFERS: usize = 4;
const DOUBLE_CLICK_WINDOW: Duration = Duration::from_millis(400);
const REMOTE_PLACEHOLDER: &str = "远端未连接";

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

// ---------------------------------------------------------------------------
// SftpView
// ---------------------------------------------------------------------------

pub struct SftpView {
    focus_handle: FocusHandle,
    tab_id: Uuid,
    session_id: Option<Uuid>,
    session: Option<SessionConfig>,
    status: SftpStatus,
    status_text: String,
    service: Option<std::sync::Arc<SftpService>>,
    queue: Option<std::sync::Arc<TransferQueue>>,
    transfers: Vec<TransferSnapshot>,
    local: Pane,
    remote: Pane,
    active_side: PaneSide,
    sort_key: SortKey,
    sort_ascending: bool,
    menu: Option<MenuState>,
    dialog: Option<DialogState>,
    last_click: Option<(PaneSide, usize, Instant)>,
    generation: u64,
}

impl SftpView {
    pub fn new(cx: &mut Context<Self>) -> Self {
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
        let remote_address = cx.new(|cx| {
            TextInput::new(cx).with_placeholder("/home/user")
        });

        Self {
            focus_handle: cx.focus_handle(),
            tab_id: Uuid::new_v4(),
            session_id: None,
            session: None,
            status: SftpStatus::Connecting,
            status_text: "等待连接…".to_string(),
            service: None,
            queue: None,
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
            last_click: None,
            generation: 0,
        }
    }

    pub fn tab_id(&self) -> Uuid {
        self.tab_id
    }

    pub fn focus_handle(&self) -> FocusHandle {
        self.focus_handle.clone()
    }

    /// 打开 SFTP Tab：独立 SSH 连接 + SFTP 子系统。
    pub fn connect(&mut self, session_id: Uuid, cx: &mut Context<Self>) {
        let stored = {
            let state = cx.global::<AppState>();
            let store = state.session_store.lock().unwrap();
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
        let generation = self.generation;

        let (tx, mut rx) = unbounded::<Result<SftpService, String>>();
        std::thread::Builder::new()
            .name("rterm-sftp-connect".into())
            .spawn(move || {
                let mut ssh = SshSession::new();
                let result = ssh
                    .connect(&session)
                    .map_err(|error| format!("{error:#}"))
                    .and_then(|_| {
                        let raw = ssh
                            .session_clone()
                            .ok_or_else(|| "SSH 会话不可用".to_string())?;
                        SftpService::new(raw).map_err(|error| format!("{error:#}"))
                    });
                let _ = tx.unbounded_send(result);
            })
            .ok();

        cx.spawn(async move |this, cx| {
            if let Some(result) = rx.next().await {
                let _ = this.update(cx, |this, cx| {
                    this.handle_connected(generation, result, cx)
                });
            }
        })
        .detach();

        cx.notify();
    }

    /// 关闭 Tab 时调用。
    pub fn shutdown(&mut self, cx: &mut Context<Self>) {
        self.generation = self.generation.wrapping_add(1);
        if let Some(queue) = self.queue.take() {
            queue.shutdown();
        }
        self.service = None;
        self.status = SftpStatus::Failed;
        self.status_text = "已断开".to_string();
        self.transfers.clear();
        self.emit(cx, AppEvent::SftpClosed { tab_id: self.tab_id });
        cx.notify();
    }

    fn handle_connected(
        &mut self,
        generation: u64,
        result: Result<SftpService, String>,
        cx: &mut Context<Self>,
    ) {
        if generation != self.generation {
            return;
        }
        match result {
            Ok(service) => {
                let service = std::sync::Arc::new(service);
                let home = service.home_dir().unwrap_or_else(|_| "/".to_string());
                self.status = SftpStatus::Connected;
                self.status_text = format!("已连接 · {home}");
                self.service = Some(service.clone());

                let (events_tx, mut events_rx) = unbounded::<TransferEvent>();
                let queue = std::sync::Arc::new(TransferQueue::new(service.clone(), events_tx, 2));
                self.queue = Some(queue.clone());

                let generation = self.generation;
                cx.spawn(async move |this, cx| {
                    while let Some(event) = events_rx.next().await {
                        let alive = this
                            .update(cx, |this, cx| {
                                this.handle_transfer_event(generation, event, &queue, cx)
                            })
                            .is_ok();
                        if !alive {
                            break;
                        }
                    }
                })
                .detach();

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
        self.remote.error = Some(reason);
        self.emit(
            cx,
            AppEvent::StatusBarMessage {
                text: "SFTP 连接失败".into(),
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

    fn navigate(&mut self, side: PaneSide, raw_path: String, cx: &mut Context<Self>) {
        if side.is_remote() {
            let Some(service) = self.service.clone() else {
                self.remote.error = Some(REMOTE_PLACEHOLDER.to_string());
                cx.notify();
                return;
            };
            let path = normalize_remote(&raw_path, &self.remote.path);
            match service.listdir(&path) {
                Ok(mut rows) => {
                    sort_entries(&mut rows, self.sort_key, self.sort_ascending);
                    self.remote.rows = rows;
                    self.remote.path = path.clone();
                    self.remote.selected = None;
                    self.remote.scroll = 0;
                    self.remote.error = None;
                    self.set_address(PaneSide::Remote, &path, cx);
                    if let Some(session_id) = self.session_id {
                        self.emit(
                            cx,
                            AppEvent::SftpNavigated {
                                tab_id: self.tab_id,
                                session_id,
                                path,
                            },
                        );
                    }
                }
                Err(error) => {
                    self.remote.error = Some(format!("{error:#}"));
                }
            }
        } else {
            let path = normalize_local(&raw_path, &self.local.path);
            match list_local(&path) {
                Ok(mut rows) => {
                    sort_entries(&mut rows, self.sort_key, self.sort_ascending);
                    self.local.rows = rows;
                    self.local.path = path.clone();
                    self.local.selected = None;
                    self.local.scroll = 0;
                    self.local.error = None;
                    self.set_address(PaneSide::Local, &path, cx);
                }
                Err(error) => {
                    self.local.error = Some(error);
                }
            }
        }
        self.menu = None;
        cx.notify();
    }

    fn set_address(&mut self, side: PaneSide, path: &str, cx: &mut Context<Self>) {
        let address = self.pane(side).address.clone();
        let path = path.to_string();
        address.update(cx, |input, cx| {
            input.set_content(path, cx);
        });
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

    fn on_pane_scroll(
        &mut self,
        side: PaneSide,
        event: &ScrollWheelEvent,
        cx: &mut Context<Self>,
    ) {
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
        let pane = self.pane_mut(side);
        let next = (pane.scroll as i32 - lines).max(0) as usize;
        pane.scroll = next.min(pane.rows.len().saturating_sub(1));
        cx.notify();
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
                self.enqueue_download(&entry, &local_dir, cx);
            }
            PaneSide::Local => {
                let remote_dir = self.remote.path.clone();
                self.enqueue_upload(&entry, &remote_dir, cx);
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
        cx: &mut Context<Self>,
    ) {
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
                self.enqueue_download(&entry, &local_dir, cx);
            }
            MenuAction::Upload(index) => {
                let Some(entry) = self.local.rows.get(index).cloned() else {
                    return;
                };
                let remote_dir = self.remote.path.clone();
                self.enqueue_upload(&entry, &remote_dir, cx);
            }
            MenuAction::Rename(index) => self.open_dialog_for_row(DialogKind::Rename(self.active_side, index), cx),
            MenuAction::Delete(index) => self.delete_row(self.active_side, index, cx),
            MenuAction::Chmod(index) => self.open_dialog_for_row(DialogKind::Chmod(self.active_side, index), cx),
            MenuAction::Properties(index) => self.show_properties(self.active_side, index, cx),
            MenuAction::NewFolder => {
                let side = self.active_side;
                self.open_dialog(DialogKind::Mkdir(side), "新建文件夹", "新建文件夹", cx);
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
        let input = cx.new(|cx| {
            TextInput::new(cx).with_content(content.clone())
        });
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
                    if let Some(service) = self.service.clone() {
                        let path = remote_join(&self.remote.path, &value);
                        match service.mkdir(&path) {
                            Ok(()) => self.set_status(format!("已创建目录 {value}"), cx),
                            Err(error) => self.set_status(format!("创建失败: {error:#}"), cx),
                        }
                    }
                } else {
                    let path = PathBuf::from(&self.local.path).join(&value);
                    match std::fs::create_dir(&path) {
                        Ok(()) => self.set_status(format!("已创建目录 {value}"), cx),
                        Err(error) => self.set_status(format!("创建失败: {error}"), cx),
                    }
                }
                let current = self.pane(side).path.clone();
                self.navigate(side, current, cx);
            }
            DialogKind::Rename(side, index) => {
                let Some(entry) = self.pane(side).rows.get(index).cloned() else {
                    return;
                };
                let new_path = if side.is_remote() {
                    remote_join(&self.remote.path, &value)
                } else {
                    PathBuf::from(&self.local.path)
                        .join(&value)
                        .to_string_lossy()
                        .into_owned()
                };
                let result = if side.is_remote() {
                    self.service
                        .clone()
                        .map(|service| service.rename(&entry.path, &new_path))
                        .unwrap_or_else(|| Err(anyhow::anyhow!("远端未连接")))
                } else {
                    std::fs::rename(&entry.path, &new_path)
                        .map_err(|error| anyhow::anyhow!(error))
                };
                match result {
                    Ok(()) => self.set_status(format!("已重命名为 {value}"), cx),
                    Err(error) => self.set_status(format!("重命名失败: {error:#}"), cx),
                }
                let current = self.pane(side).path.clone();
                self.navigate(side, current, cx);
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
                if let Some(service) = self.service.clone() {
                    match service.chmod(&entry.path, mode) {
                        Ok(()) => self.set_status(
                            format!("{} 权限已改为 {mode:o}", entry.name),
                            cx,
                        ),
                        Err(error) => self.set_status(format!("修改权限失败: {error:#}"), cx),
                    }
                }
                let current = self.pane(side).path.clone();
                self.navigate(side, current, cx);
            }
        }
        cx.notify();
    }

    fn delete_row(&mut self, side: PaneSide, index: usize, cx: &mut Context<Self>) {
        let Some(entry) = self.pane(side).rows.get(index).cloned() else {
            return;
        };
        let result = if side.is_remote() {
            match self.service.clone() {
                Some(service) => {
                    if entry.is_dir {
                        match service.is_dir_empty(&entry.path) {
                            Ok(true) => service.remove_dir(&entry.path),
                            Ok(false) => Err(anyhow::anyhow!("目录非空，拒绝删除")),
                            Err(error) => Err(error),
                        }
                    } else {
                        service.remove_file(&entry.path)
                    }
                }
                None => Err(anyhow::anyhow!("远端未连接")),
            }
        } else if entry.is_dir {
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
        queue: &std::sync::Arc<TransferQueue>,
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
        self.transfers = queue.snapshots();
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
                if self.dialog.take().is_some() {
                    cx.notify();
                } else if self.menu.take().is_some() {
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

fn normalize_remote(raw: &str, current: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return current.to_string();
    }
    let expanded = if trimmed == "~" {
        current.to_string()
    } else if let Some(rest) = trimmed.strip_prefix("~/") {
        remote_join(current, rest)
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
    let read = std::fs::read_dir(&dir).map_err(|error| format!("无法读取 {}: {error}", dir.display()))?;
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
            std::path::Component::Prefix(prefix) => prefix.as_os_str().to_string_lossy().into_owned(),
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

        // 顶部工具栏
        let status_color = match self.status {
            SftpStatus::Connected => t.success(),
            SftpStatus::Failed => t.danger(),
            SftpStatus::Connecting => t.text_subtle(),
        };
        let header = div()
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
                        this.open_dialog(DialogKind::Mkdir(side), "新建文件夹", "新建文件夹", cx);
                    }))
                    .child("＋ 文件夹"),
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
            );

        let body = div()
            .w_full()
            .flex_1()
            .flex()
            .child(self.render_pane(
                PaneSide::Local,
                pane_width(window),
                name_width(window),
                cx,
            ))
            .child(self.render_pane(
                PaneSide::Remote,
                pane_width(window),
                name_width(window),
                cx,
            ));

        let mut content = root
            .child(header)
            .child(body)
            .child(self.render_transfers(cx));

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

        content
    }
}

impl SftpView {
    fn render_pane(
        &self,
        side: PaneSide,
        pane_w: f32,
        name_w: f32,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = theme(cx);
        let pane = self.pane(side);
        let active = self.active_side == side;
        let pane_bg = if active {
            t.card_bg()
        } else {
            t.card_bg()
        };
        let header_bg = t.card_header_bg();

        // 列头
        let header = div()
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
            .child(div().w(px(20.)))
            .child(self.render_sort_button("名称", SortKey::Name, name_w, side, cx))
            .child(self.render_sort_button("大小", SortKey::Size, 70.0, side, cx))
            .child(self.render_sort_button("权限", SortKey::Mode, 80.0, side, cx))
            .child(self.render_sort_button("修改时间", SortKey::Modified, 110.0, side, cx));

        let rows: Vec<AnyElement> = pane
            .rows
            .iter()
            .enumerate()
            .skip(pane.scroll)
            .map(|(index, entry)| self.render_row(side, index, entry, name_w, cx))
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
            .w(gpui::relative(0.5))
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
                            .text_color(if active { t.primary() } else { t.text_on_card_header_bg() })
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
                    .h(px(26.))
                    .bg(t.card_bg())
                    .flex()
                    .items_center()
                    .px_2()
                    .gap_1()
                    .child(
                        div()
                            .w(px((pane_w - 78.0).max(80.0)))
                            .h(px(24.))
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
                    .on_scroll_wheel(cx.listener(
                        move |this, event: &ScrollWheelEvent, _w, cx| {
                            this.on_pane_scroll(side, event, cx);
                        },
                    ))
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
                row = row.child(
                    div()
                        .text_color(t.text_placeholder())
                        .child("›"),
                );
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

    fn render_sort_button(
        &self,
        label: &'static str,
        key: SortKey,
        width: f32,
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
        div()
            .h_full()
            .w(px(width))
            .flex()
            .items_center()
            .cursor_pointer()
            .id(format!("sftp-sort-{}-{}", side_key(side), label))
            .on_click(cx.listener(move |this, _ev: &ClickEvent, _w, cx| {
                this.sort_by(key, cx);
            }))
            .text_color(if active { t.primary() } else { t.text_subtle() })
            .child(format!("{label}{arrow}"))
            .into_any_element()
    }

    fn render_row(
        &self,
        side: PaneSide,
        index: usize,
        entry: &FileEntry,
        name_w: f32,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = theme(cx);
        let selected = self.pane(side).selected == Some(index);
        let row_bg = if selected {
            t.row_selected_bg()
        } else {
            rgba(0x00000000)
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

        div()
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
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, ev: &MouseDownEvent, _w, cx| {
                    let x = f32::from(ev.position.x) - SIDEBAR_W - CONTENT_BORDER_W;
                    let y = f32::from(ev.position.y) - TOOLBAR_H - TAB_BAR_H;
                    this.open_menu(side, Some(index), x.max(0.), y.max(0.), cx);
                }),
            )
            .child(
                div()
                    .w(px(20.))
                    .child(icon),
            )
            .child(
                div()
                    .w(px(name_w))
                    .truncate()
                    .child(name),
            )
            .child(
                div()
                    .w(px(70.))
                    .text_color(if selected { text_color } else { t.text_subtle() })
                    .child(if entry.is_dir {
                        "-".to_string()
                    } else {
                        format_size(entry.size)
                    }),
            )
            .child(
                div()
                    .w(px(80.))
                    .text_color(if selected { text_color } else { t.text_subtle() })
                    .child(format_mode(entry)),
            )
            .child(
                div()
                    .w(px(110.))
                    .text_color(if selected { text_color } else { t.text_subtle() })
                    .child(format_time(entry.mtime)),
            )
            .into_any_element()
    }

    fn render_menu(
        &self,
        menu: &MenuState,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = theme(cx);
        let width = 190.0;
        let item_h = 28.0;
        let items = menu_items(menu);
        let height = items.len() as f32 * item_h + 8.0;
        let viewport = window.viewport_size();
        let max_x = (f32::from(viewport.width) - width - 4.0).max(0.0);
        let max_y = (f32::from(viewport.height) - height - 4.0).max(0.0);
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
            .on_mouse_down_out(cx.listener(
                |this: &mut Self, _ev: &MouseDownEvent, _w, cx| {
                    this.menu = None;
                    cx.notify();
                },
            ));

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
                        .text_color(t.text_subtle())
                        .child(arrow),
                )
                .child(
                    div()
                        .w(px(160.))
                        .overflow_hidden()
                        .text_color(t.text_on_card_bg())
                        .child(name),
                )
                .child(
                    div()
                        .flex_1()
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
                        .w(px(110.))
                        .text_color(state_color)
                        .child(state_text),
                );
            if active {
                row = row
                    .child(
                        div()
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
                                        .map(|snapshot| {
                                            snapshot.state == TransferState::Paused
                                        })
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

    fn render_dialog(&self, cx: &mut Context<Self>) -> AnyElement {
        let t = theme(cx);
        let Some(dialog) = &self.dialog else {
            return div().into_any_element();
        };
        let title = dialog.title;
        let input = dialog.input.clone();
        let width = 420.0;
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
                    .child(
                        div()
                            .h(px(30.))
                            .child(input),
                    )
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
}

fn side_key(side: PaneSide) -> &'static str {
    match side {
        PaneSide::Local => "local",
        PaneSide::Remote => "remote",
    }
}

/// 每个 pane 的宽度是内容区的一半。
fn pane_width(window: &Window) -> f32 {
    let viewport = window.viewport_size();
    (f32::from(viewport.width) - SIDEBAR_W - CONTENT_BORDER_W) / 2.0
}

fn name_width(window: &Window) -> f32 {
    (pane_width(window) - 16.0 - 20.0 - 70.0 - 80.0 - 110.0 - 8.0).max(60.0)
}

fn shared_id(prefix: &str, side: PaneSide) -> String {
    format!("{prefix}-{}", side_key(side))
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
            ("📁 新建文件夹…".into(), false, MenuAction::NewFolder),
            ("⟳ 刷新".into(), false, MenuAction::Refresh),
        ]
    }
}

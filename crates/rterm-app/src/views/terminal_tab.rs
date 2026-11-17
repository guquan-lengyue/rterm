//! TerminalTabView — 终端 + 右侧 SFTP 侧栏组合视图（S12 12.3）
//!
//! 职责边界：
//! - 终端渲染 / PTY / cwd 解析仍由 `TerminalView` 负责
//! - SFTP 浏览 / 传输由 `SftpView`（Dock 模式）负责
//! - 本视图只做布局（可拖拽分隔条）、显隐与生命周期编排
//!
//! 路径跟随（12.4）：订阅 `TerminalCwdHint`，按同步模式门控后驱动
//! `SftpView::navigate_to_remote`，并用 `expect_sftp_navigation` 防回环。

use gpui::prelude::*;
use gpui::{
    div, px, AnyElement, ClickEvent, Context, CursorStyle, Entity, IntoElement, MouseButton,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, Render, Window,
};
use rterm_sync::SyncMode;
use uuid::Uuid;

use crate::event::{AppEvent, EventBusHost};
use crate::state::{lock_or_recover, AppState};
use crate::ui::theme;

use super::sftp::{SftpDisplayMode, SftpView};
use super::terminal::TerminalView;

/// 侧栏宽度范围（12.7 默认值与当前宽度写入设置）
const DOCK_MIN_W: f32 = 280.0;
const DOCK_MAX_W: f32 = 720.0;
const DIVIDER_W: f32 = 6.0;

pub struct TerminalTabView {
    terminal: Entity<TerminalView>,
    /// 首次显示侧栏时才创建实例（避免未打开也占资源）。
    dock: Option<Entity<SftpView>>,
    dock_visible: bool,
    dock_width: f32,
    resizing: bool,
    resize_start_x: f32,
    resize_start_width: f32,
    session_id: Uuid,
    /// 终端最近一次 cwd：侧栏打开 / 重开时立即定位（12.4）。
    last_cwd: Option<String>,
}

impl TerminalTabView {
    /// 侧栏显隐与宽度来自全局设置（12.7）；`terminal_sftp_dock = true` 时立即创建并连接。
    pub fn new(cx: &mut Context<Self>, session_id: Uuid) -> Self {
        let (dock_visible, dock_width) = {
            let settings = &cx.global::<AppState>().settings;
            (
                settings.terminal_sftp_dock,
                settings
                    .terminal_sftp_dock_width
                    .clamp(DOCK_MIN_W, DOCK_MAX_W),
            )
        };
        let terminal = cx.new(TerminalView::new);
        terminal.update(cx, |view, cx| view.connect(session_id, cx));

        // 12.4：终端 cwd 跟随 + 侧栏连接完成后的补跳
        let host = cx.global::<AppState>().event_bus_host.clone();
        cx.subscribe(
            &host,
            |this: &mut Self, _host, event: &AppEvent, cx| match event {
                AppEvent::TerminalCwdHint { tab_id, cwd, .. } => {
                    this.on_terminal_cwd(*tab_id, cwd, cx);
                }
                AppEvent::SftpOpened { tab_id, .. } => {
                    this.on_dock_opened(*tab_id, cx);
                }
                _ => {}
            },
        )
        .detach();

        let mut this = Self {
            terminal,
            dock: None,
            dock_visible: false,
            dock_width,
            resizing: false,
            resize_start_x: 0.0,
            resize_start_width: 0.0,
            session_id,
            last_cwd: None,
        };
        if dock_visible {
            this.show_dock(cx);
        }
        this
    }

    /// MainWindow 通过它取终端实体（日志开关 / cwd 注入 / SyncBridge 等）。
    pub fn terminal(&self) -> Entity<TerminalView> {
        self.terminal.clone()
    }

    pub fn focus_handle(&self, cx: &gpui::App) -> gpui::FocusHandle {
        self.terminal.read(cx).focus_handle()
    }

    /// 终端主机密钥弹窗或侧栏内部弹窗打开时，屏蔽全局快捷键。
    pub fn blocks_global_shortcuts(&self, cx: &gpui::App) -> bool {
        let terminal_block = self.terminal.read(cx).has_host_key_prompt();
        let dock_block = self
            .dock
            .as_ref()
            .map(|dock| dock.read(cx).has_modal_dialog())
            .unwrap_or(false);
        terminal_block || dock_block
    }

    /// 关闭 Tab：终端断开 + 侧栏释放（含线程 / SFTP 连接）。
    pub fn close(&mut self, cx: &mut Context<Self>) {
        self.terminal.update(cx, |view, cx| view.disconnect(cx));
        if let Some(dock) = self.dock.take() {
            dock.update(cx, |view, cx| view.shutdown(cx));
        }
    }

    /// 开关 SFTP 侧栏（12.6 会补按钮与 Ctrl+Shift+S）。
    pub fn toggle_dock(&mut self, cx: &mut Context<Self>) {
        if self.dock_visible {
            self.dock_visible = false;
            cx.notify();
        } else {
            self.show_dock(cx);
        }
    }

    fn show_dock(&mut self, cx: &mut Context<Self>) {
        self.dock_visible = true;
        let existed = self.dock.is_some();
        if self.dock.is_none() {
            let width = self.dock_width;
            let session_id = self.session_id;
            let dock = cx.new(|cx| SftpView::new_with_mode(cx, SftpDisplayMode::Dock));
            dock.update(cx, |view, cx| {
                view.set_dock_width(width, cx);
                view.connect(session_id, cx);
            });
            self.dock = Some(dock);
        }
        // 已有连接（重复打开侧栏）：立即定位到终端当前 cwd
        if existed && terminal_to_sftp_enabled(cx) {
            if let Some(cwd) = self.last_cwd.clone() {
                self.navigate_dock(&cwd, cx);
            }
        }
        cx.notify();
    }

    // -----------------------------------------------------------------------
    // 12.4 路径跟随
    // -----------------------------------------------------------------------

    /// 终端 cwd 变化：先记录，侧栏可见且模式允许时驱动 SFTP 导航。
    fn on_terminal_cwd(&mut self, tab_id: Uuid, cwd: &str, cx: &mut Context<Self>) {
        if tab_id != self.terminal.read(cx).tab_id() || cwd.is_empty() {
            return;
        }
        self.last_cwd = Some(cwd.to_string());
        if !self.dock_visible || self.dock.is_none() || !terminal_to_sftp_enabled(cx) {
            return;
        }
        self.navigate_dock(cwd, cx);
    }

    /// 侧栏 SFTP 连接完成：登记远端 home（防初始化导航反向 cd），再补跳终端 cwd。
    fn on_dock_opened(&mut self, tab_id: Uuid, cx: &mut Context<Self>) {
        let Some(dock) = self.dock.clone() else {
            return;
        };
        if dock.read(cx).tab_id() != tab_id {
            return;
        }
        if let Some(home) = dock.read(cx).remote_home() {
            let bridge = self.terminal.read(cx).sync_bridge();
            let mut bridge = lock_or_recover(&bridge);
            bridge.set_remote_home(home.clone());
            bridge.mark_sftp_synced(std::path::Path::new(&home));
        }
        if !terminal_to_sftp_enabled(cx) {
            return;
        }
        if let Some(cwd) = self.last_cwd.clone() {
            self.navigate_dock(&cwd, cx);
        }
    }

    /// 驱动侧栏导航；先登记期望事件，避免 SftpNavigated 反向注入 `cd` 形成回环。
    fn navigate_dock(&mut self, path: &str, cx: &mut Context<Self>) {
        let Some(dock) = self.dock.clone() else {
            return;
        };
        let bridge = self.terminal.read(cx).sync_bridge();
        lock_or_recover(&bridge).expect_sftp_navigation(1);
        dock.update(cx, |view, cx| view.navigate_to_remote(path, cx));
    }

    fn on_divider_down(&mut self, event: &MouseDownEvent, cx: &mut Context<Self>) {
        self.resizing = true;
        self.resize_start_x = f32::from(event.position.x);
        self.resize_start_width = self.dock_width;
        cx.notify();
    }

    fn on_mouse_move(&mut self, event: &MouseMoveEvent, cx: &mut Context<Self>) {
        if !self.resizing {
            return;
        }
        let dx = f32::from(event.position.x) - self.resize_start_x;
        let next = (self.resize_start_width - dx).clamp(DOCK_MIN_W, DOCK_MAX_W);
        if (next - self.dock_width).abs() < 0.5 {
            return;
        }
        self.dock_width = next;
        if let Some(dock) = &self.dock {
            dock.update(cx, |view, cx| view.set_dock_width(next, cx));
        }
        cx.notify();
    }

    fn on_mouse_up(&mut self, cx: &mut Context<Self>) {
        if self.resizing {
            self.resizing = false;
            // 12.7：拖拽结束后记住宽度（新建终端 Tab 沿用）
            let width = self.dock_width;
            {
                let state = cx.global_mut::<AppState>();
                state.settings.terminal_sftp_dock_width = width;
                if let Err(error) = state.settings.save() {
                    log::warn!("[terminal-tab] 保存 SFTP 侧栏宽度失败: {error:#}");
                }
            }
            cx.notify();
        }
    }

    fn render_divider(&self, cx: &mut Context<Self>) -> AnyElement {
        let t = theme(cx);
        let color = if self.resizing {
            t.primary()
        } else {
            t.border_subtle()
        };
        div()
            .w(px(DIVIDER_W))
            .h_full()
            .flex_none()
            .cursor(CursorStyle::ResizeLeftRight)
            .bg(color)
            .id("terminal-sftp-divider")
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &MouseDownEvent, _w, cx| {
                    this.on_divider_down(event, cx);
                }),
            )
            .into_any_element()
    }
}

/// 同步模式是否允许「终端 → SFTP」方向（off / sftp_to_term 不跟随）。
fn terminal_to_sftp_enabled(cx: &mut Context<TerminalTabView>) -> bool {
    matches!(
        SyncMode::from_setting(&cx.global::<AppState>().settings.sftp_sync_mode),
        SyncMode::TerminalToSftp | SyncMode::Bidirectional
    )
}

impl Render for TerminalTabView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        let mut root = div()
            .size_full()
            .relative()
            .flex()
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _w, cx| {
                this.on_mouse_move(event, cx);
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _event: &MouseUpEvent, _w, cx| {
                    this.on_mouse_up(cx);
                }),
            )
            .child(
                div()
                    .flex_1()
                    .h_full()
                    .overflow_hidden()
                    .child(self.terminal.clone()),
            );

        if self.dock_visible {
            if let Some(dock) = &self.dock {
                root = root.child(self.render_divider(cx)).child(
                    div()
                        .w(px(self.dock_width))
                        .h_full()
                        .flex_none()
                        .overflow_hidden()
                        .child(dock.clone()),
                );
            }
        }

        // 临时开关（12.6 会替换为终端头部按钮 + Ctrl+Shift+S）
        root.child(
            div()
                .absolute()
                .top(px(3.))
                .right(px(8.))
                .h(px(18.))
                .px_2()
                .rounded_md()
                .bg(t.card_header_bg())
                .text_size(px(10.))
                .text_color(t.text_subtle())
                .cursor_pointer()
                .id("terminal-sftp-toggle")
                .hover(move |s| s.bg(t.secondary_btn_hover()))
                .on_click(cx.listener(|this, _ev: &ClickEvent, _w, cx| {
                    this.toggle_dock(cx);
                }))
                .child(if self.dock_visible {
                    "隐藏 SFTP"
                } else {
                    "SFTP"
                }),
        )
    }
}

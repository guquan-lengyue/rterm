//! TerminalTabView — 终端 + 右侧 SFTP 侧栏组合视图（S12 12.3）
//!
//! 职责边界：
//! - 终端渲染 / PTY / cwd 解析仍由 `TerminalView` 负责
//! - SFTP 浏览 / 传输由 `SftpView`（Dock 模式）负责
//! - 本视图只做布局（可拖拽分隔条）、显隐与生命周期编排
//!
//! 路径跟随（12.4）后续接入：订阅 `TerminalCwdHint` 后调用 `SftpView::navigate_to_remote`。

use gpui::prelude::*;
use gpui::{
    div, px, AnyElement, ClickEvent, Context, CursorStyle, Entity, IntoElement, MouseButton,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, Render, Window,
};
use uuid::Uuid;

use crate::ui::theme;

use super::sftp::{SftpDisplayMode, SftpView};
use super::terminal::TerminalView;

/// 侧栏宽度范围与默认值（12.7 会把默认值 / 当前宽度写入设置）
const DOCK_MIN_W: f32 = 280.0;
const DOCK_MAX_W: f32 = 720.0;
const DOCK_DEFAULT_W: f32 = 380.0;
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
}

impl TerminalTabView {
    /// `dock_visible = true` 时立即创建并连接 SFTP 侧栏（12.2 会改成首次显示再连接）。
    pub fn new(cx: &mut Context<Self>, session_id: Uuid, dock_visible: bool) -> Self {
        let terminal = cx.new(TerminalView::new);
        terminal.update(cx, |view, cx| view.connect(session_id, cx));

        let mut this = Self {
            terminal,
            dock: None,
            dock_visible: false,
            dock_width: DOCK_DEFAULT_W,
            resizing: false,
            resize_start_x: 0.0,
            resize_start_width: 0.0,
            session_id,
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
        cx.notify();
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

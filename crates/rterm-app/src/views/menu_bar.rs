//! menu_bar — 顶部常驻菜单栏（会话 / 连接 / 视图 / 帮助）
//!
//! 自绘菜单栏，保证 Windows / Linux / macOS 行为一致：
//! - 点击或 F10 展开；展开后悬停切换、Esc / 点击外部关闭
//! - 下拉菜单由 MainWindow 在根节点渲染（复用 `context_menu` 的菜单项组件）
//! - 动作统一通过 `AppEvent::MenuCommand` 分发

use gpui::prelude::*;
use gpui::{div, px, Context, FocusHandle, Focusable, IntoElement, KeyDownEvent, Render, Window};

use crate::event::{AppEvent, EventBusHost, MenuCommand};
use crate::state::AppState;
use crate::ui::theme;

/// 菜单栏高度（主窗口布局与下拉定位共用）。
pub const MENU_BAR_H: f32 = 26.0;
/// 单个菜单按钮宽度。
const MENU_BUTTON_W: f32 = 56.0;
/// 菜单按钮起始 x。
const MENU_BUTTON_START_X: f32 = 8.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuKind {
    Session,
    Connection,
    View,
    Help,
}

impl MenuKind {
    fn label(&self) -> &'static str {
        match self {
            Self::Session => "会话",
            Self::Connection => "连接",
            Self::View => "视图",
            Self::Help => "帮助",
        }
    }

    pub fn index(&self) -> usize {
        match self {
            Self::Session => 0,
            Self::Connection => 1,
            Self::View => 2,
            Self::Help => 3,
        }
    }

    pub fn from_index(index: usize) -> Self {
        match index % 4 {
            0 => Self::Session,
            1 => Self::Connection,
            2 => Self::View,
            _ => Self::Help,
        }
    }
}

/// 下拉菜单按钮的锚点 x（与菜单栏按钮布局一致）。
pub fn menu_anchor_x(kind: MenuKind) -> f32 {
    MENU_BUTTON_START_X + kind.index() as f32 * MENU_BUTTON_W
}

#[derive(Clone, Copy, Debug)]
pub struct MenuBarEntry {
    pub label: &'static str,
    pub command: MenuCommand,
    pub enabled: bool,
    pub divider_before: bool,
}

fn item(label: &'static str, command: MenuCommand) -> MenuBarEntry {
    MenuBarEntry {
        label,
        command,
        enabled: true,
        divider_before: false,
    }
}

fn disabled_item(label: &'static str, command: MenuCommand, enabled: bool) -> MenuBarEntry {
    MenuBarEntry {
        label,
        command,
        enabled,
        divider_before: false,
    }
}

fn divider(label: &'static str, command: MenuCommand) -> MenuBarEntry {
    MenuBarEntry {
        label,
        command,
        enabled: true,
        divider_before: true,
    }
}

/// 某菜单的下拉项。`has_selection` 控制「连接」类菜单是否可用。
pub fn entries(kind: MenuKind, has_selection: bool, sidebar_visible: bool) -> Vec<MenuBarEntry> {
    match kind {
        MenuKind::Session => vec![
            item("＋ 新建会话", MenuCommand::NewSession),
            item("📁 新建分组", MenuCommand::NewFolder),
            divider("⬆ 导入会话…", MenuCommand::ImportSessions),
            item("⬇ 导出会话…", MenuCommand::ExportSessions),
            divider("⚙ 设置…", MenuCommand::OpenSettings),
        ],
        MenuKind::Connection => vec![
            disabled_item("▶ 打开终端", MenuCommand::OpenTerminal, has_selection),
            disabled_item("📁 打开 SFTP", MenuCommand::OpenSftp, has_selection),
            disabled_item("📊 打开监控", MenuCommand::OpenMonitor, has_selection),
            disabled_item("🔀 端口转发…", MenuCommand::OpenPortForward, has_selection),
        ],
        MenuKind::View => vec![
            item(
                if sidebar_visible {
                    "隐藏会话侧栏"
                } else {
                    "显示会话侧栏"
                },
                MenuCommand::ToggleSidebar,
            ),
            divider("展开全部", MenuCommand::ExpandAll),
            item("折叠全部", MenuCommand::CollapseAll),
            divider("切换主题", MenuCommand::ToggleTheme),
            item("增大字号", MenuCommand::ZoomIn),
            item("减小字号", MenuCommand::ZoomOut),
        ],
        MenuKind::Help => vec![
            item("快捷键", MenuCommand::ShowShortcuts),
            item("打开日志目录", MenuCommand::OpenLogDir),
            divider("关于 rterm", MenuCommand::About),
        ],
    }
}

// ---------------------------------------------------------------------------
// MenuBarView
// ---------------------------------------------------------------------------

pub struct MenuBarView {
    open: Option<MenuKind>,
    has_selection: bool,
    sidebar_visible: bool,
    focus_handle: FocusHandle,
}

impl MenuBarView {
    pub fn new(cx: &mut Context<Self>) -> Self {
        Self {
            open: None,
            has_selection: false,
            sidebar_visible: true,
            focus_handle: cx.focus_handle(),
        }
    }

    pub fn open_kind(&self) -> Option<MenuKind> {
        self.open
    }

    pub fn set_state(
        &mut self,
        has_selection: bool,
        sidebar_visible: bool,
        cx: &mut Context<Self>,
    ) {
        if self.has_selection != has_selection || self.sidebar_visible != sidebar_visible {
            self.has_selection = has_selection;
            self.sidebar_visible = sidebar_visible;
            cx.notify();
        }
    }

    /// F10 / 菜单快捷键：展开第一个菜单，再按一次关闭。
    pub fn toggle_first(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open = match self.open {
            Some(MenuKind::Session) => None,
            _ => Some(MenuKind::Session),
        };
        if self.open.is_some() {
            window.focus(&self.focus_handle, cx);
        }
        cx.notify();
    }

    pub fn close(&mut self, cx: &mut Context<Self>) {
        if self.open.take().is_some() {
            cx.notify();
        }
    }

    fn toggle(&mut self, kind: MenuKind, window: &mut Window, cx: &mut Context<Self>) {
        self.open = if self.open == Some(kind) {
            None
        } else {
            Some(kind)
        };
        if self.open.is_some() {
            window.focus(&self.focus_handle, cx);
        }
        cx.notify();
    }
}

impl Focusable for MenuBarView {
    fn focus_handle(&self, _cx: &gpui::App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for MenuBarView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        let open = self.open;
        let mut bar = div()
            .w_full()
            .h(px(MENU_BAR_H))
            .flex_shrink_0()
            .bg(t.toolbar_bg())
            .border_b_1()
            .border_color(t.border_subtle())
            .flex()
            .items_center()
            .px(px(MENU_BUTTON_START_X))
            .track_focus(&self.focus_handle)
            .on_key_down(
                cx.listener(|this: &mut Self, event: &KeyDownEvent, _w, cx| {
                    if event.keystroke.key == "escape" || event.keystroke.key == "f10" {
                        this.open = None;
                        cx.notify();
                    }
                }),
            );

        for kind in [
            MenuKind::Session,
            MenuKind::Connection,
            MenuKind::View,
            MenuKind::Help,
        ] {
            let is_open = open == Some(kind);
            let bg = if is_open {
                t.row_selected_bg()
            } else {
                t.toolbar_bg()
            };
            let fg = if is_open {
                t.text_on_row_selected_bg()
            } else {
                t.text_on_toolbar_bg()
            };
            bar = bar.child(
                div()
                    .id(format!("rterm-menu-{}", kind.index()))
                    .w(px(MENU_BUTTON_W))
                    .h_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .bg(bg)
                    .text_color(fg)
                    .text_size(px(12.))
                    .cursor_pointer()
                    .hover(|style| style.bg(t.row_hover_bg()))
                    .on_click(cx.listener(
                        move |this: &mut Self, _ev: &gpui::ClickEvent, window, cx| {
                            this.toggle(kind, window, cx);
                        },
                    ))
                    .on_mouse_move(cx.listener(
                        move |this: &mut Self, _ev: &gpui::MouseMoveEvent, _w, cx| {
                            if this.open.is_some() && this.open != Some(kind) {
                                this.open = Some(kind);
                                cx.notify();
                            }
                        },
                    ))
                    .child(kind.label()),
            );
        }

        bar
    }
}

/// 广播菜单动作（MenuBarView 与 MainWindow 共用）。
pub fn emit_menu_command<T: 'static>(cx: &mut Context<T>, command: MenuCommand) {
    let host = cx.global::<AppState>().event_bus_host.clone();
    cx.update_entity(&host, |_host, cx: &mut Context<EventBusHost>| {
        cx.emit(AppEvent::MenuCommand(command));
    });
}

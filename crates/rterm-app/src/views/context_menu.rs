//! context_menu — 三类右键菜单（空白 / 分组 / 会话）的统一模型与渲染
//!
//! 职责边界：
//! - 本模块只定义「目标 → 菜单分区 / 动作」和菜单视觉组件
//! - 动作执行由 `MainWindow` 按动作枚举分派，避免全局功能混入单会话菜单
//!
//! 菜单项 / 分隔线 / 分区标题 / 位置夹取同时供顶部菜单栏复用。

use gpui::prelude::*;
use gpui::{div, px, AnyElement, Context, IntoElement, Rgba, SharedString, Window};
use uuid::Uuid;

use crate::ui::theme;

// ---------------------------------------------------------------------------
// 尺寸常量
// ---------------------------------------------------------------------------

/// 菜单行左右内边距：文字距菜单边缘的距离。
pub const MENU_ITEM_PAD_X: f32 = 10.0;
pub const MENU_ITEM_H: f32 = 30.0;
pub const MENU_SECTION_H: f32 = 20.0;
pub const MENU_DIVIDER_H: f32 = 9.0;
pub const MENU_PADDING_Y: f32 = 8.0;

// ---------------------------------------------------------------------------
// 目标 / 动作
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ContextMenuTarget {
    Blank,
    Folder(String),
    Session(Uuid),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BlankMenuAction {
    NewSession,
    NewFolder,
    ImportSessions,
    ExpandAll,
    CollapseAll,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FolderMenuAction {
    NewSessionInFolder,
    NewSubfolder,
    RenameFolder,
    DeleteFolder,
    ExportFolder,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SessionMenuAction {
    OpenTerminal,
    OpenSftp,
    OpenMonitor,
    OpenPortForward,
    EditSession,
    ChangeIcon,
    MoveToFolder,
    DuplicateSession,
    DeleteSession,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ContextMenuAction {
    Blank(BlankMenuAction),
    Folder(FolderMenuAction),
    Session(SessionMenuAction),
}

#[derive(Clone, Debug)]
pub struct ContextMenuState {
    pub target: ContextMenuTarget,
    pub x: i32,
    pub y: i32,
}

/// 单个菜单项（含分区标题与前置分隔线）。
#[derive(Clone, Debug)]
pub struct MenuEntry {
    pub id: String,
    pub label: String,
    pub action: ContextMenuAction,
    pub danger: bool,
    pub section: Option<&'static str>,
    pub divider_before: bool,
}

fn entry(
    id: &str,
    label: &str,
    action: ContextMenuAction,
    section: Option<&'static str>,
    divider_before: bool,
    danger: bool,
) -> MenuEntry {
    MenuEntry {
        id: id.to_string(),
        label: label.to_string(),
        action,
        danger,
        section,
        divider_before,
    }
}

/// 按右键目标生成菜单项（分区顺序即渲染顺序）。
pub fn entries_for(target: &ContextMenuTarget) -> Vec<MenuEntry> {
    match target {
        ContextMenuTarget::Blank => vec![
            entry(
                "ctx-blank-new-session",
                "＋ 新建会话",
                ContextMenuAction::Blank(BlankMenuAction::NewSession),
                Some("新建"),
                false,
                false,
            ),
            entry(
                "ctx-blank-new-folder",
                "📁 新建分组",
                ContextMenuAction::Blank(BlankMenuAction::NewFolder),
                None,
                false,
                false,
            ),
            entry(
                "ctx-blank-import",
                "⬆ 导入会话",
                ContextMenuAction::Blank(BlankMenuAction::ImportSessions),
                None,
                false,
                false,
            ),
            entry(
                "ctx-blank-expand-all",
                "展开全部",
                ContextMenuAction::Blank(BlankMenuAction::ExpandAll),
                Some("视图"),
                true,
                false,
            ),
            entry(
                "ctx-blank-collapse-all",
                "折叠全部",
                ContextMenuAction::Blank(BlankMenuAction::CollapseAll),
                None,
                false,
                false,
            ),
        ],
        ContextMenuTarget::Folder(_) => vec![
            entry(
                "ctx-folder-new-session",
                "＋ 在此分组新建会话",
                ContextMenuAction::Folder(FolderMenuAction::NewSessionInFolder),
                Some("新建"),
                false,
                false,
            ),
            entry(
                "ctx-folder-new-subfolder",
                "📁 新建子分组",
                ContextMenuAction::Folder(FolderMenuAction::NewSubfolder),
                None,
                false,
                false,
            ),
            entry(
                "ctx-folder-rename",
                "✏ 重命名分组",
                ContextMenuAction::Folder(FolderMenuAction::RenameFolder),
                Some("管理"),
                true,
                false,
            ),
            entry(
                "ctx-folder-delete",
                "🗑 删除分组",
                ContextMenuAction::Folder(FolderMenuAction::DeleteFolder),
                None,
                false,
                true,
            ),
            entry(
                "ctx-folder-export",
                "⬇ 导出该分组",
                ContextMenuAction::Folder(FolderMenuAction::ExportFolder),
                Some("传输"),
                true,
                false,
            ),
        ],
        ContextMenuTarget::Session(_) => vec![
            entry(
                "ctx-session-terminal",
                "▶ 打开终端",
                ContextMenuAction::Session(SessionMenuAction::OpenTerminal),
                Some("打开"),
                false,
                false,
            ),
            entry(
                "ctx-session-sftp",
                "📁 打开 SFTP",
                ContextMenuAction::Session(SessionMenuAction::OpenSftp),
                None,
                false,
                false,
            ),
            entry(
                "ctx-session-monitor",
                "📊 打开监控",
                ContextMenuAction::Session(SessionMenuAction::OpenMonitor),
                None,
                false,
                false,
            ),
            entry(
                "ctx-session-forward",
                "🔀 端口转发",
                ContextMenuAction::Session(SessionMenuAction::OpenPortForward),
                None,
                false,
                false,
            ),
            entry(
                "ctx-session-edit",
                "✏ 编辑会话",
                ContextMenuAction::Session(SessionMenuAction::EditSession),
                Some("编辑"),
                true,
                false,
            ),
            entry(
                "ctx-session-icon",
                "🎨 更改图标",
                ContextMenuAction::Session(SessionMenuAction::ChangeIcon),
                None,
                false,
                false,
            ),
            entry(
                "ctx-session-move",
                "📁 移动到分组",
                ContextMenuAction::Session(SessionMenuAction::MoveToFolder),
                None,
                false,
                false,
            ),
            entry(
                "ctx-session-duplicate",
                "⧉ 复制会话",
                ContextMenuAction::Session(SessionMenuAction::DuplicateSession),
                Some("管理"),
                true,
                false,
            ),
            entry(
                "ctx-session-delete",
                "🗑 删除会话",
                ContextMenuAction::Session(SessionMenuAction::DeleteSession),
                None,
                false,
                true,
            ),
        ],
    }
}

/// 菜单总高度（用于位置夹取）。
pub fn menu_height(entries: &[MenuEntry]) -> f32 {
    let rows: f32 = entries
        .iter()
        .map(|entry| {
            MENU_ITEM_H
                + if entry.section.is_some() {
                    MENU_SECTION_H
                } else {
                    0.0
                }
                + if entry.divider_before {
                    MENU_DIVIDER_H
                } else {
                    0.0
                }
        })
        .sum();
    rows + MENU_PADDING_Y
}

/// 按最宽条目文字计算菜单宽度：文字 + 左右内边距 + 边框。
pub fn menu_width(window: &mut Window, entries: &[MenuEntry]) -> f32 {
    menu_width_for(window, entries.iter().map(|entry| entry.label.as_str()))
}

/// 按一组标签文字计算菜单宽度（顶部菜单栏下拉复用）。
pub fn menu_width_for<'a>(window: &mut Window, labels: impl IntoIterator<Item = &'a str>) -> f32 {
    let widest = labels
        .into_iter()
        .map(|label| measure_text(window, label, 12.))
        .fold(0.0_f32, f32::max);
    widest + MENU_ITEM_PAD_X * 2. + 2.
}

/// 用窗口当前字体测量单行文字宽度。
fn measure_text(window: &mut Window, text: &str, font_size: f32) -> f32 {
    let run = window.text_style().to_run(text.len());
    let line = window.text_system().shape_line(
        SharedString::from(text.to_string()),
        px(font_size),
        &[run],
        None,
    );
    f32::from(line.width())
}

/// 位置夹取：菜单不越出窗口。
pub fn clamp_position(
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    viewport_w: f32,
    viewport_h: f32,
) -> (f32, f32) {
    (
        x.min(viewport_w - width).max(0.0),
        y.min(viewport_h - height).max(0.0),
    )
}

// ---------------------------------------------------------------------------
// 共享视觉组件
// ---------------------------------------------------------------------------

/// 分区标题行。
pub fn menu_section_label(label: &str, text_color: Rgba) -> AnyElement {
    div()
        .w_full()
        .h(px(MENU_SECTION_H))
        .px(px(MENU_ITEM_PAD_X))
        .flex()
        .items_center()
        .text_size(px(9.))
        .text_color(text_color)
        .child(label.to_string())
        .into_any_element()
}

/// 菜单分隔线。
pub fn menu_divider(border_color: Rgba) -> AnyElement {
    div()
        .w_full()
        .h(px(MENU_DIVIDER_H))
        .flex()
        .items_center()
        .px(px(MENU_ITEM_PAD_X))
        .child(div().w_full().h(px(1.)).bg(border_color))
        .into_any_element()
}

/// 单个菜单项（右键菜单与顶部菜单栏共用）。
pub fn menu_item<W: 'static>(
    id: &str,
    label: &str,
    enabled: bool,
    danger: bool,
    cx: &mut Context<W>,
    on_click: impl Fn(&mut W, &mut Context<W>) + Clone + 'static,
) -> AnyElement {
    let t = theme(cx);
    let label = label.to_string();
    let text_color = if !enabled {
        t.text_placeholder()
    } else if danger {
        t.danger()
    } else {
        t.text_on_card_bg()
    };
    let hover_bg = t.row_hover_bg();

    let mut item = div()
        .w_full()
        .h(px(MENU_ITEM_H))
        .px(px(MENU_ITEM_PAD_X))
        .flex()
        .items_center()
        .text_size(px(12.))
        .text_color(text_color)
        .id(id.to_string());

    if enabled {
        item = item
            .cursor_pointer()
            .hover(move |style| style.bg(hover_bg))
            .on_click(cx.listener(move |this, _ev: &gpui::ClickEvent, _w, cx| {
                on_click(this, cx);
            }));
    }

    item.child(label).into_any_element()
}

// ---------------------------------------------------------------------------
// 右键菜单渲染
// ---------------------------------------------------------------------------

/// 渲染完整右键菜单（含分区、分隔线、位置夹取、点击外部关闭）。
pub fn render_context_menu<W: 'static>(
    window: &mut Window,
    state: &ContextMenuState,
    viewport_w: f32,
    viewport_h: f32,
    cx: &mut Context<W>,
    on_action: impl Fn(&mut W, ContextMenuAction, &mut Context<W>) + Clone + 'static,
    on_close: impl Fn(&mut W, &mut Context<W>) + Clone + 'static,
) -> AnyElement {
    let t = theme(cx);
    let entries = entries_for(&state.target);
    let height = menu_height(&entries);
    let width = menu_width(window, &entries);
    let (x, y) = clamp_position(
        state.x as f32,
        state.y as f32,
        width,
        height,
        viewport_w,
        viewport_h,
    );

    let close_handler = on_close;
    let mut menu = div()
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
        .id("rterm-context-menu")
        .on_mouse_down_out(
            cx.listener(move |this, _ev: &gpui::MouseDownEvent, _w, cx| {
                close_handler(this, cx);
            }),
        );

    for entry in &entries {
        if let Some(section) = entry.section {
            menu = menu.child(menu_section_label(section, t.text_subtle()));
        }
        if entry.divider_before {
            menu = menu.child(menu_divider(t.border_subtle()));
        }
        let action = entry.action.clone();
        let handler = on_action.clone();
        menu = menu.child(menu_item(
            &entry.id,
            &entry.label,
            true,
            entry.danger,
            cx,
            move |this, cx| {
                handler(this, action.clone(), cx);
            },
        ));
    }

    let _ = close_handler;
    menu.into_any_element()
}

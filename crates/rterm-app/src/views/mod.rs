//! views/ — gpui UI 组件
//!
//! 每个 View 实现 `gpui::Render` trait。View 层不阻塞调用远端 Service：
//! 订阅 EventBus 里的 AppEvent 或等待后台任务结果，更新内部状态 → `cx.notify()` 触发重绘。
//! 已知例外：SFTP 本地文件系统操作仍在 UI 线程同步执行（远端 I/O 已后台化）。

pub mod command_palette;
pub mod import_export;
pub mod main_window;
pub mod monitor;
pub mod scroll_list;
pub mod session_dialog;
pub mod session_tree;
pub mod settings;
pub mod sftp;
pub mod status_bar;
pub mod tab_bar;
pub mod terminal;
pub mod terminal_tab;
pub mod text_input;

// ---------------------------------------------------------------------------
// 固定布局尺寸（MainWindow / TabBar / TerminalView 共用，避免 resize 计算出错）
// ---------------------------------------------------------------------------

pub(crate) const SIDEBAR_W: f32 = 240.0;
/// 侧边栏右边框
pub(crate) const CONTENT_BORDER_W: f32 = 1.0;
pub(crate) const TOOLBAR_H: f32 = 36.0;
pub(crate) const STATUS_BAR_H: f32 = 28.0;
pub(crate) const TAB_BAR_H: f32 = 30.0;
pub(crate) const TERMINAL_HEADER_H: f32 = 26.0;

// 右键菜单（MainWindow 会话菜单 / TabBar 标签菜单 / SFTP 菜单，数值保持原样）
pub(crate) const CONTEXT_MENU_W: f32 = 170.0;
pub(crate) const CONTEXT_MENU_ITEM_H: f32 = 30.0;
pub(crate) const TAB_MENU_W: f32 = 150.0;
pub(crate) const SFTP_MENU_W: f32 = 190.0;
/// TabBar / SFTP 右键菜单行高
pub(crate) const MENU_ITEM_H: f32 = 28.0;
/// 菜单容器上下 `py_1` 合计
pub(crate) const MENU_PADDING_Y: f32 = 8.0;

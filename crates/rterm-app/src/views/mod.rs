//! views/ — gpui UI 组件
//!
//! 每个 View 实现 `gpui::Render` trait。View 层**不直接调用 Service**，
//! 只订阅 EventBus 里的 AppEvent，收到后更新内部状态 → `cx.notify()` 触发重绘。

pub mod main_window;
pub mod session_tree;
pub mod session_dialog;
pub mod settings;
pub mod import_export;
pub mod status_bar;
pub mod tab_bar;
pub mod terminal;
pub mod text_input;

// ---------------------------------------------------------------------------
// 固定布局尺寸（MainWindow / TabBar / TerminalView 共用，避免 resize 计算出错）
// ---------------------------------------------------------------------------

pub const SIDEBAR_W: f32 = 240.0;
/// 侧边栏右边框
pub const CONTENT_BORDER_W: f32 = 1.0;
pub const TOOLBAR_H: f32 = 36.0;
pub const STATUS_BAR_H: f32 = 28.0;
pub const TAB_BAR_H: f32 = 30.0;
pub const TERMINAL_HEADER_H: f32 = 26.0;

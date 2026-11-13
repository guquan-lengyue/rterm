//! views/ — gpui UI 组件
//!
//! 每个 View 实现 `gpui::Render` trait。View 层**不直接调用 Service**，
//! 只订阅 EventBus 里的 AppEvent，收到后更新内部状态 → `cx.notify()` 触发重绘。

pub mod main_window;
pub mod session_tree;
pub mod status_bar;

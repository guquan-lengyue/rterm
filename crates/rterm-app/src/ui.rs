//! ui.rs — 便捷 helper：从 AppState 取主题
//!
//! 主题通过 Arc 共享，clone 一份成本可忽略。
//! 所有 view 用 `use crate::ui::theme; let t = theme(cx); t.app_bg()`。

use std::sync::Arc;

use gpui::Context;

use crate::state::AppState;
use crate::theme::Theme;

/// 从 Context 拿到主题 Arc。clone 一份成本很低（Arc 内部是原子计数），
/// 而且避免了 `&mut Context` 借用冲突——你可以继续用 `cx.subscribe()` / `cx.global()`。
pub fn theme<T>(cx: &mut Context<T>) -> Arc<dyn Theme> {
    cx.global::<AppState>().theme.clone()
}

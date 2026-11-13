//! MainWindow — 应用顶层三栏布局
//!
//! ```
//! ┌────────────┬───────────────────────────┐
//! │ SessionTree│     Content Area          │
//! │  (240px)   │   (Terminal / SFTP / ...) │
//! ├────────────┴───────────────────────────┤
//! │            Status Bar                   │
//! └─────────────────────────────────────────┘
//! ```
//!
//! bezel-gpui 0.3 里，只有 Entity<V: Render> 才能 `.child()`。
//! 我们在 main.rs 的 open_window 闭包里创建 SessionTree / StatusBar Entity，
//! 然后存到 MainWindow 的 fields 里，render 时直接 `.child(entity)`。

use gpui::prelude::*;
use gpui::{div, px, rgb, Entity, IntoElement, Render, Window};

use super::session_tree::SessionTreeView;
use super::status_bar::StatusBar;

pub struct MainWindow {
    session_tree: Entity<SessionTreeView>,
    status_bar: Entity<StatusBar>,
}

impl MainWindow {
    pub fn new(session_tree: Entity<SessionTreeView>, status_bar: Entity<StatusBar>) -> Self {
        Self { session_tree, status_bar }
    }
}

impl Render for MainWindow {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .bg(rgb(0x1e1e1e))
            .flex()
            .flex_col()
            // 上半：左 SessionTree + 右 Content 占位
            .child(
                div()
                    .w_full()
                    .flex_1()
                    .flex()
                    .child(self.session_tree.clone()),
            )
            // 底部 StatusBar
            .child(self.status_bar.clone())
    }
}

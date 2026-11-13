//! SessionTreeView — 左侧会话树（占位阶段）
//!
//! Sprint 1：显示占位文字
//! Sprint 3：替换为真实树形

use gpui::prelude::*;
use gpui::{div, px, rgb, IntoElement, Render, Window};

pub struct SessionTreeView;

impl Render for SessionTreeView {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .w(px(240.))
            .size_full()
            .bg(rgb(0x252526))
            .flex()
            .flex_col()
            .child(
                div()
                    .w_full()
                    .px_3()
                    .py_2()
                    .text_color(rgb(0xcccccc))
                    .text_size(px(12.))
                    .child("Session Tree"),
            )
            .child(
                div()
                    .w_full()
                    .flex_1()
                    .px_3()
                    .py_2()
                    .text_color(rgb(0x808080))
                    .text_size(px(11.))
                    .child("(Sprint 3 — 会话树、右键菜单、双击打开)"),
            )
    }
}

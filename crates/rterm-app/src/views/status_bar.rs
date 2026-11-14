//! StatusBar — 底部状态栏
//!
//! 视觉：品牌色背景 + 白色文字，水平居中。

use gpui::prelude::*;
use gpui::{div, px, IntoElement, Render, Window};

use crate::ui::theme;

pub struct StatusBar {
    pub message: String,
}

impl StatusBar {
    pub fn new() -> Self {
        Self { message: "Ready — rterm Sprint 3".into() }
    }
}

impl Render for StatusBar {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        div()
            .w_full()
            .h(px(28.))
            .bg(t.primary())
            .flex()
            .items_center()
            .justify_center()
            .px_3()
            .text_color(t.text_on_primary_btn())
            .text_size(px(11.))
            .child(self.message.clone())
    }
}

impl Default for StatusBar {
    fn default() -> Self { Self::new() }
}

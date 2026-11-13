//! StatusBar — 底部状态栏（占位阶段）

use gpui::prelude::*;
use gpui::{div, px, rgb, IntoElement, Render, Window};

pub struct StatusBar {
    pub message: String,
}

impl StatusBar {
    pub fn new() -> Self {
        Self { message: "Ready — rterm Sprint 1".into() }
    }
}

impl Render for StatusBar {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .w_full()
            .h(px(28.))
            .bg(rgb(0x007acc))
            .flex()
            .items_center()
            .px_3()
            .text_color(rgb(0xffffff))
            .text_size(px(11.))
            .child(self.message.clone())
    }
}

impl Default for StatusBar {
    fn default() -> Self { Self::new() }
}

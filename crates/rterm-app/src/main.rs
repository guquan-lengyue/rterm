// rterm-app main.rs — minimal GPUI empty window skeleton (bezel-gpui 0.3).

use gpui::prelude::*;
use gpui::{App, AppContext, Context, Render, Window, WindowOptions, div, px, rgb};
use gpui_platform;

struct RtermApp;

impl Render for RtermApp {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .bg(rgb(0x1e1e1e))
            .child(
                div()
                    .size_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_color(rgb(0xcccccc))
                    .text_size(px(18.))
                    .child("rterm — SSH Terminal (gpui skeleton OK)"),
            )
    }
}

fn main() {
    env_logger::init();

    gpui_platform::application().run(|app: &mut App| {
        app.open_window(WindowOptions::default(), |_window, app| {
            app.new(|_cx| RtermApp)
        });
    });
}

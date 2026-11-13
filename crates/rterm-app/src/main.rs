// rterm-app main.rs — Sprint 1 骨架

mod event;
mod state;
mod views;

use gpui::prelude::*;
use gpui::{App, WindowOptions};
use gpui_platform;

use state::AppState;
use views::main_window::MainWindow;
use views::session_tree::SessionTreeView;
use views::status_bar::StatusBar;

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    log::info!("rterm-app starting (Sprint 1 skeleton)");

    gpui_platform::application().run(|app: &mut App| {
        app.set_global(AppState::new());

        app.open_window(WindowOptions::default(), |_window, app| {
            let session_tree = app.new(|_cx| SessionTreeView);
            let status_bar = app.new(|_cx| StatusBar::new());
            app.new(|_cx| MainWindow::new(session_tree, status_bar))
        });
    });
}

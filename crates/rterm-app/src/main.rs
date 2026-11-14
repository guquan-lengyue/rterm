// rterm-app main.rs — Sprint 3 版
//
// gpui 0.3 EventBus 设计（重要）：
//   EventEmitter 只能挂在 Entity 上。EventBusHost 是空 View struct，
//   只用来承载 EventEmitter<AppEvent>。所有 View subscribe 它；
//   emit 必须通过 app.update_entity(&host, |_, cx| cx.emit(event))

pub mod event;
pub mod state;
pub mod theme;
pub mod ui;
pub mod views;

use gpui::prelude::*;
use gpui::{App, WindowOptions};
use gpui_platform;

use state::AppState;
use views::main_window::MainWindow;
use views::session_tree::SessionTreeView;
use views::status_bar::StatusBar;

// ---------- 进程内静态：退出保存 store ----------
use std::sync::{Mutex, OnceLock};
static STORE_FOR_EXIT: OnceLock<Mutex<Option<rterm_session::SessionStore>>> = OnceLock::new();

// ---------- 启动路径 ----------
fn sessions_path() -> std::path::PathBuf {
    let mut p = dirs::data_dir().unwrap_or_else(|| std::path::PathBuf::from("."));
    p.push("rterm");
    p.push("sessions.json");
    p
}

// ---------- 入口 ----------
fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    log::info!("rterm-app starting (Sprint 3 — SessionTree + 持久化)");

    // 1. 启动加载 store（DPAPI 解密）
    let (store, path) = load_store();
    STORE_FOR_EXIT.set(Mutex::new(Some(store.clone()))).ok();

    // 2. gpui run —— 先 new EventBusHost Entity，再 set_global AppState
    gpui_platform::application().run(move |app: &mut App| {
        let event_bus_host = app.new(|_cx| event::EventBusHost);
        let app_state = AppState::new(
            event_bus_host,
            store,
            std::sync::Arc::new(theme::default_theme()),
        );
        app.set_global(app_state);

        app.open_window(WindowOptions::default(), |_window, app| {
            let session_tree = app.new(|_cx| SessionTreeView::new());
            let status_bar = app.new(|_cx| StatusBar::new());
            app.new(|_cx| MainWindow::new(session_tree, status_bar))
        }).unwrap();
    });

    // 3. 退出保存（DPAPI 加密 + 原子写）
    save_store(&path);
}

// ---------- 辅助 ----------

fn load_store() -> (rterm_session::SessionStore, std::path::PathBuf) {
    let path = sessions_path();
    log::info!("Loading sessions from: {}", path.display());

    let store = rterm_session::SessionStore::load(&path, |s| {
        rterm_session::crypto_bridge::decrypt_session_dpapi(s)
    })
    .unwrap_or_else(|e| {
        log::error!("加载会话失败（返回空 store）: {e:#}");
        rterm_session::SessionStore::new()
    });

    log::info!("Loaded {} sessions", store.count());
    (store, path)
}

fn save_store(path: &std::path::Path) {
    let Some(mutex) = STORE_FOR_EXIT.get() else {
        log::warn!("Exit store never set");
        return;
    };
    let mut store = match mutex.lock() {
        Ok(mut g) => match g.take() {
            Some(s) => s,
            None => {
                log::warn!("Exit store already consumed");
                return;
            }
        },
        Err(e) => {
            log::error!("Exit store lock poisoned: {e}");
            return;
        }
    };

    log::info!("Saving {} sessions to: {}", store.count(), path.display());
    match store.save(path, |s| rterm_session::crypto_bridge::encrypt_session_dpapi(s)) {
        Ok(_) => log::info!("Saved OK"),
        Err(e) => log::error!("保存会话失败: {e:#}"),
    }
}

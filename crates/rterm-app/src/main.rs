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
use gpui::{App, KeyBinding, WindowOptions};
use gpui_platform;

use state::AppState;
use views::main_window::MainWindow;
use views::session_tree::SessionTreeView;
use views::status_bar::StatusBar;

use std::sync::{Arc, Mutex};

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

    // 1. 启动加载 store
    let store = load_store();
    let path = sessions_path();

    // 用 Arc<Mutex<>> 包装，AppState 和退出保存共享同一份。
    // SessionEditDialog.save() 修改时写入 AppState.session_store，
    // 退出时 save_store() 从同一个 Arc 读出——两边永远一致。
    let shared_store: Arc<Mutex<rterm_session::SessionStore>> =
        Arc::new(Mutex::new(store));

    let shared_store_for_app = shared_store.clone();

    // 2. gpui run
    gpui_platform::application().run(move |app: &mut App| {
        let event_bus_host = app.new(|_cx| event::EventBusHost);

        // AppState.session_store 需要 SessionStore（不是 MutexGuard），
        // 所以这里 clone 一份 Arc。SessionEditDialog 通过 global_mut 写，
        // 而退出时 save_store 从同一份 Arc 读。
        let app_state = AppState::new(
            event_bus_host,
            shared_store_for_app,
            std::sync::Arc::new(theme::default_theme()),
        );
        app.set_global(app_state);

        // TextInput 键盘编辑绑定：Backspace/Delete 等按键走 action 派发，
        // 不是 IME 文本输入，必须显式 bind。
        app.bind_keys([
            KeyBinding::new("backspace", views::text_input::Backspace, None),
            KeyBinding::new("delete", views::text_input::Delete, None),
            KeyBinding::new("left", views::text_input::Left, None),
            KeyBinding::new("right", views::text_input::Right, None),
            KeyBinding::new("home", views::text_input::Home, None),
            KeyBinding::new("end", views::text_input::End, None),
            KeyBinding::new("cmd-a", views::text_input::SelectAll, None),
        ]);

        app.open_window(WindowOptions::default(), |_window, app| {
            let session_tree = app.new(|_cx| SessionTreeView::new());
            let status_bar = app.new(|_cx| StatusBar::new());
            app.new(|_cx| MainWindow::new(session_tree, status_bar))
        }).unwrap();
    });

    // 3. 退出保存 —— 从共享 Arc 读，一定是最新状态
    save_store(&path, &shared_store);
}

// ---------- 辅助 ----------

fn load_store() -> rterm_session::SessionStore {
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
    store
}

fn save_store(
    path: &std::path::Path,
    shared: &Arc<Mutex<rterm_session::SessionStore>>,
) {
    let mut store = match shared.lock() {
        Ok(g) => g.clone(),
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

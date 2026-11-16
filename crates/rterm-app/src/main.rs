// rterm-app main.rs — Sprint 3 版
//
// gpui 0.3 EventBus 设计（重要）：
//   EventEmitter 只能挂在 Entity 上。EventBusHost 是空 View struct，
//   只用来承载 EventEmitter<AppEvent>。所有 View subscribe 它；
//   emit 必须通过 app.update_entity(&host, |_, cx| cx.emit(event))

pub mod event;
pub mod single_instance;
pub mod state;
pub mod theme;
pub mod ui;
pub mod views;

use gpui::prelude::*;
use gpui::{App, KeyBinding, WindowHandle, WindowOptions};
use gpui_platform;

use state::AppState;
use views::main_window::MainWindow;

use std::sync::{Arc, Mutex};

// ---------- 入口 ----------
fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    log::info!("rterm-app starting (Sprint 7 — SFTP 文件管理)");

    // 0. 单实例保护：已有实例在跑就直接退出。
    //    guard 必须持有到 main 结束，否则互斥体会提前释放。
    let _instance_guard = match single_instance::acquire() {
        Some(guard) => guard,
        None => {
            log::warn!("已有 rterm 实例在运行，本次启动退出");
            return;
        }
    };

    // 1. 启动加载 store + settings
    let (store, startup_error, loaded_ok) = load_store();
    let settings = state::Settings::load();

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
            settings,
            startup_error,
        );
        app.set_global(app_state);

        // TextInput 键盘编辑绑定：Backspace/Delete 等按键走 action 派发，
        // 不是 IME 文本输入，必须显式 bind。
        let mut bindings = vec![
            KeyBinding::new("backspace", views::text_input::Backspace, Some("TextInput")),
            KeyBinding::new("delete", views::text_input::Delete, Some("TextInput")),
            KeyBinding::new("left", views::text_input::Left, Some("TextInput")),
            KeyBinding::new("right", views::text_input::Right, Some("TextInput")),
            KeyBinding::new("home", views::text_input::Home, Some("TextInput")),
            KeyBinding::new("end", views::text_input::End, Some("TextInput")),
            KeyBinding::new("cmd-a", views::text_input::SelectAll, Some("TextInput")),
            KeyBinding::new("ctrl-a", views::text_input::SelectAll, Some("TextInput")),
            KeyBinding::new("cmd-v", views::text_input::Paste, Some("TextInput")),
            KeyBinding::new("ctrl-v", views::text_input::Paste, Some("TextInput")),
        ];
        bindings.extend(views::settings::key_bindings());
        bindings.extend(views::main_window::key_bindings());
        app.bind_keys(bindings);

        let window_handle = app.open_window(WindowOptions::default(), |_window, app| {
            app.new(|cx| MainWindow::new(cx))
        }).unwrap();

        // Ctrl+, → 全局打开设置。
        // 不能挂根节点的 on_action：窗口没有聚焦元素时，action 只在
        // dispatch tree 的根节点冒泡，不会进入控件树。App::on_action 是
        // 窗口级监听，任何焦点状态都能收到。
        let window_id = window_handle.window_id();
        app.on_action(move |_: &views::settings::OpenSettings, app: &mut App| {
            // action 派发期间窗口被临时移出 App.windows，直接 update 会
            // 报 “window not found”，因此延迟到当前事件处理完成后再打开。
            app.defer(move |app| {
                let handle = WindowHandle::<MainWindow>::new(window_id);
                if let Err(e) = handle.update(app, |main, _window, cx| main.open_settings(cx)) {
                    log::error!("[shortcut] 打开设置失败: {e:#}");
                }
            });
        });

        // Ctrl+T / Ctrl+Tab / Ctrl+W → Tab 管理
        app.on_action(move |_: &views::main_window::NewTab, app: &mut App| {
            app.defer(move |app| {
                let handle = WindowHandle::<MainWindow>::new(window_id);
                if let Err(e) = handle.update(app, |main, _window, cx| main.new_tab_shortcut(cx)) {
                    log::error!("[shortcut] 新建标签失败: {e:#}");
                }
            });
        });
        app.on_action(move |_: &views::main_window::NextTab, app: &mut App| {
            app.defer(move |app| {
                let handle = WindowHandle::<MainWindow>::new(window_id);
                if let Err(e) = handle.update(app, |main, _window, cx| main.next_tab_shortcut(cx)) {
                    log::error!("[shortcut] 切换标签失败: {e:#}");
                }
            });
        });
        app.on_action(move |_: &views::main_window::CloseTab, app: &mut App| {
            app.defer(move |app| {
                let handle = WindowHandle::<MainWindow>::new(window_id);
                if let Err(e) = handle.update(app, |main, _window, cx| main.close_tab_shortcut(cx)) {
                    log::error!("[shortcut] 关闭标签失败: {e:#}");
                }
            });
        });
    });

    // 3. 退出保存 —— 从共享 Arc 读，一定是最新状态
    //    启动加载失败时绝不写盘，避免空库覆盖原文件。
    if loaded_ok {
        save_store(&shared_store);
    } else {
        log::warn!("启动加载失败，退出时不写 sessions.json，避免覆盖原文件");
    }
}

// ---------- 辅助 ----------

/// 启动加载会话。
/// 返回 (store, 启动错误提示, 是否成功加载)。
/// 加载失败返回空 store + 错误提示 + false；单条解密失败仍算成功（true）。
fn load_store() -> (rterm_session::SessionStore, Option<String>, bool) {
    let path = state::sessions_path();
    log::info!("Loading sessions from: {}", path.display());

    match rterm_session::SessionStore::load_lossy(&path, |s| {
        rterm_session::crypto_bridge::decrypt_session_dpapi(s)
    }) {
        Ok((store, issues)) => {
            log::info!("Loaded {} sessions ({} issues)", store.count(), issues.len());
            let startup_error = if issues.is_empty() {
                None
            } else {
                Some(decrypt_issue_message(&issues))
            };
            (store, startup_error, true)
        }
        Err(e) => {
            log::error!("加载会话失败（返回空 store）: {e:#}");
            let backup = backup_corrupt_sessions(&path);
            let msg = match backup {
                Some(p) => format!("加载会话失败：{e:#}。原文件已备份到 {}", p.display()),
                None => format!("加载会话失败：{e:#}"),
            };
            (rterm_session::SessionStore::new(), Some(msg), false)
        }
    }
}

/// 单条会话解密失败的启动提示：最多列 2 个会话名。
fn decrypt_issue_message(issues: &[rterm_session::LoadIssue]) -> String {
    let names: Vec<String> = issues
        .iter()
        .take(2)
        .map(|i| {
            if i.name.is_empty() {
                i.id.to_string()
            } else {
                i.name.clone()
            }
        })
        .collect();
    let mut msg = format!("{} 条会话无法解密：{}", issues.len(), names.join("、"));
    if issues.len() > 2 {
        msg.push_str(" 等");
    }
    msg
}

/// 加载失败时把原文件复制为 `sessions.corrupt-YYYYMMDD-HHMMSS.json`。
/// 只备份、不移动/不覆盖原文件；失败仅记日志。
fn backup_corrupt_sessions(path: &std::path::Path) -> Option<std::path::PathBuf> {
    let ts = chrono::Local::now().format("%Y%m%d-%H%M%S");
    let backup = path.with_file_name(format!("sessions.corrupt-{ts}.json"));
    match std::fs::copy(path, &backup) {
        Ok(_) => {
            log::warn!("损坏的会话文件已备份到 {}", backup.display());
            Some(backup)
        }
        Err(e) => {
            log::error!("备份损坏会话文件失败（原文件保持不动）: {e}");
            None
        }
    }
}

/// 退出兜底保存：复用 state::persist_sessions。
fn save_store(shared: &Arc<Mutex<rterm_session::SessionStore>>) {
    let count = match shared.lock() {
        Ok(g) => g.count(),
        Err(e) => {
            log::error!("Exit store lock poisoned: {e}");
            return;
        }
    };

    log::info!(
        "Saving {} sessions to: {}",
        count,
        state::sessions_path().display()
    );
    match state::persist_sessions(shared) {
        Ok(()) => log::info!("Saved OK"),
        Err(e) => log::error!("保存会话失败: {e:#}"),
    }
}

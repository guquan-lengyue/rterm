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

use state::AppState;
use views::main_window::MainWindow;

use std::sync::{Arc, Mutex};

// ---------- 入口 ----------
fn main() {
    install_logger();
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

    // 1.1 每日备份：距上次备份超过 24h 才真正复制，保留最近 7 份。
    if loaded_ok {
        match rterm_session::backup::ensure_daily_backup(
            &state::sessions_path(),
            std::time::SystemTime::now(),
        ) {
            Ok(Some(path)) => log::info!("[backup] 已生成会话备份: {}", path.display()),
            Ok(None) => {}
            Err(error) => log::warn!("[backup] 生成会话备份失败: {error:#}"),
        }
    }

    // 用 Arc<Mutex<>> 包装，AppState 和退出保存共享同一份。
    // SessionEditDialog.save() 修改时写入 AppState.session_store，
    // 退出时 save_store() 从同一个 Arc 读出——两边永远一致。
    let shared_store: Arc<Mutex<rterm_session::SessionStore>> = Arc::new(Mutex::new(store));

    let shared_store_for_app = shared_store.clone();

    // 1.5 panic hook：异常退出前尽量把会话落盘。
    //     必须在共享 store 构造之后安装，且用 try_lock —— panic 可能发生在
    //     已持有 store 锁的代码路径里，再 lock() 会直接死锁。
    install_panic_hook(shared_store.clone(), loaded_ok);

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

        let window_handle = app
            .open_window(WindowOptions::default(), |_window, app| {
                app.new(MainWindow::new)
            })
            .unwrap();

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
                if let Err(e) = handle.update(app, |main, _window, cx| main.close_tab_shortcut(cx))
                {
                    log::error!("[shortcut] 关闭标签失败: {e:#}");
                }
            });
        });

        // Ctrl+Shift+N → 新建会话
        app.on_action(move |_: &views::main_window::NewSession, app: &mut App| {
            app.defer(move |app| {
                let handle = WindowHandle::<MainWindow>::new(window_id);
                if let Err(e) =
                    handle.update(app, |main, _window, cx| main.new_session_shortcut(cx))
                {
                    log::error!("[shortcut] 新建会话失败: {e:#}");
                }
            });
        });

        // F11 → 切换全屏
        app.on_action(
            move |_: &views::main_window::ToggleFullscreen, app: &mut App| {
                app.defer(move |app| {
                    let handle = WindowHandle::<MainWindow>::new(window_id);
                    if let Err(e) =
                        handle.update(app, |_main, window, _cx| window.toggle_fullscreen())
                    {
                        log::error!("[shortcut] 切换全屏失败: {e:#}");
                    }
                });
            },
        );

        // Ctrl+Shift+L → 开关当前终端会话日志
        app.on_action(
            move |_: &views::main_window::ToggleSessionLog, app: &mut App| {
                app.defer(move |app| {
                    let handle = WindowHandle::<MainWindow>::new(window_id);
                    if let Err(e) = handle.update(app, |main, _window, cx| {
                        main.toggle_session_log_shortcut(cx)
                    }) {
                        log::error!("[shortcut] 切换会话日志失败: {e:#}");
                    }
                });
            },
        );
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

/// 安装 env_logger，并过滤掉 gpui Windows 后端退出期的固定噪音。
fn install_logger() {
    let inner =
        env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).build();
    if log::set_boxed_logger(Box::new(TeardownNoiseFilter(inner))).is_ok() {
        // 具体过滤由 env_logger 内部完成，全局只做放行上限。
        log::set_max_level(log::LevelFilter::Trace);
    }
}

/// 只丢弃「无 target 的窗口 teardown 错误」，其它日志原样透传。
///
/// bezel-gpui 在关闭窗口时会先销毁 HWND，再清理 OLE/TSF 对象，产生
/// `window not found` 与 `无效的窗口句柄 (0x80040102 / 0x80070578)`。
/// 这些记录没有模块 target，不影响功能，属于已知上游噪音（S10 9.22）。
struct TeardownNoiseFilter<L>(L);

impl<L: log::Log> log::Log for TeardownNoiseFilter<L> {
    fn enabled(&self, metadata: &log::Metadata) -> bool {
        self.0.enabled(metadata)
    }

    fn log(&self, record: &log::Record) {
        if is_window_teardown_noise(record) {
            return;
        }
        self.0.log(record);
    }

    fn flush(&self) {
        self.0.flush();
    }
}

fn is_window_teardown_noise(record: &log::Record) -> bool {
    if record.level() != log::Level::Error || !record.target().is_empty() {
        return false;
    }
    let message = record.args().to_string();
    message.contains("window not found") || message.contains("无效的窗口句柄")
}

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
            log::info!(
                "Loaded {} sessions ({} issues)",
                store.count(),
                issues.len()
            );
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

/// 安装 panic hook：记录日志 → 尝试落盘会话 → 调用原 hook。
///
/// 只 clone 一个 `Arc` 进闭包，不引入全局可变状态；`try_lock` 保证
/// panic 发生在持锁期间时不会二次死锁。
fn install_panic_hook(store: Arc<Mutex<rterm_session::SessionStore>>, loaded_ok: bool) {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        log::error!("rterm panic: {info}");
        if !loaded_ok {
            // 启动加载已失败，当前 store 是空库；此时落盘会覆盖损坏文件。
            log::warn!("启动加载失败，panic 时跳过保存以避免覆盖原文件");
            previous(info);
            return;
        }
        match store.try_lock() {
            Ok(mut guard) => {
                let result = guard.save(
                    &state::sessions_path(),
                    rterm_session::crypto_bridge::encrypt_session_dpapi,
                );
                match result {
                    Ok(()) => log::info!("panic 时已保存会话"),
                    Err(e) => log::error!("panic 时保存会话失败: {e:#}"),
                }
            }
            Err(_) => log::warn!("panic 时会话存储被占用，跳过保存以避免死锁"),
        }
        previous(info);
    }));
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

#[cfg(test)]
mod tests {
    use super::is_window_teardown_noise;

    fn record<'a>(
        level: log::Level,
        target: &'a str,
        args: std::fmt::Arguments<'a>,
    ) -> log::Record<'a> {
        log::Record::builder()
            .level(level)
            .target(target)
            .args(args)
            .build()
    }

    #[test]
    fn filters_only_empty_target_window_teardown_errors() {
        assert!(is_window_teardown_noise(&record(
            log::Level::Error,
            "",
            format_args!("window not found")
        )));
        assert!(is_window_teardown_noise(&record(
            log::Level::Error,
            "",
            format_args!("无效的窗口句柄。 (0x80070578)")
        )));
        // 有 target 的错误不拦（避免误伤业务日志）
        assert!(!is_window_teardown_noise(&record(
            log::Level::Error,
            "rterm_app",
            format_args!("window not found")
        )));
        // 非 Error 级别不拦
        assert!(!is_window_teardown_noise(&record(
            log::Level::Warn,
            "",
            format_args!("window not found")
        )));
        // 其它无 target 错误不拦
        assert!(!is_window_teardown_noise(&record(
            log::Level::Error,
            "",
            format_args!("some other failure")
        )));
    }
}

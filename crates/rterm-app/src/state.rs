//! AppState — 应用全局状态 + Settings 默认值
//!
//! SessionStore 已移到 `rterm_session::SessionStore`（Sprint 2 落地）。
//! Theme 系统见 `crate::theme`。

use gpui::Entity;
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex, MutexGuard};
use uuid::Uuid;

use crate::event::EventBusHost;
use crate::theme::{self, Theme};

/// 获取 Mutex 守卫；锁中毒（持锁线程 panic）时记录错误并恢复内部数据。
///
/// UI / 业务路径统一走这里，避免一次 panic 导致后续所有加锁都
/// `.unwrap()` 崩溃（白屏）。恢复后继续使用可能不一致的数据，
/// 但比直接退出更利于用户保存现场。
pub fn lock_or_recover<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    match mutex.lock() {
        Ok(guard) => guard,
        Err(poisoned) => {
            log::error!("Mutex poisoned，恢复内部状态继续运行");
            poisoned.into_inner()
        }
    }
}

// ---------------------------------------------------------------------------
// Settings（持久化 JSON，Sprint 4 落地：%APPDATA%\rterm\settings.json）
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    // SSH 默认值
    pub default_port: u16,
    pub default_username: Option<String>,
    pub keepalive_secs: u32,
    pub connect_timeout_secs: u32,

    // 终端外观
    pub default_theme: String,
    pub default_font_family: String,
    pub default_font_size: f32,
    pub terminal_scrollback_lines: usize,
    pub terminal_cursor_style: CursorStyle,

    // SFTP
    pub sftp_sync_mode: String, // "off", "term_to_sftp", "sftp_to_term", "bidirectional"

    // 监控
    pub monitor_interval_secs: u32,
    pub monitor_cpu_threshold: f32,
    pub monitor_mem_threshold: f32,

    // 路径
    pub terminal_log_dir: Option<String>,
    pub data_dir: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub enum CursorStyle {
    Block,
    Underline,
    Bar,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            default_port: 22,
            default_username: None,
            keepalive_secs: 30,
            connect_timeout_secs: 10,

            default_theme: "Catppuccin Mocha".into(),
            default_font_family: "Cascadia Code".into(),
            default_font_size: 14.0,
            terminal_scrollback_lines: 10_000,
            terminal_cursor_style: CursorStyle::Block,

            sftp_sync_mode: "bidirectional".into(),

            monitor_interval_secs: 2,
            monitor_cpu_threshold: 90.0,
            monitor_mem_threshold: 85.0,

            terminal_log_dir: None,
            data_dir: None,
        }
    }
}

// ---------------------------------------------------------------------------
// Settings 持久化 — `%APPDATA%\rterm\settings.json`
// ---------------------------------------------------------------------------

/// `%APPDATA%\rterm\settings.json`。
///
/// 注意：settings.json 自身固定放在配置目录，不跟随 `Settings.data_dir`，
/// 否则读取 data_dir 前就要先加载 settings，会形成 bootstrap 循环。
/// data_dir 只控制会话 / 导出等数据文件的位置（见 [`data_dir_path`]）。
pub fn settings_path() -> std::path::PathBuf {
    let mut p = dirs::data_dir().unwrap_or_else(|| std::path::PathBuf::from("."));
    p.push("rterm");
    p.push("settings.json");
    p
}

/// 数据目录：`Settings.data_dir`（非空时）否则 `%APPDATA%\rterm`。
///
/// 会话 / 导出等数据文件都放在该目录下。
pub fn data_dir_path() -> std::path::PathBuf {
    data_dir_for(&Settings::load())
}

/// 给定设置下的数据目录。
pub fn data_dir_for(settings: &Settings) -> std::path::PathBuf {
    match settings.data_dir.as_deref() {
        Some(dir) if !dir.trim().is_empty() => std::path::PathBuf::from(dir.trim()),
        _ => {
            let mut path = dirs::data_dir().unwrap_or_else(|| std::path::PathBuf::from("."));
            path.push("rterm");
            path
        }
    }
}

/// `data_dir` 变更时迁移 `sessions.json`（9.16b）。
///
/// 复制而非移动：旧目录保持不变，失败只需放弃保存设置即可回滚。
pub fn migrate_data_dir(old: &Settings, new: &Settings) -> anyhow::Result<()> {
    let old_dir = data_dir_for(old);
    let new_dir = data_dir_for(new);
    if old_dir == new_dir {
        return Ok(());
    }
    let source = old_dir.join("sessions.json");
    let target = new_dir.join("sessions.json");
    if !source.exists() || target.exists() {
        return Ok(());
    }
    std::fs::create_dir_all(&new_dir)
        .map_err(|error| anyhow::anyhow!("创建新数据目录失败: {error}"))?;
    std::fs::copy(&source, &target)
        .map_err(|error| anyhow::anyhow!("复制会话文件失败: {error}"))?;
    log::info!(
        "[settings] data_dir 迁移: {} -> {}",
        source.display(),
        target.display()
    );
    Ok(())
}

/// `<data_dir>\sessions.json`。
pub fn sessions_path() -> std::path::PathBuf {
    data_dir_path().join("sessions.json")
}

/// 终端会话日志目录：`Settings.terminal_log_dir`（非空时）否则 `<data_dir>\logs`。
pub fn logs_dir_path(settings: &Settings) -> std::path::PathBuf {
    match settings.terminal_log_dir.as_deref() {
        Some(dir) if !dir.trim().is_empty() => std::path::PathBuf::from(dir.trim()),
        _ => data_dir_path().join("logs"),
    }
}

/// 立即把当前 SessionStore 写盘（DPAPI 加密），用于 CRUD 后即时持久化。
///
/// 调用方必须保证调用前已释放 store 的 MutexGuard，否则会死锁。
pub fn persist_sessions(store: &Arc<Mutex<rterm_session::SessionStore>>) -> anyhow::Result<()> {
    let mut guard = store
        .lock()
        .map_err(|e| anyhow::anyhow!("会话存储锁已损坏: {e}"))?;
    guard
        .save(
            &sessions_path(),
            rterm_session::crypto_bridge::encrypt_session_dpapi,
        )
        .map_err(|e| anyhow::anyhow!("保存会话失败: {e:#}"))
}

impl Settings {
    /// 加载设置。文件不存在 / 解析失败 → 返回默认值（不阻断启动）。
    pub fn load() -> Self {
        let path = settings_path();
        if !path.exists() {
            log::info!("settings.json 不存在，使用默认设置");
            return Self::default();
        }
        match std::fs::read_to_string(&path)
            .map_err(|e| e.to_string())
            .and_then(|raw| serde_json::from_str::<Settings>(&raw).map_err(|e| e.to_string()))
        {
            Ok(mut s) => {
                // 旧版本可能存了不存在的主题名，回退默认
                if theme::theme_by_name(&s.default_theme).name() != s.default_theme {
                    s.default_theme = Settings::default().default_theme;
                }
                log::info!("Loaded settings from {}", path.display());
                s
            }
            Err(e) => {
                log::error!("加载 settings.json 失败（使用默认值）: {e}");
                Self::default()
            }
        }
    }

    /// 保存设置：先写临时文件再 rename，避免半截文件。
    pub fn save(&self) -> anyhow::Result<()> {
        let path = settings_path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| anyhow::anyhow!("创建配置目录失败: {e}"))?;
        }
        let json = serde_json::to_string_pretty(self)
            .map_err(|e| anyhow::anyhow!("序列化设置失败: {e}"))?;
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, json).map_err(|e| anyhow::anyhow!("写临时文件失败: {e}"))?;
        std::fs::rename(&tmp, &path)
            .map_err(|e| anyhow::anyhow!("重命名 settings.json 失败: {e}"))?;
        log::info!("Saved settings to {}", path.display());
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// SessionStore — 用 rterm_session::SessionStore（Sprint 2 落地）
// ---------------------------------------------------------------------------

// 用 crate 引用：state.rs 里 SessionStore 字段类型直接是 rterm_session::SessionStore

// ---------------------------------------------------------------------------
// Tab 展示模型（Sprint 6：TabInfo / TabKind 供 TabBar 使用；
// 实际 Tab 列表与生命周期在 MainWindow.tabs）
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub enum TabKind {
    Terminal,
    Sftp,
    Monitor,
}

pub struct TabInfo {
    pub id: Uuid,
    pub title: String,
    pub kind: TabKind,
}

// ---------------------------------------------------------------------------
// AppState — 顶层应用状态
// ---------------------------------------------------------------------------

pub struct AppState {
    pub event_bus_host: Entity<EventBusHost>,
    pub settings: Settings,
    pub session_store: Arc<Mutex<rterm_session::SessionStore>>,
    /// 当前主题（Arc<dyn Theme> 方便 clone 传递）
    pub theme: Arc<dyn Theme>,
    /// 启动期加载会话时的错误提示（MainWindow 用红色横幅展示）。
    pub startup_error: Option<String>,
}

// AppState 存 gpui::App::set_global，要求实现 Global
impl gpui::Global for AppState {}

impl AppState {
    /// AppState::new 需要 EventBusHost Entity —— 在 main.rs 里 app.new() 之后构造。
    /// 主题由 `settings.default_theme` 决定。
    pub fn new(
        event_bus_host: Entity<EventBusHost>,
        session_store: Arc<Mutex<rterm_session::SessionStore>>,
        settings: Settings,
        startup_error: Option<String>,
    ) -> Self {
        let theme = theme::theme_by_name(&settings.default_theme);
        Self {
            event_bus_host,
            settings,
            session_store,
            theme,
            startup_error,
        }
    }

    /// 便捷方法：从 AppState 取主题，避免每个 View 都写 Arc 解引用
    pub fn t(&self) -> &dyn Theme {
        self.theme.as_ref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lock_or_recover_recovers_from_poison() {
        let mutex = Mutex::new(7_u32);
        // 持锁期间 panic → 锁中毒。
        let poisoned = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _guard = mutex
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            panic!("intentional poison");
        }));
        assert!(poisoned.is_err());
        assert!(mutex.lock().is_err(), "锁应处于中毒状态");

        let guard = lock_or_recover(&mutex);
        assert_eq!(*guard, 7);
    }

    #[test]
    fn migrate_data_dir_copies_sessions_file() {
        let base = std::env::temp_dir().join(format!(
            "rterm-migrate-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        let old_dir = base.join("old");
        let new_dir = base.join("new");
        std::fs::create_dir_all(&old_dir).expect("建旧目录");
        std::fs::write(old_dir.join("sessions.json"), "{\"sessions\":[]}").expect("写旧会话");

        let mut old_settings = Settings::default();
        old_settings.data_dir = Some(old_dir.to_string_lossy().into_owned());
        let mut new_settings = Settings::default();
        new_settings.data_dir = Some(new_dir.to_string_lossy().into_owned());

        migrate_data_dir(&old_settings, &new_settings).expect("迁移成功");
        assert_eq!(
            std::fs::read_to_string(new_dir.join("sessions.json")).expect("读新会话"),
            "{\"sessions\":[]}"
        );
        assert!(
            old_dir.join("sessions.json").exists(),
            "复制而非移动，旧文件保留"
        );

        // 目录未变化时不做任何事
        migrate_data_dir(&new_settings, &new_settings).expect("同目录跳过");

        let _ = std::fs::remove_dir_all(&base);
    }
}

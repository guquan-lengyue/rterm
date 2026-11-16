//! AppState — 应用全局状态 + Settings 默认值 + TabRegistry
//!
//! SessionStore 已移到 `rterm_session::SessionStore`（Sprint 2 落地）。
//! Theme 系统见 `crate::theme`。

use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};
use uuid::Uuid;
use gpui::Entity;

use crate::event::EventBusHost;
use crate::theme::{self, Theme};

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
    pub sftp_sync_mode: String,  // "off", "term_to_sftp", "sftp_to_term", "bidirectional"

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

/// `%APPDATA%\rterm\settings.json`，与 sessions.json 同目录。
pub fn settings_path() -> std::path::PathBuf {
    let mut p = dirs::data_dir().unwrap_or_else(|| std::path::PathBuf::from("."));
    p.push("rterm");
    p.push("settings.json");
    p
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
        std::fs::write(&tmp, json)
            .map_err(|e| anyhow::anyhow!("写临时文件失败: {e}"))?;
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
// TabRegistry（Sprint 6：TabInfo / TabKind 已被 TabBar 使用；
// 实际 Tab 列表与生命周期在 MainWindow.tabs，此处保留给 S7/S9 抽公共 Registry）
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

pub struct TabRegistry {
    pub tabs: Vec<TabInfo>,
    pub active: usize,
}

impl TabRegistry {
    pub fn new() -> Self {
        Self { tabs: Vec::new(), active: 0 }
    }
    pub fn count(&self) -> usize { self.tabs.len() }
}

impl Default for TabRegistry {
    fn default() -> Self { Self::new() }
}

// ---------------------------------------------------------------------------
// AppState — 顶层应用状态
// ---------------------------------------------------------------------------

pub struct AppState {
    pub event_bus_host: Entity<EventBusHost>,
    pub settings: Settings,
    pub session_store: Arc<Mutex<rterm_session::SessionStore>>,
    pub tab_registry: TabRegistry,
    /// 当前主题（Arc<dyn Theme> 方便 clone 传递）
    pub theme: Arc<dyn Theme>,
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
    ) -> Self {
        let theme = theme::theme_by_name(&settings.default_theme);
        Self {
            event_bus_host,
            settings,
            session_store,
            tab_registry: TabRegistry::new(),
            theme,
        }
    }

    /// 便捷方法：从 AppState 取主题，避免每个 View 都写 Arc 解引用
    pub fn t(&self) -> &dyn Theme {
        self.theme.as_ref()
    }
}

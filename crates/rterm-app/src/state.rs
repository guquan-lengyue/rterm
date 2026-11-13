//! AppState — 应用全局状态 + Settings 默认值 + TabRegistry
//!
//! SessionStore 已移到 `rterm_session::SessionStore`（Sprint 2 落地）。

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::event::EventBus;

// ---------------------------------------------------------------------------
// Settings（持久化 JSON，Sprint 3 完成持久化；当前用 Default 占位）
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Serialize, Deserialize)]
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

            default_theme: "Solarized Dark".into(),
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
// SessionStore — 用 rterm_session::SessionStore（Sprint 2 落地）
// ---------------------------------------------------------------------------

// 用 crate 引用：state.rs 里 SessionStore 字段类型直接是 rterm_session::SessionStore

// ---------------------------------------------------------------------------
// TabRegistry（Sprint 5 完成；当前空占位）
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
    pub event_bus: EventBus,
    pub settings: Settings,
    pub session_store: rterm_session::SessionStore,
    pub tab_registry: TabRegistry,
}

// AppState 存 gpui::App::set_global，要求实现 Global
impl gpui::Global for AppState {}

impl AppState {
    pub fn new() -> Self {
        Self {
            event_bus: EventBus,
            settings: Settings::default(),
            session_store: rterm_session::SessionStore::new(),
            tab_registry: TabRegistry::new(),
        }
    }
}

impl Default for AppState {
    fn default() -> Self { Self::new() }
}

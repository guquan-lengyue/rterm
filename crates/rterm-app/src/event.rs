//! EventBus — 跨模块事件广播
//!
//! Service 层 emit，View 层 subscribe。底层是 bezel-gpui 的 EventEmitter trait。
//!
//! 用法：
//! ```ignore
//! // emit 侧：Service 层拿到 &mut App
//! app.emit(&event_bus, AppEvent::SessionConnected { id });
//!
//! // subscribe 侧：View 里
//! cx.subscribe(&event_bus, |app, ev| { ... });
//! ```

use gpui::EventEmitter;
use uuid::Uuid;

/// 跨模块事件。按领域分组：会话 / 终端 / SFTP / 监控 / 传输 / UI。
#[derive(Clone, Debug)]
pub enum AppEvent {
    // ---- 会话 ----
    SessionCreated { session_id: Uuid },
    SessionUpdated { session_id: Uuid },
    SessionDeleted { session_id: Uuid },
    SessionImported { added: usize, updated: usize, skipped: usize },
    SessionConnectRequested { session_id: Uuid },
    SessionConnected { session_id: Uuid },
    SessionFailed { session_id: Uuid, reason: String },
    SessionDisconnected { session_id: Uuid, reason: String },
    SessionReconnecting { session_id: Uuid, attempt: u8 },

    // ---- 终端 ----
    TerminalOpened { tab_id: Uuid, session_id: Uuid },
    TerminalData { tab_id: Uuid, session_id: Uuid, bytes: Vec<u8> },
    TerminalCwdHint { tab_id: Uuid, session_id: Uuid, cwd: String, source: CwdSource },
    TerminalResize { tab_id: Uuid, cols: u16, rows: u16 },
    TerminalClosed { tab_id: Uuid },

    // ---- SFTP ----
    SftpOpened { tab_id: Uuid, session_id: Uuid },
    SftpNavigated { tab_id: Uuid, session_id: Uuid, path: String },
    UserRequestedCwd { tab_id: Uuid, path: String },
    SftpClosed { tab_id: Uuid },

    // ---- 监控 ----
    MonitorOpened { tab_id: Uuid, session_id: Uuid },
    MonitorSample { session_id: Uuid, data: String },
    MonitorAlert { session_id: Uuid, metric: String, value: f64, threshold: f64 },
    MonitorClosed { tab_id: Uuid },

    // ---- 传输 ----
    FileTransferStarted { id: Uuid, remote_path: String, local_path: String },
    FileTransferProgress { id: Uuid, transferred: u64, total: u64 },
    FileTransferCompleted { id: Uuid },
    FileTransferFailed { id: Uuid, reason: String },

    // ---- SSH 基础设施 ----
    HostKeyUnknown { session_id: Uuid, host: String, fingerprint: String },
    HostKeyChanged { session_id: Uuid, host: String, old_fp: String, new_fp: String },

    // ---- UI ----
    AppSettingsChanged,
    AppThemeChanged { name: String },
    StatusBarMessage { text: String, duration_ms: u64 },
}

#[derive(Clone, Debug)]
pub enum CwdSource {
    Ps1,
    PwdFallback,
    SftpNav,
}

/// 全局事件总线。空 struct，仅用来实现 EventEmitter<AppEvent>。
/// 持有在 AppState 里，跨 Service / View 共享。
pub struct EventBus;

// EventEmitter 是 marker trait —— 只需要这一行，gpui 内部 SubscriberSet 自动管理
impl EventEmitter<AppEvent> for EventBus {}

//! 跨模块事件广播 — AppEvent 定义 + EventBusHost 宿主
//!
//! Service 层 emit，View 层 subscribe。底层是 bezel-gpui 的 EventEmitter trait。
//! gpui 0.3 中 EventEmitter 只能挂在 Entity 上，故由 [`EventBusHost`] 承载，
//! 全局只建一个并存到 `AppState.event_bus_host`。
//!
//! 用法：
//! ```ignore
//! // emit 侧：Service 层拿到 &mut App
//! let host = cx.global::<AppState>().event_bus_host.clone();
//! cx.update_entity(&host, |_host, cx| cx.emit(AppEvent::SessionConnected { session_id }));
//!
//! // subscribe 侧：View 里
//! cx.subscribe(&host, |app, ev| { ... });
//! ```

use gpui::IntoElement;
use uuid::Uuid;

/// 跨模块事件。按领域分组：会话 / 终端 / SFTP / 监控 / 传输 / UI。
#[derive(Clone, Debug)]
pub enum AppEvent {
    // ---- 会话 ----
    SessionCreated {
        session_id: Uuid,
    },
    SessionUpdated {
        session_id: Uuid,
    },
    SessionDeleted {
        session_id: Uuid,
    },
    SessionImported {
        added: usize,
        updated: usize,
        skipped: usize,
    },
    SessionConnectRequested {
        session_id: Uuid,
    },
    SessionConnected {
        session_id: Uuid,
    },
    SessionFailed {
        session_id: Uuid,
        reason: String,
    },
    SessionDisconnected {
        session_id: Uuid,
        reason: String,
    },
    SessionReconnecting {
        session_id: Uuid,
        attempt: u8,
    },
    /// SSH 握手 + 认证 + PTY 就绪耗时（Sprint 6 状态栏显示）
    SessionLatency {
        session_id: Uuid,
        latency_ms: u64,
    },

    // ---- 终端 ----
    TerminalOpened {
        tab_id: Uuid,
        session_id: Uuid,
    },
    TerminalCwdHint {
        tab_id: Uuid,
        session_id: Uuid,
        cwd: String,
        source: CwdSource,
    },
    TerminalClosed {
        tab_id: Uuid,
    },

    // ---- 标签页（TabBar → MainWindow） ----
    TabSelectRequested {
        tab_id: Uuid,
    },
    TabCloseRequested {
        tab_id: Uuid,
    },
    TabCloseOthersRequested {
        tab_id: Uuid,
    },
    TabCloseRightRequested {
        tab_id: Uuid,
    },

    // ---- SFTP ----
    SftpOpened {
        tab_id: Uuid,
        session_id: Uuid,
    },
    SftpNavigated {
        tab_id: Uuid,
        session_id: Uuid,
        path: String,
    },
    UserRequestedCwd {
        tab_id: Uuid,
        path: String,
    },
    SftpClosed {
        tab_id: Uuid,
    },

    // ---- 监控（S9：采样数据由 MonitorView 本地消费，不走全局事件） ----
    MonitorOpened {
        tab_id: Uuid,
        session_id: Uuid,
    },
    MonitorClosed {
        tab_id: Uuid,
    },

    // ---- 传输 ----
    FileTransferStarted {
        id: Uuid,
        remote_path: String,
        local_path: String,
    },
    FileTransferProgress {
        id: Uuid,
        transferred: u64,
        total: u64,
    },
    FileTransferCompleted {
        id: Uuid,
    },
    FileTransferFailed {
        id: Uuid,
        reason: String,
    },

    // ---- UI ----
    AppSettingsChanged,
    AppThemeChanged {
        name: String,
    },
    StatusBarMessage {
        text: String,
        duration_ms: u64,
    },

    // ---- 设置 / 导入导出 Modal 控制（View → MainWindow） ----
    SettingsCloseRequested {
        saved: bool,
    },
    SettingsOpenImportExport {
        export: bool,
    },
    ImportExportCloseRequested,

    // ---- SessionTreeView 内部驱动（View → EventBus → View 改状态 → 重绘） ----
    SessionTreeSelectRequested {
        session_id: Option<Uuid>,
    },
    SessionTreeFolderToggle {
        folder: String,
    },
    SessionTreeOpenEditDialog {
        session_id: Option<Uuid>,
    }, // None = 新建
    SessionTreeOpenContextMenuAt {
        session_id: Option<Uuid>,
        x: i32,
        y: i32,
    },
}

#[derive(Clone, Debug)]
pub enum CwdSource {
    Ps1,
    PwdFallback,
    SftpNav,
}

/// EventBus 的 Entity 宿主。必须有 Render（返回空）才能成为 Entity。
/// 全局只建一个，存到 AppState.event_bus_host。
pub struct EventBusHost;

impl gpui::Render for EventBusHost {
    fn render(
        &mut self,
        _window: &mut gpui::Window,
        _cx: &mut gpui::Context<Self>,
    ) -> impl IntoElement {
        gpui::div()
    }
}

impl gpui::EventEmitter<AppEvent> for EventBusHost {}

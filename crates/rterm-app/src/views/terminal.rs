//! TerminalView — Sprint 5 版（文本 placeholder）
//!
//! 负责：
//! - 订阅 `SessionConnectRequested` → 后台线程 SSH 连接 + PTY
//! - PTY 字节流 → `TerminalEngine.input_bytes`
//! - 键盘 → xterm 字节 → PTY
//! - 连接状态 / 指数退避重连 / EventBus 广播
//!
//! S6 会把文本渲染替换成 gpui Canvas 逐 Cell 渲染。

use std::sync::mpsc::{Receiver, Sender};

use futures::channel::mpsc::{unbounded, UnboundedSender};
use futures::StreamExt;
use gpui::prelude::*;
use gpui::{div, px, Context, FocusHandle, IntoElement, KeyDownEvent, Render, Window};
use uuid::Uuid;

use rterm_ssh::pty::{self, PtyCommand, PtyEvent};
use rterm_ssh::reconnect::ReconnectPolicy;
use rterm_ssh::SshSession;
use rterm_terminal::{TerminalEngine, TerminalKey};

use crate::event::{AppEvent, EventBusHost};
use crate::state::AppState;
use crate::ui::theme;

#[derive(Clone, Debug, PartialEq, Eq)]
enum TerminalStatus {
    Idle,
    Connecting,
    Connected,
    Reconnecting(u8),
    Failed(String),
}

impl TerminalStatus {
    fn label(&self) -> String {
        match self {
            Self::Idle => "未连接".into(),
            Self::Connecting => "连接中…".into(),
            Self::Connected => "已连接".into(),
            Self::Reconnecting(attempt) => format!("重连中（第 {attempt} 次）…"),
            Self::Failed(_) => "连接失败".into(),
        }
    }
}

pub struct TerminalView {
    focus_handle: FocusHandle,
    engine: TerminalEngine,
    cols: u16,
    rows: u16,
    scrollback: usize,
    font_family: String,
    font_size: f32,

    status: TerminalStatus,
    session: Option<rterm_session::Session>,
    session_id: Option<Uuid>,
    tab_id: Uuid,
    input_tx: Option<Sender<PtyCommand>>,
    generation: u64,
    reconnect_attempt: u8,
    autofocus_pending: bool,
}

impl TerminalView {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let (cols, rows, scrollback, font_family, font_size) = {
            let settings = &cx.global::<AppState>().settings;
            (
                100_u16,
                24_u16,
                settings.terminal_scrollback_lines,
                settings.default_font_family.clone(),
                settings.default_font_size,
            )
        };

        let host = cx.global::<AppState>().event_bus_host.clone();
        cx.subscribe(&host, |this: &mut Self, _host, event: &AppEvent, cx| {
            if let AppEvent::SessionConnectRequested { session_id } = event {
                this.connect(*session_id, cx);
            }
        })
        .detach();

        Self {
            focus_handle: cx.focus_handle(),
            engine: TerminalEngine::new(cols as usize, rows as usize, scrollback),
            cols,
            rows,
            scrollback,
            font_family,
            font_size,
            status: TerminalStatus::Idle,
            session: None,
            session_id: None,
            tab_id: Uuid::new_v4(),
            input_tx: None,
            generation: 0,
            reconnect_attempt: 0,
            autofocus_pending: false,
        }
    }

    // -----------------------------------------------------------------------
    // 连接编排
    // -----------------------------------------------------------------------

    fn connect(&mut self, session_id: Uuid, cx: &mut Context<Self>) {
        let stored = {
            let state = cx.global::<AppState>();
            let store = state.session_store.lock().unwrap();
            store.get(session_id).cloned()
        };

        let Some(stored) = stored else {
            self.fail(format!("会话不存在: {session_id}"), cx);
            return;
        };

        // store 里通常是明文（version=0）；导入的备份可能是密文。
        let session = match rterm_session::crypto_bridge::decrypt_session_dpapi(&stored) {
            Ok(session) => session,
            Err(error) => {
                self.fail(format!("解密会话失败: {error:#}"), cx);
                return;
            }
        };

        if session.host.trim().is_empty() {
            self.fail("会话主机地址为空".into(), cx);
            return;
        }

        log::info!(
            "[terminal] connect requested: {}@{}:{}",
            session.username,
            session.host,
            session.port
        );

        self.session = Some(session);
        self.session_id = Some(session_id);
        self.tab_id = Uuid::new_v4();
        self.reconnect_attempt = 0;
        self.status = TerminalStatus::Connecting;
        self.start_attempt(cx);
        cx.notify();
    }

    fn start_attempt(&mut self, cx: &mut Context<Self>) {
        let Some(config) = self.session.clone() else {
            return;
        };

        self.generation = self.generation.wrapping_add(1);
        let generation = self.generation;
        self.input_tx = None;
        self.autofocus_pending = false;
        self.engine =
            TerminalEngine::new(self.cols as usize, self.rows as usize, self.scrollback);

        let (output_tx, output_rx) = unbounded::<PtyEvent>();
        let (input_tx, input_rx) = std::sync::mpsc::channel::<PtyCommand>();
        self.input_tx = Some(input_tx);

        let cols = self.cols;
        let rows = self.rows;
        let spawn_result = std::thread::Builder::new()
            .name(format!("rterm-ssh-{generation}"))
            .spawn(move || run_connection(config, cols, rows, output_tx, input_rx));

        if let Err(error) = spawn_result {
            self.fail(format!("创建 SSH 线程失败: {error}"), cx);
            return;
        }

        let mut output_rx = output_rx;
        cx.spawn(async move |this, cx| {
            while let Some(event) = output_rx.next().await {
                if this
                    .update(cx, |this, cx| {
                        this.handle_pty_event(generation, event, cx)
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
    }

    fn handle_pty_event(&mut self, generation: u64, event: PtyEvent, cx: &mut Context<Self>) {
        if generation != self.generation {
            return;
        }

        match event {
            PtyEvent::HostKey(info) => {
                let Some(session_id) = self.session_id else {
                    return;
                };
                match info.decision {
                    rterm_ssh::HostKeyDecision::Unknown => {
                        self.emit(
                            cx,
                            AppEvent::HostKeyUnknown {
                                session_id,
                                host: info.host,
                                fingerprint: format!("{} {}", info.key_type, info.fingerprint),
                            },
                        );
                    }
                    rterm_ssh::HostKeyDecision::Changed => {
                        self.emit(
                            cx,
                            AppEvent::HostKeyChanged {
                                session_id,
                                host: info.host,
                                old_fp: String::new(),
                                new_fp: format!("{} {}", info.key_type, info.fingerprint),
                            },
                        );
                    }
                    rterm_ssh::HostKeyDecision::Match => {}
                }
            }
            PtyEvent::Connected => {
                self.status = TerminalStatus::Connected;
                self.reconnect_attempt = 0;
                self.autofocus_pending = true;
                if let Some(session_id) = self.session_id {
                    self.emit(cx, AppEvent::SessionConnected { session_id });
                    self.emit(
                        cx,
                        AppEvent::TerminalOpened {
                            tab_id: self.tab_id,
                            session_id,
                        },
                    );
                }
                log::info!("[terminal] SSH + PTY ready (tab {})", self.tab_id);
                cx.notify();
            }
            PtyEvent::Data(bytes) => {
                self.engine.input_bytes(&bytes);
                if let Some(session_id) = self.session_id {
                    self.emit(
                        cx,
                        AppEvent::TerminalData {
                            tab_id: self.tab_id,
                            session_id,
                            bytes,
                        },
                    );
                }
                cx.notify();
            }
            PtyEvent::Closed { reason } => {
                log::warn!("[terminal] PTY closed: {reason}");
                self.input_tx = None;
                if let Some(session_id) = self.session_id {
                    self.emit(
                        cx,
                        AppEvent::SessionDisconnected {
                            session_id,
                            reason: reason.clone(),
                        },
                    );
                }
                self.schedule_reconnect(cx);
            }
            PtyEvent::Failed { reason, retryable } => {
                log::warn!("[terminal] connect failed (retryable={retryable}): {reason}");
                self.input_tx = None;
                if retryable {
                    self.schedule_reconnect_with_reason(reason, cx);
                } else {
                    self.fail(reason, cx);
                }
            }
        }
    }

    fn schedule_reconnect(&mut self, cx: &mut Context<Self>) {
        self.schedule_reconnect_with_reason("连接已断开".into(), cx);
    }

    fn schedule_reconnect_with_reason(&mut self, reason: String, cx: &mut Context<Self>) {
        let policy = ReconnectPolicy::default();
        let next = self.reconnect_attempt.saturating_add(1);

        if !policy.should_retry(next) {
            self.status = TerminalStatus::Failed(reason.clone());
            if let Some(session_id) = self.session_id {
                self.emit(
                    cx,
                    AppEvent::SessionFailed {
                        session_id,
                        reason: format!("重连 {} 次失败: {reason}", policy.max_attempts),
                    },
                );
            }
            self.input_tx = None;
            cx.notify();
            return;
        }

        self.reconnect_attempt = next;
        self.status = TerminalStatus::Reconnecting(next);
        if let Some(session_id) = self.session_id {
            self.emit(
                cx,
                AppEvent::SessionReconnecting {
                    session_id,
                    attempt: next,
                },
            );
        }
        cx.notify();

        let delay = ReconnectPolicy::delay(next);
        log::info!("[terminal] reconnect attempt {next} in {:?}", delay);
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(delay).await;
            let _ = this.update(cx, |this, cx| this.start_attempt(cx));
        })
        .detach();
    }

    fn fail(&mut self, reason: String, cx: &mut Context<Self>) {
        self.status = TerminalStatus::Failed(reason.clone());
        self.input_tx = None;
        if let Some(session_id) = self.session_id {
            self.emit(cx, AppEvent::SessionFailed { session_id, reason });
        }
        cx.notify();
    }

    fn emit(&self, cx: &mut Context<Self>, event: AppEvent) {
        let host: gpui::Entity<EventBusHost> = cx.global::<AppState>().event_bus_host.clone();
        cx.update_entity(&host, |_host, cx: &mut Context<EventBusHost>| {
            cx.emit(event);
        });
    }

    // -----------------------------------------------------------------------
    // 键盘
    // -----------------------------------------------------------------------

    fn on_key(&mut self, event: &KeyDownEvent, _cx: &mut Context<Self>) {
        if self.status != TerminalStatus::Connected {
            return;
        }

        let keystroke = &event.keystroke;
        let mut key = None;

        if keystroke.modifiers.control
            && !keystroke.modifiers.alt
            && !keystroke.modifiers.platform
        {
            if keystroke.key.len() == 1 {
                if let Some(ch) = keystroke.key.chars().next() {
                    key = Some(TerminalKey::Ctrl(ch));
                }
            }
        }

        if key.is_none() {
            key = match keystroke.key.as_str() {
                "enter" => Some(TerminalKey::Enter),
                "tab" => Some(TerminalKey::Tab),
                "backspace" => Some(TerminalKey::Backspace),
                "escape" => Some(TerminalKey::Escape),
                "up" => Some(TerminalKey::Up),
                "down" => Some(TerminalKey::Down),
                "left" => Some(TerminalKey::Left),
                "right" => Some(TerminalKey::Right),
                "home" => Some(TerminalKey::Home),
                "end" => Some(TerminalKey::End),
                "pageup" => Some(TerminalKey::PageUp),
                "pagedown" => Some(TerminalKey::PageDown),
                "delete" => Some(TerminalKey::Delete),
                "insert" => Some(TerminalKey::Insert),
                "space" => Some(TerminalKey::Char(" ".into())),
                key if is_function_key(key) => {
                    key[1..].parse::<u8>().ok().map(TerminalKey::F)
                }
                _ => None,
            };
        }

        if key.is_none()
            && !keystroke.modifiers.control
            && !keystroke.modifiers.alt
            && !keystroke.modifiers.platform
        {
            if let Some(text) = &keystroke.key_char {
                if !text.is_empty() {
                    key = Some(TerminalKey::Char(text.clone()));
                }
            }
        }

        if let Some(key) = key {
            let bytes = self.engine.input_key(key);
            if let Some(tx) = &self.input_tx {
                let _ = tx.send(PtyCommand::Write(bytes));
            }
        }
    }
}

fn is_function_key(key: &str) -> bool {
    let Some(number) = key.strip_prefix('f') else {
        return false;
    };
    matches!(
        number.parse::<u8>(),
        Ok(1..=12)
    )
}

/// 后台线程：连接 → 打开 PTY → 进入 IO 循环。
fn run_connection(
    config: rterm_session::Session,
    cols: u16,
    rows: u16,
    output: UnboundedSender<PtyEvent>,
    commands: Receiver<PtyCommand>,
) {
    let mut ssh = SshSession::new();

    if let Err(error) = ssh.connect(&config) {
        if let Some(info) = ssh.last_host_key() {
            let _ = output.unbounded_send(PtyEvent::HostKey(info.clone()));
        }
        let reason = format!("{error:#}");
        let retryable = connect_error_is_retryable(&reason);
        let _ = output.unbounded_send(PtyEvent::Failed { reason, retryable });
        return;
    }

    if let Some(info) = ssh.last_host_key() {
        let _ = output.unbounded_send(PtyEvent::HostKey(info.clone()));
    }

    let channel = match ssh.open_pty(cols as u32, rows as u32) {
        Ok(channel) => channel,
        Err(error) => {
            let _ = output.unbounded_send(PtyEvent::Failed {
                reason: format!("打开 PTY 失败: {error:#}"),
                retryable: true,
            });
            return;
        }
    };

    let Some(session) = ssh.session_clone() else {
        let _ = output.unbounded_send(PtyEvent::Failed {
            reason: "SSH Session 丢失".into(),
            retryable: false,
        });
        return;
    };

    let _ = output.unbounded_send(PtyEvent::Connected);
    pty::run_io(channel, session, config.keepalive_secs, commands, output);
}

/// 认证 / 配置 / 主机密钥错误重试没有意义，只有网络类错误才退避重连。
fn connect_error_is_retryable(reason: &str) -> bool {
    !(reason.contains("认证失败")
        || reason.contains("认证未通过")
        || reason.contains("主机密钥已变更")
        || reason.contains("用户名为空")
        || reason.contains("密码为空")
        || reason.contains("主机地址为空")
        || reason.contains("known_hosts 校验失败"))
}

impl Render for TerminalView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);

        if self.status == TerminalStatus::Connected && self.autofocus_pending {
            window.focus(&self.focus_handle, cx);
            self.autofocus_pending = false;
        }

        let title = self
            .session
            .as_ref()
            .map(|session| session.display_label())
            .unwrap_or_else(|| "终端".to_string());
        let status = self.status.label();
        let lines = self.engine.visible_lines();
        let font_size = self.font_size;
        let line_height = (font_size * 1.35).max(font_size + 2.0);
        let text_color = t.text_on_app_bg();

        let mut root = div()
            .size_full()
            .bg(t.app_bg())
            .flex()
            .flex_col()
            .id("rterm-terminal-view")
            .track_focus(&self.focus_handle)
            .key_context("Terminal")
            .on_key_down(cx.listener(|this, event, _window, cx| this.on_key(event, cx)))
            .on_click(cx.listener(|this, _event, window, cx| {
                window.focus(&this.focus_handle, cx);
                cx.notify();
            }));

        root = root.child(
            div()
                .w_full()
                .h(px(26.))
                .px_3()
                .flex()
                .items_center()
                .bg(t.toolbar_bg())
                .border_b_1()
                .border_color(t.border_subtle())
                .text_size(px(11.))
                .text_color(t.text_on_toolbar_bg())
                .child(format!("{title}  ·  {status}")),
        );

        if self.status == TerminalStatus::Idle {
            root = root.child(
                div()
                    .flex_1()
                    .flex()
                    .items_center()
                    .justify_center()
                    .flex_col()
                    .gap_2()
                    .child(
                        div()
                            .text_size(px(13.))
                            .text_color(t.text_subtle())
                            .child("← 双击会话开始连接"),
                    )
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(t.text_placeholder())
                            .child("Sprint 5 · SSH + PTY"),
                    ),
            );
        } else {
            let mut body = div().flex_1().px_2().py_1().flex().flex_col();
            for line in lines {
                body = body.child(
                    div()
                        .h(px(line_height))
                        .text_size(px(font_size))
                        .font_family(self.font_family.clone())
                        .text_color(text_color)
                        .child(line),
                );
            }
            root = root.child(body);

            if let TerminalStatus::Failed(reason) = &self.status {
                root = root.child(
                    div()
                        .w_full()
                        .h(px(24.))
                        .px_2()
                        .text_size(px(11.))
                        .text_color(t.danger())
                        .child(format!("连接失败: {reason}")),
                );
            }
        }

        root
    }
}

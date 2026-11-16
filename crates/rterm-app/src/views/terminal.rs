//! TerminalView — Sprint 6 版（gpui Canvas 逐 Cell 渲染）
//!
//! 负责：
//! - 订阅 `SessionConnectRequested`（由 MainWindow 建 Tab 后调用 `connect()`）
//! - PTY 字节流 → `TerminalEngine.input_bytes`
//! - Canvas 逐 Cell：背景矩形 + DirectWrite glyph + ANSI 调色板
//! - 光标（方块 / 下划线 / 竖线 + 500ms blink）
//! - 滚动缓冲（滚轮 / Shift+PageUp/PageDown / 滚动条）
//! - 鼠标拖选 → 松开自动复制；右键粘贴（bracketed paste）
//! - 窗口 resize → cols/rows → `TerminalEngine.resize` + `PtyCommand::Resize`
//! - 键盘映射 + Ctrl+= / Ctrl+- / Ctrl+0 字号缩放
//! - 主题 / 设置变更即时重绘

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::mpsc::{Receiver, Sender};
use std::time::{Duration, Instant};

use alacritty_terminal::index::{Column, Line};
use alacritty_terminal::term::cell::{Cell, Flags};
use alacritty_terminal::term::TermMode;
use alacritty_terminal::vte::ansi::{Color as TermColor, NamedColor};
use futures::StreamExt;
use gpui::prelude::*;
use gpui::{
    black, canvas, div, fill, font, point, px, size, App, Bounds, ClipboardItem, Context,
    FocusHandle, Font, FontId, GlyphId, Hsla, IntoElement, KeyDownEvent, MouseButton,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, Render, Rgba, ScrollWheelEvent,
    SharedString, TextRun, Window,
};
use uuid::Uuid;

use rterm_ssh::pty::{self, PtyCommand, PtyEvent};
use rterm_ssh::reconnect::ReconnectPolicy;
use rterm_ssh::SshSession;
use rterm_terminal::{TerminalEngine, TerminalKey};

use crate::event::{AppEvent, EventBusHost};
use crate::state::{AppState, CursorStyle};
use crate::theme::TerminalPalette;
use crate::ui::theme as ui_theme;

use super::{CONTENT_BORDER_W, SIDEBAR_W, STATUS_BAR_H, TAB_BAR_H, TERMINAL_HEADER_H, TOOLBAR_H};

/// PTY 输出有界队列容量：队列满时后台线程阻塞等待 UI 消费（背压）。
const PTY_OUTPUT_QUEUE_CAPACITY: usize = 256;

// ---------------------------------------------------------------------------
// 状态
// ---------------------------------------------------------------------------

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

/// Canvas 在窗口中的实际位置与字符网格尺寸（鼠标事件坐标换算用）。
#[derive(Clone, Copy, Default)]
struct ViewMetrics {
    bounds: Option<Bounds<Pixels>>,
    cell_w: f32,
    line_h: f32,
}

// ---------------------------------------------------------------------------
// 渲染快照（render 时从 Grid 抽取，paint 闭包只读快照）
// ---------------------------------------------------------------------------

#[derive(Clone, Copy)]
struct SnapshotCell {
    ch: char,
    fg: Rgba,
    bg: Rgba,
    flags: Flags,
}

struct Snapshot {
    cols: usize,
    rows: usize,
    cells: Vec<SnapshotCell>,
    cursor: Option<(usize, usize)>,
    cursor_style: CursorStyle,
    palette: TerminalPalette,
    cell_w: f32,
    line_h: f32,
    font_size: f32,
    font_family: String,
    /// 已解析的主字体，避免 paint 时再次 resolve_font
    font_id: FontId,
    scroll_offset: usize,
    scroll_total: usize,
    /// 真实历史行数（= 最大 display_offset），0 时不画滚动条
    scroll_history: usize,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct GlyphKey {
    ch: char,
    bold: bool,
    italic: bool,
}

#[derive(Clone, Copy)]
struct CachedGlyph {
    font_id: FontId,
    glyph_id: GlyphId,
    is_emoji: bool,
    y_offset: f32,
}

type GlyphCache = Rc<RefCell<HashMap<GlyphKey, Option<CachedGlyph>>>>;

// ---------------------------------------------------------------------------
// TerminalView
// ---------------------------------------------------------------------------

pub struct TerminalView {
    focus_handle: FocusHandle,
    engine: TerminalEngine,
    cols: u16,
    rows: u16,
    scrollback: usize,
    font_family: String,
    font_size: f32,
    cell_w: f32,
    line_height: f32,
    /// 当前字体家族解析出的 FontId 缓存，避免每帧 `resolve_font`
    font_id: Option<FontId>,
    cursor_style: CursorStyle,
    palette: TerminalPalette,
    glyph_cache: GlyphCache,
    metrics: Rc<RefCell<ViewMetrics>>,

    status: TerminalStatus,
    session: Option<rterm_session::Session>,
    session_id: Option<Uuid>,
    tab_id: Uuid,
    input_tx: Option<Sender<PtyCommand>>,
    /// 终端内部应答（DSR / DA 等 PtyWrite）通道，每次喂数据后 drain 写回 PTY
    response_rx: Option<Receiver<Vec<u8>>>,
    generation: u64,
    reconnect_attempt: u8,
    autofocus_pending: bool,

    /// 选区锚点 / 头部（视口坐标 row, col）
    selection: Option<((usize, usize), (usize, usize))>,
    selecting: bool,
    blink_on: bool,

    attempt_started: Option<Instant>,
    last_latency_ms: Option<u64>,
}

impl TerminalView {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let (cols, rows, scrollback, font_family, font_size, cursor_style, theme_name) = {
            let state = cx.global::<AppState>();
            let settings = &state.settings;
            (
                100_u16,
                24_u16,
                settings.terminal_scrollback_lines,
                settings.default_font_family.clone(),
                settings.default_font_size,
                settings.terminal_cursor_style.clone(),
                state.theme.name().to_string(),
            )
        };

        // 主题 / 设置变更 → 即时重绘
        let host = cx.global::<AppState>().event_bus_host.clone();
        cx.subscribe(&host, |this: &mut Self, _host, event: &AppEvent, cx| {
            match event {
                AppEvent::AppThemeChanged { name } => {
                    this.palette = crate::theme::terminal_palette(name);
                    this.glyph_cache.borrow_mut().clear();
                    cx.notify();
                }
                AppEvent::AppSettingsChanged => {
                    this.apply_settings(cx);
                }
                _ => {}
            }
        })
        .detach();

        // 光标 blink：500ms 翻转一次
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(500))
                    .await;
                if this
                    .update(cx, |this, cx| {
                        this.blink_on = !this.blink_on;
                        cx.notify();
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();

        let (response_tx, response_rx) = std::sync::mpsc::channel::<Vec<u8>>();
        Self {
            focus_handle: cx.focus_handle(),
            engine: TerminalEngine::with_response_sender(
                cols as usize,
                rows as usize,
                scrollback,
                response_tx,
            ),
            cols,
            rows,
            scrollback,
            font_family,
            font_size,
            cell_w: font_size * 0.6,
            line_height: font_size * 1.35,
            font_id: None,
            cursor_style,
            palette: crate::theme::terminal_palette(&theme_name),
            glyph_cache: Rc::new(RefCell::new(HashMap::new())),
            metrics: Rc::new(RefCell::new(ViewMetrics::default())),
            status: TerminalStatus::Idle,
            session: None,
            session_id: None,
            tab_id: Uuid::new_v4(),
            input_tx: None,
            response_rx: Some(response_rx),
            generation: 0,
            reconnect_attempt: 0,
            autofocus_pending: false,
            selection: None,
            selecting: false,
            blink_on: true,
            attempt_started: None,
            last_latency_ms: None,
        }
    }

    pub fn tab_id(&self) -> Uuid {
        self.tab_id
    }

    pub fn focus_handle(&self) -> FocusHandle {
        self.focus_handle.clone()
    }

    pub fn session_label(&self) -> Option<String> {
        self.session.as_ref().map(|session| session.display_label())
    }

    fn apply_settings(&mut self, cx: &mut Context<Self>) {
        let (font_family, font_size, cursor_style) = {
            let settings = &cx.global::<AppState>().settings;
            (
                settings.default_font_family.clone(),
                settings.default_font_size,
                settings.terminal_cursor_style.clone(),
            )
        };

        if self.font_family != font_family || (self.font_size - font_size).abs() > f32::EPSILON {
            self.font_family = font_family;
            self.font_size = font_size;
            self.font_id = None;
            self.glyph_cache.borrow_mut().clear();
        }
        self.cursor_style = cursor_style;
        cx.notify();
    }

    // -----------------------------------------------------------------------
    // 连接编排
    // -----------------------------------------------------------------------

    pub fn connect(&mut self, session_id: Uuid, cx: &mut Context<Self>) {
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
        self.selection = None;
        // 新连接：重置本地引擎（清空历史）并重建应答通道。
        // 重连走 start_attempt，不重置，保留已有 scrollback。
        self.reset_engine();
        self.status = TerminalStatus::Connecting;
        self.start_attempt(cx);
        cx.notify();
    }

    /// 新建本地引擎并重建终端应答通道（仅新连接时调用）。
    fn reset_engine(&mut self) {
        let (response_tx, response_rx) = std::sync::mpsc::channel::<Vec<u8>>();
        self.engine = TerminalEngine::with_response_sender(
            self.cols as usize,
            self.rows as usize,
            self.scrollback,
            response_tx,
        );
        self.response_rx = Some(response_rx);
    }

    /// 关闭 Tab 时调用：通知后台线程退出 + 丢弃 Engine。
    pub fn disconnect(&mut self, cx: &mut Context<Self>) {
        self.generation = self.generation.wrapping_add(1);
        if let Some(tx) = self.input_tx.take() {
            let _ = tx.send(PtyCommand::Close);
        }
        self.status = TerminalStatus::Idle;
        self.selection = None;
        self.emit(cx, AppEvent::TerminalClosed { tab_id: self.tab_id });
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
        self.selection = None;
        self.attempt_started = Some(Instant::now());
        // 不重建 engine：重连时保留历史，字号 / 尺寸已通过 resize 同步。

        // 有界队列：UI 消费不及时时后台线程阻塞等待，避免 `cat 50MB` 撑爆内存
        let (output_tx, mut output_rx) =
            futures::channel::mpsc::channel::<PtyEvent>(PTY_OUTPUT_QUEUE_CAPACITY);
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

        cx.spawn(async move |this, cx| {
            while let Some(event) = output_rx.next().await {
                if this
                    .update(cx, |this, cx| this.handle_pty_event(generation, event, cx))
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
                if let Some(started) = self.attempt_started.take() {
                    let latency = started.elapsed().as_millis() as u64;
                    self.last_latency_ms = Some(latency);
                    if let Some(session_id) = self.session_id {
                        self.emit(cx, AppEvent::SessionLatency { session_id, latency_ms: latency });
                    }
                }
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
                // DSR / DA 等查询会通过 Event::PtyWrite 产生应答，写回 PTY
                self.drain_engine_responses();
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
    // 尺寸 / 字体
    // -----------------------------------------------------------------------

    fn measure_font(&mut self, window: &Window) -> FontId {
        let text_system = window.text_system();
        // FontId 只随字体家族变化，缓存后避免每帧 resolve_font
        let font_id = match self.font_id {
            Some(font_id) => font_id,
            None => {
                let font_id = text_system.resolve_font(&font(self.font_family.clone()));
                self.font_id = Some(font_id);
                font_id
            }
        };
        let cell_w = text_system
            .advance(font_id, px(self.font_size), 'M')
            .map(|advance| f32::from(advance.width))
            .unwrap_or(self.font_size * 0.6);
        self.cell_w = cell_w.max(1.0);
        self.line_height = (self.font_size * 1.35).max(self.font_size + 2.0);
        font_id
    }

    /// 窗口尺寸变化 → 重算 cols/rows → Engine + PTY 同步。
    ///
    /// 仅在 render 期间调用；不 emit / notify，避免事件订阅者重入。
    fn sync_grid_size(&mut self, window: &Window) {
        let viewport = window.viewport_size();
        let available_w = (f32::from(viewport.width) - SIDEBAR_W - CONTENT_BORDER_W).max(1.0);
        let available_h = (f32::from(viewport.height)
            - TOOLBAR_H
            - STATUS_BAR_H
            - TAB_BAR_H
            - TERMINAL_HEADER_H)
            .max(1.0);

        let cols = ((available_w / self.cell_w).floor() as usize).clamp(1, 500);
        let rows = ((available_h / self.line_height).floor() as usize).clamp(1, 300);

        if cols == self.cols as usize && rows == self.rows as usize {
            return;
        }

        self.cols = cols as u16;
        self.rows = rows as u16;
        self.engine.resize(cols, rows);
        self.selection = None;
        log::info!("[terminal] grid resize → {cols}x{rows}");

        if let Some(tx) = &self.input_tx {
            let _ = tx.send(PtyCommand::Resize {
                cols: cols as u32,
                rows: rows as u32,
            });
        }
    }

    fn change_font_size(&mut self, delta: f32, cx: &mut Context<Self>) {
        let new_size = if delta == 0.0 {
            14.0
        } else {
            (self.font_size + delta).clamp(8.0, 40.0)
        };
        if (new_size - self.font_size).abs() < f32::EPSILON {
            return;
        }
        self.font_size = new_size;
        self.glyph_cache.borrow_mut().clear();

        {
            let state = cx.global_mut::<AppState>();
            state.settings.default_font_size = new_size;
            if let Err(error) = state.settings.save() {
                log::warn!("[terminal] 保存字号失败: {error:#}");
            }
        }
        self.emit(cx, AppEvent::AppSettingsChanged);
        log::info!("[terminal] font size → {new_size}");
        cx.notify();
    }

    // -----------------------------------------------------------------------
    // 输入（键盘 / 鼠标 / 粘贴）
    // -----------------------------------------------------------------------

    fn send_bytes(&mut self, bytes: Vec<u8>) {
        if let Some(tx) = &self.input_tx {
            let _ = tx.send(PtyCommand::Write(bytes));
        }
    }

    /// 把终端内部应答（DSR / DA 等 PtyWrite）写回 PTY。
    /// 先收集再发送，避免同时借用 `response_rx` 与 `send_bytes`。
    fn drain_engine_responses(&mut self) {
        let mut pending = Vec::new();
        let mut disconnected = false;
        if let Some(rx) = &self.response_rx {
            loop {
                match rx.try_recv() {
                    Ok(bytes) => pending.push(bytes),
                    Err(std::sync::mpsc::TryRecvError::Empty) => break,
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                        disconnected = true;
                        break;
                    }
                }
            }
        }
        if disconnected {
            self.response_rx = None;
        }
        for bytes in pending {
            self.send_bytes(bytes);
        }
    }

    fn on_key(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) {
        let keystroke = &event.keystroke;
        let modifiers = keystroke.modifiers;

        // 字号缩放：Ctrl+= / Ctrl+- / Ctrl+0（不依赖连接状态）
        if modifiers.control && !modifiers.alt {
            match keystroke.key.as_str() {
                "=" | "+" => {
                    self.change_font_size(1.0, cx);
                    return;
                }
                "-" => {
                    self.change_font_size(-1.0, cx);
                    return;
                }
                "0" => {
                    self.change_font_size(0.0, cx);
                    return;
                }
                // 这些组合交给 MainWindow 的全局 keybinding
                "t" | "w" | "tab" | "," => return,
                _ => {}
            }
        }

        if self.status != TerminalStatus::Connected {
            return;
        }

        // Shift+PageUp / PageDown：回滚
        if modifiers.shift && !modifiers.control && !modifiers.alt {
            match keystroke.key.as_str() {
                "pageup" => {
                    self.engine.scroll_page(true);
                    self.selection = None;
                    cx.notify();
                    return;
                }
                "pagedown" => {
                    self.engine.scroll_page(false);
                    self.selection = None;
                    cx.notify();
                    return;
                }
                _ => {}
            }
        }

        let mut key = None;

        // Ctrl 组合（方向键优先，其次 Ctrl+字母）
        if modifiers.control && !modifiers.alt && !modifiers.platform {
            key = match keystroke.key.as_str() {
                "up" => Some(TerminalKey::CtrlUp),
                "down" => Some(TerminalKey::CtrlDown),
                "left" => Some(TerminalKey::CtrlLeft),
                "right" => Some(TerminalKey::CtrlRight),
                _ => None,
            };
            if key.is_none() && keystroke.key.len() == 1 {
                if let Some(ch) = keystroke.key.chars().next() {
                    key = Some(TerminalKey::Ctrl(ch));
                }
            }
        }

        // Alt 组合 → ESC + char（Alt 在 Windows 上常被系统吃掉，尽力而为）
        if key.is_none() && modifiers.alt && !modifiers.control && !modifiers.platform {
            if let Some(text) = &keystroke.key_char {
                if !text.is_empty() {
                    key = Some(TerminalKey::Alt(text.clone()));
                }
            }
            if key.is_none() && keystroke.key.len() == 1 {
                key = Some(TerminalKey::Alt(keystroke.key.clone()));
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
                key if is_function_key(key) => key[1..].parse::<u8>().ok().map(TerminalKey::F),
                _ => None,
            };
        }

        if key.is_none()
            && !modifiers.control
            && !modifiers.alt
            && !modifiers.platform
        {
            if let Some(text) = &keystroke.key_char {
                if !text.is_empty() {
                    key = Some(TerminalKey::Char(text.clone()));
                }
            }
        }

        if let Some(key) = key {
            let bytes = self.engine.input_key(key);
            self.send_bytes(bytes);
            self.engine.scroll_bottom();
            self.selection = None;
            cx.notify();
        }
    }

    fn on_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.focus(&self.focus_handle, cx);

        match event.button {
            MouseButton::Left => {
                if let Some(cell) = self.cell_at_position(event.position, false) {
                    self.selection = Some((cell, cell));
                    self.selecting = true;
                } else {
                    self.selection = None;
                    self.selecting = false;
                }
                cx.notify();
            }
            MouseButton::Right => {
                self.paste_clipboard(cx);
            }
            _ => {}
        }
    }

    fn on_mouse_move(&mut self, event: &MouseMoveEvent, cx: &mut Context<Self>) {
        if !self.selecting {
            return;
        }
        if !event.dragging() {
            return;
        }
        if let Some(cell) = self.cell_at_position(event.position, true) {
            if let Some((_, head)) = &mut self.selection {
                if *head != cell {
                    *head = cell;
                    cx.notify();
                }
            }
        }
    }

    fn on_mouse_up(&mut self, event: &MouseUpEvent, cx: &mut Context<Self>) {
        if event.button != MouseButton::Left || !self.selecting {
            return;
        }
        self.selecting = false;

        if let Some((anchor, head)) = self.selection {
            if anchor != head {
                let text = self.engine.text_in_range(anchor, head);
                if !text.is_empty() {
                    let chars = text.chars().count();
                    cx.write_to_clipboard(ClipboardItem::new_string(text));
                    log::info!("[terminal] copied selection ({chars} chars)");
                }
            }
        }
        cx.notify();
    }

    fn on_scroll_wheel(&mut self, event: &ScrollWheelEvent, cx: &mut Context<Self>) {
        if self.status != TerminalStatus::Connected {
            return;
        }
        let pixel = event.delta.pixel_delta(px(self.line_height));
        let dy = f32::from(pixel.y);
        let lines = if event.delta.precise() {
            (dy / self.line_height).round() as i32
        } else if dy.abs() > 0.1 {
            3 * dy.signum() as i32
        } else {
            0
        };
        if lines != 0 {
            self.engine.scroll_display(lines);
            self.selection = None;
            cx.notify();
        }
    }

    fn paste_clipboard(&mut self, cx: &mut Context<Self>) {
        if self.status != TerminalStatus::Connected {
            return;
        }
        let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) else {
            return;
        };
        if text.is_empty() {
            return;
        }

        let payload = if self.engine.bracketed_paste() {
            let mut payload = String::with_capacity(text.len() + 12);
            payload.push_str("\x1b[200~");
            payload.push_str(&text);
            payload.push_str("\x1b[201~");
            payload
        } else {
            text
        };
        self.send_bytes(payload.into_bytes());
        self.engine.scroll_bottom();
        self.selection = None;
        cx.notify();
    }

    /// 窗口坐标 → 视口 cell。`clamp = true` 时拖到区域外自动贴边。
    fn cell_at_position(
        &self,
        position: gpui::Point<Pixels>,
        clamp: bool,
    ) -> Option<(usize, usize)> {
        let metrics = *self.metrics.borrow();
        let bounds = metrics.bounds?;
        let x = f32::from(position.x) - f32::from(bounds.left());
        let y = f32::from(position.y) - f32::from(bounds.top());

        if !clamp && (x < 0.0 || y < 0.0) {
            return None;
        }

        let col = (x / metrics.cell_w.max(1.0)).floor();
        let row = (y / metrics.line_h.max(1.0)).floor();
        let col = col.clamp(0.0, self.cols.saturating_sub(1) as f32) as usize;
        let row = row.clamp(0.0, self.rows.saturating_sub(1) as f32) as usize;
        Some((row, col))
    }

    // -----------------------------------------------------------------------
    // 快照
    // -----------------------------------------------------------------------

    fn palette_color(&self, color: TermColor, default: Rgba) -> Rgba {
        match color {
            TermColor::Spec(rgb) => Rgba {
                r: rgb.r as f32 / 255.0,
                g: rgb.g as f32 / 255.0,
                b: rgb.b as f32 / 255.0,
                a: 1.0,
            },
            TermColor::Indexed(index) => self.indexed_color(index),
            TermColor::Named(named) => match named {
                NamedColor::Foreground => self.palette.fg,
                NamedColor::Background => self.palette.bg,
                NamedColor::Cursor => self.palette.cursor,
                NamedColor::BrightForeground => self.palette.ansi16[15],
                _ => {
                    let index = named as usize;
                    if index < 16 {
                        self.palette.ansi16[index]
                    } else {
                        default
                    }
                }
            },
        }
    }

    fn indexed_color(&self, index: u8) -> Rgba {
        match index {
            0..=15 => self.palette.ansi16[index as usize],
            16..=231 => {
                let i = index as u32 - 16;
                let steps = [0u8, 95, 135, 175, 215, 255];
                Rgba {
                    r: steps[(i / 36) as usize] as f32 / 255.0,
                    g: steps[((i % 36) / 6) as usize] as f32 / 255.0,
                    b: steps[(i % 6) as usize] as f32 / 255.0,
                    a: 1.0,
                }
            }
            232..=255 => {
                let level = 8 + (index as u32 - 232) * 10;
                let value = level as f32 / 255.0;
                Rgba {
                    r: value,
                    g: value,
                    b: value,
                    a: 1.0,
                }
            }
        }
    }

    fn build_snapshot(&self, font_id: FontId) -> Snapshot {
        let grid = self.engine.grid();
        let display_offset = grid.display_offset() as i32;
        let cols = self.cols as usize;
        let rows = self.rows as usize;

        let selection = self.selection.map(|(a, b)| if a <= b { (a, b) } else { (b, a) });
        let selection_bg = self.palette.selection_bg();

        let mut cells = Vec::with_capacity(cols * rows);
        for row in 0..rows {
            let line = Line(row as i32 - display_offset);
            for col in 0..cols {
                let cell: &Cell = &grid[line][Column(col)];
                let mut fg = self.palette_color(cell.fg, self.palette.fg);
                let mut bg = self.palette_color(cell.bg, self.palette.bg);
                if cell.flags.contains(Flags::INVERSE) {
                    std::mem::swap(&mut fg, &mut bg);
                }

                let selected = selection
                    .map(|(start, end)| {
                        (start.0, start.1) <= (row, col) && (row, col) <= (end.0, end.1)
                    })
                    .unwrap_or(false);
                if selected {
                    bg = selection_bg;
                }

                cells.push(SnapshotCell {
                    ch: cell.c,
                    fg,
                    bg,
                    flags: cell.flags,
                });
            }
        }

        let mode = self.engine.mode();
        let (cursor_row, cursor_col) = self.engine.cursor_position();
        let cursor = if self.blink_on
            && mode.contains(TermMode::SHOW_CURSOR)
            && cursor_row < rows
            && cursor_col < cols
        {
            Some((cursor_row, cursor_col))
        } else {
            None
        };

        let history = self.engine.history_size();
        Snapshot {
            cols,
            rows,
            cells,
            cursor,
            cursor_style: self.cursor_style.clone(),
            palette: self.palette,
            cell_w: self.cell_w,
            line_h: self.line_height,
            font_size: self.font_size,
            font_family: self.font_family.clone(),
            font_id,
            scroll_offset: self.engine.display_offset(),
            // 用 Grid 真实历史行数，而不是配置上限
            scroll_total: history + rows,
            scroll_history: history,
        }
    }
}

fn is_function_key(key: &str) -> bool {
    let Some(number) = key.strip_prefix('f') else {
        return false;
    };
    matches!(number.parse::<u8>(), Ok(1..=12))
}

// ---------------------------------------------------------------------------
// Canvas 绘制
// ---------------------------------------------------------------------------

fn paint_snapshot(
    snapshot: &Snapshot,
    bounds: Bounds<Pixels>,
    window: &mut Window,
    _cx: &mut App,
    glyph_cache: &GlyphCache,
) {
    let palette = snapshot.palette;
    let cell_w = snapshot.cell_w;
    let line_h = snapshot.line_h;
    let font_size = px(snapshot.font_size);

    // 1. 整块背景
    let bg: Hsla = palette.bg.into();
    window.paint_quad(fill(bounds, bg));

    // 2. 逐 Cell 背景（同色横向合并，减少 quad 数量）
    {
        let mut paint_row = |row: usize, start: usize, end: usize, color: Rgba| {
            if end > start && color != palette.bg {
                let origin = point(
                    bounds.left() + px(start as f32 * cell_w),
                    bounds.top() + px(row as f32 * line_h),
                );
                let cell_size = size(px((end - start) as f32 * cell_w), px(line_h));
                let color: Hsla = color.into();
                window.paint_quad(fill(Bounds::new(origin, cell_size), color));
            }
        };

        for row in 0..snapshot.rows {
            let mut run_start = 0usize;
            let mut run_color = snapshot.cells[row * snapshot.cols].bg;
            for col in 1..=snapshot.cols {
                let color = if col < snapshot.cols {
                    snapshot.cells[row * snapshot.cols + col].bg
                } else {
                    // 哨兵：强制 flush 最后一段
                    Rgba {
                        r: -1.0,
                        g: -1.0,
                        b: -1.0,
                        a: -1.0,
                    }
                };
                if color != run_color {
                    paint_row(row, run_start, col, run_color);
                    run_start = col;
                    run_color = color;
                }
            }
        }
    }

    // 3. 光标（方块 / 下划线 / 竖线）
    if let Some((row, col)) = snapshot.cursor {
        let origin = point(
            bounds.left() + px(col as f32 * cell_w),
            bounds.top() + px(row as f32 * line_h),
        );
        let cursor: Hsla = palette.cursor.into();
        match &snapshot.cursor_style {
            CursorStyle::Block => {
                window.paint_quad(fill(Bounds::new(origin, size(px(cell_w), px(line_h))), cursor));
            }
            CursorStyle::Underline => {
                window.paint_quad(fill(
                    Bounds::new(
                        point(origin.x, origin.y + px(line_h - 2.0)),
                        size(px(cell_w), px(2.0)),
                    ),
                    cursor,
                ));
            }
            CursorStyle::Bar => {
                window.paint_quad(fill(
                    Bounds::new(origin, size(px(2.0), px(line_h))),
                    cursor,
                ));
            }
        }
    }

    // 4. 字形
    let text_system = window.text_system().clone();
    let primary_font = font(SharedString::from(snapshot.font_family.clone()));
    let primary_font_id = snapshot.font_id;
    let ascent = f32::from(text_system.ascent(primary_font_id, font_size));
    let descent = f32::from(text_system.descent(primary_font_id, font_size));
    let baseline_padding = ((f32::from(line_h) - ascent - descent) / 2.0).max(0.0);
    let baseline = baseline_padding + ascent;

    for row in 0..snapshot.rows {
        for col in 0..snapshot.cols {
            let cell = &snapshot.cells[row * snapshot.cols + col];
            if cell.ch == ' '
                || cell.flags.contains(Flags::HIDDEN)
                || cell
                    .flags
                    .intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER)
            {
                continue;
            }

            let bold = cell.flags.contains(Flags::BOLD);
            let italic = cell.flags.contains(Flags::ITALIC);
            let Some(glyph) = cached_glyph(
                glyph_cache,
                &text_system,
                &primary_font,
                font_size,
                cell.ch,
                bold,
                italic,
            ) else {
                continue;
            };

            let origin = point(
                bounds.left() + px(col as f32 * cell_w),
                bounds.top() + px(row as f32 * line_h + baseline + glyph.y_offset),
            );
            if glyph.is_emoji {
                let _ = window.paint_emoji(origin, glyph.font_id, glyph.glyph_id, font_size);
            } else {
                let color: Hsla = cell.fg.into();
                let _ = window.paint_glyph(origin, glyph.font_id, glyph.glyph_id, font_size, color);
            }

            // 下划线 / 删除线
            if cell.flags.contains(Flags::UNDERLINE) {
                let underline: Hsla = cell.fg.into();
                window.paint_quad(fill(
                    Bounds::new(
                        point(
                            bounds.left() + px(col as f32 * cell_w),
                            bounds.top() + px((row + 1) as f32 * line_h - 2.0),
                        ),
                        size(px(cell_w), px(1.0)),
                    ),
                    underline,
                ));
            }
            if cell.flags.contains(Flags::STRIKEOUT) {
                let strike: Hsla = cell.fg.into();
                window.paint_quad(fill(
                    Bounds::new(
                        point(
                            bounds.left() + px(col as f32 * cell_w),
                            bounds.top() + px(row as f32 * line_h + line_h * 0.55),
                        ),
                        size(px(cell_w), px(1.0)),
                    ),
                    strike,
                ));
            }
        }
    }

    // 5. 滚动条（仅当有真实历史时）
    let viewport_lines = snapshot.rows;
    let history = snapshot.scroll_history;
    if history > 0 {
        let total_lines = snapshot.scroll_total.max(viewport_lines).max(1);
        let track_x = bounds.right() - px(8.0);
        let track_top = bounds.top() + px(2.0);
        let track_h = f32::from(bounds.size.height) - 4.0;
        let visible = viewport_lines as f32 / total_lines as f32;
        let thumb_h = (track_h * visible).max(20.0);
        // progress 基于真实最大 display_offset（历史行数）
        let max_offset = history as f32;
        let progress = (snapshot.scroll_offset.min(history) as f32 / max_offset).clamp(0.0, 1.0);
        // offset 越大（越往历史）thumb 越靠上
        let thumb_top = track_top + px((track_h - thumb_h) * (1.0 - progress));

        let track_color: Hsla = palette.fg.opacity(0.08).into();
        window.paint_quad(fill(
            Bounds::new(
                point(track_x, track_top),
                size(px(6.0), px(track_h)),
            ),
            track_color,
        ));
        let thumb_color: Hsla = palette.fg.opacity(0.35).into();
        window.paint_quad(fill(
            Bounds::new(
                point(track_x, thumb_top),
                size(px(6.0), px(thumb_h)),
            ),
            thumb_color,
        ));
    }
}

fn cached_glyph(
    cache: &GlyphCache,
    text_system: &gpui::WindowTextSystem,
    primary_font: &Font,
    font_size: Pixels,
    ch: char,
    bold: bool,
    italic: bool,
) -> Option<CachedGlyph> {
    let key = GlyphKey { ch, bold, italic };
    if let Some(cached) = cache.borrow().get(&key) {
        return *cached;
    }

    let mut font = primary_font.clone();
    if bold {
        font = font.bold();
    }
    if italic {
        font = font.italic();
    }
    let mut buffer = [0u8; 4];
    let text: &str = ch.encode_utf8(&mut buffer);
    let run = TextRun {
        len: text.len(),
        font,
        color: black(),
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    let shaped = text_system.shape_line(
        SharedString::from(text.to_string()),
        font_size,
        &[run],
        None,
    );
    let glyph = shaped.runs.iter().flat_map(|run| run.glyphs.iter()).next();
    let cached = glyph.map(|glyph| CachedGlyph {
        font_id: shaped.runs[0].font_id,
        glyph_id: glyph.id,
        is_emoji: glyph.is_emoji,
        y_offset: f32::from(glyph.position.y),
    });
    cache.borrow_mut().insert(key, cached);
    cached
}

// ---------------------------------------------------------------------------
// Render
// ---------------------------------------------------------------------------

impl Render for TerminalView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = ui_theme(cx);

        if self.status == TerminalStatus::Connected && self.autofocus_pending {
            window.focus(&self.focus_handle, cx);
            self.autofocus_pending = false;
        }

        // 字体度量 → 窗口尺寸 → cols/rows（render 内只更新自身状态，不 emit/notify）
        let font_id = self.measure_font(window);
        self.sync_grid_size(window);

        let title = self
            .session
            .as_ref()
            .map(|session| session.display_label())
            .unwrap_or_else(|| "终端".to_string());
        let status = match (&self.status, self.last_latency_ms) {
            (TerminalStatus::Connected, Some(latency)) => {
                format!("已连接 · {latency} ms")
            }
            _ => self.status.label(),
        };
        let palette_bg = self.palette.bg;

        let mut root = div()
            .size_full()
            .bg(palette_bg)
            .flex()
            .flex_col()
            .id("rterm-terminal-view")
            .track_focus(&self.focus_handle)
            .key_context("Terminal")
            .on_key_down(cx.listener(|this, event, _window, cx| this.on_key(event, cx)))
            .child(
                div()
                    .w_full()
                    .h(px(TERMINAL_HEADER_H))
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
                            .child("← 双击会话或 Ctrl+T 新建终端标签"),
                    )
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(t.text_placeholder())
                            .child("Sprint 6 · Canvas 终端渲染"),
                    ),
            );
        } else {
            let snapshot = self.build_snapshot(font_id);
            let metrics = self.metrics.clone();
            let cache = self.glyph_cache.clone();

            let canvas_element = canvas(
                move |bounds, _window, _cx| {
                    let mut metrics = metrics.borrow_mut();
                    metrics.bounds = Some(bounds);
                },
                move |bounds, _state, window, cx| {
                    paint_snapshot(&snapshot, bounds, window, cx, &cache);
                },
            )
            .size_full();

            let body = div()
                .w_full()
                .flex_1()
                .relative()
                .id("rterm-terminal-canvas")
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, event: &MouseDownEvent, window, cx| {
                        this.on_mouse_down(event, window, cx);
                    }),
                )
                .on_mouse_down(
                    MouseButton::Right,
                    cx.listener(|this, event: &MouseDownEvent, window, cx| {
                        this.on_mouse_down(event, window, cx);
                    }),
                )
                .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _window, cx| {
                    this.on_mouse_move(event, cx);
                }))
                .on_mouse_up(
                    MouseButton::Left,
                    cx.listener(|this, event: &MouseUpEvent, _window, cx| {
                        this.on_mouse_up(event, cx);
                    }),
                )
                .on_scroll_wheel(cx.listener(
                    |this, event: &ScrollWheelEvent, _window, cx| {
                        this.on_scroll_wheel(event, cx);
                    },
                ))
                .child(canvas_element);

            root = root.child(body);

            if let TerminalStatus::Failed(reason) = &self.status {
                let danger = t.danger();
                root = root.child(
                    div()
                        .w_full()
                        .h(px(24.))
                        .px_2()
                        .text_size(px(11.))
                        .text_color(danger)
                        .child(format!("连接失败: {reason}")),
                );
            }
        }

        root
    }
}

/// 后台线程：连接 → 打开 PTY → 进入 IO 循环。
fn run_connection(
    config: rterm_session::Session,
    cols: u16,
    rows: u16,
    mut output: futures::channel::mpsc::Sender<PtyEvent>,
    commands: Receiver<PtyCommand>,
) {
    let mut ssh = SshSession::new();

    if let Err(error) = ssh.connect(&config) {
        if let Some(info) = ssh.last_host_key() {
            if !pty::send_event(&mut output, PtyEvent::HostKey(info.clone())) {
                return;
            }
        }
        let reason = format!("{error:#}");
        let retryable = connect_error_is_retryable(&reason);
        let _ = pty::send_event(&mut output, PtyEvent::Failed { reason, retryable });
        return;
    }

    if let Some(info) = ssh.last_host_key() {
        if !pty::send_event(&mut output, PtyEvent::HostKey(info.clone())) {
            return;
        }
    }

    let channel = match ssh.open_pty(cols as u32, rows as u32) {
        Ok(channel) => channel,
        Err(error) => {
            let _ = pty::send_event(
                &mut output,
                PtyEvent::Failed {
                    reason: format!("打开 PTY 失败: {error:#}"),
                    retryable: true,
                },
            );
            return;
        }
    };

    let Some(session) = ssh.session_clone() else {
        let _ = pty::send_event(
            &mut output,
            PtyEvent::Failed {
                reason: "SSH Session 丢失".into(),
                retryable: false,
            },
        );
        return;
    };

    if !pty::send_event(&mut output, PtyEvent::Connected) {
        return;
    }
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

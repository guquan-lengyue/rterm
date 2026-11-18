//! StatusBar — 底部状态栏（Sprint 7）
//!
//! 左侧：临时消息（设置已保存 / 会话已删除…）
//! 右侧：当前主机 · 连接延迟 · 重连次数 · 时钟

use std::collections::HashMap;
use std::time::{Duration, Instant};

use gpui::prelude::*;
use gpui::{div, px, Context, IntoElement, Render, Window};
use uuid::Uuid;

use crate::event::AppEvent;
use crate::state::{lock_or_recover, AppState};
use crate::ui::theme;

use super::STATUS_BAR_H;

/// 无消息时的默认文案。
const READY_MESSAGE: &str = "就绪";

pub struct StatusBar {
    pub message: String,
    /// 临时消息（StatusBarMessage）的过期时间；None 表示持久消息。
    message_expires_at: Option<Instant>,
    /// 当前活动 Tab 对应的会话：右侧主机/延迟/重连按它展示。
    active_session: Option<Uuid>,
    hosts: HashMap<Uuid, String>,
    latencies: HashMap<Uuid, u64>,
    reconnects: HashMap<Uuid, u8>,
    clock: String,
}

impl StatusBar {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let host = cx.global::<AppState>().event_bus_host.clone();
        cx.subscribe(&host, |this: &mut Self, _host, event: &AppEvent, cx| {
            match event {
                AppEvent::SessionConnected { session_id } => {
                    let label = {
                        let state = cx.global::<AppState>();
                        let store = lock_or_recover(&state.session_store);
                        store
                            .get(*session_id)
                            .map(|session| session.display_label())
                    };
                    if let Some(label) = label {
                        this.hosts.insert(*session_id, label);
                    }
                    this.reconnects.remove(session_id);
                    if this.active_session.is_none() {
                        this.active_session = Some(*session_id);
                    }
                    this.message = "已连接".into();
                    // 连接状态属于持久消息，不参与自动清除
                    this.message_expires_at = None;
                    cx.notify();
                }
                AppEvent::StatusBarMessage { text, duration_ms } => {
                    this.message = text.clone();
                    // duration_ms = 0 视为持久消息
                    this.message_expires_at = if *duration_ms == 0 {
                        None
                    } else {
                        Some(Instant::now() + Duration::from_millis(*duration_ms))
                    };
                    cx.notify();
                }
                AppEvent::SessionLatency {
                    session_id,
                    latency_ms,
                } => {
                    this.latencies.insert(*session_id, *latency_ms);
                    cx.notify();
                }
                AppEvent::SessionReconnecting {
                    session_id,
                    attempt,
                } => {
                    this.reconnects.insert(*session_id, *attempt);
                    this.message = format!("重连中（第 {attempt} 次）…");
                    this.message_expires_at = None;
                    cx.notify();
                }
                AppEvent::SessionDisconnected { session_id, reason } => {
                    this.latencies.remove(session_id);
                    this.reconnects.remove(session_id);
                    this.message = format!("已断开：{reason}");
                    this.message_expires_at = None;
                    cx.notify();
                }
                AppEvent::SessionFailed { session_id, reason } => {
                    this.latencies.remove(session_id);
                    this.reconnects.remove(session_id);
                    this.message = format!("连接失败：{reason}");
                    this.message_expires_at = None;
                    cx.notify();
                }
                AppEvent::SessionDeleted { session_id } => {
                    this.hosts.remove(session_id);
                    this.latencies.remove(session_id);
                    this.reconnects.remove(session_id);
                    if this.active_session == Some(*session_id) {
                        this.active_session = None;
                    }
                    cx.notify();
                }
                _ => {}
            }
        })
        .detach();

        // 时钟：每秒刷新
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_secs(1)).await;
                let now = chrono::Local::now().format("%H:%M:%S").to_string();
                if this
                    .update(cx, |this, cx| {
                        this.clock = now;
                        // 复用每秒时钟循环检查临时消息是否到期，不额外开线程
                        if let Some(deadline) = this.message_expires_at {
                            if Instant::now() >= deadline {
                                this.message = READY_MESSAGE.into();
                                this.message_expires_at = None;
                            }
                        }
                        cx.notify();
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();

        Self {
            message: READY_MESSAGE.into(),
            message_expires_at: None,
            active_session: None,
            hosts: HashMap::new(),
            latencies: HashMap::new(),
            reconnects: HashMap::new(),
            clock: chrono::Local::now().format("%H:%M:%S").to_string(),
        }
    }

    /// 切换活动 Tab 时同步右侧显示的主机（MainWindow 在 Tab 变化后调用）。
    pub fn set_active_session(&mut self, session: Option<Uuid>, cx: &mut Context<Self>) {
        if self.active_session != session {
            self.active_session = session;
            cx.notify();
        }
    }

    /// 主窗口的状态提示是持久消息：清掉可能存在的临时过期时间。
    pub fn clear_expiry(&mut self) {
        self.message_expires_at = None;
    }
}

impl Render for StatusBar {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        let text_color = t.text_on_primary_btn();

        let host = self
            .active_session
            .and_then(|session| self.hosts.get(&session));
        let latency = self
            .active_session
            .and_then(|session| self.latencies.get(&session));
        let reconnect = self
            .active_session
            .and_then(|session| self.reconnects.get(&session));

        let mut right = String::new();
        if let Some(host) = host {
            right.push_str(host);
        }
        if let Some(latency) = latency {
            if !right.is_empty() {
                right.push_str("  ·  ");
            }
            right.push_str(&format!("{latency} ms"));
        }
        if let Some(attempt) = reconnect {
            if !right.is_empty() {
                right.push_str("  ·  ");
            }
            right.push_str(&format!("重连 {attempt} 次"));
        }
        if !right.is_empty() {
            right.push_str("  ·  ");
        }
        right.push_str(&self.clock);

        div()
            .w_full()
            .h(px(STATUS_BAR_H))
            .bg(t.primary())
            .flex()
            .items_center()
            .justify_between()
            .px_3()
            .text_color(text_color)
            .text_size(px(11.))
            .child(div().flex_1().truncate().child(self.message.clone()))
            .child(div().flex_none().max_w(px(420.)).truncate().child(right))
    }
}

impl Default for StatusBar {
    fn default() -> Self {
        Self {
            message: READY_MESSAGE.into(),
            message_expires_at: None,
            active_session: None,
            hosts: HashMap::new(),
            latencies: HashMap::new(),
            reconnects: HashMap::new(),
            clock: String::new(),
        }
    }
}

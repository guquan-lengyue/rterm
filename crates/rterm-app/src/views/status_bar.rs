//! StatusBar — 底部状态栏（Sprint 7）
//!
//! 左侧：临时消息（设置已保存 / 会话已删除…）
//! 右侧：当前主机 · 连接延迟 · 重连次数 · 时钟

use std::time::{Duration, Instant};

use gpui::prelude::*;
use gpui::{div, px, Context, IntoElement, Render, Window};

use crate::event::AppEvent;
use crate::state::AppState;
use crate::ui::theme;

use super::STATUS_BAR_H;

/// 无消息时的默认文案。
const READY_MESSAGE: &str = "Ready — rterm";

pub struct StatusBar {
    pub message: String,
    /// 临时消息（StatusBarMessage）的过期时间；None 表示持久消息。
    message_expires_at: Option<Instant>,
    host: Option<String>,
    latency_ms: Option<u64>,
    reconnect_attempt: Option<u8>,
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
                        let store = state.session_store.lock().unwrap();
                        store.get(*session_id).map(|session| session.display_label())
                    };
                    this.host = label.or_else(|| this.host.clone());
                    this.reconnect_attempt = None;
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
                AppEvent::SessionLatency { latency_ms, .. } => {
                    this.latency_ms = Some(*latency_ms);
                    cx.notify();
                }
                AppEvent::SessionReconnecting { attempt, .. } => {
                    this.reconnect_attempt = Some(*attempt);
                    this.message = format!("重连中（第 {attempt} 次）…");
                    this.message_expires_at = None;
                    cx.notify();
                }
                AppEvent::SessionDisconnected { reason, .. } => {
                    this.message = format!("已断开：{reason}");
                    this.reconnect_attempt = None;
                    this.latency_ms = None;
                    this.message_expires_at = None;
                    cx.notify();
                }
                AppEvent::SessionFailed { reason, .. } => {
                    this.message = format!("连接失败：{reason}");
                    this.reconnect_attempt = None;
                    this.latency_ms = None;
                    this.message_expires_at = None;
                    cx.notify();
                }
                _ => {}
            }
        })
        .detach();

        // 时钟：每秒刷新
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_secs(1))
                    .await;
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
            host: None,
            latency_ms: None,
            reconnect_attempt: None,
            clock: chrono::Local::now().format("%H:%M:%S").to_string(),
        }
    }
}

impl Render for StatusBar {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        let text_color = t.text_on_primary_btn();

        let mut right = String::new();
        if let Some(host) = &self.host {
            right.push_str(host);
        }
        if let Some(latency) = self.latency_ms {
            if !right.is_empty() {
                right.push_str("  ·  ");
            }
            right.push_str(&format!("{latency} ms"));
        }
        if let Some(attempt) = self.reconnect_attempt {
            if !right.is_empty() {
                right.push_str("  ·  ");
            }
            right.push_str(&format!("重连 {attempt}"));
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
            .child(div().flex_none().child(right))
    }
}

impl Default for StatusBar {
    fn default() -> Self {
        Self {
            message: READY_MESSAGE.into(),
            message_expires_at: None,
            host: None,
            latency_ms: None,
            reconnect_attempt: None,
            clock: String::new(),
        }
    }
}

//! MonitorView — 远程性能监控 Tab。
//!
//! 分层：
//! - 采集全部在 `rterm-monitor` 的后台线程里完成，本文件只消费事件、持有展示状态
//! - 每个 Monitor Tab 独立 SSH 连接，不占用终端 / SFTP 的会话
//! - 图表使用 gpui Canvas 直接绘制，避免引入额外依赖

use std::collections::VecDeque;
use std::time::Duration;

use futures::channel::mpsc::unbounded;
use futures::StreamExt;
use gpui::prelude::*;
use gpui::{
    canvas, div, point, px, size, AnyElement, Bounds, ClickEvent, Context, FocusHandle, Focusable,
    Hsla, IntoElement, MouseButton, MouseDownEvent, Pixels, Render, Rgba, ScrollWheelEvent, Window,
};
use uuid::Uuid;

use rterm_monitor::{
    spawn, KillSignal, Metric, MonitorAlert, MonitorCommand, MonitorConfig, MonitorEvent,
    MonitorHandle, MonitorTick, ResourceSample, TopProcess,
};
use rterm_ssh::SshSession;

use super::{CONTENT_BORDER_W, MENU_ITEM_H, SIDEBAR_W, TAB_BAR_H, TOOLBAR_H};
use crate::event::{AppEvent, EventBusHost};
use crate::state::{lock_or_recover, AppState};
use crate::ui::theme;

/// 图表窗口长度（60 点 × 2s ≈ 2 分钟）。
const MAX_SAMPLES: usize = 60;
const CARD_H: f32 = 146.0;
const PROCESS_ROW_H: f32 = 24.0;
const PROCESS_HEADER_H: f32 = 24.0;
const KILL_MENU_W: f32 = 200.0;

// ---------------------------------------------------------------------------
// 展示状态
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq)]
enum MonitorStatus {
    Connecting,
    Connected,
    Failed,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum TopSortKey {
    Pid,
    User,
    Cpu,
    Mem,
    Command,
}

impl TopSortKey {
    fn value(self, process: &TopProcess) -> String {
        match self {
            Self::Pid => format!("{:010}", process.pid),
            Self::User => process.user.clone(),
            Self::Cpu => format!("{:08.2}", process.cpu_pct),
            Self::Mem => format!("{:08.2}", process.mem_pct),
            Self::Command => process.command.clone(),
        }
    }
}

struct KillMenu {
    pid: u32,
    command: String,
    x: f32,
    y: f32,
}

struct PendingKill {
    pid: u32,
    command: String,
    signal: KillSignal,
}

// ---------------------------------------------------------------------------
// MonitorView
// ---------------------------------------------------------------------------

pub struct MonitorView {
    focus_handle: FocusHandle,
    tab_id: Uuid,
    session_id: Option<Uuid>,
    status: MonitorStatus,
    status_text: String,
    handle: Option<MonitorHandle>,
    samples: VecDeque<ResourceSample>,
    latest: Option<ResourceSample>,
    top: Vec<TopProcess>,
    sort_key: TopSortKey,
    sort_ascending: bool,
    selected_pid: Option<u32>,
    scroll: usize,
    paused: bool,
    error: Option<String>,
    alert: Option<MonitorAlert>,
    kill_menu: Option<KillMenu>,
    pending_kill: Option<PendingKill>,
    generation: u64,
}

impl MonitorView {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let host = cx.global::<AppState>().event_bus_host.clone();
        cx.subscribe(
            &host,
            |this: &mut Self, _host, event: &AppEvent, cx| match event {
                AppEvent::AppSettingsChanged => this.apply_settings(cx),
                AppEvent::AppThemeChanged { .. } => cx.notify(),
                _ => {}
            },
        )
        .detach();

        Self {
            focus_handle: cx.focus_handle(),
            tab_id: Uuid::new_v4(),
            session_id: None,
            status: MonitorStatus::Connecting,
            status_text: "等待连接…".to_string(),
            handle: None,
            samples: VecDeque::new(),
            latest: None,
            top: Vec::new(),
            sort_key: TopSortKey::Cpu,
            sort_ascending: false,
            selected_pid: None,
            scroll: 0,
            paused: false,
            error: None,
            alert: None,
            kill_menu: None,
            pending_kill: None,
            generation: 0,
        }
    }

    pub fn tab_id(&self) -> Uuid {
        self.tab_id
    }

    pub fn focus_handle(&self) -> FocusHandle {
        self.focus_handle.clone()
    }

    /// Kill 确认卡片打开时屏蔽全局快捷键。
    pub fn has_modal_dialog(&self) -> bool {
        self.pending_kill.is_some()
    }

    // -----------------------------------------------------------------------
    // 连接 / 关闭
    // -----------------------------------------------------------------------

    /// 打开监控 Tab：独立 SSH 连接 → 启动采集线程。
    pub fn connect(&mut self, session_id: Uuid, cx: &mut Context<Self>) {
        let stored = {
            let state = cx.global::<AppState>();
            let store = lock_or_recover(&state.session_store);
            store.get(session_id).cloned()
        };
        let Some(stored) = stored else {
            self.fail(format!("会话不存在: {session_id}"));
            cx.notify();
            return;
        };
        let session = match rterm_session::crypto_bridge::decrypt_session_dpapi(&stored) {
            Ok(session) => session,
            Err(error) => {
                self.fail(format!("解密会话失败: {error:#}"));
                cx.notify();
                return;
            }
        };
        if session.host.trim().is_empty() {
            self.fail("会话主机地址为空".into());
            cx.notify();
            return;
        }

        self.session_id = Some(session_id);
        self.status = MonitorStatus::Connecting;
        self.status_text = format!("连接 {}…", session.display_label());
        self.generation = self.generation.wrapping_add(1);
        let generation = self.generation;

        log::info!(
            "[monitor] connect requested: {}@{}:{}",
            session.username,
            session.host,
            session.port
        );

        let (tx, mut rx) = unbounded::<Result<SshSession, String>>();
        let spawned = std::thread::Builder::new()
            .name("rterm-monitor-connect".into())
            .spawn(move || {
                let mut ssh = SshSession::new();
                let result = ssh
                    .connect(&session)
                    .map(|()| ssh)
                    .map_err(|e| rterm_ssh::errors::describe(&e));
                let _ = tx.unbounded_send(result);
            });
        if let Err(error) = spawned {
            self.fail(format!("无法启动监控连接线程: {error}"));
            cx.notify();
            return;
        }

        cx.spawn(async move |this, cx| {
            if let Some(result) = rx.next().await {
                let _ = this.update(cx, |this, cx| this.handle_connected(generation, result, cx));
            }
        })
        .detach();

        cx.notify();
    }

    fn handle_connected(
        &mut self,
        generation: u64,
        result: Result<SshSession, String>,
        cx: &mut Context<Self>,
    ) {
        if generation != self.generation {
            return;
        }
        let ssh = match result {
            Ok(ssh) => ssh,
            Err(reason) => {
                self.fail(reason);
                cx.notify();
                return;
            }
        };

        let config = monitor_config(cx);
        let (events_tx, mut events_rx) = unbounded::<MonitorEvent>();
        let handle = match spawn(ssh, config, events_tx) {
            Ok(handle) => handle,
            Err(error) => {
                self.fail(format!("{error:#}"));
                cx.notify();
                return;
            }
        };
        self.handle = Some(handle);
        self.status = MonitorStatus::Connected;
        self.status_text = "已连接 · 等待首帧采样".to_string();
        self.error = None;

        let generation = self.generation;
        cx.spawn(async move |this, cx| {
            while let Some(event) = events_rx.next().await {
                let alive = this
                    .update(cx, |this, cx| this.on_monitor_event(generation, event, cx))
                    .is_ok();
                if !alive {
                    break;
                }
            }
        })
        .detach();

        if let Some(session_id) = self.session_id {
            self.emit(
                cx,
                AppEvent::MonitorOpened {
                    tab_id: self.tab_id,
                    session_id,
                },
            );
        }
        cx.notify();
    }

    /// 关闭 Tab 时调用。
    pub fn shutdown(&mut self, cx: &mut Context<Self>) {
        self.generation = self.generation.wrapping_add(1);
        self.handle = None; // Drop 置停止位
        self.top.clear();
        self.status = MonitorStatus::Failed;
        self.status_text = "已断开".to_string();
        self.emit(
            cx,
            AppEvent::MonitorClosed {
                tab_id: self.tab_id,
            },
        );
        cx.notify();
    }

    fn fail(&mut self, reason: String) {
        log::error!("[monitor] {reason}");
        self.status = MonitorStatus::Failed;
        self.status_text = reason;
        self.handle = None;
    }

    // -----------------------------------------------------------------------
    // 采集事件
    // -----------------------------------------------------------------------

    fn on_monitor_event(&mut self, generation: u64, event: MonitorEvent, cx: &mut Context<Self>) {
        if generation != self.generation {
            return;
        }
        match event {
            MonitorEvent::Tick(tick) => self.apply_tick(tick, cx),
            MonitorEvent::Alert(alert) => self.apply_alert(alert, cx),
            MonitorEvent::Error(message) => {
                self.error = Some(message);
                cx.notify();
            }
            MonitorEvent::KillResult { pid, ok, message } => {
                if ok {
                    self.status_text = format!("已连接 · {message}");
                    if self.selected_pid == Some(pid) {
                        self.selected_pid = None;
                    }
                } else {
                    self.error = Some(message.clone());
                }
                self.emit_status(message, cx);
                cx.notify();
            }
        }
    }

    fn apply_tick(&mut self, tick: MonitorTick, cx: &mut Context<Self>) {
        self.status_text = "已连接 · 实时采集中".to_string();
        self.error = None;
        self.samples.push_back(tick.sample.clone());
        while self.samples.len() > MAX_SAMPLES {
            self.samples.pop_front();
        }
        self.latest = Some(tick.sample.clone());
        self.top = tick.top;
        self.sort_top();
        self.clamp_scroll();

        // 指标恢复后自动收起告警条。
        if let Some(alert) = &self.alert {
            let value = tick.sample.metric(alert.metric);
            if value < alert.threshold * 0.95 {
                self.alert = None;
            }
        }
        cx.notify();
    }

    fn apply_alert(&mut self, alert: MonitorAlert, cx: &mut Context<Self>) {
        let text = format!(
            "⚠ {} 持续高于阈值：{:.0}%（阈值 {:.0}%）",
            alert.metric.label(),
            alert.value,
            alert.threshold
        );
        self.alert = Some(alert);
        self.emit_status(text, cx);
        cx.notify();
    }

    fn apply_settings(&mut self, cx: &mut Context<Self>) {
        if let Some(handle) = &self.handle {
            let _ = handle.send(MonitorCommand::SetConfig(monitor_config(cx)));
        }
        cx.notify();
    }

    // -----------------------------------------------------------------------
    // 操作
    // -----------------------------------------------------------------------

    fn toggle_pause(&mut self, cx: &mut Context<Self>) {
        let next = !self.paused;
        if let Some(handle) = &self.handle {
            let command = if next {
                MonitorCommand::Pause
            } else {
                MonitorCommand::Resume
            };
            if !handle.send(command) {
                self.error = Some("采集线程已退出".into());
                cx.notify();
                return;
            }
            self.paused = next;
            self.status_text = if next {
                "已连接 · 已暂停".to_string()
            } else {
                "已连接 · 实时采集中".to_string()
            };
            cx.notify();
        }
    }

    fn reset_history(&mut self, cx: &mut Context<Self>) {
        self.samples.clear();
        self.latest = None;
        self.alert = None;
        self.error = None;
        cx.notify();
    }

    fn set_sort(&mut self, key: TopSortKey, cx: &mut Context<Self>) {
        if self.sort_key == key {
            self.sort_ascending = !self.sort_ascending;
        } else {
            self.sort_key = key;
            // CPU / 内存默认降序，其余升序。
            self.sort_ascending = matches!(
                key,
                TopSortKey::Pid | TopSortKey::User | TopSortKey::Command
            );
        }
        self.sort_top();
        cx.notify();
    }

    fn sort_top(&mut self) {
        let key = self.sort_key;
        let ascending = self.sort_ascending;
        self.top.sort_by(|a, b| {
            let ordering = key.value(a).cmp(&key.value(b));
            if ascending {
                ordering
            } else {
                ordering.reverse()
            }
        });
    }

    fn on_scroll(&mut self, event: &ScrollWheelEvent, cx: &mut Context<Self>) {
        let delta = event.delta.pixel_delta(px(PROCESS_ROW_H));
        let dy = f32::from(delta.y);
        let lines = if event.delta.precise() {
            (dy / PROCESS_ROW_H).round() as i32
        } else if dy.abs() > 0.1 {
            3 * dy.signum() as i32
        } else {
            0
        };
        if lines == 0 {
            return;
        }
        let next = (self.scroll as i32 - lines).max(0) as usize;
        self.scroll = next.min(self.top.len().saturating_sub(1));
        cx.notify();
    }

    fn clamp_scroll(&mut self) {
        self.scroll = self.scroll.min(self.top.len().saturating_sub(1));
    }

    fn open_kill_menu(
        &mut self,
        pid: u32,
        command: String,
        x: f32,
        y: f32,
        cx: &mut Context<Self>,
    ) {
        self.selected_pid = Some(pid);
        self.kill_menu = Some(KillMenu {
            pid,
            command,
            x: x.max(0.0),
            y: y.max(0.0),
        });
        cx.notify();
    }

    fn request_kill(
        &mut self,
        pid: u32,
        command: String,
        signal: KillSignal,
        cx: &mut Context<Self>,
    ) {
        self.kill_menu = None;
        self.pending_kill = Some(PendingKill {
            pid,
            command,
            signal,
        });
        cx.notify();
    }

    fn confirm_kill(&mut self, cx: &mut Context<Self>) {
        let Some(pending) = self.pending_kill.take() else {
            return;
        };
        if let Some(handle) = &self.handle {
            if !handle.send(MonitorCommand::Kill {
                pid: pending.pid,
                signal: pending.signal,
            }) {
                self.error = Some("采集线程已退出，无法终止进程".into());
            }
        }
        cx.notify();
    }

    fn emit_status(&self, text: String, cx: &mut Context<Self>) {
        self.emit(
            cx,
            AppEvent::StatusBarMessage {
                text,
                duration_ms: 5_000,
            },
        );
    }

    fn emit(&self, cx: &mut Context<Self>, event: AppEvent) {
        let host: gpui::Entity<EventBusHost> = cx.global::<AppState>().event_bus_host.clone();
        cx.update_entity(&host, |_host, cx: &mut Context<EventBusHost>| {
            cx.emit(event);
        });
    }

    // -----------------------------------------------------------------------
    // Render 辅助
    // -----------------------------------------------------------------------

    fn render_header(&self, cx: &mut Context<Self>) -> AnyElement {
        let t = theme(cx);
        let (status_color, dot) = match self.status {
            MonitorStatus::Connecting => (t.text_subtle(), "◌"),
            MonitorStatus::Connected => {
                if self.paused {
                    (t.text_subtle(), "⏸")
                } else {
                    (t.success(), "●")
                }
            }
            MonitorStatus::Failed => (t.danger(), "✕"),
        };
        let pause_label = if self.paused {
            "▶ 继续"
        } else {
            "⏸ 暂停"
        };
        let pause_hover = t.secondary_btn_hover();
        let reset_hover = t.secondary_btn_hover();

        div()
            .w_full()
            .flex()
            .items_center()
            .gap_2()
            .child(
                div()
                    .text_size(px(11.))
                    .text_color(status_color)
                    .child(format!("{dot} {}", self.status_text)),
            )
            .child(div().flex_1())
            .child(
                div()
                    .id("monitor-pause")
                    .h(px(24.))
                    .px_3()
                    .rounded_md()
                    .bg(t.secondary_btn_bg())
                    .text_color(t.text_on_secondary_btn())
                    .text_size(px(11.))
                    .flex()
                    .items_center()
                    .cursor_pointer()
                    .hover(move |s| s.bg(pause_hover))
                    .on_click(cx.listener(|this: &mut Self, _ev: &ClickEvent, _w, cx| {
                        this.toggle_pause(cx);
                    }))
                    .child(pause_label),
            )
            .child(
                div()
                    .id("monitor-reset")
                    .h(px(24.))
                    .px_3()
                    .rounded_md()
                    .bg(t.secondary_btn_bg())
                    .text_color(t.text_on_secondary_btn())
                    .text_size(px(11.))
                    .flex()
                    .items_center()
                    .cursor_pointer()
                    .hover(move |s| s.bg(reset_hover))
                    .on_click(cx.listener(|this: &mut Self, _ev: &ClickEvent, _w, cx| {
                        this.reset_history(cx);
                    }))
                    .child("↺ 重置"),
            )
            .into_any_element()
    }

    fn render_cards(&self, cx: &mut Context<Self>) -> AnyElement {
        let t = theme(cx);
        let alarm = self.alert.as_ref().map(|alert| alert.metric);

        let cpu_values = self.series(|sample| sample.cpu_pct);
        let mem_values = self.series(|sample| sample.mem_pct);
        let swap_values = self.series(|sample| sample.swap_pct);
        let disk_values = self.series(|sample| sample.disk_pct);
        let rx_values = self.series(|sample| sample.net_rx_kbps);
        let tx_values = self.series(|sample| sample.net_tx_kbps);
        let load_values = self.series(|sample| sample.load_1);

        let latest = self.latest.clone().unwrap_or_default();
        let net_max = auto_max(rx_values.iter().chain(tx_values.iter()).copied(), 10.0);
        let load_max = auto_max(load_values.iter().copied(), 4.0);
        let normal = t.primary();
        let danger = t.danger();
        let cpu_color = if alarm == Some(Metric::Cpu) {
            danger
        } else {
            normal
        };
        let mem_color = if alarm == Some(Metric::Mem) {
            danger
        } else {
            normal
        };

        let cpu_card = self.render_card(
            "monitor-card-cpu",
            "CPU",
            format!("{:.0}%", latest.cpu_pct),
            cpu_values,
            0.0,
            100.0,
            alarm == Some(Metric::Cpu),
            cpu_color,
            cx,
        );
        let mem_card = self.render_card(
            "monitor-card-mem",
            "内存",
            format!("{:.0}%", latest.mem_pct),
            mem_values,
            0.0,
            100.0,
            alarm == Some(Metric::Mem),
            mem_color,
            cx,
        );
        let swap_card = self.render_card(
            "monitor-card-swap",
            "交换分区",
            format!("{:.0}%", latest.swap_pct),
            swap_values,
            0.0,
            100.0,
            false,
            normal,
            cx,
        );
        let disk_card = self.render_card(
            "monitor-card-disk",
            "磁盘 /",
            format!("{:.0}%", latest.disk_pct),
            disk_values,
            0.0,
            100.0,
            false,
            normal,
            cx,
        );
        let net_card = self.render_dual_card(
            "monitor-card-net",
            "网络",
            format!(
                "↓ {:.1}  ↑ {:.1} KiB/s",
                latest.net_rx_kbps, latest.net_tx_kbps
            ),
            rx_values,
            tx_values,
            net_max,
            cx,
        );
        let load_card = self.render_card(
            "monitor-card-load",
            "负载",
            format!("1m {:.2} · 5m {:.2}", latest.load_1, latest.load_5),
            load_values,
            0.0,
            load_max,
            false,
            t.success(),
            cx,
        );

        div()
            .w_full()
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .w_full()
                    .flex()
                    .gap_2()
                    .child(cpu_card)
                    .child(mem_card)
                    .child(swap_card),
            )
            .child(
                div()
                    .w_full()
                    .flex()
                    .gap_2()
                    .child(disk_card)
                    .child(net_card)
                    .child(load_card),
            )
            .into_any_element()
    }

    fn series(&self, pick: impl Fn(&ResourceSample) -> f32) -> Vec<f32> {
        self.samples.iter().map(pick).collect()
    }

    #[allow(clippy::too_many_arguments)]
    fn render_card(
        &self,
        id: &'static str,
        label: &'static str,
        value: String,
        values: Vec<f32>,
        min: f32,
        max: f32,
        alarmed: bool,
        color: Rgba,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        self.render_card_shell(
            id,
            label,
            value,
            alarmed,
            vec![Series {
                values,
                color,
                min,
                max,
            }],
            cx,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn render_dual_card(
        &self,
        id: &'static str,
        label: &'static str,
        value: String,
        rx: Vec<f32>,
        tx: Vec<f32>,
        max: f32,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = theme(cx);
        self.render_card_shell(
            id,
            label,
            value,
            false,
            vec![
                Series {
                    values: rx,
                    color: t.primary(),
                    min: 0.0,
                    max,
                },
                Series {
                    values: tx,
                    color: t.success(),
                    min: 0.0,
                    max,
                },
            ],
            cx,
        )
    }

    fn render_card_shell(
        &self,
        id: &'static str,
        label: &'static str,
        value: String,
        alarmed: bool,
        series: Vec<Series>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = theme(cx);
        let border = if alarmed {
            t.danger()
        } else {
            t.border_subtle()
        };
        let label_color = if alarmed { t.danger() } else { t.text_subtle() };
        let grid_color = t.border_subtle();
        let capacity = MAX_SAMPLES;

        div()
            .id(id)
            .flex_1()
            .h(px(CARD_H))
            .bg(t.card_bg())
            .border_1()
            .border_color(border)
            .rounded_md()
            .p_2()
            .flex()
            .flex_col()
            .gap_1()
            .child(
                div()
                    .w_full()
                    .flex()
                    .items_center()
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(label_color)
                            .child(label),
                    )
                    .child(div().flex_1())
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(t.text_on_card_bg())
                            .child(value),
                    ),
            )
            .child(
                div().w_full().flex_1().relative().child(
                    canvas(
                        move |_bounds, _window, _cx| {},
                        move |bounds, _state, window, _cx| {
                            paint_chart(&series, capacity, bounds, window, grid_color);
                        },
                    )
                    .size_full(),
                ),
            )
            .into_any_element()
    }

    fn render_error(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let error = self.error.as_ref()?;
        let t = theme(cx);
        Some(
            div()
                .w_full()
                .px_2()
                .py_1()
                .bg(t.card_bg())
                .rounded_md()
                .text_size(px(11.))
                .text_color(t.danger())
                .child(format!("采集异常: {error}"))
                .into_any_element(),
        )
    }

    fn render_table(&self, cx: &mut Context<Self>) -> AnyElement {
        let t = theme(cx);
        let header = div()
            .w_full()
            .h(px(PROCESS_HEADER_H))
            .bg(t.card_header_bg())
            .border_b_1()
            .border_color(t.border_subtle())
            .flex()
            .items_center()
            .px_2()
            .text_size(px(10.))
            .text_color(t.text_subtle())
            .child(self.render_sort_header("PID", TopSortKey::Pid, 70.0, cx))
            .child(self.render_sort_header("用户", TopSortKey::User, 100.0, cx))
            .child(self.render_sort_header("CPU", TopSortKey::Cpu, 70.0, cx))
            .child(self.render_sort_header("内存", TopSortKey::Mem, 70.0, cx))
            .child(self.render_sort_header("命令", TopSortKey::Command, 0.0, cx));

        let rows: Vec<AnyElement> = self
            .top
            .iter()
            .skip(self.scroll)
            .map(|process| self.render_process_row(process, cx))
            .collect();

        div()
            .w_full()
            .flex_1()
            .bg(t.card_bg())
            .border_1()
            .border_color(t.border_subtle())
            .rounded_md()
            .overflow_hidden()
            .flex()
            .flex_col()
            .on_scroll_wheel(
                cx.listener(|this: &mut Self, event: &ScrollWheelEvent, _w, cx| {
                    this.on_scroll(event, cx);
                }),
            )
            .child(header)
            .child(
                div()
                    .w_full()
                    .flex_1()
                    .overflow_hidden()
                    .flex()
                    .flex_col()
                    .children(rows),
            )
            .into_any_element()
    }

    fn render_sort_header(
        &self,
        label: &'static str,
        key: TopSortKey,
        width: f32,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = theme(cx);
        let active = self.sort_key == key;
        let arrow = if active {
            if self.sort_ascending {
                " ↑"
            } else {
                " ↓"
            }
        } else {
            ""
        };
        let text = format!("{label}{arrow}");
        let color = if active {
            t.text_on_card_bg()
        } else {
            t.text_subtle()
        };
        let hover = t.row_hover_bg();

        let mut header = div()
            .id(format!("monitor-sort-{}", key as u8))
            .h_full()
            .flex()
            .items_center()
            .cursor_pointer()
            .text_color(color)
            .hover(move |s| s.bg(hover))
            .on_click(
                cx.listener(move |this: &mut Self, _ev: &ClickEvent, _w, cx| {
                    this.set_sort(key, cx);
                }),
            )
            .child(text);
        if width > 0.0 {
            header = header.w(px(width));
        } else {
            header = header.flex_1();
        }
        header.into_any_element()
    }

    fn render_process_row(&self, process: &TopProcess, cx: &mut Context<Self>) -> AnyElement {
        let t = theme(cx);
        let selected = self.selected_pid == Some(process.pid);
        let bg = if selected {
            t.row_selected_bg()
        } else {
            t.card_bg()
        };
        let text_color = if selected {
            t.text_on_row_selected_bg()
        } else {
            t.text_on_card_bg()
        };
        let hover = t.row_hover_bg();
        let pid = process.pid;
        let command = process.command.clone();

        div()
            .id(format!("monitor-proc-{pid}"))
            .w_full()
            .h(px(PROCESS_ROW_H))
            .bg(bg)
            .flex()
            .items_center()
            .px_2()
            .text_size(px(10.))
            .text_color(text_color)
            .cursor_pointer()
            .hover(move |s| s.bg(hover))
            .on_click(
                cx.listener(move |this: &mut Self, _ev: &ClickEvent, _w, cx| {
                    this.selected_pid = Some(pid);
                    cx.notify();
                }),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, event: &MouseDownEvent, _w, cx| {
                    let x = f32::from(event.position.x) - SIDEBAR_W - CONTENT_BORDER_W;
                    let y = f32::from(event.position.y) - TOOLBAR_H - TAB_BAR_H;
                    this.open_kill_menu(pid, command.clone(), x, y, cx);
                }),
            )
            .child(div().w(px(70.)).child(pid.to_string()))
            .child(div().w(px(100.)).truncate().child(process.user.clone()))
            .child(div().w(px(70.)).child(format!("{:.1}", process.cpu_pct)))
            .child(div().w(px(70.)).child(format!("{:.1}", process.mem_pct)))
            .child(div().flex_1().truncate().child(process.command.clone()))
            .into_any_element()
    }

    fn render_kill_menu(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let menu = self.kill_menu.as_ref()?;
        let t = theme(cx);
        let hover = t.row_hover_bg();
        let pid = menu.pid;
        let command = menu.command.clone();

        Some(
            div()
                .absolute()
                .left(px(menu.x))
                .top(px(menu.y))
                .w(px(KILL_MENU_W))
                .py_1()
                .bg(t.card_bg())
                .border_1()
                .border_color(t.border_medium())
                .rounded_md()
                .flex()
                .flex_col()
                .id("monitor-kill-menu")
                .on_mouse_down_out(
                    cx.listener(|this: &mut Self, _ev: &MouseDownEvent, _w, cx| {
                        this.kill_menu = None;
                        cx.notify();
                    }),
                )
                .child(self.render_menu_item(
                    "monitor-kill-term",
                    "🛑 终止 (SIGTERM)",
                    pid,
                    command.clone(),
                    KillSignal::Term,
                    hover,
                    cx,
                ))
                .child(self.render_menu_item(
                    "monitor-kill-kill",
                    "☠ 强制终止 (SIGKILL)",
                    pid,
                    command,
                    KillSignal::Kill,
                    hover,
                    cx,
                ))
                .into_any_element(),
        )
    }

    fn render_menu_item(
        &self,
        id: &'static str,
        label: &'static str,
        pid: u32,
        command: String,
        signal: KillSignal,
        hover: Rgba,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = theme(cx);
        div()
            .w_full()
            .h(px(MENU_ITEM_H))
            .px_3()
            .flex()
            .items_center()
            .text_size(px(11.))
            .text_color(if signal == KillSignal::Kill {
                t.danger()
            } else {
                t.text_on_card_bg()
            })
            .cursor_pointer()
            .id(id)
            .hover(move |s| s.bg(hover))
            .on_click(
                cx.listener(move |this: &mut Self, _ev: &ClickEvent, _w, cx| {
                    this.request_kill(pid, command.clone(), signal, cx);
                }),
            )
            .child(label)
            .into_any_element()
    }

    fn render_confirm(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let pending = self.pending_kill.as_ref()?;
        let t = theme(cx);
        let message = format!(
            "确定向 PID {}（{}）发送 {}？",
            pending.pid,
            truncate_command(&pending.command, 40),
            signal_label(pending.signal)
        );
        let cancel_hover = t.secondary_btn_hover();

        Some(
            div()
                .absolute()
                .inset_0()
                .bg(t.modal_overlay())
                .flex()
                .items_center()
                .justify_center()
                .child(
                    div()
                        .w(px(420.))
                        .bg(t.card_bg())
                        .border_1()
                        .border_color(t.border_medium())
                        .rounded_lg()
                        .p_4()
                        .flex()
                        .flex_col()
                        .gap_3()
                        .child(
                            div()
                                .w_full()
                                .text_center()
                                .text_size(px(13.))
                                .text_color(t.text_on_card_bg())
                                .child(message),
                        )
                        .child(
                            div()
                                .w_full()
                                .text_center()
                                .text_size(px(11.))
                                .text_color(t.text_subtle())
                                .child("该操作会立即结束远端进程，未保存的数据可能丢失"),
                        )
                        .child(
                            div()
                                .flex()
                                .justify_center()
                                .gap_2()
                                .child(
                                    div()
                                        .id("monitor-kill-cancel")
                                        .px_3()
                                        .h(px(26.))
                                        .rounded_md()
                                        .bg(t.secondary_btn_bg())
                                        .text_size(px(11.))
                                        .text_color(t.text_on_secondary_btn())
                                        .flex()
                                        .items_center()
                                        .cursor_pointer()
                                        .hover(move |s| s.bg(cancel_hover))
                                        .on_click(cx.listener(
                                            |this: &mut Self, _ev: &ClickEvent, _w, cx| {
                                                this.pending_kill = None;
                                                cx.notify();
                                            },
                                        ))
                                        .child("取消"),
                                )
                                .child(
                                    div()
                                        .id("monitor-kill-confirm")
                                        .px_3()
                                        .h(px(26.))
                                        .rounded_md()
                                        .bg(t.danger())
                                        .text_size(px(11.))
                                        .text_color(t.text_on_primary_btn())
                                        .flex()
                                        .items_center()
                                        .cursor_pointer()
                                        .on_click(cx.listener(
                                            |this: &mut Self, _ev: &ClickEvent, _w, cx| {
                                                this.confirm_kill(cx);
                                            },
                                        ))
                                        .child("终止"),
                                ),
                        ),
                )
                .into_any_element(),
        )
    }

    fn render_alert_banner(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let alert = self.alert.as_ref()?;
        let t = theme(cx);
        Some(
            div()
                .w_full()
                .px_2()
                .py_1()
                .bg(t.danger())
                .rounded_md()
                .text_size(px(11.))
                .text_color(t.text_on_primary_btn())
                .child(format!(
                    "⚠ {} 持续高于阈值：{:.0}% ≥ {:.0}%（最近 5 次均值）",
                    alert.metric.label(),
                    alert.value,
                    alert.threshold
                ))
                .into_any_element(),
        )
    }
}

impl Focusable for MonitorView {
    fn focus_handle(&self, _cx: &gpui::App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for MonitorView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        let mut root = div()
            .size_full()
            .bg(t.app_bg())
            .track_focus(&self.focus_handle)
            .relative()
            .flex()
            .flex_col()
            .p_3()
            .gap_2()
            .child(self.render_header(cx))
            .child(self.render_cards(cx));

        if let Some(banner) = self.render_alert_banner(cx) {
            root = root.child(banner);
        }
        if let Some(error) = self.render_error(cx) {
            root = root.child(error);
        }

        root = root.child(self.render_table(cx));

        if let Some(menu) = self.render_kill_menu(cx) {
            root = root.child(menu);
        }
        if let Some(confirm) = self.render_confirm(cx) {
            root = root.child(confirm);
        }
        root
    }
}

// 删掉 BISECT 遗留的 simple_card 辅助方法

// ---------------------------------------------------------------------------
// 图表绘制（纯函数，便于理解与复用）
// ---------------------------------------------------------------------------

struct Series {
    values: Vec<f32>,
    color: Rgba,
    min: f32,
    max: f32,
}

fn paint_chart(
    series: &[Series],
    capacity: usize,
    bounds: Bounds<Pixels>,
    window: &mut Window,
    grid_color: Rgba,
) {
    let width = f32::from(bounds.size.width);
    let height = f32::from(bounds.size.height);
    if width <= 4.0 || height <= 4.0 {
        return;
    }

    // 背景网格：4 条水平线。
    let grid: Hsla = grid_color.into();
    for step in 1..=4 {
        let y = bounds.top() + px(height * step as f32 / 5.0);
        window.paint_quad(gpui::fill(
            Bounds::new(point(bounds.left(), y), size(bounds.size.width, px(1.0))),
            grid,
        ));
    }

    let padding = 2.0;
    let usable_w = (width - padding * 2.0).max(1.0);
    let usable_h = (height - padding * 2.0).max(1.0);
    let denominator = (capacity.max(2) - 1) as f32;

    for line in series {
        if line.values.len() < 2 {
            continue;
        }
        let span = (line.max - line.min).max(0.001);
        let mut builder = gpui::PathBuilder::stroke(px(2.0));
        for (index, value) in line.values.iter().enumerate() {
            let x = bounds.left() + px(padding + usable_w * index as f32 / denominator);
            let normalized = ((value - line.min) / span).clamp(0.0, 1.0);
            let y = bounds.top() + px(padding + usable_h * (1.0 - normalized));
            if index == 0 {
                builder.move_to(point(x, y));
            } else {
                builder.line_to(point(x, y));
            }
        }
        if let Ok(path) = builder.build() {
            let color: Hsla = line.color.into();
            window.paint_path(path, color);
        }
    }
}

// ---------------------------------------------------------------------------
// 小工具
// ---------------------------------------------------------------------------

fn monitor_config(cx: &mut Context<MonitorView>) -> MonitorConfig {
    let settings = &cx.global::<AppState>().settings;
    MonitorConfig {
        interval: Duration::from_secs(settings.monitor_interval_secs.max(1) as u64),
        cpu_threshold: settings.monitor_cpu_threshold,
        mem_threshold: settings.monitor_mem_threshold,
    }
    .sanitized()
}

fn auto_max(values: impl Iterator<Item = f32>, floor: f32) -> f32 {
    let max = values.fold(0.0_f32, f32::max);
    (max * 1.15).max(floor)
}

fn signal_label(signal: KillSignal) -> &'static str {
    match signal {
        KillSignal::Term => "SIGTERM",
        KillSignal::Kill => "SIGKILL",
    }
}

fn truncate_command(command: &str, max: usize) -> String {
    let mut chars = command.chars();
    let prefix: String = chars.by_ref().take(max).collect();
    if chars.next().is_some() {
        format!("{prefix}…")
    } else {
        prefix
    }
}

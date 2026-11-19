//! MonitorView — 远程性能监控 Tab。
//!
//! 分层：
//! - 采集全部在 `rterm-monitor` 的后台线程里完成，本文件只消费事件、持有展示状态
//! - 每个 Monitor Tab 独立 SSH 连接，不占用终端 / SFTP 的会话
//! - 图表使用 gpui Canvas 直接绘制，避免引入额外依赖

use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;
use std::time::Duration;

use futures::channel::mpsc::unbounded;
use futures::StreamExt;
use gpui::prelude::*;
use gpui::{
    canvas, div, point, px, AnyElement, Bounds, ClickEvent, Context, FocusHandle, Focusable, Hsla,
    IntoElement, MouseButton, MouseDownEvent, MouseMoveEvent, Path, PathBuilder, Pixels, Point,
    Render, Rgba, Window,
};
use uuid::Uuid;

use rterm_monitor::{
    spawn, DiskEntry, DiskUsage, Metric, MonitorAlert, MonitorCommand, MonitorConfig, MonitorEvent,
    MonitorHandle, ResourceSample,
};
use rterm_sftp::format_size;
use rterm_ssh::SshSession;

use crate::event::{AppEvent, EventBusHost};
use crate::state::{lock_or_recover, AppState};
use crate::ui::theme;

/// 图表窗口长度（60 点 × 2s ≈ 2 分钟）。
const MAX_SAMPLES: usize = 60;
const CARD_H: f32 = 146.0;
/// 磁盘占用分析饼图直径。
const DISK_CHART_SIZE: f32 = 240.0;
/// 图例单项宽度。
const LEGEND_ITEM_W: f32 = 264.0;
/// 磁盘使用率进入告警配色的阈值。
const DISK_DANGER_PCT: f32 = 90.0;

// ---------------------------------------------------------------------------
// 展示状态
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq)]
enum MonitorStatus {
    Connecting,
    Connected,
    Failed,
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
    paused: bool,
    error: Option<String>,
    alert: Option<MonitorAlert>,
    generation: u64,
    /// 当前查看的目录（也是最近一次扫描请求的路径）。
    disk_path: String,
    /// 最近一次扫描结果。
    disk_usage: Option<DiskUsage>,
    /// 等待中的扫描请求 id（用于丢弃过期结果）。
    disk_request: Option<u64>,
    disk_seq: u64,
    disk_error: Option<String>,
    /// 悬停的饼图扇区 / 图例下标。
    disk_hover: Option<usize>,
    /// 饼图绘制区域（窗口坐标），供鼠标命中测试使用。
    disk_chart_bounds: Rc<RefCell<Option<Bounds<Pixels>>>>,
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
            paused: false,
            error: None,
            alert: None,
            generation: 0,
            disk_path: "/".to_string(),
            disk_usage: None,
            disk_request: None,
            disk_seq: 0,
            disk_error: None,
            disk_hover: None,
            disk_chart_bounds: Rc::new(RefCell::new(None)),
        }
    }

    pub fn tab_id(&self) -> Uuid {
        self.tab_id
    }

    pub fn focus_handle(&self) -> FocusHandle {
        self.focus_handle.clone()
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
        self.request_disk_scan("/".to_string(), false, cx);

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
        self.disk_usage = None;
        self.disk_request = None;
        self.disk_hover = None;
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
            MonitorEvent::Tick(sample) => self.apply_tick(sample, cx),
            MonitorEvent::Alert(alert) => self.apply_alert(alert, cx),
            MonitorEvent::DiskScan { request_id, result } => {
                self.apply_disk_scan(request_id, result, cx)
            }
            MonitorEvent::Error(message) => {
                self.error = Some(message);
                cx.notify();
            }
        }
    }

    /// 请求扫描目录占用；`force = false` 时由采集线程复用 30 秒缓存。
    fn request_disk_scan(&mut self, path: String, force: bool, cx: &mut Context<Self>) {
        let Some(handle) = &self.handle else {
            return;
        };
        self.disk_seq = self.disk_seq.wrapping_add(1);
        let request_id = self.disk_seq;
        if handle.send(MonitorCommand::ScanDisk {
            request_id,
            path: path.clone(),
            force,
        }) {
            self.disk_path = path.clone();
            self.disk_request = Some(request_id);
            self.disk_error = None;
            self.disk_hover = None;
            if self.status == MonitorStatus::Connected {
                self.status_text = format!("已连接 · 扫描 {path}…");
            }
        } else {
            self.disk_error = Some("采集线程已退出，无法扫描目录".into());
        }
        cx.notify();
    }

    fn apply_disk_scan(
        &mut self,
        request_id: u64,
        result: Result<DiskUsage, String>,
        cx: &mut Context<Self>,
    ) {
        if self.disk_request != Some(request_id) {
            return;
        }
        self.disk_request = None;
        self.disk_hover = None;
        match result {
            Ok(usage) => {
                self.disk_path = usage.path.clone();
                self.disk_usage = Some(usage);
                self.disk_error = None;
                if self.status == MonitorStatus::Connected {
                    self.status_text = if self.paused {
                        "已连接 · 已暂停".to_string()
                    } else {
                        "已连接 · 实时采集中".to_string()
                    };
                }
            }
            Err(message) => {
                self.disk_error = Some(message);
            }
        }
        cx.notify();
    }

    /// 进入子目录 / 面包屑跳转（仅目录可导航，命中缓存时无需等待扫描）。
    fn navigate_disk(&mut self, path: String, cx: &mut Context<Self>) {
        self.request_disk_scan(path, false, cx);
    }

    fn disk_go_up(&mut self, cx: &mut Context<Self>) {
        if let Some(parent) = parent_path(&self.disk_path) {
            self.request_disk_scan(parent, false, cx);
        }
    }

    /// 鼠标当前落在哪个扇区。
    fn disk_hit_test(&self, position: Point<Pixels>) -> Option<usize> {
        let bounds = (*self.disk_chart_bounds.borrow())?;
        let entries = self.disk_usage.as_ref()?;
        let values: Vec<u64> = entries.entries.iter().map(|entry| entry.bytes).collect();
        hit_test_pie(bounds, position, &values)
    }

    fn apply_tick(&mut self, sample: ResourceSample, cx: &mut Context<Self>) {
        self.status_text = "已连接 · 实时采集中".to_string();
        self.error = None;

        // 指标恢复后自动收起告警条。
        if let Some(alert) = &self.alert {
            if sample.metric(alert.metric) < alert.threshold * 0.95 {
                self.alert = None;
            }
        }

        self.samples.push_back(sample.clone());
        while self.samples.len() > MAX_SAMPLES {
            self.samples.pop_front();
        }
        self.latest = Some(sample);
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
                    .child(net_card)
                    .child(load_card),
            )
            .into_any_element()
    }

    // -----------------------------------------------------------------------
    // 磁盘占用分析
    // -----------------------------------------------------------------------

    /// 交互式磁盘占用：饼图按目录/文件分段，悬停高亮，点击目录下钻。
    fn render_disk_panel(&self, cx: &mut Context<Self>) -> AnyElement {
        let t = theme(cx);
        let entries = self
            .disk_usage
            .as_ref()
            .map(|usage| usage.entries.as_slice())
            .unwrap_or(&[]);
        let scanned_total = self
            .disk_usage
            .as_ref()
            .map(|usage| usage.total_bytes)
            .unwrap_or(0);
        let palette = t.chart_palette();

        let (status_text, status_color) = if let Some(error) = &self.disk_error {
            (error.clone(), t.danger())
        } else if self.disk_request.is_some() {
            ("扫描中…".to_string(), t.primary())
        } else if let Some(usage) = &self.disk_usage {
            (
                format!(
                    "{} 项 · 合计 {}",
                    usage.entries.len(),
                    format_size(usage.total_bytes)
                ),
                t.text_subtle(),
            )
        } else {
            ("等待扫描".to_string(), t.text_subtle())
        };

        let capacity_total = self
            .latest
            .as_ref()
            .map(|s| s.disk_total_bytes)
            .unwrap_or(0);
        let capacity_used = self.latest.as_ref().map(|s| s.disk_used_bytes).unwrap_or(0);
        let capacity_pct = disk_ratio(capacity_used, capacity_total) * 100.0;
        let has_capacity = capacity_total > 0;
        let value_color = t.text_on_card_bg();
        let pct_color = if capacity_pct >= DISK_DANGER_PCT {
            t.danger()
        } else {
            value_color
        };

        div()
            .w_full()
            .flex_1()
            .min_h(px(0.))
            .bg(t.card_bg())
            .border_1()
            .border_color(t.border_subtle())
            .rounded_md()
            .p_3()
            .flex()
            .flex_col()
            .gap_3()
            .child(
                div()
                    .w_full()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(t.text_subtle())
                            .child("磁盘占用分析"),
                    )
                    .child(self.render_disk_breadcrumb(cx))
                    .child(div().flex_1())
                    .child(
                        div()
                            .text_size(px(10.))
                            .text_color(status_color)
                            .child(status_text),
                    )
                    .child(self.render_disk_up_button(cx))
                    .child(self.render_disk_refresh_button(cx)),
            )
            .child(
                div()
                    .w_full()
                    .flex_1()
                    .min_h(px(0.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .gap_8()
                    .child(self.render_disk_chart(entries, scanned_total, &palette, cx))
                    .child(self.render_disk_legend(entries, scanned_total, &palette, cx)),
            )
            .child(
                div()
                    .w_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .gap_8()
                    .child(self.render_disk_stat(
                        "总容量",
                        if has_capacity {
                            format_size(capacity_total)
                        } else {
                            "—".into()
                        },
                        value_color,
                        cx,
                    ))
                    .child(self.render_disk_stat(
                        "已用",
                        if has_capacity {
                            format_size(capacity_used)
                        } else {
                            "—".into()
                        },
                        value_color,
                        cx,
                    ))
                    .child(self.render_disk_stat(
                        "可用",
                        if has_capacity {
                            format_size(capacity_total.saturating_sub(capacity_used))
                        } else {
                            "—".into()
                        },
                        value_color,
                        cx,
                    ))
                    .child(self.render_disk_stat(
                        "使用率",
                        if has_capacity {
                            format!("{capacity_pct:.0}%")
                        } else {
                            "—".into()
                        },
                        pct_color,
                        cx,
                    )),
            )
            .into_any_element()
    }

    /// 面包屑：根 `/` 及当前路径的每一级都可点击跳转。
    fn render_disk_breadcrumb(&self, cx: &mut Context<Self>) -> AnyElement {
        let t = theme(cx);
        let hint = t.text_subtle();
        let mut items: Vec<AnyElement> = Vec::new();
        let mut accumulated = String::new();
        items.push(self.render_crumb("/", "/".to_string(), cx));
        for part in self.disk_path.split('/').filter(|part| !part.is_empty()) {
            accumulated.push('/');
            accumulated.push_str(part);
            items.push(
                div()
                    .text_size(px(10.))
                    .text_color(hint)
                    .child("›")
                    .into_any_element(),
            );
            items.push(self.render_crumb(part, accumulated.clone(), cx));
        }
        div()
            .flex()
            .items_center()
            .gap_1()
            .children(items)
            .into_any_element()
    }

    fn render_crumb(&self, label: &str, target: String, cx: &mut Context<Self>) -> AnyElement {
        let t = theme(cx);
        let hover = t.row_hover_bg();
        let text = label.to_string();
        div()
            .id(format!("monitor-crumb-{target}"))
            .px_1()
            .h(px(20.))
            .rounded_sm()
            .flex()
            .items_center()
            .text_size(px(10.))
            .text_color(t.text_on_card_bg())
            .cursor_pointer()
            .hover(move |style| style.bg(hover))
            .on_click(
                cx.listener(move |this: &mut Self, _ev: &ClickEvent, _w, cx| {
                    this.navigate_disk(target.clone(), cx);
                }),
            )
            .child(text)
            .into_any_element()
    }

    fn render_disk_up_button(&self, cx: &mut Context<Self>) -> AnyElement {
        let t = theme(cx);
        let enabled = parent_path(&self.disk_path).is_some();
        let hover = t.secondary_btn_hover();
        let mut button = div()
            .id("monitor-disk-up")
            .h(px(22.))
            .px_2()
            .rounded_md()
            .flex()
            .items_center()
            .text_size(px(10.))
            .bg(if enabled {
                t.secondary_btn_bg()
            } else {
                t.card_bg()
            })
            .text_color(if enabled {
                t.text_on_secondary_btn()
            } else {
                t.text_subtle()
            });
        if enabled {
            button = button
                .cursor_pointer()
                .hover(move |style| style.bg(hover))
                .on_click(cx.listener(|this: &mut Self, _ev: &ClickEvent, _w, cx| {
                    this.disk_go_up(cx);
                }));
        }
        button.child("← 上级").into_any_element()
    }

    fn render_disk_refresh_button(&self, cx: &mut Context<Self>) -> AnyElement {
        let t = theme(cx);
        let enabled = self.disk_request.is_none();
        let hover = t.secondary_btn_hover();
        let mut button = div()
            .id("monitor-disk-refresh")
            .h(px(22.))
            .px_2()
            .rounded_md()
            .flex()
            .items_center()
            .text_size(px(10.))
            .bg(if enabled {
                t.secondary_btn_bg()
            } else {
                t.card_bg()
            })
            .text_color(if enabled {
                t.text_on_secondary_btn()
            } else {
                t.text_subtle()
            });
        if enabled {
            button = button
                .cursor_pointer()
                .hover(move |style| style.bg(hover))
                .on_click(cx.listener(|this: &mut Self, _ev: &ClickEvent, _w, cx| {
                    let path = this.disk_path.clone();
                    // 刷新按钮跳过缓存，强制重新扫描。
                    this.request_disk_scan(path, true, cx);
                }));
        }
        button.child("⟳ 刷新").into_any_element()
    }

    fn render_disk_chart(
        &self,
        entries: &[DiskEntry],
        total: u64,
        palette: &[Rgba; 8],
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = theme(cx);
        let hover = self.disk_hover;
        let fractions = pie_fractions(entries.iter().map(|entry| entry.bytes));
        let colors: Vec<Rgba> = (0..entries.len())
            .map(|index| palette[index % palette.len()])
            .collect();
        let targets: Vec<(String, bool)> = entries
            .iter()
            .map(|entry| (entry.path.clone(), entry.is_dir))
            .collect();

        let base_color = t.secondary_btn_bg();
        let hole_color = t.card_bg();
        let bounds_paint = self.disk_chart_bounds.clone();

        let (center_main, center_sub) = match hover.and_then(|index| entries.get(index)) {
            Some(entry) => (truncate_label(&entry.name, 14), format_size(entry.bytes)),
            None if self.disk_request.is_some() => ("扫描中…".to_string(), self.disk_path.clone()),
            None if total > 0 => (format_size(total), "当前目录".to_string()),
            None => ("—".to_string(), "当前目录".to_string()),
        };

        div()
            .relative()
            .w(px(DISK_CHART_SIZE))
            .h(px(DISK_CHART_SIZE))
            .child(
                canvas(
                    move |_bounds, _window, _cx| {},
                    move |bounds, _state, window, _cx| {
                        *bounds_paint.borrow_mut() = Some(bounds);
                        paint_disk_breakdown(
                            bounds, &fractions, &colors, base_color, hole_color, hover, window,
                        );
                    },
                )
                .size_full(),
            )
            .child(
                div()
                    .absolute()
                    .inset_0()
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .gap_1()
                    .child(
                        div()
                            .text_size(px(if hover.is_some() { 15. } else { 20. }))
                            .text_color(t.text_on_card_bg())
                            .child(center_main),
                    )
                    .child(
                        div()
                            .px_4()
                            .truncate()
                            .text_size(px(10.))
                            .text_color(t.text_subtle())
                            .child(center_sub),
                    ),
            )
            .on_mouse_move(
                cx.listener(|this: &mut Self, event: &MouseMoveEvent, _w, cx| {
                    let next = this.disk_hit_test(event.position);
                    if this.disk_hover != next {
                        this.disk_hover = next;
                        cx.notify();
                    }
                }),
            )
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this: &mut Self, event: &MouseDownEvent, _w, cx| {
                    let Some(index) = this.disk_hit_test(event.position) else {
                        return;
                    };
                    let Some((path, is_dir)) = targets.get(index).cloned() else {
                        return;
                    };
                    if is_dir {
                        this.navigate_disk(path, cx);
                    }
                }),
            )
            .on_mouse_exit(cx.listener(|this: &mut Self, _ev, _w, cx| {
                if this.disk_hover.is_some() {
                    this.disk_hover = None;
                    cx.notify();
                }
            }))
            .into_any_element()
    }

    fn render_disk_legend(
        &self,
        entries: &[DiskEntry],
        total: u64,
        palette: &[Rgba; 8],
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = theme(cx);
        if entries.is_empty() {
            return div()
                .flex_1()
                .flex()
                .items_center()
                .justify_center()
                .text_size(px(11.))
                .text_color(t.text_subtle())
                .child(if self.disk_request.is_some() {
                    "正在扫描目录占用…"
                } else {
                    "该目录下没有可统计的内容"
                })
                .into_any_element();
        }

        let rows = entries
            .iter()
            .enumerate()
            .map(|(index, entry)| {
                let color = palette[index % palette.len()];
                let hovered = self.disk_hover == Some(index);
                let row_bg = if hovered {
                    t.row_hover_bg()
                } else {
                    t.card_bg()
                };
                let pct = if total > 0 {
                    (entry.bytes as f64 / total as f64 * 100.0).clamp(0.0, 100.0)
                } else {
                    0.0
                };
                let label = if entry.is_dir {
                    format!("{}/", entry.name)
                } else {
                    entry.name.clone()
                };
                let path = entry.path.clone();
                let is_dir = entry.is_dir;

                let mut row = div()
                    .id(format!("monitor-legend-{index}"))
                    .w(px(LEGEND_ITEM_W))
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .bg(row_bg)
                    .flex()
                    .items_center()
                    .gap_2()
                    .on_hover(cx.listener(move |this: &mut Self, hovered: &bool, _w, cx| {
                        let next = if *hovered {
                            Some(index)
                        } else if this.disk_hover == Some(index) {
                            None
                        } else {
                            return;
                        };
                        if this.disk_hover != next {
                            this.disk_hover = next;
                            cx.notify();
                        }
                    }))
                    .child(div().w(px(8.)).h(px(8.)).rounded_full().bg(color))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .truncate()
                            .text_size(px(10.))
                            .text_color(t.text_on_card_bg())
                            .child(label),
                    )
                    .child(
                        div()
                            .text_size(px(10.))
                            .text_color(t.text_subtle())
                            .child(format_size(entry.bytes)),
                    )
                    .child(
                        div()
                            .w(px(44.))
                            .text_size(px(9.))
                            .text_color(t.text_subtle())
                            .child(format!("{pct:.1}%")),
                    );
                if is_dir {
                    row = row.cursor_pointer().on_click(cx.listener(
                        move |this: &mut Self, _ev: &ClickEvent, _w, cx| {
                            this.navigate_disk(path.clone(), cx);
                        },
                    ));
                }
                row.into_any_element()
            })
            .collect::<Vec<_>>();

        div()
            .flex_1()
            .min_w(px(0.))
            .flex()
            .items_center()
            .child(div().w_full().flex().flex_wrap().gap_2().children(rows))
            .into_any_element()
    }

    fn render_disk_stat(
        &self,
        label: &'static str,
        value: String,
        value_color: Rgba,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = theme(cx);
        div()
            .flex()
            .flex_col()
            .items_center()
            .gap_1()
            .child(
                div()
                    .text_size(px(10.))
                    .text_color(t.text_subtle())
                    .child(label),
            )
            .child(
                div()
                    .text_size(px(13.))
                    .text_color(value_color)
                    .child(value),
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

        root.child(self.render_disk_panel(cx))
    }
}

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
            Bounds::new(
                point(bounds.left(), y),
                gpui::size(bounds.size.width, px(1.0)),
            ),
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

/// 目录占比环形图：按 `fractions` 依次绘制扇区，悬停扇区放大高亮。
fn paint_disk_breakdown(
    bounds: Bounds<Pixels>,
    fractions: &[f32],
    colors: &[Rgba],
    base_color: Rgba,
    hole_color: Rgba,
    hover: Option<usize>,
    window: &mut Window,
) {
    let width = f32::from(bounds.size.width);
    let height = f32::from(bounds.size.height);
    let diameter = width.min(height);
    if diameter <= 8.0 {
        return;
    }

    let center = point(
        bounds.left() + px(width / 2.0),
        bounds.top() + px(height / 2.0),
    );
    let radius = diameter / 2.0 - 1.0;

    if let Some(path) = circle_path(center, radius) {
        let color: Hsla = base_color.into();
        window.paint_path(path, color);
    }

    // 先累加出每个扇区的角度区间，保证悬停放大时角度与命中测试一致。
    let mut spans = Vec::with_capacity(fractions.len());
    let mut cumulative = 0.0_f32;
    for fraction in fractions {
        let start = cumulative;
        cumulative = (cumulative + fraction.max(0.0)).min(1.0);
        spans.push((start, cumulative));
    }

    let paint_slice = |index: usize, slice_radius: f32, window: &mut Window| {
        let (start, end) = spans[index];
        let start_deg = -90.0 + start * 360.0;
        let end_deg = -90.0 + end * 360.0;
        if end_deg - start_deg < 0.05 {
            return;
        }
        if let Some(path) = pie_slice_path(center, slice_radius, start_deg, end_deg) {
            let color: Hsla = colors[index].into();
            window.paint_path(path, color);
        }
    };

    for index in 0..spans.len() {
        if hover != Some(index) {
            paint_slice(index, radius, window);
        }
    }
    if let Some(index) = hover {
        if index < spans.len() {
            paint_slice(index, radius + 6.0, window);
        }
    }

    if let Some(path) = circle_path(center, radius * 0.6) {
        let color: Hsla = hole_color.into();
        window.paint_path(path, color);
    }
}

/// 整圆（由两段半圆弧拼接，避免起终点重合导致路径退化）。
fn circle_path(center: Point<Pixels>, radius: f32) -> Option<Path<Pixels>> {
    let top = point(center.x, center.y - px(radius));
    let bottom = point(center.x, center.y + px(radius));
    let radii = point(px(radius), px(radius));

    let mut builder = PathBuilder::fill();
    builder.move_to(top);
    builder.arc_to(radii, px(0.0), false, true, bottom);
    builder.arc_to(radii, px(0.0), false, true, top);
    builder.close();
    builder.build().ok()
}

/// 从 `start_deg` 顺时针扫到 `end_deg` 的扇形（角度以 12 点方向为 -90°）。
fn pie_slice_path(
    center: Point<Pixels>,
    radius: f32,
    start_deg: f32,
    end_deg: f32,
) -> Option<Path<Pixels>> {
    let sweep = end_deg - start_deg;
    if sweep >= 359.99 {
        return circle_path(center, radius);
    }

    let start = polar_point(center, radius, start_deg);
    let end = polar_point(center, radius, end_deg);

    let mut builder = PathBuilder::fill();
    builder.move_to(center);
    builder.line_to(start);
    builder.arc_to(
        point(px(radius), px(radius)),
        px(0.0),
        sweep > 180.0,
        true,
        end,
    );
    builder.close();
    builder.build().ok()
}

/// 角度以 12 点方向为 -90°、屏幕坐标顺时针为正。
fn polar_point(center: Point<Pixels>, radius: f32, degrees: f32) -> Point<Pixels> {
    let radians = degrees.to_radians();
    point(
        center.x + px(radius * radians.cos()),
        center.y + px(radius * radians.sin()),
    )
}

// ---------------------------------------------------------------------------
// 小工具
// ---------------------------------------------------------------------------

/// 每段占比 `0..=1`；全部为 0（空目录）时返回等长 0 向量。
fn pie_fractions(sizes: impl Iterator<Item = u64>) -> Vec<f32> {
    let values: Vec<u64> = sizes.collect();
    let total: u64 = values.iter().sum();
    if total == 0 {
        return vec![0.0; values.len()];
    }
    values
        .iter()
        .map(|value| *value as f32 / total as f32)
        .collect()
}

/// 饼图命中测试：角度以 12 点为 0°、顺时针递增。
fn hit_test_pie(bounds: Bounds<Pixels>, position: Point<Pixels>, values: &[u64]) -> Option<usize> {
    let total: u64 = values.iter().sum();
    if total == 0 {
        return None;
    }

    let width = f32::from(bounds.size.width);
    let height = f32::from(bounds.size.height);
    let center_x = f32::from(bounds.left()) + width / 2.0;
    let center_y = f32::from(bounds.top()) + height / 2.0;
    let dx = f32::from(position.x) - center_x;
    let dy = f32::from(position.y) - center_y;
    let distance = (dx * dx + dy * dy).sqrt();
    let radius = width.min(height) / 2.0 - 1.0;

    // 只响应圆环区域（悬停放大预留 8px）。
    if distance < radius * 0.6 || distance > radius + 8.0 {
        return None;
    }

    let fraction = (dy.atan2(dx).to_degrees() + 90.0).rem_euclid(360.0) / 360.0;
    let mut cumulative = 0.0_f32;
    for (index, value) in values.iter().enumerate() {
        cumulative += *value as f32 / total as f32;
        if fraction <= cumulative {
            return Some(index);
        }
    }
    Some(values.len() - 1)
}

/// 上一级目录；根目录返回 `None`。
fn parent_path(path: &str) -> Option<String> {
    let trimmed = path.trim_end_matches('/');
    if trimmed.is_empty() {
        return None;
    }
    match trimmed.rfind('/') {
        Some(0) => Some("/".to_string()),
        Some(index) => Some(trimmed[..index].to_string()),
        None => None,
    }
}

/// 饼图中心文字截断（按字符而非字节）。
fn truncate_label(label: &str, max_chars: usize) -> String {
    let mut chars = label.chars();
    let prefix: String = chars.by_ref().take(max_chars).collect();
    if chars.next().is_some() {
        format!("{prefix}…")
    } else {
        prefix
    }
}

fn monitor_config(cx: &mut Context<MonitorView>) -> MonitorConfig {
    let settings = &cx.global::<AppState>().settings;
    MonitorConfig {
        interval: Duration::from_secs(settings.monitor_interval_secs.max(1) as u64),
        cpu_threshold: settings.monitor_cpu_threshold,
        mem_threshold: settings.monitor_mem_threshold,
    }
    .sanitized()
}

/// 已用容量占比 `0..=1`；总容量未知时返回 0。
fn disk_ratio(used_bytes: u64, total_bytes: u64) -> f32 {
    if total_bytes == 0 {
        0.0
    } else {
        (used_bytes as f64 / total_bytes as f64).clamp(0.0, 1.0) as f32
    }
}

fn auto_max(values: impl Iterator<Item = f32>, floor: f32) -> f32 {
    let max = values.fold(0.0_f32, f32::max);
    (max * 1.15).max(floor)
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::size;

    #[test]
    fn disk_ratio_is_clamped() {
        assert_eq!(disk_ratio(0, 0), 0.0);
        assert_eq!(disk_ratio(50, 100), 0.5);
        assert_eq!(disk_ratio(200, 100), 1.0);
    }

    #[test]
    fn parent_path_walks_up() {
        assert_eq!(parent_path("/"), None);
        assert_eq!(parent_path("/var"), Some("/".to_string()));
        assert_eq!(parent_path("/var/log"), Some("/var".to_string()));
        assert_eq!(parent_path("/var/log/"), Some("/var".to_string()));
    }

    #[test]
    fn pie_fractions_normalize() {
        assert_eq!(pie_fractions([1_u64, 3].into_iter()), vec![0.25, 0.75]);
        assert_eq!(pie_fractions([0_u64, 0].into_iter()), vec![0.0, 0.0]);
    }

    #[test]
    fn hit_test_maps_angles_to_slices() {
        let bounds = Bounds::new(point(px(0.), px(0.)), size(px(240.), px(240.)));
        // 12 点方向 → 第一段
        assert_eq!(
            hit_test_pie(bounds, point(px(120.), px(30.)), &[1, 3]),
            Some(0)
        );
        // 6 点方向（累计 75%）→ 第二段
        assert_eq!(
            hit_test_pie(bounds, point(px(120.), px(210.)), &[1, 3]),
            Some(1)
        );
        // 圆环内部 / 圆外 → 未命中
        assert_eq!(
            hit_test_pie(bounds, point(px(120.), px(120.)), &[1, 3]),
            None
        );
        assert_eq!(hit_test_pie(bounds, point(px(0.), px(0.)), &[1, 3]), None);
        // 空目录不响应
        assert_eq!(
            hit_test_pie(bounds, point(px(120.), px(30.)), &[0, 0]),
            None
        );
    }

    #[test]
    fn label_truncation_counts_chars() {
        assert_eq!(truncate_label("abcdef", 3), "abc…");
        assert_eq!(truncate_label("目录", 4), "目录");
        assert_eq!(truncate_label("目录名称过长", 2), "目录…");
    }
}

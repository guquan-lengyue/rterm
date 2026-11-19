//! PortForwardView — 端口转发 Tab
//!
//! 分层：
//! - 规则数据保存在 `Session.port_forwards`（本 View 只做展示与编辑）
//! - 转发运行时在 `rterm-forward` 的后台线程，状态经 channel 回到本 View
//! - 每条规则使用独立 SSH 连接；关闭 Tab 时统一停止

use std::collections::HashMap;

use futures::channel::mpsc::{unbounded, UnboundedSender};
use futures::StreamExt;
use gpui::prelude::*;
use gpui::{
    div, px, AnyElement, Context, Entity, FocusHandle, Focusable, IntoElement, Render,
    ScrollHandle, Window,
};
use uuid::Uuid;

use rterm_forward::{start_forward, ForwardEvent, ForwardHandle, ForwardStatus};
use rterm_session::{ForwardKind, PortForward, Session};

use crate::event::{AppEvent, EventBusHost};
use crate::state::{lock_or_recover, AppState};
use crate::ui::theme;

use super::text_input::TextInput;

// ---------------------------------------------------------------------------
// 规则编辑器
// ---------------------------------------------------------------------------

struct RuleEditor {
    rule_id: Option<Uuid>,
    kind: ForwardKind,
    bind_host: Entity<TextInput>,
    bind_port: Entity<TextInput>,
    target_host: Entity<TextInput>,
    target_port: Entity<TextInput>,
    auto_start: bool,
    error: Option<String>,
}

impl RuleEditor {
    fn from_rule(cx: &mut Context<PortForwardView>, rule: Option<&PortForward>) -> Self {
        let make = |cx: &mut Context<PortForwardView>, value: String, placeholder: &str| {
            cx.new(|cx| {
                TextInput::new(cx)
                    .with_content(value)
                    .with_placeholder(placeholder)
            })
        };
        let rule = rule.cloned();
        Self {
            rule_id: rule.as_ref().map(|rule| rule.id),
            kind: rule.as_ref().map(|rule| rule.kind).unwrap_or_default(),
            bind_host: make(
                cx,
                rule.as_ref()
                    .map(|rule| rule.bind_host.clone())
                    .unwrap_or_else(|| "127.0.0.1".to_string()),
                "127.0.0.1",
            ),
            bind_port: make(
                cx,
                rule.as_ref()
                    .map(|rule| rule.bind_port.to_string())
                    .unwrap_or_default(),
                "监听端口",
            ),
            target_host: make(
                cx,
                rule.as_ref()
                    .map(|rule| rule.target_host.clone())
                    .unwrap_or_else(|| "127.0.0.1".to_string()),
                "目标地址",
            ),
            target_port: make(
                cx,
                rule.as_ref()
                    .map(|rule| rule.target_port.to_string())
                    .unwrap_or_default(),
                "目标端口",
            ),
            auto_start: rule.as_ref().map(|rule| rule.auto_start).unwrap_or(false),
            error: None,
        }
    }

    fn is_new(&self) -> bool {
        self.rule_id.is_none()
    }

    fn parse(&self, cx: &Context<PortForwardView>) -> Result<PortForward, String> {
        let bind_port = parse_port(&self.bind_port.read(cx).content, "监听端口")?;
        let target_port = if self.kind.needs_target() {
            parse_port(&self.target_port.read(cx).content, "目标端口")?
        } else {
            0
        };
        let rule = PortForward {
            id: self.rule_id.unwrap_or_else(Uuid::new_v4),
            kind: self.kind,
            bind_host: self.bind_host.read(cx).content.trim().to_string(),
            bind_port,
            target_host: if self.kind.needs_target() {
                self.target_host.read(cx).content.trim().to_string()
            } else {
                "127.0.0.1".to_string()
            },
            target_port,
            auto_start: self.auto_start,
        };
        rule.validate()?;
        Ok(rule)
    }
}

fn parse_port(text: &str, label: &str) -> Result<u16, String> {
    match text.trim().parse::<u16>() {
        Ok(port) if port >= 1 => Ok(port),
        _ => Err(format!("{label}必须是 1–65535 的数字")),
    }
}

/// 非回环绑定判定（需要二次确认）。
fn is_loopback(host: &str) -> bool {
    matches!(
        host.trim().to_ascii_lowercase().as_str(),
        "127.0.0.1" | "localhost" | "::1"
    )
}

// ---------------------------------------------------------------------------
// PortForwardView
// ---------------------------------------------------------------------------

pub struct PortForwardView {
    tab_id: Uuid,
    session_id: Option<Uuid>,
    session_name: String,
    statuses: HashMap<Uuid, ForwardStatus>,
    handles: HashMap<Uuid, ForwardHandle>,
    events_tx: UnboundedSender<ForwardEvent>,
    editor: Option<RuleEditor>,
    /// 非回环绑定待确认的规则。
    pending_loopback_confirm: Option<PortForward>,
    focus_handle: FocusHandle,
    list_scroll: ScrollHandle,
    message: Option<String>,
}

impl PortForwardView {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let (events_tx, mut events_rx) = unbounded::<ForwardEvent>();
        cx.spawn(async move |this, cx| {
            while let Some(event) = events_rx.next().await {
                let alive = this
                    .update(cx, |this: &mut Self, cx| this.on_forward_event(event, cx))
                    .is_ok();
                if !alive {
                    break;
                }
            }
        })
        .detach();

        Self {
            tab_id: Uuid::new_v4(),
            session_id: None,
            session_name: String::new(),
            statuses: HashMap::new(),
            handles: HashMap::new(),
            events_tx,
            editor: None,
            pending_loopback_confirm: None,
            focus_handle: cx.focus_handle(),
            list_scroll: ScrollHandle::new(),
            message: None,
        }
    }

    pub fn tab_id(&self) -> Uuid {
        self.tab_id
    }

    pub fn focus_handle(&self) -> FocusHandle {
        self.focus_handle.clone()
    }

    /// 绑定会话并自动启动 `auto_start` 规则。
    pub fn connect(&mut self, session_id: Uuid, cx: &mut Context<Self>) {
        self.session_id = Some(session_id);
        self.session_name = {
            let store = lock_or_recover(&cx.global::<AppState>().session_store);
            store
                .get(session_id)
                .map(|session| session.display_label())
                .unwrap_or_else(|| "端口转发".to_string())
        };
        let auto_rules: Vec<PortForward> = self
            .rules(cx)
            .into_iter()
            .filter(|rule| rule.auto_start)
            .collect();
        for rule in auto_rules {
            self.start_rule(rule.id, cx);
        }
        self.emit(
            cx,
            AppEvent::PortForwardOpened {
                tab_id: self.tab_id,
                session_id,
            },
        );
        cx.notify();
    }

    pub fn shutdown(&mut self, cx: &mut Context<Self>) {
        self.stop_all(cx);
        self.emit(
            cx,
            AppEvent::PortForwardClosed {
                tab_id: self.tab_id,
            },
        );
        cx.notify();
    }

    fn emit(&self, cx: &mut Context<Self>, event: AppEvent) {
        let host = cx.global::<AppState>().event_bus_host.clone();
        cx.update_entity(&host, |_host, cx: &mut Context<EventBusHost>| {
            cx.emit(event);
        });
    }

    fn rules(&self, cx: &Context<Self>) -> Vec<PortForward> {
        let Some(session_id) = self.session_id else {
            return Vec::new();
        };
        let store = lock_or_recover(&cx.global::<AppState>().session_store);
        store
            .get(session_id)
            .map(|session| session.port_forwards.clone())
            .unwrap_or_default()
    }

    fn decrypt_session(&self, cx: &Context<Self>) -> Result<Session, String> {
        let Some(session_id) = self.session_id else {
            return Err("未绑定会话".into());
        };
        let stored = {
            let store = lock_or_recover(&cx.global::<AppState>().session_store);
            store.get(session_id).cloned()
        };
        let stored = stored.ok_or_else(|| "会话不存在".to_string())?;
        rterm_session::crypto_bridge::decrypt_session_dpapi(&stored)
            .map_err(|error| format!("解密会话失败: {error:#}"))
    }

    fn start_rule(&mut self, rule_id: Uuid, cx: &mut Context<Self>) {
        if self.handles.contains_key(&rule_id) {
            return;
        }
        let Some(rule) = self.rules(cx).into_iter().find(|rule| rule.id == rule_id) else {
            self.message = Some("规则不存在".into());
            cx.notify();
            return;
        };
        let config = match self.decrypt_session(cx) {
            Ok(config) => config,
            Err(error) => {
                self.statuses.insert(rule_id, ForwardStatus::Error(error));
                cx.notify();
                return;
            }
        };

        match start_forward(config, rule, self.events_tx.clone()) {
            Ok(handle) => {
                self.handles.insert(rule_id, handle);
                self.statuses.insert(rule_id, ForwardStatus::Starting);
                self.message = None;
            }
            Err(error) => {
                self.statuses
                    .insert(rule_id, ForwardStatus::Error(format!("{error:#}")));
            }
        }
        cx.notify();
    }

    fn stop_rule(&mut self, rule_id: Uuid, cx: &mut Context<Self>) {
        if let Some(mut handle) = self.handles.remove(&rule_id) {
            handle.stop();
        }
        self.statuses.remove(&rule_id);
        cx.notify();
    }

    fn stop_all(&mut self, _cx: &mut Context<Self>) {
        for (_, mut handle) in self.handles.drain() {
            handle.stop();
        }
        self.statuses.clear();
    }

    fn on_forward_event(&mut self, event: ForwardEvent, cx: &mut Context<Self>) {
        match &event.status {
            ForwardStatus::Running | ForwardStatus::Starting => {
                self.statuses.insert(event.rule_id, event.status);
            }
            ForwardStatus::Error(reason) => {
                self.statuses
                    .insert(event.rule_id, ForwardStatus::Error(reason.clone()));
                // 线程已退出，句柄可安全回收。
                self.handles.remove(&event.rule_id);
            }
            ForwardStatus::Stopped => {
                self.statuses.remove(&event.rule_id);
                self.handles.remove(&event.rule_id);
            }
        }
        cx.notify();
    }

    // -----------------------------------------------------------------------
    // 规则编辑 / 持久化
    // -----------------------------------------------------------------------

    fn open_editor(&mut self, rule: Option<&PortForward>, cx: &mut Context<Self>) {
        self.editor = Some(RuleEditor::from_rule(cx, rule));
        cx.notify();
    }

    fn save_editor(&mut self, cx: &mut Context<Self>) {
        let Some(editor) = self.editor.as_ref() else {
            return;
        };
        let rule = match editor.parse(cx) {
            Ok(rule) => rule,
            Err(error) => {
                if let Some(editor) = self.editor.as_mut() {
                    editor.error = Some(error);
                }
                cx.notify();
                return;
            }
        };

        if !is_loopback(&rule.bind_host) {
            self.pending_loopback_confirm = Some(rule);
            cx.notify();
            return;
        }
        self.commit_rule(rule, cx);
    }

    fn commit_rule(&mut self, rule: PortForward, cx: &mut Context<Self>) {
        let Some(session_id) = self.session_id else {
            return;
        };

        let result: Result<(), String> = {
            let state = cx.global_mut::<AppState>();
            let mut store = lock_or_recover(&state.session_store);
            match store.get_mut(session_id) {
                Some(session) => {
                    // 先在副本上合并并校验，失败时不修改原数据。
                    let mut rules = session.port_forwards.clone();
                    match rules.iter().position(|existing| existing.id == rule.id) {
                        Some(index) => rules[index] = rule.clone(),
                        None => rules.push(rule.clone()),
                    }
                    match rterm_session::validate_port_forwards(&rules) {
                        Ok(()) => {
                            session.port_forwards = rules;
                            session.last_modified = rterm_session::default_now();
                            Ok(())
                        }
                        Err(error) => Err(error),
                    }
                }
                None => Err("会话不存在".into()),
            }
        };

        match result {
            Ok(()) => {
                // 规则变更后重启运行时，确保监听参数一致。
                self.stop_rule(rule.id, cx);
                let store = cx.global::<AppState>().session_store.clone();
                if let Err(error) = crate::state::persist_sessions(&store) {
                    log::error!("保存端口转发规则失败: {error:#}");
                }
                self.emit(cx, AppEvent::SessionUpdated { session_id });
                self.editor = None;
                self.pending_loopback_confirm = None;
                self.message = Some(format!("已保存规则: {}", rule.summary()));
            }
            Err(error) => {
                if let Some(editor) = self.editor.as_mut() {
                    editor.error = Some(error);
                }
            }
        }
        cx.notify();
    }

    fn delete_rule(&mut self, rule_id: Uuid, cx: &mut Context<Self>) {
        let Some(session_id) = self.session_id else {
            return;
        };
        self.stop_rule(rule_id, cx);
        {
            let state = cx.global_mut::<AppState>();
            let mut store = lock_or_recover(&state.session_store);
            if let Some(session) = store.get_mut(session_id) {
                session.port_forwards.retain(|rule| rule.id != rule_id);
                session.last_modified = rterm_session::default_now();
            }
        }
        let store = cx.global::<AppState>().session_store.clone();
        if let Err(error) = crate::state::persist_sessions(&store) {
            log::error!("删除端口转发规则后保存失败: {error:#}");
        }
        self.emit(cx, AppEvent::SessionUpdated { session_id });
        self.message = Some("已删除规则".into());
        cx.notify();
    }

    fn toggle_auto_start(&mut self, rule_id: Uuid, cx: &mut Context<Self>) {
        let Some(session_id) = self.session_id else {
            return;
        };
        let mut changed = false;
        {
            let state = cx.global_mut::<AppState>();
            let mut store = lock_or_recover(&state.session_store);
            if let Some(session) = store.get_mut(session_id) {
                if let Some(rule) = session
                    .port_forwards
                    .iter_mut()
                    .find(|rule| rule.id == rule_id)
                {
                    rule.auto_start = !rule.auto_start;
                    changed = true;
                }
            }
        }
        if changed {
            let store = cx.global::<AppState>().session_store.clone();
            if let Err(error) = crate::state::persist_sessions(&store) {
                log::error!("切换自动启动失败: {error:#}");
            }
            cx.notify();
        }
    }
}

impl Focusable for PortForwardView {
    fn focus_handle(&self, _cx: &gpui::App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

// ---------------------------------------------------------------------------
// Render
// ---------------------------------------------------------------------------

impl Render for PortForwardView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        let _ = &self.focus_handle;
        let rules = self.rules(cx);
        let running = self.handles.len();

        let mut root = div().size_full().bg(t.app_bg()).flex().flex_col().child(
            div()
                .w_full()
                .h(px(34.))
                .px_3()
                .flex()
                .items_center()
                .gap_2()
                .bg(t.toolbar_bg())
                .border_b_1()
                .border_color(t.border_subtle())
                .text_size(px(12.))
                .text_color(t.text_on_toolbar_bg())
                .child(format!("🔀 {} · 端口转发", self.session_name))
                .child(div().flex_1())
                .child(self.action_button(
                    "pf-add",
                    "＋ 新增规则",
                    true,
                    true,
                    cx,
                    |this, cx| this.open_editor(None, cx),
                ))
                .child(self.action_button(
                    "pf-start-all",
                    "全部启动",
                    false,
                    !rules.is_empty(),
                    cx,
                    |this, cx| {
                        for rule in this.rules(cx) {
                            this.start_rule(rule.id, cx);
                        }
                    },
                ))
                .child(self.action_button(
                    "pf-stop-all",
                    "全部停止",
                    false,
                    running > 0,
                    cx,
                    |this, cx| this.stop_all(cx),
                )),
        );

        if let Some(message) = self.message.clone() {
            root = root.child(
                div()
                    .w_full()
                    .px_3()
                    .py_1()
                    .text_size(px(10.))
                    .text_color(t.text_subtle())
                    .child(message),
            );
        }

        let mut list = div()
            .w_full()
            .flex_1()
            .min_h(px(0.))
            .p_3()
            .flex()
            .flex_col()
            .gap_1();
        if rules.is_empty() {
            list = list.child(
                div()
                    .w_full()
                    .pt_8()
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap_1()
                    .text_size(px(11.))
                    .text_color(t.text_subtle())
                    .child("还没有端口转发规则")
                    .child(
                        div()
                            .text_size(px(10.))
                            .text_color(t.text_placeholder())
                            .child("支持本地 -L / 远程 -R / 动态 -D"),
                    ),
            );
        }
        for rule in &rules {
            list = list.child(self.render_rule(rule, cx));
        }
        root = root.child(
            list.id("pf-list")
                .overflow_y_scroll()
                .track_scroll(&self.list_scroll),
        );

        if self.editor.is_some() {
            root = root.child(self.render_editor(cx));
        }
        if let Some(rule) = self.pending_loopback_confirm.clone() {
            root = root.child(self.render_loopback_confirm(rule, cx));
        }
        root
    }
}

impl PortForwardView {
    fn render_rule(&self, rule: &PortForward, cx: &mut Context<Self>) -> AnyElement {
        let t = theme(cx);
        let rule_id = rule.id;
        let running = self.handles.contains_key(&rule_id);
        let status = self.statuses.get(&rule_id).cloned();
        let (status_label, status_color) = match status {
            Some(ForwardStatus::Starting) => ("启动中".to_string(), t.primary()),
            Some(ForwardStatus::Running) => ("● 运行中".to_string(), t.success()),
            Some(ForwardStatus::Error(reason)) => (format!("错误: {reason}"), t.danger()),
            _ => ("未运行".to_string(), t.text_subtle()),
        };

        div()
            .w_full()
            .px_3()
            .py_2()
            .flex()
            .items_center()
            .gap_3()
            .rounded_md()
            .border_1()
            .border_color(t.border_subtle())
            .bg(t.card_bg())
            .text_size(px(11.))
            .child(
                div()
                    .w(px(70.))
                    .text_color(t.text_on_card_bg())
                    .child(rule.kind.label()),
            )
            .child(
                div()
                    .flex_1()
                    .truncate()
                    .text_color(t.text_on_card_bg())
                    .child(rule.summary()),
            )
            .child(
                div()
                    .w(px(60.))
                    .text_center()
                    .text_color(if rule.auto_start {
                        t.primary()
                    } else {
                        t.text_subtle()
                    })
                    .id(format!("pf-auto-{rule_id}"))
                    .cursor_pointer()
                    .child(if rule.auto_start {
                        "自动 ✓"
                    } else {
                        "自动 ✗"
                    })
                    .on_click(cx.listener(
                        move |this: &mut Self, _ev: &gpui::ClickEvent, _w, cx| {
                            this.toggle_auto_start(rule_id, cx);
                        },
                    )),
            )
            .child(
                div()
                    .w(px(190.))
                    .truncate()
                    .text_color(status_color)
                    .child(status_label),
            )
            .child(self.action_button(
                &format!("pf-toggle-{rule_id}"),
                if running { "停止" } else { "启动" },
                false,
                true,
                cx,
                move |this, cx| {
                    if this.handles.contains_key(&rule_id) {
                        this.stop_rule(rule_id, cx);
                    } else {
                        this.start_rule(rule_id, cx);
                    }
                },
            ))
            .child(self.action_button(
                &format!("pf-edit-{rule_id}"),
                "编辑",
                false,
                true,
                cx,
                move |this, cx| {
                    let rule = this.rules(cx).into_iter().find(|rule| rule.id == rule_id);
                    if let Some(rule) = rule {
                        this.open_editor(Some(&rule), cx);
                    }
                },
            ))
            .child(self.action_button(
                &format!("pf-delete-{rule_id}"),
                "删除",
                false,
                true,
                cx,
                move |this, cx| this.delete_rule(rule_id, cx),
            ))
            .into_any_element()
    }

    /// 通用按钮（`enabled = false` 时灰显且不响应）。
    fn action_button(
        &self,
        id: &str,
        label: &str,
        primary: bool,
        enabled: bool,
        cx: &mut Context<Self>,
        on_click: impl Fn(&mut Self, &mut Context<Self>) + Clone + 'static,
    ) -> AnyElement {
        let t = theme(cx);
        let label = label.to_string();
        let (bg, hover, fg) = if !enabled {
            (
                t.secondary_btn_bg(),
                t.secondary_btn_bg(),
                t.text_placeholder(),
            )
        } else if primary {
            (t.primary(), t.primary_hover(), t.text_on_primary_btn())
        } else {
            (
                t.secondary_btn_bg(),
                t.secondary_btn_hover(),
                t.text_on_secondary_btn(),
            )
        };
        let mut button = div()
            .id(id.to_string())
            .h(px(26.))
            .px_3()
            .flex()
            .items_center()
            .justify_center()
            .rounded_md()
            .bg(bg)
            .text_size(px(11.))
            .text_color(fg);
        if enabled {
            button = button
                .cursor_pointer()
                .hover(move |style| style.bg(hover))
                .on_click(cx.listener(move |this, _ev: &gpui::ClickEvent, _w, cx| {
                    on_click(this, cx);
                }));
        }
        button.child(label).into_any_element()
    }

    // -----------------------------------------------------------------------
    // 规则编辑弹窗
    // -----------------------------------------------------------------------

    fn render_editor(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let t = theme(cx);
        let Some(editor) = self.editor.as_ref() else {
            return div().into_any_element();
        };
        let is_new = editor.is_new();
        let kind = editor.kind;
        let auto_start = editor.auto_start;
        let error = editor.error.clone().unwrap_or_default();
        let target_hidden = !kind.needs_target();

        let mut card = div()
            .w(px(440.))
            .bg(t.card_bg())
            .border_1()
            .border_color(t.border_medium())
            .rounded_lg()
            .flex()
            .flex_col()
            .child(
                div()
                    .w_full()
                    .h(px(42.))
                    .px_4()
                    .flex()
                    .items_center()
                    .bg(t.card_header_bg())
                    .border_b_1()
                    .border_color(t.border_subtle())
                    .text_size(px(13.))
                    .text_color(t.text_on_card_header_bg())
                    .child(if is_new {
                        "新增转发规则"
                    } else {
                        "编辑转发规则"
                    }),
            )
            .child(
                div()
                    .w_full()
                    .p_4()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(
                        div()
                            .w_full()
                            .flex()
                            .gap_1()
                            .child(self.kind_radio("本地 -L", ForwardKind::Local, kind, cx))
                            .child(self.kind_radio("远程 -R", ForwardKind::Remote, kind, cx))
                            .child(self.kind_radio("动态 -D", ForwardKind::Dynamic, kind, cx)),
                    )
                    .child(self.editor_field("监听地址", editor.bind_host.clone(), cx))
                    .child(self.editor_field("监听端口", editor.bind_port.clone(), cx)),
            );

        if !target_hidden {
            card = card.child(
                div()
                    .w_full()
                    .px_4()
                    .pb_2()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(self.editor_field("目标地址", editor.target_host.clone(), cx))
                    .child(self.editor_field("目标端口", editor.target_port.clone(), cx)),
            );
        }

        card = card.child(
            div()
                .w_full()
                .px_4()
                .flex()
                .items_center()
                .gap_2()
                .cursor_pointer()
                .id("pf-auto-toggle")
                .on_click(
                    cx.listener(|this: &mut Self, _ev: &gpui::ClickEvent, _w, cx| {
                        if let Some(editor) = this.editor.as_mut() {
                            editor.auto_start = !editor.auto_start;
                        }
                        cx.notify();
                    }),
                )
                .child(
                    div()
                        .w(px(14.))
                        .h(px(14.))
                        .rounded(px(3.))
                        .border_1()
                        .border_color(t.border_input_idle())
                        .bg(if auto_start {
                            t.primary()
                        } else {
                            t.input_bg()
                        }),
                )
                .child(
                    div()
                        .text_size(px(11.))
                        .text_color(t.text_on_card_bg())
                        .child("打开端口转发 Tab 时自动启动"),
                ),
        );

        if !error.is_empty() {
            card = card.child(
                div()
                    .w_full()
                    .px_4()
                    .pt_2()
                    .text_size(px(10.))
                    .text_color(t.danger())
                    .child(error),
            );
        }

        card = card.child(
            div()
                .w_full()
                .h(px(50.))
                .px_4()
                .flex()
                .items_center()
                .justify_center()
                .gap_2()
                .child(self.action_button(
                    "pf-editor-cancel",
                    "取消",
                    false,
                    true,
                    cx,
                    |this, cx| {
                        this.editor = None;
                        cx.notify();
                    },
                ))
                .child(self.action_button(
                    "pf-editor-save",
                    "保存",
                    true,
                    true,
                    cx,
                    |this, cx| {
                        this.save_editor(cx);
                    },
                )),
        );

        div()
            .absolute()
            .inset_0()
            .bg(t.modal_overlay())
            .flex()
            .items_center()
            .justify_center()
            .on_mouse_down_out(cx.listener(
                |this: &mut Self, _ev: &gpui::MouseDownEvent, _w, cx| {
                    this.editor = None;
                    cx.notify();
                },
            ))
            .child(card)
            .into_any_element()
    }

    fn kind_radio(
        &self,
        label: &str,
        value: ForwardKind,
        selected: ForwardKind,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = theme(cx);
        let selected = value == selected;
        div()
            .id(format!("pf-kind-{label}"))
            .h(px(26.))
            .px_3()
            .flex()
            .items_center()
            .justify_center()
            .rounded_md()
            .bg(if selected {
                t.row_selected_bg()
            } else {
                t.secondary_btn_bg()
            })
            .text_size(px(11.))
            .text_color(if selected {
                t.text_on_row_selected_bg()
            } else {
                t.text_on_secondary_btn()
            })
            .cursor_pointer()
            .on_click(
                cx.listener(move |this: &mut Self, _ev: &gpui::ClickEvent, _w, cx| {
                    if let Some(editor) = this.editor.as_mut() {
                        editor.kind = value;
                    }
                    cx.notify();
                }),
            )
            .child(format!("{} {}", if selected { "●" } else { "○" }, label))
            .into_any_element()
    }

    fn editor_field(
        &self,
        label: &str,
        input: Entity<TextInput>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = theme(cx);
        div()
            .w_full()
            .flex()
            .flex_col()
            .gap_1()
            .child(
                div()
                    .text_size(px(10.))
                    .text_color(t.text_subtle())
                    .child(label.to_string()),
            )
            .child(input)
            .into_any_element()
    }

    fn render_loopback_confirm(&self, rule: PortForward, cx: &mut Context<Self>) -> AnyElement {
        let t = theme(cx);
        let rule_for_confirm = rule.clone();
        div()
            .absolute()
            .inset_0()
            .bg(t.modal_overlay())
            .flex()
            .items_center()
            .justify_center()
            .child(
                div()
                    .w(px(400.))
                    .p_4()
                    .bg(t.card_bg())
                    .border_1()
                    .border_color(t.border_medium())
                    .rounded_lg()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .child(
                        div()
                            .w_full()
                            .text_center()
                            .text_size(px(13.))
                            .text_color(t.text_on_card_bg())
                            .child(format!("确认绑定非回环地址 {}？", rule.bind_host)),
                    )
                    .child(
                        div()
                            .w_full()
                            .text_center()
                            .text_size(px(11.))
                            .text_color(t.text_subtle())
                            .child("该端口可能暴露到局域网，请确认网络环境安全"),
                    )
                    .child(
                        div()
                            .flex()
                            .justify_center()
                            .gap_2()
                            .child(self.action_button(
                                "pf-loopback-cancel",
                                "取消",
                                false,
                                true,
                                cx,
                                |this, cx| {
                                    this.pending_loopback_confirm = None;
                                    cx.notify();
                                },
                            ))
                            .child(self.action_button(
                                "pf-loopback-confirm",
                                "仍然绑定",
                                true,
                                true,
                                cx,
                                move |this, cx| {
                                    this.pending_loopback_confirm = None;
                                    this.commit_rule(rule_for_confirm.clone(), cx);
                                },
                            )),
                    ),
            )
            .into_any_element()
    }
}

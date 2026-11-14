//! SessionTreeView — 左侧会话树
//!
//! Sprint 3 真实实现。核心 EventBus 桥接：
//! - subscribe：`cx.subscribe(&state.event_bus_host, |this, _host, ev, cx| { ... })`
//! - emit：`cx.update_entity(&state.event_bus_host, |_host, cx| cx.emit(event))`
//! - 事件回调：`cx.listener(closure)` 把签名从 App 升级到 View Context

use gpui::*;
use gpui::{div, px, rgb, ClickEvent, IntoElement, Render, Window};

use crate::event::{AppEvent, EventBusHost};
use crate::state::AppState;

use std::collections::{BTreeMap, HashSet};
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Tree 节点
// ---------------------------------------------------------------------------

enum TreeNode {
    Folder { name: String, children: Vec<TreeNode> },
    Session { id: Uuid, name: String, host: String },
}

impl TreeNode {
    fn build(sessions: &[&rterm_session::Session]) -> Vec<TreeNode> {
        let mut tree: BTreeMap<String, Vec<&rterm_session::Session>> = BTreeMap::new();
        let mut root: Vec<&rterm_session::Session> = Vec::new();

        for s in sessions {
            match s.folder.as_deref() {
                Some(f) if !f.is_empty() => tree.entry(f.to_string()).or_default().push(s),
                _ => root.push(s),
            }
        }

        let mut nodes: Vec<TreeNode> = Vec::new();
        for (folder, kids) in tree {
            let children: Vec<TreeNode> = kids
                .iter()
                .map(|s| TreeNode::Session {
                    id: s.id,
                    name: s.name.clone(),
                    host: s.host.clone(),
                })
                .collect();
            nodes.push(TreeNode::Folder { name: folder, children });
        }
        for s in root {
            nodes.push(TreeNode::Session {
                id: s.id,
                name: s.name.clone(),
                host: s.host.clone(),
            });
        }
        nodes
    }
}

// ---------------------------------------------------------------------------
// SessionTreeView
// ---------------------------------------------------------------------------

pub struct SessionTreeView {
    expanded: HashSet<String>,
    selected: Option<Uuid>,
}

impl SessionTreeView {
    pub fn new() -> Self {
        Self {
            expanded: HashSet::new(),
            selected: None,
        }
    }

    /// emit 一个 EventBus 事件（在当前 Context 里通过 update_entity 间接触发）
    fn emit_event(&self, cx: &mut Context<Self>, ev: AppEvent) {
        let host = cx.global::<AppState>().event_bus_host.clone();
        cx.update_entity(&host, |_host, cx: &mut Context<EventBusHost>| {
            cx.emit(ev);
        });
    }
}

impl Render for SessionTreeView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // 订阅 EventBusHost Entity
        let host = cx.global::<AppState>().event_bus_host.clone();
        cx.subscribe(&host, |this: &mut Self, _host_entity, ev, cx| {
            match ev {
                AppEvent::SessionCreated { .. }
                | AppEvent::SessionUpdated { .. }
                | AppEvent::SessionDeleted { .. } => cx.notify(),
                AppEvent::SessionTreeSelectRequested { session_id } => {
                    this.selected = *session_id;
                    cx.notify();
                }
                _ => {}
            }
        }).detach();

        let state = cx.global::<AppState>();
        let sessions = state.session_store.list().to_vec();
        let sessions_refs: Vec<&rterm_session::Session> = sessions.iter().collect();
        let tree = TreeNode::build(&sessions_refs);

        let mut root = div()
            .w(px(240.))
            .size_full()
            .bg(rgb(0x1e1e2e))
            .flex()
            .flex_col();

        root = root.child(
            div()
                .w_full()
                .px_3()
                .py_2()
                .bg(rgb(0x181825))
                .text_color(rgb(0xa6adc8))
                .text_size(px(11.))
                .child(format!("会话 ({})", sessions.len())),
        );

        let mut list = div().w_full().flex_1();
        for node in &tree {
            list = list.child(self.render_node(node, 0, cx));
        }
        root = root.child(list);

        root
    }
}

impl SessionTreeView {
    fn render_node(
        &mut self,
        node: &TreeNode,
        indent: u32,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        match node {
            TreeNode::Folder { name, children } => {
                let is_expanded = self.expanded.contains(name);
                let name_clone = name.clone();

                let row = div()
                    .w_full()
                    .pl(px(12. * indent as f32))
                    .pr(px(8.))
                    .py_1()
                    .text_size(px(12.))
                    .text_color(rgb(0xa6adc8))
                    .id(format!("folder-{}", name))
                    .hover(|s| s.bg(rgb(0x313244)))
                    .on_click(cx.listener(move |this: &mut Self, _ev: &ClickEvent, _w, cx| {
                        let f = name_clone.clone();
                        if this.expanded.contains(&f) {
                            this.expanded.remove(&f);
                        } else {
                            this.expanded.insert(f);
                        }
                        cx.notify();
                    }))
                    .child(format!("{} 📁 {}", if is_expanded { "▼" } else { "▶" }, name));

                let mut container = row.into_element();
                if is_expanded {
                    for child in children {
                        container = container.child(self.render_node(child, indent + 1, cx));
                    }
                }
                container.into_any_element()
            }

            TreeNode::Session { id, name, host } => {
                let id_copy = *id;
                let selected = self.selected == Some(id_copy);
                let bg = if selected { rgb(0x313244) } else { rgb(0x1e1e2e) };

                let row = div()
                    .w_full()
                    .pl(px(12. * indent as f32 + 8.))
                    .pr(px(8.))
                    .py_1()
                    .bg(bg)
                    .text_size(px(12.))
                    .text_color(rgb(0xcdd6f4))
                    .id(format!("session-{}", id_copy))
                    .hover(|s| s.bg(rgb(0x313244)))
                    .on_click(cx.listener(move |this: &mut Self, _ev: &ClickEvent, _w, cx| {
                        let sid = id_copy;
                        this.selected = Some(sid);
                        cx.notify();
                        // emit 两个事件：选中状态 + 连接请求（先 log，S5 接）
                        this.emit_event(cx, AppEvent::SessionTreeSelectRequested { session_id: Some(sid) });
                        this.emit_event(cx, AppEvent::SessionConnectRequested { session_id: sid });
                        log::info!("SessionTree selected {}", sid);
                    }))
                    .child(format!("🖥️ {}", name))
                    .child(
                        div()
                            .text_size(px(10.))
                            .text_color(rgb(0x6c7086))
                            .child(format!("{}:22", host)),
                    );

                row.into_any_element()
            }
        }
    }
}

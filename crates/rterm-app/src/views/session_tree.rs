//! SessionTreeView — 左侧会话树
//!
//! 所有颜色通过 `crate::ui::theme(cx)` 取主题，不硬编码 rgb。
//! 所有文字居中（`.items_center()`）。

use gpui::*;
use gpui::{div, px, ClickEvent, IntoElement, MouseButton, MouseDownEvent, Render, Window};

use crate::event::{AppEvent, EventBusHost};
use crate::state::AppState;
use crate::ui::theme;

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

    fn emit_event(&self, cx: &mut Context<Self>, ev: AppEvent) {
        let host = cx.global::<AppState>().event_bus_host.clone();
        cx.update_entity(&host, |_host, cx: &mut Context<EventBusHost>| {
            cx.emit(ev);
        });
    }
}

impl Render for SessionTreeView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);

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
        let sessions = state.session_store.lock().unwrap().list().to_vec();
        let sessions_refs: Vec<&rterm_session::Session> = sessions.iter().collect();
        let tree = TreeNode::build(&sessions_refs);

        let mut root = div()
            .w(px(240.))
            .size_full()
            .bg(t.sidebar_bg())
            .flex()
            .flex_col();

        root = root.child(
            div()
                .w_full()
                .h(px(28.))
                .px_3()
                .bg(t.toolbar_bg())
                .border_b_1()
                .border_color(t.border_subtle())
                .flex()
                .items_center()
                .text_color(t.text_subtle())
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
        let t = theme(cx);

        match node {
            TreeNode::Folder { name, children } => {
                let is_expanded = self.expanded.contains(name);
                let name_clone = name.clone();

                let row = div()
                    .w_full()
                    .pl(px(12. * indent as f32))
                    .pr(px(8.))
                    .py_1()
                    .flex()
                    .items_center()
                    .text_size(px(12.))
                    .text_color(t.text_on_sidebar_bg())
                    .id(format!("folder-{}", name))
                    .hover(|s| s.bg(t.row_hover_bg()))
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(move |this: &mut Self, ev: &MouseDownEvent, _w, cx| {
                            this.emit_event(
                                cx,
                                AppEvent::SessionTreeOpenContextMenuAt {
                                    session_id: None,
                                    x: ev.position.x.to_f64() as i32,
                                    y: ev.position.y.to_f64() as i32,
                                },
                            );
                        }),
                    )
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
                let bg = if selected { t.row_selected_bg() } else { t.sidebar_bg() };

                let row = div()
                    .w_full()
                    .pl(px(12. * indent as f32 + 8.))
                    .pr(px(8.))
                    .py_1()
                    .bg(bg)
                    .flex()
                    .flex_col()
                    .text_size(px(12.))
                    .text_color(if selected {
                        t.text_on_row_selected_bg()
                    } else {
                        t.text_on_sidebar_bg()
                    })
                    .id(format!("session-{}", id_copy))
                    .hover(|s| s.bg(t.row_hover_bg()))
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(move |this: &mut Self, ev: &MouseDownEvent, _w, cx| {
                            this.selected = Some(id_copy);
                            cx.notify();
                            this.emit_event(
                                cx,
                                AppEvent::SessionTreeOpenContextMenuAt {
                                    session_id: Some(id_copy),
                                    x: ev.position.x.to_f64() as i32,
                                    y: ev.position.y.to_f64() as i32,
                                },
                            );
                        }),
                    )
                    .on_click(cx.listener(move |this: &mut Self, ev: &ClickEvent, _w, cx| {
                        let sid = id_copy;
                        let click_count = match ev {
                            ClickEvent::Mouse(m) => m.down.click_count,
                            ClickEvent::Touch(t) => t.tap_count,
                            ClickEvent::Keyboard(_) => 1,
                        };
                        this.selected = Some(sid);
                        cx.notify();
                        this.emit_event(cx, AppEvent::SessionTreeSelectRequested { session_id: Some(sid) });
                        // 双击才发起连接，单击仅选中，避免误触
                        if click_count >= 2 {
                            log::info!("SessionTree double-click connect {}", sid);
                            this.emit_event(cx, AppEvent::SessionConnectRequested { session_id: sid });
                        }
                    }))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .child(format!("🖥️ {}", name)),
                    )
                    .child(
                        div()
                            .text_size(px(10.))
                            .text_color(t.text_subtle())
                            .child(format!("{}:22", host)),
                    );

                row.into_any_element()
            }
        }
    }
}

//! SessionTreeView — 左侧会话树
//!
//! 所有颜色通过 `crate::ui::theme(cx)` 取主题，不硬编码 rgb。
//! 所有文字居中（`.items_center()`）。

use gpui::*;
use gpui::{div, px, ClickEvent, IntoElement, MouseButton, MouseDownEvent, Render, Window};

use crate::event::{AppEvent, EventBusHost};
use crate::state::AppState;
use crate::ui::theme;

use std::collections::{HashMap, HashSet};
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Tree 节点
// ---------------------------------------------------------------------------

enum TreeNode {
    Folder {
        /// 显示名（路径最后一段）
        name: String,
        /// 完整路径，作为 expanded 与节点 id 的 key
        path: String,
        children: Vec<TreeNode>,
    },
    Session {
        id: Uuid,
        name: String,
        host: String,
        port: u16,
    },
}

impl TreeNode {
    /// 按 `folder` 中的 `/` 构建多级文件夹树。
    fn build(sessions: &[&rterm_session::Session]) -> Vec<TreeNode> {
        let mut nodes: Vec<TreeNode> = Vec::new();
        for s in sessions {
            let folder = s.folder.as_deref().unwrap_or("").trim_matches('/');
            if folder.is_empty() {
                nodes.push(session_node(s));
            } else {
                insert_session(&mut nodes, folder, s);
            }
        }
        sort_nodes(&mut nodes);
        nodes
    }
}

fn session_node(s: &rterm_session::Session) -> TreeNode {
    TreeNode::Session {
        id: s.id,
        name: s.name.clone(),
        host: s.host.clone(),
        port: s.port,
    }
}

/// 按 `/` 分隔的多级路径逐层插入会话节点。
fn insert_session(nodes: &mut Vec<TreeNode>, folder: &str, session: &rterm_session::Session) {
    let mut path = String::new();
    let mut level = nodes;
    for segment in folder.split('/').filter(|seg| !seg.is_empty()) {
        if !path.is_empty() {
            path.push('/');
        }
        path.push_str(segment);

        let index = match level
            .iter()
            .position(|node| matches!(node, TreeNode::Folder { path: p, .. } if *p == path))
        {
            Some(i) => i,
            None => {
                level.push(TreeNode::Folder {
                    name: segment.to_string(),
                    path: path.clone(),
                    children: Vec::new(),
                });
                level.len() - 1
            }
        };
        level = match &mut level[index] {
            TreeNode::Folder { children, .. } => children,
            // 同一路径前缀只会是文件夹，这里防御性退出，不 panic。
            TreeNode::Session { .. } => return,
        };
    }
    level.push(session_node(session));
}

/// 文件夹按名称排序且在会话前；会话按名称排序。
fn sort_nodes(nodes: &mut [TreeNode]) {
    nodes.sort_by(|a, b| match (a, b) {
        (TreeNode::Folder { name: na, .. }, TreeNode::Folder { name: nb, .. }) => na.cmp(nb),
        (TreeNode::Folder { .. }, TreeNode::Session { .. }) => std::cmp::Ordering::Less,
        (TreeNode::Session { .. }, TreeNode::Folder { .. }) => std::cmp::Ordering::Greater,
        (TreeNode::Session { name: na, .. }, TreeNode::Session { name: nb, .. }) => na.cmp(nb),
    });
    for node in nodes.iter_mut() {
        if let TreeNode::Folder { children, .. } = node {
            sort_nodes(children);
        }
    }
}

// ---------------------------------------------------------------------------
// SessionTreeView
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ConnectionBadge {
    None,
    Connecting,
    Connected,
    Reconnecting,
    Failed,
}

pub struct SessionTreeView {
    expanded: HashSet<String>,
    selected: Option<Uuid>,
    connection: HashMap<Uuid, ConnectionBadge>,
}

impl SessionTreeView {
    pub fn new(cx: &mut Context<Self>) -> Self {
        // 只在构造时订阅一次，避免 render 每次重绘泄漏订阅
        let host = cx.global::<AppState>().event_bus_host.clone();
        cx.subscribe(&host, |this: &mut Self, _host_entity, ev, cx| {
            match ev {
                AppEvent::SessionCreated { .. }
                | AppEvent::SessionUpdated { .. }
                | AppEvent::SessionDeleted { .. }
                | AppEvent::SessionImported { .. } => cx.notify(),
                AppEvent::SessionTreeSelectRequested { session_id } => {
                    this.selected = *session_id;
                    cx.notify();
                }
                AppEvent::SessionConnectRequested { session_id } => {
                    this.connection
                        .insert(*session_id, ConnectionBadge::Connecting);
                    cx.notify();
                }
                AppEvent::SessionConnected { session_id } => {
                    this.connection.insert(*session_id, ConnectionBadge::Connected);
                    cx.notify();
                }
                AppEvent::SessionFailed { session_id, .. } => {
                    this.connection.insert(*session_id, ConnectionBadge::Failed);
                    cx.notify();
                }
                AppEvent::SessionDisconnected { session_id, .. } => {
                    this.connection.insert(*session_id, ConnectionBadge::None);
                    cx.notify();
                }
                AppEvent::SessionReconnecting { session_id, .. } => {
                    this.connection
                        .insert(*session_id, ConnectionBadge::Reconnecting);
                    cx.notify();
                }
                _ => {}
            }
        })
        .detach();

        Self {
            expanded: HashSet::new(),
            selected: None,
            connection: HashMap::new(),
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
            TreeNode::Folder {
                name,
                path,
                children,
            } => {
                let is_expanded = self.expanded.contains(path);
                let path_clone = path.clone();

                let row = div()
                    .w_full()
                    .pl(px(12. * indent as f32))
                    .pr(px(8.))
                    .py_1()
                    .flex()
                    .items_center()
                    .text_size(px(12.))
                    .text_color(t.text_on_sidebar_bg())
                    .id(format!("folder-{}", path))
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
                        let f = path_clone.clone();
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

            TreeNode::Session {
                id,
                name,
                host,
                port,
            } => {
                let id_copy = *id;
                let selected = self.selected == Some(id_copy);
                let bg = if selected { t.row_selected_bg() } else { t.sidebar_bg() };
                let badge = self
                    .connection
                    .get(&id_copy)
                    .copied()
                    .unwrap_or(ConnectionBadge::None);
                let (dot, dot_color) = match badge {
                    ConnectionBadge::Connected => ("●", t.success()),
                    ConnectionBadge::Connecting | ConnectionBadge::Reconnecting => {
                        ("●", t.primary())
                    }
                    ConnectionBadge::Failed => ("●", t.danger()),
                    ConnectionBadge::None => ("○", t.text_placeholder()),
                };

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
                            .gap_1()
                            .child(
                                div()
                                    .text_size(px(9.))
                                    .text_color(dot_color)
                                    .child(dot),
                            )
                            .child(format!("🖥️ {}", name)),
                    )
                    .child(
                        div()
                            .text_size(px(10.))
                            .text_color(t.text_subtle())
                            .child(format!("{}:{}", host, port)),
                    );

                row.into_any_element()
            }
        }
    }
}

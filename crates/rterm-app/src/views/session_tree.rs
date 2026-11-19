//! SessionTreeView — 左侧会话树
//!
//! S13：
//! - 树节点 = 显式分组（含空分组）∪ 会话 `folder` 派生路径
//! - 会话按 `icon` 渲染；未设置为默认图标
//! - 右键三态：空白 / 分组 / 会话，行内 `stop_propagation` 避免误弹空白菜单
//! - 选中态通过 `SessionTreeSelectionChanged` 广播给顶部菜单栏

use gpui::*;
use gpui::{div, px, ClickEvent, IntoElement, MouseButton, MouseDownEvent, Render, Window};

use crate::event::{AppEvent, EventBusHost};
use crate::state::{lock_or_recover, AppState};
use crate::ui::theme;

use std::collections::{HashMap, HashSet};
use uuid::Uuid;

// ---------------------------------------------------------------------------
// 布局常量
// ---------------------------------------------------------------------------

/// 单级缩进宽度（像素）。
const INDENT_STEP: f32 = 12.0;
/// 树行左侧基础内边距，保证根级节点不贴边。
const ROW_PAD_LEFT: f32 = 8.0;
/// 折叠箭头占位宽度：会话图标据此与文件夹图标对齐。
const CHEVRON_W: f32 = 10.0;

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
        icon: String,
        host: String,
        port: u16,
    },
}

impl TreeNode {
    /// 合并显式分组与会话派生分组后构建多级文件夹树。
    fn build(sessions: &[&rterm_session::Session], folders: &[String]) -> Vec<TreeNode> {
        let mut nodes: Vec<TreeNode> = Vec::new();
        for folder in folders {
            insert_folder_path(&mut nodes, folder);
        }
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
        icon: s.icon_or_default().to_string(),
        host: s.host.clone(),
        port: s.port,
    }
}

/// 插入显式分组路径（每一级都建节点，空分组也能显示）。
fn insert_folder_path(nodes: &mut Vec<TreeNode>, folder: &str) {
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
            TreeNode::Session { .. } => return,
        };
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

/// 收集树中所有分组路径（展开全部用）。
fn collect_folder_paths(nodes: &[TreeNode], out: &mut HashSet<String>) {
    for node in nodes {
        if let TreeNode::Folder { path, children, .. } = node {
            out.insert(path.clone());
            collect_folder_paths(children, out);
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

/// 拖拽会话时携带的负载：仅 id 与显示名（用于拖拽预览）。
struct SessionDrag {
    session_id: Uuid,
    label: String,
}

/// 拖拽时跟随鼠标的轻量预览。
struct SessionDragPreview {
    label: String,
    /// 抓取点相对会话行左上角的偏移。GPUI 把预览画在 `鼠标位置 - 偏移` 处，
    /// 这里用等量内边距抵消，让预览始终贴在鼠标右下角。
    grab_offset: Point<Pixels>,
}

impl Render for SessionDragPreview {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        div()
            .pl(self.grab_offset.x + px(14.))
            .pt(self.grab_offset.y + px(10.))
            .child(
                div()
                    .px_3()
                    .py_1()
                    .rounded_md()
                    .border_1()
                    .border_color(t.border_medium())
                    .bg(t.card_bg())
                    .text_size(px(11.))
                    .text_color(t.text_on_card_bg())
                    .child(self.label.clone()),
            )
    }
}

/// 统计分组子树内的会话数。
fn count_sessions(nodes: &[TreeNode]) -> usize {
    nodes
        .iter()
        .map(|node| match node {
            TreeNode::Session { .. } => 1,
            TreeNode::Folder { children, .. } => count_sessions(children),
        })
        .sum()
}

/// 展开 `path` 的所有父级（移动会话后让目标分组可见）。
fn expand_ancestors(expanded: &mut HashSet<String>, path: &str) {
    let mut acc = String::new();
    for segment in path.split('/').filter(|segment| !segment.is_empty()) {
        if !acc.is_empty() {
            acc.push('/');
        }
        acc.push_str(segment);
        expanded.insert(acc.clone());
    }
}

pub struct SessionTreeView {
    expanded: HashSet<String>,
    pub selected: Option<Uuid>,
    connection: HashMap<Uuid, ConnectionBadge>,
    /// 会话较多时的纵向滚动
    scroll: ScrollHandle,
}

impl SessionTreeView {
    pub fn new(cx: &mut Context<Self>) -> Self {
        // 只在构造时订阅一次，避免 render 每次重绘泄漏订阅
        let host = cx.global::<AppState>().event_bus_host.clone();
        cx.subscribe(&host, |this: &mut Self, _host_entity, ev, cx| match ev {
            AppEvent::SessionCreated { session_id } => {
                // 新建会话落位后自动展开其所在分组的每一级父路径
                let folder = {
                    let state = cx.global::<AppState>();
                    lock_or_recover(&state.session_store)
                        .get(*session_id)
                        .and_then(|session| session.folder.clone())
                };
                if let Some(folder) = folder {
                    expand_ancestors(&mut this.expanded, &folder);
                }
                cx.notify()
            }
            AppEvent::SessionUpdated { .. }
            | AppEvent::SessionImported { .. }
            | AppEvent::SessionTreeFoldersChanged => cx.notify(),
            AppEvent::SessionDeleted { session_id } => {
                if this.selected == Some(*session_id) {
                    this.selected = None;
                    this.emit_event(
                        cx,
                        AppEvent::SessionTreeSelectionChanged { session_id: None },
                    );
                }
                cx.notify()
            }
            AppEvent::SessionTreeSelectRequested { session_id } => {
                this.selected = *session_id;
                cx.notify();
            }
            AppEvent::SessionTreeExpandAll { expanded } => {
                if *expanded {
                    let state = cx.global::<AppState>();
                    let store = lock_or_recover(&state.session_store);
                    let sessions = store.list().to_vec();
                    let folders = store.list_folders();
                    drop(store);
                    let refs: Vec<&rterm_session::Session> = sessions.iter().collect();
                    let tree = TreeNode::build(&refs, &folders);
                    let mut paths = HashSet::new();
                    collect_folder_paths(&tree, &mut paths);
                    this.expanded = paths;
                } else {
                    this.expanded.clear();
                }
                cx.notify();
            }
            AppEvent::SessionConnectRequested { session_id } => {
                this.connection
                    .insert(*session_id, ConnectionBadge::Connecting);
                cx.notify();
            }
            AppEvent::SessionConnected { session_id } => {
                this.connection
                    .insert(*session_id, ConnectionBadge::Connected);
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
        })
        .detach();

        Self {
            expanded: HashSet::new(),
            selected: None,
            connection: HashMap::new(),
            scroll: ScrollHandle::new(),
        }
    }

    fn emit_event(&self, cx: &mut Context<Self>, ev: AppEvent) {
        let host = cx.global::<AppState>().event_bus_host.clone();
        cx.update_entity(&host, |_host, cx: &mut Context<EventBusHost>| {
            cx.emit(ev);
        });
    }

    fn select(&mut self, session_id: Option<Uuid>, cx: &mut Context<Self>) {
        if self.selected == session_id {
            return;
        }
        self.selected = session_id;
        self.emit_event(cx, AppEvent::SessionTreeSelectionChanged { session_id });
        cx.notify();
    }

    /// 空白区右键：发送 Blank 菜单事件。
    fn open_blank_menu(&self, ev: &MouseDownEvent, cx: &mut Context<Self>) {
        self.emit_event(
            cx,
            AppEvent::SessionTreeOpenContextMenuAt {
                session_id: None,
                folder: None,
                x: ev.position.x.to_f64() as i32,
                y: ev.position.y.to_f64() as i32,
            },
        );
    }
}

impl Render for SessionTreeView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);

        let (sessions, folders) = {
            let state = cx.global::<AppState>();
            let store = lock_or_recover(&state.session_store);
            (store.list().to_vec(), store.list_folders())
        };
        let sessions_refs: Vec<&rterm_session::Session> = sessions.iter().collect();
        let tree = TreeNode::build(&sessions_refs, &folders);

        let mut root = div()
            .w(px(super::SIDEBAR_W))
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

        // 会话列表：滚动容器 + 空白区右键 + 空状态引导
        let mut body = div()
            .w_full()
            .flex_1()
            .min_h(px(0.))
            .id("session-tree-blank")
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|this: &mut Self, ev: &MouseDownEvent, _w, cx| {
                    this.open_blank_menu(ev, cx);
                }),
            )
            .on_click(cx.listener(|this: &mut Self, _ev: &ClickEvent, _w, cx| {
                // 拖拽落位后的 mouse up 不算点击，避免丢失刚移动的会话选中态
                if cx.has_active_drag() {
                    return;
                }
                this.select(None, cx);
            }))
            // 空白区 = 根目录：把会话拖到这里即移出分组
            .on_drop(cx.listener(|this: &mut Self, drag: &SessionDrag, _w, cx| {
                this.drop_session(drag.session_id, None, cx);
            }));

        if tree.is_empty() {
            body = body.child(
                div()
                    .w_full()
                    .pt_6()
                    .px_3()
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap_1()
                    .text_size(px(11.))
                    .text_color(t.text_subtle())
                    .child("还没有会话")
                    .child(
                        div()
                            .text_size(px(10.))
                            .text_color(t.text_placeholder())
                            .child("点击上方「＋ 新建会话」开始"),
                    ),
            );
        } else {
            let mut rows = div().w_full().flex_shrink_0();
            for node in &tree {
                rows = rows.child(self.render_node(node, 0, cx));
            }
            body = body.child(rows);
        }
        root = root.child(body);
        root
    }
}

impl SessionTreeView {
    /// 拖拽落位：把会话移动到目标分组（`None` 表示根目录）。
    fn drop_session(&mut self, session_id: Uuid, folder: Option<String>, cx: &mut Context<Self>) {
        let folder = folder
            .map(|path| path.trim_matches('/').to_string())
            .filter(|path| !path.is_empty());
        if let Some(path) = folder.as_deref() {
            // 目标分组立即展开，移动后的会话才能被看到
            expand_ancestors(&mut self.expanded, path);
        }
        self.emit_event(cx, AppEvent::SessionMoveRequested { session_id, folder });
        cx.notify();
    }

    fn render_node(
        &mut self,
        node: &TreeNode,
        indent: u32,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let t = theme(cx);
        let depth = indent as f32;

        match node {
            TreeNode::Folder {
                name,
                path,
                children,
            } => {
                let is_expanded = self.expanded.contains(path);
                let drop_bg = t.primary().opacity(0.16);
                let menu_path = path.clone();
                let drop_path = path.clone();
                let toggle_path = path.clone();
                let session_count = count_sessions(children);

                // 只有 header 是交互区：hover / 右键 / 折叠 / drop 高亮都不会覆盖子树
                let mut header = div()
                    .id(format!("folder-{}", path))
                    .w_full()
                    .h(px(24.))
                    .pl(px(ROW_PAD_LEFT + INDENT_STEP * depth))
                    .pr(px(8.))
                    .rounded_sm()
                    .flex()
                    .items_center()
                    .gap_1()
                    .text_size(px(12.))
                    .text_color(t.text_on_sidebar_bg())
                    .hover(|s| s.bg(t.row_hover_bg()))
                    .drag_over::<SessionDrag>(move |style, _drag, _window, _cx| style.bg(drop_bg))
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(move |this: &mut Self, ev: &MouseDownEvent, _w, cx| {
                            cx.stop_propagation();
                            this.emit_event(
                                cx,
                                AppEvent::SessionTreeOpenContextMenuAt {
                                    session_id: None,
                                    folder: Some(menu_path.clone()),
                                    x: ev.position.x.to_f64() as i32,
                                    y: ev.position.y.to_f64() as i32,
                                },
                            );
                        }),
                    )
                    .on_click(
                        cx.listener(move |this: &mut Self, _ev: &ClickEvent, _w, cx| {
                            cx.stop_propagation();
                            if !this.expanded.remove(&toggle_path) {
                                this.expanded.insert(toggle_path.clone());
                            }
                            cx.notify();
                        }),
                    )
                    .child(
                        div()
                            .w(px(CHEVRON_W))
                            .flex()
                            .justify_center()
                            .text_size(px(9.))
                            .text_color(t.text_subtle())
                            .child(if is_expanded { "▾" } else { "▸" }),
                    )
                    .child(div().text_size(px(11.)).child("📁"))
                    .child(div().flex_1().min_w(px(0.)).truncate().child(name.clone()));
                if session_count > 0 {
                    header = header.child(
                        div()
                            .flex_shrink_0()
                            .text_size(px(10.))
                            .text_color(t.text_placeholder())
                            .child(session_count.to_string()),
                    );
                }

                // 子树容器：拖到分组内任意位置（含空分组提示）都落到该分组
                let mut column = div().w_full().flex().flex_col().on_drop(cx.listener(
                    move |this: &mut Self, drag: &SessionDrag, _w, cx| {
                        this.drop_session(drag.session_id, Some(drop_path.clone()), cx);
                    },
                ));
                column = column.child(header);
                if is_expanded {
                    if children.is_empty() {
                        column = column.child(
                            div()
                                .w_full()
                                .h(px(22.))
                                .pl(px(ROW_PAD_LEFT + INDENT_STEP * (depth + 1.)))
                                .flex()
                                .items_center()
                                .text_size(px(11.))
                                .text_color(t.text_placeholder())
                                .child("空分组"),
                        );
                    } else {
                        for child in children {
                            column = column.child(self.render_node(child, indent + 1, cx));
                        }
                    }
                }
                column.into_any_element()
            }

            TreeNode::Session {
                id,
                name,
                icon,
                host,
                port,
            } => {
                let id_copy = *id;
                let selected = self.selected == Some(id_copy);
                let bg = if selected {
                    t.row_selected_bg()
                } else {
                    t.sidebar_bg()
                };
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
                let icon = icon.clone();
                let name = name.clone();
                let drag = SessionDrag {
                    session_id: id_copy,
                    label: name.clone(),
                };

                let row = div()
                    .id(format!("session-{}", id_copy))
                    .w_full()
                    // 会话内容从箭头之后开始，图标与同级文件夹图标对齐
                    .pl(px(ROW_PAD_LEFT + INDENT_STEP * depth + CHEVRON_W + 4.))
                    .pr(px(8.))
                    .py_1()
                    .rounded_sm()
                    .bg(bg)
                    .flex()
                    .flex_col()
                    .text_size(px(12.))
                    .text_color(if selected {
                        t.text_on_row_selected_bg()
                    } else {
                        t.text_on_sidebar_bg()
                    })
                    .hover(|s| s.bg(t.row_hover_bg()))
                    .on_drag(drag, |drag, grab_offset, _window, cx| {
                        cx.new(|_| SessionDragPreview {
                            label: drag.label.clone(),
                            grab_offset,
                        })
                    })
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(move |this: &mut Self, ev: &MouseDownEvent, _w, cx| {
                            cx.stop_propagation();
                            this.select(Some(id_copy), cx);
                            this.emit_event(
                                cx,
                                AppEvent::SessionTreeOpenContextMenuAt {
                                    session_id: Some(id_copy),
                                    folder: None,
                                    x: ev.position.x.to_f64() as i32,
                                    y: ev.position.y.to_f64() as i32,
                                },
                            );
                        }),
                    )
                    .on_click(
                        cx.listener(move |this: &mut Self, ev: &ClickEvent, _w, cx| {
                            cx.stop_propagation();
                            let sid = id_copy;
                            let click_count = match ev {
                                ClickEvent::Mouse(m) => m.down.click_count,
                                ClickEvent::Touch(t) => t.tap_count,
                                ClickEvent::Keyboard(_) => 1,
                            };
                            this.select(Some(sid), cx);
                            this.emit_event(
                                cx,
                                AppEvent::SessionTreeSelectRequested {
                                    session_id: Some(sid),
                                },
                            );
                            // 双击才发起连接，单击仅选中，避免误触
                            if click_count >= 2 {
                                log::info!("SessionTree double-click connect {}", sid);
                                this.emit_event(
                                    cx,
                                    AppEvent::SessionConnectRequested { session_id: sid },
                                );
                            }
                        }),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_1()
                            .child(
                                div()
                                    .flex_shrink_0()
                                    .text_size(px(9.))
                                    .text_color(dot_color)
                                    .child(dot),
                            )
                            .child(div().flex_shrink_0().text_size(px(11.)).child(icon))
                            .child(div().flex_1().min_w(px(0.)).truncate().child(name)),
                    )
                    .child(
                        div()
                            .pl(px(13.))
                            .text_size(px(10.))
                            .text_color(t.text_subtle())
                            .truncate()
                            .child(format!("{}:{}", host, port)),
                    );

                row.into_any_element()
            }
        }
    }
}

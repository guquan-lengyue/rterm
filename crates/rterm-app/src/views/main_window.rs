//! MainWindow — 应用顶层三栏布局 + Modal overlay
//!
//! ```
//! ┌────────────┬───────────────────────────┐
//! │ SessionTree│  Toolbar (New)            │
//! │  (240px)   ├───────────────────────────┤
//! │            │     Content Area          │
//! │            │   (Terminal / SFTP / ...) │
//! ├────────────┴───────────────────────────┤
//! │            Status Bar                   │
//! └─────────────────────────────────────────┘
//! ```
//! Modal overlay（半透明 dim + 居中卡片）叠在 Content Area 上。

use gpui::prelude::*;
use gpui::{div, px, rgb, Entity, IntoElement, Render, Window};

use crate::event::AppEvent;
use crate::state::AppState;

use super::session_dialog::SessionEditDialog;
use super::session_tree::SessionTreeView;
use super::status_bar::StatusBar;

pub struct MainWindow {
    session_tree: Entity<SessionTreeView>,
    status_bar: Entity<StatusBar>,
    pub edit_dialog: Option<Entity<SessionEditDialog>>,
}

impl MainWindow {
    pub fn new(session_tree: Entity<SessionTreeView>, status_bar: Entity<StatusBar>) -> Self {
        Self {
            session_tree,
            status_bar,
            edit_dialog: None,
        }
    }

    fn open_edit_dialog(&mut self, session: Option<&rterm_session::Session>, cx: &mut Context<Self>) {
        let dialog = cx.new(|cx| SessionEditDialog::from_session(cx, session));
        self.edit_dialog = Some(dialog);
        cx.notify();
    }

    fn close_edit_dialog(&mut self, cx: &mut Context<Self>) {
        self.edit_dialog = None;
        cx.notify();
    }
}

impl Render for MainWindow {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // 订阅事件
        let host = cx.global::<AppState>().event_bus_host.clone();
        cx.subscribe(&host, |this: &mut Self, _host, ev, cx| {
            match ev {
                AppEvent::SessionTreeOpenEditDialog { session_id } => {
                    match session_id {
                        Some(sid) => {
                            let session = {
                                let state = cx.global::<AppState>();
                                state.session_store.get(*sid).cloned()
                            };
                            if let Some(s) = session {
                                this.open_edit_dialog(Some(&s), cx);
                            }
                        }
                        None => {
                            // None = 关闭 dialog（由 dialog.cancel() 发出）
                            this.close_edit_dialog(cx);
                        }
                    }
                }
                _ => {}
            }
        }).detach();

        // New 按钮 id（用来触发新建会话）
        let new_btn_id = "rterm-new-session-btn".to_string();

        let mut root = div()
            .size_full()
            .bg(rgb(0x1e1e1e))
            .flex()
            .flex_col()
            // Toolbar
            .child(
                div()
                    .w_full()
                    .h(px(36.))
                    .bg(rgb(0x181825))
                    .flex()
                    .items_center()
                    .px_3()
                    .border_b_1()
                    .border_color(rgb(0x313244))
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(rgb(0x9399b2))
                            .mr_4()
                            .child("🔌 RTerm"),
                    )
                    .child(
                        div()
                            .h(px(26.))
                            .px_3()
                            .rounded_md()
                            .bg(rgb(0x007acc))
                            .text_color(rgb(0xffffff))
                            .text_size(px(11.))
                            .cursor_pointer()
                            .id(new_btn_id)
                            .hover(|s| s.bg(rgb(0x2096e0)))
                            .on_click(cx.listener(move |this: &mut Self, _ev: &gpui::ClickEvent, _w, cx| {
                                this.open_edit_dialog(None, cx);
                            }))
                            .child("＋ 新建会话"),
                    ),
            )
            // 左 SessionTree + 右 Content Area
            .child(
                div()
                    .w_full()
                    .flex_1()
                    .flex()
                    .child(
                        div()
                            .w(px(240.))
                            .h_full()
                            .bg(rgb(0x181825))
                            .border_r_1()
                            .border_color(rgb(0x313244))
                            .child(self.session_tree.clone()),
                    )
                    .child(
                        div()
                            .flex_1()
                            .h_full()
                            .bg(rgb(0x1e1e1e))
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(
                                div()
                                    .text_size(px(13.))
                                    .text_color(rgb(0x9399b2))
                                    .child("← 选择一个会话开始连接，或点击「新建会话」创建新连接"),
                            ),
                    ),
            )
            // 底部 StatusBar
            .child(self.status_bar.clone());

        // 如果 dialog 打开，叠一层 Modal overlay
        if let Some(dialog) = &self.edit_dialog {
            root = root.child(dialog.clone());
        }

        root
    }
}

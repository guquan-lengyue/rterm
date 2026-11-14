//! MainWindow — 应用顶层三栏布局 + Modal overlay
//!
//! 所有颜色通过 `crate::ui::theme(cx)` 取主题，不硬编码 rgb。
//! 所有文字居中。
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

use gpui::prelude::*;
use gpui::{div, px, Entity, IntoElement, Render, Window};

use crate::event::AppEvent;
use crate::state::AppState;
use crate::ui::theme;

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
        let t = theme(cx);

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
                            this.close_edit_dialog(cx);
                        }
                    }
                }
                _ => {}
            }
        }).detach();

        let new_btn_id = "rterm-new-session-btn".to_string();

        let mut root = div()
            .size_full()
            .bg(t.app_bg())
            .flex()
            .flex_col()
            // Toolbar
            .child(
                div()
                    .w_full()
                    .h(px(36.))
                    .bg(t.toolbar_bg())
                    .flex()
                    .items_center()
                    .px_3()
                    .border_b_1()
                    .border_color(t.border_subtle())
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(t.text_on_toolbar_bg())
                            .mr_4()
                            .child("🔌 RTerm"),
                    )
                    .child(
                        div()
                            .h(px(26.))
                            .px_3()
                            .rounded_md()
                            .bg(t.primary())
                            .text_color(t.text_on_primary_btn())
                            .text_size(px(11.))
                            .cursor_pointer()
                            .id(new_btn_id)
                            .hover(|s| s.bg(t.primary_hover()))
                            .on_click(cx.listener(move |this: &mut Self, _ev: &gpui::ClickEvent, _w, cx| {
                                this.open_edit_dialog(None, cx);
                            }))
                            .flex()
                            .items_center()
                            .justify_center()
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
                            .bg(t.sidebar_bg())
                            .border_r_1()
                            .border_color(t.border_subtle())
                            .child(self.session_tree.clone()),
                    )
                    .child(
                        div()
                            .flex_1()
                            .h_full()
                            .bg(t.app_bg())
                            .flex()
                            .items_center()
                            .justify_center()
                            .flex_col()
                            .gap_2()
                            .child(
                                div()
                                    .text_size(px(13.))
                                    .text_color(t.text_subtle())
                                    .child("← 选择一个会话开始连接"),
                            )
                            .child(
                                div()
                                    .text_size(px(12.))
                                    .text_color(t.text_placeholder())
                                    .child("或点击工具栏「新建会话」创建新连接"),
                            ),
                    ),
            )
            // 底部 StatusBar
            .child(self.status_bar.clone());

        if let Some(dialog) = &self.edit_dialog {
            root = root.child(dialog.clone());
        }

        root
    }
}

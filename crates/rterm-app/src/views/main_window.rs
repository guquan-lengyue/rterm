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
use gpui::{
    div, px, ClickEvent, Entity, IntoElement, MouseDownEvent, Render, Window,
};
use uuid::Uuid;

use crate::event::AppEvent;
use crate::state::AppState;
use crate::ui::theme;

use super::session_dialog::SessionEditDialog;
use super::session_tree::SessionTreeView;
use super::status_bar::StatusBar;

// ---------------------------------------------------------------------------
// 右键菜单
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug)]
pub struct ContextMenuState {
    /// None = 空白处/文件夹右键（只提供「新建会话」）
    pub session_id: Option<Uuid>,
    pub x: i32,
    pub y: i32,
}

#[derive(Clone, Copy, Debug)]
enum ContextMenuAction {
    NewSession,
    EditSession(Uuid),
    DeleteSession(Uuid),
    ExportAll,
    ImportFile,
}

pub struct MainWindow {
    session_tree: Entity<SessionTreeView>,
    status_bar: Entity<StatusBar>,
    pub edit_dialog: Option<Entity<SessionEditDialog>>,
    context_menu: Option<ContextMenuState>,
}

impl MainWindow {
    pub fn new(session_tree: Entity<SessionTreeView>, status_bar: Entity<StatusBar>) -> Self {
        Self {
            session_tree,
            status_bar,
            edit_dialog: None,
            context_menu: None,
        }
    }

    fn open_edit_dialog(&mut self, session: Option<&rterm_session::Session>, cx: &mut Context<Self>) {
        let dialog = cx.new(|cx| SessionEditDialog::from_session(cx, session));
        self.edit_dialog = Some(dialog);
        self.context_menu = None;
        cx.notify();
    }

    fn close_edit_dialog(&mut self, cx: &mut Context<Self>) {
        self.edit_dialog = None;
        cx.notify();
    }

    fn edit_session(&mut self, session_id: Uuid, cx: &mut Context<Self>) {
        let session = {
            let state = cx.global::<AppState>();
            state.session_store.lock().unwrap().get(session_id).cloned()
        };
        if let Some(s) = session {
            self.open_edit_dialog(Some(&s), cx);
        }
    }

    fn delete_session(&mut self, session_id: Uuid, cx: &mut Context<Self>) {
        {
            let state = cx.global_mut::<AppState>();
            state.session_store.lock().unwrap().delete(session_id);
        }
        self.context_menu = None;
        let host = cx.global::<AppState>().event_bus_host.clone();
        cx.update_entity(&host, |_host, cx: &mut Context<crate::event::EventBusHost>| {
            cx.emit(AppEvent::SessionDeleted { session_id });
        });
        log::info!("Session deleted: {}", session_id);
        cx.notify();
    }

    fn set_status(&mut self, msg: impl Into<String>, cx: &mut Context<Self>) {
        let msg = msg.into();
        self.status_bar.update(cx, |bar, cx| {
            bar.message = msg;
            cx.notify();
        });
    }

    /// 导出全部会话到固定路径 `%APPDATA%\rterm\sessions-export.json`
    fn export_sessions(&mut self, cx: &mut Context<Self>) {
        let path = export_path();
        let sessions = {
            let state = cx.global::<AppState>();
            state.session_store.lock().unwrap().list().to_vec()
        };

        let result: Result<(), String> = (|| {
            let json = rterm_session::export::export_json(
                &sessions,
                rterm_session::ExportScope::All,
                None,
            )
            .map_err(|e| format!("{e:#}"))?;
            if let Some(parent) = path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            std::fs::write(&path, json).map_err(|e| format!("写入失败: {e}"))?;
            Ok(())
        })();

        self.context_menu = None;
        match result {
            Ok(()) => {
                log::info!("Exported {} sessions to {}", sessions.len(), path.display());
                self.set_status(
                    format!("已导出 {} 个会话 → {}", sessions.len(), path.display()),
                    cx,
                );
            }
            Err(e) => {
                log::error!("导出失败: {e}");
                self.set_status(format!("导出失败: {e}"), cx);
            }
        }
        cx.notify();
    }

    /// 从固定路径导入会话（冲突策略：跳过已存在）
    fn import_sessions(&mut self, cx: &mut Context<Self>) {
        let path = export_path();
        let result: Result<(usize, usize, usize), String> = (|| {
            let json = std::fs::read_to_string(&path).map_err(|e| format!("读取失败: {e}"))?;
            let existing = {
                let state = cx.global::<AppState>();
                state.session_store.lock().unwrap().list().to_vec()
            };
            let (merged, report) = rterm_session::export::import_json(
                &json,
                &existing,
                rterm_session::ConflictPolicy::Skip,
                None,
            )
            .map_err(|e| format!("{e:#}"))?;
            {
                let state = cx.global_mut::<AppState>();
                let mut store = state.session_store.lock().unwrap();
                store.clear();
                for s in merged {
                    store.upsert(s);
                }
            }
            Ok((report.added, report.updated, report.skipped))
        })();

        self.context_menu = None;
        match result {
            Ok((added, updated, skipped)) => {
                let host = cx.global::<AppState>().event_bus_host.clone();
                cx.update_entity(&host, |_host, cx: &mut Context<crate::event::EventBusHost>| {
                    cx.emit(AppEvent::SessionImported { added, updated, skipped });
                });
                log::info!("Imported: added={} updated={} skipped={}", added, updated, skipped);
                self.set_status(
                    format!("导入完成：新增 {added}，更新 {updated}，跳过 {skipped}"),
                    cx,
                );
            }
            Err(e) => {
                log::error!("导入失败: {e}");
                self.set_status(format!("导入失败: {e}"), cx);
            }
        }
        cx.notify();
    }
}

/// 导出/导入固定路径：`%APPDATA%\rterm\sessions-export.json`
fn export_path() -> std::path::PathBuf {
    let mut p = dirs::data_dir().unwrap_or_else(|| std::path::PathBuf::from("."));
    p.push("rterm");
    p.push("sessions-export.json");
    p
}

impl Render for MainWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);

        let host = cx.global::<AppState>().event_bus_host.clone();
        cx.subscribe(&host, |this: &mut Self, _host, ev, cx| {
            match ev {
                AppEvent::SessionTreeOpenEditDialog { session_id } => {
                    match session_id {
                        Some(sid) => {
                            this.edit_session(*sid, cx);
                        }
                        None => {
                            this.close_edit_dialog(cx);
                        }
                    }
                }
                AppEvent::SessionTreeOpenContextMenuAt { session_id, x, y } => {
                    this.context_menu = Some(ContextMenuState {
                        session_id: *session_id,
                        x: *x,
                        y: *y,
                    });
                    cx.notify();
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

        if let Some(menu_state) = self.context_menu {
            let viewport = window.viewport_size();
            let menu_w = 170.0_f32;
            let menu_h = if menu_state.session_id.is_some() { 158.0_f32 } else { 98.0_f32 };
            let x = (menu_state.x as f32).min(f32::from(viewport.width) - menu_w).max(0.);
            let y = (menu_state.y as f32).min(f32::from(viewport.height) - menu_h).max(0.);

            let mut menu = div()
                .absolute()
                .left(px(x))
                .top(px(y))
                .w(px(menu_w))
                .py_1()
                .bg(t.card_bg())
                .border_1()
                .border_color(t.border_medium())
                .rounded_md()
                .flex()
                .flex_col()
                .id("rterm-context-menu")
                .on_mouse_down_out(cx.listener(
                    |this: &mut Self, _ev: &MouseDownEvent, _w, cx| {
                        this.context_menu = None;
                        cx.notify();
                    },
                ))
                .child(self.render_menu_item(
                    "rterm-ctx-new",
                    "＋ 新建会话",
                    false,
                    ContextMenuAction::NewSession,
                    cx,
                ));

            if let Some(sid) = menu_state.session_id {
                menu = menu
                    .child(self.render_menu_item(
                        "rterm-ctx-edit",
                        "✏ 编辑会话",
                        false,
                        ContextMenuAction::EditSession(sid),
                        cx,
                    ))
                    .child(self.render_menu_item(
                        "rterm-ctx-delete",
                        "🗑 删除会话",
                        true,
                        ContextMenuAction::DeleteSession(sid),
                        cx,
                    ));
            }

            menu = menu
                .child(self.render_menu_item(
                    "rterm-ctx-export",
                    "⬇ 导出会话 JSON",
                    false,
                    ContextMenuAction::ExportAll,
                    cx,
                ))
                .child(self.render_menu_item(
                    "rterm-ctx-import",
                    "⬆ 导入会话 JSON",
                    false,
                    ContextMenuAction::ImportFile,
                    cx,
                ));

            root = root.child(menu);
        }

        if let Some(dialog) = &self.edit_dialog {
            root = root.child(dialog.clone());
        }

        root
    }
}

impl MainWindow {
    fn render_menu_item(
        &self,
        id: &'static str,
        label: &str,
        danger: bool,
        action: ContextMenuAction,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let t = theme(cx);
        let label = label.to_string();
        let color = if danger { t.danger() } else { t.text_on_card_bg() };
        let hover_bg = t.row_hover_bg();

        div()
            .w_full()
            .h(px(30.))
            .px_3()
            .flex()
            .items_center()
            .justify_center()
            .text_size(px(12.))
            .text_color(color)
            .cursor_pointer()
            .id(id)
            .hover(move |s| s.bg(hover_bg))
            .on_click(cx.listener(
                move |this: &mut Self, _ev: &ClickEvent, _w, cx| {
                    this.context_menu = None;
                    match action {
                        ContextMenuAction::NewSession => this.open_edit_dialog(None, cx),
                        ContextMenuAction::EditSession(sid) => this.edit_session(sid, cx),
                        ContextMenuAction::DeleteSession(sid) => this.delete_session(sid, cx),
                        ContextMenuAction::ExportAll => this.export_sessions(cx),
                        ContextMenuAction::ImportFile => this.import_sessions(cx),
                    }
                    cx.notify();
                },
            ))
            .child(label)
            .into_any_element()
    }
}

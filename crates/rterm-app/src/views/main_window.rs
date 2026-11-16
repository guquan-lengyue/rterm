//! MainWindow — 顶层布局 + Tab 管理 + Modal overlay
//!
//! Sprint 6：
//! - 右侧内容区 = TabBar + 当前 Tab 内容
//! - SessionConnectRequested → 新建 Terminal Tab（每个 Tab 独立 SSH/PTY）
//! - Ctrl+T 新标签（同活动会话）/ Ctrl+Tab 切换 / Ctrl+W 关闭
//!
//! ```
//! ┌────────────┬───────────────────────────┐
//! │ SessionTree│  Toolbar (New)            │
//! │  (240px)   ├───────────────────────────┤
//! │            │  TabBar (30px)            │
//! │            ├───────────────────────────┤
//! │            │  Active Tab               │
//! ├────────────┴───────────────────────────┤
//! │            Status Bar                   │
//! └─────────────────────────────────────────┘
//! ```

use gpui::prelude::*;
use gpui::{
    actions, div, px, AnyElement, ClickEvent, Context, Entity, FocusHandle, IntoElement, KeyBinding,
    MouseDownEvent, Render, Window,
};
use uuid::Uuid;

use crate::event::{AppEvent, EventBusHost};
use crate::state::{AppState, TabInfo, TabKind};
use crate::ui::theme;

use super::import_export::{ImportExportDialog, IoMode};
use super::session_dialog::SessionEditDialog;
use super::session_tree::SessionTreeView;
use super::sftp::SftpView;
use super::settings::SettingsView;
use super::status_bar::StatusBar;
use super::tab_bar::TabBar;
use super::terminal::TerminalView;
use super::{SIDEBAR_W, TOOLBAR_H};

// ---------------------------------------------------------------------------
// 标签页快捷键（main.rs 里 App::on_action 转发到 MainWindow）
// ---------------------------------------------------------------------------

actions!(rterm_tabs, [NewTab, NextTab, CloseTab]);

pub fn key_bindings() -> [KeyBinding; 3] {
    [
        KeyBinding::new("ctrl-t", NewTab, None),
        KeyBinding::new("ctrl-tab", NextTab, None),
        KeyBinding::new("ctrl-w", CloseTab, None),
    ]
}

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
    OpenTerminal(Uuid),
    OpenSftp(Uuid),
    DeleteSession(Uuid),
    OpenExportDialog,
    OpenImportDialog,
    OpenSettingsDialog,
}

// ---------------------------------------------------------------------------
// Tab
// ---------------------------------------------------------------------------

/// 标签页内容（S7 起支持多类型）。
#[derive(Clone)]
pub enum TabContentView {
    Terminal(Entity<TerminalView>),
    Sftp(Entity<SftpView>),
}

impl TabContentView {
    fn focus_handle(&self, cx: &mut Context<MainWindow>) -> FocusHandle {
        match self {
            Self::Terminal(view) => view.read(cx).focus_handle(),
            Self::Sftp(view) => view.read(cx).focus_handle(),
        }
    }

    fn close(&self, cx: &mut Context<MainWindow>) {
        match self {
            Self::Terminal(view) => {
                view.update(cx, |view, cx| view.disconnect(cx));
            }
            Self::Sftp(view) => {
                view.update(cx, |view, cx| view.shutdown(cx));
            }
        }
    }

    /// Tab 内部是否有需要独占键盘的弹窗（S7.5.3.9：屏蔽全局快捷键）。
    fn blocks_global_shortcuts(&self, cx: &mut Context<MainWindow>) -> bool {
        match self {
            Self::Terminal(_) => false,
            Self::Sftp(view) => view.read(cx).has_modal_dialog(),
        }
    }

    fn into_element(&self) -> AnyElement {
        match self {
            Self::Terminal(view) => view.clone().into_any_element(),
            Self::Sftp(view) => view.clone().into_any_element(),
        }
    }
}

pub struct TabEntry {
    pub id: Uuid,
    pub session_id: Uuid,
    pub title: String,
    pub kind: TabKind,
    pub content: TabContentView,
}

pub struct MainWindow {
    session_tree: Entity<SessionTreeView>,
    status_bar: Entity<StatusBar>,
    tab_bar: Entity<TabBar>,
    tabs: Vec<TabEntry>,
    active_tab: usize,
    /// 切换 / 新建 Tab 后，下一帧把焦点交给活动终端
    focus_pending: bool,
    pub edit_dialog: Option<Entity<SessionEditDialog>>,
    settings_dialog: Option<Entity<SettingsView>>,
    import_export_dialog: Option<Entity<ImportExportDialog>>,
    context_menu: Option<ContextMenuState>,
    /// 等待确认删除的会话（S7.5.3.10 二次确认）。
    pending_delete: Option<Uuid>,
    /// 启动期加载会话的错误提示；点击横幅 ✕ 后清空。
    startup_error: Option<String>,
}

impl MainWindow {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let session_tree = cx.new(|cx| SessionTreeView::new(cx));
        let status_bar = cx.new(|cx| StatusBar::new(cx));
        let tab_bar = cx.new(|_cx| TabBar::new());

        let host = cx.global::<AppState>().event_bus_host.clone();
        let startup_error = cx.global::<AppState>().startup_error.clone();
        cx.subscribe(&host, |this: &mut Self, _host, event: &AppEvent, cx| {
            match event {
                AppEvent::SessionConnectRequested { session_id } => {
                    this.open_terminal_tab(*session_id, cx);
                }
                AppEvent::TabSelectRequested { tab_id } => {
                    this.activate_tab(*tab_id, cx);
                }
                AppEvent::TabCloseRequested { tab_id } => {
                    this.close_tab(*tab_id, cx);
                }
                AppEvent::TabCloseOthersRequested { tab_id } => {
                    this.close_other_tabs(*tab_id, cx);
                }
                AppEvent::TabCloseRightRequested { tab_id } => {
                    this.close_tabs_right_of(*tab_id, cx);
                }
                AppEvent::SessionTreeOpenEditDialog { session_id } => match session_id {
                    Some(sid) => this.edit_session(*sid, cx),
                    None => this.close_edit_dialog(cx),
                },
                AppEvent::SessionTreeOpenContextMenuAt { session_id, x, y } => {
                    this.context_menu = Some(ContextMenuState {
                        session_id: *session_id,
                        x: *x,
                        y: *y,
                    });
                    cx.notify();
                }
                AppEvent::SettingsCloseRequested { saved } => {
                    this.close_settings(*saved, cx);
                }
                AppEvent::SettingsOpenImportExport { export } => {
                    this.open_import_export(*export, cx);
                }
                AppEvent::ImportExportCloseRequested => {
                    this.close_import_export(cx);
                }
                _ => {}
            }
        })
        .detach();

        Self {
            session_tree,
            status_bar,
            tab_bar,
            tabs: Vec::new(),
            active_tab: 0,
            focus_pending: false,
            edit_dialog: None,
            settings_dialog: None,
            import_export_dialog: None,
            context_menu: None,
            pending_delete: None,
            startup_error,
        }
    }

    // -----------------------------------------------------------------------
    // Tab 生命周期
    // -----------------------------------------------------------------------

    fn open_terminal_tab(&mut self, session_id: Uuid, cx: &mut Context<Self>) {
        let title = {
            let state = cx.global::<AppState>();
            let store = state.session_store.lock().unwrap();
            store
                .get(session_id)
                .map(|session| session.display_label())
                .unwrap_or_else(|| "终端".to_string())
        };

        let view = cx.new(|cx| TerminalView::new(cx));
        view.update(cx, |view, cx| view.connect(session_id, cx));
        let id = view.read(cx).tab_id();

        self.tabs.push(TabEntry {
            id,
            session_id,
            title,
            kind: TabKind::Terminal,
            content: TabContentView::Terminal(view),
        });
        self.active_tab = self.tabs.len() - 1;
        self.focus_pending = true;
        self.sync_tab_bar(cx);
        cx.notify();
    }

    fn open_sftp_tab(&mut self, session_id: Uuid, cx: &mut Context<Self>) {
        let title = {
            let state = cx.global::<AppState>();
            let store = state.session_store.lock().unwrap();
            store
                .get(session_id)
                .map(|session| format!("{} · SFTP", session.display_label()))
                .unwrap_or_else(|| "SFTP".to_string())
        };

        let view = cx.new(|cx| SftpView::new(cx));
        view.update(cx, |view, cx| view.connect(session_id, cx));
        let id = view.read(cx).tab_id();

        self.tabs.push(TabEntry {
            id,
            session_id,
            title,
            kind: TabKind::Sftp,
            content: TabContentView::Sftp(view),
        });
        self.active_tab = self.tabs.len() - 1;
        self.focus_pending = true;
        self.sync_tab_bar(cx);
        cx.notify();
    }

    fn activate_tab(&mut self, tab_id: Uuid, cx: &mut Context<Self>) {
        if let Some(index) = self.tabs.iter().position(|tab| tab.id == tab_id) {
            self.active_tab = index;
            self.focus_pending = true;
            self.sync_tab_bar(cx);
            cx.notify();
        }
    }

    fn close_tab(&mut self, tab_id: Uuid, cx: &mut Context<Self>) {
        let Some(index) = self.tabs.iter().position(|tab| tab.id == tab_id) else {
            return;
        };
        let entry = self.tabs.remove(index);
        let session_id = entry.session_id;
        entry.content.close(cx);

        if index < self.active_tab {
            self.active_tab -= 1;
        }
        if self.active_tab >= self.tabs.len() {
            self.active_tab = self.tabs.len().saturating_sub(1);
        }
        self.focus_pending = true;
        self.sync_tab_bar(cx);
        self.reset_session_badge_if_unused([session_id], cx);
        cx.notify();
        log::info!("[tabs] closed tab {tab_id} ({} remaining)", self.tabs.len());
    }

    fn close_other_tabs(&mut self, tab_id: Uuid, cx: &mut Context<Self>) {
        let mut removed = Vec::new();
        let mut kept = Vec::new();
        for tab in self.tabs.drain(..) {
            if tab.id == tab_id {
                kept.push(tab);
            } else {
                removed.push(tab);
            }
        }
        self.tabs = kept;
        self.active_tab = 0;
        let removed_sessions: Vec<Uuid> = removed.iter().map(|tab| tab.session_id).collect();
        for entry in removed {
            entry.content.close(cx);
        }
        self.focus_pending = true;
        self.sync_tab_bar(cx);
        self.reset_session_badge_if_unused(removed_sessions, cx);
        cx.notify();
    }

    fn close_tabs_right_of(&mut self, tab_id: Uuid, cx: &mut Context<Self>) {
        let Some(index) = self.tabs.iter().position(|tab| tab.id == tab_id) else {
            return;
        };
        let removed: Vec<TabEntry> = self.tabs.drain(index + 1..).collect();
        let removed_sessions: Vec<Uuid> = removed.iter().map(|tab| tab.session_id).collect();
        for entry in removed {
            entry.content.close(cx);
        }
        if self.active_tab > index {
            self.active_tab = index;
        }
        self.focus_pending = true;
        self.sync_tab_bar(cx);
        self.reset_session_badge_if_unused(removed_sessions, cx);
        cx.notify();
    }

    /// 关闭 Tab 后，若某会话已没有任何 Tab，通知会话树把状态点复位。
    ///
    /// 由 MainWindow 统一判断（而不是在 TerminalView::disconnect 里发），
    /// 因为同一会话可能还开着别的 Tab。
    fn reset_session_badge_if_unused(
        &self,
        session_ids: impl IntoIterator<Item = Uuid>,
        cx: &mut Context<Self>,
    ) {
        let host = cx.global::<AppState>().event_bus_host.clone();
        for session_id in session_ids {
            if self.tabs.iter().any(|tab| tab.session_id == session_id) {
                continue;
            }
            cx.update_entity(&host, |_host, cx: &mut Context<EventBusHost>| {
                cx.emit(AppEvent::SessionDisconnected {
                    session_id,
                    reason: "标签已关闭".to_string(),
                });
            });
            log::info!("[tabs] session {session_id} has no tabs left, badge reset");
        }
    }

    fn sync_tab_bar(&mut self, cx: &mut Context<Self>) {
        let tabs: Vec<TabInfo> = self
            .tabs
            .iter()
            .map(|tab| TabInfo {
                id: tab.id,
                title: tab.title.clone(),
                kind: tab.kind.clone(),
            })
            .collect();
        let active = self.active_tab;
        self.tab_bar.update(cx, |bar, cx| bar.set_tabs(tabs, active, cx));
    }

    /// 是否有任何弹窗 / 确认卡片打开（弹窗期间屏蔽全局 Tab 快捷键）。
    fn dialogs_open(&self, cx: &mut Context<Self>) -> bool {
        self.settings_dialog.is_some()
            || self.edit_dialog.is_some()
            || self.import_export_dialog.is_some()
            || self.pending_delete.is_some()
            || self
                .tabs
                .get(self.active_tab)
                .map(|tab| tab.content.blocks_global_shortcuts(cx))
                .unwrap_or(false)
    }

    // -----------------------------------------------------------------------
    // 快捷键入口（main.rs 的 App::on_action 调用）
    // -----------------------------------------------------------------------

    pub fn new_tab_shortcut(&mut self, cx: &mut Context<Self>) {
        if self.dialogs_open(cx) {
            return;
        }
        if let Some(tab) = self.tabs.get(self.active_tab) {
            let session_id = tab.session_id;
            self.open_terminal_tab(session_id, cx);
        } else {
            self.set_status("没有活动会话：先双击左侧会话打开终端", cx);
        }
    }

    pub fn next_tab_shortcut(&mut self, cx: &mut Context<Self>) {
        if self.dialogs_open(cx) || self.tabs.is_empty() {
            return;
        }
        self.active_tab = (self.active_tab + 1) % self.tabs.len();
        self.focus_pending = true;
        self.sync_tab_bar(cx);
        cx.notify();
    }

    pub fn close_tab_shortcut(&mut self, cx: &mut Context<Self>) {
        if self.dialogs_open(cx) {
            return;
        }
        if let Some(tab) = self.tabs.get(self.active_tab) {
            let tab_id = tab.id;
            self.close_tab(tab_id, cx);
        }
    }

    // -----------------------------------------------------------------------
    // Modal / 会话操作
    // -----------------------------------------------------------------------

    pub fn open_settings(&mut self, cx: &mut Context<Self>) {
        if self.settings_dialog.is_some() {
            return;
        }
        let dialog = cx.new(SettingsView::new);
        self.settings_dialog = Some(dialog);
        self.context_menu = None;
        cx.notify();
    }

    fn close_settings(&mut self, saved: bool, cx: &mut Context<Self>) {
        self.settings_dialog = None;
        self.set_status(if saved { "设置已保存" } else { "设置已取消" }, cx);
        cx.notify();
    }

    fn open_import_export(&mut self, export: bool, cx: &mut Context<Self>) {
        let mode = if export { IoMode::Export } else { IoMode::Import };
        let dialog = cx.new(|cx| ImportExportDialog::new(cx, mode));
        self.import_export_dialog = Some(dialog);
        self.context_menu = None;
        cx.notify();
    }

    fn close_import_export(&mut self, cx: &mut Context<Self>) {
        self.import_export_dialog = None;
        cx.notify();
    }

    fn open_edit_dialog(
        &mut self,
        session: Option<&rterm_session::Session>,
        cx: &mut Context<Self>,
    ) {
        let dialog = cx.new(|cx| SessionEditDialog::from_session(cx, session));
        self.edit_dialog = Some(dialog);
        self.context_menu = None;
        cx.notify();
    }

    fn close_edit_dialog(&mut self, cx: &mut Context<Self>) {
        self.edit_dialog = None;
        cx.notify();
    }

    fn request_terminal(&mut self, session_id: Uuid, cx: &mut Context<Self>) {
        self.open_terminal_tab(session_id, cx);
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

        // 删除后立即写盘，失败只记日志（不阻断 UI）。
        let session_store = cx.global::<AppState>().session_store.clone();
        if let Err(e) = crate::state::persist_sessions(&session_store) {
            log::error!("删除会话后保存失败: {e:#}");
        }

        self.context_menu = None;
        let host = cx.global::<AppState>().event_bus_host.clone();
        cx.update_entity(&host, |_host, cx: &mut Context<EventBusHost>| {
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
}

// ---------------------------------------------------------------------------
// Render
// ---------------------------------------------------------------------------

impl Render for MainWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        let new_btn_id = "rterm-new-session-btn".to_string();

        // 切换 / 新建 / 关闭 Tab 后，把键盘焦点交给活动终端
        if self.focus_pending {
            let handle = self
                .tabs
                .get(self.active_tab)
                .map(|tab| tab.content.focus_handle(cx));
            if let Some(handle) = handle {
                window.focus(&handle, cx);
            }
            self.focus_pending = false;
        }

        let settings_hover = t.secondary_btn_hover();
        let mut root = div()
            .size_full()
            .bg(t.app_bg())
            .relative()
            .flex()
            .flex_col()
            // Toolbar
            .child(
                div()
                    .w_full()
                    .h(px(TOOLBAR_H))
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
                            .on_click(cx.listener(
                                move |this: &mut Self, _ev: &ClickEvent, _w, cx| {
                                    this.open_edit_dialog(None, cx);
                                },
                            ))
                            .flex()
                            .items_center()
                            .justify_center()
                            .child("＋ 新建会话"),
                    )
                    .child(div().flex_1())
                    .child(
                        div()
                            .h(px(26.))
                            .px_3()
                            .rounded_md()
                            .bg(t.secondary_btn_bg())
                            .text_color(t.text_on_secondary_btn())
                            .text_size(px(11.))
                            .cursor_pointer()
                            .id("rterm-settings-btn")
                            .hover(move |s| s.bg(settings_hover))
                            .on_click(cx.listener(
                                move |this: &mut Self, _ev: &ClickEvent, _w, cx| {
                                    this.open_settings(cx);
                                },
                            ))
                            .flex()
                            .items_center()
                            .justify_center()
                            .child("⚙ 设置"),
                    ),
            );

        root = root
            // 左 SessionTree + 右 TabBar + Content
            .child(
                div()
                    .w_full()
                    .flex_1()
                    .flex()
                    .child(
                        div()
                            .w(px(SIDEBAR_W))
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
                            .flex_col()
                            .child(self.tab_bar.clone())
                            .child(
                                div()
                                    .flex_1()
                                    .w_full()
                                    .relative()
                                    .child(self.render_content(cx)),
                            ),
                    ),
            )
            // 底部 StatusBar
            .child(self.status_bar.clone());

        // 启动加载出错时的红色横幅：绝对定位覆盖在内容区顶部，
        // 不参与布局高度计算（否则终端 cols/rows 会算错）。
        if let Some(err) = self.startup_error.clone() {
            root = root.child(
                div()
                    .absolute()
                    .top(px(TOOLBAR_H))
                    .left(px(SIDEBAR_W))
                    .right_0()
                    .px_3()
                    .py_1()
                    .flex()
                    .items_center()
                    .gap_2()
                    .bg(t.danger())
                    .text_color(t.text_on_primary_btn())
                    .text_size(px(11.))
                    .child(div().flex_1().truncate().child(err))
                    .child(
                        div()
                            .id("rterm-dismiss-startup-error")
                            .px_2()
                            .cursor_pointer()
                            .child("✕")
                            .on_click(cx.listener(
                                |this: &mut Self, _ev: &ClickEvent, _w, cx| {
                                    this.startup_error = None;
                                    cx.notify();
                                },
                            )),
                    ),
            );
        }

        if let Some(menu_state) = self.context_menu {
            let viewport = window.viewport_size();
            let menu_w = 170.0_f32;
            let menu_h = if menu_state.session_id.is_some() {
                248.0_f32
            } else {
                128.0_f32
            };
            let x = (menu_state.x as f32)
                .min(f32::from(viewport.width) - menu_w)
                .max(0.);
            let y = (menu_state.y as f32)
                .min(f32::from(viewport.height) - menu_h)
                .max(0.);

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
                        "rterm-ctx-open-terminal",
                        "▶ 打开终端",
                        false,
                        ContextMenuAction::OpenTerminal(sid),
                        cx,
                    ))
                    .child(self.render_menu_item(
                        "rterm-ctx-open-sftp",
                        "📁 打开 SFTP",
                        false,
                        ContextMenuAction::OpenSftp(sid),
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
                    "⬇ 导出会话…",
                    false,
                    ContextMenuAction::OpenExportDialog,
                    cx,
                ))
                .child(self.render_menu_item(
                    "rterm-ctx-import",
                    "⬆ 导入会话…",
                    false,
                    ContextMenuAction::OpenImportDialog,
                    cx,
                ))
                .child(self.render_menu_item(
                    "rterm-ctx-settings",
                    "⚙ 设置…",
                    false,
                    ContextMenuAction::OpenSettingsDialog,
                    cx,
                ));

            root = root.child(menu);
        }

        if let Some(dialog) = &self.edit_dialog {
            root = root.child(dialog.clone());
        }
        if let Some(dialog) = &self.settings_dialog {
            root = root.child(dialog.clone());
        }
        if let Some(dialog) = &self.import_export_dialog {
            root = root.child(dialog.clone());
        }

        // 删除会话二次确认（S7.5.3.10）。
        if let Some(session_id) = self.pending_delete {
            let name = {
                let state = cx.global::<AppState>();
                let store = state.session_store.lock();
                store
                    .as_ref()
                    .ok()
                    .and_then(|store| store.get(session_id))
                    .map(|session| session.display_label())
                    .unwrap_or_else(|| "该会话".to_string())
            };
            let cancel_hover = t.secondary_btn_hover();
            root = root.child(
                div()
                    .absolute()
                    .inset_0()
                    .bg(t.modal_overlay())
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(
                        div()
                            .w(px(360.))
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
                                    .child(format!("确定删除会话「{name}」？")),
                            )
                            .child(
                                div()
                                    .w_full()
                                    .text_center()
                                    .text_size(px(11.))
                                    .text_color(t.text_subtle())
                                    .child("删除后无法恢复"),
                            )
                            .child(
                                div()
                                    .flex()
                                    .justify_center()
                                    .gap_2()
                                    .child(
                                        div()
                                            .px_3()
                                            .h(px(26.))
                                            .rounded_md()
                                            .bg(t.secondary_btn_bg())
                                            .text_size(px(11.))
                                            .text_color(t.text_on_secondary_btn())
                                            .flex()
                                            .items_center()
                                            .cursor_pointer()
                                            .id("rterm-confirm-delete-cancel")
                                            .hover(move |s| s.bg(cancel_hover))
                                            .on_click(cx.listener(
                                                |this: &mut Self, _ev: &ClickEvent, _w, cx| {
                                                    this.pending_delete = None;
                                                    cx.notify();
                                                },
                                            ))
                                            .child("取消"),
                                    )
                                    .child(
                                        div()
                                            .px_3()
                                            .h(px(26.))
                                            .rounded_md()
                                            .bg(t.danger())
                                            .text_size(px(11.))
                                            .text_color(t.text_on_primary_btn())
                                            .flex()
                                            .items_center()
                                            .cursor_pointer()
                                            .id("rterm-confirm-delete-session")
                                            .on_click(cx.listener(
                                                move |this: &mut Self, _ev: &ClickEvent, _w, cx| {
                                                    this.pending_delete = None;
                                                    this.delete_session(session_id, cx);
                                                },
                                            ))
                                            .child("删除"),
                                    ),
                            ),
                    ),
            );
        }

        root
    }
}

impl MainWindow {
    fn render_content(&mut self, cx: &mut Context<Self>) -> gpui::AnyElement {
        if let Some(tab) = self.tabs.get(self.active_tab) {
            return tab.content.into_element();
        }

        let t = theme(cx);
        let hint = t.text_subtle();
        let subtle = t.text_placeholder();
        div()
            .size_full()
            .bg(t.app_bg())
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap_2()
            .child(
                div()
                    .text_size(px(16.))
                    .text_color(hint)
                    .child("🚀 欢迎使用 rterm"),
            )
            .child(
                div()
                    .mt_2()
                    .text_size(px(11.))
                    .text_color(subtle)
                    .child("双击左侧会话打开终端"),
            )
            .child(
                div()
                    .text_size(px(11.))
                    .text_color(subtle)
                    .child("Ctrl+T 新标签  ·  Ctrl+Tab 切换  ·  Ctrl+W 关闭"),
            )
            .child(
                div()
                    .text_size(px(11.))
                    .text_color(subtle)
                    .child("Ctrl+, 设置  ·  Ctrl+= / Ctrl+- 调整字号"),
            )
            .into_any_element()
    }

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
        let color = if danger {
            t.danger()
        } else {
            t.text_on_card_bg()
        };
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
                        ContextMenuAction::OpenTerminal(sid) => this.request_terminal(sid, cx),
                        ContextMenuAction::OpenSftp(sid) => this.open_sftp_tab(sid, cx),
                        ContextMenuAction::DeleteSession(sid) => {
                            // 不立即删除：先弹出确认卡片。
                            this.pending_delete = Some(sid);
                        }
                        ContextMenuAction::OpenExportDialog => this.open_import_export(true, cx),
                        ContextMenuAction::OpenImportDialog => this.open_import_export(false, cx),
                        ContextMenuAction::OpenSettingsDialog => this.open_settings(cx),
                    }
                    cx.notify();
                },
            ))
            .child(label)
            .into_any_element()
    }
}

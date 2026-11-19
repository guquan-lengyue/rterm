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

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};

use gpui::prelude::*;
use gpui::{
    actions, div, px, AnyElement, ClickEvent, Context, CursorStyle, Entity, FocusHandle,
    IntoElement, KeyBinding, KeyDownEvent, MouseButton, MouseDownEvent, Render, ScrollHandle,
    Window,
};
use rterm_sync::SyncBridge;
use uuid::Uuid;

use crate::event::{AppEvent, EventBusHost, MenuCommand};
use crate::state::{lock_or_recover, AppState, TabInfo, TabKind};
use crate::ui::theme;

use super::command_palette::{PaletteAction, PaletteItem, PaletteState};
use super::context_menu::{
    self, BlankMenuAction, ContextMenuAction, ContextMenuState, ContextMenuTarget,
    FolderMenuAction, SessionMenuAction,
};
use super::group_picker::GroupPickerDialog;
use super::icon_picker;
use super::import_export::{ImportExportDialog, IoMode};
use super::menu_bar::{self, MenuBarView};
use super::monitor::MonitorView;
use super::port_forward::PortForwardView;
use super::prompt::TextPromptDialog;
use super::session_dialog::SessionEditDialog;
use super::session_tree::SessionTreeView;
use super::settings::SettingsView;
use super::sftp::SftpView;
use super::status_bar::StatusBar;
use super::tab_bar::TabBar;
use super::terminal_tab::TerminalTabView;
use super::text_input::{TextInput, TextInputEvent};
use super::{SIDEBAR_W, TOOLBAR_H};

// ---------------------------------------------------------------------------
// 标签页快捷键（main.rs 里 App::on_action 转发到 MainWindow）
// ---------------------------------------------------------------------------

actions!(
    rterm_tabs,
    [
        NewTab,
        NextTab,
        CloseTab,
        NewSession,
        ToggleFullscreen,
        ToggleSessionLog,
        ToggleSftpDock,
        ToggleMenuBar,
        OpenPalette,
        PaletteUp,
        PaletteDown,
        PaletteConfirm,
        PaletteClose
    ]
);

pub fn key_bindings() -> [KeyBinding; 13] {
    [
        KeyBinding::new("ctrl-t", NewTab, None),
        KeyBinding::new("ctrl-tab", NextTab, None),
        KeyBinding::new("ctrl-w", CloseTab, None),
        KeyBinding::new("ctrl-shift-n", NewSession, None),
        KeyBinding::new("f11", ToggleFullscreen, None),
        KeyBinding::new("ctrl-shift-l", ToggleSessionLog, None),
        KeyBinding::new("ctrl-shift-s", ToggleSftpDock, None),
        KeyBinding::new("f10", ToggleMenuBar, None),
        KeyBinding::new("ctrl-p", OpenPalette, None),
        KeyBinding::new("up", PaletteUp, Some("CommandPalette")),
        KeyBinding::new("down", PaletteDown, Some("CommandPalette")),
        KeyBinding::new("enter", PaletteConfirm, Some("CommandPalette")),
        KeyBinding::new("escape", PaletteClose, Some("CommandPalette")),
    ]
}

// ---------------------------------------------------------------------------
// Tab
// ---------------------------------------------------------------------------

/// 标签页内容（S7 起支持多类型）。
#[derive(Clone)]
pub enum TabContentView {
    /// 终端 Tab：TerminalTabView 内部再组合可选的 SFTP 侧栏（S12）
    Terminal(Entity<TerminalTabView>),
    Sftp(Entity<SftpView>),
    Monitor(Entity<MonitorView>),
    PortForward(Entity<PortForwardView>),
}

impl TabContentView {
    fn focus_handle(&self, cx: &mut Context<MainWindow>) -> FocusHandle {
        match self {
            Self::Terminal(view) => view.read(cx).focus_handle(cx),
            Self::Sftp(view) => view.read(cx).focus_handle(),
            Self::Monitor(view) => view.read(cx).focus_handle(),
            Self::PortForward(view) => view.read(cx).focus_handle(),
        }
    }

    fn close(&self, cx: &mut Context<MainWindow>) {
        match self {
            Self::Terminal(view) => {
                view.update(cx, |view, cx| view.close(cx));
            }
            Self::Sftp(view) => {
                view.update(cx, |view, cx| view.shutdown(cx));
            }
            Self::Monitor(view) => {
                view.update(cx, |view, cx| view.shutdown(cx));
            }
            Self::PortForward(view) => {
                view.update(cx, |view, cx| view.shutdown(cx));
            }
        }
    }

    /// Tab 内部是否有需要独占键盘的弹窗（S7.5.3.9：屏蔽全局快捷键）。
    fn blocks_global_shortcuts(&self, cx: &mut Context<MainWindow>) -> bool {
        match self {
            Self::Terminal(view) => view.read(cx).blocks_global_shortcuts(cx),
            Self::Sftp(view) => view.read(cx).has_modal_dialog(),
            Self::Monitor(_) => false,
            Self::PortForward(_) => false,
        }
    }

    fn element(&self) -> AnyElement {
        match self {
            Self::Terminal(view) => view.clone().into_any_element(),
            Self::Sftp(view) => view.clone().into_any_element(),
            Self::Monitor(view) => view.clone().into_any_element(),
            Self::PortForward(view) => view.clone().into_any_element(),
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
    menu_bar: Entity<MenuBarView>,
    tabs: Vec<TabEntry>,
    /// 每个终端 Tab 一份路径同步状态机，tab_id → bridge。
    sync_bridges: HashMap<Uuid, Arc<Mutex<SyncBridge>>>,
    active_tab: usize,
    /// 切换 / 新建 Tab 后，下一帧把焦点交给活动终端
    focus_pending: bool,
    /// 主窗口根节点焦点：右键菜单 / 删除确认打开时承接 Esc
    root_focus: FocusHandle,
    root_focus_pending: bool,
    overlay_focus_active: bool,
    /// 上一帧菜单栏是否展开（关闭时把焦点还给终端）。
    menu_was_open: bool,
    pub edit_dialog: Option<Entity<SessionEditDialog>>,
    settings_dialog: Option<Entity<SettingsView>>,
    import_export_dialog: Option<Entity<ImportExportDialog>>,
    context_menu: Option<ContextMenuState>,
    /// 新建 / 重命名分组的文本弹窗。
    folder_prompt: Option<Entity<TextPromptDialog>>,
    /// 移动到分组弹窗。
    group_dialog: Option<Entity<GroupPickerDialog>>,
    /// 「更改图标」弹窗对应的会话。
    icon_dialog_session: Option<Uuid>,
    /// 快捷键 / 关于信息弹窗。
    info_dialog: Option<(String, String)>,
    /// 会话树当前选中项（顶部菜单栏「连接」依赖）。
    selected_session: Option<Uuid>,
    sidebar_visible: bool,
    /// 等待确认删除的会话（S7.5.3.10 二次确认）。
    pending_delete: Option<Uuid>,
    /// 启动期加载会话的错误提示；点击横幅 ✕ 后清空。
    startup_error: Option<String>,
    /// Ctrl+P 命令面板（打开期间独占键盘焦点）。
    palette: Option<PaletteState>,
    /// 命令面板结果列表的滚动状态（选中项自动滚入视野）
    palette_scroll: ScrollHandle,
    /// 命令面板查询框（TextInput：支持 IME 与光标编辑）
    palette_input: Option<Entity<TextInput>>,
    /// 文件夹右键菜单打开时记录分组路径，供「新建会话」预填
    pending_new_folder: Option<String>,
}

impl MainWindow {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let session_tree = cx.new(SessionTreeView::new);
        let status_bar = cx.new(StatusBar::new);
        let tab_bar = cx.new(TabBar::new);
        let menu_bar = cx.new(MenuBarView::new);
        let sidebar_visible = cx.global::<AppState>().settings.sidebar_visible;

        let host = cx.global::<AppState>().event_bus_host.clone();
        let startup_error = cx.global::<AppState>().startup_error.clone();
        cx.subscribe(
            &host,
            |this: &mut Self, _host, event: &AppEvent, cx| match event {
                AppEvent::SessionConnectRequested { session_id } => {
                    this.request_terminal_tab(*session_id, cx);
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
                AppEvent::SessionTreeOpenContextMenuAt {
                    session_id,
                    folder,
                    x,
                    y,
                } => {
                    let target = match (session_id, folder) {
                        (Some(id), _) => ContextMenuTarget::Session(*id),
                        (None, Some(path)) => ContextMenuTarget::Folder(path.clone()),
                        (None, None) => ContextMenuTarget::Blank,
                    };
                    this.pending_new_folder = folder.clone();
                    this.context_menu = Some(ContextMenuState {
                        target,
                        x: *x,
                        y: *y,
                    });
                    this.root_focus_pending = true;
                    this.overlay_focus_active = true;
                    cx.notify();
                }
                AppEvent::SessionTreeSelectionChanged { session_id } => {
                    this.selected_session = *session_id;
                    let has_selection = this.selected_session.is_some();
                    let visible = this.sidebar_visible;
                    this.menu_bar.update(cx, |bar, cx| {
                        bar.set_state(has_selection, visible, cx);
                    });
                }
                AppEvent::SessionMoveRequested { session_id, folder } => {
                    this.move_session(*session_id, folder.clone(), cx);
                }
                AppEvent::GroupPickerCloseRequested => {
                    this.group_dialog = None;
                    this.focus_pending = true;
                    cx.notify();
                }
                AppEvent::FolderCreateRequested { path } => {
                    this.create_folder(path.clone(), cx);
                }
                AppEvent::FolderRenameRequested { from, to } => {
                    this.rename_folder(from.clone(), to.clone(), cx);
                }
                AppEvent::TextPromptCloseRequested => {
                    this.folder_prompt = None;
                    this.focus_pending = true;
                    cx.notify();
                }
                AppEvent::MenuCommand(command) => {
                    this.handle_menu_command(*command, cx);
                }
                AppEvent::PortForwardOpened { .. } | AppEvent::PortForwardClosed { .. } => {
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
                AppEvent::TerminalCwdHint {
                    tab_id,
                    session_id,
                    cwd,
                    ..
                } => {
                    // 终端 → SFTP：同一会话的所有 SFTP Tab 跟随；先登记期望事件，
                    // 避免 SftpNavigated 反向注入 cd 形成回环。
                    let sftp_views: Vec<Entity<SftpView>> = this
                        .tabs
                        .iter()
                        .filter(|tab| {
                            tab.session_id == *session_id && matches!(&tab.kind, TabKind::Sftp)
                        })
                        .filter_map(|tab| match &tab.content {
                            TabContentView::Sftp(view) => Some(view.clone()),
                            _ => None,
                        })
                        .collect();
                    if !sftp_views.is_empty() {
                        if let Some(bridge) = this.sync_bridges.get(tab_id).cloned() {
                            lock_or_recover(&bridge).expect_sftp_navigation(sftp_views.len());
                        }
                        for view in sftp_views {
                            view.update(cx, |view, cx| view.navigate_to_remote(cwd, cx));
                        }
                    }
                }
                AppEvent::SftpOpened { tab_id, session_id } => {
                    // SFTP 连接完成：把远端 home 告诉同会话的终端桥，
                    // 并把 home 标记为已同步，避免初始化导航反向 cd。
                    let home = this
                        .tabs
                        .iter()
                        .find(|tab| tab.id == *tab_id)
                        .and_then(|tab| match &tab.content {
                            TabContentView::Sftp(view) => view.read(cx).remote_home(),
                            _ => None,
                        });
                    if let Some(home) = home {
                        for tab in this.tabs.iter().filter(|tab| {
                            tab.session_id == *session_id && matches!(&tab.kind, TabKind::Terminal)
                        }) {
                            if let Some(bridge) = this.sync_bridges.get(&tab.id) {
                                let mut bridge = lock_or_recover(bridge);
                                bridge.set_remote_home(home.clone());
                                bridge.mark_sftp_synced(Path::new(&home));
                            }
                        }
                    }
                }
                AppEvent::SftpNavigated {
                    session_id, path, ..
                } => {
                    // SFTP → 终端：选择活动终端（否则该会话第一个终端）注入 cd。
                    let active_id = this.tabs.get(this.active_tab).map(|tab| tab.id);
                    let target = this
                        .tabs
                        .iter()
                        .find(|tab| {
                            tab.session_id == *session_id
                                && matches!(&tab.kind, TabKind::Terminal)
                                && Some(tab.id) == active_id
                        })
                        .or_else(|| {
                            this.tabs.iter().find(|tab| {
                                tab.session_id == *session_id
                                    && matches!(&tab.kind, TabKind::Terminal)
                            })
                        })
                        .and_then(|tab| match &tab.content {
                            TabContentView::Terminal(view) => {
                                Some((tab.id, view.read(cx).terminal()))
                            }
                            _ => None,
                        });

                    let Some((terminal_tab_id, view)) = target else {
                        return;
                    };
                    let Some(bridge) = this.sync_bridges.get(&terminal_tab_id).cloned() else {
                        return;
                    };
                    let command = {
                        let mut bridge = lock_or_recover(&bridge);
                        bridge.on_sftp_navigated(Path::new(path))
                    };
                    if let Some(command) = command {
                        view.update(cx, |view, cx| view.inject_command(&command, cx));
                    }
                }
                _ => {}
            },
        )
        .detach();

        Self {
            session_tree,
            status_bar,
            tab_bar,
            menu_bar,
            tabs: Vec::new(),
            sync_bridges: HashMap::new(),
            active_tab: 0,
            focus_pending: false,
            root_focus: cx.focus_handle(),
            root_focus_pending: false,
            overlay_focus_active: false,
            menu_was_open: false,
            edit_dialog: None,
            settings_dialog: None,
            import_export_dialog: None,
            context_menu: None,
            folder_prompt: None,
            group_dialog: None,
            icon_dialog_session: None,
            info_dialog: None,
            selected_session: None,
            sidebar_visible,
            pending_delete: None,
            startup_error,
            palette: None,
            palette_scroll: ScrollHandle::new(),
            palette_input: None,
            pending_new_folder: None,
        }
    }

    // -----------------------------------------------------------------------
    // Tab 生命周期
    // -----------------------------------------------------------------------

    fn open_terminal_tab(&mut self, session_id: Uuid, cx: &mut Context<Self>) {
        let title = {
            let state = cx.global::<AppState>();
            let store = lock_or_recover(&state.session_store);
            store
                .get(session_id)
                .map(|session| session.display_label())
                .unwrap_or_else(|| "终端".to_string())
        };
        let title = self.unique_title(title, TabKind::Terminal);

        let tab = cx.new(|cx| TerminalTabView::new(cx, session_id));
        let terminal = tab.read(cx).terminal();
        let id = terminal.read(cx).tab_id();
        self.sync_bridges
            .insert(id, terminal.read(cx).sync_bridge());

        self.tabs.push(TabEntry {
            id,
            session_id,
            title,
            kind: TabKind::Terminal,
            content: TabContentView::Terminal(tab),
        });
        self.active_tab = self.tabs.len() - 1;
        self.focus_pending = true;
        self.ensure_auto_start_forwards(session_id, cx);
        self.sync_tab_bar(cx);
        cx.notify();
    }

    fn open_sftp_tab(&mut self, session_id: Uuid, cx: &mut Context<Self>) {
        let title = {
            let state = cx.global::<AppState>();
            let store = lock_or_recover(&state.session_store);
            store
                .get(session_id)
                .map(|session| format!("{} · SFTP", session.display_label()))
                .unwrap_or_else(|| "SFTP".to_string())
        };
        let title = self.unique_title(title, TabKind::Sftp);

        let view = cx.new(SftpView::new);
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

    fn open_monitor_tab(&mut self, session_id: Uuid, cx: &mut Context<Self>) {
        let title = {
            let state = cx.global::<AppState>();
            let store = lock_or_recover(&state.session_store);
            store
                .get(session_id)
                .map(|session| format!("{} · 监控", session.display_label()))
                .unwrap_or_else(|| "监控".to_string())
        };
        let title = self.unique_title(title, TabKind::Monitor);

        let view = cx.new(MonitorView::new);
        view.update(cx, |view, cx| view.connect(session_id, cx));
        let id = view.read(cx).tab_id();

        self.tabs.push(TabEntry {
            id,
            session_id,
            title,
            kind: TabKind::Monitor,
            content: TabContentView::Monitor(view),
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

    /// 同类型 + 同会话的已开 Tab（用于「打开终端/SFTP/监控」时复用，避免重复实例）。
    fn find_open_tab(&self, session_id: Uuid, kind: TabKind) -> Option<Uuid> {
        self.tabs
            .iter()
            .find(|tab| tab.session_id == session_id && tab.kind == kind)
            .map(|tab| tab.id)
    }

    /// 同名 Tab 加序号，避免多个 Tab 完全无法区分。
    fn unique_title(&self, base: String, kind: TabKind) -> String {
        let taken = |title: &str| {
            self.tabs
                .iter()
                .any(|tab| tab.kind == kind && tab.title == title)
        };
        if !taken(&base) {
            return base;
        }
        for n in 2..1000 {
            let candidate = format!("{base} ({n})");
            if !taken(&candidate) {
                return candidate;
            }
        }
        base
    }

    /// 打开终端：已有同会话终端 Tab 时直接激活，否则新建。
    fn request_terminal_tab(&mut self, session_id: Uuid, cx: &mut Context<Self>) {
        if let Some(tab_id) = self.find_open_tab(session_id, TabKind::Terminal) {
            self.activate_tab(tab_id, cx);
        } else {
            self.open_terminal_tab(session_id, cx);
        }
    }

    /// 打开 SFTP：已有同会话 SFTP Tab 时直接激活。
    fn request_sftp_tab(&mut self, session_id: Uuid, cx: &mut Context<Self>) {
        if let Some(tab_id) = self.find_open_tab(session_id, TabKind::Sftp) {
            self.activate_tab(tab_id, cx);
        } else {
            self.open_sftp_tab(session_id, cx);
        }
    }

    /// 打开监控：已有同会话监控 Tab 时直接激活。
    fn request_monitor_tab(&mut self, session_id: Uuid, cx: &mut Context<Self>) {
        if let Some(tab_id) = self.find_open_tab(session_id, TabKind::Monitor) {
            self.activate_tab(tab_id, cx);
        } else {
            self.open_monitor_tab(session_id, cx);
        }
    }

    fn open_port_forward_tab(&mut self, session_id: Uuid, cx: &mut Context<Self>) {
        self.open_port_forward_tab_impl(session_id, true, cx);
    }

    /// 会话连接时按 `auto_start` 拉起转发：后台开 Tab，不抢占焦点。
    fn ensure_auto_start_forwards(&mut self, session_id: Uuid, cx: &mut Context<Self>) {
        let has_auto_start = {
            let store = lock_or_recover(&cx.global::<AppState>().session_store);
            store
                .get(session_id)
                .map(|session| session.port_forwards.iter().any(|rule| rule.auto_start))
                .unwrap_or(false)
        };
        if has_auto_start
            && self
                .find_open_tab(session_id, TabKind::PortForward)
                .is_none()
        {
            self.open_port_forward_tab_impl(session_id, false, cx);
        }
    }

    fn open_port_forward_tab_impl(
        &mut self,
        session_id: Uuid,
        activate: bool,
        cx: &mut Context<Self>,
    ) {
        let title = {
            let state = cx.global::<AppState>();
            let store = lock_or_recover(&state.session_store);
            store
                .get(session_id)
                .map(|session| format!("{} · 端口转发", session.display_label()))
                .unwrap_or_else(|| "端口转发".to_string())
        };
        let title = self.unique_title(title, TabKind::PortForward);

        let view = cx.new(PortForwardView::new);
        view.update(cx, |view, cx| view.connect(session_id, cx));
        let id = view.read(cx).tab_id();

        self.tabs.push(TabEntry {
            id,
            session_id,
            title,
            kind: TabKind::PortForward,
            content: TabContentView::PortForward(view),
        });
        if activate {
            self.active_tab = self.tabs.len() - 1;
            self.focus_pending = true;
        }
        self.sync_tab_bar(cx);
        cx.notify();
    }

    /// 某会话已无普通 Tab 时，关闭其端口转发 Tab（停止全部规则）。
    fn close_forward_tabs_if_idle(&mut self, session_id: Uuid, cx: &mut Context<Self>) {
        let has_session_tab = self
            .tabs
            .iter()
            .any(|tab| tab.session_id == session_id && tab.kind != TabKind::PortForward);
        if has_session_tab {
            return;
        }
        let forward_tabs: Vec<Uuid> = self
            .tabs
            .iter()
            .filter(|tab| tab.session_id == session_id && tab.kind == TabKind::PortForward)
            .map(|tab| tab.id)
            .collect();
        for tab_id in forward_tabs {
            self.close_tab(tab_id, cx);
        }
    }

    /// 打开端口转发：已有同会话 Tab 时直接激活。
    fn request_port_forward_tab(&mut self, session_id: Uuid, cx: &mut Context<Self>) {
        if let Some(tab_id) = self.find_open_tab(session_id, TabKind::PortForward) {
            self.activate_tab(tab_id, cx);
        } else {
            self.open_port_forward_tab(session_id, cx);
        }
    }

    fn close_tab(&mut self, tab_id: Uuid, cx: &mut Context<Self>) {
        let Some(index) = self.tabs.iter().position(|tab| tab.id == tab_id) else {
            return;
        };
        let entry = self.tabs.remove(index);
        let session_id = entry.session_id;
        let closed_kind = entry.kind;
        self.sync_bridges.remove(&tab_id);
        entry.content.close(cx);

        // 会话的终端 / SFTP / 监控全部关闭后，端口转发随会话一起停止。
        if closed_kind != TabKind::PortForward {
            self.close_forward_tabs_if_idle(session_id, cx);
        }

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
            self.sync_bridges.remove(&entry.id);
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
            self.sync_bridges.remove(&entry.id);
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
                kind: tab.kind,
            })
            .collect();
        let active = self.active_tab;
        self.tab_bar
            .update(cx, |bar, cx| bar.set_tabs(tabs, active, cx));
        // 状态栏右侧跟随活动 Tab 的会话（不是“最后连接的会话”）
        let active_session = self.tabs.get(self.active_tab).map(|tab| tab.session_id);
        self.status_bar
            .update(cx, |bar, cx| bar.set_active_session(active_session, cx));
    }

    /// 是否有任何弹窗 / 确认卡片打开（弹窗期间屏蔽全局 Tab 快捷键）。
    fn dialogs_open(&self, cx: &mut Context<Self>) -> bool {
        self.palette.is_some()
            || self.settings_dialog.is_some()
            || self.edit_dialog.is_some()
            || self.import_export_dialog.is_some()
            || self.pending_delete.is_some()
            || self.context_menu.is_some()
            || self.folder_prompt.is_some()
            || self.group_dialog.is_some()
            || self.icon_dialog_session.is_some()
            || self.info_dialog.is_some()
            || self.menu_bar.read(cx).open_kind().is_some()
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

    /// Ctrl+Shift+N：打开「新建会话」对话框。
    pub fn new_session_shortcut(&mut self, cx: &mut Context<Self>) {
        if self.dialogs_open(cx) {
            return;
        }
        self.open_edit_dialog(None, cx);
    }

    /// Ctrl+Shift+L：按 Tab 开关当前终端的会话日志。
    pub fn toggle_session_log_shortcut(&mut self, cx: &mut Context<Self>) {
        if self.dialogs_open(cx) {
            return;
        }
        let terminal = self
            .tabs
            .get(self.active_tab)
            .and_then(|tab| match &tab.content {
                TabContentView::Terminal(view) => Some(view.read(cx).terminal()),
                _ => None,
            });
        match terminal {
            Some(view) => {
                view.update(cx, |view, cx| view.toggle_session_log(cx));
            }
            None => self.set_status("当前标签不是终端，无法开启会话日志", cx),
        }
    }

    /// Ctrl+Shift+S：开关当前终端 Tab 的 SFTP 侧栏（S12 12.6）。
    pub fn toggle_sftp_dock_shortcut(&mut self, cx: &mut Context<Self>) {
        if self.dialogs_open(cx) {
            return;
        }
        let terminal_tab = self
            .tabs
            .get(self.active_tab)
            .and_then(|tab| match &tab.content {
                TabContentView::Terminal(view) => Some(view.clone()),
                _ => None,
            });
        match terminal_tab {
            Some(view) => {
                view.update(cx, |view, cx| view.toggle_dock(cx));
            }
            None => self.set_status("当前标签不是终端，无法切换 SFTP 侧栏", cx),
        }
    }

    // -----------------------------------------------------------------------
    // 顶部菜单栏 / 分组 / 右键动作
    // -----------------------------------------------------------------------

    /// F10：展开 / 关闭顶部菜单栏。
    pub fn toggle_menu_bar_shortcut(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.dialogs_open(cx) && self.menu_bar.read(cx).open_kind().is_none() {
            return;
        }
        let has_selection = self.selected_session.is_some();
        let visible = self.sidebar_visible;
        self.menu_bar.update(cx, |bar, cx| {
            bar.set_state(has_selection, visible, cx);
            bar.toggle_first(window, cx);
        });
        cx.notify();
    }

    fn handle_menu_command(&mut self, command: MenuCommand, cx: &mut Context<Self>) {
        let selected = self.selected_session;
        match command {
            MenuCommand::NewSession => self.open_edit_dialog(None, cx),
            MenuCommand::NewFolder => self.open_folder_prompt(None, cx),
            MenuCommand::ImportSessions => self.open_import_export(false, cx),
            MenuCommand::ExportSessions => self.open_import_export(true, cx),
            MenuCommand::OpenSettings => self.open_settings(cx),
            MenuCommand::OpenTerminal => {
                if let Some(id) = selected {
                    self.request_terminal_tab(id, cx);
                }
            }
            MenuCommand::OpenSftp => {
                if let Some(id) = selected {
                    self.request_sftp_tab(id, cx);
                }
            }
            MenuCommand::OpenMonitor => {
                if let Some(id) = selected {
                    self.request_monitor_tab(id, cx);
                }
            }
            MenuCommand::OpenPortForward => {
                if let Some(id) = selected {
                    self.request_port_forward_tab(id, cx);
                }
            }
            MenuCommand::ToggleSidebar => self.toggle_sidebar(cx),
            MenuCommand::ExpandAll => {
                self.emit_event(cx, AppEvent::SessionTreeExpandAll { expanded: true })
            }
            MenuCommand::CollapseAll => {
                self.emit_event(cx, AppEvent::SessionTreeExpandAll { expanded: false })
            }
            MenuCommand::ToggleTheme => self.cycle_theme(cx),
            MenuCommand::ZoomIn => self.change_font_size(1.0, cx),
            MenuCommand::ZoomOut => self.change_font_size(-1.0, cx),
            MenuCommand::ShowShortcuts => {
                self.info_dialog = Some(("快捷键".into(), shortcut_text()));
            }
            MenuCommand::OpenLogDir => self.open_log_dir(cx),
            MenuCommand::About => {
                self.info_dialog = Some((
                    "关于 rterm".into(),
                    format!(
                        "rterm {}\n\nSSH 会话管理 / 终端 / SFTP / 监控 / 端口转发",
                        env!("CARGO_PKG_VERSION")
                    ),
                ));
            }
        }
        // 菜单动作执行后关闭下拉，把焦点还给内容区。
        self.menu_bar.update(cx, |bar, cx| bar.close(cx));
        self.focus_pending = true;
        cx.notify();
    }

    fn toggle_sidebar(&mut self, cx: &mut Context<Self>) {
        self.sidebar_visible = !self.sidebar_visible;
        {
            let state = cx.global_mut::<AppState>();
            state.settings.sidebar_visible = self.sidebar_visible;
            if let Err(error) = state.settings.save() {
                log::error!("保存侧栏可见性失败: {error:#}");
            }
        }
        let has_selection = self.selected_session.is_some();
        let visible = self.sidebar_visible;
        self.menu_bar.update(cx, |bar, cx| {
            bar.set_state(has_selection, visible, cx);
        });
        cx.notify();
    }

    fn cycle_theme(&mut self, cx: &mut Context<Self>) {
        let themes = crate::theme::all_themes();
        let current = cx.global::<AppState>().settings.default_theme.clone();
        let index = themes
            .iter()
            .position(|theme| theme.name() == current)
            .unwrap_or(0);
        let next = themes[(index + 1) % themes.len()].name().to_string();
        {
            let state = cx.global_mut::<AppState>();
            state.settings.default_theme = next.clone();
            state.theme = crate::theme::theme_by_name(&next);
            if let Err(error) = state.settings.save() {
                log::error!("保存主题失败: {error:#}");
            }
        }
        self.emit_event(cx, AppEvent::AppThemeChanged { name: next.clone() });
        self.set_status(format!("主题已切换: {next}"), cx);
    }

    fn change_font_size(&mut self, delta: f32, cx: &mut Context<Self>) {
        let size = {
            let state = cx.global_mut::<AppState>();
            let size = (state.settings.default_font_size + delta).clamp(8.0, 40.0);
            state.settings.default_font_size = size;
            if let Err(error) = state.settings.save() {
                log::error!("保存字号失败: {error:#}");
            }
            size
        };
        self.emit_event(cx, AppEvent::AppSettingsChanged);
        self.set_status(format!("终端字号: {size:.0}"), cx);
    }

    fn open_log_dir(&mut self, cx: &mut Context<Self>) {
        let settings = cx.global::<AppState>().settings.clone();
        let dir = crate::state::logs_dir_path(&settings);
        if let Err(error) = std::fs::create_dir_all(&dir) {
            self.set_status(format!("创建日志目录失败: {error}"), cx);
            return;
        }
        #[cfg(target_os = "windows")]
        let opened = std::process::Command::new("explorer.exe").arg(&dir).spawn();
        #[cfg(not(target_os = "windows"))]
        let opened = std::process::Command::new("xdg-open").arg(&dir).spawn();

        match opened {
            Ok(_) => self.set_status(format!("已打开日志目录: {}", dir.display()), cx),
            Err(error) => self.set_status(format!("打开日志目录失败: {error}"), cx),
        }
    }

    fn emit_event(&self, cx: &mut Context<Self>, event: AppEvent) {
        let host = cx.global::<AppState>().event_bus_host.clone();
        cx.update_entity(&host, |_host, cx: &mut Context<EventBusHost>| {
            cx.emit(event);
        });
    }

    /// 空白 / 分组 / 会话三类右键动作分发。
    fn handle_context_action(&mut self, action: ContextMenuAction, cx: &mut Context<Self>) {
        let target = self.context_menu.as_ref().map(|state| state.target.clone());
        self.context_menu = None;

        match action {
            ContextMenuAction::Blank(action) => match action {
                BlankMenuAction::NewSession => self.open_edit_dialog(None, cx),
                BlankMenuAction::NewFolder => self.open_folder_prompt(None, cx),
                BlankMenuAction::ImportSessions => self.open_import_export(false, cx),
                BlankMenuAction::ExpandAll => {
                    self.emit_event(cx, AppEvent::SessionTreeExpandAll { expanded: true })
                }
                BlankMenuAction::CollapseAll => {
                    self.emit_event(cx, AppEvent::SessionTreeExpandAll { expanded: false })
                }
            },
            ContextMenuAction::Folder(action) => {
                let ContextMenuTarget::Folder(folder) = (match target {
                    Some(target) => target,
                    None => ContextMenuTarget::Blank,
                }) else {
                    self.focus_pending = true;
                    return;
                };
                match action {
                    FolderMenuAction::NewSessionInFolder => {
                        self.pending_new_folder = Some(folder.clone());
                        self.open_edit_dialog(None, cx);
                        if let Some(dialog) = self.edit_dialog.clone() {
                            dialog.update(cx, |dialog, cx| {
                                dialog.group_picker.update(cx, |picker, cx| {
                                    picker.set_selected(Some(folder.clone()), cx);
                                });
                                cx.notify();
                            });
                        }
                    }
                    FolderMenuAction::NewSubfolder => {
                        self.open_folder_prompt(Some(folder.clone()), cx)
                    }
                    FolderMenuAction::RenameFolder => self.open_rename_prompt(folder, cx),
                    FolderMenuAction::DeleteFolder => self.delete_folder(folder, cx),
                    FolderMenuAction::ExportFolder => self.open_export_folder(folder, cx),
                }
            }
            ContextMenuAction::Session(action) => {
                let ContextMenuTarget::Session(session_id) = (match target {
                    Some(target) => target,
                    None => ContextMenuTarget::Blank,
                }) else {
                    self.focus_pending = true;
                    return;
                };
                match action {
                    SessionMenuAction::OpenTerminal => self.request_terminal_tab(session_id, cx),
                    SessionMenuAction::OpenSftp => self.request_sftp_tab(session_id, cx),
                    SessionMenuAction::OpenMonitor => self.request_monitor_tab(session_id, cx),
                    SessionMenuAction::OpenPortForward => {
                        self.request_port_forward_tab(session_id, cx)
                    }
                    SessionMenuAction::EditSession => self.edit_session(session_id, cx),
                    SessionMenuAction::ChangeIcon => {
                        self.icon_dialog_session = Some(session_id);
                    }
                    SessionMenuAction::MoveToFolder => self.open_move_dialog(session_id, cx),
                    SessionMenuAction::DuplicateSession => self.duplicate_session(session_id, cx),
                    SessionMenuAction::DeleteSession => {
                        self.pending_delete = Some(session_id);
                        self.root_focus_pending = true;
                        self.overlay_focus_active = true;
                    }
                }
            }
        }
        self.pending_new_folder = None;
        self.focus_pending = true;
        cx.notify();
    }

    // ---------------- 分组操作 ----------------

    fn open_folder_prompt(&mut self, parent: Option<String>, cx: &mut Context<Self>) {
        let dialog = cx.new(|cx| match parent.clone() {
            Some(parent) => TextPromptDialog::new_folder_under(cx, parent),
            None => TextPromptDialog::new_folder(cx),
        });
        self.folder_prompt = Some(dialog);
        self.context_menu = None;
        cx.notify();
    }

    fn open_rename_prompt(&mut self, folder: String, cx: &mut Context<Self>) {
        let dialog = cx.new(|cx| TextPromptDialog::rename_folder(cx, folder));
        self.folder_prompt = Some(dialog);
        self.context_menu = None;
        cx.notify();
    }

    fn create_folder(&mut self, path: String, cx: &mut Context<Self>) {
        let result = {
            let state = cx.global_mut::<AppState>();
            let mut store = lock_or_recover(&state.session_store);
            store.create_folder(&path)
        };
        match result {
            Ok(()) => {
                self.persist_and_refresh_folders(cx);
                self.folder_prompt = None;
                self.focus_pending = true;
                self.set_status(format!("已创建分组「{path}」"), cx);
            }
            Err(error) => {
                if let Some(dialog) = self.folder_prompt.clone() {
                    dialog.update(cx, |dialog, cx| {
                        dialog.set_error(format!("{error}"), cx);
                    });
                }
            }
        }
        cx.notify();
    }

    fn rename_folder(&mut self, from: String, to: String, cx: &mut Context<Self>) {
        let result = {
            let state = cx.global_mut::<AppState>();
            let mut store = lock_or_recover(&state.session_store);
            store.rename_folder(&from, &to)
        };
        match result {
            Ok(()) => {
                self.persist_and_refresh_folders(cx);
                self.folder_prompt = None;
                self.focus_pending = true;
                self.set_status(format!("分组已重命名为「{to}」"), cx);
            }
            Err(error) => {
                if let Some(dialog) = self.folder_prompt.clone() {
                    dialog.update(cx, |dialog, cx| {
                        dialog.set_error(format!("{error}"), cx);
                    });
                }
            }
        }
        cx.notify();
    }

    fn delete_folder(&mut self, folder: String, cx: &mut Context<Self>) {
        let result = {
            let state = cx.global_mut::<AppState>();
            let mut store = lock_or_recover(&state.session_store);
            store.delete_empty_folder(&folder)
        };
        match result {
            Ok(()) => {
                self.persist_and_refresh_folders(cx);
                self.set_status(format!("已删除空分组「{folder}」"), cx);
            }
            Err(error) => self.set_status(format!("删除分组失败: {error}"), cx),
        }
        cx.notify();
    }

    fn persist_and_refresh_folders(&mut self, cx: &mut Context<Self>) {
        let store = cx.global::<AppState>().session_store.clone();
        if let Err(error) = crate::state::persist_sessions(&store) {
            log::error!("保存分组失败: {error:#}");
        }
        self.emit_event(cx, AppEvent::SessionTreeFoldersChanged);
    }

    fn open_export_folder(&mut self, folder: String, cx: &mut Context<Self>) {
        let dialog = cx.new(|cx| ImportExportDialog::new(cx, IoMode::Export));
        dialog.update(cx, |dialog, cx| dialog.set_export_folder(folder, cx));
        self.import_export_dialog = Some(dialog);
        self.context_menu = None;
        cx.notify();
    }

    fn move_session(&mut self, session_id: Uuid, folder: Option<String>, cx: &mut Context<Self>) {
        let result = {
            let state = cx.global_mut::<AppState>();
            let mut store = lock_or_recover(&state.session_store);
            store.move_session(session_id, folder.as_deref())
        };
        match result {
            Ok(()) => {
                let store = cx.global::<AppState>().session_store.clone();
                if let Err(error) = crate::state::persist_sessions(&store) {
                    log::error!("移动会话后保存失败: {error:#}");
                }
                self.emit_event(cx, AppEvent::SessionUpdated { session_id });
                self.group_dialog = None;
                self.focus_pending = true;
                self.set_status(
                    format!(
                        "已移动到「{}」",
                        folder.clone().unwrap_or_else(|| "根目录".into())
                    ),
                    cx,
                );
            }
            Err(error) => self.set_status(format!("移动失败: {error}"), cx),
        }
        cx.notify();
    }

    fn open_move_dialog(&mut self, session_id: Uuid, cx: &mut Context<Self>) {
        let info = {
            let store = lock_or_recover(&cx.global::<AppState>().session_store);
            store
                .get(session_id)
                .map(|session| (session.display_label(), session.folder.clone()))
        };
        if let Some((name, folder)) = info {
            let dialog = cx.new(|cx| GroupPickerDialog::new(cx, session_id, name, folder));
            self.group_dialog = Some(dialog);
            self.context_menu = None;
            cx.notify();
        }
    }

    fn duplicate_session(&mut self, session_id: Uuid, cx: &mut Context<Self>) {
        let new_id = Uuid::new_v4();
        let result: Result<(), String> = {
            let state = cx.global_mut::<AppState>();
            let mut store = lock_or_recover(&state.session_store);
            match store.get(session_id).cloned() {
                Some(mut copy) => {
                    copy.id = new_id;
                    copy.name = if copy.name.is_empty() {
                        "未命名会话 副本".to_string()
                    } else {
                        format!("{} 副本", copy.name)
                    };
                    // 规则复制后重新分配 id，避免与源会话的运行态互相干扰。
                    for rule in &mut copy.port_forwards {
                        rule.id = Uuid::new_v4();
                    }
                    copy.created_at = rterm_session::default_now();
                    copy.last_modified = rterm_session::default_now();
                    copy.last_connected = None;
                    store.upsert(copy);
                    Ok(())
                }
                None => Err("会话不存在".into()),
            }
        };
        match result {
            Ok(()) => {
                let store = cx.global::<AppState>().session_store.clone();
                if let Err(error) = crate::state::persist_sessions(&store) {
                    log::error!("复制会话后保存失败: {error:#}");
                }
                self.emit_event(cx, AppEvent::SessionCreated { session_id: new_id });
                self.set_status("已复制会话", cx);
            }
            Err(error) => self.set_status(format!("复制会话失败: {error}"), cx),
        }
        cx.notify();
    }

    fn apply_session_icon(&mut self, session_id: Uuid, icon: &str, cx: &mut Context<Self>) {
        {
            let state = cx.global_mut::<AppState>();
            let mut store = lock_or_recover(&state.session_store);
            if let Some(session) = store.get_mut(session_id) {
                session.icon = Some(icon.to_string());
                session.last_modified = rterm_session::default_now();
            }
        }
        let store = cx.global::<AppState>().session_store.clone();
        if let Err(error) = crate::state::persist_sessions(&store) {
            log::error!("保存会话图标失败: {error:#}");
        }
        self.emit_event(cx, AppEvent::SessionUpdated { session_id });
        self.icon_dialog_session = None;
        self.focus_pending = true;
        self.set_status("会话图标已更新", cx);
        cx.notify();
    }

    // -----------------------------------------------------------------------
    // 命令面板（Ctrl+P）
    // -----------------------------------------------------------------------

    /// 打开命令面板：会话（终端 / SFTP / 监控）+ 常用操作。
    pub fn open_command_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.palette.is_some()
            || self.settings_dialog.is_some()
            || self.edit_dialog.is_some()
            || self.import_export_dialog.is_some()
            || self.pending_delete.is_some()
        {
            return;
        }
        let mut items = Vec::new();
        {
            let state = cx.global::<AppState>();
            let store = lock_or_recover(&state.session_store);
            for session in store.list() {
                let label = session.display_label();
                let id = session.id;
                items.push(PaletteItem::new(
                    format!("打开终端 · {label}"),
                    "终端",
                    PaletteAction::OpenTerminal(id),
                ));
                items.push(PaletteItem::new(
                    format!("打开 SFTP · {label}"),
                    "SFTP",
                    PaletteAction::OpenSftp(id),
                ));
                items.push(PaletteItem::new(
                    format!("打开监控 · {label}"),
                    "监控",
                    PaletteAction::OpenMonitor(id),
                ));
            }
        }
        items.push(PaletteItem::new(
            "新建会话…",
            "会话",
            PaletteAction::NewSession,
        ));
        items.push(PaletteItem::new(
            "打开设置…",
            "设置",
            PaletteAction::OpenSettings,
        ));
        items.push(PaletteItem::new(
            "导入会话…",
            "会话",
            PaletteAction::ImportSessions,
        ));
        items.push(PaletteItem::new(
            "导出会话…",
            "会话",
            PaletteAction::ExportSessions,
        ));
        items.push(PaletteItem::new(
            "导出诊断包…",
            "诊断",
            PaletteAction::ExportDiagnostics,
        ));

        self.palette = Some(PaletteState::new(items));
        self.context_menu = None;

        // 查询框用 TextInput：支持中文 IME 与光标编辑
        let input = cx.new(|cx| TextInput::new(cx).with_placeholder("输入以搜索会话或命令…"));
        cx.subscribe(
            &input,
            |this: &mut Self, input: Entity<TextInput>, _ev: &TextInputEvent, cx| {
                if let Some(palette) = this.palette.as_mut() {
                    palette.set_query(input.read(cx).content.to_string());
                }
                cx.notify();
            },
        )
        .detach();
        self.palette_input = Some(input.clone());
        window.focus(&input.read(cx).focus_handle(cx), cx);
        cx.notify();
    }

    fn close_command_palette(&mut self, cx: &mut Context<Self>) {
        if self.palette.take().is_some() {
            self.palette_input = None;
            self.focus_pending = true;
            cx.notify();
        }
    }

    fn palette_move(&mut self, delta: isize, cx: &mut Context<Self>) {
        if let Some(palette) = self.palette.as_mut() {
            palette.move_selection(delta);
            cx.notify();
        }
    }

    fn palette_confirm(&mut self, cx: &mut Context<Self>) {
        let Some(action) = self
            .palette
            .as_ref()
            .and_then(|palette| palette.selected_item().map(|item| item.action.clone()))
        else {
            self.close_command_palette(cx);
            return;
        };
        self.close_command_palette(cx);
        match action {
            PaletteAction::OpenTerminal(session_id) => self.request_terminal_tab(session_id, cx),
            PaletteAction::OpenSftp(session_id) => self.request_sftp_tab(session_id, cx),
            PaletteAction::OpenMonitor(session_id) => self.request_monitor_tab(session_id, cx),
            PaletteAction::NewSession => self.open_edit_dialog(None, cx),
            PaletteAction::OpenSettings => self.open_settings(cx),
            PaletteAction::ImportSessions => self.open_import_export(false, cx),
            PaletteAction::ExportSessions => self.open_import_export(true, cx),
            PaletteAction::ExportDiagnostics => self.export_diagnostics(cx),
        }
    }

    /// 生成诊断包到数据目录，并在状态栏提示路径。
    fn export_diagnostics(&mut self, cx: &mut Context<Self>) {
        let sessions_count = {
            let state = cx.global::<AppState>();
            let store = lock_or_recover(&state.session_store);
            store.list().len()
        };
        let settings = cx.global::<AppState>().settings.clone();
        let dest = crate::diagnostics::default_export_path();
        match crate::diagnostics::export_diagnostics(&dest, sessions_count, &settings) {
            Ok(path) => self.set_status(format!("诊断包已导出: {}", path.display()), cx),
            Err(error) => self.set_status(format!("导出诊断包失败: {error:#}"), cx),
        }
    }

    /// 面板内文本输入由 TextInput 实体负责（含 IME）；这里只保留导航动作。
    fn render_command_palette(&self, cx: &mut Context<Self>) -> AnyElement {
        let t = theme(cx);
        let Some(palette) = self.palette.as_ref() else {
            return div().into_any_element();
        };
        let visible = palette.visible();

        let mut list = div().w_full().flex().flex_col();
        if visible.is_empty() {
            list = list.child(
                div()
                    .px_3()
                    .py_2()
                    .text_size(px(12.))
                    .text_color(t.text_subtle())
                    .child("没有匹配的命令"),
            );
        }
        for (index, item) in visible.iter().enumerate() {
            let selected = index == palette.selected;
            let row_bg = if selected {
                t.secondary_btn_hover()
            } else {
                t.card_bg()
            };
            list = list.child(
                div()
                    .id(("rterm-palette-row", index))
                    .w_full()
                    .px_3()
                    .py_2()
                    .flex()
                    .items_center()
                    .gap_2()
                    .bg(row_bg)
                    .cursor(CursorStyle::PointingHand)
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _: &MouseDownEvent, _window, cx| {
                            if let Some(palette) = this.palette.as_mut() {
                                palette.selected = index;
                            }
                            this.palette_confirm(cx);
                        }),
                    )
                    .child(
                        div()
                            .flex_1()
                            .text_size(px(12.))
                            .text_color(t.text_on_card_bg())
                            .child(item.label.clone()),
                    )
                    .child(
                        div()
                            .text_size(px(10.))
                            .text_color(t.text_subtle())
                            .child(item.hint.clone()),
                    ),
            );
        }

        let list = list
            .id("rterm-palette-list")
            .flex_1()
            .min_h(px(0.))
            .overflow_y_scroll()
            .track_scroll(&self.palette_scroll);
        // 选中项始终保持在可视区内（↑/↓ 移动超出视口时自动滚动）
        self.palette_scroll.scroll_to_item(palette.selected);

        let query_row: AnyElement = match &self.palette_input {
            Some(input) => div()
                .w_full()
                .px_2()
                .py_1()
                .border_b_1()
                .border_color(t.border_subtle())
                .child(input.clone())
                .into_any_element(),
            None => div()
                .w_full()
                .px_3()
                .py_2()
                .border_b_1()
                .border_color(t.border_subtle())
                .text_size(px(13.))
                .text_color(t.text_placeholder())
                .child("输入以搜索会话或命令…")
                .into_any_element(),
        };

        div()
            .absolute()
            .inset_0()
            .bg(t.modal_overlay())
            .flex()
            .justify_center()
            .pt(px(80.))
            .key_context("CommandPalette")
            .on_action(cx.listener(|this, _: &PaletteUp, _window, cx| this.palette_move(-1, cx)))
            .on_action(cx.listener(|this, _: &PaletteDown, _window, cx| this.palette_move(1, cx)))
            .on_action(
                cx.listener(|this, _: &PaletteConfirm, _window, cx| this.palette_confirm(cx)),
            )
            .on_action(
                cx.listener(|this, _: &PaletteClose, _window, cx| this.close_command_palette(cx)),
            )
            .child(
                div()
                    .w(px(520.))
                    .max_h(px(420.))
                    .overflow_hidden()
                    .bg(t.card_bg())
                    .border_1()
                    .border_color(t.border_medium())
                    .rounded_lg()
                    .flex()
                    .flex_col()
                    .child(query_row)
                    .child(list),
            )
            .into_any_element()
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
        self.set_status(
            if saved {
                "设置已保存"
            } else {
                "设置未保存"
            },
            cx,
        );
        cx.notify();
    }

    fn open_import_export(&mut self, export: bool, cx: &mut Context<Self>) {
        let mode = if export {
            IoMode::Export
        } else {
            IoMode::Import
        };
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

    fn edit_session(&mut self, session_id: Uuid, cx: &mut Context<Self>) {
        let session = {
            let state = cx.global::<AppState>();
            lock_or_recover(&state.session_store)
                .get(session_id)
                .cloned()
        };
        if let Some(s) = session {
            self.open_edit_dialog(Some(&s), cx);
        }
    }

    fn delete_session(&mut self, session_id: Uuid, cx: &mut Context<Self>) {
        let name = {
            let state = cx.global::<AppState>();
            lock_or_recover(&state.session_store)
                .get(session_id)
                .map(|session| session.display_label())
                .unwrap_or_else(|| "会话".to_string())
        };
        {
            let state = cx.global_mut::<AppState>();
            lock_or_recover(&state.session_store).delete(session_id);
        }

        // 删除后立即写盘，失败只记日志（不阻断 UI）。
        let session_store = cx.global::<AppState>().session_store.clone();
        if let Err(e) = crate::state::persist_sessions(&session_store) {
            log::error!("删除会话后保存失败: {e:#}");
        }

        // 该会话已打开的终端 / SFTP / 监控 Tab 一并关闭，避免“删除后仍在运行”
        let tab_ids: Vec<Uuid> = self
            .tabs
            .iter()
            .filter(|tab| tab.session_id == session_id)
            .map(|tab| tab.id)
            .collect();
        for tab_id in tab_ids {
            self.close_tab(tab_id, cx);
        }

        self.context_menu = None;
        let host = cx.global::<AppState>().event_bus_host.clone();
        cx.update_entity(&host, |_host, cx: &mut Context<EventBusHost>| {
            cx.emit(AppEvent::SessionDeleted { session_id });
        });
        self.set_status(format!("已删除会话「{name}」"), cx);
        log::info!("Session deleted: {}", session_id);
        cx.notify();
    }

    fn set_status(&mut self, msg: impl Into<String>, cx: &mut Context<Self>) {
        let msg = msg.into();
        self.status_bar.update(cx, |bar, cx| {
            bar.message = msg;
            bar.clear_expiry();
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

        // 菜单 / 删除确认打开时把焦点交给根节点，Esc 才能生效
        if self.root_focus_pending {
            window.focus(&self.root_focus, cx);
            self.root_focus_pending = false;
        }
        // 菜单 / 确认框已关闭：焦点还给终端
        if self.overlay_focus_active && self.context_menu.is_none() && self.pending_delete.is_none()
        {
            self.overlay_focus_active = false;
            self.focus_pending = true;
        }

        // 菜单栏关闭后把焦点还给终端
        let menu_open = self.menu_bar.read(cx).open_kind().is_some();
        if self.menu_was_open && !menu_open {
            self.focus_pending = true;
        }
        self.menu_was_open = menu_open;

        let settings_hover = t.secondary_btn_hover();
        let mut root = div()
            .size_full()
            .bg(t.app_bg())
            .relative()
            .track_focus(&self.root_focus)
            .on_key_down(
                cx.listener(|this: &mut Self, event: &KeyDownEvent, _w, cx| {
                    if event.keystroke.key != "escape" {
                        return;
                    }
                    let mut closed = false;
                    if this.pending_delete.is_some() {
                        this.pending_delete = None;
                        closed = true;
                    } else if this.context_menu.is_some() {
                        this.context_menu = None;
                        closed = true;
                    } else if this.info_dialog.is_some() {
                        this.info_dialog = None;
                        closed = true;
                    } else if this.icon_dialog_session.is_some() {
                        this.icon_dialog_session = None;
                        closed = true;
                    }
                    if closed {
                        this.overlay_focus_active = false;
                        this.focus_pending = true;
                        cx.notify();
                    }
                }),
            )
            .flex()
            .flex_col()
            // 顶部菜单栏
            .child(self.menu_bar.clone())
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

        // 启动加载出错：作为正常布局行显示在工具条下方，不再覆盖 TabBar
        if let Some(err) = self.startup_error.clone() {
            root = root.child(
                div()
                    .w_full()
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
                            .on_click(cx.listener(|this: &mut Self, _ev: &ClickEvent, _w, cx| {
                                this.startup_error = None;
                                cx.notify();
                            })),
                    ),
            );
        }

        let mut main_row = div().w_full().flex_1().flex();
        if self.sidebar_visible {
            main_row = main_row.child(
                div()
                    .w(px(SIDEBAR_W))
                    .h_full()
                    .bg(t.sidebar_bg())
                    .border_r_1()
                    .border_color(t.border_subtle())
                    .child(self.session_tree.clone()),
            );
        }
        main_row = main_row.child(
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
        );
        root = root
            .child(main_row)
            // 底部 StatusBar
            .child(self.status_bar.clone());

        if let Some(menu_state) = self.context_menu.clone() {
            let viewport = window.viewport_size();
            root = root.child(context_menu::render_context_menu(
                &menu_state,
                f32::from(viewport.width),
                f32::from(viewport.height),
                cx,
                |this, action, cx| this.handle_context_action(action, cx),
                |this, cx| {
                    this.context_menu = None;
                    this.overlay_focus_active = false;
                    this.focus_pending = true;
                    cx.notify();
                },
            ));
        }

        // 顶部菜单栏下拉（会话 / 连接 / 视图 / 帮助）
        if let Some(kind) = self.menu_bar.read(cx).open_kind() {
            let entries =
                menu_bar::entries(kind, self.selected_session.is_some(), self.sidebar_visible);
            let viewport = window.viewport_size();
            let width = context_menu::CONTEXT_MENU_W;
            let height = entries
                .iter()
                .map(|entry| {
                    context_menu::MENU_ITEM_H
                        + if entry.divider_before {
                            context_menu::MENU_DIVIDER_H
                        } else {
                            0.0
                        }
                })
                .sum::<f32>()
                + context_menu::MENU_PADDING_Y;
            let (x, y) = context_menu::clamp_position(
                menu_bar::menu_anchor_x(kind),
                menu_bar::MENU_BAR_H,
                width,
                height,
                f32::from(viewport.width),
                f32::from(viewport.height),
            );

            let mut menu = div()
                .absolute()
                .left(px(x))
                .top(px(y))
                .w(px(width))
                .py_1()
                .bg(t.card_bg())
                .border_1()
                .border_color(t.border_medium())
                .rounded_md()
                .flex()
                .flex_col()
                .id("rterm-menu-dropdown")
                .on_mouse_down_out(
                    cx.listener(|this: &mut Self, _ev: &MouseDownEvent, _w, cx| {
                        this.menu_bar.update(cx, |bar, cx| bar.close(cx));
                        this.focus_pending = true;
                        cx.notify();
                    }),
                );

            for (index, entry) in entries.into_iter().enumerate() {
                if entry.divider_before {
                    menu = menu.child(context_menu::menu_divider(t.border_subtle()));
                }
                let command = entry.command;
                menu = menu.child(context_menu::menu_item(
                    &format!("rterm-menu-cmd-{index}"),
                    entry.label,
                    entry.enabled,
                    false,
                    cx,
                    move |this, cx| {
                        this.menu_bar.update(cx, |bar, cx| bar.close(cx));
                        this.focus_pending = true;
                        this.emit_event(cx, AppEvent::MenuCommand(command));
                    },
                ));
            }
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
        if let Some(dialog) = &self.folder_prompt {
            root = root.child(dialog.clone());
        }
        if let Some(dialog) = &self.group_dialog {
            root = root.child(dialog.clone());
        }

        // 更改会话图标
        if let Some(session_id) = self.icon_dialog_session {
            let current = {
                let store = lock_or_recover(&cx.global::<AppState>().session_store);
                store
                    .get(session_id)
                    .map(|session| session.icon_or_default().to_string())
                    .unwrap_or_else(|| rterm_session::DEFAULT_SESSION_ICON.to_string())
            };
            root = root.child(
                div()
                    .absolute()
                    .inset_0()
                    .bg(t.modal_overlay())
                    .flex()
                    .items_center()
                    .justify_center()
                    .on_mouse_down_out(cx.listener(
                        |this: &mut Self, _ev: &MouseDownEvent, _w, cx| {
                            this.icon_dialog_session = None;
                            this.focus_pending = true;
                            cx.notify();
                        },
                    ))
                    .child(
                        div()
                            .w(px(360.))
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
                                    .text_size(px(13.))
                                    .text_color(t.text_on_card_bg())
                                    .flex()
                                    .justify_center()
                                    .child("更改会话图标"),
                            )
                            .child(icon_picker::render_icon_grid(
                                "icon-dialog",
                                &current,
                                cx,
                                move |this, icon, cx| this.apply_session_icon(session_id, icon, cx),
                            ))
                            .child(
                                div().w_full().flex().justify_center().child(
                                    div()
                                        .id("rterm-icon-dialog-close")
                                        .h(px(26.))
                                        .px_4()
                                        .rounded_md()
                                        .bg(t.secondary_btn_bg())
                                        .text_size(px(11.))
                                        .text_color(t.text_on_secondary_btn())
                                        .flex()
                                        .items_center()
                                        .cursor_pointer()
                                        .hover(|style| style.bg(t.secondary_btn_hover()))
                                        .on_click(cx.listener(
                                            |this: &mut Self, _ev: &ClickEvent, _w, cx| {
                                                this.icon_dialog_session = None;
                                                this.focus_pending = true;
                                                cx.notify();
                                            },
                                        ))
                                        .child("关闭"),
                                ),
                            ),
                    ),
            );
        }

        // 快捷键 / 关于
        if let Some((title, body)) = self.info_dialog.clone() {
            root = root.child(
                div()
                    .absolute()
                    .inset_0()
                    .bg(t.modal_overlay())
                    .flex()
                    .items_center()
                    .justify_center()
                    .on_mouse_down_out(cx.listener(
                        |this: &mut Self, _ev: &MouseDownEvent, _w, cx| {
                            this.info_dialog = None;
                            this.focus_pending = true;
                            cx.notify();
                        },
                    ))
                    .child(
                        div()
                            .w(px(460.))
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
                                    .child(title),
                            )
                            .child(
                                div()
                                    .w_full()
                                    .p_4()
                                    .text_size(px(11.))
                                    .text_color(t.text_on_card_bg())
                                    .child(body),
                            )
                            .child(
                                div()
                                    .w_full()
                                    .h(px(46.))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .border_t_1()
                                    .border_color(t.border_subtle())
                                    .child(
                                        div()
                                            .id("rterm-info-dialog-close")
                                            .h(px(26.))
                                            .px_4()
                                            .rounded_md()
                                            .bg(t.secondary_btn_bg())
                                            .text_size(px(11.))
                                            .text_color(t.text_on_secondary_btn())
                                            .flex()
                                            .items_center()
                                            .cursor_pointer()
                                            .hover(|style| style.bg(t.secondary_btn_hover()))
                                            .on_click(cx.listener(
                                                |this: &mut Self, _ev: &ClickEvent, _w, cx| {
                                                    this.info_dialog = None;
                                                    this.focus_pending = true;
                                                    cx.notify();
                                                },
                                            ))
                                            .child("关闭"),
                                    ),
                            ),
                    ),
            );
        }

        // Ctrl+P 命令面板（最上层）。
        if self.palette.is_some() {
            root = root.child(self.render_command_palette(cx));
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
                                    .child("配置不可恢复，相关标签会一并关闭"),
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
            return tab.content.element();
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
}

/// 「帮助 → 快捷键」弹窗内容。
fn shortcut_text() -> String {
    [
        "Ctrl+T          新建标签",
        "Ctrl+Tab        切换标签",
        "Ctrl+W          关闭标签",
        "Ctrl+Shift+N    新建会话",
        "Ctrl+Shift+S    开关 SFTP 侧栏",
        "Ctrl+Shift+L    开关会话日志",
        "Ctrl+P          命令面板",
        "F10             顶部菜单栏",
        "F11             全屏",
        "Ctrl+,          设置",
        "Ctrl+= / Ctrl+- 调整字号",
    ]
    .join("\n")
}

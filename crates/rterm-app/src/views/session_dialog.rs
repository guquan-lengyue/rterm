//! SessionEditDialog — 会话编辑 Modal
//!
//! 所有颜色通过 `crate::ui::theme(cx)` 取主题，不硬编码 rgb。
//! 所有文字居中。
//!
//! 密码/口令字段用 `EncryptedString::new(&plain)` 创建 version=0 明文，
//! 主程序退出时 crypto_bridge 统一 DPAPI 加密。

use gpui::prelude::*;
use gpui::{div, px, Entity, FocusHandle, Focusable, IntoElement, Render, ScrollHandle, Window};
use uuid::Uuid;

use crate::event::{AppEvent, EventBusHost};
use crate::state::{lock_or_recover, AppState};
use crate::ui::theme;

use super::group_picker::{GroupPicker, GroupPickerEvent};
use super::icon_picker;
use super::text_input::TextInput;

// ---------------------------------------------------------------------------
// Tab 枚举
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DialogTab {
    Basic,
    Auth,
    Advanced,
}

impl DialogTab {
    fn label(&self) -> &'static str {
        match self {
            DialogTab::Basic => "基础",
            DialogTab::Auth => "认证",
            DialogTab::Advanced => "高级",
        }
    }
}

// ---------------------------------------------------------------------------
// Auth type
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AuthType {
    Password,
    PrivateKey,
    Agent,
}

// ---------------------------------------------------------------------------
// 测试连接状态
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub enum TestStatus {
    Testing,
    Ok(String),
    Failed(String),
}

/// 密码框在编辑模式下的掩码占位内容。
/// 显示为 8 个 ●，保存时若内容等于该串则保留原密码，不覆盖。
const PASSWORD_SENTINEL: &str = "********";

// ---------------------------------------------------------------------------
// SessionEditDialog View
// ---------------------------------------------------------------------------

pub struct SessionEditDialog {
    pub editing_id: Option<Uuid>,
    pub active_tab: DialogTab,

    focus_handle: FocusHandle,
    /// 首次渲染时把焦点交给弹窗根节点（Esc 关闭依赖键盘事件冒泡）
    autofocus_pending: bool,

    // Basic
    pub name_input: Entity<TextInput>,
    pub host_input: Entity<TextInput>,
    pub port_input: Entity<TextInput>,
    pub username_input: Entity<TextInput>,
    /// 分组选择：可搜索下拉 + 内联新建（替代自由文本，避免错名）。
    pub group_picker: Entity<GroupPicker>,

    /// 会话图标（None = 默认图标）；预设选择见 `icon_picker`。
    pub icon: Option<String>,
    icon_open: bool,

    /// 跳板代理：从已有会话中选择的 id（None = 直连）。
    pub proxy_choice: Option<Uuid>,
    /// 编辑时的既有代理配置（可能来自 OpenSSH 导入，不在会话库中）。
    original_proxy: Option<Box<rterm_session::Session>>,
    /// 用户未改动代理时保留 `original_proxy`。
    proxy_custom_kept: bool,
    proxy_open: bool,

    // Auth
    pub auth_type: AuthType,
    pub password_input: Entity<TextInput>,
    pub key_path_input: Entity<TextInput>,
    pub passphrase_input: Entity<TextInput>,

    // Advanced
    pub keepalive_input: Entity<TextInput>,
    pub timeout_input: Entity<TextInput>,
    pub compress_enabled: bool,

    // 测试连接结果
    pub test_status: Option<TestStatus>,

    // 表单校验错误（保存时展示在 footer）
    pub form_error: Option<String>,

    /// 表单滚动状态：窗口变矮时可滚动，切换 Tab 复位
    form_scroll: ScrollHandle,
}

impl SessionEditDialog {
    fn make_ti(
        cx: &mut Context<Self>,
        content: &str,
        placeholder: &str,
        password: bool,
    ) -> Entity<TextInput> {
        let mut ti = TextInput::new(cx)
            .with_content(content)
            .with_placeholder(placeholder);
        if password {
            ti = ti.password();
        }
        cx.new(|_cx| ti)
    }

    pub fn new(cx: &mut Context<Self>) -> Self {
        Self::from_session(cx, None)
    }

    pub fn from_session(cx: &mut Context<Self>, session: Option<&rterm_session::Session>) -> Self {
        let editing_id = session.map(|s| s.id);

        // 新建会话时用设置里的 SSH 默认值（编辑已有会话保持原值）
        let settings = cx.global::<AppState>().settings.clone();

        let (name, host, port, username, folder) = match session {
            Some(s) => (
                s.name.clone(),
                s.host.clone(),
                s.port.to_string(),
                s.username.clone(),
                s.folder.clone().unwrap_or_default(),
            ),
            None => (
                String::new(),
                String::new(),
                settings.default_port.to_string(),
                settings.default_username.clone().unwrap_or_default(),
                String::new(),
            ),
        };

        let (auth_type, password, key_path, passphrase) = match session {
            Some(s) => match &s.auth {
                rterm_session::AuthMethod::Password { password } => {
                    let pw = if password.is_empty() {
                        String::new()
                    } else {
                        PASSWORD_SENTINEL.to_string()
                    };
                    (AuthType::Password, pw, String::new(), String::new())
                }
                rterm_session::AuthMethod::PrivateKey { path, passphrase } => {
                    // 已有口令 → 显示掩码占位；无口令则保持空
                    let pp = match passphrase {
                        Some(p) if !p.is_empty() => PASSWORD_SENTINEL.to_string(),
                        _ => String::new(),
                    };
                    (
                        AuthType::PrivateKey,
                        String::new(),
                        path.to_string_lossy().into(),
                        pp,
                    )
                }
                rterm_session::AuthMethod::Agent { .. } => {
                    (AuthType::Agent, String::new(), String::new(), String::new())
                }
            },
            None => (
                AuthType::Password,
                String::new(),
                String::new(),
                String::new(),
            ),
        };

        let (keepalive, timeout, compress) = match session {
            Some(s) => (
                s.keepalive_secs.to_string(),
                s.connect_timeout_secs.to_string(),
                s.compress,
            ),
            None => (
                settings.keepalive_secs.to_string(),
                settings.connect_timeout_secs.to_string(),
                true,
            ),
        };

        // 代理：既有配置若来自会话库则转成「选择已有会话」，否则原样保留。
        let (icon, original_proxy, proxy_choice, proxy_custom_kept) = match session {
            Some(s) => {
                let original = s.proxy_jump.clone();
                let in_store = original
                    .as_ref()
                    .map(|proxy| {
                        lock_or_recover(&cx.global::<AppState>().session_store)
                            .get(proxy.id)
                            .is_some()
                    })
                    .unwrap_or(false);
                let choice = if in_store {
                    original.as_ref().map(|proxy| proxy.id)
                } else {
                    None
                };
                (
                    s.icon.clone(),
                    original.clone(),
                    choice,
                    original.is_some() && !in_store,
                )
            }
            None => (None, None, None, false),
        };

        let initial_folder = if folder.trim().is_empty() {
            None
        } else {
            Some(folder)
        };
        let group_picker = cx.new(|cx| GroupPicker::new(cx, initial_folder));
        cx.subscribe(
            &group_picker,
            |this: &mut Self, _picker, event: &GroupPickerEvent, cx| {
                let GroupPickerEvent::CreateFolder(path) = event;
                this.create_folder_from_picker(path.clone(), cx);
            },
        )
        .detach();

        Self {
            editing_id,
            active_tab: DialogTab::Basic,
            focus_handle: cx.focus_handle(),
            autofocus_pending: true,
            name_input: Self::make_ti(cx, &name, "会话名称", false),
            host_input: Self::make_ti(cx, &host, "主机地址", false),
            port_input: Self::make_ti(cx, &port, "22", false),
            username_input: Self::make_ti(cx, &username, "用户名", false),
            group_picker,
            icon,
            icon_open: false,
            proxy_choice,
            original_proxy,
            proxy_custom_kept,
            proxy_open: false,
            auth_type,
            password_input: Self::make_ti(cx, &password, "密码", true),
            key_path_input: Self::make_ti(cx, &key_path, "密钥文件路径", false),
            passphrase_input: Self::make_ti(cx, &passphrase, "密钥口令", true),
            keepalive_input: Self::make_ti(cx, &keepalive, "30", false),
            timeout_input: Self::make_ti(cx, &timeout, "10", false),
            compress_enabled: compress,
            test_status: None,
            form_error: None,
            form_scroll: ScrollHandle::new(),
        }
    }

    /// GroupPicker 内联新建：落库 + 持久化 + 回填选择。
    fn create_folder_from_picker(&mut self, path: String, cx: &mut Context<Self>) {
        let result = {
            let state = cx.global_mut::<AppState>();
            let mut store = lock_or_recover(&state.session_store);
            store.create_folder(&path)
        };
        match result {
            Ok(()) => {
                let store = cx.global::<AppState>().session_store.clone();
                if let Err(error) = crate::state::persist_sessions(&store) {
                    log::error!("创建分组后保存失败: {error:#}");
                }
                let host = cx.global::<AppState>().event_bus_host.clone();
                cx.update_entity(&host, |_host, cx: &mut Context<EventBusHost>| {
                    cx.emit(AppEvent::SessionTreeFoldersChanged);
                });
                self.group_picker
                    .update(cx, |picker, cx| picker.set_selected(Some(path), cx));
            }
            Err(error) => {
                self.group_picker.update(cx, |picker, cx| {
                    picker.set_error(Some(format!("{error}")), cx)
                });
            }
        }
    }

    pub fn save(&mut self, cx: &mut Context<Self>) {
        // —— 表单校验：任一项失败都不 upsert / 落盘 ——
        let form = match validate_session_form(
            &self.name_input.read(cx).content,
            &self.host_input.read(cx).content,
            &self.port_input.read(cx).content,
            &self.keepalive_input.read(cx).content,
            &self.timeout_input.read(cx).content,
        ) {
            Ok(form) => form,
            Err(error) => {
                self.form_error = Some(error);
                cx.notify();
                return;
            }
        };
        let name = form.name;
        let host = form.host;
        let port = form.port;
        let keepalive_secs = form.keepalive_secs;
        let connect_timeout_secs = form.connect_timeout_secs;

        self.form_error = None;
        let username = self.username_input.read(cx).content.to_string();
        let folder = self.group_picker.read(cx).selected();

        let session_id = self.editing_id.unwrap_or_else(Uuid::new_v4);

        // 先取旧会话：保留 created_at / last_connected / 未修改的原密码
        let old = {
            let state = cx.global::<AppState>();
            let store = lock_or_recover(&state.session_store);
            store.get(session_id).cloned()
        };

        let auth = match self.auth_type {
            AuthType::Password => {
                let pw = self.password_input.read(cx).content.to_string();
                if pw == PASSWORD_SENTINEL {
                    // 密码框未被修改（仍是掩码占位）→ 沿用旧密码
                    match old.as_ref() {
                        Some(s)
                            if matches!(&s.auth, rterm_session::AuthMethod::Password { .. }) =>
                        {
                            s.auth.clone()
                        }
                        _ => rterm_session::AuthMethod::Password {
                            password: rterm_session::EncryptedString::new(""),
                        },
                    }
                } else {
                    rterm_session::AuthMethod::Password {
                        password: rterm_session::EncryptedString::new(&pw),
                    }
                }
            }
            AuthType::PrivateKey => {
                let path =
                    std::path::PathBuf::from(self.key_path_input.read(cx).content.to_string());
                let pp = self.passphrase_input.read(cx).content.to_string();
                let passphrase = if pp == PASSWORD_SENTINEL {
                    // 口令框未被修改（仍是掩码占位）→ 原样保留旧口令
                    match old.as_ref().map(|s| &s.auth) {
                        Some(rterm_session::AuthMethod::PrivateKey { passphrase, .. }) => {
                            passphrase.clone()
                        }
                        _ => None,
                    }
                } else if pp.is_empty() {
                    None
                } else {
                    Some(rterm_session::EncryptedString::new(&pp))
                };
                rterm_session::AuthMethod::PrivateKey { path, passphrase }
            }
            AuthType::Agent => rterm_session::AuthMethod::Agent {
                key_comment: String::new(),
            },
        };

        let (created_at, last_connected) = match &old {
            Some(o) => (o.created_at, o.last_connected),
            None => (rterm_session::default_now(), None),
        };

        // 代理：优先保留用户未改动的导入配置，其次使用选择的会话快照。
        let proxy_jump = if self.proxy_custom_kept {
            self.original_proxy.clone()
        } else if let Some(proxy_id) = self.proxy_choice {
            let candidate = {
                let state = cx.global::<AppState>();
                let store = lock_or_recover(&state.session_store);
                store.get(proxy_id).cloned()
            };
            match candidate {
                Some(candidate) => {
                    if let Err(error) =
                        rterm_session::proxy::validate_selection(session_id, &candidate)
                    {
                        self.form_error = Some(format!("跳板代理无效: {error}"));
                        cx.notify();
                        return;
                    }
                    Some(Box::new(candidate))
                }
                None => {
                    self.form_error = Some("所选跳板会话已被删除，请重新选择".into());
                    cx.notify();
                    return;
                }
            }
        } else {
            None
        };

        let mut session = rterm_session::Session {
            id: session_id,
            name,
            icon: self.icon.clone(),
            host,
            port,
            username,
            auth,
            proxy_jump,
            keepalive_secs,
            connect_timeout_secs,
            folder,
            compress: self.compress_enabled,
            created_at,
            last_modified: rterm_session::default_now(),
            last_connected,
            ..rterm_session::Session::default()
        };
        // 编辑时保留本次表单未覆盖的字段（端口转发 / 标签 / 主题覆盖）。
        if let Some(old) = &old {
            session.port_forwards = old.port_forwards.clone();
            session.tags = old.tags.clone();
            session.theme_override = old.theme_override.clone();
        }

        if let Err(error) = session.validate() {
            self.form_error = Some(error);
            cx.notify();
            return;
        }

        {
            let state = cx.global_mut::<AppState>();
            let mut store = lock_or_recover(&state.session_store);
            if let Some(folder) = session.folder.as_deref() {
                // 自由输入的新分组自动登记为显式分组，重启后仍可见。
                let _ = store.create_folder(folder);
            }
            store.upsert(session);
        }

        // 写盘即时持久化：失败只记日志，不阻断后续 UI 流程。
        let session_store = cx.global::<AppState>().session_store.clone();
        if let Err(e) = crate::state::persist_sessions(&session_store) {
            log::error!("会话保存到磁盘失败: {e:#}");
        }

        let host = cx.global::<AppState>().event_bus_host.clone();
        let is_new = self.editing_id.is_none();
        cx.update_entity(&host, |_host, cx: &mut Context<EventBusHost>| {
            let ev = if is_new {
                AppEvent::SessionCreated { session_id }
            } else {
                AppEvent::SessionUpdated { session_id }
            };
            cx.emit(ev);
            cx.emit(AppEvent::SessionTreeOpenEditDialog { session_id: None });
        });

        log::info!(
            "SessionEditDialog saved session {} (new={})",
            session_id,
            is_new
        );
    }

    pub fn cancel(&mut self, cx: &mut Context<Self>) {
        let host = cx.global::<AppState>().event_bus_host.clone();
        cx.update_entity(&host, |_host, cx: &mut Context<EventBusHost>| {
            cx.emit(AppEvent::SessionTreeOpenEditDialog { session_id: None });
        });
    }

    /// TCP 连通性测试（5s 超时，后台线程执行，不阻塞 UI）
    fn test_connection(&mut self, cx: &mut Context<Self>) {
        let host = self.host_input.read(cx).content.trim().to_string();
        let port: u16 = match self.port_input.read(cx).content.trim().parse::<u16>() {
            Ok(p) if p >= 1 => p,
            _ => {
                self.test_status = Some(TestStatus::Failed("端口必须是 1–65535 的数字".into()));
                cx.notify();
                return;
            }
        };

        if host.is_empty() {
            self.test_status = Some(TestStatus::Failed("请先填写主机地址".into()));
            cx.notify();
            return;
        }

        self.test_status = Some(TestStatus::Testing);
        cx.notify();

        let executor = cx.background_executor().clone();
        cx.spawn(async move |this, cx| {
            let result = executor.spawn(async move { tcp_check(&host, port) }).await;
            this.update(cx, |this, cx| {
                this.test_status = Some(result);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }
}

/// 校验并规整后的会话表单字段。
#[derive(Debug, PartialEq, Eq)]
struct ValidatedSessionForm {
    name: String,
    host: String,
    port: u16,
    keepalive_secs: u32,
    connect_timeout_secs: u32,
}

/// 纯函数表单校验：成功返回规整后的字段，失败返回展示用错误文案。
///
/// 端口必须是 1–65535；KeepAlive / 连接超时留空时用默认值（30 / 10），
/// 非空则必须是正整数。
fn validate_session_form(
    name: &str,
    host: &str,
    port: &str,
    keepalive: &str,
    timeout: &str,
) -> Result<ValidatedSessionForm, String> {
    let name = name.trim().to_string();
    if name.is_empty() {
        return Err("会话名称不能为空".into());
    }

    let host = host.trim().to_string();
    if host.is_empty() {
        return Err("主机地址不能为空".into());
    }

    let port = match port.trim().parse::<u16>() {
        Ok(port) if port >= 1 => port,
        _ => return Err("端口必须是 1–65535 的数字".into()),
    };

    let keepalive_secs = parse_positive_or_default(keepalive, 30, "KeepAlive 必须是正整数秒")?;
    let connect_timeout_secs = parse_positive_or_default(timeout, 10, "连接超时必须正整数秒")?;

    Ok(ValidatedSessionForm {
        name,
        host,
        port,
        keepalive_secs,
        connect_timeout_secs,
    })
}

fn parse_positive_or_default(value: &str, default: u32, error: &str) -> Result<u32, String> {
    let value = value.trim();
    if value.is_empty() {
        return Ok(default);
    }
    match value.parse::<u32>() {
        Ok(parsed) if parsed > 0 => Ok(parsed),
        _ => Err(error.to_string()),
    }
}

/// 阻塞式 TCP 连接检查，返回结果状态。
fn tcp_check(host: &str, port: u16) -> TestStatus {
    use std::net::{TcpStream, ToSocketAddrs};
    use std::time::Duration;

    let addr = match (host, port).to_socket_addrs() {
        Ok(mut iter) => match iter.next() {
            Some(a) => a,
            None => return TestStatus::Failed(format!("无法解析主机 {host}")),
        },
        Err(e) => return TestStatus::Failed(format!("解析主机失败: {e}")),
    };

    match TcpStream::connect_timeout(&addr, Duration::from_secs(5)) {
        Ok(_) => TestStatus::Ok(format!("TCP 可达（未验证登录）— {addr}")),
        Err(e) => TestStatus::Failed(rterm_ssh::errors::describe(&anyhow::Error::new(e))),
    }
}

impl Focusable for SessionEditDialog {
    fn focus_handle(&self, _cx: &gpui::App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

// ---------------------------------------------------------------------------
// Render
// ---------------------------------------------------------------------------

impl Render for SessionEditDialog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        if self.autofocus_pending {
            window.focus(&self.focus_handle, cx);
            self.autofocus_pending = false;
        }
        let title = if self.editing_id.is_some() {
            "编辑会话"
        } else {
            "新建会话"
        };
        // 自适应尺寸：默认 520×470，窗口更小时收缩，内容放不下时滚动
        let (card_w, card_h) = super::adaptive_dialog_size(window, 520.0, 470.0, 420.0, 300.0);

        div()
            .size_full()
            .absolute()
            .inset_0()
            .bg(t.modal_overlay())
            .flex()
            .items_center()
            .justify_center()
            .track_focus(&self.focus_handle)
            .on_key_down(
                cx.listener(|this: &mut Self, event: &gpui::KeyDownEvent, _w, cx| {
                    if event.keystroke.key == "escape" {
                        this.cancel(cx);
                    }
                }),
            )
            .child(
                div()
                    .w(px(card_w))
                    // 高度随窗口收缩；字段超出时内容区滚动，footer 始终可见
                    .h(px(card_h))
                    .bg(t.card_bg())
                    .border_1()
                    .border_color(t.border_medium())
                    .rounded_lg()
                    .flex()
                    .flex_col()
                    .child(
                        // 标题栏
                        div()
                            .w_full()
                            .h(px(44.))
                            .flex_shrink_0()
                            .px_4()
                            .flex()
                            .items_center()
                            .bg(t.card_header_bg())
                            .border_b_1()
                            .border_color(t.border_subtle())
                            .text_color(t.text_on_card_header_bg())
                            .text_size(px(14.))
                            .child(title),
                    )
                    .child(
                        // Tab 栏
                        div()
                            .w_full()
                            .h(px(36.))
                            .flex_shrink_0()
                            .flex()
                            .border_b_1()
                            .border_color(t.border_subtle())
                            .child(self.render_tab_header(DialogTab::Basic, cx))
                            .child(self.render_tab_header(DialogTab::Auth, cx))
                            .child(self.render_tab_header(DialogTab::Advanced, cx)),
                    )
                    .child(
                        // 内容区占用剩余高度；窗口变矮时可滚动，避免字段盖住 footer
                        div()
                            .id("session-form")
                            .w_full()
                            .flex_1()
                            .min_h(px(0.))
                            .overflow_y_scroll()
                            .track_scroll(&self.form_scroll)
                            // 子项保持自然高度，内容超出时滚动距离 > 0
                            .child(
                                div()
                                    .w_full()
                                    .flex_shrink_0()
                                    .child(self.render_tab_content(cx)),
                            ),
                    )
                    .child(
                        // 底部按钮栏：左侧 测试连接 + 状态，右侧 取消/保存
                        div()
                            .w_full()
                            .h(px(52.))
                            .flex_shrink_0()
                            .px_4()
                            .flex()
                            .items_center()
                            .gap_3()
                            .border_t_1()
                            .border_color(t.border_subtle())
                            .child(self.render_test_button(cx))
                            .child(self.render_test_status(cx))
                            .child(self.render_form_error(cx))
                            .child(div().flex_1())
                            .child(self.render_button("取消", false, cx))
                            .child(self.render_button("保存", true, cx)),
                    ),
            )
    }
}

impl SessionEditDialog {
    fn render_tab_header(&self, tab: DialogTab, cx: &mut Context<Self>) -> gpui::AnyElement {
        let t = theme(cx);
        let is_active = self.active_tab == tab;
        let bg = if is_active {
            t.row_selected_bg()
        } else {
            t.card_header_bg()
        };
        let text_color = if is_active {
            t.text_on_row_selected_bg()
        } else {
            t.text_subtle()
        };
        let border_color = if is_active {
            t.primary()
        } else {
            t.border_subtle()
        };
        let label = tab.label().to_string();
        let id_str = format!("rterm-tab-{:?}", tab.label());

        div()
            .h_full()
            .px_4()
            .flex()
            .items_center()
            .justify_center()
            .bg(bg)
            .text_color(text_color)
            .text_size(px(12.))
            .cursor_pointer()
            .border_b_2()
            .border_color(border_color)
            .hover(|s| s.text_color(t.text_on_card_header_bg()))
            .id(id_str)
            .on_click(
                cx.listener(move |this: &mut Self, _ev: &gpui::ClickEvent, _w, cx| {
                    this.active_tab = tab;
                    this.form_scroll = ScrollHandle::new();
                    cx.notify();
                }),
            )
            .child(label)
            .into_any_element()
    }

    fn render_tab_content(&mut self, cx: &mut Context<Self>) -> gpui::AnyElement {
        match self.active_tab {
            DialogTab::Basic => self.render_basic_tab(cx),
            DialogTab::Auth => self.render_auth_tab(cx),
            DialogTab::Advanced => self.render_advanced_tab(cx),
        }
    }

    fn render_field(
        label: &str,
        input: Entity<TextInput>,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let t = theme(cx);
        let label_owned = label.to_string();
        div()
            .w_full()
            .mb_3()
            .flex()
            .flex_col()
            .gap_1()
            .child(
                div()
                    .text_size(px(11.))
                    .text_color(t.text_subtle())
                    .child(label_owned),
            )
            .child(input)
            .into_any_element()
    }

    fn render_basic_tab(&mut self, cx: &mut Context<Self>) -> gpui::AnyElement {
        div()
            .w_full()
            .flex_1()
            .px_4()
            .py_4()
            .child(Self::render_field(
                "会话名称 *",
                self.name_input.clone(),
                cx,
            ))
            .child(self.render_icon_field(cx))
            .child(Self::render_field(
                "主机地址 *",
                self.host_input.clone(),
                cx,
            ))
            .child(Self::render_field("端口", self.port_input.clone(), cx))
            .child(Self::render_field(
                "用户名",
                self.username_input.clone(),
                cx,
            ))
            .child(
                div()
                    .w_full()
                    .mb_3()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(theme(cx).text_subtle())
                            .child("分组"),
                    )
                    .child(self.group_picker.clone()),
            )
            .child(self.render_proxy_field(cx))
            .into_any_element()
    }

    /// 图标选择：当前图标 + 展开后的预设网格。
    fn render_icon_field(&mut self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let t = theme(cx);
        let icon = self
            .icon
            .as_deref()
            .filter(|icon| !icon.trim().is_empty())
            .unwrap_or(rterm_session::DEFAULT_SESSION_ICON)
            .to_string();
        let open = self.icon_open;

        let mut container =
            div()
                .w_full()
                .mb_3()
                .flex()
                .flex_col()
                .gap_1()
                .child(
                    div()
                        .text_size(px(11.))
                        .text_color(t.text_subtle())
                        .child("图标"),
                )
                .child(
                    div()
                        .id("rterm-session-icon-toggle")
                        .flex()
                        .items_center()
                        .gap_2()
                        .cursor_pointer()
                        .on_click(
                            cx.listener(|this: &mut Self, _ev: &gpui::ClickEvent, _w, cx| {
                                this.icon_open = !this.icon_open;
                                cx.notify();
                            }),
                        )
                        .child(
                            div()
                                .w(px(38.))
                                .h(px(30.))
                                .flex()
                                .items_center()
                                .justify_center()
                                .rounded_md()
                                .border_1()
                                .border_color(t.border_subtle())
                                .bg(t.input_bg())
                                .text_size(px(15.))
                                .child(icon.clone()),
                        )
                        .child(div().text_size(px(11.)).text_color(t.text_subtle()).child(
                            if open {
                                "收起图标列表"
                            } else {
                                "选择图标"
                            },
                        )),
                );

        if open {
            container = container.child(icon_picker::render_icon_grid(
                "session-dialog",
                &icon,
                cx,
                |this, icon, cx| {
                    this.icon = Some(icon.to_string());
                    cx.notify();
                },
            ));
        }
        container.into_any_element()
    }

    /// 代理选择：无 / 已有会话（排除自身与环路）；下方展示链式预览。
    fn render_proxy_field(&mut self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let t = theme(cx);
        let target_id = self.editing_id.unwrap_or_default();
        let open = self.proxy_open;

        let candidates: Vec<rterm_session::Session> = {
            let state = cx.global::<AppState>();
            let store = lock_or_recover(&state.session_store);
            store
                .list()
                .iter()
                .filter(|candidate| rterm_session::proxy::can_be_proxy_of(target_id, candidate))
                .cloned()
                .collect()
        };

        let selected_label = if self.proxy_custom_kept {
            match self.original_proxy.as_deref() {
                Some(proxy) => format!("{}（原配置）", proxy.display_label()),
                None => "无（直连）".to_string(),
            }
        } else if let Some(proxy_id) = self.proxy_choice {
            candidates
                .iter()
                .find(|candidate| candidate.id == proxy_id)
                .map(|candidate| candidate.display_label())
                .unwrap_or_else(|| "已删除的会话".to_string())
        } else {
            "无（直连）".to_string()
        };

        // 链式预览：候选链路 + 当前会话占位名。
        let preview_session = rterm_session::Session {
            id: target_id,
            name: self.name_input.read(cx).content.to_string(),
            host: self.host_input.read(cx).content.to_string(),
            proxy_jump: if self.proxy_custom_kept {
                self.original_proxy.clone()
            } else {
                self.proxy_choice.and_then(|id| {
                    candidates
                        .iter()
                        .find(|candidate| candidate.id == id)
                        .cloned()
                        .map(Box::new)
                })
            },
            ..rterm_session::Session::default()
        };
        let chain_text = rterm_session::proxy::describe_chain(&preview_session);
        let chain_invalid = chain_text.starts_with("代理链无效");

        let mut container = div()
            .w_full()
            .flex()
            .flex_col()
            .gap_1()
            .child(
                div()
                    .text_size(px(11.))
                    .text_color(t.text_subtle())
                    .child("代理（跳板机）"),
            )
            .child(
                div()
                    .id("rterm-session-proxy-toggle")
                    .w_full()
                    .h(px(32.))
                    .px_2()
                    .flex()
                    .items_center()
                    .justify_between()
                    .rounded_md()
                    .border_1()
                    .border_color(if open {
                        t.border_input_focused()
                    } else {
                        t.border_input_idle()
                    })
                    .bg(t.input_bg())
                    .text_size(px(12.))
                    .text_color(t.text_on_card_bg())
                    .cursor_pointer()
                    .on_click(
                        cx.listener(|this: &mut Self, _ev: &gpui::ClickEvent, _w, cx| {
                            this.proxy_open = !this.proxy_open;
                            cx.notify();
                        }),
                    )
                    .child(div().truncate().child(selected_label))
                    .child(
                        div()
                            .text_size(px(10.))
                            .text_color(t.text_subtle())
                            .child(if open { "▴" } else { "▾" }),
                    ),
            );

        if open {
            let mut list = div()
                .w_full()
                .flex()
                .flex_col()
                .rounded_md()
                .border_1()
                .border_color(t.border_medium())
                .bg(t.card_bg());

            // 「无（直连）」
            list = list.child(self.render_proxy_option(
                "rterm-proxy-none",
                "无（直连）".to_string(),
                !self.proxy_custom_kept && self.proxy_choice.is_none(),
                None,
                false,
                cx,
            ));

            // 编辑时保留的导入代理
            if let Some(original) = self.original_proxy.as_deref() {
                list = list.child(self.render_proxy_option(
                    "rterm-proxy-original",
                    format!("{}（原配置）", original.display_label()),
                    self.proxy_custom_kept,
                    None,
                    true,
                    cx,
                ));
            }

            for candidate in &candidates {
                let id = candidate.id;
                list = list.child(self.render_proxy_option(
                    &format!("rterm-proxy-{id}"),
                    format!(
                        "{} · {}:{}",
                        candidate.display_label(),
                        candidate.host,
                        candidate.port
                    ),
                    !self.proxy_custom_kept && self.proxy_choice == Some(id),
                    Some(id),
                    false,
                    cx,
                ));
            }
            container = container.child(list);
        }

        container = container.child(
            div()
                .text_size(px(10.))
                .text_color(if chain_invalid {
                    t.danger()
                } else {
                    t.text_subtle()
                })
                .child(format!("链路: {chain_text}")),
        );

        container.into_any_element()
    }

    /// 代理下拉中的单行选项。
    fn render_proxy_option(
        &self,
        id: &str,
        label: String,
        selected: bool,
        proxy_id: Option<Uuid>,
        use_original: bool,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let t = theme(cx);
        let row_bg = if selected {
            t.row_selected_bg()
        } else {
            t.card_bg()
        };
        div()
            .id(id.to_string())
            .w_full()
            .h(px(28.))
            .px_2()
            .flex()
            .items_center()
            .justify_center()
            .bg(row_bg)
            .text_size(px(11.))
            .text_color(if selected {
                t.text_on_row_selected_bg()
            } else {
                t.text_on_card_bg()
            })
            .cursor_pointer()
            .hover(|style| style.bg(t.row_hover_bg()))
            .on_click(
                cx.listener(move |this: &mut Self, _ev: &gpui::ClickEvent, _w, cx| {
                    this.proxy_custom_kept = use_original;
                    this.proxy_choice = proxy_id;
                    this.proxy_open = false;
                    cx.notify();
                }),
            )
            .child(label)
            .into_any_element()
    }

    fn render_auth_tab(&mut self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let t = theme(cx);
        let selected_fg = t.primary();
        let unselected_fg = t.border_medium();

        let radio = |label_str: &str, variant: AuthType, selected: bool, cx: &mut Context<Self>| {
            let label_owned = label_str.to_string();
            let dot_color = if selected { selected_fg } else { unselected_fg };
            let label_color = if selected {
                t.text_on_card_bg()
            } else {
                t.text_subtle()
            };
            let id_str = format!("rterm-auth-{:?}", variant);
            div()
                .flex()
                .items_center()
                .gap_2()
                .cursor_pointer()
                .mb_2()
                .id(id_str)
                .on_click(
                    cx.listener(move |this: &mut Self, _ev: &gpui::ClickEvent, _w, cx| {
                        this.auth_type = variant;
                        cx.notify();
                    }),
                )
                .child(
                    div()
                        .w(px(12.))
                        .h(px(12.))
                        .rounded_full()
                        .border_2()
                        .border_color(dot_color),
                )
                .child(
                    div()
                        .text_size(px(12.))
                        .text_color(label_color)
                        .child(label_owned),
                )
        };

        let mut container = div()
            .w_full()
            .flex_1()
            .px_4()
            .py_4()
            .child(
                div()
                    .text_size(px(11.))
                    .text_color(t.text_subtle())
                    .mb_2()
                    .child("认证方式"),
            )
            .child(radio(
                "密码",
                AuthType::Password,
                self.auth_type == AuthType::Password,
                cx,
            ))
            .child(radio(
                "私钥",
                AuthType::PrivateKey,
                self.auth_type == AuthType::PrivateKey,
                cx,
            ))
            .child(radio(
                "SSH Agent",
                AuthType::Agent,
                self.auth_type == AuthType::Agent,
                cx,
            ));

        match self.auth_type {
            AuthType::Password => {
                container =
                    container.child(Self::render_field("密码", self.password_input.clone(), cx));
            }
            AuthType::PrivateKey => {
                container = container
                    .child(Self::render_field(
                        "密钥文件路径",
                        self.key_path_input.clone(),
                        cx,
                    ))
                    .child(Self::render_field(
                        "密钥口令",
                        self.passphrase_input.clone(),
                        cx,
                    ));
            }
            AuthType::Agent => {
                container = container.child(
                    div()
                        .text_size(px(11.))
                        .text_color(t.text_subtle())
                        .mt_2()
                        .child("SSH Agent 将从运行时环境枚举密钥"),
                );
            }
        }

        container.into_any_element()
    }

    fn render_advanced_tab(&mut self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let t = theme(cx);
        let compress_id = "rterm-compress-toggle".to_string();
        div()
            .w_full()
            .flex_1()
            .px_4()
            .py_4()
            .child(Self::render_field(
                "KeepAlive 秒数",
                self.keepalive_input.clone(),
                cx,
            ))
            .child(Self::render_field(
                "连接超时秒数",
                self.timeout_input.clone(),
                cx,
            ))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .cursor_pointer()
                    .mb_2()
                    .id(compress_id)
                    .on_click(cx.listener(
                        move |this: &mut Self, _ev: &gpui::ClickEvent, _w, cx| {
                            this.compress_enabled = !this.compress_enabled;
                            cx.notify();
                        },
                    ))
                    .child(
                        div()
                            .w(px(14.))
                            .h(px(14.))
                            .rounded(px(3.))
                            .border_1()
                            .border_color(t.border_input_idle())
                            .bg(if self.compress_enabled {
                                t.primary()
                            } else {
                                t.input_bg()
                            }),
                    )
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(t.text_on_card_bg())
                            .child("启用压缩"),
                    ),
            )
            .into_any_element()
    }

    fn render_button(
        &self,
        label: &str,
        is_primary: bool,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let t = theme(cx);
        let label_owned = label.to_string();
        let id_str = format!("rterm-dialog-btn-{}", label);

        let bg = if is_primary {
            t.primary()
        } else {
            t.secondary_btn_bg()
        };
        let hover_bg = if is_primary {
            t.primary_hover()
        } else {
            t.secondary_btn_hover()
        };
        let text_color = if is_primary {
            t.text_on_primary_btn()
        } else {
            t.text_on_secondary_btn()
        };

        div()
            .h(px(32.))
            .px_4()
            .rounded_md()
            .bg(bg)
            .text_color(text_color)
            .text_size(px(12.))
            .cursor_pointer()
            .hover(move |s| s.bg(hover_bg))
            .id(id_str)
            .flex()
            .items_center()
            .justify_center()
            .on_click(
                cx.listener(move |this: &mut Self, _ev: &gpui::ClickEvent, _w, cx| {
                    if is_primary {
                        this.save(cx);
                    } else {
                        this.cancel(cx);
                    }
                }),
            )
            .child(label_owned)
            .into_any_element()
    }

    fn render_test_button(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let t = theme(cx);
        let bg = t.secondary_btn_bg();
        let hover_bg = t.secondary_btn_hover();
        let text_color = t.text_on_secondary_btn();

        div()
            .h(px(32.))
            .px_4()
            .rounded_md()
            .bg(bg)
            .text_color(text_color)
            .text_size(px(12.))
            .cursor_pointer()
            .hover(move |s| s.bg(hover_bg))
            .id("rterm-dialog-test")
            .flex()
            .items_center()
            .justify_center()
            .on_click(
                cx.listener(|this: &mut Self, _ev: &gpui::ClickEvent, _w, cx| {
                    this.test_connection(cx);
                }),
            )
            .child("测试连接")
            .into_any_element()
    }

    fn render_test_status(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let t = theme(cx);
        let (text, color) = match &self.test_status {
            None => (String::new(), t.text_subtle()),
            Some(TestStatus::Testing) => ("正在连接…".to_string(), t.text_subtle()),
            Some(TestStatus::Ok(msg)) => (msg.clone(), t.success()),
            Some(TestStatus::Failed(msg)) => (msg.clone(), t.danger()),
        };

        div()
            .text_size(px(11.))
            .text_color(color)
            .child(text)
            .into_any_element()
    }

    /// 表单校验错误（红色，显示在 footer 测试连接状态旁）。
    fn render_form_error(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let t = theme(cx);
        let text = self.form_error.clone().unwrap_or_default();
        div()
            .max_w(px(200.))
            .overflow_hidden()
            .truncate()
            .text_size(px(11.))
            .text_color(t.danger())
            .child(text)
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::validate_session_form;

    fn valid() -> Result<super::ValidatedSessionForm, String> {
        validate_session_form("prod", "example.com", "22", "30", "10")
    }

    #[test]
    fn rejects_empty_name() {
        assert!(validate_session_form("  ", "example.com", "22", "", "").is_err());
    }

    #[test]
    fn rejects_empty_host() {
        assert!(validate_session_form("prod", "  ", "22", "", "").is_err());
    }

    #[test]
    fn rejects_invalid_port() {
        assert!(validate_session_form("prod", "example.com", "0", "", "").is_err());
        assert!(validate_session_form("prod", "example.com", "65536", "", "").is_err());
        assert!(validate_session_form("prod", "example.com", "abc", "", "").is_err());
        assert!(validate_session_form("prod", "example.com", "", "", "").is_err());
    }

    #[test]
    fn rejects_invalid_keepalive_or_timeout() {
        assert!(validate_session_form("prod", "example.com", "22", "0", "").is_err());
        assert!(validate_session_form("prod", "example.com", "22", "abc", "").is_err());
        assert!(validate_session_form("prod", "example.com", "22", "", "0").is_err());
    }

    #[test]
    fn accepts_valid_form() {
        let form = valid().expect("合法输入应通过校验");
        assert_eq!(form.name, "prod");
        assert_eq!(form.host, "example.com");
        assert_eq!(form.port, 22);
        assert_eq!(form.keepalive_secs, 30);
        assert_eq!(form.connect_timeout_secs, 10);
    }

    #[test]
    fn trims_inputs_and_defaults_empty_numbers() {
        let form = validate_session_form("  prod  ", " example.com ", " 2222 ", "", "")
            .expect("空白数值应使用默认值");
        assert_eq!(form.name, "prod");
        assert_eq!(form.host, "example.com");
        assert_eq!(form.port, 2222);
        assert_eq!(form.keepalive_secs, 30);
        assert_eq!(form.connect_timeout_secs, 10);
    }
}

//! SessionEditDialog — 会话编辑 Modal
//!
//! 所有颜色通过 `crate::ui::theme(cx)` 取主题，不硬编码 rgb。
//! 所有文字居中。
//!
//! 密码/口令字段用 `EncryptedString::new(&plain)` 创建 version=0 明文，
//! 主程序退出时 crypto_bridge 统一 DPAPI 加密。

use gpui::prelude::*;
use gpui::{div, px, Entity, IntoElement, Render, Window};
use uuid::Uuid;

use crate::event::{AppEvent, EventBusHost};
use crate::state::AppState;
use crate::ui::theme;

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
// SessionEditDialog View
// ---------------------------------------------------------------------------

pub struct SessionEditDialog {
    pub editing_id: Option<Uuid>,
    pub active_tab: DialogTab,

    // Basic
    pub name_input: Entity<TextInput>,
    pub host_input: Entity<TextInput>,
    pub port_input: Entity<TextInput>,
    pub username_input: Entity<TextInput>,
    pub folder_input: Entity<TextInput>,

    // Auth
    pub auth_type: AuthType,
    pub password_input: Entity<TextInput>,
    pub key_path_input: Entity<TextInput>,
    pub passphrase_input: Entity<TextInput>,

    // Advanced
    pub keepalive_input: Entity<TextInput>,
    pub timeout_input: Entity<TextInput>,
    pub compress_enabled: bool,
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

        let (name, host, port, username, folder) = match session {
            Some(s) => (
                s.name.clone(),
                s.host.clone(),
                s.port.to_string(),
                s.username.clone(),
                s.folder.clone().unwrap_or_default(),
            ),
            None => (String::new(), String::new(), "22".into(), String::new(), String::new()),
        };

        let (auth_type, password, key_path, passphrase) = match session {
            Some(s) => match &s.auth {
                rterm_session::AuthMethod::Password { .. } => {
                    (AuthType::Password, String::new(), String::new(), String::new())
                }
                rterm_session::AuthMethod::PrivateKey { path, passphrase } => {
                    let pp = passphrase.as_ref().map(|_| String::new()).unwrap_or_default();
                    (AuthType::PrivateKey, String::new(), path.to_string_lossy().into(), pp)
                }
                rterm_session::AuthMethod::Agent { .. } => {
                    (AuthType::Agent, String::new(), String::new(), String::new())
                }
            },
            None => (AuthType::Password, String::new(), String::new(), String::new()),
        };

        let (keepalive, timeout, compress) = match session {
            Some(s) => (s.keepalive_secs.to_string(), s.connect_timeout_secs.to_string(), s.compress),
            None => ("30".into(), "10".into(), true),
        };

        Self {
            editing_id,
            active_tab: DialogTab::Basic,
            name_input: Self::make_ti(cx, &name, "会话名称", false),
            host_input: Self::make_ti(cx, &host, "主机地址", false),
            port_input: Self::make_ti(cx, &port, "22", false),
            username_input: Self::make_ti(cx, &username, "用户名", false),
            folder_input: Self::make_ti(cx, &folder, "分组", false),
            auth_type,
            password_input: Self::make_ti(cx, &password, "密码", true),
            key_path_input: Self::make_ti(cx, &key_path, "密钥文件路径", false),
            passphrase_input: Self::make_ti(cx, &passphrase, "密钥口令", true),
            keepalive_input: Self::make_ti(cx, &keepalive, "30", false),
            timeout_input: Self::make_ti(cx, &timeout, "10", false),
            compress_enabled: compress,
        }
    }

    pub fn save(&mut self, cx: &mut Context<Self>) {
        let name = self.name_input.read(cx).content.to_string();
        let host = self.host_input.read(cx).content.to_string();
        let port: u16 = self.port_input.read(cx).content.parse().unwrap_or(22);
        let username = self.username_input.read(cx).content.to_string();
        let folder_val = self.folder_input.read(cx).content.to_string();
        let folder = if folder_val.is_empty() { None } else { Some(folder_val) };

        let auth = match self.auth_type {
            AuthType::Password => {
                let pw = self.password_input.read(cx).content.to_string();
                rterm_session::AuthMethod::Password {
                    password: rterm_session::EncryptedString::new(&pw),
                }
            }
            AuthType::PrivateKey => {
                let path = std::path::PathBuf::from(self.key_path_input.read(cx).content.to_string());
                let pp = self.passphrase_input.read(cx).content.to_string();
                let passphrase = if pp.is_empty() {
                    None
                } else {
                    Some(rterm_session::EncryptedString::new(&pp))
                };
                rterm_session::AuthMethod::PrivateKey { path, passphrase }
            }
            AuthType::Agent => rterm_session::AuthMethod::Agent { key_comment: String::new() },
        };

        let keepalive_secs: u32 = self.keepalive_input.read(cx).content.parse().unwrap_or(30);
        let connect_timeout_secs: u32 = self.timeout_input.read(cx).content.parse().unwrap_or(10);

        let session_id = self.editing_id.unwrap_or_else(Uuid::new_v4);

        let (created_at, last_connected) = {
            let state = cx.global::<AppState>();
            if let Some(old) = state.session_store.get(session_id) {
                (old.created_at, old.last_connected)
            } else {
                (rterm_session::default_now(), None)
            }
        };

        let mut session = rterm_session::Session::default();
        session.id = session_id;
        session.name = name;
        session.host = host;
        session.port = port;
        session.username = username;
        session.auth = auth;
        session.keepalive_secs = keepalive_secs;
        session.connect_timeout_secs = connect_timeout_secs;
        session.folder = folder;
        session.compress = self.compress_enabled;
        session.created_at = created_at;
        session.last_modified = rterm_session::default_now();
        session.last_connected = last_connected;

        {
            let state = cx.global_mut::<AppState>();
            state.session_store.upsert(session);
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

        log::info!("Dialog button clicked: is_save=true");
        log::info!("SessionEditDialog saved session {} (new={})", session_id, is_new);
    }

    pub fn cancel(&mut self, cx: &mut Context<Self>) {
        let host = cx.global::<AppState>().event_bus_host.clone();
        cx.update_entity(&host, |_host, cx: &mut Context<EventBusHost>| {
            cx.emit(AppEvent::SessionTreeOpenEditDialog { session_id: None });
        });
        log::info!("Dialog button clicked: is_save=false");
    }
}

// ---------------------------------------------------------------------------
// Render
// ---------------------------------------------------------------------------

impl Render for SessionEditDialog {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        let title = if self.editing_id.is_some() { "编辑会话" } else { "新建会话" };

        div()
            .size_full()
            .absolute()
            .inset_0()
            .bg(t.modal_overlay())
            .flex()
            .items_center()
            .justify_center()
            .child(
                div()
                    .w(px(520.))
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
                            .flex()
                            .border_b_1()
                            .border_color(t.border_subtle())
                            .child(self.render_tab_header(DialogTab::Basic, cx))
                            .child(self.render_tab_header(DialogTab::Auth, cx))
                            .child(self.render_tab_header(DialogTab::Advanced, cx)),
                    )
                    .child(self.render_tab_content(cx))
                    .child(
                        // 底部按钮栏
                        div()
                            .w_full()
                            .h(px(52.))
                            .px_4()
                            .flex()
                            .justify_end()
                            .items_center()
                            .gap_3()
                            .border_t_1()
                            .border_color(t.border_subtle())
                            .child(self.render_button("取消", false, cx))
                            .child(self.render_button("保存", true, cx)),
                    ),
            )
    }
}

impl SessionEditDialog {
    fn render_tab_header(
        &self,
        tab: DialogTab,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let t = theme(cx);
        let is_active = self.active_tab == tab;
        let bg = if is_active { t.row_selected_bg() } else { t.card_header_bg() };
        let text_color = if is_active { t.text_on_row_selected_bg() } else { t.text_subtle() };
        let border_color = if is_active { t.primary() } else { t.border_subtle() };
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
            .on_click(cx.listener(move |this: &mut Self, _ev: &gpui::ClickEvent, _w, cx| {
                this.active_tab = tab;
                cx.notify();
            }))
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
            .child(Self::render_field("会话名称 *", self.name_input.clone(), cx))
            .child(Self::render_field("主机地址 *", self.host_input.clone(), cx))
            .child(Self::render_field("端口", self.port_input.clone(), cx))
            .child(Self::render_field("用户名", self.username_input.clone(), cx))
            .child(Self::render_field("分组", self.folder_input.clone(), cx))
            .into_any_element()
    }

    fn render_auth_tab(&mut self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let t = theme(cx);
        let selected_fg = t.primary();
        let unselected_fg = t.border_medium();

        let radio = |label_str: &str, variant: AuthType, selected: bool, cx: &mut Context<Self>| {
            let label_owned = label_str.to_string();
            let dot_color = if selected { selected_fg } else { unselected_fg };
            let label_color = if selected { t.text_on_card_bg() } else { t.text_subtle() };
            let id_str = format!("rterm-auth-{:?}", variant);
            div()
                .flex()
                .items_center()
                .gap_2()
                .cursor_pointer()
                .mb_2()
                .id(id_str)
                .on_click(cx.listener(move |this: &mut Self, _ev: &gpui::ClickEvent, _w, cx| {
                    this.auth_type = variant;
                    cx.notify();
                }))
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
            .child(radio("密码", AuthType::Password, self.auth_type == AuthType::Password, cx))
            .child(radio("私钥", AuthType::PrivateKey, self.auth_type == AuthType::PrivateKey, cx))
            .child(radio("SSH Agent", AuthType::Agent, self.auth_type == AuthType::Agent, cx));

        match self.auth_type {
            AuthType::Password => {
                container = container.child(Self::render_field("密码", self.password_input.clone(), cx));
            }
            AuthType::PrivateKey => {
                container = container
                    .child(Self::render_field("密钥文件路径", self.key_path_input.clone(), cx))
                    .child(Self::render_field("密钥口令", self.passphrase_input.clone(), cx));
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
            .child(Self::render_field("KeepAlive 秒数", self.keepalive_input.clone(), cx))
            .child(Self::render_field("连接超时秒数", self.timeout_input.clone(), cx))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .cursor_pointer()
                    .mb_2()
                    .id(compress_id)
                    .on_click(cx.listener(move |this: &mut Self, _ev: &gpui::ClickEvent, _w, cx| {
                        this.compress_enabled = !this.compress_enabled;
                        cx.notify();
                    }))
                    .child(
                        div()
                            .w(px(14.))
                            .h(px(14.))
                            .rounded(px(3.))
                            .border_1()
                            .border_color(t.border_input_idle())
                            .bg(if self.compress_enabled { t.primary() } else { t.input_bg() }),
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

        let bg = if is_primary { t.primary() } else { t.secondary_btn_bg() };
        let hover_bg = if is_primary { t.primary_hover() } else { t.secondary_btn_hover() };
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
            .on_click(cx.listener(move |this: &mut Self, _ev: &gpui::ClickEvent, _w, cx| {
                if is_primary {
                    this.save(cx);
                } else {
                    this.cancel(cx);
                }
            }))
            .child(label_owned)
            .into_any_element()
    }
}

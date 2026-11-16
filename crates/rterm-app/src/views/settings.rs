//! SettingsView — 全局设置 Modal（Sprint 4）
//!
//! 左侧 Tab 列表 + 右侧表单面板。所有颜色走 `crate::ui::theme(cx)`，
//! 所有文字在其组件内居中。
//!
//! 保存流程：
//!   1. `apply_inputs()` 把 TextInput 内容解析进 `draft`（带校验）
//!   2. `Settings::save()` 写 `%APPDATA%\rterm\settings.json`
//!   3. 更新 `AppState.settings` + `AppState.theme`（主题即时生效）
//!   4. 广播 `AppSettingsChanged` / `AppThemeChanged`，请求 MainWindow 关闭

use gpui::prelude::*;
use gpui::{
    actions, div, px, Context, Entity, IntoElement, KeyBinding, MouseButton, MouseDownEvent,
    Render, SharedString, Window,
};

use crate::event::{AppEvent, EventBusHost};
use crate::state::{AppState, CursorStyle, Settings};
use crate::theme;
use crate::ui::theme;

use super::text_input::TextInput;

// Ctrl+, 快捷键动作
actions!(rterm_settings, [OpenSettings]);

pub fn key_bindings() -> [KeyBinding; 1] {
    [KeyBinding::new("ctrl-,", OpenSettings, None)]
}

// ---------------------------------------------------------------------------
// Tab
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SettingsTab {
    General,
    SshDefaults,
    Terminal,
    Sftp,
    Monitor,
    Advanced,
    ImportExport,
}

impl SettingsTab {
    const ALL: [SettingsTab; 7] = [
        SettingsTab::General,
        SettingsTab::SshDefaults,
        SettingsTab::Terminal,
        SettingsTab::Sftp,
        SettingsTab::Monitor,
        SettingsTab::Advanced,
        SettingsTab::ImportExport,
    ];

    fn label(&self) -> &'static str {
        match self {
            SettingsTab::General => "常规",
            SettingsTab::SshDefaults => "SSH 默认值",
            SettingsTab::Terminal => "终端外观",
            SettingsTab::Sftp => "SFTP",
            SettingsTab::Monitor => "监控",
            SettingsTab::Advanced => "高级",
            SettingsTab::ImportExport => "导入导出",
        }
    }
}

// SFTP 同步模式：与 Settings.sftp_sync_mode 的字符串一一对应
const SFTP_MODES: [(&str, &str); 4] = [
    ("关闭", "off"),
    ("仅终端 → SFTP", "term_to_sftp"),
    ("仅 SFTP → 终端", "sftp_to_term"),
    ("双向同步", "bidirectional"),
];

const FONT_CHOICES: [&str; 4] = ["Cascadia Code", "Consolas", "JetBrains Mono", "Fira Code"];

// ---------------------------------------------------------------------------
// SettingsView
// ---------------------------------------------------------------------------

pub struct SettingsView {
    pub active_tab: SettingsTab,
    draft: Settings,

    port_input: Entity<TextInput>,
    username_input: Entity<TextInput>,
    keepalive_input: Entity<TextInput>,
    timeout_input: Entity<TextInput>,

    font_size_input: Entity<TextInput>,
    scrollback_input: Entity<TextInput>,

    monitor_interval_input: Entity<TextInput>,
    cpu_threshold_input: Entity<TextInput>,
    mem_threshold_input: Entity<TextInput>,

    log_dir_input: Entity<TextInput>,
    data_dir_input: Entity<TextInput>,

    pub error: Option<String>,
}

impl SettingsView {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let draft = cx.global::<AppState>().settings.clone();

        let ti = |cx: &mut Context<Self>, content: String| -> Entity<TextInput> {
            cx.new(|_cx| TextInput::new(_cx).with_content(content))
        };

        Self {
            active_tab: SettingsTab::General,
            port_input: ti(cx, draft.default_port.to_string()),
            username_input: ti(cx, draft.default_username.clone().unwrap_or_default()),
            keepalive_input: ti(cx, draft.keepalive_secs.to_string()),
            timeout_input: ti(cx, draft.connect_timeout_secs.to_string()),
            font_size_input: ti(cx, draft.default_font_size.to_string()),
            scrollback_input: ti(cx, draft.terminal_scrollback_lines.to_string()),

            monitor_interval_input: ti(cx, draft.monitor_interval_secs.to_string()),
            cpu_threshold_input: ti(cx, draft.monitor_cpu_threshold.to_string()),
            mem_threshold_input: ti(cx, draft.monitor_mem_threshold.to_string()),

            log_dir_input: ti(cx, draft.terminal_log_dir.clone().unwrap_or_default()),
            data_dir_input: ti(cx, draft.data_dir.clone().unwrap_or_default()),

            draft,
            error: None,
        }
    }

    // ---------------- 数据同步 ----------------

    fn set_input(&self, input: &Entity<TextInput>, value: String, cx: &mut Context<Self>) {
        input.update(cx, |ti, cx| {
            ti.content = SharedString::from(value);
            ti.selected_range = ti.content.len()..ti.content.len();
            cx.notify();
        });
    }

    fn sync_inputs(&mut self, cx: &mut Context<Self>) {
        self.set_input(&self.port_input, self.draft.default_port.to_string(), cx);
        self.set_input(
            &self.username_input,
            self.draft.default_username.clone().unwrap_or_default(),
            cx,
        );
        self.set_input(&self.keepalive_input, self.draft.keepalive_secs.to_string(), cx);
        self.set_input(&self.timeout_input, self.draft.connect_timeout_secs.to_string(), cx);
        self.set_input(&self.font_size_input, self.draft.default_font_size.to_string(), cx);
        self.set_input(
            &self.scrollback_input,
            self.draft.terminal_scrollback_lines.to_string(),
            cx,
        );
        self.set_input(
            &self.monitor_interval_input,
            self.draft.monitor_interval_secs.to_string(),
            cx,
        );
        self.set_input(
            &self.cpu_threshold_input,
            self.draft.monitor_cpu_threshold.to_string(),
            cx,
        );
        self.set_input(
            &self.mem_threshold_input,
            self.draft.monitor_mem_threshold.to_string(),
            cx,
        );
        self.set_input(
            &self.log_dir_input,
            self.draft.terminal_log_dir.clone().unwrap_or_default(),
            cx,
        );
        self.set_input(&self.data_dir_input, self.draft.data_dir.clone().unwrap_or_default(), cx);
    }

    /// 把输入框内容解析进 draft，失败返回中文错误。
    fn apply_inputs(&mut self, cx: &mut Context<Self>) -> Result<(), String> {
        let text = |e: &Entity<TextInput>, cx: &mut Context<Self>| -> String {
            e.read(cx).content.trim().to_string()
        };

        let port: u16 = text(&self.port_input, cx)
            .parse()
            .map_err(|_| "默认端口必须是 1–65535 的数字".to_string())?;
        if port == 0 {
            return Err("默认端口必须是 1–65535 的数字".into());
        }
        let keepalive: u32 = text(&self.keepalive_input, cx)
            .parse()
            .map_err(|_| "KeepAlive 必须是整数秒".to_string())?;
        let timeout: u32 = text(&self.timeout_input, cx)
            .parse()
            .map_err(|_| "连接超时必须整数秒".to_string())?;
        let font_size: f32 = text(&self.font_size_input, cx)
            .parse()
            .map_err(|_| "字号必须是数字".to_string())?;
        if !(10.0..=24.0).contains(&font_size) {
            return Err("字号必须在 10–24 之间".into());
        }
        let scrollback: usize = text(&self.scrollback_input, cx)
            .parse()
            .map_err(|_| "滚动缓冲必须是正整数".to_string())?;
        if scrollback == 0 {
            return Err("滚动缓冲必须大于 0".into());
        }
        let interval: u32 = text(&self.monitor_interval_input, cx)
            .parse()
            .map_err(|_| "监控间隔必须是整数秒".to_string())?;
        if interval == 0 {
            return Err("监控间隔必须大于 0".into());
        }
        let cpu: f32 = text(&self.cpu_threshold_input, cx)
            .parse()
            .map_err(|_| "CPU 阈值必须是数字".to_string())?;
        let mem: f32 = text(&self.mem_threshold_input, cx)
            .parse()
            .map_err(|_| "内存阈值必须是数字".to_string())?;
        if !(0.0..=100.0).contains(&cpu) || !(0.0..=100.0).contains(&mem) {
            return Err("阈值必须在 0–100 之间".into());
        }
        let username = text(&self.username_input, cx);
        let log_dir = text(&self.log_dir_input, cx);
        let data_dir = text(&self.data_dir_input, cx);

        self.draft.default_port = port;
        self.draft.default_username = if username.is_empty() { None } else { Some(username) };
        self.draft.keepalive_secs = keepalive;
        self.draft.connect_timeout_secs = timeout;
        self.draft.default_font_size = font_size;
        self.draft.terminal_scrollback_lines = scrollback;
        self.draft.monitor_interval_secs = interval;
        self.draft.monitor_cpu_threshold = cpu;
        self.draft.monitor_mem_threshold = mem;
        self.draft.terminal_log_dir = if log_dir.is_empty() { None } else { Some(log_dir) };
        self.draft.data_dir = if data_dir.is_empty() { None } else { Some(data_dir) };

        Ok(())
    }

    // ---------------- 动作 ----------------

    fn emit(&self, cx: &mut Context<Self>, ev: AppEvent) {
        let host = cx.global::<AppState>().event_bus_host.clone();
        cx.update_entity(&host, |_host, cx: &mut Context<EventBusHost>| {
            cx.emit(ev);
        });
    }

    fn save(&mut self, cx: &mut Context<Self>) {
        if let Err(e) = self.apply_inputs(cx) {
            self.error = Some(e);
            cx.notify();
            return;
        }

        let settings = self.draft.clone();
        if let Err(e) = settings.save() {
            self.error = Some(format!("保存失败: {e:#}"));
            cx.notify();
            return;
        }

        {
            let state = cx.global_mut::<AppState>();
            state.settings = settings.clone();
            state.theme = theme::theme_by_name(&settings.default_theme);
        }

        self.emit(cx, AppEvent::AppSettingsChanged);
        self.emit(
            cx,
            AppEvent::AppThemeChanged {
                name: settings.default_theme.clone(),
            },
        );
        self.emit(cx, AppEvent::SettingsCloseRequested { saved: true });
        log::info!("Settings saved: theme={}", settings.default_theme);
    }

    pub fn cancel(&mut self, cx: &mut Context<Self>) {
        // 取消时把实时预览的主题还原成已保存的主题
        let saved_theme = cx.global::<AppState>().settings.default_theme.clone();
        cx.global_mut::<AppState>().theme = theme::theme_by_name(&saved_theme);
        self.emit(
            cx,
            AppEvent::AppThemeChanged {
                name: saved_theme,
            },
        );
        self.emit(cx, AppEvent::SettingsCloseRequested { saved: false });
    }

    fn reset_defaults(&mut self, cx: &mut Context<Self>) {
        self.draft = Settings::default();
        self.sync_inputs(cx);
        let name = self.draft.default_theme.clone();
        cx.global_mut::<AppState>().theme = theme::theme_by_name(&name);
        self.emit(cx, AppEvent::AppThemeChanged { name });
        self.error = None;
        cx.notify();
    }

    // ---------------- 复用小组件 ----------------

    fn field(label: &str, input: Entity<TextInput>, cx: &mut Context<Self>) -> gpui::AnyElement {
        let t = theme(cx);
        let label = label.to_string();
        div()
            .w_full()
            .mb_2()
            .flex()
            .flex_col()
            .gap_1()
            .child(
                div()
                    .w_full()
                    .flex()
                    .justify_center()
                    .text_size(px(11.))
                    .text_color(t.text_subtle())
                    .child(label),
            )
            .child(input)
            .into_any_element()
    }

    fn action_button(
        &self,
        id: &'static str,
        label: &str,
        primary: bool,
        cx: &mut Context<Self>,
        on_click: impl Fn(&mut Self, &mut Context<Self>) + 'static,
    ) -> gpui::AnyElement {
        let t = theme(cx);
        let label = label.to_string();
        let bg = if primary { t.primary() } else { t.secondary_btn_bg() };
        let hover = if primary { t.primary_hover() } else { t.secondary_btn_hover() };
        let fg = if primary {
            t.text_on_primary_btn()
        } else {
            t.text_on_secondary_btn()
        };

        div()
            .h(px(30.))
            .px_4()
            .rounded_md()
            .bg(bg)
            .text_color(fg)
            .text_size(px(12.))
            .cursor_pointer()
            .flex()
            .items_center()
            .justify_center()
            .id(id)
            .hover(move |s| s.bg(hover))
            .on_click(cx.listener(move |this: &mut Self, _ev: &gpui::ClickEvent, _w, cx| {
                on_click(this, cx);
            }))
            .child(label)
            .into_any_element()
    }

    fn radio(
        &self,
        id: String,
        label: &str,
        selected: bool,
        cx: &mut Context<Self>,
        on_click: impl Fn(&mut Self, &mut Context<Self>) + 'static,
    ) -> gpui::AnyElement {
        let t = theme(cx);
        let label = label.to_string();
        let bg = if selected { t.row_selected_bg() } else { t.secondary_btn_bg() };
        let fg = if selected {
            t.text_on_row_selected_bg()
        } else {
            t.text_on_secondary_btn()
        };

        div()
            .h(px(28.))
            .px_3()
            .rounded_md()
            .bg(bg)
            .text_size(px(12.))
            .text_color(fg)
            .cursor_pointer()
            .flex()
            .items_center()
            .justify_center()
            .id(id)
            .on_click(cx.listener(move |this: &mut Self, _ev: &gpui::ClickEvent, _w, cx| {
                on_click(this, cx);
                cx.notify();
            }))
            .child(format!("{} {}", if selected { "●" } else { "○" }, label))
            .into_any_element()
    }

    fn section_title(text: &str, cx: &mut Context<Self>) -> gpui::AnyElement {
        let t = theme(cx);
        div()
            .w_full()
            .text_size(px(12.))
            .text_color(t.text_on_card_bg())
            .mb_2()
            .flex()
            .justify_center()
            .child(text.to_string())
            .into_any_element()
    }

    // ---------------- 各 Tab ----------------

    fn render_general(&mut self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let t = theme(cx);
        let session_count = cx.global::<AppState>().session_store.lock().unwrap().count();
        let path = crate::state::settings_path();

        div()
            .w_full()
            .flex()
            .flex_col()
            .gap_3()
            .child(
                div()
                    .w_full()
                    .h(px(60.))
                    .rounded_md()
                    .bg(t.input_bg())
                    .border_1()
                    .border_color(t.border_subtle())
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .gap_1()
                    .child(
                        div()
                            .text_size(px(14.))
                            .text_color(t.text_on_card_bg())
                            .child("RTerm — SSH 工作台"),
                    )
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(t.text_subtle())
                            .child(format!("版本 {}", env!("CARGO_PKG_VERSION"))),
                    ),
            )
            .child(
                div()
                    .w_full()
                    .text_size(px(12.))
                    .text_color(t.text_on_card_bg())
                    .flex()
                    .justify_center()
                    .child(format!("已保存会话：{session_count} 个")),
            )
            .child(
                div()
                    .w_full()
                    .text_size(px(11.))
                    .text_color(t.text_subtle())
                    .flex()
                    .justify_center()
                    .child("快捷键 Ctrl+, 可随时打开设置"),
            )
            .child(
                div()
                    .w_full()
                    .p_2()
                    .rounded_md()
                    .bg(t.card_header_bg())
                    .text_size(px(10.))
                    .text_color(t.text_subtle())
                    .flex()
                    .justify_center()
                    .child(format!("配置文件：{}", path.display())),
            )
            .into_any_element()
    }

    fn render_ssh(&mut self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let t = theme(cx);
        let hint_color = t.text_subtle();
        div()
            .w_full()
            .flex()
            .flex_col()
            .child(Self::section_title("新建会话时的 SSH 默认参数", cx))
            .child(Self::field("默认端口", self.port_input.clone(), cx))
            .child(Self::field("默认用户名（可空）", self.username_input.clone(), cx))
            .child(Self::field("KeepAlive（秒）", self.keepalive_input.clone(), cx))
            .child(Self::field("连接超时（秒）", self.timeout_input.clone(), cx))
            .child(
                div()
                    .text_size(px(10.))
                    .text_color(hint_color)
                    .flex()
                    .justify_center()
                    .child("默认用户名留空时，连接时会提示输入"),
            )
            .into_any_element()
    }

    fn render_terminal(&mut self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let t = theme(cx);

        // ---- 左侧表单 ----
        let mut form = div().w(px(300.)).flex().flex_col().gap_2();

        form = form.child(Self::section_title("主题", cx));
        let mut theme_row = div().w_full().flex().flex_wrap().gap_1();
        for th in theme::all_themes() {
            let name = th.name().to_string();
            let selected = self.draft.default_theme == name;
            let id = format!("settings-theme-{name}");
            let bg = if selected { t.primary() } else { t.secondary_btn_bg() };
            let fg = if selected {
                t.text_on_primary_btn()
            } else {
                t.text_on_secondary_btn()
            };
            let click_name = name.clone();
            theme_row = theme_row.child(
                div()
                    .h(px(26.))
                    .px_3()
                    .rounded_md()
                    .bg(bg)
                    .text_color(fg)
                    .text_size(px(11.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .id(id)
                    .on_click(cx.listener(move |this: &mut Self, _ev: &gpui::ClickEvent, _w, cx| {
                        this.draft.default_theme = click_name.clone();
                        // 主题即时生效（取消时还原）
                        cx.global_mut::<AppState>().theme =
                            crate::theme::theme_by_name(&this.draft.default_theme);
                        // 终端调色板跟随预览（Sprint 6）
                        let name = this.draft.default_theme.clone();
                        this.emit(cx, AppEvent::AppThemeChanged { name });
                        cx.notify();
                    }))
                    .child(if selected {
                        format!("✓ {name}")
                    } else {
                        name
                    }),
            );
        }
        form = form.child(theme_row);

        form = form.child(Self::section_title("字体", cx));
        let mut font_row = div().w_full().flex().flex_wrap().gap_1();
        for font in FONT_CHOICES {
            let selected = self.draft.default_font_family == font;
            let id = format!("settings-font-{font}");
            let bg = if selected { t.row_selected_bg() } else { t.secondary_btn_bg() };
            let fg = if selected {
                t.text_on_row_selected_bg()
            } else {
                t.text_on_secondary_btn()
            };
            font_row = font_row.child(
                div()
                    .h(px(26.))
                    .px_3()
                    .rounded_md()
                    .bg(bg)
                    .text_color(fg)
                    .text_size(px(11.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .id(id)
                    .on_click(cx.listener(move |this: &mut Self, _ev: &gpui::ClickEvent, _w, cx| {
                        this.draft.default_font_family = font.to_string();
                        cx.notify();
                    }))
                    .child(font.to_string()),
            );
        }
        form = form.child(font_row);

        form = form
            .child(Self::field("字号（10–24）", self.font_size_input.clone(), cx))
            .child(
                div()
                    .w_full()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .mb_2()
                    .child(
                        div()
                            .w_full()
                            .flex()
                            .justify_center()
                            .text_size(px(11.))
                            .text_color(t.text_subtle())
                            .child("光标样式"),
                    )
                    .child(
                        div()
                            .w_full()
                            .flex()
                            .gap_1()
                            .child(self.radio(
                                "settings-cursor-block".into(),
                                "方块",
                                self.draft.terminal_cursor_style == CursorStyle::Block,
                                cx,
                                |this, _cx| this.draft.terminal_cursor_style = CursorStyle::Block,
                            ))
                            .child(self.radio(
                                "settings-cursor-underline".into(),
                                "下划线",
                                self.draft.terminal_cursor_style == CursorStyle::Underline,
                                cx,
                                |this, _cx| {
                                    this.draft.terminal_cursor_style = CursorStyle::Underline
                                },
                            ))
                            .child(self.radio(
                                "settings-cursor-bar".into(),
                                "竖线",
                                self.draft.terminal_cursor_style == CursorStyle::Bar,
                                cx,
                                |this, _cx| this.draft.terminal_cursor_style = CursorStyle::Bar,
                            )),
                    ),
            )
            .child(Self::field("滚动缓冲行数", self.scrollback_input.clone(), cx));

        // ---- 右侧预览 ----
        let preview = self.render_preview();

        div()
            .w_full()
            .h_full()
            .flex()
            .gap_4()
            .child(form)
            .child(div().flex_1().h_full().child(preview))
            .into_any_element()
    }

    fn render_preview(&self) -> gpui::AnyElement {
        let t = theme::theme_by_name(&self.draft.default_theme);
        let font_family = self.draft.default_font_family.clone();
        let size = px(self.draft.default_font_size);
        let cursor = match self.draft.terminal_cursor_style {
            CursorStyle::Block => "█",
            CursorStyle::Underline => "_",
            CursorStyle::Bar => "│",
        };

        div()
            .w_full()
            .h_full()
            .p_3()
            .rounded_md()
            .bg(t.app_bg())
            .border_1()
            .border_color(t.border_medium())
            .flex()
            .flex_col()
            .gap_1()
            .text_size(size)
            .font_family(font_family)
            .child(
                div()
                    .w_full()
                    .text_color(t.text_subtle())
                    .child("Terminal Preview"),
            )
            .child(
                div()
                    .w_full()
                    .text_color(t.success())
                    .child("$ echo hello"),
            )
            .child(
                div()
                    .w_full()
                    .text_color(t.text_on_app_bg())
                    .child("hello world"),
            )
            .child(
                div()
                    .w_full()
                    .text_color(t.primary())
                    .child("$ ls -la"),
            )
            .child(
                div()
                    .w_full()
                    .text_color(t.text_on_app_bg())
                    .child("drwxr-xr-x  src  Cargo.toml"),
            )
            .child(div().flex_1())
            .child(
                div()
                    .w_full()
                    .text_color(t.text_on_app_bg())
                    .child(format!("$ {cursor}")),
            )
            .into_any_element()
    }

    fn render_sftp(&mut self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let mut list = div().w_full().flex().flex_col().gap_2();
        for (label, value) in SFTP_MODES {
            let selected = self.draft.sftp_sync_mode == value;
            let id = format!("settings-sftp-{value}");
            let v = value.to_string();
            list = list.child(self.radio(id, label, selected, cx, move |this, _cx| {
                this.draft.sftp_sync_mode = v.clone();
            }));
        }

        div()
            .w_full()
            .flex()
            .flex_col()
            .child(Self::section_title("SFTP 路径同步默认模式", cx))
            .child(list)
            .into_any_element()
    }

    fn render_monitor(&mut self, cx: &mut Context<Self>) -> gpui::AnyElement {
        div()
            .w_full()
            .flex()
            .flex_col()
            .child(Self::section_title("监控采样与告警阈值", cx))
            .child(Self::field("采样间隔（秒）", self.monitor_interval_input.clone(), cx))
            .child(Self::field("CPU 告警阈值（%）", self.cpu_threshold_input.clone(), cx))
            .child(Self::field("内存告警阈值（%）", self.mem_threshold_input.clone(), cx))
            .into_any_element()
    }

    fn render_advanced(&mut self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let t = theme(cx);
        div()
            .w_full()
            .flex()
            .flex_col()
            .child(Self::section_title("自定义目录（留空使用默认）", cx))
            .child(Self::field("终端日志目录", self.log_dir_input.clone(), cx))
            .child(Self::field("数据目录", self.data_dir_input.clone(), cx))
            .child(
                div()
                    .w_full()
                    .p_2()
                    .rounded_md()
                    .bg(t.card_header_bg())
                    .text_size(px(10.))
                    .text_color(t.text_subtle())
                    .flex()
                    .justify_center()
                    .child(format!(
                        "默认数据目录：{}",
                        crate::state::settings_path()
                            .parent()
                            .map(|p| p.display().to_string())
                            .unwrap_or_default()
                    )),
            )
            .into_any_element()
    }

    fn render_import_export(&mut self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let t = theme(cx);
        div()
            .w_full()
            .flex()
            .flex_col()
            .gap_3()
            .child(Self::section_title("会话数据迁移", cx))
            .child(
                div()
                    .w_full()
                    .text_size(px(11.))
                    .text_color(t.text_subtle())
                    .flex()
                    .justify_center()
                    .child("导出全部 / 文件夹 / 选中会话；导入支持口令解密与冲突策略"),
            )
            .child(
                div()
                    .w_full()
                    .flex()
                    .gap_3()
                    .justify_center()
                    .child(self.action_button(
                        "settings-open-export",
                        "⬇ 导出会话…",
                        true,
                        cx,
                        |this, cx| {
                            this.emit(cx, AppEvent::SettingsOpenImportExport { export: true });
                        },
                    ))
                    .child(self.action_button(
                        "settings-open-import",
                        "⬆ 导入会话…",
                        false,
                        cx,
                        |this, cx| {
                            this.emit(cx, AppEvent::SettingsOpenImportExport { export: false });
                        },
                    )),
            )
            .into_any_element()
    }

    fn render_tab_button(&self, tab: SettingsTab, cx: &mut Context<Self>) -> gpui::AnyElement {
        let t = theme(cx);
        let active = self.active_tab == tab;
        let bg = if active { t.row_selected_bg() } else { t.card_header_bg() };
        let fg = if active {
            t.text_on_row_selected_bg()
        } else {
            t.text_subtle()
        };
        let border = if active { t.primary() } else { t.card_header_bg() };

        div()
            .w_full()
            .h(px(36.))
            .flex()
            .items_center()
            .justify_center()
            .bg(bg)
            .text_size(px(12.))
            .text_color(fg)
            .cursor_pointer()
            .border_l_2()
            .border_color(border)
            .id(format!("settings-tab-{:?}", tab))
            .hover(|s| s.text_color(t.text_on_card_header_bg()))
            .on_click(cx.listener(move |this: &mut Self, _ev: &gpui::ClickEvent, _w, cx| {
                this.active_tab = tab;
                cx.notify();
            }))
            .child(tab.label())
            .into_any_element()
    }
}

impl Render for SettingsView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);

        let mut tabs = div()
            .w(px(140.))
            .h_full()
            .bg(t.card_header_bg())
            .border_r_1()
            .border_color(t.border_subtle())
            .flex()
            .flex_col();
        for tab in SettingsTab::ALL {
            tabs = tabs.child(self.render_tab_button(tab, cx));
        }

        let content = match self.active_tab {
            SettingsTab::General => self.render_general(cx),
            SettingsTab::SshDefaults => self.render_ssh(cx),
            SettingsTab::Terminal => self.render_terminal(cx),
            SettingsTab::Sftp => self.render_sftp(cx),
            SettingsTab::Monitor => self.render_monitor(cx),
            SettingsTab::Advanced => self.render_advanced(cx),
            SettingsTab::ImportExport => self.render_import_export(cx),
        };

        let error_text = self.error.clone().unwrap_or_default();
        let error_color = t.danger();

        let footer = div()
            .w_full()
            .h(px(52.))
            .px_4()
            .flex()
            .items_center()
            .gap_2()
            .border_t_1()
            .border_color(t.border_subtle())
            .child(
                div()
                    .flex_1()
                    .overflow_hidden()
                    .truncate()
                    .text_size(px(11.))
                    .text_color(error_color)
                    .flex()
                    .justify_center()
                    .child(error_text),
            )
            .child(self.action_button("settings-reset", "恢复默认", false, cx, |this, cx| {
                this.reset_defaults(cx);
            }))
            .child(self.action_button("settings-cancel", "取消", false, cx, |this, cx| {
                this.cancel(cx);
            }))
            .child(self.action_button("settings-save", "保存", true, cx, |this, cx| {
                this.save(cx);
            }));

        div()
            .size_full()
            .absolute()
            .inset_0()
            .bg(t.modal_overlay())
            .flex()
            .items_center()
            .justify_center()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this: &mut Self, _ev: &MouseDownEvent, _w, cx| {
                    // 点击遮罩空白处 = 取消
                    this.cancel(cx);
                }),
            )
            .child(
                div()
                    .w(px(760.))
                    .h(px(560.))
                    .bg(t.card_bg())
                    .border_1()
                    .border_color(t.border_medium())
                    .rounded_lg()
                    .flex()
                    .flex_col()
                    // 阻止点击卡片冒泡到遮罩
                    .on_mouse_down(MouseButton::Left, |_ev, _w, cx| cx.stop_propagation())
                    .child(
                        div()
                            .w_full()
                            .h(px(44.))
                            .px_4()
                            .flex()
                            .items_center()
                            .justify_center()
                            .bg(t.card_header_bg())
                            .border_b_1()
                            .border_color(t.border_subtle())
                            .text_color(t.text_on_card_header_bg())
                            .text_size(px(14.))
                            .child("⚙ 设置"),
                    )
                    .child(
                        div()
                            .w_full()
                            .flex_1()
                            .flex()
                            .child(tabs)
                            .child(
                                div()
                                    .flex_1()
                                    .h_full()
                                    .px_4()
                                    .py_3()
                                    .flex()
                                    .flex_col()
                                    .overflow_hidden()
                                    .child(content),
                            ),
                    )
                    .child(footer),
            )
    }
}

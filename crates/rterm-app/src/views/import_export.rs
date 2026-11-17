//! ImportExportDialog — 会话导入 / 导出 Modal（Sprint 4）
//!
//! 导出：ExportScope（全部 / 文件夹 / 勾选会话）+ 加密方式（DPAPI / AES-GCM 口令）
//!       + 输出路径（rfd 文件选择器）。
//! 导入：源文件 + 可选口令 + ConflictPolicy + 预览 diff + 执行并显示 ImportReport。
//!
//! 所有颜色走 `crate::ui::theme(cx)`，所有文字在其组件内居中。

use std::collections::HashSet;

use gpui::prelude::*;
use gpui::{
    div, px, ClickEvent, Context, Entity, IntoElement, MouseButton, MouseDownEvent, Render,
    ScrollHandle, ScrollWheelEvent, SharedString, Window,
};
use uuid::Uuid;

use crate::event::{AppEvent, EventBusHost};
use crate::state::{lock_or_recover, AppState};
use crate::ui::theme;

use super::scroll_list::{scroll_list, ScrollMetrics};
use super::text_input::TextInput;

/// 勾选会话 / 预览列表：定高行 + 固定视口（9.13 自绘滚动容器）
const LIST_ROW_H: f32 = 24.0;
const SELECT_LIST_VIEW_H: f32 = 120.0;
const PREVIEW_LIST_VIEW_H: f32 = 144.0;

// ---------------------------------------------------------------------------
// 模式 / 选项
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IoMode {
    Export,
    Import,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ScopeKind {
    All,
    Folder,
    Selection,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum EncryptionKind {
    Dpapi,
    Passphrase,
}

/// 默认导出路径：`<data_dir>\sessions-export.json`
/// （data_dir 默认 `%APPDATA%\rterm`，可在设置 → 高级中修改）
pub fn default_export_path() -> std::path::PathBuf {
    crate::state::data_dir_path().join("sessions-export.json")
}

// ---------------------------------------------------------------------------
// ImportExportDialog
// ---------------------------------------------------------------------------

pub struct ImportExportDialog {
    pub mode: IoMode,

    // 导出
    scope: ScopeKind,
    selected_folder: Option<String>,
    selected_ids: HashSet<Uuid>,
    encryption: EncryptionKind,
    export_pass: Entity<TextInput>,
    export_pass_confirm: Entity<TextInput>,
    export_path: Entity<TextInput>,

    // 导入
    import_path: Entity<TextInput>,
    import_pass: Entity<TextInput>,
    policy: rterm_session::ConflictPolicy,
    preview: Option<Vec<rterm_session::ImportPreviewItem>>,

    /// 勾选会话 / 预览列表的滚动状态（9.13）
    select_scroll: ScrollMetrics,
    preview_scroll: ScrollMetrics,
    /// 弹窗主体滚动状态：窗口变矮时表单可滚动，切换导入/导出复位
    body_scroll: ScrollHandle,

    /// (是否成功, 文本)
    pub message: Option<(bool, String)>,
}

impl ImportExportDialog {
    pub fn new(cx: &mut Context<Self>, mode: IoMode) -> Self {
        let default_path = default_export_path().display().to_string();
        let ti = |cx: &mut Context<Self>, content: String, password: bool| -> Entity<TextInput> {
            cx.new(|_cx| {
                let mut input = TextInput::new(_cx).with_content(content);
                if password {
                    input = input.password();
                }
                input
            })
        };

        Self {
            mode,
            scope: ScopeKind::All,
            selected_folder: None,
            selected_ids: HashSet::new(),
            encryption: EncryptionKind::Dpapi,
            export_pass: ti(cx, String::new(), true),
            export_pass_confirm: ti(cx, String::new(), true),
            export_path: ti(cx, default_path.clone(), false),

            import_path: ti(cx, default_path, false),
            import_pass: ti(cx, String::new(), true),
            policy: rterm_session::ConflictPolicy::Skip,
            preview: None,

            select_scroll: ScrollMetrics::new(SELECT_LIST_VIEW_H, LIST_ROW_H),
            preview_scroll: ScrollMetrics::new(PREVIEW_LIST_VIEW_H, LIST_ROW_H),
            body_scroll: ScrollHandle::new(),

            message: None,
        }
    }

    // ---------------- 数据读取 ----------------

    fn text(&self, input: &Entity<TextInput>, cx: &Context<Self>) -> String {
        input.read(cx).content.trim().to_string()
    }

    fn set_input(&self, input: &Entity<TextInput>, value: String, cx: &mut Context<Self>) {
        input.update(cx, |ti, cx| {
            ti.content = SharedString::from(value);
            ti.selected_range = ti.content.len()..ti.content.len();
            cx.notify();
        });
    }

    fn emit(&self, cx: &mut Context<Self>, ev: AppEvent) {
        let host = cx.global::<AppState>().event_bus_host.clone();
        cx.update_entity(&host, |_host, cx: &mut Context<EventBusHost>| {
            cx.emit(ev);
        });
    }

    fn failure(&mut self, msg: impl Into<String>, cx: &mut Context<Self>) {
        self.message = Some((false, msg.into()));
        cx.notify();
    }

    // ---------------- 导出 ----------------

    fn browse_export(&mut self, cx: &mut Context<Self>) {
        if let Some(path) = rfd::FileDialog::new()
            .set_title("导出会话")
            .set_file_name("sessions-export.json")
            .add_filter("JSON", &["json"])
            .save_file()
        {
            self.set_input(&self.export_path, path.display().to_string(), cx);
        }
    }

    fn do_export(&mut self, cx: &mut Context<Self>) {
        let path_str = self.text(&self.export_path, cx);
        if path_str.is_empty() {
            self.failure("请填写输出文件路径", cx);
            return;
        }

        let scope = match self.scope {
            ScopeKind::All => rterm_session::ExportScope::All,
            ScopeKind::Folder => match &self.selected_folder {
                Some(f) => rterm_session::ExportScope::Folder(f.clone()),
                None => {
                    self.failure("请选择一个文件夹", cx);
                    return;
                }
            },
            ScopeKind::Selection => {
                if self.selected_ids.is_empty() {
                    self.failure("请至少勾选一个会话", cx);
                    return;
                }
                rterm_session::ExportScope::Selection(self.selected_ids.iter().copied().collect())
            }
        };

        let passphrase = match self.encryption {
            EncryptionKind::Dpapi => None,
            EncryptionKind::Passphrase => {
                let p1 = self.text(&self.export_pass, cx);
                let p2 = self.text(&self.export_pass_confirm, cx);
                if p1.is_empty() {
                    self.failure("口令不能为空", cx);
                    return;
                }
                if p1 != p2 {
                    self.failure("两次输入的口令不一致", cx);
                    return;
                }
                Some(p1)
            }
        };

        let sessions = lock_or_recover(&cx.global::<AppState>().session_store)
            .list()
            .to_vec();
        let count = count_scope(&sessions, &scope);

        let result = rterm_session::export::export_json(&sessions, scope, passphrase.as_deref())
            .map_err(|e| format!("{e:#}"))
            .and_then(|json| {
                if let Some(parent) = std::path::Path::new(&path_str).parent() {
                    let _ = std::fs::create_dir_all(parent);
                }
                std::fs::write(&path_str, json).map_err(|e| format!("写入文件失败: {e}"))
            });

        match result {
            Ok(()) => {
                log::info!("Exported {count} sessions to {path_str}");
                self.message = Some((true, format!("已导出 {count} 个会话 → {path_str}")));
            }
            Err(e) => {
                log::error!("导出失败: {e}");
                self.message = Some((false, format!("导出失败: {e}")));
            }
        }
        cx.notify();
    }

    // ---------------- 导入 ----------------

    fn browse_import(&mut self, cx: &mut Context<Self>) {
        if let Some(path) = rfd::FileDialog::new()
            .set_title("导入会话")
            .add_filter("JSON", &["json"])
            .pick_file()
        {
            self.set_input(&self.import_path, path.display().to_string(), cx);
        }
    }

    fn do_preview(&mut self, cx: &mut Context<Self>) {
        let path = self.text(&self.import_path, cx);
        if path.is_empty() {
            self.failure("请填写导入文件路径", cx);
            return;
        }
        let json = match std::fs::read_to_string(&path) {
            Ok(j) => j,
            Err(e) => {
                self.failure(format!("读取文件失败: {e}"), cx);
                return;
            }
        };
        let passphrase = {
            let p = self.text(&self.import_pass, cx);
            if p.is_empty() {
                None
            } else {
                Some(p)
            }
        };
        let existing = lock_or_recover(&cx.global::<AppState>().session_store)
            .list()
            .to_vec();

        match rterm_session::export::preview_import(
            &json,
            &existing,
            self.policy,
            passphrase.as_deref(),
        ) {
            Ok(items) => {
                self.preview = Some(items);
                self.message = None;
            }
            Err(e) => {
                self.preview = None;
                self.message = Some((false, format!("{e}")));
            }
        }
        cx.notify();
    }

    fn do_import(&mut self, cx: &mut Context<Self>) {
        let path = self.text(&self.import_path, cx);
        if path.is_empty() {
            self.failure("请填写导入文件路径", cx);
            return;
        }
        let json = match std::fs::read_to_string(&path) {
            Ok(j) => j,
            Err(e) => {
                self.failure(format!("读取文件失败: {e}"), cx);
                return;
            }
        };
        let passphrase = {
            let p = self.text(&self.import_pass, cx);
            if p.is_empty() {
                None
            } else {
                Some(p)
            }
        };
        let existing = lock_or_recover(&cx.global::<AppState>().session_store)
            .list()
            .to_vec();

        match rterm_session::export::import_json(
            &json,
            &existing,
            self.policy,
            passphrase.as_deref(),
        ) {
            Ok((merged, report)) => {
                {
                    let state = cx.global_mut::<AppState>();
                    let mut store = lock_or_recover(&state.session_store);
                    store.clear();
                    for s in merged {
                        store.upsert(s);
                    }
                }

                // 合并写回后立即落盘，失败只记日志（不阻断导入结果展示）。
                let session_store = cx.global::<AppState>().session_store.clone();
                if let Err(e) = crate::state::persist_sessions(&session_store) {
                    log::error!("导入后保存会话失败: {e:#}");
                }

                self.emit(
                    cx,
                    AppEvent::SessionImported {
                        added: report.added,
                        updated: report.updated,
                        skipped: report.skipped,
                    },
                );
                self.preview = None;
                self.message = Some((
                    true,
                    format!(
                        "导入完成：新增 {}，更新 {}，跳过 {}，改名 {}",
                        report.added, report.updated, report.skipped, report.renamed
                    ),
                ));
                log::info!(
                    "Imported: added={} updated={} skipped={} renamed={}",
                    report.added,
                    report.updated,
                    report.skipped,
                    report.renamed
                );
            }
            Err(e) => {
                self.preview = None;
                self.message = Some((false, format!("{e}")));
            }
        }
        cx.notify();
    }

    // ---------------- 小组件 ----------------

    fn section(&self, label: &str, cx: &mut Context<Self>) -> gpui::AnyElement {
        let t = theme(cx);
        div()
            .w_full()
            .mt_1()
            .text_size(px(11.))
            .text_color(t.text_subtle())
            .flex()
            .justify_center()
            .child(label.to_string())
            .into_any_element()
    }

    fn field(label: &str, input: Entity<TextInput>, cx: &mut Context<Self>) -> gpui::AnyElement {
        let t = theme(cx);
        div()
            .w_full()
            .flex()
            .flex_col()
            .gap_1()
            .child(
                div()
                    .w_full()
                    .text_size(px(11.))
                    .text_color(t.text_subtle())
                    .flex()
                    .justify_center()
                    .child(label.to_string()),
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
        let bg = if primary {
            t.primary()
        } else {
            t.secondary_btn_bg()
        };
        let hover = if primary {
            t.primary_hover()
        } else {
            t.secondary_btn_hover()
        };
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
            .flex()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .id(id)
            .hover(move |s| s.bg(hover))
            .on_click(
                cx.listener(move |this: &mut Self, _ev: &ClickEvent, _w, cx| {
                    on_click(this, cx);
                }),
            )
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
        let bg = if selected {
            t.row_selected_bg()
        } else {
            t.secondary_btn_bg()
        };
        let fg = if selected {
            t.text_on_row_selected_bg()
        } else {
            t.text_on_secondary_btn()
        };

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
            .on_click(
                cx.listener(move |this: &mut Self, _ev: &ClickEvent, _w, cx| {
                    on_click(this, cx);
                    cx.notify();
                }),
            )
            .child(format!("{} {}", if selected { "●" } else { "○" }, label))
            .into_any_element()
    }

    fn mode_tab(&self, mode: IoMode, label: &str, cx: &mut Context<Self>) -> gpui::AnyElement {
        let t = theme(cx);
        let active = self.mode == mode;
        let bg = if active {
            t.row_selected_bg()
        } else {
            t.card_header_bg()
        };
        let fg = if active {
            t.text_on_row_selected_bg()
        } else {
            t.text_subtle()
        };
        let border = if active {
            t.primary()
        } else {
            t.card_header_bg()
        };

        div()
            .h(px(30.))
            .flex_1()
            .flex()
            .items_center()
            .justify_center()
            .bg(bg)
            .text_color(fg)
            .text_size(px(12.))
            .cursor_pointer()
            .border_b_2()
            .border_color(border)
            .id(format!("io-mode-{label}"))
            .on_click(
                cx.listener(move |this: &mut Self, _ev: &ClickEvent, _w, cx| {
                    this.mode = mode;
                    this.message = None;
                    this.preview = None;
                    this.body_scroll = ScrollHandle::new();
                    cx.notify();
                }),
            )
            .child(label.to_string())
            .into_any_element()
    }

    // ---------------- 导出 / 导入 表单 ----------------

    fn render_export_body(&mut self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let t = theme(cx);

        let mut body = div().w_full().flex().flex_col().gap_2();

        body = body.child(self.section("导出范围", cx));
        body = body.child(
            div()
                .w_full()
                .flex()
                .gap_1()
                .child(self.radio(
                    "io-scope-all".into(),
                    "全部会话",
                    self.scope == ScopeKind::All,
                    cx,
                    |this, _cx| this.scope = ScopeKind::All,
                ))
                .child(self.radio(
                    "io-scope-folder".into(),
                    "某个文件夹",
                    self.scope == ScopeKind::Folder,
                    cx,
                    |this, _cx| this.scope = ScopeKind::Folder,
                ))
                .child(self.radio(
                    "io-scope-selection".into(),
                    "选择会话",
                    self.scope == ScopeKind::Selection,
                    cx,
                    |this, _cx| this.scope = ScopeKind::Selection,
                )),
        );

        if self.scope == ScopeKind::Folder {
            let folders = {
                let sessions = lock_or_recover(&cx.global::<AppState>().session_store)
                    .list()
                    .to_vec();
                let mut set: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
                for s in &sessions {
                    if let Some(f) = s.folder.as_ref().filter(|f| !f.is_empty()) {
                        set.insert(f.clone());
                    }
                }
                set.into_iter().collect::<Vec<_>>()
            };

            if folders.is_empty() {
                body = body.child(
                    div()
                        .w_full()
                        .text_size(px(10.))
                        .text_color(t.text_subtle())
                        .flex()
                        .justify_center()
                        .child("暂无文件夹（会话都未分组）"),
                );
            } else {
                let mut chips = div().w_full().flex().flex_wrap().gap_1();
                for f in folders {
                    let selected = self.selected_folder.as_deref() == Some(f.as_str());
                    let id = format!("io-folder-{f}");
                    let bg = if selected {
                        t.primary()
                    } else {
                        t.secondary_btn_bg()
                    };
                    let fg = if selected {
                        t.text_on_primary_btn()
                    } else {
                        t.text_on_secondary_btn()
                    };
                    let v = f.clone();
                    chips = chips.child(
                        div()
                            .h(px(24.))
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
                            .on_click(cx.listener(
                                move |this: &mut Self, _ev: &ClickEvent, _w, cx| {
                                    this.selected_folder = Some(v.clone());
                                    cx.notify();
                                },
                            ))
                            .child(f),
                    );
                }
                body = body.child(chips);
            }
        }

        if self.scope == ScopeKind::Selection {
            let sessions = lock_or_recover(&cx.global::<AppState>().session_store)
                .list()
                .to_vec();
            if sessions.is_empty() {
                body = body.child(
                    div()
                        .w_full()
                        .text_size(px(10.))
                        .text_color(t.text_subtle())
                        .flex()
                        .justify_center()
                        .child("暂无会话"),
                );
            } else {
                // 9.13：自绘滚动容器，24+ 会话也能滚动勾选（定高行虚拟化）
                self.select_scroll.set_rows(sessions.len());
                let list_theme = t.clone();
                let selected_ids = &self.selected_ids;
                let list_cx: &Context<Self> = cx;
                body = body.child(
                    div()
                        .w_full()
                        .p_1()
                        .rounded_md()
                        .bg(t.card_header_bg())
                        .border_1()
                        .border_color(t.border_subtle())
                        .child(scroll_list(
                            "io-select-list",
                            self.select_scroll,
                            t.border_subtle(),
                            t.text_subtle(),
                            cx.listener(|this: &mut Self, event: &ScrollWheelEvent, _w, cx| {
                                if this.select_scroll.on_wheel(event) {
                                    cx.notify();
                                }
                            }),
                            move |index| {
                                let s = &sessions[index];
                                let checked = selected_ids.contains(&s.id);
                                let bg = if checked {
                                    list_theme.row_selected_bg()
                                } else {
                                    list_theme.card_header_bg()
                                };
                                let fg = list_theme.text_on_card_bg();
                                let sid = s.id;
                                div()
                                    .w_full()
                                    .h_full()
                                    .px_2()
                                    .rounded_md()
                                    .bg(bg)
                                    .text_color(fg)
                                    .text_size(px(10.))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .cursor_pointer()
                                    .id(format!("io-sel-{sid}"))
                                    .on_click(list_cx.listener(
                                        move |this: &mut Self, _ev: &ClickEvent, _w, cx| {
                                            if !this.selected_ids.remove(&sid) {
                                                this.selected_ids.insert(sid);
                                            }
                                            cx.notify();
                                        },
                                    ))
                                    .child(format!(
                                        "{} {}",
                                        if checked { "☑" } else { "☐" },
                                        s.display_label()
                                    ))
                                    .into_any_element()
                            },
                        )),
                );
            }
        }

        body = body.child(self.section("加密方式", cx));
        body = body.child(
            div()
                .w_full()
                .flex()
                .gap_1()
                .child(self.radio(
                    "io-enc-dpapi".into(),
                    "本机 DPAPI",
                    self.encryption == EncryptionKind::Dpapi,
                    cx,
                    |this, _cx| this.encryption = EncryptionKind::Dpapi,
                ))
                .child(self.radio(
                    "io-enc-pass".into(),
                    "口令加密（AES-GCM）",
                    self.encryption == EncryptionKind::Passphrase,
                    cx,
                    |this, _cx| this.encryption = EncryptionKind::Passphrase,
                )),
        );

        if self.encryption == EncryptionKind::Passphrase {
            body = body
                .child(Self::field("口令", self.export_pass.clone(), cx))
                .child(Self::field(
                    "确认口令",
                    self.export_pass_confirm.clone(),
                    cx,
                ));
        }

        body = body.child(self.section("输出文件", cx)).child(
            div()
                .w_full()
                .flex()
                .gap_2()
                .child(div().flex_1().child(self.export_path.clone()))
                .child(self.action_button(
                    "io-browse-export",
                    "浏览…",
                    false,
                    cx,
                    |this, cx| this.browse_export(cx),
                )),
        );

        body.into_any_element()
    }

    fn render_import_body(&mut self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let t = theme(cx);

        let mut body = div().w_full().flex().flex_col().gap_2();

        body = body
            .child(self.section("源文件", cx))
            .child(
                div()
                    .w_full()
                    .flex()
                    .gap_2()
                    .child(div().flex_1().child(self.import_path.clone()))
                    .child(self.action_button(
                        "io-browse-import",
                        "浏览…",
                        false,
                        cx,
                        |this, cx| this.browse_import(cx),
                    )),
            )
            .child(Self::field(
                "口令（空 = 按 DPAPI 解密）",
                self.import_pass.clone(),
                cx,
            ))
            .child(self.section("冲突策略", cx))
            .child(
                div()
                    .w_full()
                    .flex()
                    .gap_1()
                    .child(self.radio(
                        "io-policy-overwrite".into(),
                        "覆盖已有",
                        self.policy == rterm_session::ConflictPolicy::Overwrite,
                        cx,
                        |this, _cx| this.policy = rterm_session::ConflictPolicy::Overwrite,
                    ))
                    .child(self.radio(
                        "io-policy-skip".into(),
                        "跳过已有",
                        self.policy == rterm_session::ConflictPolicy::Skip,
                        cx,
                        |this, _cx| this.policy = rterm_session::ConflictPolicy::Skip,
                    ))
                    .child(self.radio(
                        "io-policy-rename".into(),
                        "改名后新增",
                        self.policy == rterm_session::ConflictPolicy::Rename,
                        cx,
                        |this, _cx| this.policy = rterm_session::ConflictPolicy::Rename,
                    )),
            );

        // 预览区
        let preview_title = match &self.preview {
            Some(items) => format!("预览（{} 个会话）", items.len()),
            None => "预览（点击下方「预览」按钮）".to_string(),
        };
        body = body.child(self.section(&preview_title, cx));

        // 9.13：自绘滚动容器（12+ 条预览可滚动；空状态保留固定高度避免弹窗跳动）
        self.preview_scroll
            .set_rows(self.preview.as_ref().map_or(0, |items| items.len()));
        let mut list = div()
            .w_full()
            .h(px(PREVIEW_LIST_VIEW_H + 16.0))
            .p_2()
            .rounded_md()
            .bg(t.card_header_bg())
            .border_1()
            .border_color(t.border_subtle())
            .flex()
            .flex_col();

        match &self.preview {
            Some(items) if !items.is_empty() => {
                let list_theme = t.clone();
                list = list.child(scroll_list(
                    "io-preview-list",
                    self.preview_scroll,
                    t.border_subtle(),
                    t.text_subtle(),
                    cx.listener(|this: &mut Self, event: &ScrollWheelEvent, _w, cx| {
                        if this.preview_scroll.on_wheel(event) {
                            cx.notify();
                        }
                    }),
                    move |index| {
                        let item = &items[index];
                        let (dot, label, color) = match item.status {
                            rterm_session::ImportPreviewStatus::Added => {
                                ("🟢", "新增", list_theme.success())
                            }
                            rterm_session::ImportPreviewStatus::Updated => {
                                ("🟡", "更新", list_theme.primary())
                            }
                            rterm_session::ImportPreviewStatus::Skipped => {
                                ("⚪", "跳过", list_theme.text_subtle())
                            }
                            rterm_session::ImportPreviewStatus::Renamed => {
                                ("🔵", "改名", list_theme.primary())
                            }
                        };
                        div()
                            .w_full()
                            .h_full()
                            .text_size(px(11.))
                            .text_color(color)
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(format!("{dot} {label}  {}", item.name))
                            .into_any_element()
                    },
                ));
            }
            Some(_) => {
                list = list.child(
                    div()
                        .w_full()
                        .flex_1()
                        .text_size(px(11.))
                        .text_color(t.text_subtle())
                        .flex()
                        .items_center()
                        .justify_center()
                        .child("文件里没有会话"),
                );
            }
            None => {
                list = list.child(
                    div()
                        .w_full()
                        .flex_1()
                        .text_size(px(11.))
                        .text_color(t.text_subtle())
                        .flex()
                        .items_center()
                        .justify_center()
                        .child("预览会列出每条会话将被新增 / 更新 / 跳过"),
                );
            }
        }
        body = body.child(list);

        body.into_any_element()
    }
}

fn count_scope(sessions: &[rterm_session::Session], scope: &rterm_session::ExportScope) -> usize {
    match scope {
        rterm_session::ExportScope::All => sessions.len(),
        rterm_session::ExportScope::Folder(f) => sessions
            .iter()
            .filter(|s| s.folder.as_deref() == Some(f.as_str()))
            .count(),
        rterm_session::ExportScope::Selection(ids) => {
            sessions.iter().filter(|s| ids.contains(&s.id)).count()
        }
    }
}

// ---------------------------------------------------------------------------
// Render
// ---------------------------------------------------------------------------

impl Render for ImportExportDialog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        // 自适应尺寸：默认 620×600，窗口更小时收缩，主体可滚动
        let (card_w, card_h) = super::adaptive_dialog_size(window, 620.0, 600.0);
        let card_w = card_w.max(520.0);
        let card_h = card_h.max(320.0);

        let body = match self.mode {
            IoMode::Export => self.render_export_body(cx),
            IoMode::Import => self.render_import_body(cx),
        };

        let (msg_text, msg_color) = match &self.message {
            Some((true, text)) => (text.clone(), t.success()),
            Some((false, text)) => (text.clone(), t.danger()),
            None => (String::new(), t.text_subtle()),
        };

        let primary_label = match self.mode {
            IoMode::Export => "导出",
            IoMode::Import => "执行导入",
        };
        let primary_id = match self.mode {
            IoMode::Export => "io-do-export",
            IoMode::Import => "io-do-import",
        };

        let mut footer = div()
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
                    .text_color(msg_color)
                    .flex()
                    .justify_center()
                    .child(msg_text),
            )
            .child(
                self.action_button("io-cancel", "取消", false, cx, |this, cx| {
                    this.emit(cx, AppEvent::ImportExportCloseRequested);
                }),
            );

        if self.mode == IoMode::Import {
            footer = footer.child(self.action_button(
                "io-do-preview",
                "预览",
                false,
                cx,
                |this, cx| this.do_preview(cx),
            ));
            footer = footer.child(self.action_button(
                primary_id,
                primary_label,
                true,
                cx,
                |this, cx| this.do_import(cx),
            ));
        } else {
            footer = footer.child(self.action_button(
                primary_id,
                primary_label,
                true,
                cx,
                |this, cx| this.do_export(cx),
            ));
        }

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
                    this.emit(cx, AppEvent::ImportExportCloseRequested);
                }),
            )
            .child(
                div()
                    .w(px(card_w))
                    .h(px(card_h))
                    .bg(t.card_bg())
                    .border_1()
                    .border_color(t.border_medium())
                    .rounded_lg()
                    .flex()
                    .flex_col()
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
                            .child("会话导入 / 导出"),
                    )
                    .child(
                        div()
                            .w_full()
                            .h(px(30.))
                            .flex()
                            .child(self.mode_tab(IoMode::Export, "导出", cx))
                            .child(self.mode_tab(IoMode::Import, "导入", cx)),
                    )
                    .child(
                        div()
                            .id("io-body")
                            .w_full()
                            .flex_1()
                            .min_h(px(0.))
                            .px_4()
                            .py_2()
                            .flex()
                            .flex_col()
                            .overflow_y_scroll()
                            .track_scroll(&self.body_scroll)
                            // 子项保持自然高度，内容超出时滚动距离 > 0
                            .child(div().w_full().flex_shrink_0().child(body)),
                    )
                    .child(footer),
            )
    }
}

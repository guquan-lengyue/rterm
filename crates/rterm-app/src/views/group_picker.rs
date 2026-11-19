//! group_picker — 可搜索分组下拉 + 内联新建
//!
//! 职责：
//! - `GroupPicker`：会话编辑对话框中的分组字段（替代自由文本，避免错名）
//! - `GroupPickerDialog`：右键「移动到分组」弹窗
//!
//! 分组数据本身由 `SessionStore` 维护；本组件只负责选择与发出新建事件，
//! 创建 / 持久化由父级（SessionEditDialog / MainWindow）执行。

use gpui::prelude::*;
use gpui::{
    div, px, AnyElement, Context, Entity, EventEmitter, FocusHandle, Focusable, IntoElement,
    Render, ScrollHandle, Window,
};
use uuid::Uuid;

use crate::event::{AppEvent, EventBusHost};
use crate::state::{lock_or_recover, AppState};
use crate::ui::theme;

use super::text_input::{TextInput, TextInputEvent};

// ---------------------------------------------------------------------------
// GroupPicker
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GroupPickerEvent {
    /// 用户请求新建分组（父级负责校验 / 落库 / 回填选择）。
    CreateFolder(String),
}

pub struct GroupPicker {
    selected: Option<String>,
    open: bool,
    query: Entity<TextInput>,
    creating: bool,
    new_input: Entity<TextInput>,
    error: Option<String>,
    list_scroll: ScrollHandle,
}

impl EventEmitter<GroupPickerEvent> for GroupPicker {}

impl GroupPicker {
    pub fn new(cx: &mut Context<Self>, initial: Option<String>) -> Self {
        let query = cx.new(|cx| TextInput::new(cx).with_placeholder("搜索分组…"));
        let new_input =
            cx.new(|cx| TextInput::new(cx).with_placeholder("新分组名称（支持 a/b 层级）"));
        cx.subscribe(&query, |_this, _input, _ev: &TextInputEvent, cx| {
            cx.notify();
        })
        .detach();

        Self {
            selected: initial,
            open: false,
            query,
            creating: false,
            new_input,
            error: None,
            list_scroll: ScrollHandle::new(),
        }
    }

    pub fn selected(&self) -> Option<String> {
        self.selected.clone()
    }

    pub fn set_selected(&mut self, value: Option<String>, cx: &mut Context<Self>) {
        self.selected = value;
        self.open = false;
        self.creating = false;
        self.error = None;
        cx.notify();
    }

    pub fn set_error(&mut self, error: Option<String>, cx: &mut Context<Self>) {
        self.error = error;
        cx.notify();
    }

    fn submit_new_folder(&mut self, cx: &mut Context<Self>) {
        let name = self.new_input.read(cx).content.trim().to_string();
        if name.is_empty() {
            self.error = Some("分组名称不能为空".into());
            cx.notify();
            return;
        }
        match rterm_session::normalize_folder_path(&name) {
            Ok(path) => {
                self.error = None;
                cx.emit(GroupPickerEvent::CreateFolder(path));
            }
            Err(error) => {
                self.error = Some(format!("分组路径非法: {error}"));
                cx.notify();
            }
        }
    }
}

impl Render for GroupPicker {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        let label = self
            .selected
            .clone()
            .unwrap_or_else(|| "未分组（根目录）".to_string());
        let open = self.open;
        let creating = self.creating;

        let folders: Vec<String> = {
            let store = lock_or_recover(&cx.global::<AppState>().session_store);
            store.list_folders()
        };
        let query = self.query.read(cx).content.to_lowercase();
        let filtered: Vec<String> = folders
            .into_iter()
            .filter(|folder| query.is_empty() || folder.to_lowercase().contains(&query))
            .collect();

        let mut root = div().w_full().flex().flex_col().gap_1().child(
            div()
                .id("rterm-group-picker-toggle")
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
                    cx.listener(|this: &mut Self, _ev: &gpui::ClickEvent, window, cx| {
                        this.open = !this.open;
                        if this.open {
                            let query = this.query.clone();
                            window.focus(&query.read(cx).focus_handle(cx), cx);
                        }
                        cx.notify();
                    }),
                )
                .child(div().truncate().child(label))
                .child(
                    div()
                        .text_size(px(10.))
                        .text_color(t.text_subtle())
                        .child(if open { "▴" } else { "▾" }),
                ),
        );

        if open {
            let mut panel = div()
                .w_full()
                .p_1()
                .flex()
                .flex_col()
                .gap_1()
                .rounded_md()
                .border_1()
                .border_color(t.border_medium())
                .bg(t.card_bg())
                .child(self.query.clone());

            // 「未分组」
            panel = panel.child(self.render_option(
                "rterm-group-root",
                "未分组（根目录）".to_string(),
                self.selected.is_none(),
                None,
                cx,
            ));

            let mut rows = div().w_full().flex().flex_col();
            if filtered.is_empty() {
                rows = rows.child(
                    div()
                        .w_full()
                        .py_2()
                        .text_size(px(10.))
                        .text_color(t.text_subtle())
                        .flex()
                        .justify_center()
                        .child("没有匹配的分组"),
                );
            }
            for folder in &filtered {
                let path = folder.clone();
                rows = rows.child(self.render_option(
                    &format!("rterm-group-{folder}"),
                    folder.clone(),
                    self.selected.as_deref() == Some(folder.as_str()),
                    Some(path),
                    cx,
                ));
            }
            panel = panel.child(
                rows.id("rterm-group-list")
                    .max_h(px(132.))
                    .overflow_y_scroll()
                    .track_scroll(&self.list_scroll),
            );

            // 内联新建
            if creating {
                panel = panel.child(self.new_input.clone()).child(
                    div()
                        .w_full()
                        .flex()
                        .gap_1()
                        .child(self.render_action_button("创建", true, cx))
                        .child(self.render_action_button("取消", false, cx)),
                );
            } else {
                panel = panel.child(
                    div()
                        .id("rterm-group-create-toggle")
                        .w_full()
                        .h(px(26.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded_md()
                        .bg(t.secondary_btn_bg())
                        .text_size(px(11.))
                        .text_color(t.text_on_secondary_btn())
                        .cursor_pointer()
                        .hover(|style| style.bg(t.secondary_btn_hover()))
                        .on_click(cx.listener(
                            |this: &mut Self, _ev: &gpui::ClickEvent, window, cx| {
                                this.creating = true;
                                this.error = None;
                                let input = this.new_input.clone();
                                window.focus(&input.read(cx).focus_handle(cx), cx);
                                cx.notify();
                            },
                        ))
                        .child("＋ 新建分组…"),
                );
            }

            if let Some(error) = self.error.clone() {
                panel = panel.child(
                    div()
                        .w_full()
                        .text_size(px(10.))
                        .text_color(t.danger())
                        .child(error),
                );
            }

            root = root.child(panel);
        }

        root
    }
}

impl GroupPicker {
    fn render_option(
        &self,
        id: &str,
        label: String,
        selected: bool,
        value: Option<String>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = theme(cx);
        let bg = if selected {
            t.row_selected_bg()
        } else {
            t.card_bg()
        };
        div()
            .id(id.to_string())
            .w_full()
            .h(px(26.))
            .px_2()
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(4.))
            .bg(bg)
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
                    this.set_selected(value.clone(), cx);
                }),
            )
            .child(label)
            .into_any_element()
    }

    fn render_action_button(
        &self,
        label: &str,
        primary: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = theme(cx);
        let label_owned = label.to_string();
        let (bg, hover, fg) = if primary {
            (t.primary(), t.primary_hover(), t.text_on_primary_btn())
        } else {
            (
                t.secondary_btn_bg(),
                t.secondary_btn_hover(),
                t.text_on_secondary_btn(),
            )
        };
        let is_create = primary;
        let id = if is_create {
            "group-picker-create-confirm"
        } else {
            "group-picker-create-cancel"
        };
        div()
            .id(id)
            .h(px(26.))
            .px_3()
            .flex()
            .items_center()
            .justify_center()
            .rounded_md()
            .bg(bg)
            .text_size(px(11.))
            .text_color(fg)
            .cursor_pointer()
            .hover(move |style| style.bg(hover))
            .on_click(
                cx.listener(move |this: &mut Self, _ev: &gpui::ClickEvent, _w, cx| {
                    if is_create {
                        this.submit_new_folder(cx);
                    } else {
                        this.creating = false;
                        this.error = None;
                        cx.notify();
                    }
                }),
            )
            .child(label_owned)
            .into_any_element()
    }
}

// ---------------------------------------------------------------------------
// GroupPickerDialog — 移动到分组
// ---------------------------------------------------------------------------

pub struct GroupPickerDialog {
    session_id: Uuid,
    session_name: String,
    picker: Entity<GroupPicker>,
    focus_handle: FocusHandle,
    autofocus_pending: bool,
}

impl GroupPickerDialog {
    pub fn new(
        cx: &mut Context<Self>,
        session_id: Uuid,
        session_name: String,
        current_folder: Option<String>,
    ) -> Self {
        let picker = cx.new(|cx| GroupPicker::new(cx, current_folder));
        cx.subscribe(
            &picker,
            |this: &mut Self, _picker, event: &GroupPickerEvent, cx| match event {
                GroupPickerEvent::CreateFolder(path) => this.create_folder(path.clone(), cx),
            },
        )
        .detach();

        Self {
            session_id,
            session_name,
            picker,
            focus_handle: cx.focus_handle(),
            autofocus_pending: true,
        }
    }

    fn create_folder(&mut self, path: String, cx: &mut Context<Self>) {
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
                self.picker
                    .update(cx, |picker, cx| picker.set_selected(Some(path), cx));
                emit_folders_changed(cx);
            }
            Err(error) => {
                self.picker.update(cx, |picker, cx| {
                    picker.set_error(Some(format!("{error}")), cx)
                });
            }
        }
    }

    fn confirm(&mut self, cx: &mut Context<Self>) {
        let folder = self.picker.read(cx).selected();
        let session_id = self.session_id;
        emit_event(cx, AppEvent::SessionMoveRequested { session_id, folder });
    }

    fn cancel(&mut self, cx: &mut Context<Self>) {
        emit_event(cx, AppEvent::GroupPickerCloseRequested);
    }
}

impl Focusable for GroupPickerDialog {
    fn focus_handle(&self, _cx: &gpui::App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for GroupPickerDialog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        if self.autofocus_pending {
            window.focus(&self.focus_handle, cx);
            self.autofocus_pending = false;
        }
        let (card_w, card_h) = super::adaptive_dialog_size(window, 420.0, 300.0, 320.0, 220.0);

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
                    .h(px(card_h))
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
                            .flex_shrink_0()
                            .px_4()
                            .flex()
                            .items_center()
                            .bg(t.card_header_bg())
                            .border_b_1()
                            .border_color(t.border_subtle())
                            .text_size(px(13.))
                            .text_color(t.text_on_card_header_bg())
                            .child(format!("移动会话「{}」到分组", self.session_name)),
                    )
                    .child(
                        div()
                            .w_full()
                            .flex_1()
                            .min_h(px(0.))
                            .p_4()
                            .flex()
                            .flex_col()
                            .gap_2()
                            .child(self.picker.clone()),
                    )
                    .child(
                        div()
                            .w_full()
                            .h(px(50.))
                            .flex_shrink_0()
                            .px_4()
                            .flex()
                            .items_center()
                            .justify_center()
                            .gap_2()
                            .border_t_1()
                            .border_color(t.border_subtle())
                            .child(self.render_button("取消", false, cx))
                            .child(self.render_button("移动", true, cx)),
                    ),
            )
    }
}

impl GroupPickerDialog {
    fn render_button(&self, label: &str, primary: bool, cx: &mut Context<Self>) -> AnyElement {
        let t = theme(cx);
        let label_owned = label.to_string();
        let id = if primary {
            "group-dialog-confirm"
        } else {
            "group-dialog-cancel"
        };
        let (bg, hover, fg) = if primary {
            (t.primary(), t.primary_hover(), t.text_on_primary_btn())
        } else {
            (
                t.secondary_btn_bg(),
                t.secondary_btn_hover(),
                t.text_on_secondary_btn(),
            )
        };
        div()
            .id(id)
            .h(px(30.))
            .px_4()
            .flex()
            .items_center()
            .justify_center()
            .rounded_md()
            .bg(bg)
            .text_size(px(12.))
            .text_color(fg)
            .cursor_pointer()
            .hover(move |style| style.bg(hover))
            .on_click(
                cx.listener(move |this: &mut Self, _ev: &gpui::ClickEvent, _w, cx| {
                    if primary {
                        this.confirm(cx);
                    } else {
                        this.cancel(cx);
                    }
                }),
            )
            .child(label_owned)
            .into_any_element()
    }
}

/// 广播分组变更（树 / 导出对话框刷新）。
pub fn emit_folders_changed<T: 'static>(cx: &mut Context<T>) {
    emit_event(cx, AppEvent::SessionTreeFoldersChanged);
}

/// 通过 EventBus 广播应用事件。
fn emit_event<T: 'static>(cx: &mut Context<T>, event: AppEvent) {
    let host = cx.global::<AppState>().event_bus_host.clone();
    cx.update_entity(&host, |_host, cx: &mut Context<EventBusHost>| {
        cx.emit(event);
    });
}

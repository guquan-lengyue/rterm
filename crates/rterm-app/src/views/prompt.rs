//! prompt — 通用单行文本输入弹窗
//!
//! 用于「新建分组」「重命名分组」等简单输入场景；业务动作通过 EventBus
//! 交给 MainWindow，由数据层校验并落库。

use gpui::prelude::*;
use gpui::{div, px, Context, Entity, FocusHandle, Focusable, IntoElement, Render, Window};

use crate::event::{AppEvent, EventBusHost};
use crate::state::AppState;
use crate::ui::theme;

use super::text_input::TextInput;

#[derive(Clone, Debug)]
pub enum PromptAction {
    NewFolder,
    RenameFolder { from: String },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PromptKind {
    NewFolder,
    RenameFolder,
}

pub struct TextPromptDialog {
    title: String,
    label: String,
    action: PromptAction,
    input: Entity<TextInput>,
    focus_handle: FocusHandle,
    autofocus_pending: bool,
    error: Option<String>,
}

impl TextPromptDialog {
    pub fn new_folder(cx: &mut Context<Self>) -> Self {
        Self::new(
            cx,
            "新建分组".into(),
            "分组名称（支持 a/b 层级）".into(),
            PromptAction::NewFolder,
            String::new(),
        )
    }

    /// 在指定父分组下新建子分组（输入框预填 `父路径/`）。
    pub fn new_folder_under(cx: &mut Context<Self>, parent: String) -> Self {
        Self::new(
            cx,
            "新建子分组".into(),
            "子分组名称".into(),
            PromptAction::NewFolder,
            format!("{parent}/"),
        )
    }

    /// 展示校验错误（由 MainWindow 在分组重名 / 非法时回填）。
    pub fn set_error(&mut self, error: String, cx: &mut Context<Self>) {
        self.error = Some(error);
        cx.notify();
    }

    pub fn rename_folder(cx: &mut Context<Self>, from: String) -> Self {
        let initial = from.rsplit('/').next().unwrap_or(&from).to_string();
        Self::new(
            cx,
            "重命名分组".into(),
            "新的分组名称".into(),
            PromptAction::RenameFolder { from },
            initial,
        )
    }

    fn new(
        cx: &mut Context<Self>,
        title: String,
        label: String,
        action: PromptAction,
        initial: String,
    ) -> Self {
        let input = cx.new(|cx| TextInput::new(cx).with_content(initial));
        Self {
            title,
            label,
            action,
            input,
            focus_handle: cx.focus_handle(),
            autofocus_pending: true,
            error: None,
        }
    }

    fn kind(&self) -> PromptKind {
        match self.action {
            PromptAction::NewFolder => PromptKind::NewFolder,
            PromptAction::RenameFolder { .. } => PromptKind::RenameFolder,
        }
    }

    fn confirm(&mut self, cx: &mut Context<Self>) {
        let value = self.input.read(cx).content.trim().to_string();
        if value.is_empty() {
            self.error = Some("名称不能为空".into());
            cx.notify();
            return;
        }
        let event = match &self.action {
            PromptAction::NewFolder => AppEvent::FolderCreateRequested { path: value },
            PromptAction::RenameFolder { from } => AppEvent::FolderRenameRequested {
                from: from.clone(),
                to: value,
            },
        };
        self.emit(cx, event);
    }

    fn cancel(&mut self, cx: &mut Context<Self>) {
        self.emit(cx, AppEvent::TextPromptCloseRequested);
    }

    fn emit(&self, cx: &mut Context<Self>, event: AppEvent) {
        let host = cx.global::<AppState>().event_bus_host.clone();
        cx.update_entity(&host, |_host, cx: &mut Context<EventBusHost>| {
            cx.emit(event);
        });
    }
}

impl Focusable for TextPromptDialog {
    fn focus_handle(&self, _cx: &gpui::App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for TextPromptDialog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        if self.autofocus_pending {
            window.focus(&self.focus_handle, cx);
            self.autofocus_pending = false;
        }
        let (card_w, _) = super::adaptive_dialog_size(window, 420.0, 230.0, 320.0, 200.0);
        let kind = self.kind();

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
                    match event.keystroke.key.as_str() {
                        "escape" => this.cancel(cx),
                        "enter" => this.confirm(cx),
                        _ => {}
                    }
                }),
            )
            .child(
                div()
                    .w(px(card_w))
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
                            .child(self.title.clone()),
                    )
                    .child(
                        div()
                            .w_full()
                            .p_4()
                            .flex()
                            .flex_col()
                            .gap_2()
                            .child(
                                div()
                                    .text_size(px(11.))
                                    .text_color(t.text_subtle())
                                    .child(self.label.clone()),
                            )
                            .child(self.input.clone())
                            .child(
                                div()
                                    .text_size(px(10.))
                                    .text_color(t.danger())
                                    .child(self.error.clone().unwrap_or_default()),
                            ),
                    )
                    .child(
                        div()
                            .w_full()
                            .h(px(50.))
                            .px_4()
                            .flex()
                            .items_center()
                            .justify_center()
                            .gap_2()
                            .border_t_1()
                            .border_color(t.border_subtle())
                            .child(self.render_button("取消", false, cx))
                            .child(self.render_button(
                                if kind == PromptKind::NewFolder {
                                    "创建"
                                } else {
                                    "重命名"
                                },
                                true,
                                cx,
                            )),
                    ),
            )
    }
}

impl TextPromptDialog {
    fn render_button(
        &self,
        label: &str,
        primary: bool,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let t = theme(cx);
        let label_owned = label.to_string();
        let id = if primary {
            "text-prompt-confirm"
        } else {
            "text-prompt-cancel"
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

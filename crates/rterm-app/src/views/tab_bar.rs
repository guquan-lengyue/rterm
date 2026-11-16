//! TabBar — 终端 / SFTP / 监控标签栏（Sprint 6）
//!
//! 纯展示 View：MainWindow 通过 `set_tabs()` 推送当前标签列表，
//! 用户操作只 emit AppEvent（TabSelectRequested / TabCloseRequested / ...），
//! 由 MainWindow 统一管理 Tab 生命周期。

use gpui::prelude::*;
use gpui::{div, px, Context, IntoElement, MouseButton, MouseDownEvent, Render, Window};
use uuid::Uuid;

use crate::event::{AppEvent, EventBusHost};
use crate::state::{AppState, TabInfo, TabKind};
use crate::ui::theme;

use super::{CONTENT_BORDER_W, MENU_ITEM_H, SIDEBAR_W, TAB_BAR_H, TAB_MENU_W};

/// 右键菜单
#[derive(Clone, Copy)]
struct TabMenu {
    tab_id: Uuid,
    /// 相对 TabBar 左边缘的 x
    x: f32,
}

pub struct TabBar {
    tabs: Vec<TabInfo>,
    active: usize,
    menu: Option<TabMenu>,
}

impl TabBar {
    pub fn new() -> Self {
        Self {
            tabs: Vec::new(),
            active: 0,
            menu: None,
        }
    }

    pub fn set_tabs(&mut self, tabs: Vec<TabInfo>, active: usize, cx: &mut Context<Self>) {
        self.tabs = tabs;
        self.active = active;
        if self
            .menu
            .as_ref()
            .is_some_and(|menu| !self.tabs.iter().any(|tab| tab.id == menu.tab_id))
        {
            self.menu = None;
        }
        cx.notify();
    }

    fn emit(&self, cx: &mut Context<Self>, event: AppEvent) {
        let host: gpui::Entity<EventBusHost> = cx.global::<AppState>().event_bus_host.clone();
        cx.update_entity(&host, |_host, cx: &mut Context<EventBusHost>| {
            cx.emit(event);
        });
    }

    fn icon(kind: &TabKind) -> &'static str {
        match kind {
            TabKind::Terminal => "📟",
            TabKind::Sftp => "📁",
            TabKind::Monitor => "📊",
        }
    }
}

impl Render for TabBar {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        let active = self.active;

        let mut bar = div()
            .w_full()
            .h(px(TAB_BAR_H))
            .bg(t.toolbar_bg())
            .border_b_1()
            .border_color(t.border_subtle())
            .flex()
            .items_center()
            .id("rterm-tab-bar")
            .on_mouse_down_out(cx.listener(|this, _ev: &MouseDownEvent, _w, cx| {
                if this.menu.is_some() {
                    this.menu = None;
                    cx.notify();
                }
            }));

        if self.tabs.is_empty() {
            bar = bar.child(
                div()
                    .px_3()
                    .text_size(px(11.))
                    .text_color(t.text_placeholder())
                    .child("双击左侧会话打开终端"),
            );
        }

        for (index, tab) in self.tabs.iter().enumerate() {
            let is_active = index == active;
            let tab_id = tab.id;
            let title = format!("{} {}", Self::icon(&tab.kind), tab.title);
            let bg = if is_active {
                t.app_bg()
            } else {
                t.toolbar_bg()
            };
            let text_color = if is_active {
                t.text_on_app_bg()
            } else {
                t.text_subtle()
            };
            let hover_bg = t.row_hover_bg();
            let close_hover_bg = t.danger();
            let close_hover_text = t.text_on_primary_btn();

            let label = div()
                .h_full()
                .flex()
                .items_center()
                .px_3()
                .text_size(px(11.))
                .text_color(text_color)
                .bg(bg)
                .border_r_1()
                .border_color(t.border_subtle())
                .cursor_pointer()
                .id(format!("rterm-tab-{tab_id}"))
                .hover(move |style| style.bg(hover_bg))
                .on_click(cx.listener(move |this, _ev: &gpui::ClickEvent, _w, cx| {
                    this.emit(cx, AppEvent::TabSelectRequested { tab_id });
                }))
                .on_mouse_down(
                    MouseButton::Right,
                    cx.listener(move |this, ev: &MouseDownEvent, _w, cx| {
                        let x = (f32::from(ev.position.x) - SIDEBAR_W - CONTENT_BORDER_W).max(0.0);
                        this.menu = Some(TabMenu { tab_id, x });
                        cx.notify();
                    }),
                )
                .child(title)
                .child(
                    div()
                        .ml_2()
                        .w(px(16.))
                        .h(px(16.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded_sm()
                        .text_size(px(11.))
                        .text_color(t.text_placeholder())
                        .cursor_pointer()
                        .id(format!("rterm-tab-close-{tab_id}"))
                        .hover(move |style| style.bg(close_hover_bg).text_color(close_hover_text))
                        .on_click(cx.listener(move |this, _ev: &gpui::ClickEvent, _w, cx| {
                            this.emit(cx, AppEvent::TabCloseRequested { tab_id });
                        }))
                        .child("✕"),
                );

            bar = bar.child(label);
        }

        if let Some(menu) = self.menu {
            let mut items = div()
                .absolute()
                .left(px(menu.x))
                .top(px(TAB_BAR_H))
                .w(px(TAB_MENU_W))
                .py_1()
                .bg(t.card_bg())
                .border_1()
                .border_color(t.border_medium())
                .rounded_md()
                .flex()
                .flex_col()
                .id("rterm-tab-menu");

            let entries = ["关闭标签", "关闭其他", "关闭右侧"];
            for (index, label) in entries.into_iter().enumerate() {
                let tab_id = menu.tab_id;
                let color = t.text_on_card_bg();
                let hover = t.row_hover_bg();
                items = items.child(
                    div()
                        .w_full()
                        .h(px(MENU_ITEM_H))
                        .px_3()
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_size(px(12.))
                        .text_color(color)
                        .cursor_pointer()
                        .id(format!("rterm-tab-menu-item-{index}"))
                        .hover(move |style| style.bg(hover))
                        .on_click(cx.listener(move |this, _ev: &gpui::ClickEvent, _w, cx| {
                            let event = match index {
                                0 => AppEvent::TabCloseRequested { tab_id },
                                1 => AppEvent::TabCloseOthersRequested { tab_id },
                                _ => AppEvent::TabCloseRightRequested { tab_id },
                            };
                            this.emit(cx, event);
                            this.menu = None;
                            cx.notify();
                        }))
                        .child(label),
                );
            }
            bar = bar.child(items);
        }

        bar
    }
}

impl Default for TabBar {
    fn default() -> Self {
        Self::new()
    }
}

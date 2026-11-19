//! icon_picker — 会话预设图标选择
//!
//! 只使用常见 emoji，避免依赖图片资源；未设置 / 缺字时回退默认图标。
//! 选择器以网格形式复用：会话编辑对话框内联、右键「更改图标…」弹窗。

use gpui::prelude::*;
use gpui::{div, px, AnyElement, Context, IntoElement};

use crate::ui::theme;

/// 预设图标（顺序即网格顺序）。
pub const PRESET_ICONS: [&str; 12] = [
    "🖥️", "🐧", "🪟", "🍎", "☁️", "🗄️", "🌐", "🔧", "🐳", "🧪", "📦", "🚀",
];

/// 渲染 6 列图标网格。`id_prefix` 用于区分不同弹窗中的元素 id。
pub fn render_icon_grid<W: 'static>(
    id_prefix: &str,
    selected: &str,
    cx: &mut Context<W>,
    on_pick: impl Fn(&mut W, &'static str, &mut Context<W>) + Clone + 'static,
) -> AnyElement {
    let t = theme(cx);
    let prefix = id_prefix.to_string();
    let selected = selected.to_string();

    let mut grid = div().w_full().flex().flex_wrap().gap_1();
    for icon in PRESET_ICONS {
        let is_selected = icon == selected;
        let pick = on_pick.clone();
        let id = format!("{prefix}-icon-{icon}");
        grid = grid.child(
            div()
                .id(id)
                .w(px(34.))
                .h(px(30.))
                .flex()
                .items_center()
                .justify_center()
                .rounded_md()
                .border_1()
                .border_color(if is_selected {
                    t.primary()
                } else {
                    t.border_subtle()
                })
                .bg(if is_selected {
                    t.row_selected_bg()
                } else {
                    t.input_bg()
                })
                .text_size(px(15.))
                .cursor_pointer()
                .hover(|style| style.bg(t.row_hover_bg()))
                .on_click(cx.listener(move |this, _ev: &gpui::ClickEvent, _w, cx| {
                    pick(this, icon, cx);
                }))
                .child(icon),
        );
    }
    grid.into_any_element()
}

#[cfg(test)]
mod tests {
    use super::PRESET_ICONS;

    #[test]
    fn preset_icons_are_unique_and_non_empty() {
        let mut seen = std::collections::HashSet::new();
        for icon in PRESET_ICONS {
            assert!(!icon.is_empty());
            assert!(seen.insert(icon), "图标重复: {icon}");
        }
    }
}

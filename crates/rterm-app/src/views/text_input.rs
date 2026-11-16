//! TextInput — 简化版可复用单行文本输入框
//!
//! 参考 bezel-gpui 0.3 examples/input.rs 的 EntityInputHandler 模式，
//! 但只实现最核心的编辑能力：
//! - 字符输入（IME 通过 handle_input 自动处理）
//! - Backspace / Delete
//! - 密码模式（显示 ●●●）
//! - 焦点高亮
//!
//! 不支持：光标移动、选择、复制粘贴（后续迭代）

use std::ops::Range;

use gpui::prelude::*;
use gpui::{
    actions, App, Bounds, Context, CursorStyle, ElementId, ElementInputHandler, Entity,
    EntityInputHandler, FocusHandle, Focusable, GlobalElementId, Hsla, IntoElement, LayoutId,
    PaintQuad, Pixels, ShapedLine, SharedString, Style, TextRun, UTF16Selection, UnderlineStyle,
    Window, div, fill, point, px, relative, rgba, size,
};

// 键盘编辑动作。绑定在 main.rs 的 App::bind_keys。
// 注意：EntityInputHandler 只处理 IME 文本输入，Backspace/Delete 等
// 按键必须走 action + key binding。
actions!(
    rterm_text_input,
    [Backspace, Delete, Left, Right, Home, End, SelectAll]
);

// ---------------------------------------------------------------------------
// TextInput View
// ---------------------------------------------------------------------------

pub struct TextInput {
    focus_handle: FocusHandle,
    pub content: SharedString,
    pub placeholder: SharedString,
    pub selected_range: Range<usize>,
    selection_reversed: bool,
    marked_range: Option<Range<usize>>,
    pub password_mode: bool,
    last_layout: Option<ShapedLine>,
    last_bounds: Option<Bounds<Pixels>>,
}

impl TextInput {
    pub fn new(cx: &mut App) -> Self {
        Self {
            focus_handle: cx.focus_handle(),
            content: SharedString::default(),
            placeholder: SharedString::from(""),
            selected_range: 0..0,
            selection_reversed: false,
            marked_range: None,
            password_mode: false,
            last_layout: None,
            last_bounds: None,
        }
    }

    pub fn with_content(mut self, text: impl Into<SharedString>) -> Self {
        self.content = text.into();
        self
    }

    pub fn with_placeholder(mut self, text: impl Into<SharedString>) -> Self {
        self.placeholder = text.into();
        self
    }

    pub fn password(mut self) -> Self {
        self.password_mode = true;
        self
    }

    pub fn clear(&mut self, cx: &mut Context<Self>) {
        self.content = SharedString::default();
        self.selected_range = 0..0;
        self.marked_range = None;
        cx.notify();
    }

    /// 外部替换内容：同时清空 IME marked_range 与选区，避免越界。
    pub fn set_content(&mut self, text: impl Into<SharedString>, cx: &mut Context<Self>) {
        self.content = text.into();
        let end = self.content.len();
        self.selected_range = end..end;
        self.marked_range = None;
        cx.notify();
    }

    // ---------------- 键盘编辑（action handler） ----------------

    /// 用新文本替换当前选中区域（空选中 = 在光标处插入）。
    fn replace_selection(&mut self, new_text: &str, cx: &mut Context<Self>) {
        let range = clamp_range(self.selected_range.clone(), &self.content);
        let start = range.start;
        let end = range.end;
        self.content = SharedString::from(format!(
            "{}{}{}",
            &self.content[..start],
            new_text,
            &self.content[end..]
        ));
        let cursor = start + new_text.len();
        self.selected_range = cursor..cursor;
        self.marked_range = None;
        cx.notify();
    }

    fn prev_boundary(&self, offset: usize) -> usize {
        if offset == 0 {
            return 0;
        }
        let mut i = offset - 1;
        while i > 0 && !self.content.is_char_boundary(i) {
            i -= 1;
        }
        i
    }

    fn next_boundary(&self, offset: usize) -> usize {
        let len = self.content.len();
        if offset >= len {
            return len;
        }
        let mut i = offset + 1;
        while i < len && !self.content.is_char_boundary(i) {
            i += 1;
        }
        i
    }

    fn backspace(&mut self, _: &Backspace, _window: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            let cursor = self.cursor_offset();
            let prev = self.prev_boundary(cursor);
            if prev == cursor {
                return;
            }
            self.selected_range = prev..cursor;
        }
        self.replace_selection("", cx);
    }

    fn delete(&mut self, _: &Delete, _window: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            let cursor = self.cursor_offset();
            let next = self.next_boundary(cursor);
            if next == cursor {
                return;
            }
            self.selected_range = cursor..next;
        }
        self.replace_selection("", cx);
    }

    fn left(&mut self, _: &Left, _window: &mut Window, cx: &mut Context<Self>) {
        let cursor = self.prev_boundary(self.cursor_offset());
        self.selected_range = cursor..cursor;
        cx.notify();
    }

    fn right(&mut self, _: &Right, _window: &mut Window, cx: &mut Context<Self>) {
        let cursor = self.next_boundary(self.cursor_offset());
        self.selected_range = cursor..cursor;
        cx.notify();
    }

    fn home(&mut self, _: &Home, _window: &mut Window, cx: &mut Context<Self>) {
        self.selected_range = 0..0;
        cx.notify();
    }

    fn end(&mut self, _: &End, _window: &mut Window, cx: &mut Context<Self>) {
        let len = self.content.len();
        self.selected_range = len..len;
        cx.notify();
    }

    fn select_all(&mut self, _: &SelectAll, _window: &mut Window, cx: &mut Context<Self>) {
        self.selected_range = 0..self.content.len();
        cx.notify();
    }

    pub fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }

    fn display_content(&self) -> SharedString {
        if self.password_mode && !self.content.is_empty() {
            SharedString::from("●".repeat(self.content.chars().count()))
        } else {
            self.content.clone()
        }
    }

    fn cursor_offset(&self) -> usize {
        if self.selection_reversed {
            self.selected_range.start
        } else {
            self.selected_range.end
        }
    }

    fn offset_to_utf16(&self, offset: usize) -> usize {
        let mut utf16 = 0;
        let mut utf8 = 0;
        for ch in self.content.chars() {
            if utf8 >= offset {
                break;
            }
            utf8 += ch.len_utf8();
            utf16 += ch.len_utf16();
        }
        utf16
    }

    fn offset_from_utf16(&self, offset: usize) -> usize {
        let mut utf8 = 0;
        let mut utf16 = 0;
        for ch in self.content.chars() {
            if utf16 >= offset {
                break;
            }
            utf16 += ch.len_utf16();
            utf8 += ch.len_utf8();
        }
        utf8
    }
}

impl EntityInputHandler for TextInput {
    fn text_for_range(
        &mut self,
        range_utf16: Range<usize>,
        actual_range: &mut Option<Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        let start = self.offset_to_utf16(clamp_index(&self.content, self.offset_from_utf16(range_utf16.start)));
        let end = self.offset_to_utf16(clamp_index(&self.content, self.offset_from_utf16(range_utf16.end)));
        let (start, end) = if start <= end { (start, end) } else { (end, start) };
        actual_range.replace(start..end);
        let range = clamp_range(start..end, &self.content);
        Some(self.content[range].to_string())
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        let range = clamp_range(self.selected_range.clone(), &self.content);
        Some(UTF16Selection {
            range: self.offset_to_utf16(range.start)..self.offset_to_utf16(range.end),
            reversed: self.selection_reversed,
        })
    }

    fn marked_text_range(&self, _window: &mut Window, _cx: &mut Context<Self>) -> Option<Range<usize>> {
        self.marked_range
            .as_ref()
            .map(|r| clamp_range(r.clone(), &self.content))
            .map(|r| self.offset_to_utf16(r.start)..self.offset_to_utf16(r.end))
    }

    fn unmark_text(&mut self, _window: &mut Window, _cx: &mut Context<Self>) {
        self.marked_range = None;
    }

    fn replace_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let raw = range_utf16
            .as_ref()
            .map(|r| self.offset_from_utf16(r.start)..self.offset_from_utf16(r.end))
            .or(self.marked_range.clone())
            .unwrap_or(self.selected_range.clone());
        let range = clamp_range(raw, &self.content);

        self.content =
            SharedString::from(format!("{}{}{}", &self.content[..range.start], new_text, &self.content[range.end..]));
        let new_end = range.start + new_text.len();
        self.selected_range = new_end..new_end;
        self.marked_range = None;
        cx.notify();
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        _new_selected_range_utf16: Option<Range<usize>>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let raw = range_utf16
            .as_ref()
            .map(|r| self.offset_from_utf16(r.start)..self.offset_from_utf16(r.end))
            .or(self.marked_range.clone())
            .unwrap_or(self.selected_range.clone());
        let range = clamp_range(raw, &self.content);

        self.content =
            SharedString::from(format!("{}{}{}", &self.content[..range.start], new_text, &self.content[range.end..]));
        if !new_text.is_empty() {
            self.marked_range = Some(range.start..range.start + new_text.len());
        } else {
            self.marked_range = None;
        }
        let cursor = range.start + new_text.len();
        self.selected_range = cursor..cursor;
        cx.notify();
    }

    fn bounds_for_range(
        &mut self,
        range_utf16: Range<usize>,
        bounds: Bounds<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let line = self.last_layout.as_ref()?;
        // 密码模式显示的是 ●，与 content 下标不一致，跳过候选窗定位
        if self.password_mode {
            return None;
        }
        let start = clamp_index(&self.content, self.offset_from_utf16(range_utf16.start));
        let end = clamp_index(&self.content, self.offset_from_utf16(range_utf16.end));
        Some(Bounds::from_corners(
            point(bounds.left() + line.x_for_index(start), bounds.top()),
            point(bounds.left() + line.x_for_index(end), bounds.bottom()),
        ))
    }

    fn character_index_for_point(
        &mut self,
        point: gpui::Point<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        let line_point = self.last_bounds?.localize(&point)?;
        let line = self.last_layout.as_ref()?;
        let utf8_index = line.index_for_x(line_point.x)?;
        Some(self.offset_to_utf16(utf8_index))
    }
}

// ---------------------------------------------------------------------------
// 下标安全工具
// ---------------------------------------------------------------------------

/// 夹到 `text` 的字节长度内，并回退到最近的字符边界。
fn clamp_index(text: &str, index: usize) -> usize {
    let mut index = index.min(text.len());
    while index > 0 && !text.is_char_boundary(index) {
        index -= 1;
    }
    index
}

/// 夹住一个区间；保证 start <= end 且均为合法字符边界。
fn clamp_range(range: Range<usize>, text: &str) -> Range<usize> {
    let start = clamp_index(text, range.start);
    let end = clamp_index(text, range.end).max(start);
    start..end
}

#[cfg(test)]
mod tests {
    use super::{clamp_index, clamp_range};

    #[test]
    fn clamp_index_handles_overflow_and_char_boundaries() {
        assert_eq!(clamp_index("abc", 2), 2);
        assert_eq!(clamp_index("abc", 99), 3);
        // "你" 占 3 字节；落在中间时回退到字符起点
        assert_eq!(clamp_index("你好", 4), 3);
        assert_eq!(clamp_index("你好", 99), 6);
        assert_eq!(clamp_index("", 10), 0);
    }

    #[test]
    fn clamp_range_orders_and_bounds_ime_ranges() {
        assert_eq!(clamp_range(0..3, "abc"), 0..3);
        assert_eq!(clamp_range(5..2, "abc"), 3..3);
        assert_eq!(clamp_range(0..99, "abc"), 0..3);
        assert_eq!(clamp_range(99..100, "abc"), 3..3);
        // IME 标记落在多字节字符中间时也不会 panic
        assert_eq!(clamp_range(2..5, "你好"), 0..3);
    }
}

// ---------------------------------------------------------------------------
// TextElement — 自定义绘制 Element（处理 layout + paint）
// ---------------------------------------------------------------------------

struct PrepaintState {
    line: Option<ShapedLine>,
    cursor: Option<PaintQuad>,
    selection: Option<PaintQuad>,
}

struct TextElement {
    input: Entity<TextInput>,
}

impl IntoElement for TextElement {
    type Element = Self;
    fn into_element(self) -> Self { self }
}

impl Element for TextElement {
    type RequestLayoutState = ();
    type PrepaintState = PrepaintState;

    fn id(&self) -> Option<ElementId> { None }
    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> { None }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        style.size.height = window.line_height().into();
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        let input = self.input.read(cx);
        let content = input.display_content();
        // IME marked_range 记录的是 content 字节下标；password 模式 / 内容被外部替换后
        // 可能超出 display_text 长度，这里全部夹到合法字符边界，避免渲染越界 panic。
        let selected_range = clamp_range(input.selected_range.clone(), &content);
        let cursor = clamp_index(&content, input.cursor_offset());
        let style = window.text_style();

        // 从 AppState 拿主题色（TextInput 是 gpui::Global，prepaint 用 &mut App）
        let (text_color, placeholder_color, cursor_color) = {
            let state = cx.global::<crate::state::AppState>();
            let t = state.t();
            (t.text_on_card_bg(), t.text_placeholder(), t.primary())
        };

        let (display_text, color) = if content.is_empty() {
            (input.placeholder.clone(), placeholder_color)
        } else {
            (content, text_color)
        };
        let display_len = display_text.len();

        let run = TextRun {
            len: display_len,
            font: style.font(),
            color: color.into(),
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let marked_range = if input.content.is_empty() || input.password_mode {
            None
        } else {
            input
                .marked_range
                .as_ref()
                .map(|range| clamp_range(range.clone(), &display_text))
                .filter(|range| !range.is_empty())
        };
        let runs = if let Some(marked_range) = marked_range {
            vec![
                TextRun { len: marked_range.start, ..run.clone() },
                TextRun {
                    len: marked_range.end - marked_range.start,
                    underline: Some(UnderlineStyle {
                        color: Some(run.color),
                        thickness: px(1.0),
                        wavy: false,
                    }),
                    ..run.clone()
                },
                TextRun { len: display_len - marked_range.end, ..run },
            ]
            .into_iter()
            .filter(|run| run.len > 0)
            .collect()
        } else {
            vec![run]
        };

        let font_size = style.font_size.to_pixels(window.rem_size());
        let line = window.text_system().shape_line(display_text, font_size, &runs, None);
        let cursor_pos = line.x_for_index(cursor);

        let cursor_hsla: Hsla = cursor_color.into();

        let (selection, cursor) = if selected_range.is_empty() {
            (None, Some(fill(
                Bounds::new(
                    point(bounds.left() + cursor_pos, bounds.top()),
                    size(px(2.), bounds.bottom() - bounds.top()),
                ),
                cursor_hsla,
            )))
        } else {
            (Some(fill(
                Bounds::from_corners(
                    point(bounds.left() + line.x_for_index(selected_range.start), bounds.top()),
                    point(bounds.left() + line.x_for_index(selected_range.end), bounds.bottom()),
                ),
                rgba(0x3311ff30),
            )), None)
        };

        PrepaintState { line: Some(line), cursor, selection }
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let focus_handle = self.input.read(cx).focus_handle.clone();
        window.handle_input(&focus_handle, ElementInputHandler::new(bounds, self.input.clone()), cx);
        if let Some(sel) = prepaint.selection.take() {
            window.paint_quad(sel);
        }
        let line = prepaint.line.take().unwrap();
        let _ = line.paint(bounds.origin, window.line_height(), gpui::TextAlign::Left, None, window, cx);

        if focus_handle.is_focused(window) {
            if let Some(cur) = prepaint.cursor.take() {
                window.paint_quad(cur);
            }
        }

        self.input.update(cx, |input, _cx| {
            input.last_layout = Some(line);
            input.last_bounds = Some(bounds);
        });
    }
}

// ---------------------------------------------------------------------------
// Render impl
// ---------------------------------------------------------------------------

impl Focusable for TextInput {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for TextInput {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let is_focused = self.focus_handle.is_focused(window);

        // 主题色从 AppState 拿
        let (border_idle, border_focused, input_bg) = {
            let state = cx.global::<crate::state::AppState>();
            let t = state.t();
            (t.border_input_idle(), t.border_input_focused(), t.input_bg())
        };
        let border_color = if is_focused { border_focused } else { border_idle };

        div()
            .w_full()
            .h(px(32.))
            .id(format!("rterm-ti-{}", cx.entity_id()))
            .key_context("TextInput")
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::backspace))
            .on_action(cx.listener(Self::delete))
            .on_action(cx.listener(Self::left))
            .on_action(cx.listener(Self::right))
            .on_action(cx.listener(Self::home))
            .on_action(cx.listener(Self::end))
            .on_action(cx.listener(Self::select_all))
            .cursor(CursorStyle::IBeam)
            .border_1()
            .border_color(border_color)
            .bg(input_bg)
            .rounded_md()
            .child(
                div()
                    .w_full()
                    .h(px(30.))
                    .px_2()
                    .child(TextElement { input: cx.entity() }),
            )
    }
}

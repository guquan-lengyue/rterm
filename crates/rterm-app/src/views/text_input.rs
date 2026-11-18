//! TextInput — 简化版可复用单行文本输入框
//!
//! 参考 bezel-gpui 0.3 examples/input.rs 的 EntityInputHandler 模式，
//! 但只实现最核心的编辑能力：
//! - 字符输入（IME 通过 handle_input 自动处理）
//! - Backspace / Delete / 方向键 / Home / End / 全选
//! - Ctrl+V 粘贴
//! - 鼠标点击定位光标
//! - 密码模式（显示 ●●●）
//! - 焦点高亮

use std::ops::Range;

use gpui::prelude::*;
use gpui::{
    actions, div, fill, point, px, relative, size, App, Bounds, Context, CursorStyle, ElementId,
    ElementInputHandler, Entity, EntityInputHandler, FocusHandle, Focusable, GlobalElementId, Hsla,
    IntoElement, LayoutId, MouseButton, MouseDownEvent, PaintQuad, Pixels, ShapedLine,
    SharedString, Style, TextRun, UTF16Selection, UnderlineStyle, Window,
};

// 键盘编辑动作。绑定在 main.rs 的 App::bind_keys。
// 注意：EntityInputHandler 只处理 IME 文本输入，Backspace/Delete 等
// 按键必须走 action + key binding。
actions!(
    rterm_text_input,
    [Backspace, Delete, Left, Right, Home, End, SelectAll, Paste]
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

    /// Ctrl+V：读取剪贴板文本并替换当前选区（无选区则在光标处插入）。
    fn paste(&mut self, _: &Paste, _window: &mut Window, cx: &mut Context<Self>) {
        let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) else {
            return;
        };
        if text.is_empty() {
            return;
        }
        self.replace_selection(&text, cx);
    }

    /// 鼠标左键：按点击位置（字符 x 坐标）定位光标。
    ///
    /// 处理要点：
    /// - GPUI 的 `index_for_x` 超出文本时返回 None → 光标放到末尾
    /// - 点击字符右半部分时 → 光标前进到下一个字符边界
    /// - 密码模式在"显示坐标"里定位（● 串），再换算回明文字节偏移
    fn on_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.focus(&self.focus_handle, cx);

        let (Some(bounds), Some(line)) = (self.last_bounds, self.last_layout.as_ref()) else {
            return;
        };
        let Some(local) = bounds.localize(&event.position) else {
            return;
        };

        let display = self.display_content();
        let display_len = display.len();

        // 1) 拿候选索引；index_for_x 返回超范围（None）→ 放末尾
        let hit = match line.index_for_x(local.x) {
            Some(index) => clamp_index(&display, index),
            None => display_len,
        };

        // 2) 取当前边界与下一个边界的 x 中点，点击落在右半 → 前进一格
        let display_index = if hit >= display_len {
            display_len
        } else {
            let current_x = f32::from(line.x_for_index(hit));
            let next = next_char_boundary(&display, hit);
            let next_x = f32::from(line.x_for_index(next));
            if f32::from(local.x) > (current_x + next_x) / 2.0 {
                next
            } else {
                hit
            }
        };

        let index = self.plain_offset(display_index);
        self.selected_range = index..index;
        self.marked_range = None;
        cx.notify();
    }

    pub fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }

    fn display_content(&self) -> SharedString {
        if self.password_mode && !self.content.is_empty() {
            SharedString::from(PASSWORD_MASK.repeat(self.content.chars().count()))
        } else {
            self.content.clone()
        }
    }

    /// 明文字节偏移 → 显示文本字节偏移（密码模式按 ● 个数换算）。
    fn display_offset(&self, byte_offset: usize) -> usize {
        to_display_offset(&self.content, self.password_mode, byte_offset)
    }

    /// 显示文本字节偏移 → 明文字节偏移（密码模式把 ● 个数还原为字符数）。
    fn plain_offset(&self, display_offset: usize) -> usize {
        to_plain_offset(&self.content, self.password_mode, display_offset)
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
        let start = self.offset_to_utf16(clamp_index(
            &self.content,
            self.offset_from_utf16(range_utf16.start),
        ));
        let end = self.offset_to_utf16(clamp_index(
            &self.content,
            self.offset_from_utf16(range_utf16.end),
        ));
        let (start, end) = if start <= end {
            (start, end)
        } else {
            (end, start)
        };
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

    fn marked_text_range(
        &self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Range<usize>> {
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

        self.content = SharedString::from(format!(
            "{}{}{}",
            &self.content[..range.start],
            new_text,
            &self.content[range.end..]
        ));
        let new_end = range.start + new_text.len();
        self.selected_range = new_end..new_end;
        self.marked_range = None;
        cx.notify();
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        new_selected_range_utf16: Option<Range<usize>>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let raw = range_utf16
            .as_ref()
            .map(|r| self.offset_from_utf16(r.start)..self.offset_from_utf16(r.end))
            .or(self.marked_range.clone())
            .unwrap_or(self.selected_range.clone());
        let range = clamp_range(raw, &self.content);

        self.content = SharedString::from(format!(
            "{}{}{}",
            &self.content[..range.start],
            new_text,
            &self.content[range.end..]
        ));
        if !new_text.is_empty() {
            self.marked_range = Some(range.start..range.start + new_text.len());
        } else {
            self.marked_range = None;
        }
        // new_selected_range 是相对 new_text 的 UTF-16 偏移，需转成
        // 相对 content 的 UTF-8 字节偏移（注意多字节 / 代理对边界）。
        let selected_range = new_selected_range_utf16
            .as_ref()
            .map(|r| {
                let start = range.start + utf16_to_utf8_offset(new_text, r.start);
                let end = range.start + utf16_to_utf8_offset(new_text, r.end);
                start..end
            })
            .unwrap_or_else(|| {
                let cursor = range.start + new_text.len();
                cursor..cursor
            });
        self.selected_range = selected_range;
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

/// 密码模式的占位符（每个明文字符显示一个）。
const PASSWORD_MASK: &str = "●";

/// 明文字节偏移 → 显示文本字节偏移。
///
/// 密码模式显示的是 `●●●`，每个占位符占 `PASSWORD_MASK.len()` 字节，
/// 需要按"字符数"换算，否则光标会停在第一个圆点后面。
fn to_display_offset(content: &str, password_mode: bool, byte_offset: usize) -> usize {
    let clamped = clamp_index(content, byte_offset);
    if !password_mode {
        return clamped;
    }
    content[..clamped].chars().count() * PASSWORD_MASK.len()
}

/// 显示文本字节偏移 → 明文字节偏移（[`to_display_offset`] 的逆运算）。
fn to_plain_offset(content: &str, password_mode: bool, display_offset: usize) -> usize {
    if !password_mode {
        return clamp_index(content, display_offset);
    }
    let chars = display_offset / PASSWORD_MASK.len();
    content
        .char_indices()
        .nth(chars)
        .map(|(index, _)| index)
        .unwrap_or(content.len())
}

/// 夹到 `text` 的字节长度内，并回退到最近的字符边界。
fn clamp_index(text: &str, index: usize) -> usize {
    let mut index = index.min(text.len());
    while index > 0 && !text.is_char_boundary(index) {
        index -= 1;
    }
    index
}

/// `offset` 之后的下一个字符边界（已到末尾则返回 `text.len()`）。
fn next_char_boundary(text: &str, offset: usize) -> usize {
    let len = text.len();
    let mut index = offset.min(len);
    if index >= len {
        return len;
    }
    index += 1;
    while index < len && !text.is_char_boundary(index) {
        index += 1;
    }
    index
}

/// 夹住一个区间；保证 start <= end 且均为合法字符边界。
fn clamp_range(range: Range<usize>, text: &str) -> Range<usize> {
    let start = clamp_index(text, range.start);
    let end = clamp_index(text, range.end).max(start);
    start..end
}

/// UTF-16 偏移 → UTF-8 字节偏移（相对同一段文本）。
/// 偏移落在代理对中间时回退到该字符起点。
fn utf16_to_utf8_offset(text: &str, utf16_offset: usize) -> usize {
    let mut utf16 = 0;
    let mut utf8 = 0;
    for ch in text.chars() {
        let next = utf16 + ch.len_utf16();
        // 偏移落在字符中间（代理对）时停在字符起点
        if next > utf16_offset {
            break;
        }
        utf16 = next;
        utf8 += ch.len_utf8();
    }
    utf8
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
    fn into_element(self) -> Self {
        self
    }
}

impl Element for TextElement {
    type RequestLayoutState = ();
    type PrepaintState = PrepaintState;

    fn id(&self) -> Option<ElementId> {
        None
    }
    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

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
        // 选区 / 光标都换算到"显示文本"坐标：密码模式下显示的是 ● 串，
        // 与明文 content 的字节偏移不一致，直接复用会画错位置。
        let content_len = content.len();
        let cursor = input.display_offset(input.cursor_offset()).min(content_len);
        let sel_start = input
            .display_offset(input.selected_range.start)
            .min(content_len);
        let sel_end = input
            .display_offset(input.selected_range.end)
            .min(content_len);
        let (sel_start, sel_end) = if sel_start <= sel_end {
            (sel_start, sel_end)
        } else {
            (sel_end, sel_start)
        };
        let is_focused = input.focus_handle.is_focused(window);
        let style = window.text_style();

        // 从 AppState 拿主题色（TextInput 是 gpui::Global，prepaint 用 &mut App）
        let (text_color, placeholder_color, cursor_color, selection_color) = {
            let state = cx.global::<crate::state::AppState>();
            let t = state.t();
            (
                t.text_on_card_bg(),
                t.text_placeholder(),
                t.primary(),
                t.text_selection_bg(),
            )
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
                TextRun {
                    len: marked_range.start,
                    ..run.clone()
                },
                TextRun {
                    len: marked_range.end - marked_range.start,
                    underline: Some(UnderlineStyle {
                        color: Some(run.color),
                        thickness: px(1.0),
                        wavy: false,
                    }),
                    ..run.clone()
                },
                TextRun {
                    len: display_len - marked_range.end,
                    ..run
                },
            ]
            .into_iter()
            .filter(|run| run.len > 0)
            .collect()
        } else {
            vec![run]
        };

        let font_size = style.font_size.to_pixels(window.rem_size());
        let line = window
            .text_system()
            .shape_line(display_text, font_size, &runs, None);
        let cursor_pos = line.x_for_index(cursor);

        let cursor_hsla: Hsla = cursor_color.into();

        // 失焦时既不画光标也不画选区高亮：否则选中文字后点到别处，
        // 蓝色高亮会残留在输入框里。
        let (selection, cursor) = if !is_focused {
            (None, None)
        } else if sel_start == sel_end {
            (
                None,
                Some(fill(
                    Bounds::new(
                        point(bounds.left() + cursor_pos, bounds.top()),
                        size(px(2.), bounds.bottom() - bounds.top()),
                    ),
                    cursor_hsla,
                )),
            )
        } else {
            (
                Some(fill(
                    Bounds::from_corners(
                        point(bounds.left() + line.x_for_index(sel_start), bounds.top()),
                        point(bounds.left() + line.x_for_index(sel_end), bounds.bottom()),
                    ),
                    selection_color,
                )),
                None,
            )
        };

        PrepaintState {
            line: Some(line),
            cursor,
            selection,
        }
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
        window.handle_input(
            &focus_handle,
            ElementInputHandler::new(bounds, self.input.clone()),
            cx,
        );
        if let Some(sel) = prepaint.selection.take() {
            window.paint_quad(sel);
        }
        let line = prepaint.line.take().unwrap();
        let _ = line.paint(
            bounds.origin,
            window.line_height(),
            gpui::TextAlign::Left,
            None,
            window,
            cx,
        );

        if let Some(cur) = prepaint.cursor.take() {
            window.paint_quad(cur);
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
            (
                t.border_input_idle(),
                t.border_input_focused(),
                t.input_bg(),
            )
        };
        let border_color = if is_focused {
            border_focused
        } else {
            border_idle
        };

        div()
            .w_full()
            .h(px(32.))
            .overflow_hidden()
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
            .on_action(cx.listener(Self::paste))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
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

#[cfg(test)]
mod tests {
    use super::{
        clamp_index, clamp_range, next_char_boundary, to_display_offset, to_plain_offset,
        utf16_to_utf8_offset,
    };

    #[test]
    fn password_offsets_map_between_plain_and_display() {
        // 明文 3 个 ASCII → 3 个 ●，每个 ● 占 3 字节
        assert_eq!(to_display_offset("abc", true, 0), 0);
        assert_eq!(to_display_offset("abc", true, 2), 6);
        assert_eq!(to_display_offset("abc", true, 3), 9);
        assert_eq!(to_plain_offset("abc", true, 0), 0);
        assert_eq!(to_plain_offset("abc", true, 6), 2);
        assert_eq!(to_plain_offset("abc", true, 9), 3);
    }

    #[test]
    fn password_offsets_handle_multibyte_and_overflow() {
        // 中文：1 字符 = 3 字节明文，显示仍是 1 个 ●（3 字节）
        assert_eq!(to_display_offset("你好", true, 3), 3);
        assert_eq!(to_plain_offset("你好", true, 3), 3);
        // emoji 占 1 个 UTF-16 代理对 / 4 字节，显示仍是 1 个 ●
        assert_eq!(to_display_offset("😀", true, 4), 3);
        assert_eq!(to_plain_offset("😀", true, 3), 4);
        // 超出范围收敛到末尾
        assert_eq!(to_display_offset("ab", true, 99), 6);
        assert_eq!(to_plain_offset("ab", true, 99), 2);
        // 非密码模式恒等
        assert_eq!(to_display_offset("abc", false, 2), 2);
        assert_eq!(to_plain_offset("abc", false, 2), 2);
    }

    #[test]
    fn next_char_boundary_steps_over_multibyte_chars() {
        assert_eq!(next_char_boundary("abc", 1), 2);
        assert_eq!(next_char_boundary("abc", 3), 3);
        assert_eq!(next_char_boundary("你好", 0), 3);
        assert_eq!(next_char_boundary("你好", 3), 6);
        assert_eq!(next_char_boundary("", 0), 0);
    }

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
        // 反向 range：用变量构造，避免 clippy::reversed_empty_ranges 直接命中字面量
        let (start, end) = (5usize, 2usize);
        assert_eq!(clamp_range(start..end, "abc"), 3..3);
        assert_eq!(clamp_range(0..99, "abc"), 0..3);
        assert_eq!(clamp_range(99..100, "abc"), 3..3);
        // IME 标记落在多字节字符中间时也不会 panic
        assert_eq!(clamp_range(2..5, "你好"), 0..3);
    }

    #[test]
    fn utf16_offset_maps_to_utf8_bytes() {
        assert_eq!(utf16_to_utf8_offset("abc", 2), 2);
        // 中文每字 1 个 UTF-16 单元、3 个字节
        assert_eq!(utf16_to_utf8_offset("你好", 1), 3);
        assert_eq!(utf16_to_utf8_offset("你好", 2), 6);
        // emoji 是代理对：2 个 UTF-16 单元、4 个字节
        assert_eq!(utf16_to_utf8_offset("😀", 2), 4);
        // 落在代理对中间 → 回退到字符起点
        assert_eq!(utf16_to_utf8_offset("😀", 1), 0);
        assert_eq!(utf16_to_utf8_offset("", 3), 0);
    }
}

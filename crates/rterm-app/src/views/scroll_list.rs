//! ScrollList — gpui 0.3 没有内建滚动容器，这里自绘最小可用版本。
//!
//! 职责分离：
//! - [`ScrollMetrics`]：纯几何状态（偏移 / 可视区间 / 滑块位置），不依赖 gpui 渲染，可单测
//! - [`scroll_list`]：只把 metrics 翻译成元素树（视口裁剪 + 内容位移 + 滚动条 + 滚轮回调）
//!
//! 采用定高行虚拟化：每帧只渲染可视区内的行，行数再多也不影响布局成本。

use gpui::prelude::*;
use gpui::{div, px, AnyElement, App, ElementId, Rgba, ScrollWheelEvent, Window};

/// 滚动条宽度（视口右侧覆盖绘制，不挤占内容宽度）。
pub(crate) const SCROLL_TRACK_W: f32 = 4.0;

// ---------------------------------------------------------------------------
// 纯状态
// ---------------------------------------------------------------------------

/// 定高列表的滚动状态。
#[derive(Clone, Copy, Debug, Default)]
pub struct ScrollMetrics {
    offset: f32,
    viewport_h: f32,
    row_h: f32,
    row_count: usize,
}

impl ScrollMetrics {
    pub fn new(viewport_h: f32, row_h: f32) -> Self {
        Self {
            offset: 0.0,
            viewport_h: viewport_h.max(1.0),
            row_h: row_h.max(1.0),
            row_count: 0,
        }
    }

    pub fn offset(&self) -> f32 {
        self.offset
    }

    pub fn row_count(&self) -> usize {
        self.row_count
    }

    pub fn content_h(&self) -> f32 {
        self.row_h * self.row_count as f32
    }

    /// 最大偏移；内容不足一屏时为 0（不出滚动条）。
    pub fn max_offset(&self) -> f32 {
        (self.content_h() - self.viewport_h).max(0.0)
    }

    /// 更新行数并把偏移收进合法范围。返回 offset 是否变化。
    pub fn set_rows(&mut self, count: usize) -> bool {
        self.row_count = count;
        self.clamp()
    }

    fn clamp(&mut self) -> bool {
        let clamped = self.offset.clamp(0.0, self.max_offset());
        let changed = (clamped - self.offset).abs() > f32::EPSILON;
        self.offset = clamped;
        changed
    }

    /// 按像素滚动（正数 = 内容向上，即滚轮向下）。返回是否真的滚动。
    pub fn scroll_by(&mut self, delta: f32) -> bool {
        if delta == 0.0 || self.max_offset() <= 0.0 {
            return false;
        }
        let before = self.offset;
        self.offset += delta;
        self.clamp();
        (self.offset - before).abs() > f32::EPSILON
    }

    /// 处理一次滚轮事件（行模式按 `row_h` 折算成像素）。
    /// gpui 的滚轮 delta 向上为正，与内容偏移方向相反。
    pub fn on_wheel(&mut self, event: &ScrollWheelEvent) -> bool {
        let delta = event.delta.pixel_delta(px(self.row_h));
        self.scroll_by(-f32::from(delta.y))
    }

    /// 需要渲染的行区间 `[start, end)`；行数为 0 时返回 `(0, 0)`。
    pub fn visible(&self) -> (usize, usize) {
        if self.row_count == 0 {
            return (0, 0);
        }
        let start = (self.offset / self.row_h).floor().max(0.0) as usize;
        let start = start.min(self.row_count - 1);
        // +1 覆盖上下两侧被裁掉一半的行。
        let rows = (self.viewport_h / self.row_h).ceil() as usize + 1;
        (start, (start + rows).min(self.row_count))
    }

    /// 滚动条滑块 `(top, len)`（相对视口顶部）；内容不溢出时返回 None。
    pub fn thumb(&self) -> Option<(f32, f32)> {
        if self.max_offset() <= 0.0 {
            return None;
        }
        let ratio = (self.viewport_h / self.content_h()).clamp(0.05, 1.0);
        let len = (self.viewport_h * ratio).max(18.0);
        let travel = (self.viewport_h - len).max(0.0);
        let top = self.offset / self.max_offset() * travel;
        Some((top, len))
    }
}

// ---------------------------------------------------------------------------
// 渲染
// ---------------------------------------------------------------------------

/// 固定高度可滚动列表：滚轮滚动 + 视口裁剪 + 右侧滚动条。
///
/// - `metrics`：当前帧状态快照（调用方持有，滚轮回调里更新后 `cx.notify()`）
/// - `on_wheel`：通常传 `cx.listener(...)`
/// - `render_row`：只对可视行调用，返回定高行的内容
pub fn scroll_list(
    id: impl Into<ElementId>,
    metrics: ScrollMetrics,
    track_color: Rgba,
    thumb_color: Rgba,
    on_wheel: impl Fn(&ScrollWheelEvent, &mut Window, &mut App) + 'static,
    render_row: impl Fn(usize) -> AnyElement,
) -> AnyElement {
    let (start, end) = metrics.visible();
    // 首行在视口里的位置：start 行顶部相对内容顶部的偏移，再减去已滚距离。
    let first_top = start as f32 * metrics.row_h - metrics.offset();

    let mut list = div()
        .id(id)
        .relative()
        .w_full()
        .h(px(metrics.viewport_h))
        .flex_none()
        .overflow_hidden()
        .on_scroll_wheel(on_wheel)
        .child(
            div()
                .absolute()
                .left_0()
                .top(px(first_top))
                .right(px(SCROLL_TRACK_W + 2.0))
                .flex()
                .flex_col()
                .children((start..end).map(|index| {
                    div()
                        .w_full()
                        .h(px(metrics.row_h))
                        .flex_none()
                        .child(render_row(index))
                })),
        );

    if let Some((top, len)) = metrics.thumb() {
        list = list
            .child(
                div()
                    .absolute()
                    .right(px(1.0))
                    .top_0()
                    .w(px(SCROLL_TRACK_W))
                    .h_full()
                    .rounded_full()
                    .bg(track_color),
            )
            .child(
                div()
                    .absolute()
                    .right(px(1.0))
                    .top(px(top))
                    .w(px(SCROLL_TRACK_W))
                    .h(px(len))
                    .rounded_full()
                    .bg(thumb_color),
            );
    }

    list.into_any_element()
}

// ---------------------------------------------------------------------------
// 测试
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn metrics(viewport_h: f32, row_h: f32, rows: usize) -> ScrollMetrics {
        let mut m = ScrollMetrics::new(viewport_h, row_h);
        m.set_rows(rows);
        m
    }

    #[test]
    fn scroll_and_visible_range() {
        let mut m = metrics(100.0, 20.0, 10);
        assert_eq!(m.content_h(), 200.0);
        assert_eq!(m.max_offset(), 100.0);
        assert_eq!(m.visible(), (0, 6));

        assert!(m.scroll_by(40.0));
        assert_eq!(m.offset(), 40.0);
        assert_eq!(m.visible(), (2, 8));

        // 越过底部会被夹住
        assert!(m.scroll_by(1000.0));
        assert_eq!(m.offset(), 100.0);
        assert_eq!(m.visible(), (5, 10));
    }

    #[test]
    fn scroll_up_stops_at_zero() {
        let mut m = metrics(100.0, 20.0, 10);
        assert!(!m.scroll_by(-30.0));
        assert_eq!(m.offset(), 0.0);
    }

    #[test]
    fn set_rows_clamps_offset() {
        let mut m = metrics(100.0, 20.0, 10);
        m.scroll_by(80.0);
        assert_eq!(m.offset(), 80.0);

        // 行数减少 → max_offset 变小，offset 收紧
        assert!(m.set_rows(5));
        assert_eq!(m.max_offset(), 0.0);
        assert_eq!(m.offset(), 0.0);
        assert_eq!(m.visible(), (0, 5));
        assert!(m.thumb().is_none());
    }

    #[test]
    fn thumb_geometry_follows_offset() {
        let mut m = metrics(100.0, 20.0, 10);
        let (top, len) = m.thumb().expect("内容溢出应出现滑块");
        assert!((len - 50.0).abs() < 0.01);
        assert!(top.abs() < 0.01);

        m.scroll_by(100.0);
        let (top, _) = m.thumb().unwrap();
        assert!((top - 50.0).abs() < 0.01);
    }

    #[test]
    fn empty_and_fitting_lists_have_no_scrollbar() {
        let empty = metrics(100.0, 20.0, 0);
        assert_eq!(empty.visible(), (0, 0));
        assert!(empty.thumb().is_none());

        let mut fitting = metrics(100.0, 20.0, 3);
        assert!(!fitting.scroll_by(10.0));
        assert!(fitting.thumb().is_none());
    }
}

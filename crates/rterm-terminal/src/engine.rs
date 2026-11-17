//! TerminalEngine — alacritty_terminal glue
//!
//! 职责：
//! - 把 PTY 字节流喂给 `vte` 解析器 → `Term`
//! - 暴露 Grid / Cursor / Colors 供 View 渲染
//! - 把按键映射成 xterm 字节序列
//!
//! `Term` 是 `!Send + !Sync`，所以本类型只能在 gpui 主线程创建和使用。

use alacritty_terminal::event::{Event, EventListener};
use alacritty_terminal::grid::{Dimensions, Grid, Scroll};
use alacritty_terminal::index::{Column, Line};
use alacritty_terminal::term::cell::{Cell, Flags};
use alacritty_terminal::term::color::Colors;
use alacritty_terminal::term::{Config, RenderableCursor, Term, TermMode};
use alacritty_terminal::vte::ansi::{Processor, StdSyncHandler};

// ---------------------------------------------------------------------------
// EventProxy / SizeInfo
// ---------------------------------------------------------------------------

/// 终端内部事件（标题变化 / 剪贴板请求等）。
///
/// `PtyWrite`（DSR / DA 等查询应答）需要通过 `responses` 写回 PTY；
/// `ClipboardStore`（OSC 52 远程复制）推入共享队列，由 View 取出写入系统剪贴板。
/// 其余事件仍只记录 debug 日志。含 `Sender` 后不再实现 `Copy`。
#[derive(Clone, Debug, Default)]
pub struct EventProxy {
    responses: Option<std::sync::mpsc::Sender<Vec<u8>>>,
    clipboard: Option<std::rc::Rc<std::cell::RefCell<Vec<String>>>>,
}

impl EventListener for EventProxy {
    fn send_event(&self, event: Event) {
        match event {
            Event::PtyWrite(text) => {
                if let Some(tx) = &self.responses {
                    let _ = tx.send(text.into_bytes());
                }
            }
            Event::ClipboardStore(_kind, text) => {
                if let Some(queue) = &self.clipboard {
                    queue.borrow_mut().push(text);
                }
            }
            other => log::debug!("[terminal] event: {other:?}"),
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct SizeInfo {
    pub cols: usize,
    pub rows: usize,
}

impl SizeInfo {
    pub fn new(cols: usize, rows: usize) -> Self {
        Self {
            cols: cols.max(1),
            rows: rows.max(1),
        }
    }
}

impl Dimensions for SizeInfo {
    fn total_lines(&self) -> usize {
        self.rows
    }

    fn screen_lines(&self) -> usize {
        self.rows
    }

    fn columns(&self) -> usize {
        self.cols
    }
}

// ---------------------------------------------------------------------------
// TerminalKey
// ---------------------------------------------------------------------------

/// View 层按键 → xterm 序列。字符输入走 `Char`。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TerminalKey {
    Char(String),
    Enter,
    Tab,
    Backspace,
    Escape,
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
    PageUp,
    PageDown,
    Delete,
    Insert,
    F(u8),
    Ctrl(char),
    Alt(String),
    CtrlUp,
    CtrlDown,
    CtrlLeft,
    CtrlRight,
}

/// vi 模式的移动动作（只暴露 View 需要的子集）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ViMove {
    Up,
    Down,
    Left,
    Right,
    WordLeft,
    WordRight,
    Home,
    End,
    Top,
    Bottom,
}

impl From<ViMove> for alacritty_terminal::vi_mode::ViMotion {
    fn from(value: ViMove) -> Self {
        use alacritty_terminal::vi_mode::ViMotion;
        match value {
            ViMove::Up => ViMotion::Up,
            ViMove::Down => ViMotion::Down,
            ViMove::Left => ViMotion::Left,
            ViMove::Right => ViMotion::Right,
            ViMove::WordLeft => ViMotion::SemanticLeft,
            ViMove::WordRight => ViMotion::SemanticRight,
            ViMove::Home => ViMotion::First,
            ViMove::End => ViMotion::Last,
            ViMove::Top => ViMotion::High,
            ViMove::Bottom => ViMotion::Low,
        }
    }
}

// ---------------------------------------------------------------------------
// TerminalEngine
// ---------------------------------------------------------------------------

pub struct TerminalEngine {
    term: Term<EventProxy>,
    parser: Processor<StdSyncHandler>,
    cols: usize,
    rows: usize,
    scrollback: usize,
    /// OSC 52 远程复制请求（由 EventProxy 推入，View 取出写系统剪贴板）。
    clipboard_writes: std::rc::Rc<std::cell::RefCell<Vec<String>>>,
    /// 注入命令的回显抑制状态（9.21），None = 不做过滤。
    pending_echo: Option<EchoSuppression>,
    /// 上一次抑制被新注入打断时暂存的字节，下一次喂数据时先放行。
    deferred_output: Vec<u8>,
}

/// 等待从 PTY 输出里吞掉的一段回显字节。
struct EchoSuppression {
    expected: Vec<u8>,
    buffered: Vec<u8>,
}

/// 抑制窗口上限：超过后放弃匹配（避免异常输出被无限缓冲）。
const ECHO_BUFFER_LIMIT: usize = 512;

impl TerminalEngine {
    /// 注入命令前登记要在输出里吞掉的回显（不含结尾回车）。
    ///
    /// 远端 shell 会把 `cd '/path'` 原样回显，这里在下一次输出到达时把这
    /// 段字节从流中删掉，界面上就不会出现注入命令的输入行。
    pub fn suppress_echo(&mut self, echo: Vec<u8>) {
        if echo.is_empty() {
            return;
        }
        // 上一条注入还没等到回显就来了新注入：旧缓冲原样放行，避免丢输出。
        if let Some(previous) = self.pending_echo.take() {
            self.deferred_output.extend_from_slice(&previous.buffered);
        }
        self.pending_echo = Some(EchoSuppression {
            expected: echo,
            buffered: Vec::new(),
        });
    }

    pub fn new(cols: usize, rows: usize, scrollback: usize) -> Self {
        Self::build(cols, rows, scrollback, None)
    }

    /// 带应答通道的构造：终端要写回 PTY 的字节（DSR / DA 等）通过
    /// `responses` 发送，View 侧从对应 Receiver 取出后写回 PTY。
    pub fn with_response_sender(
        cols: usize,
        rows: usize,
        scrollback: usize,
        responses: std::sync::mpsc::Sender<Vec<u8>>,
    ) -> Self {
        Self::build(cols, rows, scrollback, Some(responses))
    }

    fn build(
        cols: usize,
        rows: usize,
        scrollback: usize,
        responses: Option<std::sync::mpsc::Sender<Vec<u8>>>,
    ) -> Self {
        let size = SizeInfo::new(cols, rows);
        let config = Config {
            scrolling_history: scrollback,
            // OSC 52 只允许 copy（远程 → 本地），不允许远程读取本地剪贴板。
            osc52: alacritty_terminal::term::Osc52::OnlyCopy,
            ..Default::default()
        };
        let clipboard_writes = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let term = Term::new(
            config,
            &size,
            EventProxy {
                responses,
                clipboard: Some(clipboard_writes.clone()),
            },
        );
        Self {
            term,
            parser: Processor::new(),
            cols: size.cols,
            rows: size.rows,
            scrollback,
            clipboard_writes,
            pending_echo: None,
            deferred_output: Vec::new(),
        }
    }

    /// 取出并清空 OSC 52 的剪贴板写入请求。
    pub fn take_clipboard_writes(&mut self) -> Vec<String> {
        std::mem::take(&mut *self.clipboard_writes.borrow_mut())
    }

    /// 是否处于 vi 模式。
    pub fn vi_mode(&self) -> bool {
        self.term.mode().contains(TermMode::VI)
    }

    /// 进入 / 退出 vi 模式（退出时历史滚动由 alacritty 归零）。
    pub fn toggle_vi_mode(&mut self) {
        self.term.toggle_vi_mode();
    }

    /// vi 光标在视口内的 (row, col)；非 vi 模式返回 None。
    pub fn vi_cursor(&self) -> Option<(usize, usize)> {
        if !self.vi_mode() {
            return None;
        }
        let point = self.term.vi_mode_cursor.point;
        let row = point.line.0 + self.term.grid().display_offset() as i32;
        if row < 0 || row as usize >= self.rows {
            return None;
        }
        Some((row as usize, point.column.0))
    }

    /// vi 模式移动光标（超出视口时 alacritty 会自动滚动历史）。
    pub fn vi_move(&mut self, motion: ViMove) {
        self.term.vi_motion(motion.into());
    }

    /// 喂 PTY 输出字节。
    ///
    /// 有待抑制的注入回显时先做过滤（9.21）：匹配到的字节直接丢弃，
    /// 其余字节照常进入解析器；等不到回显时最多缓冲 512 字节后放弃。
    pub fn input_bytes(&mut self, bytes: &[u8]) {
        let mut incoming = std::mem::take(&mut self.deferred_output);
        incoming.extend_from_slice(bytes);
        if self.pending_echo.is_none() {
            self.parser.advance(&mut self.term, &incoming);
            return;
        }

        let mut outgoing: Option<Vec<u8>> = None;
        {
            let suppression = self.pending_echo.as_mut().expect("pending_echo 已判空");
            suppression.buffered.extend_from_slice(&incoming);
            if let Some(position) = find_subslice(&suppression.buffered, &suppression.expected) {
                let mut output = std::mem::take(&mut suppression.buffered);
                output.drain(position..position + suppression.expected.len());
                outgoing = Some(output);
            } else if suppression.buffered.len() > suppression.expected.len() + ECHO_BUFFER_LIMIT {
                outgoing = Some(std::mem::take(&mut suppression.buffered));
            }
        }
        if let Some(output) = outgoing {
            self.pending_echo = None;
            self.parser.advance(&mut self.term, &output);
        }
    }

    pub fn resize(&mut self, cols: usize, rows: usize) {
        let size = SizeInfo::new(cols, rows);
        self.term.resize(size);
        self.cols = size.cols;
        self.rows = size.rows;
    }

    pub fn grid(&self) -> &Grid<Cell> {
        self.term.grid()
    }

    pub fn cursor(&self) -> RenderableCursor {
        self.term.renderable_content().cursor
    }

    pub fn colors(&self) -> &Colors {
        self.term.renderable_content().colors
    }

    pub fn cols(&self) -> usize {
        self.cols
    }

    pub fn rows(&self) -> usize {
        self.rows
    }

    pub fn scrollback(&self) -> usize {
        self.scrollback
    }

    /// Grid 中真实的历史行数（不包含当前视口）。滚动条 / 回滚范围以此为准，
    /// 配置的 `scrollback` 只是上限。
    pub fn history_size(&self) -> usize {
        self.term.grid().history_size()
    }

    /// 当前视口文本（每行去掉尾部空格）。S5 placeholder 渲染用；
    /// S6 Canvas 渲染会直接读 Grid。
    pub fn visible_lines(&self) -> Vec<String> {
        let grid = self.term.grid();
        let display_offset = grid.display_offset() as i32;
        (0..self.rows)
            .map(|row| {
                let line = Line(row as i32 - display_offset);
                let cells = &grid[line];
                let mut text = String::with_capacity(self.cols);
                for col in 0..self.cols {
                    let cell = &cells[Column(col)];
                    if cell
                        .flags
                        .intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER)
                    {
                        continue;
                    }
                    text.push(cell.c);
                }
                text.trim_end_matches(' ').to_string()
            })
            .collect()
    }

    /// 光标在视口中的 (row, col)；行号已加 display_offset。
    pub fn cursor_position(&self) -> (usize, usize) {
        let cursor = self.cursor();
        let row = cursor.point.line.0 + self.term.grid().display_offset() as i32;
        (row.max(0) as usize, cursor.point.column.0)
    }

    /// 按键 → xterm 字节序列。S5 不回显到本地 Grid，交给远程 PTY echo。
    ///
    /// DECCKM（`TermMode::APP_CURSOR`）下方向键 / Home / End 使用 SS3 序列，
    /// 否则用普通 CSI 序列。
    pub fn input_key(&mut self, key: TerminalKey) -> Vec<u8> {
        let app_cursor = self.mode().contains(TermMode::APP_CURSOR);
        match key {
            TerminalKey::Char(text) => text.into_bytes(),
            TerminalKey::Enter => b"\r".to_vec(),
            TerminalKey::Tab => b"\t".to_vec(),
            TerminalKey::Backspace => vec![0x7f],
            TerminalKey::Escape => vec![0x1b],
            TerminalKey::Up => {
                if app_cursor {
                    b"\x1bOA".to_vec()
                } else {
                    b"\x1b[A".to_vec()
                }
            }
            TerminalKey::Down => {
                if app_cursor {
                    b"\x1bOB".to_vec()
                } else {
                    b"\x1b[B".to_vec()
                }
            }
            TerminalKey::Right => {
                if app_cursor {
                    b"\x1bOC".to_vec()
                } else {
                    b"\x1b[C".to_vec()
                }
            }
            TerminalKey::Left => {
                if app_cursor {
                    b"\x1bOD".to_vec()
                } else {
                    b"\x1b[D".to_vec()
                }
            }
            TerminalKey::Home => {
                if app_cursor {
                    b"\x1bOH".to_vec()
                } else {
                    b"\x1b[H".to_vec()
                }
            }
            TerminalKey::End => {
                if app_cursor {
                    b"\x1bOF".to_vec()
                } else {
                    b"\x1b[F".to_vec()
                }
            }
            TerminalKey::PageUp => b"\x1b[5~".to_vec(),
            TerminalKey::PageDown => b"\x1b[6~".to_vec(),
            TerminalKey::Delete => b"\x1b[3~".to_vec(),
            TerminalKey::Insert => b"\x1b[2~".to_vec(),
            TerminalKey::F(n) => f_key_sequence(n),
            TerminalKey::Ctrl(c) => vec![(c as u8) & 0x1f],
            TerminalKey::Alt(text) => {
                let mut bytes = Vec::with_capacity(text.len() + 1);
                bytes.push(0x1b);
                bytes.extend_from_slice(text.as_bytes());
                bytes
            }
            TerminalKey::CtrlUp => b"\x1b[1;5A".to_vec(),
            TerminalKey::CtrlDown => b"\x1b[1;5B".to_vec(),
            TerminalKey::CtrlRight => b"\x1b[1;5C".to_vec(),
            TerminalKey::CtrlLeft => b"\x1b[1;5D".to_vec(),
        }
    }

    // -----------------------------------------------------------------------
    // Sprint 6：滚动 / 模式 / 选区文本
    // -----------------------------------------------------------------------

    pub fn mode(&self) -> TermMode {
        *self.term.mode()
    }

    /// 终端是否请求了 bracketed paste（DECSET 2004）。
    pub fn bracketed_paste(&self) -> bool {
        self.mode().contains(TermMode::BRACKETED_PASTE)
    }

    /// 滚动历史缓冲：正数向上（看历史），负数向下。
    pub fn scroll_display(&mut self, delta: i32) {
        if delta != 0 {
            self.term.scroll_display(Scroll::Delta(delta));
        }
    }

    pub fn scroll_page(&mut self, up: bool) {
        self.term
            .scroll_display(if up { Scroll::PageUp } else { Scroll::PageDown });
    }

    /// 回到最底部（有新输入时调用）。
    pub fn scroll_bottom(&mut self) {
        self.term.scroll_display(Scroll::Bottom);
    }

    pub fn display_offset(&self) -> usize {
        self.term.grid().display_offset()
    }

    /// 把视口滚动到指定历史偏移（0 = 底部）。滚动条拖拽用。
    pub fn set_display_offset(&mut self, offset: usize) {
        let offset = offset
            .min(self.term.grid().history_size())
            .min(i32::MAX as usize);
        let current = self.term.grid().display_offset() as i32;
        let target = offset as i32;
        if target != current {
            self.term.scroll_display(Scroll::Delta(target - current));
        }
    }

    /// 双击选词：返回与 `(row, col)` 同属一个空白分隔词的视口区间（含端点）。
    /// 点中空白时返回 None。
    pub fn word_range(&self, row: usize, col: usize) -> Option<((usize, usize), (usize, usize))> {
        let chars = self.row_chars(row)?;
        let (start, end) = word_bounds(&chars, col)?;
        Some(((row, start), (row, end)))
    }

    /// 三击整行：返回该行从第 0 列到最后一个非空白字符的区间。
    pub fn line_range(&self, row: usize) -> Option<((usize, usize), (usize, usize))> {
        let chars = self.row_chars(row)?;
        let (start, end) = line_bounds(&chars)?;
        Some(((row, start), (row, end)))
    }

    /// 视口某行的所有字符（宽字符 spacer 归一为空格）。
    fn row_chars(&self, row: usize) -> Option<Vec<char>> {
        if row >= self.rows {
            return None;
        }
        let line = Line(row as i32 - self.display_offset() as i32);
        let cells = &self.term.grid()[line];
        Some((0..self.cols).map(|col| cells[Column(col)].c).collect())
    }

    /// 视口坐标 → Grid Cell。行 0 = 屏幕顶部，已考虑 display_offset。
    pub fn cell_at(&self, row: usize, col: usize) -> Option<&Cell> {
        if row >= self.rows || col >= self.cols {
            return None;
        }
        let line = Line(row as i32 - self.display_offset() as i32);
        Some(&self.term.grid()[line][Column(col)])
    }

    /// 视口坐标区间 `(row, col)` 的文本（含端点），每行去尾部空格，
    /// 行间用 `\n` 连接。宽字符只取一次，跳过 spacer。
    pub fn text_in_range(&self, start: (usize, usize), end: (usize, usize)) -> String {
        let (start, end) = if start <= end {
            (start, end)
        } else {
            (end, start)
        };

        let mut rows = Vec::new();
        for row in start.0..=end.0.min(self.rows.saturating_sub(1)) {
            let line = Line(row as i32 - self.display_offset() as i32);
            let cells = &self.term.grid()[line];
            let from = if row == start.0 { start.1 } else { 0 };
            let to = if row == end.0 {
                end.1.min(self.cols.saturating_sub(1))
            } else {
                self.cols.saturating_sub(1)
            };

            let mut text = String::new();
            for col in from..=to {
                let cell = &cells[Column(col)];
                if cell
                    .flags
                    .intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER)
                {
                    continue;
                }
                text.push(cell.c);
            }
            rows.push(text.trim_end_matches(' ').to_string());
        }
        rows.join("\n")
    }
}

fn f_key_sequence(n: u8) -> Vec<u8> {
    match n {
        1 => b"\x1bOP".to_vec(),
        2 => b"\x1bOQ".to_vec(),
        3 => b"\x1bOR".to_vec(),
        4 => b"\x1bOS".to_vec(),
        5 => b"\x1b[15~".to_vec(),
        6 => b"\x1b[17~".to_vec(),
        7 => b"\x1b[18~".to_vec(),
        8 => b"\x1b[19~".to_vec(),
        9 => b"\x1b[20~".to_vec(),
        10 => b"\x1b[21~".to_vec(),
        11 => b"\x1b[23~".to_vec(),
        12 => b"\x1b[24~".to_vec(),
        _ => Vec::new(),
    }
}

// ---------------------------------------------------------------------------
// 选词边界（纯函数，便于单测）
// ---------------------------------------------------------------------------

/// 以空白为分隔符，返回 `col` 所在词的 `[start, end]` 列区间；点中空白返回 None。
fn word_bounds(chars: &[char], col: usize) -> Option<(usize, usize)> {
    let is_word = |ch: char| !ch.is_whitespace() && ch != '\0';
    let clicked = *chars.get(col)?;
    if !is_word(clicked) {
        return None;
    }
    let mut start = col;
    while start > 0 && is_word(chars[start - 1]) {
        start -= 1;
    }
    let mut end = col;
    while end + 1 < chars.len() && is_word(chars[end + 1]) {
        end += 1;
    }
    Some((start, end))
}

/// 整行选区：第 0 列到最后一个非空白字符；全空白行返回 None。
fn line_bounds(chars: &[char]) -> Option<(usize, usize)> {
    let last = chars.iter().rposition(|ch| !ch.is_whitespace())?;
    Some((0, last))
}

/// 在 `haystack` 中查找 `needle` 第一次出现的下标；空 needle 返回 None。
fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_does_not_panic() {
        let engine = TerminalEngine::new(80, 24, 10_000);
        assert_eq!(engine.cols(), 80);
        assert_eq!(engine.rows(), 24);
    }

    #[test]
    fn osc52_copy_is_forwarded() {
        let mut engine = TerminalEngine::new(80, 24, 1_000);
        // base64("hello") = aGVsbG8=
        engine.input_bytes(b"\x1b]52;c;aGVsbG8=\x07");
        assert_eq!(engine.take_clipboard_writes(), vec!["hello".to_string()]);
        // 取走后再取应为空
        assert!(engine.take_clipboard_writes().is_empty());
    }

    #[test]
    fn vi_mode_toggles_and_moves() {
        let mut engine = TerminalEngine::new(20, 4, 100);
        for index in 0..10 {
            engine.input_bytes(format!("line {index}\r\n").as_bytes());
        }
        assert!(!engine.vi_mode());
        assert!(engine.vi_cursor().is_none());

        engine.toggle_vi_mode();
        assert!(engine.vi_mode());
        let start = engine.vi_cursor().expect("vi 光标存在");

        engine.vi_move(ViMove::Top);
        let top = engine.vi_cursor().expect("顶部仍有光标");
        assert!(top.0 <= start.0, "Top 不应让屏幕行变大");

        engine.toggle_vi_mode();
        assert!(!engine.vi_mode());
        assert!(engine.vi_cursor().is_none());
    }

    #[test]
    fn input_bytes_lands_in_grid() {
        let mut engine = TerminalEngine::new(80, 24, 1_000);
        engine.input_bytes(b"hello\r\n");
        let lines = engine.visible_lines();
        assert_eq!(lines[0], "hello");
    }

    #[test]
    fn word_and_line_bounds_cover_clicked_token() {
        let chars: Vec<char> = "git commit -m msg".chars().collect();
        assert_eq!(word_bounds(&chars, 0), Some((0, 2)));
        assert_eq!(word_bounds(&chars, 5), Some((4, 9)));
        assert_eq!(word_bounds(&chars, 3), None, "空格不算词");
        assert_eq!(word_bounds(&chars, 999), None, "越界返回 None");
        assert_eq!(line_bounds(&chars), Some((0, 16)));
        assert_eq!(line_bounds(&[' ', ' ', ' ']), None, "全空白行不选区");
    }

    #[test]
    fn word_and_line_range_from_engine() {
        let mut engine = TerminalEngine::new(40, 5, 100);
        engine.input_bytes(b"echo hello world");
        assert_eq!(engine.word_range(0, 7), Some(((0, 5), (0, 9))));
        assert_eq!(engine.word_range(0, 0), Some(((0, 0), (0, 3))));
        assert_eq!(engine.line_range(0), Some(((0, 0), (0, 15))));
        assert_eq!(engine.line_range(4), None, "全空白行返回 None");
    }

    #[test]
    fn set_display_offset_clamps_to_history() {
        let mut engine = TerminalEngine::new(20, 4, 100);
        for index in 0..30 {
            engine.input_bytes(format!("line {index}\r\n").as_bytes());
        }
        let history = engine.history_size();
        assert!(history > 0);

        engine.set_display_offset(3);
        assert_eq!(engine.display_offset(), 3);
        engine.set_display_offset(0);
        assert_eq!(engine.display_offset(), 0);
        // 超出上限时由 alacritty 夹到最大历史
        engine.set_display_offset(usize::MAX);
        assert_eq!(engine.display_offset(), history);
    }

    #[test]
    fn find_subslice_reports_position() {
        assert_eq!(find_subslice(b"abcde", b"cd"), Some(2));
        assert_eq!(find_subslice(b"abcde", b"zz"), None);
        assert_eq!(find_subslice(b"ab", b"abc"), None);
        assert_eq!(find_subslice(b"abc", b""), None);
    }

    #[test]
    fn echoed_injected_command_is_swallowed_across_chunks() {
        let mut engine = TerminalEngine::new(40, 5, 100);
        engine.suppress_echo(b"cd '/tmp'".to_vec());
        // 回显被拆成两个 PTY 读块：先缓冲，等拼齐再吞掉
        engine.input_bytes(b"cd '/t");
        assert_eq!(engine.visible_lines()[0], "");
        engine.input_bytes(b"mp'\r\n$ ");
        let lines = engine.visible_lines();
        assert_eq!(lines[0], "", "注入命令的回显不应出现在第一行");
        assert_eq!(lines[1], "$", "回车与提示符照常显示");
    }

    #[test]
    fn echo_suppression_gives_up_on_unrelated_output() {
        let mut engine = TerminalEngine::new(80, 5, 100);
        engine.suppress_echo(b"cd '/tmp'".to_vec());
        let noise = vec![b'x'; ECHO_BUFFER_LIMIT + 32];
        engine.input_bytes(&noise);
        assert_eq!(engine.visible_lines()[0].len(), 80, "超限后原样放行输出");
    }

    #[test]
    fn ansi_colors_and_cursor_moves() {
        let mut engine = TerminalEngine::new(40, 10, 1_000);
        engine.input_bytes(b"\x1b[31mred\x1b[0m\r\nworld");
        let lines = engine.visible_lines();
        assert_eq!(lines[0], "red");
        assert_eq!(lines[1], "world");
        let (row, col) = engine.cursor_position();
        assert_eq!(row, 1);
        assert_eq!(col, 5);
    }

    #[test]
    fn resize_updates_grid_dimensions() {
        let mut engine = TerminalEngine::new(80, 24, 1_000);
        engine.input_bytes(b"hello");
        engine.resize(120, 30);
        assert_eq!(engine.cols(), 120);
        assert_eq!(engine.rows(), 30);
        assert_eq!(engine.visible_lines().len(), 30);
        assert_eq!(engine.visible_lines()[0], "hello");
    }

    #[test]
    fn input_key_sequences() {
        let mut engine = TerminalEngine::new(80, 24, 1_000);
        assert_eq!(engine.input_key(TerminalKey::Enter), b"\r");
        assert_eq!(engine.input_key(TerminalKey::Backspace), vec![0x7f]);
        assert_eq!(engine.input_key(TerminalKey::Up), b"\x1b[A");
        assert_eq!(engine.input_key(TerminalKey::Ctrl('c')), vec![3]);
        assert_eq!(
            engine.input_key(TerminalKey::Char("pwd".into())),
            b"pwd".to_vec()
        );
    }

    #[test]
    fn modified_key_sequences() {
        let mut engine = TerminalEngine::new(80, 24, 1_000);
        assert_eq!(engine.input_key(TerminalKey::Alt("f".into())), b"\x1bf");
        assert_eq!(engine.input_key(TerminalKey::CtrlUp), b"\x1b[1;5A");
        assert_eq!(engine.input_key(TerminalKey::CtrlLeft), b"\x1b[1;5D");
    }

    #[test]
    fn text_in_range_reads_visible_rows() {
        let mut engine = TerminalEngine::new(20, 5, 100);
        engine.input_bytes(b"hello\r\nworld");
        assert_eq!(engine.text_in_range((0, 0), (0, 4)), "hello");
        assert_eq!(engine.text_in_range((0, 0), (1, 4)), "hello\nworld");
        assert_eq!(engine.text_in_range((0, 2), (0, 9)), "llo");
        // 反向区间自动归一化
        assert_eq!(engine.text_in_range((1, 4), (0, 0)), "hello\nworld");
    }

    #[test]
    fn scrollback_changes_display_offset() {
        let mut engine = TerminalEngine::new(10, 3, 100);
        for i in 0..10 {
            engine.input_bytes(format!("line{i}\r\n").as_bytes());
        }
        assert_eq!(engine.display_offset(), 0);
        engine.scroll_display(2);
        assert!(engine.display_offset() > 0);
        engine.scroll_bottom();
        assert_eq!(engine.display_offset(), 0);
    }

    #[test]
    fn bracketed_paste_tracks_terminal_mode() {
        let mut engine = TerminalEngine::new(10, 3, 100);
        assert!(!engine.bracketed_paste());
        engine.input_bytes(b"\x1b[?2004h");
        assert!(engine.bracketed_paste());
        engine.input_bytes(b"\x1b[?2004l");
        assert!(!engine.bracketed_paste());
    }

    #[test]
    fn history_size_tracks_real_scrollback() {
        let mut engine = TerminalEngine::new(10, 3, 100);
        assert_eq!(engine.history_size(), 0);
        for i in 0..10 {
            engine.input_bytes(format!("line{i}\r\n").as_bytes());
        }
        assert!(engine.history_size() > 0);
        // 新引擎没有历史
        assert_eq!(TerminalEngine::new(10, 3, 100).history_size(), 0);
    }

    #[test]
    fn app_cursor_mode_uses_ss3_sequences() {
        let mut engine = TerminalEngine::new(80, 24, 1_000);
        // 默认（DECCKM 关闭）为 CSI
        assert_eq!(engine.input_key(TerminalKey::Up), b"\x1b[A");
        assert_eq!(engine.input_key(TerminalKey::Home), b"\x1b[H");
        // DECSET ?1h 开启应用光标模式
        engine.input_bytes(b"\x1b[?1h");
        assert_eq!(engine.input_key(TerminalKey::Up), b"\x1bOA");
        assert_eq!(engine.input_key(TerminalKey::Down), b"\x1bOB");
        assert_eq!(engine.input_key(TerminalKey::Right), b"\x1bOC");
        assert_eq!(engine.input_key(TerminalKey::Left), b"\x1bOD");
        assert_eq!(engine.input_key(TerminalKey::Home), b"\x1bOH");
        assert_eq!(engine.input_key(TerminalKey::End), b"\x1bOF");
        // DECRST ?1l 恢复 CSI
        engine.input_bytes(b"\x1b[?1l");
        assert_eq!(engine.input_key(TerminalKey::Up), b"\x1b[A");
    }

    #[test]
    fn dsr_query_emits_pty_response() {
        let (tx, rx) = std::sync::mpsc::channel();
        let mut engine = TerminalEngine::with_response_sender(80, 24, 1_000, tx);
        engine.input_bytes(b"\x1b[6n");
        let response = rx.try_recv().expect("DSR 应产生 PtyWrite 应答");
        let text = String::from_utf8(response).unwrap();
        assert!(
            text.starts_with("\x1b["),
            "unexpected DSR response: {text:?}"
        );
        assert!(text.ends_with('R'), "unexpected DSR response: {text:?}");
    }
}

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

/// 终端内部事件（标题变化 / 剪贴板请求等）。S5 只记录日志，
/// S6 渲染 / S10 OSC 52 再接管。
///
/// `PtyWrite`（DSR / DA 等查询应答）需要通过 `responses` 写回 PTY；
/// 其余事件仍只记录 debug 日志。含 `Sender` 后不再实现 `Copy`。
#[derive(Clone, Debug, Default)]
pub struct EventProxy {
    responses: Option<std::sync::mpsc::Sender<Vec<u8>>>,
}

impl EventListener for EventProxy {
    fn send_event(&self, event: Event) {
        match event {
            Event::PtyWrite(text) => {
                if let Some(tx) = &self.responses {
                    let _ = tx.send(text.into_bytes());
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

// ---------------------------------------------------------------------------
// TerminalEngine
// ---------------------------------------------------------------------------

pub struct TerminalEngine {
    term: Term<EventProxy>,
    parser: Processor<StdSyncHandler>,
    cols: usize,
    rows: usize,
    scrollback: usize,
}

impl TerminalEngine {
    pub fn new(cols: usize, rows: usize, scrollback: usize) -> Self {
        let size = SizeInfo::new(cols, rows);
        let config = Config {
            scrolling_history: scrollback,
            ..Default::default()
        };
        let term = Term::new(config, &size, EventProxy::default());
        Self {
            term,
            parser: Processor::new(),
            cols: size.cols,
            rows: size.rows,
            scrollback,
        }
    }

    /// 带应答通道的构造：终端要写回 PTY 的字节（DSR / DA 等）通过
    /// `responses` 发送，View 侧从对应 Receiver 取出后写回 PTY。
    pub fn with_response_sender(
        cols: usize,
        rows: usize,
        scrollback: usize,
        responses: std::sync::mpsc::Sender<Vec<u8>>,
    ) -> Self {
        let size = SizeInfo::new(cols, rows);
        let config = Config {
            scrolling_history: scrollback,
            ..Default::default()
        };
        let term = Term::new(
            config,
            &size,
            EventProxy {
                responses: Some(responses),
            },
        );
        Self {
            term,
            parser: Processor::new(),
            cols: size.cols,
            rows: size.rows,
            scrollback,
        }
    }

    /// 喂 PTY 输出字节。
    pub fn input_bytes(&mut self, bytes: &[u8]) {
        self.parser.advance(&mut self.term, bytes);
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
        self.term.scroll_display(if up {
            Scroll::PageUp
        } else {
            Scroll::PageDown
        });
    }

    /// 回到最底部（有新输入时调用）。
    pub fn scroll_bottom(&mut self) {
        self.term.scroll_display(Scroll::Bottom);
    }

    pub fn display_offset(&self) -> usize {
        self.term.grid().display_offset()
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
    pub fn text_in_range(
        &self,
        start: (usize, usize),
        end: (usize, usize),
    ) -> String {
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
    fn input_bytes_lands_in_grid() {
        let mut engine = TerminalEngine::new(80, 24, 1_000);
        engine.input_bytes(b"hello\r\n");
        let lines = engine.visible_lines();
        assert_eq!(lines[0], "hello");
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
        assert!(text.starts_with("\x1b["), "unexpected DSR response: {text:?}");
        assert!(text.ends_with('R'), "unexpected DSR response: {text:?}");
    }
}

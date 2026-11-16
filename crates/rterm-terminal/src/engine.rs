//! TerminalEngine — alacritty_terminal glue
//!
//! 职责：
//! - 把 PTY 字节流喂给 `vte` 解析器 → `Term`
//! - 暴露 Grid / Cursor / Colors 供 View 渲染
//! - 把按键映射成 xterm 字节序列
//!
//! `Term` 是 `!Send + !Sync`，所以本类型只能在 gpui 主线程创建和使用。

use alacritty_terminal::event::{Event, EventListener};
use alacritty_terminal::grid::{Dimensions, Grid};
use alacritty_terminal::index::{Column, Line};
use alacritty_terminal::term::cell::{Cell, Flags};
use alacritty_terminal::term::color::Colors;
use alacritty_terminal::term::{Config, RenderableCursor, Term};
use alacritty_terminal::vte::ansi::{Processor, StdSyncHandler};

// ---------------------------------------------------------------------------
// EventProxy / SizeInfo
// ---------------------------------------------------------------------------

/// 终端内部事件（标题变化 / 剪贴板请求等）。S5 只记录日志，
/// S6 渲染 / S10 OSC 52 再接管。
#[derive(Clone, Copy, Debug, Default)]
pub struct EventProxy;

impl EventListener for EventProxy {
    fn send_event(&self, event: Event) {
        log::debug!("[terminal] event: {event:?}");
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
        let term = Term::new(config, &size, EventProxy);
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
    pub fn input_key(&mut self, key: TerminalKey) -> Vec<u8> {
        match key {
            TerminalKey::Char(text) => text.into_bytes(),
            TerminalKey::Enter => b"\r".to_vec(),
            TerminalKey::Tab => b"\t".to_vec(),
            TerminalKey::Backspace => vec![0x7f],
            TerminalKey::Escape => vec![0x1b],
            TerminalKey::Up => b"\x1b[A".to_vec(),
            TerminalKey::Down => b"\x1b[B".to_vec(),
            TerminalKey::Right => b"\x1b[C".to_vec(),
            TerminalKey::Left => b"\x1b[D".to_vec(),
            TerminalKey::Home => b"\x1b[H".to_vec(),
            TerminalKey::End => b"\x1b[F".to_vec(),
            TerminalKey::PageUp => b"\x1b[5~".to_vec(),
            TerminalKey::PageDown => b"\x1b[6~".to_vec(),
            TerminalKey::Delete => b"\x1b[3~".to_vec(),
            TerminalKey::Insert => b"\x1b[2~".to_vec(),
            TerminalKey::F(n) => f_key_sequence(n),
            TerminalKey::Ctrl(c) => vec![(c as u8) & 0x1f],
        }
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
}

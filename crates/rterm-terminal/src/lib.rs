//! rterm-terminal — alacritty_terminal glue
//!
//! Sprint 5 落地：TerminalEngine（Grid / 字节输入 / 按键映射）。
//! Sprint 6 在此之上做 gpui Canvas 渲染。

pub mod engine;

pub use engine::{SizeInfo, TerminalEngine, TerminalKey};

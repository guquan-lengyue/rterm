//! 会话日志 — 把 PTY 原始输出追加写入文件。
//!
//! 保留 ANSI 转义序列，便于日后原样回放；时间戳前缀为可选项。

use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

/// 单次终端会话的日志写入器。
pub struct SessionLog {
    path: PathBuf,
    writer: BufWriter<File>,
    timestamps: bool,
    /// 下一段输出是否需要补时间戳前缀（按行首判定）。
    at_line_start: bool,
}

impl SessionLog {
    /// 打开（不存在则创建）`path` 追加写入。
    pub fn create(path: &Path, timestamps: bool) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("创建日志目录失败: {}", parent.display()))?;
        }
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .with_context(|| format!("打开会话日志失败: {}", path.display()))?;
        Ok(Self {
            path: path.to_path_buf(),
            writer: BufWriter::new(file),
            timestamps,
            at_line_start: true,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// 追加一段 PTY 原始输出；写入失败只告警，不影响终端使用。
    pub fn write(&mut self, bytes: &[u8]) {
        let result = if self.timestamps {
            self.write_with_timestamps(bytes)
        } else {
            self.writer.write_all(bytes)
        };
        if let Err(error) = result {
            log::warn!("[session-log] 写入失败 {}: {error}", self.path.display());
        }
    }

    pub fn flush(&mut self) {
        let _ = self.writer.flush();
    }

    fn write_with_timestamps(&mut self, bytes: &[u8]) -> std::io::Result<()> {
        for line in bytes.split_inclusive(|byte| *byte == b'\n') {
            if self.at_line_start {
                write!(self.writer, "[{}] ", utc_clock())?;
                self.at_line_start = false;
            }
            self.writer.write_all(line)?;
            if line.last() == Some(&b'\n') {
                self.at_line_start = true;
            }
        }
        Ok(())
    }
}

impl Drop for SessionLog {
    fn drop(&mut self) {
        self.flush();
    }
}

/// `HH:MM:SS`（UTC）——只用于日志时间戳，不引入时区依赖。
fn utc_clock() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0);
    format!(
        "{:02}:{:02}:{:02}",
        (secs / 3600) % 24,
        (secs / 60) % 60,
        secs % 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_path(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "rterm-session-log-{}-{}-{}.log",
            std::process::id(),
            tag,
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ))
    }

    #[test]
    fn writes_raw_output_and_appends() {
        let path = temp_path("raw");
        {
            let mut log = SessionLog::create(&path, false).expect("创建日志");
            log.write(b"hello ");
            log.write(b"world\n");
        }
        {
            let mut log = SessionLog::create(&path, false).expect("重开日志");
            log.write(b"second line\n");
        }
        let content = std::fs::read_to_string(&path).expect("读日志");
        assert_eq!(content, "hello world\nsecond line\n");

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn timestamp_prefix_only_at_line_start() {
        let path = temp_path("ts");
        let mut log = SessionLog::create(&path, true).expect("创建日志");
        log.write(b"abc");
        log.write(b"def\nghi\n");
        log.flush();
        drop(log);

        let content = std::fs::read_to_string(&path).expect("读日志");
        let lines: Vec<&str> = content.lines().collect();
        assert_eq!(lines.len(), 2);
        assert!(lines[0].starts_with('[') && lines[0].ends_with("abcdef"));
        assert!(lines[1].starts_with('[') && lines[1].ends_with("ghi"));

        let _ = std::fs::remove_file(&path);
    }
}

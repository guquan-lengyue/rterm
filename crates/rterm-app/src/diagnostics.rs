//! 诊断包导出（9.11）
//!
//! 生成 `diag-<时间戳>.zip`，包含：
//! - `diag.txt`：版本 / OS / 架构 / 会话数 / 目录 / 关键设置摘要
//! - 最近的终端会话日志（最多 8 份）
//!
//! 刻意不包含 `sessions.json` 与任何凭据，避免泄露主机与密钥信息。

use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use zip::write::SimpleFileOptions;

use crate::state::{logs_dir_path, Settings};

/// 最多打包的会话日志份数。
const MAX_LOG_FILES: usize = 8;

/// 导出一份诊断包到 `dest`，返回实际写入的路径。
pub fn export_diagnostics(
    dest: &Path,
    sessions_count: usize,
    settings: &Settings,
) -> Result<PathBuf> {
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("创建导出目录失败: {}", parent.display()))?;
    }
    let file = std::fs::File::create(dest)
        .with_context(|| format!("创建诊断包失败: {}", dest.display()))?;
    let mut zip = zip::ZipWriter::new(file);
    let options = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);

    let summary = build_summary(sessions_count, settings);
    zip.start_file("diag.txt", options)
        .context("写入 diag.txt 失败")?;
    zip.write_all(summary.as_bytes())
        .context("写入 diag.txt 内容失败")?;

    for path in recent_logs(settings, MAX_LOG_FILES) {
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("session.log");
        let entry = format!("logs/{name}");
        match std::fs::read(&path) {
            Ok(bytes) => {
                zip.start_file(entry, options).context("写入日志条目失败")?;
                zip.write_all(&bytes).context("写入日志内容失败")?;
            }
            Err(error) => log::warn!("[diag] 跳过无法读取的日志 {}: {error}", path.display()),
        }
    }

    zip.finish().context("完成诊断包失败")?;
    Ok(dest.to_path_buf())
}

/// 默认导出路径：`<data_dir>/diag-<时间戳>.zip`。
pub fn default_export_path() -> PathBuf {
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
    crate::state::data_dir_path().join(format!("diag-{stamp}.zip"))
}

fn build_summary(sessions_count: usize, settings: &Settings) -> String {
    let log_dir = logs_dir_path(settings);
    let logs = recent_logs(settings, MAX_LOG_FILES);
    let mut text = String::new();
    text.push_str("rterm-app diagnostics\n");
    text.push_str("=====================\n");
    text.push_str(&format!("version: {}\n", env!("CARGO_PKG_VERSION")));
    text.push_str(&format!("os: {}\n", std::env::consts::OS));
    text.push_str(&format!("arch: {}\n", std::env::consts::ARCH));
    text.push_str(&format!("sessions: {sessions_count}\n"));
    text.push_str(&format!(
        "data_dir: {}\n",
        crate::state::data_dir_path().display()
    ));
    text.push_str(&format!("log_dir: {}\n", log_dir.display()));
    text.push_str(&format!(
        "generated_at: {}\n",
        chrono::Local::now().to_rfc3339()
    ));
    text.push_str(&format!("theme: {}\n", settings.default_theme));
    text.push_str(&format!("font_family: {}\n", settings.default_font_family));
    text.push_str(&format!("font_size: {}\n", settings.default_font_size));
    text.push_str(&format!("sftp_sync_mode: {}\n", settings.sftp_sync_mode));
    text.push_str(&format!(
        "monitor_interval_secs: {}\n",
        settings.monitor_interval_secs
    ));
    text.push_str(&format!(
        "term_logging_dir: {:?}\n",
        settings.terminal_log_dir
    ));
    text.push_str("backup_included: false (sessions.json 不含凭据导出)\n");
    text.push_str("included_logs:\n");
    if logs.is_empty() {
        text.push_str("  (none)\n");
    } else {
        for path in logs {
            text.push_str(&format!("  {}\n", path.display()));
        }
    }
    text
}

/// 日志目录下最近的若干份 `.log`，按修改时间降序。
fn recent_logs(settings: &Settings, limit: usize) -> Vec<PathBuf> {
    let dir = logs_dir_path(settings);
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut logs: Vec<(std::time::SystemTime, PathBuf)> = entries
        .filter_map(|entry| entry.ok())
        .filter(|entry| {
            entry
                .path()
                .extension()
                .map(|ext| ext.eq_ignore_ascii_case("log"))
                .unwrap_or(false)
        })
        .filter_map(|entry| {
            let modified = entry.metadata().and_then(|meta| meta.modified()).ok()?;
            Some((modified, entry.path()))
        })
        .collect();
    logs.sort_by_key(|entry| std::cmp::Reverse(entry.0));
    logs.into_iter().take(limit).map(|(_, path)| path).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let base = std::env::temp_dir().join(format!(
            "rterm-diag-{}-{}-{}",
            std::process::id(),
            tag,
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&base).expect("创建临时目录");
        base
    }

    #[test]
    fn export_contains_summary_and_logs() {
        let base = temp_dir("zip");
        let log_dir = base.join("logs");
        std::fs::create_dir_all(&log_dir).expect("创建日志目录");
        std::fs::write(log_dir.join("demo.log"), b"hello from remote\n").expect("写日志");

        let settings = Settings {
            data_dir: Some(base.to_string_lossy().into_owned()),
            terminal_log_dir: Some(log_dir.to_string_lossy().into_owned()),
            ..Settings::default()
        };

        let dest = base.join("diag.zip");
        export_diagnostics(&dest, 2, &settings).expect("导出诊断包");

        let file = std::fs::File::open(&dest).expect("打开诊断包");
        let mut archive = zip::ZipArchive::new(file).expect("解析 zip");

        let mut summary = String::new();
        {
            use std::io::Read;
            let mut entry = archive.by_name("diag.txt").expect("diag.txt 存在");
            entry.read_to_string(&mut summary).expect("读 diag.txt");
        }
        assert!(summary.contains("version:"));
        assert!(summary.contains("sessions: 2"));
        assert!(summary.contains("os:"));
        assert!(summary.contains("demo.log"));

        {
            use std::io::Read;
            let mut entry = archive.by_name("logs/demo.log").expect("日志被打包");
            let mut content = String::new();
            entry.read_to_string(&mut content).expect("读日志");
            assert_eq!(content, "hello from remote\n");
        }

        // 不打包 sessions.json（防泄露）
        assert!(archive.by_name("sessions.json").is_err());

        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn export_without_log_dir_still_succeeds() {
        let base = temp_dir("nolog");
        let settings = Settings {
            data_dir: Some(base.to_string_lossy().into_owned()),
            terminal_log_dir: Some(base.join("missing").to_string_lossy().into_owned()),
            ..Settings::default()
        };

        let dest = base.join("diag.zip");
        export_diagnostics(&dest, 0, &settings).expect("无日志也应成功");
        assert!(dest.exists());

        let _ = std::fs::remove_dir_all(&base);
    }
}

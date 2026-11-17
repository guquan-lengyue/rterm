//! 会话文件自动备份 — 24 小时节流 + 保留最近 7 份
//!
//! 备份与 `sessions.json` 同目录，命名为
//! `sessions.json.<YYYYmmdd-HHMMSS>.bak`，字典序即时间序，便于轮转。

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use anyhow::{Context, Result};

/// 自动备份间隔。
pub const BACKUP_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);
/// 最多保留的备份份数。
pub const MAX_BACKUPS: usize = 7;
/// 备份文件名后缀（位于时间戳之后）。
const BACKUP_SUFFIX: &str = ".bak";

/// 备份文件前缀：`<源文件名>.<时间戳>.bak`。
fn backup_prefix(source: &Path) -> String {
    let stem = source
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("sessions.json");
    format!("{stem}.")
}

/// 判断 `name` 是否为 `source` 的备份文件。
fn is_backup_of(name: &str, prefix: &str) -> bool {
    name.starts_with(prefix) && name.ends_with(BACKUP_SUFFIX)
}

/// 立即生成一份备份（不检查节流），返回备份路径。
pub fn backup_now(sessions_path: &Path, now: SystemTime) -> Result<Option<PathBuf>> {
    if !sessions_path.exists() {
        return Ok(None);
    }
    let dir = sessions_path.parent().unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(dir).with_context(|| format!("创建备份目录失败: {}", dir.display()))?;

    let stamp = chrono::DateTime::<chrono::Utc>::from(now).format("%Y%m%d-%H%M%S");
    let target = dir.join(format!(
        "{}{stamp}{BACKUP_SUFFIX}",
        backup_prefix(sessions_path)
    ));
    std::fs::copy(sessions_path, &target)
        .with_context(|| format!("写入备份失败: {}", target.display()))?;

    prune_backups(sessions_path, MAX_BACKUPS)?;
    Ok(Some(target))
}

/// 距上次备份超过 [`BACKUP_INTERVAL`] 时才备份；返回新备份路径（跳过时为 None）。
pub fn ensure_daily_backup(sessions_path: &Path, now: SystemTime) -> Result<Option<PathBuf>> {
    if !sessions_path.exists() {
        return Ok(None);
    }
    if !last_backup_overdue(sessions_path, now) {
        return Ok(None);
    }
    backup_now(sessions_path, now)
}

/// 最近一份备份是否已超过 24h（没有备份时视为需要备份）。
pub fn last_backup_overdue(sessions_path: &Path, now: SystemTime) -> bool {
    let Some(newest) = newest_backup(sessions_path) else {
        return true;
    };
    let modified = std::fs::metadata(&newest)
        .and_then(|meta| meta.modified())
        .unwrap_or(SystemTime::UNIX_EPOCH);
    now.duration_since(modified)
        .map(|elapsed| elapsed >= BACKUP_INTERVAL)
        .unwrap_or(true)
}

/// 已存在的备份文件，按名称（即时间）升序。
pub fn list_backups(sessions_path: &Path) -> Vec<PathBuf> {
    let dir = sessions_path.parent().unwrap_or_else(|| Path::new("."));
    let prefix = backup_prefix(sessions_path);
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut backups: Vec<PathBuf> = entries
        .filter_map(|entry| entry.ok())
        .filter(|entry| {
            entry
                .file_name()
                .to_str()
                .map(|name| is_backup_of(name, &prefix))
                .unwrap_or(false)
        })
        .map(|entry| entry.path())
        .collect();
    backups.sort();
    backups
}

fn newest_backup(sessions_path: &Path) -> Option<PathBuf> {
    list_backups(sessions_path).pop()
}

/// 只保留最近 `keep` 份备份，删除更早的（删除失败只记日志）。
pub fn prune_backups(sessions_path: &Path, keep: usize) -> Result<()> {
    let backups = list_backups(sessions_path);
    if backups.len() <= keep {
        return Ok(());
    }
    let remove_count = backups.len() - keep;
    for path in backups.into_iter().take(remove_count) {
        if let Err(error) = std::fs::remove_file(&path) {
            log::warn!("[backup] 删除旧备份失败 {}: {error}", path.display());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let base = std::env::temp_dir().join(format!(
            "rterm-backup-{}-{}-{}",
            std::process::id(),
            tag,
            SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&base).expect("创建临时目录");
        base
    }

    fn write_sessions(dir: &Path) -> PathBuf {
        let path = dir.join("sessions.json");
        std::fs::write(&path, "{\"sessions\":[]}").expect("写 sessions.json");
        path
    }

    #[test]
    fn backup_creates_file_and_skips_missing_source() {
        let dir = temp_dir("create");
        let missing = dir.join("missing.json");
        assert!(backup_now(&missing, SystemTime::now()).unwrap().is_none());

        let sessions = write_sessions(&dir);
        let backup = backup_now(&sessions, SystemTime::now())
            .unwrap()
            .expect("应生成备份");
        assert!(backup.exists());
        assert_eq!(
            std::fs::read_to_string(&backup).unwrap(),
            "{\"sessions\":[]}"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn daily_backup_is_throttled() {
        let dir = temp_dir("throttle");
        let sessions = write_sessions(&dir);

        let now = SystemTime::now();
        assert!(
            ensure_daily_backup(&sessions, now).unwrap().is_some(),
            "首次应备份"
        );
        assert!(
            ensure_daily_backup(&sessions, now).unwrap().is_none(),
            "24h 内应跳过"
        );

        let later = now + BACKUP_INTERVAL + Duration::from_secs(60);
        assert!(
            ensure_daily_backup(&sessions, later).unwrap().is_some(),
            "超过 24h 应再次备份"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn prune_keeps_newest_seven() {
        let dir = temp_dir("prune");
        let sessions = write_sessions(&dir);
        let prefix = backup_prefix(&sessions);
        for index in 0..10 {
            std::fs::write(
                dir.join(format!("{prefix}2026010{index}-000000{BACKUP_SUFFIX}")),
                b"old",
            )
            .expect("写假备份");
        }

        prune_backups(&sessions, MAX_BACKUPS).expect("轮转");
        let backups = list_backups(&sessions);
        assert_eq!(backups.len(), MAX_BACKUPS);
        // 保留的是最新 7 份（字典序最大）
        assert!(backups[0]
            .file_name()
            .unwrap()
            .to_string_lossy()
            .contains("20260103-000000"));

        let _ = std::fs::remove_dir_all(&dir);
    }
}

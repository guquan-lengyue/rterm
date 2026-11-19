//! 目录扫描结果缓存 — 短时间内重复访问同一目录直接复用，避免反复执行 `du`。
//!
//! 纯内存缓存，生命周期与采集线程一致；过期条目在写入时顺带清理。

use std::collections::HashMap;
use std::time::{Duration, Instant};

use crate::types::DiskUsage;

/// 缓存条目上限，避免长时间运行后无限增长。
const MAX_ENTRIES: usize = 64;

struct CachedScan {
    usage: DiskUsage,
    fetched_at: Instant,
}

/// 带 TTL 的目录扫描缓存。
pub struct DiskScanCache {
    ttl: Duration,
    entries: HashMap<String, CachedScan>,
}

impl DiskScanCache {
    pub fn new(ttl: Duration) -> Self {
        Self {
            ttl,
            entries: HashMap::new(),
        }
    }

    /// 读取未过期结果；`None` 表示需要重新扫描。
    pub fn get(&self, path: &str) -> Option<DiskUsage> {
        self.get_at(path, Instant::now())
    }

    /// 写入 / 刷新结果，同时清理过期条目并限制总容量。
    pub fn insert(&mut self, usage: DiskUsage) {
        self.insert_at(usage, Instant::now())
    }

    /// 可注入时间点的读取，便于单测。
    pub fn get_at(&self, path: &str, now: Instant) -> Option<DiskUsage> {
        let entry = self.entries.get(normalize_path(path))?;
        (now.saturating_duration_since(entry.fetched_at) < self.ttl).then(|| entry.usage.clone())
    }

    /// 可注入时间点的写入，便于单测。
    pub fn insert_at(&mut self, usage: DiskUsage, now: Instant) {
        self.entries
            .retain(|_, entry| now.saturating_duration_since(entry.fetched_at) < self.ttl);

        if self.entries.len() >= MAX_ENTRIES {
            let oldest = self
                .entries
                .iter()
                .min_by_key(|(_, entry)| entry.fetched_at)
                .map(|(path, _)| path.clone());
            if let Some(path) = oldest {
                self.entries.remove(&path);
            }
        }

        self.entries.insert(
            normalize_path(&usage.path).to_string(),
            CachedScan {
                usage,
                fetched_at: now,
            },
        );
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.entries.len()
    }
}

/// 去掉末尾 `/`（根目录保持 `/`），保证 `/var` 与 `/var/` 命中同一条缓存。
fn normalize_path(path: &str) -> &str {
    let trimmed = path.trim_end_matches('/');
    if trimmed.is_empty() {
        "/"
    } else {
        trimmed
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::DiskEntry;

    fn usage(path: &str) -> DiskUsage {
        DiskUsage {
            path: path.to_string(),
            total_bytes: 1024,
            entries: vec![DiskEntry {
                name: "child".into(),
                path: format!("{path}/child"),
                bytes: 512,
                is_dir: true,
            }],
        }
    }

    #[test]
    fn hit_within_ttl_and_miss_after_expiry() {
        let base = Instant::now();
        let mut cache = DiskScanCache::new(Duration::from_secs(30));
        cache.insert_at(usage("/var"), base);

        assert!(cache
            .get_at("/var", base + Duration::from_secs(29))
            .is_some());
        assert!(cache
            .get_at("/var", base + Duration::from_secs(31))
            .is_none());
    }

    #[test]
    fn trailing_slash_hits_same_entry() {
        let base = Instant::now();
        let mut cache = DiskScanCache::new(Duration::from_secs(30));
        cache.insert_at(usage("/var/"), base);
        assert!(cache.get_at("/var", base).is_some());
        assert!(cache.get_at("/var/", base).is_some());
    }

    #[test]
    fn expired_entries_are_pruned_on_insert() {
        let base = Instant::now();
        let mut cache = DiskScanCache::new(Duration::from_secs(30));
        cache.insert_at(usage("/a"), base);
        cache.insert_at(usage("/b"), base + Duration::from_secs(40));
        assert!(cache.get_at("/a", base + Duration::from_secs(40)).is_none());
        assert_eq!(cache.len(), 1);
    }

    #[test]
    fn capacity_is_bounded() {
        let base = Instant::now();
        let mut cache = DiskScanCache::new(Duration::from_secs(30));
        for index in 0..(MAX_ENTRIES + 8) {
            cache.insert_at(usage(&format!("/dir{index}")), base);
        }
        assert!(cache.len() <= MAX_ENTRIES);
    }
}

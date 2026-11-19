//! 采集输出解析 — Linux / macOS 两套脚本共用同一套段落切分。
//!
//! 解析器只依赖纯文本，便于在 Windows 开发机上用固定样本做单元测试。

use std::collections::BTreeMap;
use std::time::{Duration, SystemTime};

use anyhow::{bail, Context, Result};

use crate::scripts::MARKER_PREFIX;
use crate::types::{DiskEntry, DiskUsage, ResourceSample};

/// 跨采样保留的增量计数（CPU 时间片 / 网卡字节数）。
#[derive(Debug, Default)]
pub struct ParseState {
    prev_cpu: Option<(u64, u64)>,
    prev_net: Option<(u64, u64)>,
}

impl ParseState {
    pub fn new() -> Self {
        Self::default()
    }
}

/// 解析 Linux 采集脚本输出。
pub fn parse_linux(
    state: &mut ParseState,
    output: &str,
    elapsed: Duration,
    now: SystemTime,
) -> Result<ResourceSample> {
    let sections = split_sections(output);

    // STAT 段：首行是聚合 `cpu`，其后每行一个 `cpuN`。
    let stat = sections
        .get("STAT")
        .and_then(|lines| lines.first())
        .copied()
        .context("采集输出缺少 /proc/stat")?;
    let (busy, total) = parse_cpu_stat(stat);
    let cpu_pct = cpu_delta_pct(state, busy, total);

    let mem_lines = sections.get("MEM").map(Vec::as_slice).unwrap_or(&[]);
    let mem_pct = memory_pct(mem_lines).context("解析 /proc/meminfo 失败")?;
    let swap_pct = swap_pct(mem_lines);

    let disk = disk_usage(&sections);

    let (rx, tx) = sections
        .get("NET")
        .map(|lines| parse_linux_net(lines))
        .unwrap_or((0, 0));
    let (net_rx_kbps, net_tx_kbps) = net_rates(state, rx, tx, elapsed);

    let (load_1, load_5) = sections
        .get("LOAD")
        .and_then(|lines| lines.first())
        .map(|line| parse_loadavg(line))
        .unwrap_or((0.0, 0.0));

    Ok(ResourceSample {
        cpu_pct,
        mem_pct,
        swap_pct,
        disk_pct: disk.pct(),
        disk_total_bytes: disk.total_bytes,
        disk_used_bytes: disk.used_bytes,
        net_rx_kbps,
        net_tx_kbps,
        load_1,
        load_5,
        timestamp: now,
    })
}

/// 解析 macOS 采集脚本输出。
pub fn parse_macos(
    state: &mut ParseState,
    output: &str,
    elapsed: Duration,
    now: SystemTime,
) -> Result<ResourceSample> {
    let sections = split_sections(output);

    let cpu_lines = sections.get("CPU").map(Vec::as_slice).unwrap_or(&[]);
    let cpu_pct = parse_macos_cpu(cpu_lines);

    let mem_lines = sections.get("MEM").map(Vec::as_slice).unwrap_or(&[]);
    let (mem_pct, swap_pct) = parse_macos_memory(mem_lines)?;

    let disk = disk_usage(&sections);

    let (rx, tx) = sections
        .get("NET")
        .and_then(|lines| lines.first())
        .map(|line| {
            let mut parts = line.split_whitespace();
            (
                parts.next().and_then(|v| v.parse::<u64>().ok()),
                parts.next().and_then(|v| v.parse::<u64>().ok()),
            )
        })
        .map(|(rx, tx)| (rx.unwrap_or(0), tx.unwrap_or(0)))
        .unwrap_or((0, 0));
    let (net_rx_kbps, net_tx_kbps) = net_rates(state, rx, tx, elapsed);

    let (load_1, load_5) = sections
        .get("LOAD")
        .and_then(|lines| lines.first())
        .map(|line| parse_loadavg(line))
        .unwrap_or((0.0, 0.0));

    Ok(ResourceSample {
        cpu_pct,
        mem_pct,
        swap_pct,
        disk_pct: disk.pct(),
        disk_total_bytes: disk.total_bytes,
        disk_used_bytes: disk.used_bytes,
        net_rx_kbps,
        net_tx_kbps,
        load_1,
        load_5,
        timestamp: now,
    })
}

// ---------------------------------------------------------------------------
// 段落切分
// ---------------------------------------------------------------------------

fn split_sections(text: &str) -> BTreeMap<&str, Vec<&str>> {
    let mut sections: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    let mut current: Option<&str> = None;
    for raw in text.lines() {
        let line = raw.trim_end_matches('\r');
        if let Some(name) = line
            .strip_prefix(MARKER_PREFIX)
            .and_then(|rest| rest.strip_suffix("@@"))
        {
            current = Some(name);
            sections.entry(name).or_default();
        } else if let Some(name) = current {
            if !line.trim().is_empty() {
                sections.entry(name).or_default().push(line);
            }
        }
    }
    sections
}

// ---------------------------------------------------------------------------
// 指标解析
// ---------------------------------------------------------------------------

/// `/proc/stat` 首行 → (busy, total) 时间片。
fn parse_cpu_stat(line: &str) -> (u64, u64) {
    let mut fields = line
        .split_whitespace()
        .skip(1)
        .map(|v| v.parse::<u64>().unwrap_or(0));
    let user = fields.next().unwrap_or(0);
    let nice = fields.next().unwrap_or(0);
    let system = fields.next().unwrap_or(0);
    let idle = fields.next().unwrap_or(0);
    let iowait = fields.next().unwrap_or(0);
    let irq = fields.next().unwrap_or(0);
    let softirq = fields.next().unwrap_or(0);
    let steal = fields.next().unwrap_or(0);
    let total = user + nice + system + idle + iowait + irq + softirq + steal;
    let idle_total = idle + iowait;
    (total.saturating_sub(idle_total), total)
}

/// 用相邻两帧的时间片增量计算整机 CPU 使用率。
fn cpu_delta_pct(state: &mut ParseState, busy: u64, total: u64) -> f32 {
    let pct = match state.prev_cpu {
        Some((prev_busy, prev_total)) if total > prev_total => {
            let busy_delta = busy.saturating_sub(prev_busy);
            let total_delta = total - prev_total;
            (busy_delta as f32 / total_delta as f32 * 100.0).clamp(0.0, 100.0)
        }
        // 首帧没有增量基线，先给 0，下一帧即可得到真实值。
        _ => 0.0,
    };
    state.prev_cpu = Some((busy, total));
    pct
}

fn net_rates(state: &mut ParseState, rx: u64, tx: u64, elapsed: Duration) -> (f32, f32) {
    let secs = elapsed.as_secs_f32();
    let rates = match state.prev_net {
        Some((prev_rx, prev_tx)) if secs > 0.0 => {
            let rx_delta = rx.saturating_sub(prev_rx) as f32;
            let tx_delta = tx.saturating_sub(prev_tx) as f32;
            (rx_delta / secs / 1024.0, tx_delta / secs / 1024.0)
        }
        _ => (0.0, 0.0),
    };
    state.prev_net = Some((rx, tx));
    rates
}

fn memory_pct(lines: &[&str]) -> Option<f32> {
    let total = meminfo_kb(lines, "MemTotal:")?;
    let available = meminfo_kb(lines, "MemAvailable:")?;
    if total <= 0.0 {
        return Some(0.0);
    }
    Some(((total - available) / total * 100.0).clamp(0.0, 100.0))
}

fn swap_pct(lines: &[&str]) -> f32 {
    let total = meminfo_kb(lines, "SwapTotal:").unwrap_or(0.0);
    let free = meminfo_kb(lines, "SwapFree:").unwrap_or(0.0);
    if total <= 0.0 {
        0.0
    } else {
        ((total - free) / total * 100.0).clamp(0.0, 100.0)
    }
}

fn meminfo_kb(lines: &[&str], key: &str) -> Option<f32> {
    lines.iter().find_map(|line| {
        let mut parts = line.split_whitespace();
        if parts.next()? != key {
            return None;
        }
        parts.next()?.parse::<f32>().ok()
    })
}

// ---------------------------------------------------------------------------
// 磁盘
// ---------------------------------------------------------------------------

/// 根文件系统容量（字节）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct FilesystemUsage {
    total_bytes: u64,
    used_bytes: u64,
}

impl FilesystemUsage {
    /// 已用占比 `0..=100`；总容量为 0 时返回 0。
    fn pct(self) -> f32 {
        if self.total_bytes == 0 {
            0.0
        } else {
            (self.used_bytes as f64 / self.total_bytes as f64 * 100.0).clamp(0.0, 100.0) as f32
        }
    }
}

/// 从段落中取 `df -Pk` 首行，缺失或格式异常时返回全 0。
fn disk_usage(sections: &BTreeMap<&str, Vec<&str>>) -> FilesystemUsage {
    sections
        .get("DISK")
        .and_then(|lines| lines.first())
        .and_then(|line| df_usage(line))
        .unwrap_or_default()
}

/// `df -Pk` 行 → 容量。
///
/// 列格式：`Filesystem 1024-blocks Used Available Capacity Mounted`，
/// 块单位固定为 1 KiB。
fn df_usage(line: &str) -> Option<FilesystemUsage> {
    let mut parts = line.split_whitespace();
    parts.next()?; // Filesystem
    let total_kb = parts.next()?.parse::<u64>().ok()?;
    let used_kb = parts.next()?.parse::<u64>().ok()?;
    Some(FilesystemUsage {
        total_bytes: total_kb.saturating_mul(1024),
        used_bytes: used_kb.saturating_mul(1024),
    })
}

// ---------------------------------------------------------------------------
// 目录占用扫描
// ---------------------------------------------------------------------------

/// 解析 [`crate::scripts::disk_scan_script`] 的输出。
///
/// 条目按占用降序排列；`du` 缺失总计时用子项之和兜底。
pub fn parse_disk_usage(root: &str, output: &str) -> Result<DiskUsage> {
    let sections = split_sections(output);
    let lines = sections.get("DU").map(Vec::as_slice).unwrap_or(&[]);
    if lines.is_empty() {
        bail!("目录扫描没有输出（目录不存在或无访问权限）");
    }

    let root = normalize_dir(root);
    let mut total_bytes = 0_u64;
    let mut entries = Vec::with_capacity(lines.len().saturating_sub(1));

    for line in lines {
        let mut parts = line.splitn(3, '\t');
        let (Some(kind), Some(size_kb), Some(path)) =
            (parts.next(), parts.next(), parts.next())
        else {
            continue;
        };
        let Ok(size_kb) = size_kb.trim().parse::<u64>() else {
            continue;
        };
        let bytes = size_kb.saturating_mul(1024);
        let path = path.trim_end_matches('/');
        let path = if path.is_empty() { "/" } else { path };

        if path == root {
            total_bytes = bytes;
            continue;
        }
        let Some(name) = entry_name(path) else {
            continue;
        };
        entries.push(DiskEntry {
            name: name.to_string(),
            path: path.to_string(),
            bytes,
            is_dir: kind.trim() == "d",
        });
    }

    if total_bytes == 0 {
        total_bytes = entries.iter().map(|entry| entry.bytes).sum();
    }
    entries.sort_by(|left, right| {
        right
            .bytes
            .cmp(&left.bytes)
            .then_with(|| left.name.cmp(&right.name))
    });

    Ok(DiskUsage {
        path: root.to_string(),
        total_bytes,
        entries,
    })
}

/// 去掉末尾 `/`（根目录保持 `/`）。
fn normalize_dir(path: &str) -> &str {
    let trimmed = path.trim_end_matches('/');
    if trimmed.is_empty() {
        "/"
    } else {
        trimmed
    }
}

fn entry_name(path: &str) -> Option<&str> {
    path.rsplit('/').find(|part| !part.is_empty())
}

fn parse_loadavg(line: &str) -> (f32, f32) {
    let values: Vec<f32> = line
        .split_whitespace()
        .filter_map(|token| {
            token
                .trim_matches(|c: char| !c.is_ascii_digit() && c != '.' && c != '-')
                .parse::<f32>()
                .ok()
        })
        .collect();
    (
        values.first().copied().unwrap_or(0.0),
        values.get(1).copied().unwrap_or(0.0),
    )
}

fn parse_linux_net(lines: &[&str]) -> (u64, u64) {
    let mut rx = 0_u64;
    let mut tx = 0_u64;
    for line in lines {
        let Some((iface, rest)) = line.split_once(':') else {
            continue;
        };
        if iface.trim() == "lo" {
            continue;
        }
        let fields: Vec<&str> = rest.split_whitespace().collect();
        if fields.len() < 9 {
            continue;
        }
        rx += fields[0].parse::<u64>().unwrap_or(0);
        tx += fields[8].parse::<u64>().unwrap_or(0);
    }
    (rx, tx)
}

// ---------------------------------------------------------------------------
// macOS 专用解析
// ---------------------------------------------------------------------------

fn parse_macos_cpu(lines: &[&str]) -> f32 {
    let cores = lines
        .first()
        .and_then(|v| v.trim().parse::<f32>().ok())
        .unwrap_or(1.0)
        .max(1.0);
    let sum = lines
        .get(1)
        .and_then(|v| v.trim().parse::<f32>().ok())
        .unwrap_or(0.0);
    (sum / cores).clamp(0.0, 100.0)
}

fn parse_macos_memory(lines: &[&str]) -> Result<(f32, f32)> {
    let total = lines
        .first()
        .and_then(|v| v.trim().parse::<f64>().ok())
        .context("缺少 hw.memsize")?;
    if total <= 0.0 {
        bail!("hw.memsize 非法");
    }

    let page_size = lines
        .iter()
        .find_map(|line| {
            let start = line.find("page size of ")? + "page size of ".len();
            line[start..].split_whitespace().next()?.parse::<f64>().ok()
        })
        .unwrap_or(4096.0);

    let pages = |key: &str| -> f64 {
        lines
            .iter()
            .find_map(|line| {
                let rest = line.strip_prefix(key)?;
                rest.trim().trim_end_matches('.').parse::<f64>().ok()
            })
            .unwrap_or(0.0)
    };

    // active + wired + compressor 近似的“已用”内存（不含可回收的 inactive）。
    let used_pages = pages("Pages active:")
        + pages("Pages wired down:")
        + pages("Pages occupied by compressor:");
    let mem_pct = (used_pages * page_size / total * 100.0).clamp(0.0, 100.0) as f32;

    let swap_pct = lines
        .iter()
        .find_map(|line| parse_swap_usage(line))
        .unwrap_or(0.0);

    Ok((mem_pct, swap_pct))
}

/// `total = 1024.00M  used = 100.00M  free = 924.00M` → 9.77
fn parse_swap_usage(line: &str) -> Option<f32> {
    let tokens: Vec<&str> = line.split_whitespace().collect();
    let mut total = None;
    let mut used = None;
    for window in tokens.windows(3) {
        if window[1] != "=" {
            continue;
        }
        match window[0] {
            "total" => total = parse_size_mb(window[2]),
            "used" => used = parse_size_mb(window[2]),
            _ => {}
        }
    }
    let total = total?;
    let used = used?;
    if total <= 0.0 {
        Some(0.0)
    } else {
        Some((used / total * 100.0).clamp(0.0, 100.0))
    }
}

/// `123.45M` / `1.5G` / `0B` → MB。
fn parse_size_mb(raw: &str) -> Option<f32> {
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    let (number, unit) = raw.split_at(raw.len().saturating_sub(1));
    if let Ok(value) = raw.parse::<f32>() {
        return Some(value / 1024.0 / 1024.0); // 纯字节数
    }
    let value = number.parse::<f32>().ok()?;
    match unit {
        "B" | "b" => Some(value / 1024.0 / 1024.0),
        "K" | "k" => Some(value / 1024.0),
        "M" | "m" => Some(value),
        "G" | "g" => Some(value * 1024.0),
        "T" | "t" => Some(value * 1024.0 * 1024.0),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `/proc/stat` 聚合行 + 一条 `cpu0`。
    fn linux_fixture(cpu_line: &str, net_rx: u64) -> String {
        format!(
            "@@RT_STAT@@\n{cpu_line}\ncpu0 {cpu_line_rest}\n@@RT_MEM@@\nMemTotal: 1000 kB\nMemAvailable: 400 kB\nSwapTotal: 200 kB\nSwapFree: 50 kB\n@@RT_DISK@@\n/dev/sda1 1000 600 400 60% /\n@@RT_NET@@\nInter-|   Receive                                                |  Transmit\n face |bytes    packets errs drop fifo frame compressed multicast|bytes    packets errs drop fifo colls carrier compressed\n    lo: 999999 0 0 0 0 0 0 0 999999 0 0 0 0 0 0 0\n  eth0: {net_rx} 10 0 0 0 0 0 0 2048 0 0 0 0 0 0 0\n@@RT_LOAD@@\n1.50 0.75 0.50 1/100 12345\n",
            cpu_line_rest = cpu_line.trim_start_matches("cpu"),
        )
    }

    fn approx(actual: f32, expected: f32) {
        assert!(
            (actual - expected).abs() < 0.01,
            "expected {expected}, got {actual}"
        );
    }

    #[test]
    fn parses_linux_sample() {
        let mut state = ParseState::new();
        let now = SystemTime::UNIX_EPOCH;
        let first = parse_linux(
            &mut state,
            &linux_fixture("cpu 100 0 100 800 0 0 0 0", 1024),
            Duration::from_secs(2),
            now,
        )
        .unwrap();
        assert_eq!(first.cpu_pct, 0.0, "首帧没有增量基线");
        approx(first.mem_pct, 60.0);
        approx(first.swap_pct, 75.0);
        approx(first.disk_pct, 60.0);
        assert_eq!(first.disk_total_bytes, 1000 * 1024);
        assert_eq!(first.disk_used_bytes, 600 * 1024);
        assert_eq!(first.net_rx_kbps, 0.0);
        approx(first.load_1, 1.5);
        approx(first.load_5, 0.75);

        // 第二帧：CPU busy 增量 200 / total 增量 400 → 50%；网卡 +2048 B/2s → 1 KiB/s
        let second = parse_linux(
            &mut state,
            &linux_fixture("cpu 200 0 200 1000 0 0 0 0", 3072),
            Duration::from_secs(2),
            now,
        )
        .unwrap();
        assert_eq!(second.cpu_pct, 50.0);
        assert_eq!(second.net_rx_kbps, 1.0);
        assert_eq!(second.net_tx_kbps, 0.0);
    }

    #[test]
    fn linux_cpu_counter_reset_is_zero() {
        let mut state = ParseState::new();
        let now = SystemTime::UNIX_EPOCH;
        parse_linux(
            &mut state,
            &linux_fixture("cpu 500 0 500 1000 0 0 0 0", 0),
            Duration::from_secs(2),
            now,
        )
        .unwrap();
        let sample = parse_linux(
            &mut state,
            &linux_fixture("cpu 10 0 10 20 0 0 0 0", 0),
            Duration::from_secs(2),
            now,
        )
        .unwrap();
        assert_eq!(sample.cpu_pct, 0.0, "计数器回绕不应出现负值/异常值");
    }

    #[test]
    fn linux_missing_stat_is_error() {
        let mut state = ParseState::new();
        let err = parse_linux(
            &mut state,
            "@@RT_MEM@@\nMemTotal: 1 kB\n",
            Duration::from_secs(1),
            SystemTime::UNIX_EPOCH,
        );
        assert!(err.is_err());
    }

    #[test]
    fn linux_ignores_loopback_net() {
        let mut state = ParseState::new();
        let now = SystemTime::UNIX_EPOCH;
        parse_linux(
            &mut state,
            &linux_fixture("cpu 1 0 1 8 0 0 0 0", 0),
            Duration::from_secs(1),
            now,
        )
        .unwrap();
        let sample = parse_linux(
            &mut state,
            &linux_fixture("cpu 2 0 2 16 0 0 0 0", 0),
            Duration::from_secs(1),
            now,
        )
        .unwrap();
        assert_eq!(sample.net_rx_kbps, 0.0);
    }

    #[test]
    fn parses_df_usage() {
        let disk = df_usage("/dev/x 2048 1024 1024 50% /").unwrap();
        assert_eq!(disk.total_bytes, 2048 * 1024);
        assert_eq!(disk.used_bytes, 1024 * 1024);
        approx(disk.pct(), 50.0);
        assert_eq!(df_usage("garbage"), None);
        assert_eq!(FilesystemUsage::default().pct(), 0.0);
    }

    #[test]
    fn parses_disk_usage_sorted_and_flags_dirs() {
        let output = "@@RT_DU@@\nd\t4096\t/var\nf\t512\t/swap.img\nd\t1024\t/usr\nd\t4096\t/\n";
        let usage = parse_disk_usage("/", output).unwrap();
        assert_eq!(usage.path, "/");
        assert_eq!(usage.total_bytes, 4096 * 1024);
        assert_eq!(usage.entries.len(), 3);
        assert_eq!(usage.entries[0].name, "var");
        assert!(usage.entries[0].is_dir);
        assert_eq!(usage.entries[1].name, "usr");
        assert_eq!(usage.entries[2].name, "swap.img");
        assert!(!usage.entries[2].is_dir);
    }

    #[test]
    fn disk_usage_without_total_sums_entries() {
        let output = "@@RT_DU@@\nd\t100\t/var\nf\t20\t/a.log\n";
        let usage = parse_disk_usage("/", output).unwrap();
        assert_eq!(usage.total_bytes, 120 * 1024);
        assert_eq!(usage.entries.len(), 2);
    }

    #[test]
    fn disk_usage_normalizes_trailing_slash_and_root_entry() {
        let output = "@@RT_DU@@\nd\t100\t/var/log\nd\t300\t/var\n";
        let usage = parse_disk_usage("/var/", output).unwrap();
        assert_eq!(usage.path, "/var");
        assert_eq!(usage.total_bytes, 300 * 1024);
        assert_eq!(usage.entries.len(), 1);
        assert_eq!(usage.entries[0].path, "/var/log");
    }

    #[test]
    fn disk_usage_empty_output_is_error() {
        assert!(parse_disk_usage("/", "").is_err());
        assert!(parse_disk_usage("/", "@@RT_DU@@\n").is_err());
    }

    #[test]
    fn parses_macos_sample() {
        let output = r#"
@@RT_CPU@@
8
123.40
@@RT_MEM@@
17179869184
Mach Virtual Memory Statistics: (page size of 16384 bytes)
Pages free:                               10000.
Pages active:                            200000.
Pages inactive:                          100000.
Pages speculative:                         5000.
Pages wired down:                        100000.
Pages occupied by compressor:             50000.
total = 2048.00M  used = 512.00M  free = 1536.00M
@@RT_DISK@@
/dev/disk1s1 100 50 40 55% /
@@RT_NET@@
4096 8192
@@RT_LOAD@@
{ 1.23 4.56 7.89 }
"#;
        let mut state = ParseState::new();
        let now = SystemTime::UNIX_EPOCH;
        let first = parse_macos(&mut state, output, Duration::from_secs(2), now).unwrap();
        assert!((first.cpu_pct - 15.425).abs() < 0.01);
        // (200000 + 100000 + 50000) * 16384 / 17179869184 * 100 ≈ 33.36
        assert!((first.mem_pct - 33.36).abs() < 0.05);
        assert!((first.swap_pct - 25.0).abs() < 0.01);
        approx(first.disk_pct, 50.0);
        assert_eq!(first.disk_total_bytes, 100 * 1024);
        assert_eq!(first.disk_used_bytes, 50 * 1024);
        assert_eq!(first.load_1, 1.23);
        assert_eq!(first.load_5, 4.56);

        let second = parse_macos(&mut state, output, Duration::from_secs(2), now).unwrap();
        assert_eq!(second.net_rx_kbps, 0.0);
    }

    #[test]
    fn parses_swap_usage_units() {
        assert!(
            (parse_swap_usage("total = 1.00G  used = 512.00M  free = 512.00M").unwrap() - 50.0)
                .abs()
                < 0.01
        );
        assert_eq!(parse_swap_usage("total = 0B  used = 0B"), Some(0.0));
        assert_eq!(parse_swap_usage("no swap info"), None);
    }

    #[test]
    fn macos_missing_memsize_is_error() {
        let mut state = ParseState::new();
        let err = parse_macos(
            &mut state,
            "@@RT_CPU@@\n4\n10.0\n@@RT_MEM@@\n",
            Duration::from_secs(2),
            SystemTime::UNIX_EPOCH,
        );
        assert!(err.is_err());
    }

    #[test]
    fn parses_size_units() {
        assert_eq!(parse_size_mb("1024.00M"), Some(1024.0));
        assert_eq!(parse_size_mb("1.00G"), Some(1024.0));
        assert_eq!(parse_size_mb("512K"), Some(0.5));
        assert_eq!(parse_size_mb("1048576"), Some(1.0));
        assert_eq!(parse_size_mb(""), None);
    }
}

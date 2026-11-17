//! 采集输出解析 — Linux / macOS 两套脚本共用同一套段落切分。
//!
//! 解析器只依赖纯文本，便于在 Windows 开发机上用固定样本做单元测试。

use std::collections::{BTreeMap, HashMap};
use std::time::{Duration, SystemTime};

use anyhow::{bail, Context, Result};

use crate::scripts::MARKER_PREFIX;
use crate::types::{MonitorTick, ResourceSample, TopProcess};

/// 跨采样保留的增量计数（CPU 时间片 / 网卡字节数 / 进程 tick）。
#[derive(Debug, Default)]
pub struct ParseState {
    prev_cpu: Option<(u64, u64)>,
    prev_net: Option<(u64, u64)>,
    /// pid → (utime + stime) 时间片，用于进程级 CPU% 增量。
    prev_proc_ticks: HashMap<u32, u64>,
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
) -> Result<MonitorTick> {
    let sections = split_sections(output);

    // STAT 段：首行是聚合 `cpu`，其后每行一个 `cpuN` → 计数即逻辑核数。
    let stat_lines = sections.get("STAT").map(Vec::as_slice).unwrap_or(&[]);
    let stat = stat_lines
        .first()
        .copied()
        .context("采集输出缺少 /proc/stat")?;
    let ncpu = stat_lines.len().saturating_sub(1).max(1) as f32;
    let (busy, total) = parse_cpu_stat(stat);
    let (cpu_pct, total_delta) = cpu_delta_pct(state, busy, total);

    let mem_lines = sections.get("MEM").map(Vec::as_slice).unwrap_or(&[]);
    let mem_pct = memory_pct(mem_lines).context("解析 /proc/meminfo 失败")?;
    let swap_pct = swap_pct(mem_lines);
    let mem_total_kb = meminfo_kb(mem_lines, "MemTotal:").unwrap_or(0.0);

    let disk_pct = sections
        .get("DISK")
        .and_then(|lines| lines.first())
        .and_then(|line| df_capacity(line))
        .unwrap_or(0.0);

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

    let top = sections
        .get("PS")
        .map(|lines| parse_linux_processes(state, lines, total_delta, ncpu, mem_total_kb))
        .unwrap_or_default();

    Ok(MonitorTick {
        sample: ResourceSample {
            cpu_pct,
            mem_pct,
            swap_pct,
            disk_pct,
            net_rx_kbps,
            net_tx_kbps,
            load_1,
            load_5,
            timestamp: now,
        },
        top,
    })
}

/// 解析 macOS 采集脚本输出。
pub fn parse_macos(
    state: &mut ParseState,
    output: &str,
    elapsed: Duration,
    now: SystemTime,
) -> Result<MonitorTick> {
    let sections = split_sections(output);

    let cpu_lines = sections.get("CPU").map(Vec::as_slice).unwrap_or(&[]);
    let cpu_pct = parse_macos_cpu(cpu_lines);

    let mem_lines = sections.get("MEM").map(Vec::as_slice).unwrap_or(&[]);
    let (mem_pct, swap_pct) = parse_macos_memory(mem_lines)?;

    let disk_pct = sections
        .get("DISK")
        .and_then(|lines| lines.first())
        .and_then(|line| df_capacity(line))
        .unwrap_or(0.0);

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

    let top = sections
        .get("PS")
        .map(|lines| parse_top_processes(lines))
        .unwrap_or_default();

    Ok(MonitorTick {
        sample: ResourceSample {
            cpu_pct,
            mem_pct,
            swap_pct,
            disk_pct,
            net_rx_kbps,
            net_tx_kbps,
            load_1,
            load_5,
            timestamp: now,
        },
        top,
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

/// 返回 (整机 CPU 使用率, 本帧 total 时间片增量)。
///
/// `total_delta` 供进程级 CPU% 复用：进程 utime/stime 与 `/proc/stat`
/// 同源同刻度，用同一个 tick 增量做分母可避免墙钟漂移。
fn cpu_delta_pct(state: &mut ParseState, busy: u64, total: u64) -> (f32, Option<u64>) {
    let result = match state.prev_cpu {
        Some((prev_busy, prev_total)) if total > prev_total => {
            let busy_delta = busy.saturating_sub(prev_busy);
            let total_delta = total - prev_total;
            (
                (busy_delta as f32 / total_delta as f32 * 100.0).clamp(0.0, 100.0),
                Some(total_delta),
            )
        }
        // 首帧没有增量基线，先给 0，下一帧即可得到真实值。
        _ => (0.0, None),
    };
    state.prev_cpu = Some((busy, total));
    result
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

fn df_capacity(line: &str) -> Option<f32> {
    let raw = line.split_whitespace().nth(4)?;
    raw.trim_end_matches('%').parse::<f32>().ok()
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

/// 单帧保留的 Top 进程数量。
const TOP_N: usize = 20;

/// 解析 Linux `/proc` 进程行：`pid \t user \t utime \t stime \t rss_kb \t comm`。
///
/// CPU% 用 tick 增量计算，与 `top` 语义一致（单核跑满 = 100%，多线程可超过 100%）：
///
/// ```text
/// proc% = Δproc_ticks / Δtotal_ticks × ncpu × 100
/// ```
///
/// 首帧没有基线 → 全部为 0；进程消失后 `prev_proc_ticks` 自然淘汰。
fn parse_linux_processes(
    state: &mut ParseState,
    lines: &[&str],
    total_delta: Option<u64>,
    ncpu: f32,
    mem_total_kb: f32,
) -> Vec<TopProcess> {
    let mut next_ticks = HashMap::with_capacity(lines.len());
    let mut rows = Vec::with_capacity(lines.len());

    for line in lines {
        let mut parts = line.split('\t');
        let (Some(pid), Some(user)) = (
            parts.next().and_then(|v| v.parse::<u32>().ok()),
            parts.next(),
        ) else {
            continue;
        };
        let (Some(utime), Some(stime), Some(rss_kb)) = (
            parts.next().and_then(|v| v.parse::<u64>().ok()),
            parts.next().and_then(|v| v.parse::<u64>().ok()),
            parts.next().and_then(|v| v.parse::<f32>().ok()),
        ) else {
            continue;
        };
        let Some(command) = parts.next().filter(|c| !c.is_empty()) else {
            continue;
        };

        let ticks = utime.saturating_add(stime);
        let cpu_pct = match (total_delta, state.prev_proc_ticks.get(&pid)) {
            (Some(delta), Some(previous)) if delta > 0 => {
                let used = ticks.saturating_sub(*previous) as f32;
                (used / delta as f32 * ncpu * 100.0).clamp(0.0, ncpu * 100.0)
            }
            _ => 0.0,
        };
        next_ticks.insert(pid, ticks);

        let mem_pct = if mem_total_kb > 0.0 {
            (rss_kb / mem_total_kb * 100.0).clamp(0.0, 100.0)
        } else {
            0.0
        };

        rows.push(TopProcess {
            pid,
            user: user.to_string(),
            cpu_pct,
            mem_pct,
            command: command.to_string(),
        });
    }

    state.prev_proc_ticks = next_ticks;

    rows.sort_by(|a, b| {
        b.cpu_pct
            .partial_cmp(&a.cpu_pct)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| {
                b.mem_pct
                    .partial_cmp(&a.mem_pct)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
    });
    rows.truncate(TOP_N);
    rows
}

/// 解析 macOS `ps -Ao pid=,user=,%cpu=,%mem=,comm=` 输出。
fn parse_top_processes(lines: &[&str]) -> Vec<TopProcess> {
    lines
        .iter()
        .filter_map(|line| {
            let mut parts = line.split_whitespace();
            let pid = parts.next()?.parse::<u32>().ok()?;
            let user = parts.next()?.to_string();
            let cpu_pct = parts.next()?.parse::<f32>().ok()?;
            let mem_pct = parts.next()?.parse::<f32>().ok()?;
            let command = parts.collect::<Vec<_>>().join(" ");
            if command.is_empty() {
                return None;
            }
            Some(TopProcess {
                pid,
                user,
                cpu_pct,
                mem_pct,
                command,
            })
        })
        .collect()
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

    /// `/proc/stat` 聚合行 + 一条 `cpu0`（ncpu = 1）。
    fn linux_fixture(cpu_line: &str, net_rx: u64, ps: &str) -> String {
        format!(
            "@@RT_STAT@@\n{cpu_line}\ncpu0 {cpu_line_rest}\n@@RT_MEM@@\nMemTotal: 1000 kB\nMemAvailable: 400 kB\nSwapTotal: 200 kB\nSwapFree: 50 kB\n@@RT_DISK@@\n/dev/sda1 1000 600 400 60% /\n@@RT_NET@@\nInter-|   Receive                                                |  Transmit\n face |bytes    packets errs drop fifo frame compressed multicast|bytes    packets errs drop fifo colls carrier compressed\n    lo: 999999 0 0 0 0 0 0 0 999999 0 0 0 0 0 0 0\n  eth0: {net_rx} 10 0 0 0 0 0 0 2048 0 0 0 0 0 0 0\n@@RT_LOAD@@\n1.50 0.75 0.50 1/100 12345\n@@RT_PS@@\n{ps}",
            cpu_line_rest = cpu_line.trim_start_matches("cpu"),
        )
    }

    /// 首帧进程：pid 120 已用 150 ticks，pid 121 已用 15 ticks。
    const PS_FIRST: &str =
        "120\troot\t100\t50\t600\tnginx: worker process\n121\tubuntu\t10\t5\t300\tbash\n";

    /// 次帧进程：120 → 300 ticks（Δ150），121 → 45 ticks（Δ30），新增 122。
    const PS_SECOND: &str =
        "120\troot\t200\t100\t600\tnginx: worker process\n121\tubuntu\t30\t15\t300\tbash\n122\troot\t0\t0\t100\tnewproc\n";

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
            &linux_fixture("cpu 100 0 100 800 0 0 0 0", 1024, PS_FIRST),
            Duration::from_secs(2),
            now,
        )
        .unwrap();
        assert_eq!(first.sample.cpu_pct, 0.0, "首帧没有增量基线");
        approx(first.sample.mem_pct, 60.0);
        approx(first.sample.swap_pct, 75.0);
        approx(first.sample.disk_pct, 60.0);
        assert_eq!(first.sample.net_rx_kbps, 0.0);
        approx(first.sample.load_1, 1.5);
        approx(first.sample.load_5, 0.75);
        assert_eq!(first.top.len(), 2);
        assert_eq!(first.top[0].command, "nginx: worker process");
        assert_eq!(first.top[0].pid, 120);
        assert_eq!(first.top[0].cpu_pct, 0.0, "首帧没有进程 tick 基线");
        approx(first.top[0].mem_pct, 60.0);

        // 第二帧：CPU busy 增量 200 / total 增量 400 → 50%；网卡 +2048 B/2s → 1 KiB/s
        let second = parse_linux(
            &mut state,
            &linux_fixture("cpu 200 0 200 1000 0 0 0 0", 3072, PS_SECOND),
            Duration::from_secs(2),
            now,
        )
        .unwrap();
        assert_eq!(second.sample.cpu_pct, 50.0);
        assert_eq!(second.sample.net_rx_kbps, 1.0);
        assert_eq!(second.sample.net_tx_kbps, 0.0);
        // Δproc 150 / Δtotal 400 × ncpu 1 × 100 = 37.5%
        assert_eq!(second.top.len(), 3);
        assert_eq!(second.top[0].pid, 120);
        approx(second.top[0].cpu_pct, 37.5);
        // Δproc 30 / Δtotal 400 × 100 = 7.5%
        assert_eq!(second.top[1].pid, 121);
        approx(second.top[1].cpu_pct, 7.5);
        // 新进程没有基线 → 0%
        assert_eq!(second.top[2].pid, 122);
        assert_eq!(second.top[2].cpu_pct, 0.0);
        approx(second.top[2].mem_pct, 10.0);
    }

    #[test]
    fn linux_process_ticks_prune_dead_pids() {
        let mut state = ParseState::new();
        let now = SystemTime::UNIX_EPOCH;
        parse_linux(
            &mut state,
            &linux_fixture("cpu 100 0 100 800 0 0 0 0", 0, PS_FIRST),
            Duration::from_secs(1),
            now,
        )
        .unwrap();
        // 次帧只剩 pid 120：121 已退出，基线不应残留
        let tick = parse_linux(
            &mut state,
            &linux_fixture(
                "cpu 150 0 150 900 0 0 0 0",
                0,
                "120\troot\t200\t100\t600\tnginx: worker process\n",
            ),
            Duration::from_secs(1),
            now,
        )
        .unwrap();
        assert_eq!(tick.top.len(), 1);
        // Δtotal = 200，Δproc = 150 → 75%
        approx(tick.top[0].cpu_pct, 75.0);
        assert_eq!(state.prev_proc_ticks.len(), 1, "已退出进程的基线被淘汰");
    }

    #[test]
    fn linux_process_rows_skip_malformed() {
        let mut rows_state = ParseState::new();
        let rows = parse_linux_processes(
            &mut rows_state,
            &[
                "not-a-pid\troot\t1\t1\t10\tbad",
                "7\troot\tnot-a-number\t1\t10\tbad",
                "8\troot\t1\t1\t10\t", // 命令为空 → 丢弃
                "9\troot\t1\t1\t10\tok",
            ],
            Some(100),
            1.0,
            1000.0,
        );
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].pid, 9);
    }

    #[test]
    fn linux_cpu_counter_reset_is_zero() {
        let mut state = ParseState::new();
        let now = SystemTime::UNIX_EPOCH;
        parse_linux(
            &mut state,
            &linux_fixture("cpu 500 0 500 1000 0 0 0 0", 0, PS_FIRST),
            Duration::from_secs(2),
            now,
        )
        .unwrap();
        let tick = parse_linux(
            &mut state,
            &linux_fixture("cpu 10 0 10 20 0 0 0 0", 0, PS_FIRST),
            Duration::from_secs(2),
            now,
        )
        .unwrap();
        assert_eq!(tick.sample.cpu_pct, 0.0, "计数器回绕不应出现负值/异常值");
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
            &linux_fixture("cpu 1 0 1 8 0 0 0 0", 0, PS_FIRST),
            Duration::from_secs(1),
            now,
        )
        .unwrap();
        let tick = parse_linux(
            &mut state,
            &linux_fixture("cpu 2 0 2 16 0 0 0 0", 0, PS_FIRST),
            Duration::from_secs(1),
            now,
        )
        .unwrap();
        assert_eq!(tick.sample.net_rx_kbps, 0.0);
    }

    #[test]
    fn top_process_command_keeps_spaces() {
        let rows = parse_top_processes(&[
            "  42 root 1.0 2.0 /usr/bin/python3 worker.py",
            "not-a-process line",
        ]);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].command, "/usr/bin/python3 worker.py");
    }

    #[test]
    fn parses_df_percentage() {
        assert_eq!(df_capacity("/dev/x 10 9 1 90% /"), Some(90.0));
        assert_eq!(df_capacity("garbage"), None);
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
@@RT_PS@@
  501 root 25.0 5.0 /usr/libexec/foo
"#;
        let mut state = ParseState::new();
        let now = SystemTime::UNIX_EPOCH;
        let first = parse_macos(&mut state, output, Duration::from_secs(2), now).unwrap();
        assert!((first.sample.cpu_pct - 15.425).abs() < 0.01);
        // (200000 + 100000 + 50000) * 16384 / 17179869184 * 100 ≈ 33.36
        assert!((first.sample.mem_pct - 33.36).abs() < 0.05);
        assert!((first.sample.swap_pct - 25.0).abs() < 0.01);
        assert_eq!(first.sample.disk_pct, 55.0);
        assert_eq!(first.sample.load_1, 1.23);
        assert_eq!(first.sample.load_5, 4.56);
        assert_eq!(first.top[0].pid, 501);

        let second = parse_macos(&mut state, output, Duration::from_secs(2), now).unwrap();
        assert_eq!(second.sample.net_rx_kbps, 0.0);
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

//! 后台采集线程 — 独占一条独立 SSH 连接，串行执行采样。
//!
//! 线程模型与 S7.5 一致：std::thread + futures unbounded channel，
//! UI 侧只通过 `MonitorHandle` 发命令、从事件通道收结果。

use std::io::Read;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use anyhow::{anyhow, Context, Result};
use futures::channel::mpsc::UnboundedSender;

use rterm_ssh::SshSession;

use crate::alerts::{AlertGate, RollingAverage};
use crate::cache::DiskScanCache;
use crate::parse::{parse_disk_usage, parse_linux, parse_macos, ParseState};
use crate::scripts::{disk_scan_script, LINUX_SCRIPT, MACOS_SCRIPT, PROBE_SCRIPT};
use crate::types::{DiskUsage, MonitorCommand, MonitorConfig, MonitorEvent};

/// 单次命令输出上限，防止异常脚本把内存打满。
const MAX_OUTPUT_BYTES: usize = 512 * 1024;
/// 目录扫描可能远超采样命令耗时，临时放宽会话读取超时（2 分钟）。
const SCAN_TIMEOUT_MS: u32 = 120_000;
/// 目录扫描结果缓存时长。
const DISK_CACHE_TTL: Duration = Duration::from_secs(30);
/// 事件发送失败（UI 已关闭）时线程尽快退出。
const STOP_CHECK_INTERVAL: Duration = Duration::from_millis(100);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RemoteOs {
    Linux,
    Macos,
}

/// 采集线程句柄。
///
/// `Drop` / [`shutdown`](Self::shutdown) 只置停止位，不 join：
/// 线程可能正阻塞在 SSH 读取上（最长一个连接超时），UI 线程不能等待。
pub struct MonitorHandle {
    stop: Arc<AtomicBool>,
    commands: Sender<MonitorCommand>,
}

impl MonitorHandle {
    /// 向采集线程投递命令；线程已退出时返回 false。
    pub fn send(&self, command: MonitorCommand) -> bool {
        self.commands.send(command).is_ok()
    }

    /// 请求线程退出（异步，不阻塞调用方）。
    pub fn shutdown(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

impl Drop for MonitorHandle {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// 启动后台采集线程。`ssh` 必须已完成认证，线程接管其生命周期。
pub fn spawn(
    ssh: SshSession,
    config: MonitorConfig,
    events: UnboundedSender<MonitorEvent>,
) -> Result<MonitorHandle> {
    let stop = Arc::new(AtomicBool::new(false));
    let (commands, command_rx) = mpsc::channel();
    let thread_stop = stop.clone();

    std::thread::Builder::new()
        .name("rterm-monitor".into())
        .spawn(move || run(ssh, config.sanitized(), events, command_rx, thread_stop))
        .map_err(|error| anyhow!("无法启动监控线程: {error}"))?;

    Ok(MonitorHandle { stop, commands })
}

fn run(
    mut ssh: SshSession,
    mut config: MonitorConfig,
    events: UnboundedSender<MonitorEvent>,
    commands: Receiver<MonitorCommand>,
    stop: Arc<AtomicBool>,
) {
    let Some(session) = ssh.session_clone() else {
        let _ = events.unbounded_send(MonitorEvent::Error("SSH 会话不可用".into()));
        return;
    };
    let os = detect_os(&session);
    log::info!(
        "[monitor] collector started (os = {os:?}, interval = {:?})",
        config.interval
    );

    let mut state = ParseState::new();
    let mut cpu_avg = RollingAverage::new(5);
    let mut mem_avg = RollingAverage::new(5);
    let mut gate = AlertGate::new();
    let mut disk_cache = DiskScanCache::new(DISK_CACHE_TTL);
    let mut last_tick: Option<Instant> = None;
    let mut paused = false;
    let mut next_tick = Instant::now();

    loop {
        if stop.load(Ordering::Relaxed) {
            break;
        }

        if !paused && Instant::now() >= next_tick {
            tick(
                &session,
                os,
                &mut state,
                &mut cpu_avg,
                &mut mem_avg,
                &mut gate,
                &config,
                &events,
                &mut last_tick,
            );
            next_tick = Instant::now() + config.interval;
        }

        let wait = if paused {
            STOP_CHECK_INTERVAL
        } else {
            next_tick
                .saturating_duration_since(Instant::now())
                .max(STOP_CHECK_INTERVAL)
        };
        match commands.recv_timeout(wait) {
            Ok(MonitorCommand::Pause) => {
                paused = true;
                log::info!("[monitor] paused");
            }
            Ok(MonitorCommand::Resume) => {
                paused = false;
                next_tick = Instant::now();
                log::info!("[monitor] resumed");
            }
            Ok(MonitorCommand::SetConfig(config_update)) => {
                config = config_update.sanitized();
                next_tick = Instant::now();
                log::info!("[monitor] config updated: {:?}", config);
            }
            Ok(MonitorCommand::ScanDisk {
                request_id,
                path,
                force,
            }) => {
                // 导航优先复用 30 秒缓存；刷新（force）才真正重新扫描。
                let cached = if force { None } else { disk_cache.get(&path) };
                let from_cache = cached.is_some();
                let result = match cached {
                    Some(usage) => {
                        log::info!("[monitor] disk scan cache hit: {path}");
                        Ok(usage)
                    }
                    None => scan_disk(&session, &path)
                        .inspect(|usage| {
                            disk_cache.insert(usage.clone());
                        })
                        .map_err(|error| format!("{error:#}")),
                };
                let _ = events.unbounded_send(MonitorEvent::DiskScan { request_id, result });
                if !from_cache {
                    // 扫描期间无法采样，恢复后重新建立 CPU / 网络增量基线。
                    last_tick = None;
                    next_tick = Instant::now();
                }
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }
    }

    ssh.disconnect();
    log::info!("[monitor] collector stopped");
}

#[allow(clippy::too_many_arguments)]
fn tick(
    session: &ssh2::Session,
    os: RemoteOs,
    state: &mut ParseState,
    cpu_avg: &mut RollingAverage,
    mem_avg: &mut RollingAverage,
    gate: &mut AlertGate,
    config: &MonitorConfig,
    events: &UnboundedSender<MonitorEvent>,
    last_tick: &mut Option<Instant>,
) {
    let now = Instant::now();
    let elapsed = last_tick
        .map(|previous| now.saturating_duration_since(previous))
        .unwrap_or(config.interval);
    *last_tick = Some(now);

    let script = match os {
        RemoteOs::Linux => LINUX_SCRIPT,
        RemoteOs::Macos => MACOS_SCRIPT,
    };
    let output = match exec_capture(session, script) {
        Ok(output) => output,
        Err(error) => {
            let _ = events.unbounded_send(MonitorEvent::Error(format!("采集失败: {error:#}")));
            return;
        }
    };

    let parsed = match os {
        RemoteOs::Linux => parse_linux(state, &output, elapsed, SystemTime::now()),
        RemoteOs::Macos => parse_macos(state, &output, elapsed, SystemTime::now()),
    };
    let sample = match parsed {
        Ok(sample) => sample,
        Err(error) => {
            let _ =
                events.unbounded_send(MonitorEvent::Error(format!("解析采集结果失败: {error:#}")));
            return;
        }
    };

    let cpu_avg_value = cpu_avg.push(sample.cpu_pct);
    let mem_avg_value = mem_avg.push(sample.mem_pct);
    let alerts = [
        gate.evaluate(
            crate::types::Metric::Cpu,
            cpu_avg_value,
            config.cpu_threshold,
        ),
        gate.evaluate(
            crate::types::Metric::Mem,
            mem_avg_value,
            config.mem_threshold,
        ),
    ];

    let _ = events.unbounded_send(MonitorEvent::Tick(sample));
    for alert in alerts.into_iter().flatten() {
        log::warn!(
            "[monitor] alert {} = {:.1}% (threshold {:.1}%)",
            alert.metric.label(),
            alert.value,
            alert.threshold
        );
        let _ = events.unbounded_send(MonitorEvent::Alert(alert));
    }
}

fn scan_disk(session: &ssh2::Session, path: &str) -> Result<DiskUsage> {
    if !path.starts_with('/') || path.contains('\0') {
        return Err(anyhow!("目录路径必须是绝对路径: {path}"));
    }
    let started = Instant::now();
    // 大目录递归统计可能超过采样连接的读取超时，扫描期间临时放宽。
    let previous_timeout = session.timeout();
    session.set_timeout(SCAN_TIMEOUT_MS);
    let output = exec_capture(session, &disk_scan_script(path));
    session.set_timeout(previous_timeout);
    let output = output.context("目录扫描命令执行失败")?;
    let usage = parse_disk_usage(path, &output)?;
    log::info!(
        "[monitor] scanned {} ({} entries, {:.1} MB) in {:?}",
        usage.path,
        usage.entries.len(),
        usage.total_bytes as f64 / 1024.0 / 1024.0,
        started.elapsed()
    );
    Ok(usage)
}

fn detect_os(session: &ssh2::Session) -> RemoteOs {
    match exec_capture(session, PROBE_SCRIPT) {
        Ok(output) if output.contains("Darwin") => RemoteOs::Macos,
        Ok(_) => RemoteOs::Linux,
        Err(error) => {
            log::warn!("[monitor] uname 探测失败，按 Linux 处理: {error:#}");
            RemoteOs::Linux
        }
    }
}

/// 执行远端命令并读取 stdout（stderr 由调用方在脚本里重定向）。
fn exec_capture(session: &ssh2::Session, command: &str) -> Result<String> {
    let mut channel = session.channel_session().context("创建 SSH 通道失败")?;
    channel.exec(command).context("执行远端命令失败")?;

    let mut output = Vec::new();
    let mut buffer = [0_u8; 8192];
    loop {
        match channel.read(&mut buffer) {
            Ok(0) => break,
            Ok(read) => {
                output.extend_from_slice(&buffer[..read]);
                if output.len() >= MAX_OUTPUT_BYTES {
                    log::warn!("[monitor] 命令输出超过 {MAX_OUTPUT_BYTES} 字节，截断");
                    break;
                }
            }
            Err(error) => return Err(anyhow!("读取远端命令输出失败: {error}")),
        }
    }
    Ok(String::from_utf8_lossy(&output).into_owned())
}

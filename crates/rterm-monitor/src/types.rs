//! 监控领域类型 — 与采集实现、UI 完全解耦的纯数据模型。

use std::time::{Duration, SystemTime};

/// 一次采样得到的资源指标快照。
///
/// 百分比统一使用 `0..=100`；网络速率单位 KiB/s。
#[derive(Clone, Debug, PartialEq)]
pub struct ResourceSample {
    pub cpu_pct: f32,
    pub mem_pct: f32,
    pub swap_pct: f32,
    pub disk_pct: f32,
    pub net_rx_kbps: f32,
    pub net_tx_kbps: f32,
    pub load_1: f32,
    pub load_5: f32,
    pub timestamp: SystemTime,
}

impl Default for ResourceSample {
    fn default() -> Self {
        Self {
            cpu_pct: 0.0,
            mem_pct: 0.0,
            swap_pct: 0.0,
            disk_pct: 0.0,
            net_rx_kbps: 0.0,
            net_tx_kbps: 0.0,
            load_1: 0.0,
            load_5: 0.0,
            timestamp: SystemTime::UNIX_EPOCH,
        }
    }
}

impl ResourceSample {
    /// 全 0 样本（首帧尚未拿到 CPU / 网络增量时使用）。
    pub fn zeroed(timestamp: SystemTime) -> Self {
        Self {
            timestamp,
            ..Self::default()
        }
    }

    /// 按指标取值，供图表 / 告警复用。
    pub fn metric(&self, metric: Metric) -> f32 {
        match metric {
            Metric::Cpu => self.cpu_pct,
            Metric::Mem => self.mem_pct,
            Metric::Swap => self.swap_pct,
            Metric::Disk => self.disk_pct,
            Metric::NetRx => self.net_rx_kbps,
            Metric::NetTx => self.net_tx_kbps,
            Metric::Load1 => self.load_1,
        }
    }
}

/// 图表可展示的指标。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Metric {
    Cpu,
    Mem,
    Swap,
    Disk,
    NetRx,
    NetTx,
    Load1,
}

impl Metric {
    /// 中文短标签（卡片标题 / 告警文案）。
    pub fn label(self) -> &'static str {
        match self {
            Self::Cpu => "CPU",
            Self::Mem => "内存",
            Self::Swap => "交换分区",
            Self::Disk => "磁盘",
            Self::NetRx => "下行",
            Self::NetTx => "上行",
            Self::Load1 => "负载",
        }
    }
}

/// Top 进程行。
#[derive(Clone, Debug, PartialEq)]
pub struct TopProcess {
    pub pid: u32,
    pub user: String,
    pub cpu_pct: f32,
    pub mem_pct: f32,
    pub command: String,
}

/// 一次采集的完整结果。
#[derive(Clone, Debug)]
pub struct MonitorTick {
    pub sample: ResourceSample,
    pub top: Vec<TopProcess>,
}

/// 阈值告警（同一指标一次超限只报一次，回落后重新布防）。
#[derive(Clone, Debug, PartialEq)]
pub struct MonitorAlert {
    pub metric: Metric,
    pub value: f32,
    pub threshold: f32,
}

/// 采集配置：由调用方从全局 Settings 映射而来。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MonitorConfig {
    pub interval: Duration,
    pub cpu_threshold: f32,
    pub mem_threshold: f32,
}

impl Default for MonitorConfig {
    fn default() -> Self {
        Self {
            interval: Duration::from_secs(2),
            cpu_threshold: 90.0,
            mem_threshold: 85.0,
        }
    }
}

impl MonitorConfig {
    /// 把外部配置收敛到安全范围，避免 0 间隔 / 负阈值。
    pub fn sanitized(self) -> Self {
        let secs = self.interval.as_secs_f32().clamp(0.5, 60.0);
        Self {
            interval: Duration::from_secs_f32(secs),
            cpu_threshold: self.cpu_threshold.clamp(0.0, 100.0),
            mem_threshold: self.mem_threshold.clamp(0.0, 100.0),
        }
    }
}

/// 终止进程时使用的信号。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KillSignal {
    Term,
    Kill,
}

impl KillSignal {
    pub(crate) fn flag(self) -> &'static str {
        match self {
            Self::Term => "TERM",
            Self::Kill => "KILL",
        }
    }
}

/// 采集线程推给 UI 的事件。
#[derive(Debug)]
pub enum MonitorEvent {
    Tick(MonitorTick),
    Alert(MonitorAlert),
    Error(String),
    KillResult { pid: u32, ok: bool, message: String },
}

/// UI → 采集线程的命令（同一线程串行执行，避免并发操作 SSH Session）。
pub enum MonitorCommand {
    Pause,
    Resume,
    SetConfig(MonitorConfig),
    Kill { pid: u32, signal: KillSignal },
}

impl KillSignal {
    pub(crate) fn command(self, pid: u32) -> String {
        format!("kill -{} {} 2>&1 || echo RT_KILL_FAIL", self.flag(), pid)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_is_sanitized() {
        let config = MonitorConfig {
            interval: Duration::from_millis(10),
            cpu_threshold: 250.0,
            mem_threshold: -5.0,
        }
        .sanitized();
        assert_eq!(config.interval, Duration::from_secs_f32(0.5));
        assert_eq!(config.cpu_threshold, 100.0);
        assert_eq!(config.mem_threshold, 0.0);
    }

    #[test]
    fn kill_commands_are_well_formed() {
        assert_eq!(
            KillSignal::Term.command(42),
            "kill -TERM 42 2>&1 || echo RT_KILL_FAIL"
        );
        assert_eq!(
            KillSignal::Kill.command(7),
            "kill -KILL 7 2>&1 || echo RT_KILL_FAIL"
        );
    }

    #[test]
    fn sample_metric_lookup() {
        let sample = ResourceSample {
            cpu_pct: 10.0,
            mem_pct: 20.0,
            swap_pct: 30.0,
            disk_pct: 40.0,
            net_rx_kbps: 50.0,
            net_tx_kbps: 60.0,
            load_1: 1.5,
            load_5: 0.5,
            timestamp: SystemTime::UNIX_EPOCH,
        };
        assert_eq!(sample.metric(Metric::Cpu), 10.0);
        assert_eq!(sample.metric(Metric::Mem), 20.0);
        assert_eq!(sample.metric(Metric::NetTx), 60.0);
        assert_eq!(sample.metric(Metric::Load1), 1.5);
    }
}

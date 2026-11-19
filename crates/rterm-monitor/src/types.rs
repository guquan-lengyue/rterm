//! 监控领域类型 — 与采集实现、UI 完全解耦的纯数据模型。

use std::time::{Duration, SystemTime};

/// 一次采样得到的资源指标快照。
///
/// 百分比统一使用 `0..=100`；网络速率单位 KiB/s；磁盘容量单位字节。
#[derive(Clone, Debug, PartialEq)]
pub struct ResourceSample {
    pub cpu_pct: f32,
    pub mem_pct: f32,
    pub swap_pct: f32,
    pub disk_pct: f32,
    /// 根文件系统总容量（字节）。
    pub disk_total_bytes: u64,
    /// 根文件系统已用容量（字节）。
    pub disk_used_bytes: u64,
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
            disk_total_bytes: 0,
            disk_used_bytes: 0,
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

/// 目录占用扫描中的一个条目（目录或文件）。
#[derive(Clone, Debug, PartialEq)]
pub struct DiskEntry {
    pub name: String,
    pub path: String,
    pub bytes: u64,
    pub is_dir: bool,
}

/// 一次目录占用扫描的结果。
#[derive(Clone, Debug, PartialEq)]
pub struct DiskUsage {
    /// 被扫描的目录（绝对路径）。
    pub path: String,
    /// 该目录内所有内容合计（`du` 汇总值）。
    pub total_bytes: u64,
    /// 直接子项，按占用从大到小排序。
    pub entries: Vec<DiskEntry>,
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

/// 采集线程推给 UI 的事件。
#[derive(Debug)]
pub enum MonitorEvent {
    Tick(ResourceSample),
    Alert(MonitorAlert),
    /// 目录占用扫描结果；`request_id` 用于丢弃过期的导航请求。
    DiskScan {
        request_id: u64,
        result: Result<DiskUsage, String>,
    },
    Error(String),
}

/// UI → 采集线程的命令（同一线程串行执行，避免并发操作 SSH Session）。
pub enum MonitorCommand {
    Pause,
    Resume,
    SetConfig(MonitorConfig),
    /// 扫描指定目录的直接子项占用（会暂停一次采样）。
    /// `force = false` 时优先复用 30 秒内的扫描缓存。
    ScanDisk {
        request_id: u64,
        path: String,
        force: bool,
    },
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
    fn sample_metric_lookup() {
        let sample = ResourceSample {
            cpu_pct: 10.0,
            mem_pct: 20.0,
            swap_pct: 30.0,
            disk_pct: 40.0,
            disk_total_bytes: 2_000,
            disk_used_bytes: 800,
            net_rx_kbps: 50.0,
            net_tx_kbps: 60.0,
            load_1: 1.5,
            load_5: 0.5,
            timestamp: SystemTime::UNIX_EPOCH,
        };
        assert_eq!(sample.metric(Metric::Cpu), 10.0);
        assert_eq!(sample.metric(Metric::Mem), 20.0);
        assert_eq!(sample.metric(Metric::Disk), 40.0);
        assert_eq!(sample.metric(Metric::NetTx), 60.0);
        assert_eq!(sample.metric(Metric::Load1), 1.5);
    }

    #[test]
    fn zeroed_sample_has_empty_disk() {
        let sample = ResourceSample::zeroed(SystemTime::UNIX_EPOCH);
        assert_eq!(sample.disk_total_bytes, 0);
        assert_eq!(sample.disk_used_bytes, 0);
    }
}

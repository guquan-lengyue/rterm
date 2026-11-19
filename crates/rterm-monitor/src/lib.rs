//! rterm-monitor — 远程性能监控采集。
//!
//! 分层：
//! - [`types`]：纯数据模型（UI / 采集共用）
//! - [`scripts`]：远端采集脚本与段落标记
//! - [`parse`]：Linux / macOS 输出解析（纯函数，可单测）
//! - [`alerts`]：阈值告警闸门与滑动平均
//! - [`cache`]：目录扫描结果的短期缓存
//! - [`collector`]：独立 SSH 连接 + 后台采样线程

pub mod alerts;
pub mod cache;
pub mod collector;
pub mod parse;
pub mod scripts;
pub mod types;

pub use alerts::{AlertGate, RollingAverage};
pub use cache::DiskScanCache;
pub use collector::{spawn, MonitorHandle};
pub use parse::{parse_disk_usage, parse_linux, parse_macos, ParseState};
pub use types::{
    DiskEntry, DiskUsage, Metric, MonitorAlert, MonitorCommand, MonitorConfig, MonitorEvent,
    ResourceSample,
};

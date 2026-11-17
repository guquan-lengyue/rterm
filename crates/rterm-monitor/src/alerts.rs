//! 阈值告警 — 带回落滞回的“一次超限只报一次”闸门。

use crate::types::{Metric, MonitorAlert};

/// CPU / 内存各自的布防状态。
#[derive(Debug, Default)]
pub struct AlertGate {
    cpu_armed: bool,
    mem_armed: bool,
}

impl AlertGate {
    pub fn new() -> Self {
        Self::default()
    }

    /// 传入滑动平均与阈值；达到阈值且此前已回落时返回一条告警。
    ///
    /// 回落判定使用 95% 滞回，避免在阈值附近抖动导致告警风暴。
    pub fn evaluate(&mut self, metric: Metric, value: f32, threshold: f32) -> Option<MonitorAlert> {
        if !threshold.is_finite() || threshold <= 0.0 {
            return None;
        }
        let armed = match metric {
            Metric::Cpu => &mut self.cpu_armed,
            Metric::Mem => &mut self.mem_armed,
            _ => return None,
        };
        if value >= threshold {
            if !*armed {
                *armed = true;
                return Some(MonitorAlert {
                    metric,
                    value,
                    threshold,
                });
            }
        } else if value < threshold * 0.95 {
            *armed = false;
        }
        None
    }
}

/// 固定长度的滑动平均（只保留最近 N 个点）。
#[derive(Debug)]
pub struct RollingAverage {
    capacity: usize,
    values: std::collections::VecDeque<f32>,
}

impl RollingAverage {
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity: capacity.max(1),
            values: std::collections::VecDeque::new(),
        }
    }

    pub fn push(&mut self, value: f32) -> f32 {
        self.values.push_back(value);
        while self.values.len() > self.capacity {
            self.values.pop_front();
        }
        self.average()
    }

    pub fn average(&self) -> f32 {
        if self.values.is_empty() {
            return 0.0;
        }
        self.values.iter().sum::<f32>() / self.values.len() as f32
    }

    pub fn len(&self) -> usize {
        self.values.len()
    }

    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alert_fires_once_until_recovered() {
        let mut gate = AlertGate::new();
        assert!(gate.evaluate(Metric::Cpu, 95.0, 90.0).is_some());
        assert!(
            gate.evaluate(Metric::Cpu, 96.0, 90.0).is_none(),
            "持续超限不重复告警"
        );
        // 未跌破滞回区间仍保持布防
        assert!(gate.evaluate(Metric::Cpu, 87.0, 90.0).is_none());
        // 跌破 85.5 后重新布防
        assert!(gate.evaluate(Metric::Cpu, 80.0, 90.0).is_none());
        assert!(gate.evaluate(Metric::Cpu, 91.0, 90.0).is_some());
    }

    #[test]
    fn cpu_and_mem_are_independent() {
        let mut gate = AlertGate::new();
        assert!(gate.evaluate(Metric::Cpu, 95.0, 90.0).is_some());
        assert!(gate.evaluate(Metric::Mem, 90.0, 85.0).is_some());
    }

    #[test]
    fn invalid_threshold_never_alerts() {
        let mut gate = AlertGate::new();
        assert!(gate.evaluate(Metric::Cpu, 100.0, 0.0).is_none());
        assert!(gate.evaluate(Metric::Cpu, 100.0, -1.0).is_none());
    }

    #[test]
    fn rolling_average_keeps_window() {
        let mut avg = RollingAverage::new(3);
        assert_eq!(avg.push(10.0), 10.0);
        assert_eq!(avg.push(20.0), 15.0);
        assert_eq!(avg.push(30.0), 20.0);
        assert_eq!(avg.push(60.0), 110.0 / 3.0);
        assert_eq!(avg.len(), 3);
    }
}

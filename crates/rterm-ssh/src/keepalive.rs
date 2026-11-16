//! KeepAlive — SSH 心跳计时器
//!
//! S5 由 PTY IO 线程驱动：每隔 N 秒调用 `Session::keepalive_send()`。
//! `interval_secs == 0` 表示关闭。

use std::time::{Duration, Instant};

pub struct KeepAliveTimer {
    interval: Option<Duration>,
    last_sent: Instant,
}

impl KeepAliveTimer {
    pub fn new(interval_secs: u32) -> Self {
        Self {
            interval: (interval_secs > 0).then(|| Duration::from_secs(interval_secs as u64)),
            last_sent: Instant::now(),
        }
    }

    pub fn is_enabled(&self) -> bool {
        self.interval.is_some()
    }

    pub fn due(&self) -> bool {
        match self.interval {
            Some(interval) => self.last_sent.elapsed() >= interval,
            None => false,
        }
    }

    pub fn mark_sent(&mut self) {
        self.last_sent = Instant::now();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_interval_disables_keepalive() {
        let timer = KeepAliveTimer::new(0);
        assert!(!timer.is_enabled());
        assert!(!timer.due());
    }

    #[test]
    fn positive_interval_is_enabled() {
        let timer = KeepAliveTimer::new(30);
        assert!(timer.is_enabled());
        assert!(!timer.due());
    }
}

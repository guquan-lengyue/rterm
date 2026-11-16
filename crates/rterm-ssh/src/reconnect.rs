//! 重连策略 — 指数退避 1s / 2s / 4s / 8s / 16s，最多 5 次。

use std::time::Duration;

#[derive(Clone, Copy, Debug)]
pub struct ReconnectPolicy {
    pub max_attempts: u8,
}

impl Default for ReconnectPolicy {
    fn default() -> Self {
        Self { max_attempts: 5 }
    }
}

impl ReconnectPolicy {
    /// `attempt` 从 1 开始。
    pub fn delay(attempt: u8) -> Duration {
        let shift = attempt.saturating_sub(1).min(4) as u32;
        Duration::from_secs(1u64 << shift)
    }

    pub fn should_retry(&self, attempt: u8) -> bool {
        attempt <= self.max_attempts
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_sequence() {
        assert_eq!(ReconnectPolicy::delay(1), Duration::from_secs(1));
        assert_eq!(ReconnectPolicy::delay(2), Duration::from_secs(2));
        assert_eq!(ReconnectPolicy::delay(3), Duration::from_secs(4));
        assert_eq!(ReconnectPolicy::delay(4), Duration::from_secs(8));
        assert_eq!(ReconnectPolicy::delay(5), Duration::from_secs(16));
        assert_eq!(ReconnectPolicy::delay(9), Duration::from_secs(16));
    }
}

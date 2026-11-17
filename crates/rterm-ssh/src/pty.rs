//! PTY — 打开远程 shell + 后台读写线程
//!
//! ssh2 的 blocking read 会锁住整个 Session，导致输入写不出去，
//! 所以 shell 打开后切换 non-blocking，由单线程轮询：
//! - 读 PTY 输出 → `PtyEvent::Data`
//! - 从命令通道取键盘输入 / resize
//! - 定期 `keepalive_send`

use std::io::{Read, Write};
use std::sync::mpsc::{Receiver, TryRecvError};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use futures::channel::mpsc::Sender;

use crate::keepalive::KeepAliveTimer;
use crate::known_hosts::{HostKeyChoice, HostKeyInfo};

/// UI → 后台线程的命令。
#[derive(Debug)]
pub enum PtyCommand {
    Write(Vec<u8>),
    Resize { cols: u32, rows: u32 },
    Close,
}

/// 后台线程 → UI 的事件。
#[derive(Debug)]
pub enum PtyEvent {
    /// 未知 / 变更主机密钥：暂停连接并等待 UI 三选一。
    HostKeyPrompt {
        info: HostKeyInfo,
        reply: std::sync::mpsc::Sender<HostKeyChoice>,
    },
    Connected,
    Data(Vec<u8>),
    Closed {
        reason: String,
    },
    /// `retryable == false` 时（认证失败 / 主机密钥变更 / 配置错误）不应自动重连。
    Failed {
        reason: String,
        retryable: bool,
    },
}

/// 向 UI 发送事件；队列满时阻塞重试（背压），不丢弃事件。
///
/// 返回 `false` 表示接收端已关闭，调用方应退出 IO 循环。
pub fn send_event(output: &mut Sender<PtyEvent>, event: PtyEvent) -> bool {
    let mut event = event;
    loop {
        match output.try_send(event) {
            Ok(()) => return true,
            Err(error) if error.is_full() => {
                // 队列满：保留事件，等待 UI 消费后重试，避免丢弃 Data 破坏终端状态
                event = error.into_inner();
                std::thread::sleep(Duration::from_millis(1));
            }
            Err(error) => {
                let _ = error.into_inner();
                log::warn!("[pty] PTY 事件通道已关闭，接收端不存在");
                return false;
            }
        }
    }
}

/// 空闲轮询间隔：活跃时保持 8ms，连续空闲约 1s 后逐步拉长到 50ms。
fn idle_poll_interval(idle_for: Duration) -> Duration {
    const ACTIVE_INTERVAL: Duration = Duration::from_millis(8);
    if idle_for < Duration::from_millis(1000) {
        ACTIVE_INTERVAL
    } else if idle_for < Duration::from_millis(2000) {
        Duration::from_millis(16)
    } else if idle_for < Duration::from_millis(4000) {
        Duration::from_millis(32)
    } else {
        Duration::from_millis(50)
    }
}

/// IO 线程唤醒句柄（9.19）。
///
/// 空闲时 IO 线程用 `park_timeout` 代替 `sleep`；UI 每次发送 `PtyCommand`
/// 后调用 [`IoWaker::wake`]，线程立即被唤醒处理输入，不再等最长 50ms 的轮询间隔。
#[derive(Clone, Debug, Default)]
pub struct IoWaker {
    thread: std::sync::Arc<std::sync::Mutex<Option<std::thread::Thread>>>,
}

impl IoWaker {
    pub fn new() -> Self {
        Self::default()
    }

    /// 由 IO 线程启动时登记自己（之后 UI 的 `wake` 才能命中）。
    fn install(&self) {
        if let Ok(mut slot) = self.thread.lock() {
            *slot = Some(std::thread::current());
        }
    }

    /// UI 线程调用：唤醒正在 `park_timeout` 的 IO 线程。
    /// 线程尚未启动 / 已退出时是无操作。
    pub fn wake(&self) {
        let thread = self
            .thread
            .lock()
            .ok()
            .and_then(|slot| slot.as_ref().cloned());
        if let Some(thread) = thread {
            thread.unpark();
        }
    }
}

/// 请求 PTY 并启动远程 shell；调用后 Session 进入 non-blocking 模式。
pub fn open_pty(session: &ssh2::Session, cols: u32, rows: u32) -> Result<ssh2::Channel> {
    let mut channel = session.channel_session().context("创建 SSH channel 失败")?;
    channel
        .request_pty(
            "xterm-256color",
            None,
            Some((cols.max(1), rows.max(1), 0, 0)),
        )
        .context("请求 PTY 失败")?;
    channel.shell().context("启动远程 shell 失败")?;
    session.set_blocking(false);
    Ok(channel)
}

/// 在后台线程运行 PTY IO 循环（阻塞直到连接断开）。
pub fn run_io(
    mut channel: ssh2::Channel,
    session: ssh2::Session,
    keepalive_secs: u32,
    commands: Receiver<PtyCommand>,
    mut output: Sender<PtyEvent>,
    waker: IoWaker,
) {
    waker.install();
    let mut keepalive = KeepAliveTimer::new(keepalive_secs);
    let mut pending_write: Vec<u8> = Vec::new();
    let mut pending_resize: Option<(u32, u32)> = None;
    let mut buf = [0u8; 8192];
    // 最近一次有输入 / 输出 / 命令活动的时间，用于空闲自适应退避
    let mut last_activity = Instant::now();

    loop {
        // 1. 处理 UI 命令
        loop {
            match commands.try_recv() {
                Ok(PtyCommand::Write(bytes)) => {
                    pending_write.extend_from_slice(&bytes);
                    last_activity = Instant::now();
                }
                Ok(PtyCommand::Resize { cols, rows }) => {
                    pending_resize = Some((cols, rows));
                    last_activity = Instant::now();
                }
                Ok(PtyCommand::Close) => {
                    let _ = channel.send_eof();
                    let _ = channel.close();
                    let _ = send_event(
                        &mut output,
                        PtyEvent::Closed {
                            reason: "会话已关闭".into(),
                        },
                    );
                    return;
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    let _ = channel.send_eof();
                    let _ = channel.close();
                    return;
                }
            }
        }

        // 2. 处理 resize
        if let Some((cols, rows)) = pending_resize {
            match channel.request_pty_size(cols, rows, None, None) {
                Ok(()) => {
                    pending_resize = None;
                    last_activity = Instant::now();
                }
                Err(e) if is_ssh_eagain(&e) => {
                    // 远端暂不可写：保留待重试的 resize，保持快速轮询
                    last_activity = Instant::now();
                }
                Err(e) => {
                    log::warn!("[pty] resize 失败: {e}");
                    pending_resize = None;
                }
            }
        }

        // 3. 写键盘输入
        if !pending_write.is_empty() {
            match channel.write(&pending_write) {
                Ok(0) => {}
                Ok(n) => {
                    pending_write.drain(..n);
                    let _ = channel.flush();
                    last_activity = Instant::now();
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    // 远端写缓冲满：仍有待发送输入，保持快速轮询
                    last_activity = Instant::now();
                }
                Err(e) => {
                    let _ = send_event(
                        &mut output,
                        PtyEvent::Closed {
                            reason: format!("写入失败: {e}"),
                        },
                    );
                    return;
                }
            }
        }

        // 4. KeepAlive
        if keepalive.due() {
            match session.keepalive_send() {
                Ok(_) => {}
                Err(e) if is_ssh_eagain(&e) => {}
                Err(e) => log::warn!("[pty] keepalive 失败: {e}"),
            }
            keepalive.mark_sent();
        }

        // 5. 读 PTY 输出
        match channel.read(&mut buf) {
            Ok(0) => {
                if channel.eof() {
                    let _ = send_event(
                        &mut output,
                        PtyEvent::Closed {
                            reason: "远程 shell 已退出".into(),
                        },
                    );
                    return;
                }
            }
            Ok(n) => {
                // 队列满时阻塞等待 UI 消费，保证输出不丢块
                if !send_event(&mut output, PtyEvent::Data(buf[..n].to_vec())) {
                    let _ = channel.send_eof();
                    let _ = channel.close();
                    return;
                }
                last_activity = Instant::now();
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(e) => {
                let _ = send_event(
                    &mut output,
                    PtyEvent::Closed {
                        reason: format!("读取失败: {e}"),
                    },
                );
                return;
            }
        }

        // 6. 可中断等待（9.19）：活跃 8ms 退避到空闲 50ms；
        //    UI 发命令后 unpark 会立即打断等待，首键回显不再受轮询间隔影响。
        std::thread::park_timeout(idle_poll_interval(last_activity.elapsed()));
    }
}

fn is_ssh_eagain(error: &ssh2::Error) -> bool {
    matches!(error.code(), ssh2::ErrorCode::Session(-37))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    #[test]
    fn wake_interrupts_parked_thread() {
        let waker = IoWaker::new();
        // 未 install 时 wake 是无操作，不应 panic
        waker.wake();

        let (ready_tx, ready_rx) = mpsc::channel::<()>();
        let cloned = waker.clone();
        let handle = std::thread::spawn(move || {
            cloned.install();
            ready_tx.send(()).expect("发送就绪信号");
            let started = Instant::now();
            // 模拟空闲等待：正常情况下会睡满 5s
            std::thread::park_timeout(Duration::from_secs(5));
            started.elapsed()
        });
        ready_rx.recv().expect("等待 IO 线程就绪");
        waker.wake();
        let waited = handle.join().expect("IO 线程不应 panic");
        assert!(
            waited < Duration::from_secs(1),
            "unpark 应立即结束空闲等待，实际 {waited:?}"
        );
    }

    #[test]
    fn idle_interval_backs_off() {
        assert_eq!(
            idle_poll_interval(Duration::from_millis(0)),
            Duration::from_millis(8)
        );
        assert_eq!(
            idle_poll_interval(Duration::from_millis(1500)),
            Duration::from_millis(16)
        );
        assert_eq!(
            idle_poll_interval(Duration::from_millis(3000)),
            Duration::from_millis(32)
        );
        assert_eq!(
            idle_poll_interval(Duration::from_secs(60)),
            Duration::from_millis(50)
        );
    }
}

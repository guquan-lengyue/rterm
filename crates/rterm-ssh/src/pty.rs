//! PTY — 打开远程 shell + 后台读写线程
//!
//! ssh2 的 blocking read 会锁住整个 Session，导致输入写不出去，
//! 所以 shell 打开后切换 non-blocking，由单线程轮询：
//! - 读 PTY 输出 → `PtyEvent::Data`
//! - 从命令通道取键盘输入 / resize
//! - 定期 `keepalive_send`

use std::io::{Read, Write};
use std::sync::mpsc::{Receiver, TryRecvError};
use std::time::Duration;

use anyhow::{Context, Result};
use futures::channel::mpsc::UnboundedSender;

use crate::keepalive::KeepAliveTimer;
use crate::known_hosts::HostKeyInfo;

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
    HostKey(HostKeyInfo),
    Connected,
    Data(Vec<u8>),
    Closed { reason: String },
    /// `retryable == false` 时（认证失败 / 主机密钥变更 / 配置错误）不应自动重连。
    Failed { reason: String, retryable: bool },
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
    output: UnboundedSender<PtyEvent>,
) {
    let mut keepalive = KeepAliveTimer::new(keepalive_secs);
    let mut pending_write: Vec<u8> = Vec::new();
    let mut pending_resize: Option<(u32, u32)> = None;
    let mut buf = [0u8; 8192];

    loop {
        // 1. 处理 UI 命令
        loop {
            match commands.try_recv() {
                Ok(PtyCommand::Write(bytes)) => pending_write.extend_from_slice(&bytes),
                Ok(PtyCommand::Resize { cols, rows }) => pending_resize = Some((cols, rows)),
                Ok(PtyCommand::Close) => {
                    let _ = channel.send_eof();
                    let _ = channel.close();
                    let _ = output.unbounded_send(PtyEvent::Closed {
                        reason: "会话已关闭".into(),
                    });
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
                Ok(()) => pending_resize = None,
                Err(e) if is_ssh_eagain(&e) => {}
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
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(e) => {
                    let _ = output.unbounded_send(PtyEvent::Closed {
                        reason: format!("写入失败: {e}"),
                    });
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
                    let _ = output.unbounded_send(PtyEvent::Closed {
                        reason: "远程 shell 已退出".into(),
                    });
                    return;
                }
            }
            Ok(n) => {
                if output
                    .unbounded_send(PtyEvent::Data(buf[..n].to_vec()))
                    .is_err()
                {
                    let _ = channel.send_eof();
                    let _ = channel.close();
                    return;
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(e) => {
                let _ = output.unbounded_send(PtyEvent::Closed {
                    reason: format!("读取失败: {e}"),
                });
                return;
            }
        }

        std::thread::sleep(Duration::from_millis(8));
    }
}

fn is_ssh_eagain(error: &ssh2::Error) -> bool {
    matches!(error.code(), ssh2::ErrorCode::Session(-37))
}

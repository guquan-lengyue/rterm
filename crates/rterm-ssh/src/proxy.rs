//! proxy — 跳板链路所需的本地中继
//!
//! `ssh2` 只允许用真实 socket（Windows 下 `AsRawSocket`）作为 `set_tcp_stream`，
//! 无法直接把上一跳的 `Channel` 交给下一跳的 `Session`。因此这里采用
//! 「本地回环中继」方案：
//!
//! ```text
//! 下一跳 Session ──TcpStream──▶ 127.0.0.1:随机端口
//!                                   │ (Relay 线程双向泵)
//!                                   ▼
//!                        上一跳 Session.channel_direct_tcpip(next_host, next_port)
//! ```
//!
//! 中继线程只占用上一跳的 Channel，不与该跳的其他 libssh2 调用并发，
//! 生命周期由 `SshSession` 持有的 `Relay` 句柄决定（断开时随 Session 关闭而退出）。

use std::io;
use std::net::{TcpListener, TcpStream};

use anyhow::{Context, Result};

/// 一条本地回环中继。Drop 后线程会随通道关闭自然退出。
pub(crate) struct Relay {
    local_port: u16,
    _thread: std::thread::JoinHandle<()>,
}

impl Relay {
    /// 在 `parent` 上打开到 `target_host:target_port` 的直连通道，
    /// 并启动本地监听，返回可用于下一跳 `TcpStream::connect` 的端口。
    pub fn start(parent: &ssh2::Session, target_host: &str, target_port: u16) -> Result<Self> {
        let channel = parent
            .channel_direct_tcpip(target_host, target_port, None)
            .with_context(|| format!("经跳板打开到 {target_host}:{target_port} 的直连通道失败"))?;

        let listener = TcpListener::bind(("127.0.0.1", 0)).context("创建本地中继监听失败")?;
        let local_port = listener
            .local_addr()
            .context("读取中继监听地址失败")?
            .port();

        let thread = std::thread::Builder::new()
            .name(format!("rterm-ssh-relay-{local_port}"))
            .spawn(move || run_relay(channel, listener))
            .context("启动 SSH 中继线程失败")?;

        log::debug!("[ssh] relay listening on 127.0.0.1:{local_port}");
        Ok(Self {
            local_port,
            _thread: thread,
        })
    }

    pub fn local_port(&self) -> u16 {
        self.local_port
    }
}

fn run_relay(channel: ssh2::Channel, listener: TcpListener) {
    let stream = match listener.accept() {
        Ok((stream, _)) => stream,
        Err(error) => {
            log::warn!("[ssh] relay accept 失败: {error}");
            return;
        }
    };
    let _ = stream.set_nodelay(true);
    pump(channel, stream);
}

/// 双向字节泵：任一方向关闭时一并结束，避免线程泄漏。
fn pump(channel: ssh2::Channel, stream: TcpStream) {
    let mut download_stream = match stream.try_clone() {
        Ok(stream) => stream,
        Err(error) => {
            log::warn!("[ssh] relay 复制 socket 失败: {error}");
            return;
        }
    };
    let mut upload_stream = stream;
    let mut upload_channel = channel.clone();
    let mut download_channel = channel;

    let upload = std::thread::Builder::new()
        .name("rterm-ssh-relay-up".into())
        .spawn(move || {
            if let Err(error) = io::copy(&mut upload_stream, &mut upload_channel) {
                log::debug!("[ssh] relay 上行结束: {error}");
            }
            let _ = upload_channel.send_eof();
        });

    if let Err(error) = io::copy(&mut download_channel, &mut download_stream) {
        log::debug!("[ssh] relay 下行结束: {error}");
    }
    let _ = download_stream.shutdown(std::net::Shutdown::Both);
    if let Ok(handle) = upload {
        let _ = handle.join();
    }
}

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
//! 关键约束：libssh2 的阻塞式读会长期持有 session 锁，双线程同时读写同一
//! Channel 会让另一方向饥饿（握手超时）。因此 Relay 在建立通道后把上一跳
//! Session 切到非阻塞模式，由**单线程轮询**双向搬运字节，任何时刻只有一个
//! libssh2 调用在途。

use std::io::{ErrorKind, Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::time::Duration;

use anyhow::{Context, Result};

const BUFFER_SIZE: usize = 16 * 1024;
const MAX_PENDING: usize = 256 * 1024;
const POLL_INTERVAL: Duration = Duration::from_millis(2);

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

        // 通道建立后再切非阻塞：之后的字节搬运全部由单线程轮询完成，
        // 避免阻塞读长期占用 session 锁导致反方向握手数据饿死。
        parent.set_blocking(false);

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
    let _ = stream.set_nonblocking(true);
    pump(channel, stream);
}

/// 单线程双向泵：非阻塞轮询 TCP 与 SSH Channel，任一方向 EOF 时结束。
fn pump(channel: ssh2::Channel, stream: TcpStream) {
    let mut channel = channel;
    let mut tcp = stream;
    let mut to_channel: Vec<u8> = Vec::new();
    let mut to_tcp: Vec<u8> = Vec::new();
    let mut tcp_open = true;
    let mut channel_open = true;
    let mut buffer = vec![0u8; BUFFER_SIZE];

    while (tcp_open || !to_channel.is_empty()) && (channel_open || !to_tcp.is_empty()) {
        let mut progressed = false;

        // TCP → 待发送缓冲
        if tcp_open && to_channel.len() < MAX_PENDING {
            match tcp.read(&mut buffer) {
                Ok(0) => {
                    tcp_open = false;
                    let _ = channel.send_eof();
                }
                Ok(read) => {
                    to_channel.extend_from_slice(&buffer[..read]);
                    progressed = true;
                }
                Err(error) if error.kind() == ErrorKind::WouldBlock => {}
                Err(_) => {
                    tcp_open = false;
                    let _ = channel.send_eof();
                }
            }
        }

        // 待发送缓冲 → SSH Channel
        if !to_channel.is_empty() {
            match channel.write(&to_channel) {
                Ok(written) => {
                    to_channel.drain(..written);
                    progressed = true;
                }
                Err(error) if error.kind() == ErrorKind::WouldBlock => {}
                Err(_) => {
                    channel_open = false;
                    to_channel.clear();
                }
            }
        }

        // SSH Channel → 待发送缓冲
        if channel_open {
            match channel.read(&mut buffer) {
                Ok(0) => channel_open = false,
                Ok(read) => {
                    to_tcp.extend_from_slice(&buffer[..read]);
                    progressed = true;
                }
                Err(error) if error.kind() == ErrorKind::WouldBlock => {}
                Err(_) => channel_open = false,
            }
        }

        // 待发送缓冲 → TCP
        if !to_tcp.is_empty() {
            match tcp.write(&to_tcp) {
                Ok(written) => {
                    to_tcp.drain(..written);
                    progressed = true;
                }
                Err(error) if error.kind() == ErrorKind::WouldBlock => {}
                Err(_) => {
                    channel_open = false;
                    to_tcp.clear();
                }
            }
        }

        if !progressed {
            std::thread::sleep(POLL_INTERVAL);
        }
    }

    let _ = tcp.shutdown(Shutdown::Both);
    let _ = channel.close();
}

//! rterm-forward — SSH 端口转发运行时
//!
//! 设计要点：
//! - 每条启用规则使用独立 SSH 连接（不与终端 / SFTP / 监控共享 Session，
//!   规避 libssh2 并发限制与相互阻塞）
//! - 规则线程持有连接与监听器，状态通过 channel 回 UI
//! - 停止时先让监听/连接退出，再断开 SSH，确保端口释放
//!
//! v1 支持本地 `-L`、远程 `-R` 与动态 `-D`（最小 SOCKS5，仅 CONNECT）。

use std::io::{Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{bail, Context, Result};
use futures::channel::mpsc::UnboundedSender;
use uuid::Uuid;

use rterm_session::{ForwardKind, PortForward, Session};
use rterm_ssh::SshSession;

// ---------------------------------------------------------------------------
// 事件 / 句柄
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ForwardStatus {
    Starting,
    Running,
    Error(String),
    Stopped,
}

#[derive(Clone, Debug)]
pub struct ForwardEvent {
    pub rule_id: Uuid,
    pub status: ForwardStatus,
}

/// 规则线程句柄。Drop 等价于 `stop()`，避免规则泄漏。
pub struct ForwardHandle {
    rule_id: Uuid,
    stop: Arc<AtomicBool>,
    session: Arc<Mutex<Option<ssh2::Session>>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl ForwardHandle {
    pub fn rule_id(&self) -> Uuid {
        self.rule_id
    }

    /// 停止规则：置停止位 → 断开 SSH（唤醒阻塞的 accept）→ 等待线程退出。
    pub fn stop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Ok(mut guard) = self.session.lock() {
            if let Some(session) = guard.take() {
                let _ = session.disconnect(None, "forward stopped", None);
            }
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Drop for ForwardHandle {
    fn drop(&mut self) {
        self.stop();
    }
}

// ---------------------------------------------------------------------------
// 启动
// ---------------------------------------------------------------------------

/// 启动一条转发规则（后台线程）。返回的句柄负责停止与清理。
pub fn start_forward(
    config: Session,
    rule: PortForward,
    events: UnboundedSender<ForwardEvent>,
) -> Result<ForwardHandle> {
    if let Err(error) = rule.validate() {
        bail!("转发规则无效: {error}");
    }

    let rule_id = rule.id;
    let stop = Arc::new(AtomicBool::new(false));
    let session_slot: Arc<Mutex<Option<ssh2::Session>>> = Arc::new(Mutex::new(None));

    let stop_for_thread = stop.clone();
    let session_for_thread = session_slot.clone();
    let events_for_thread = events.clone();
    let rule_for_thread = rule.clone();

    let thread = std::thread::Builder::new()
        .name(format!("rterm-forward-{rule_id}"))
        .spawn(move || {
            run_worker(
                config,
                rule_for_thread,
                stop_for_thread,
                session_for_thread,
                events_for_thread,
            );
        })
        .context("启动端口转发线程失败")?;

    Ok(ForwardHandle {
        rule_id,
        stop,
        session: session_slot,
        thread: Some(thread),
    })
}

fn run_worker(
    config: Session,
    rule: PortForward,
    stop: Arc<AtomicBool>,
    session_slot: Arc<Mutex<Option<ssh2::Session>>>,
    events: UnboundedSender<ForwardEvent>,
) {
    let rule_id = rule.id;

    let mut ssh = SshSession::new();
    if let Err(error) = ssh.connect(&config) {
        send(
            &events,
            rule_id,
            ForwardStatus::Error(format!(
                "连接 {} 失败: {}",
                config.display_label(),
                rterm_ssh::errors::describe(&error)
            )),
        );
        return;
    }
    let Some(session) = ssh.session_clone() else {
        send(&events, rule_id, ForwardStatus::Error("SSH 未连接".into()));
        return;
    };
    if let Ok(mut guard) = session_slot.lock() {
        *guard = Some(session.clone());
    }

    let result = match rule.kind {
        ForwardKind::Local => run_local(session.clone(), &rule, &stop, &events),
        ForwardKind::Remote => run_remote(session.clone(), &rule, &stop, &events),
        ForwardKind::Dynamic => run_dynamic(session.clone(), &rule, &stop, &events),
    };

    ssh.disconnect();
    if let Ok(mut guard) = session_slot.lock() {
        guard.take();
    }

    match result {
        Ok(()) => send(&events, rule_id, ForwardStatus::Stopped),
        Err(error) => send(&events, rule_id, ForwardStatus::Error(error)),
    }
}

fn send(events: &UnboundedSender<ForwardEvent>, rule_id: Uuid, status: ForwardStatus) {
    let _ = events.unbounded_send(ForwardEvent { rule_id, status });
}

// ---------------------------------------------------------------------------
// 本地 / 动态转发（本地监听）
// ---------------------------------------------------------------------------

fn run_local(
    session: ssh2::Session,
    rule: &PortForward,
    stop: &AtomicBool,
    events: &UnboundedSender<ForwardEvent>,
) -> Result<(), String> {
    let listener = bind_listener(rule)?;
    send(events, rule.id, ForwardStatus::Running);
    log::info!("[forward] local {} 已启动", rule.summary());

    let target_host = rule.target_host.clone();
    let target_port = rule.target_port;
    let rule_id = rule.id;
    let mut workers: Vec<std::thread::JoinHandle<()>> = Vec::new();

    while !stop.load(Ordering::SeqCst) {
        match listener.accept() {
            Ok((stream, _)) => {
                // Windows 上非阻塞 listener accept 出的 socket 可能继承非阻塞，
                // 显式恢复阻塞模式，保证 SOCKS5 握手与字节泵可用。
                let _ = stream.set_nonblocking(false);
                let session = session.clone();
                let target_host = target_host.clone();
                workers.push(std::thread::spawn(move || {
                    match session.channel_direct_tcpip(&target_host, target_port, None) {
                        Ok(channel) => pump(channel, stream),
                        Err(error) => log::warn!(
                            "[forward] {rule_id} 打开 {}:{} 通道失败: {error}",
                            target_host,
                            target_port
                        ),
                    }
                }));
                workers.retain(|worker| !worker.is_finished());
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(100));
            }
            Err(error) => return Err(format!("本地监听接受连接失败: {error}")),
        }
    }

    drop(listener);
    log::info!("[forward] local {} 已停止（端口释放）", rule.summary());
    Ok(())
}

// ---------------------------------------------------------------------------
// 远程转发（远端监听）
// ---------------------------------------------------------------------------

fn run_remote(
    session: ssh2::Session,
    rule: &PortForward,
    stop: &AtomicBool,
    events: &UnboundedSender<ForwardEvent>,
) -> Result<(), String> {
    // accept 是阻塞调用：设置超时后循环检查停止位。
    session.set_timeout(500);
    let (mut listener, bound_port) = session
        .channel_forward_listen(rule.bind_port, Some(&rule.bind_host), None)
        .map_err(|error| {
            format!(
                "远端监听 {}:{} 失败（服务端可能禁止 AllowTcpForwarding）: {error}",
                rule.bind_host, rule.bind_port
            )
        })?;
    send(events, rule.id, ForwardStatus::Running);
    log::info!(
        "[forward] remote {}:{} 已启动（绑定端口 {bound_port}）",
        rule.bind_host,
        rule.bind_port
    );

    let target_host = rule.target_host.clone();
    let target_port = rule.target_port;
    let rule_id = rule.id;

    while !stop.load(Ordering::SeqCst) {
        match listener.accept() {
            Ok(channel) => {
                let target_host = target_host.clone();
                std::thread::spawn(move || {
                    match TcpStream::connect((target_host.as_str(), target_port)) {
                        Ok(stream) => pump(channel, stream),
                        Err(error) => log::warn!(
                        "[forward] {rule_id} 连接本地目标 {target_host}:{target_port} 失败: {error}"
                    ),
                    }
                });
            }
            Err(error) => {
                // 超时是正常的轮询节拍；连接断开/停止时退出。
                let io_error = std::io::Error::from(error);
                if io_error.kind() == std::io::ErrorKind::TimedOut {
                    continue;
                }
                if stop.load(Ordering::SeqCst) {
                    break;
                }
                return Err(format!("远端转发监听中断: {io_error}"));
            }
        }
    }

    drop(listener);
    log::info!("[forward] remote {} 已停止", rule.summary());
    Ok(())
}

// ---------------------------------------------------------------------------
// 动态转发（最小 SOCKS5：无认证 + CONNECT）
// ---------------------------------------------------------------------------

fn run_dynamic(
    session: ssh2::Session,
    rule: &PortForward,
    stop: &AtomicBool,
    events: &UnboundedSender<ForwardEvent>,
) -> Result<(), String> {
    let listener = bind_listener(rule)?;
    send(events, rule.id, ForwardStatus::Running);
    log::info!("[forward] dynamic {} 已启动", rule.summary());
    let rule_id = rule.id;

    while !stop.load(Ordering::SeqCst) {
        match listener.accept() {
            Ok((mut stream, _)) => {
                let _ = stream.set_nonblocking(false);
                let session = session.clone();
                std::thread::spawn(move || {
                    if let Err(error) = socks5_connect(&session, &mut stream) {
                        log::debug!("[forward] {rule_id} SOCKS5 连接失败: {error:#}");
                    }
                });
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(100));
            }
            Err(error) => return Err(format!("动态转发监听失败: {error}")),
        }
    }

    drop(listener);
    Ok(())
}

fn socks5_connect(session: &ssh2::Session, stream: &mut TcpStream) -> Result<()> {
    // 1. 握手：VER=5, NMETHODS, METHODS（接受 0x00 无认证）
    let mut header = [0u8; 2];
    stream.read_exact(&mut header).context("读取 SOCKS5 握手")?;
    if header[0] != 5 {
        bail!("不支持的 SOCKS 版本: {}", header[0]);
    }
    let mut methods = vec![0u8; header[1] as usize];
    stream
        .read_exact(&mut methods)
        .context("读取 SOCKS5 方法")?;
    if !methods.contains(&0) {
        stream.write_all(&[5, 0xFF]).ok();
        bail!("客户端不支持无认证方式");
    }
    stream.write_all(&[5, 0]).context("回复 SOCKS5 握手")?;

    // 2. 请求：VER=5, CMD=1, RSV, ATYP, DST.ADDR, DST.PORT
    let mut request = [0u8; 4];
    stream
        .read_exact(&mut request)
        .context("读取 SOCKS5 请求")?;
    if request[0] != 5 || request[1] != 1 {
        stream.write_all(&[5, 7, 0, 1, 0, 0, 0, 0, 0, 0]).ok();
        bail!("仅支持 SOCKS5 CONNECT");
    }
    let host = match request[3] {
        1 => {
            let mut addr = [0u8; 4];
            stream.read_exact(&mut addr).context("读取 IPv4 地址")?;
            std::net::Ipv4Addr::from(addr).to_string()
        }
        3 => {
            let mut len = [0u8; 1];
            stream.read_exact(&mut len).context("读取域名长度")?;
            let mut name = vec![0u8; len[0] as usize];
            stream.read_exact(&mut name).context("读取域名")?;
            String::from_utf8(name).context("域名不是 UTF-8")?
        }
        4 => {
            let mut addr = [0u8; 16];
            stream.read_exact(&mut addr).context("读取 IPv6 地址")?;
            std::net::Ipv6Addr::from(addr).to_string()
        }
        other => bail!("不支持的地址类型: {other}"),
    };
    let mut port_bytes = [0u8; 2];
    stream.read_exact(&mut port_bytes).context("读取目标端口")?;
    let port = u16::from_be_bytes(port_bytes);

    let channel = session
        .channel_direct_tcpip(&host, port, None)
        .with_context(|| format!("建立到 {host}:{port} 的通道失败"))?;
    // 3. 成功回复
    stream
        .write_all(&[5, 0, 0, 1, 0, 0, 0, 0, 0, 0])
        .context("回复 SOCKS5 成功")?;
    pump(channel, stream.try_clone().context("复制 SOCKS5 连接")?);
    Ok(())
}

// ---------------------------------------------------------------------------
// 公共辅助
// ---------------------------------------------------------------------------

fn bind_listener(rule: &PortForward) -> Result<TcpListener, String> {
    let addr = format!("{}:{}", rule.bind_host.trim(), rule.bind_port);
    let listener =
        TcpListener::bind(&addr).map_err(|error| format!("监听 {addr} 失败: {error}"))?;
    listener
        .set_nonblocking(true)
        .map_err(|error| format!("设置监听非阻塞失败: {error}"))?;
    Ok(listener)
}

/// 双向字节泵：任一方向关闭时结束两侧，避免线程泄漏。
fn pump(channel: ssh2::Channel, stream: TcpStream) {
    let _ = stream.set_nodelay(true);
    let mut download_stream = match stream.try_clone() {
        Ok(stream) => stream,
        Err(error) => {
            log::warn!("[forward] 复制 socket 失败: {error}");
            return;
        }
    };
    let mut upload_stream = stream;
    let mut upload_channel = channel.clone();
    let mut download_channel = channel;

    let upload = std::thread::Builder::new()
        .name("rterm-forward-up".into())
        .spawn(move || {
            let _ = std::io::copy(&mut upload_stream, &mut upload_channel);
            let _ = upload_channel.send_eof();
        });

    let _ = std::io::copy(&mut download_channel, &mut download_stream);
    let _ = download_stream.shutdown(Shutdown::Both);
    if let Ok(handle) = upload {
        let _ = handle.join();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rterm_session::{ForwardKind, PortForward};

    fn rule(kind: ForwardKind) -> PortForward {
        PortForward {
            kind,
            bind_host: "127.0.0.1".into(),
            bind_port: 13306,
            target_host: "127.0.0.1".into(),
            target_port: 3306,
            ..Default::default()
        }
    }

    #[test]
    fn rejects_invalid_rule_before_spawn() {
        let (tx, _rx) = futures::channel::mpsc::unbounded();
        let mut invalid = rule(ForwardKind::Local);
        invalid.bind_port = 0;
        assert!(start_forward(Session::default(), invalid, tx).is_err());
    }

    #[test]
    fn local_listener_releases_port_on_stop() {
        // 直连一个不存在的 SSH 服务：启动会失败并发送 Error，不占用端口。
        let (tx, _rx) = futures::channel::mpsc::unbounded();
        // 先占用再释放，拿到一个当前空闲的端口，避免与真实服务冲突。
        let free_port = {
            let probe = TcpListener::bind(("127.0.0.1", 0)).expect("probe bind");
            probe.local_addr().expect("probe addr").port()
        };
        let mut config = Session::new("unreachable", "127.0.0.1", "ops");
        config.port = 1;
        config.connect_timeout_secs = 1;
        let mut local_rule = rule(ForwardKind::Local);
        local_rule.bind_port = free_port;
        let handle = start_forward(config, local_rule, tx).expect("spawn");
        drop(handle);
        // 端口应可再次绑定
        let listener = TcpListener::bind(("127.0.0.1", free_port));
        assert!(listener.is_ok(), "规则停止后端口必须释放");
    }
}

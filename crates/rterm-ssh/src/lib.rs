//! rterm-ssh — ssh2 薄封装
//!
//! Sprint 5 范围：
//! - TCP + handshake + 三种认证
//! - known_hosts 校验（TOFU）+ 变更检测
//! - PTY 打开 + 非阻塞读写线程
//! - KeepAlive 计时器 + 指数退避重连策略
//!
//! 所有阻塞调用都必须由调用方放到后台线程执行。

pub mod auth;
pub mod errors;
pub mod keepalive;
pub mod known_hosts;
mod proxy;
pub mod pty;
pub mod reconnect;

use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::time::Duration;

use anyhow::{bail, Context, Result};

pub use known_hosts::{HostKeyChoice, HostKeyDecision, HostKeyInfo, HostKeyPolicy};

/// 主机密钥确认回调：未知 / 变更时由 UI 三选一后回传结果。
pub type HostKeyVerifier = Box<dyn Fn(&HostKeyInfo) -> HostKeyChoice + Send>;

/// 连接状态机。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConnectionState {
    Disconnected,
    Connecting,
    Connected,
    Reconnecting(u8),
}

pub struct SshSession {
    inner: Option<ssh2::Session>,
    state: ConnectionState,
    last_host_key: Option<HostKeyInfo>,
    /// 跳板链上的中间 Session（最外层在前），必须在目标会话存续期间保持连接。
    jump_sessions: Vec<ssh2::Session>,
    /// 每两跳之间的本地回环中继；Drop 时线程随通道关闭退出。
    relays: Vec<proxy::Relay>,
}

impl Default for SshSession {
    fn default() -> Self {
        Self::new()
    }
}

impl SshSession {
    pub fn new() -> Self {
        Self {
            inner: None,
            state: ConnectionState::Disconnected,
            last_host_key: None,
            jump_sessions: Vec::new(),
            relays: Vec::new(),
        }
    }

    pub fn state(&self) -> ConnectionState {
        self.state
    }

    pub fn is_connected(&self) -> bool {
        self.state == ConnectionState::Connected && self.inner.is_some()
    }

    pub fn last_host_key(&self) -> Option<&HostKeyInfo> {
        self.last_host_key.as_ref()
    }

    /// 连接 + 主机密钥校验 + 认证（未知 / 变更时按 TOFU 处理）。阻塞调用。
    pub fn connect(&mut self, config: &rterm_session::Session) -> Result<()> {
        self.connect_with_verifier(config, None)
    }

    /// 连接 + 主机密钥校验 + 认证；`verifier` 用于未知 / 变更时三选一。
    ///
    /// 配置了 `proxy_jump` 时按 A→B→目标 逐跳建立连接：
    /// 每一跳都使用自身的超时、known_hosts 与认证配置。
    pub fn connect_with_verifier(
        &mut self,
        config: &rterm_session::Session,
        verifier: Option<HostKeyVerifier>,
    ) -> Result<()> {
        self.state = ConnectionState::Connecting;
        self.last_host_key = None;
        self.inner = None;
        self.jump_sessions.clear();
        self.relays.clear();

        // 去掉 Send 约束后再传给 connect_inner（校验发生在同一线程内）。
        let borrowed_verifier: Option<&dyn Fn(&HostKeyInfo) -> HostKeyChoice> = match &verifier {
            Some(verifier) => {
                let verifier: &dyn Fn(&HostKeyInfo) -> HostKeyChoice = verifier.as_ref();
                Some(verifier)
            }
            None => None,
        };
        match self.connect_inner(config, borrowed_verifier) {
            Ok(()) => {
                self.state = ConnectionState::Connected;
                Ok(())
            }
            Err(error) => {
                self.inner = None;
                self.jump_sessions.clear();
                self.relays.clear();
                self.state = ConnectionState::Disconnected;
                Err(error)
            }
        }
    }

    fn connect_inner(
        &mut self,
        config: &rterm_session::Session,
        verifier: Option<&dyn Fn(&HostKeyInfo) -> HostKeyChoice>,
    ) -> Result<()> {
        if config.host.trim().is_empty() {
            bail!("主机地址为空");
        }

        // 纯函数校验：环路 / 超过 5 跳在连接前直接拒绝。
        let chain = rterm_session::proxy::build_chain(config)?;
        let mut jump_sessions: Vec<ssh2::Session> = Vec::new();
        let mut relays: Vec<proxy::Relay> = Vec::new();
        let mut previous: Option<ssh2::Session> = None;

        for (index, hop) in chain.iter().enumerate() {
            let is_target = index + 1 == chain.len();
            let hop_result = connect_hop(hop, previous.as_ref(), &mut relays, verifier);

            match hop_result {
                Ok((session, info)) => {
                    if is_target {
                        self.inner = Some(session);
                        self.last_host_key = Some(info);
                    } else {
                        jump_sessions.push(session.clone());
                        previous = Some(session);
                    }
                }
                Err(error) => {
                    // 已建立的跳板连接逆序断开，避免连接半途失败后残留。
                    for session in jump_sessions.iter().rev() {
                        let _ = session.disconnect(None, "proxy chain aborted", None);
                    }
                    return Err(match chain.get(index.wrapping_sub(1)) {
                        Some(previous_hop) if index > 0 => error.context(format!(
                            "经跳板「{}」连接「{}」失败",
                            previous_hop.display_label(),
                            hop.display_label()
                        )),
                        _ => error,
                    });
                }
            }
        }

        self.jump_sessions = jump_sessions;
        self.relays = relays;
        log::info!(
            "[ssh] connected {}@{}:{} ({})",
            config.username,
            config.host,
            config.port,
            if config.proxy_jump.is_some() {
                format!("经 {} 跳跳板", self.jump_sessions.len())
            } else {
                "直连".to_string()
            }
        );
        Ok(())
    }

    /// 打开远程 shell。必须在 connect 成功后调用。
    pub fn open_pty(&mut self, cols: u32, rows: u32) -> Result<ssh2::Channel> {
        let session = self.inner.as_ref().context("SSH 尚未连接")?;
        pty::open_pty(session, cols, rows)
    }

    /// 调整远程 PTY 尺寸。
    pub fn resize_pty(channel: &mut ssh2::Channel, cols: u32, rows: u32) -> Result<()> {
        channel
            .request_pty_size(cols.max(1), rows.max(1), None, None)
            .context("调整 PTY 大小失败")
    }

    /// 复制底层 Session（PTY IO 线程用于 keepalive）。
    pub fn session_clone(&self) -> Option<ssh2::Session> {
        self.inner.clone()
    }

    /// 目标会话的原始句柄（端口转发需要 `channel_direct_tcpip` / `channel_forward_listen`）。
    pub fn raw_session(&self) -> Option<&ssh2::Session> {
        self.inner.as_ref()
    }

    pub fn disconnect(&mut self) {
        if let Some(session) = self.inner.take() {
            let _ = session.disconnect(None, "disconnected", None);
        }
        // 逆序断开跳板，确保目标断流后中间连接再释放。
        for session in self.jump_sessions.drain(..).rev() {
            let _ = session.disconnect(None, "disconnected", None);
        }
        self.relays.clear();
        self.state = ConnectionState::Disconnected;
        log::info!("[ssh] disconnected");
    }
}

/// 建立单跳：直连或经上一跳的本地中继；随后握手、校验主机密钥并认证。
fn connect_hop(
    hop: &rterm_session::Session,
    previous: Option<&ssh2::Session>,
    relays: &mut Vec<proxy::Relay>,
    verifier: Option<&dyn Fn(&HostKeyInfo) -> HostKeyChoice>,
) -> Result<(ssh2::Session, HostKeyInfo)> {
    let timeout_secs = hop.connect_timeout_secs.max(1) as u64;
    let timeout = Duration::from_secs(timeout_secs);

    let tcp = match previous {
        None => connect_tcp(&hop.host, hop.port, timeout)?,
        Some(parent) => {
            let relay = proxy::Relay::start(parent, &hop.host, hop.port)?;
            let port = relay.local_port();
            let stream = TcpStream::connect(("127.0.0.1", port))
                .with_context(|| format!("连接本地中继 127.0.0.1:{port} 失败"))?;
            relays.push(relay);
            stream
        }
    };

    handshake_hop(hop, tcp, timeout_secs, verifier)
}

/// 单跳握手 + known_hosts 校验 + 认证。
fn handshake_hop(
    hop: &rterm_session::Session,
    tcp: TcpStream,
    timeout_secs: u64,
    verifier: Option<&dyn Fn(&HostKeyInfo) -> HostKeyChoice>,
) -> Result<(ssh2::Session, HostKeyInfo)> {
    tcp.set_nodelay(true).ok();

    let mut session = ssh2::Session::new().context("创建 SSH Session 失败")?;
    session.set_tcp_stream(tcp);
    session.set_timeout((timeout_secs * 1000).min(u32::MAX as u64) as u32);
    // 部分 OpenSSH 9+ 服务端默认禁用 ssh-rsa；显式优先 ed25519
    // （libssh2 的 rsa-sha2 支持在不同构建里不一致）。
    let _ = session.method_pref(ssh2::MethodType::Kex, "curve25519-sha256");
    let _ = session.method_pref(ssh2::MethodType::HostKey, "ssh-ed25519");
    session.set_compress(hop.compress);
    session
        .handshake()
        .with_context(|| format!("SSH 握手失败: {}:{}", hop.host, hop.port))?;

    let info = known_hosts::verify_host_key(
        &session,
        &hop.host,
        hop.port,
        &known_hosts::default_path(),
        HostKeyPolicy::TrustOnFirstUse,
        verifier,
    )?;

    // Changed 且用户未确认时由 verify_host_key 原样返回，这里继续拒绝。
    if info.decision == HostKeyDecision::Changed {
        bail!(
            "主机密钥已变更（{}:{}，{} {}），已拒绝连接",
            hop.host,
            hop.port,
            info.key_type,
            info.fingerprint
        );
    }

    auth::authenticate(&session, hop)?;
    Ok((session, info))
}

fn connect_tcp(host: &str, port: u16, timeout: Duration) -> Result<TcpStream> {
    let addrs: Vec<SocketAddr> = (host, port)
        .to_socket_addrs()
        .with_context(|| format!("解析主机失败: {host}:{port}"))?
        .collect();
    if addrs.is_empty() {
        bail!("解析主机无结果: {host}:{port}");
    }

    let mut last_error = None;
    for addr in addrs {
        match TcpStream::connect_timeout(&addr, timeout) {
            Ok(stream) => return Ok(stream),
            Err(error) => last_error = Some(error),
        }
    }

    Err(last_error.unwrap()).with_context(|| format!("TCP 连接失败: {host}:{port}"))
}

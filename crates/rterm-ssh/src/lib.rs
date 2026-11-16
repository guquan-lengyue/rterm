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
pub mod keepalive;
pub mod known_hosts;
pub mod pty;
pub mod reconnect;

use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::time::Duration;

use anyhow::{bail, Context, Result};

pub use known_hosts::{HostKeyDecision, HostKeyInfo, HostKeyPolicy};

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

    /// 连接 + 主机密钥校验 + 认证。阻塞调用。
    pub fn connect(&mut self, config: &rterm_session::Session) -> Result<()> {
        self.state = ConnectionState::Connecting;
        self.last_host_key = None;

        match self.connect_inner(config) {
            Ok(()) => {
                self.state = ConnectionState::Connected;
                Ok(())
            }
            Err(error) => {
                self.inner = None;
                self.state = ConnectionState::Disconnected;
                Err(error)
            }
        }
    }

    fn connect_inner(&mut self, config: &rterm_session::Session) -> Result<()> {
        if config.host.trim().is_empty() {
            bail!("主机地址为空");
        }

        let timeout_secs = config.connect_timeout_secs.max(1) as u64;
        let timeout = Duration::from_secs(timeout_secs);
        let tcp = connect_tcp(&config.host, config.port, timeout)?;
        tcp.set_nodelay(true).ok();

        let mut session = ssh2::Session::new().context("创建 SSH Session 失败")?;
        session.set_tcp_stream(tcp);
        session.set_timeout((timeout_secs * 1000).min(u32::MAX as u64) as u32);
        // 部分 OpenSSH 9+ 服务端默认禁用 ssh-rsa；显式优先 ed25519
        // （libssh2 的 rsa-sha2 支持在不同构建里不一致）。
        let _ = session.method_pref(ssh2::MethodType::Kex, "curve25519-sha256");
        let _ = session.method_pref(ssh2::MethodType::HostKey, "ssh-ed25519");
        session.set_compress(config.compress);
        session
            .handshake()
            .with_context(|| format!("SSH 握手失败: {}:{}", config.host, config.port))?;

        let info = known_hosts::check_and_record(
            &session,
            &config.host,
            config.port,
            &known_hosts::default_path(),
            HostKeyPolicy::TrustOnFirstUse,
        )?;
        let decision = info.decision;
        self.last_host_key = Some(info.clone());

        if decision == HostKeyDecision::Changed {
            bail!(
                "主机密钥已变更（{}:{}，{} {}），已拒绝连接",
                config.host,
                config.port,
                info.key_type,
                info.fingerprint
            );
        }

        auth::authenticate(&session, config)?;
        self.inner = Some(session);
        log::info!(
            "[ssh] connected {}@{}:{}",
            config.username,
            config.host,
            config.port
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

    pub fn disconnect(&mut self) {
        if let Some(session) = self.inner.take() {
            let _ = session.disconnect(None, "disconnected", None);
        }
        self.state = ConnectionState::Disconnected;
        log::info!("[ssh] disconnected");
    }
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

    Err(last_error.unwrap())
        .with_context(|| format!("TCP 连接失败: {host}:{port}"))
}

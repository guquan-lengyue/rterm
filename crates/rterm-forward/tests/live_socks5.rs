//! SOCKS5 动态转发实机验证（默认忽略，需真实 SSH 主机）。
#![allow(linker_messages)]
//!
//! 运行：
//! ```powershell
//! $env:RTERM_TEST_HOST='192.168.1.103'
//! $env:RTERM_TEST_USER='k'
//! $env:RTERM_TEST_PASSWORD='...'
//! cargo test -p rterm-forward --test live_socks5 -- --ignored --nocapture
//! ```
//!
//! 建立 `-D` 规则后，通过 SOCKS5 CONNECT 访问远端 `127.0.0.1:22`，
//! 读取 SSH banner 即证明：本地 SOCKS5 → SSH 通道 → 远端回环 的整条链路可用。

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::time::{Duration, Instant};

use futures::channel::mpsc::unbounded;
use futures::StreamExt;

use rterm_forward::{start_forward, ForwardStatus};
use rterm_session::{AuthMethod, EncryptedString, ForwardKind, PortForward, Session};

fn test_session() -> Session {
    Session {
        name: "socks5-live".into(),
        host: std::env::var("RTERM_TEST_HOST").expect("需要 RTERM_TEST_HOST"),
        port: 22,
        username: std::env::var("RTERM_TEST_USER").expect("需要 RTERM_TEST_USER"),
        auth: AuthMethod::Password {
            password: EncryptedString::new(
                &std::env::var("RTERM_TEST_PASSWORD").expect("需要 RTERM_TEST_PASSWORD"),
            ),
        },
        connect_timeout_secs: 10,
        ..Session::default()
    }
}

fn free_port() -> u16 {
    let probe = TcpListener::bind(("127.0.0.1", 0)).expect("probe bind");
    probe.local_addr().expect("probe addr").port()
}

/// 等待规则进入 Running（跳过 Starting），失败时返回错误文本。
fn wait_running(
    rx: &mut futures::channel::mpsc::UnboundedReceiver<rterm_forward::ForwardEvent>,
) -> Result<(), String> {
    let deadline = Instant::now() + Duration::from_secs(20);
    while Instant::now() < deadline {
        let event = futures::executor::block_on(async { rx.next().await });
        let Some(event) = event else {
            return Err("事件通道已关闭".into());
        };
        match event.status {
            ForwardStatus::Starting => continue,
            ForwardStatus::Running => return Ok(()),
            ForwardStatus::Error(error) => return Err(error),
            ForwardStatus::Stopped => return Err("规则提前停止".into()),
        }
    }
    Err("等待规则启动超时".into())
}

#[test]
#[ignore = "需要真实 SSH 主机：设置 RTERM_TEST_HOST / RTERM_TEST_USER / RTERM_TEST_PASSWORD"]
fn socks5_forward_reaches_remote_loopback() {
    let port = free_port();
    let rule = PortForward {
        kind: ForwardKind::Dynamic,
        bind_host: "127.0.0.1".into(),
        bind_port: port,
        ..PortForward::default()
    };

    let (tx, mut rx) = unbounded();
    let mut handle = start_forward(test_session(), rule, tx).expect("启动动态转发");
    wait_running(&mut rx).expect("动态转发应进入运行中");

    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("连接 SOCKS5");
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .expect("设置读超时");

    // 握手：VER=5, NMETHODS=1, 无认证
    stream.write_all(&[5, 1, 0]).expect("发送握手");
    let mut greeting = [0u8; 2];
    stream.read_exact(&mut greeting).expect("读取握手响应");
    assert_eq!(greeting, [5, 0], "SOCKS5 应选择无认证方式");

    // CONNECT 127.0.0.1:22（IPv4）
    let mut request = vec![5, 1, 0, 1, 127, 0, 0, 1];
    request.extend_from_slice(&22u16.to_be_bytes());
    stream.write_all(&request).expect("发送 CONNECT");
    let mut reply = [0u8; 10];
    stream.read_exact(&mut reply).expect("读取 CONNECT 响应");
    assert_eq!(&reply[..2], &[5, 0], "SOCKS5 CONNECT 应成功: {reply:?}");

    // 远端 127.0.0.1:22 会主动发送 SSH banner
    let mut banner = [0u8; 96];
    let read = stream.read(&mut banner).expect("读取 SSH banner");
    let banner = String::from_utf8_lossy(&banner[..read]);
    assert!(
        banner.starts_with("SSH-2.0"),
        "应通过 SOCKS5 读到 SSH banner，实际: {banner:?}"
    );

    drop(stream);
    handle.stop();
    // 停止后监听端口应释放
    let rebound = TcpListener::bind(("127.0.0.1", port));
    assert!(rebound.is_ok(), "停止后端口必须释放");
}

//! 跳板链实机验证（默认忽略，需真实 SSH 主机）。
#![allow(linker_messages)]
//!
//! 运行：
//! ```powershell
//! $env:RTERM_TEST_HOST='192.168.1.103'
//! $env:RTERM_TEST_USER='k'
//! $env:RTERM_TEST_PASSWORD='...'
//! cargo test -p rterm-ssh --test live_proxy -- --ignored --nocapture
//! ```
//!
//! 使用「自跳板」：先直连跳板机，再经 `channel_direct_tcpip` 连到跳板机自身的
//! `127.0.0.1:22` 作为目标。目标侧看到的客户端地址为 127.0.0.1，证明第二跳
//! 确实由跳板机发起，而不是 Windows 本机直连。

use std::io::Read;

use rterm_session::{AuthMethod, EncryptedString, Session};
use rterm_ssh::SshSession;

fn test_session(name: &str, host: &str) -> Session {
    Session {
        name: name.into(),
        host: host.into(),
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

#[test]
#[ignore = "需要真实 SSH 主机：设置 RTERM_TEST_HOST / RTERM_TEST_USER / RTERM_TEST_PASSWORD"]
fn connects_through_single_jump_chain() {
    let host = std::env::var("RTERM_TEST_HOST").expect("需要 RTERM_TEST_HOST");

    let jump = test_session("jump", &host);
    let mut target = test_session("target", "127.0.0.1");
    target.proxy_jump = Some(Box::new(jump));

    let mut ssh = SshSession::new();
    ssh.connect(&target).expect("经单跳跳板连接目标失败");
    assert!(ssh.is_connected(), "跳板连接后状态应为 Connected");

    let raw = ssh.session_clone().expect("目标 Session");
    let mut channel = raw.channel_session().expect("打开 exec 通道");
    channel
        .exec("echo $SSH_CONNECTION")
        .expect("执行 SSH_CONNECTION");
    let mut output = String::new();
    channel.read_to_string(&mut output).expect("读取输出");

    // SSH_CONNECTION: <client_ip> <client_port> <server_ip> <server_port>
    let client_ip = output.split_whitespace().next().unwrap_or_default();
    assert_eq!(
        client_ip, "127.0.0.1",
        "目标连接的客户端应来自跳板机自身，实际输出: {output:?}"
    );
    ssh.disconnect();
}

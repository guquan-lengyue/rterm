//! 真实服务器 smoke test（默认 ignore）。
//!
//! 运行：
//! ```powershell
//! $env:RTERM_TEST_HOST="192.168.1.95"
//! $env:RTERM_TEST_USER="ubuntu"
//! $env:RTERM_TEST_PASSWORD="..."
//! cargo test -p rterm-ssh --test live_ssh -- --ignored --nocapture
//! ```

use std::io::{Read, Write};
use std::time::{Duration, Instant};

use rterm_session::{AuthMethod, EncryptedString, Session};
use rterm_ssh::SshSession;

#[test]
#[ignore]
fn connect_open_pty_and_run_pwd() {
    let host = std::env::var("RTERM_TEST_HOST").expect("RTERM_TEST_HOST");
    let user = std::env::var("RTERM_TEST_USER").expect("RTERM_TEST_USER");
    let password = std::env::var("RTERM_TEST_PASSWORD").expect("RTERM_TEST_PASSWORD");

    let known_hosts = std::env::temp_dir().join(format!(
        "rterm-known-hosts-{}.txt",
        std::process::id()
    ));
    let _ = std::fs::remove_file(&known_hosts);
    std::env::set_var("RTERM_KNOWN_HOSTS", &known_hosts);

    let mut config = Session::default();
    config.name = "live-smoke".into();
    config.host = host;
    config.port = 22;
    config.username = user;
    config.auth = AuthMethod::Password {
        password: EncryptedString::new(&password),
    };
    config.connect_timeout_secs = 10;

    let mut ssh = SshSession::new();
    ssh.connect(&config).expect("SSH connect failed");
    assert!(ssh.is_connected());

    let mut channel = ssh.open_pty(100, 30).expect("open_pty failed");

    let deadline = Instant::now() + Duration::from_secs(10);
    let mut written = false;
    let mut output = Vec::new();
    let mut buf = [0u8; 4096];

    while Instant::now() < deadline {
        if !written {
            match channel.write(b"pwd\r") {
                Ok(_) => {
                    let _ = channel.flush();
                    written = true;
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(e) => panic!("PTY write failed: {e}"),
            }
        }

        match channel.read(&mut buf) {
            Ok(0) => {}
            Ok(n) => {
                output.extend_from_slice(&buf[..n]);
                let text = String::from_utf8_lossy(&output);
                if written && (text.contains("/home/") || text.contains("~")) {
                    println!("--- PTY output ---\n{text}");
                    let _ = std::fs::remove_file(&known_hosts);
                    return;
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(e) => panic!("PTY read failed: {e}"),
        }

        std::thread::sleep(Duration::from_millis(20));
    }

    let text = String::from_utf8_lossy(&output);
    let _ = std::fs::remove_file(&known_hosts);
    panic!("timed out waiting for pwd output; got:\n{text}");
}

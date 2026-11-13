//! openssh_tests — OpenSSH config 解析

use rterm_session::openssh::parse_openssh_str;
use std::path::Path;

#[test]
fn parse_basic_host() {
    let raw = r#"
# 测试注释
Host myserver
    HostName 192.168.1.95
    Port 2222
    User ubuntu
    IdentityFile ~/.ssh/id_ed25519
    ServerAliveInterval 60

Host bastion
    HostName bastion.example.com
    User admin
    ProxyJump gateway.example.com

# 通配 Host 应跳过
Host prod-*
    HostName 10.0.0.1
"#;
    let home = Path::new("C:\\Users\\wst_k");
    let sessions = parse_openssh_str(raw, Path::new("test.conf"), home).expect("parse");

    assert_eq!(sessions.len(), 2, "只应解析 myserver + bastion，跳过 prod-*");

    let myserver = sessions.iter().find(|s| s.name == "myserver").unwrap();
    assert_eq!(myserver.host, "192.168.1.95");
    assert_eq!(myserver.port, 2222);
    assert_eq!(myserver.username, "ubuntu");
    assert_eq!(myserver.keepalive_secs, 60);
    assert!(matches!(myserver.auth, rterm_session::AuthMethod::PrivateKey { .. }));

    let bastion = sessions.iter().find(|s| s.name == "bastion").unwrap();
    assert_eq!(bastion.host, "bastion.example.com");
    assert_eq!(bastion.username, "admin");
    assert!(bastion.proxy_jump.is_some(), "ProxyJump 应解析为嵌套 Session");
}

#[test]
fn hostname_defaults_to_host() {
    let raw = "Host just-a-host\n    User me\n";
    let sessions = parse_openssh_str(raw, Path::new("test.conf"), Path::new(".")).unwrap();
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].host, "just-a-host");  // HostName 默认 = Host
    assert_eq!(sessions[0].port, 22);              // 默认端口
}

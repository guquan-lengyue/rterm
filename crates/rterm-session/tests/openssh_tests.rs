//! openssh_tests — OpenSSH config 解析

use rterm_session::openssh::{parse_openssh_config, parse_openssh_str};
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

    assert_eq!(
        sessions.len(),
        2,
        "只应解析 myserver + bastion，跳过 prod-*"
    );

    let myserver = sessions.iter().find(|s| s.name == "myserver").unwrap();
    assert_eq!(myserver.host, "192.168.1.95");
    assert_eq!(myserver.port, 2222);
    assert_eq!(myserver.username, "ubuntu");
    assert_eq!(myserver.keepalive_secs, 60);
    assert!(matches!(
        myserver.auth,
        rterm_session::AuthMethod::PrivateKey { .. }
    ));

    let bastion = sessions.iter().find(|s| s.name == "bastion").unwrap();
    assert_eq!(bastion.host, "bastion.example.com");
    assert_eq!(bastion.username, "admin");
    assert!(
        bastion.proxy_jump.is_some(),
        "ProxyJump 应解析为嵌套 Session"
    );
}

#[test]
fn hostname_defaults_to_host() {
    let raw = "Host just-a-host\n    User me\n";
    let sessions = parse_openssh_str(raw, Path::new("test.conf"), Path::new(".")).unwrap();
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].host, "just-a-host"); // HostName 默认 = Host
    assert_eq!(sessions[0].port, 22); // 默认端口
}

#[test]
fn top_level_include_is_parsed() {
    let base = std::env::temp_dir().join(format!(
        "rterm-openssh-test-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    std::fs::create_dir_all(&base).expect("创建临时目录");
    std::fs::write(
        base.join("extra.conf"),
        "Host included-host\n    HostName 10.9.8.7\n    User incuser\n",
    )
    .expect("写被包含文件");
    let main = base.join("config");
    std::fs::write(
        &main,
        "Include extra.conf\n\nHost main-host\n    HostName 1.2.3.4\n",
    )
    .expect("写主配置文件");

    // 相对 Include 路径按 config 所在目录解析
    let sessions = parse_openssh_config(&main).expect("parse");
    assert_eq!(sessions.len(), 2, "顶层 Include + 主文件各一个 Host");

    let included = sessions.iter().find(|s| s.name == "included-host").unwrap();
    assert_eq!(included.host, "10.9.8.7");
    assert_eq!(included.username, "incuser");

    let main_host = sessions.iter().find(|s| s.name == "main-host").unwrap();
    assert_eq!(main_host.host, "1.2.3.4");

    let _ = std::fs::remove_dir_all(&base);
}

#[test]
fn duplicate_keywords_first_wins() {
    let raw = "\
Host dup
    HostName first.example
    HostName second.example
    Port 2200
    Port 2201
    User first-user
    User second-user
";
    let sessions = parse_openssh_str(raw, Path::new("test.conf"), Path::new(".")).unwrap();
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].host, "first.example", "重复 HostName 取第一个");
    assert_eq!(sessions[0].port, 2200, "重复 Port 取第一个");
    assert_eq!(sessions[0].username, "first-user", "重复 User 取第一个");
}

#[test]
fn host_with_multiple_patterns_creates_sessions() {
    let raw = "\
Host alpha beta prod-*
    HostName shared.example
    User shared
    Port 2022
";
    let sessions = parse_openssh_str(raw, Path::new("test.conf"), Path::new(".")).unwrap();
    assert_eq!(
        sessions.len(),
        2,
        "两个明确模式各生成一个会话，通配模式跳过"
    );

    for s in &sessions {
        assert_eq!(s.host, "shared.example");
        assert_eq!(s.username, "shared");
        assert_eq!(s.port, 2022);
    }
    assert!(sessions.iter().any(|s| s.name == "alpha"));
    assert!(sessions.iter().any(|s| s.name == "beta"));
}

#[test]
fn proxy_jump_uses_own_host_port_user() {
    let raw = "\
Host target
    HostName target.example
    Port 2222
    User root
    ProxyJump jumpuser@bastion.example:2200

Host plain
    HostName plain.example
    ProxyJump bastion2
";
    let sessions = parse_openssh_str(raw, Path::new("test.conf"), Path::new(".")).unwrap();

    let target = sessions.iter().find(|s| s.name == "target").unwrap();
    let jump = target.proxy_jump.as_ref().expect("target 应有跳板机");
    assert_eq!(jump.host, "bastion.example");
    assert_eq!(jump.port, 2200, "跳板机使用自己的端口，而不是主机的 2222");
    assert_eq!(jump.username, "jumpuser");
    assert_ne!(jump.port, target.port);

    let plain = sessions.iter().find(|s| s.name == "plain").unwrap();
    let jump2 = plain.proxy_jump.as_ref().expect("plain 应有跳板机");
    assert_eq!(jump2.host, "bastion2");
    assert_eq!(jump2.port, 22, "未写端口时跳板机默认 22");
    assert_eq!(jump2.username, "");
}

#[test]
fn match_host_block_applies_when_condition_holds() {
    let raw = "\
Host web
    HostName web.example
    User deploy
Match host web
    Port 2200

Host ops
    HostName ops.example
    User ops
Match user ops
    Port 2201
";
    let sessions = parse_openssh_str(raw, Path::new("test.conf"), Path::new(".")).unwrap();

    let web = sessions.iter().find(|s| s.name == "web").unwrap();
    assert_eq!(web.port, 2200, "Match host web 应给 web 补 Port");
    assert_eq!(web.username, "deploy", "Host 自身关键字优先");

    let ops = sessions.iter().find(|s| s.name == "ops").unwrap();
    assert_eq!(ops.port, 2201, "Match user ops 应命中已出现的 User");
}

#[test]
fn match_glob_and_negation() {
    let raw = "\
Host db1
    HostName db1.example
Match host db* !db2
    User dbadmin

Host db2
    HostName db2.example
Match host db* !db2
    User wrong
";
    let sessions = parse_openssh_str(raw, Path::new("test.conf"), Path::new(".")).unwrap();

    let db1 = sessions.iter().find(|s| s.name == "db1").unwrap();
    assert_eq!(db1.username, "dbadmin", "db* 命中且未被 !db2 否定");
    let db2 = sessions.iter().find(|s| s.name == "db2").unwrap();
    assert!(db2.username.is_empty(), "!db2 否定项应让 db2 不命中");
}

#[test]
fn match_all_and_unsupported_conditions() {
    let raw = "\
Host everything
    HostName e.example
Match all
    Port 2222

Host exec-host
    HostName x.example
Match exec \"test 1\"
    Port 9999
";
    let sessions = parse_openssh_str(raw, Path::new("test.conf"), Path::new(".")).unwrap();

    let all = sessions.iter().find(|s| s.name == "everything").unwrap();
    assert_eq!(all.port, 2222, "Match all 应生效");
    let exec = sessions.iter().find(|s| s.name == "exec-host").unwrap();
    assert_eq!(exec.port, 22, "exec 条件无法静态判定，按不生效处理");
}

#[test]
fn include_recurses_two_levels_and_stops() {
    let base = std::env::temp_dir().join(format!(
        "rterm-openssh-rec-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    std::fs::create_dir_all(&base).expect("创建临时目录");
    // main -> a.conf -> b.conf -> c.conf；只允许两层。
    std::fs::write(
        base.join("a.conf"),
        "Include b.conf\nHost host-a\n    HostName 10.0.0.1\n",
    )
    .expect("写 a.conf");
    std::fs::write(
        base.join("b.conf"),
        "Include c.conf\nHost host-b\n    HostName 10.0.0.2\n",
    )
    .expect("写 b.conf");
    std::fs::write(base.join("c.conf"), "Host host-c\n    HostName 10.0.0.3\n").expect("写 c.conf");
    let main = base.join("config");
    std::fs::write(
        &main,
        "Include a.conf\nHost host-main\n    HostName 10.0.0.9\n",
    )
    .expect("写主配置");

    let sessions = parse_openssh_config(&main).expect("parse");
    assert!(sessions.iter().any(|s| s.name == "host-main"));
    assert!(sessions.iter().any(|s| s.name == "host-a"));
    assert!(
        sessions.iter().any(|s| s.name == "host-b"),
        "第二层 Include 应解析"
    );
    assert!(
        !sessions.iter().any(|s| s.name == "host-c"),
        "第三层 Include 超出深度限制，应停止"
    );

    let _ = std::fs::remove_dir_all(&base);
}

#[test]
fn match_ends_host_block_and_is_ignored() {
    let raw = "\
Host keep
    HostName keep.example
Match host foo
    User ignored
    Port 9999

Host after
    HostName after.example
    User real
";
    let sessions = parse_openssh_str(raw, Path::new("test.conf"), Path::new(".")).unwrap();
    assert_eq!(sessions.len(), 2, "Match 后的 Host 仍应解析");

    let keep = sessions.iter().find(|s| s.name == "keep").unwrap();
    assert_eq!(keep.host, "keep.example");
    assert!(
        keep.username.is_empty(),
        "Match 块内的 User 不应影响上一个 Host"
    );
    assert_eq!(keep.port, 22, "Match 块内的 Port 不应影响上一个 Host");

    let after = sessions.iter().find(|s| s.name == "after").unwrap();
    assert_eq!(after.host, "after.example");
    assert_eq!(after.username, "real");
}

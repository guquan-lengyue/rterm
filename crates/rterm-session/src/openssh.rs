//! openssh — 解析 OpenSSH `~/.ssh/config` 为 Session 列表
//!
//! OpenSSH config 语法示例（text，不编译）：
//! ```text
//! Host myserver
//!     HostName 192.168.1.95
//!     Port 22
//!     User ubuntu
//!     IdentityFile ~/.ssh/id_rsa
//!     ServerAliveInterval 30
//!     ProxyJump bastion
//! ```
//!
//! 支持的关键字（MVP）：Host / HostName / Port / User / IdentityFile /
//! ServerAliveInterval / ProxyJump / Include（递归 1 层）。

use anyhow::{Context, Result};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::model::{AuthMethod, Session};

/// 解析 OpenSSH config 文件。
pub fn parse_openssh_config(path: &Path) -> Result<Vec<Session>> {
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("读 openssh config 失败: {}", path.display()))?;
    let home = dirs_home();
    parse_openssh_str(&raw, path, &home)
}

/// 从字符串解析（测试时用内联样例）。
pub fn parse_openssh_str(raw: &str, _source_path: &Path, home: &Path) -> Result<Vec<Session>> {
    let blocks = split_into_host_blocks(raw);
    let mut sessions = Vec::new();

    for (host_pattern, kw_list) in blocks {
        // 跳过通配 Host（Host * / Host prod-*），只吃明确名称
        if host_pattern.contains('*') || host_pattern.contains('?') {
            continue;
        }

        let mut kv: HashMap<String, String> = HashMap::new();
        for (k, v) in kw_list {
            kv.insert(k, v);
        }

        // Include — 递归 1 层
        if let Some(include) = kv.remove("include") {
            let inc_path = expand_path(&include, home);
            if inc_path.exists() {
                let inc_raw = std::fs::read_to_string(&inc_path).unwrap_or_default();
                let mut incs = parse_openssh_str(&inc_raw, &inc_path, home).unwrap_or_default();
                sessions.append(&mut incs);
            }
        }

        let host = match kv.get("hostname") {
            Some(h) => h.clone(),
            None => host_pattern.clone(),  // HostName 默认就是 Host
        };

        let port = kv
            .get("port")
            .and_then(|p| p.parse::<u16>().ok())
            .unwrap_or(22);

        let username = kv.get("user").cloned().unwrap_or_default();

        let keepalive = kv
            .get("serveraliveinterval")
            .and_then(|p| p.parse::<u32>().ok())
            .unwrap_or(30);

        let auth = if let Some(idf) = kv.get("identityfile") {
            let expanded = expand_path(idf, home);
            AuthMethod::PrivateKey {
                path: expanded,
                passphrase: None,
            }
        } else {
            AuthMethod::Agent { key_comment: String::new() }
        };

        let proxy_jump = kv.get("proxyjump").map(|pj| {
            // 简化处理：只取第一个 host，递归构造 Session
            let first = pj.split(',').next().unwrap_or(pj);
            let mut j = Session::new(first, first, "");
            j.port = kv.get("port").and_then(|p| p.parse().ok()).unwrap_or(22);
            j.auth = AuthMethod::Agent { key_comment: String::new() };
            Box::new(j)
        });

        let s = Session {
            id: uuid::Uuid::new_v4(),
            name: host_pattern,
            host,
            port,
            username,
            auth,
            proxy_jump,
            keepalive_secs: keepalive,
            connect_timeout_secs: 10,
            folder: Some("OpenSSH Import".to_string()),
            created_at: chrono::Utc::now(),
            last_modified: chrono::Utc::now(),
            // 显式给了所有字段，不 default 覆盖
            ..Default::default()
        };
        sessions.push(s);
    }

    Ok(sessions)
}

/// 按 `Host xxx` 行切块。返回全所有权 String（避免 &str.leak()）。
fn split_into_host_blocks(raw: &str) -> Vec<(String, Vec<(String, String)>)> {
    let mut blocks: Vec<(String, Vec<(String, String)>)> = Vec::new();
    let mut current: Option<(String, Vec<(String, String)>)> = None;

    for line in raw.lines() {
        let trimmed = line.trim();
        // 跳过注释和空行
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }

        let (key, rest) = match trimmed.split_once(|c: char| c.is_whitespace() || c == '=') {
            Some(kv) => kv,
            None => continue,
        };
        let value = rest.trim_start_matches('=').trim();
        let key_lower = key.to_ascii_lowercase();

        if key_lower == "host" {
            // 切换 block
            if let Some(prev) = current.take() {
                blocks.push(prev);
            }
            current = Some((value.to_string(), Vec::new()));
        } else if let Some((_, ref mut list)) = current {
            list.push((key_lower, value.to_string()));
        }
    }

    if let Some(last) = current {
        blocks.push(last);
    }

    blocks
}

fn expand_path(p: &str, home: &Path) -> PathBuf {
    let trimmed = p.trim();
    if let Some(suffix) = trimmed.strip_prefix("~/") {
        home.join(suffix)
    } else {
        PathBuf::from(trimmed)
    }
}

fn dirs_home() -> PathBuf {
    if let Ok(p) = std::env::var("HOME") {
        return PathBuf::from(p);
    }
    if let Ok(p) = std::env::var("USERPROFILE") {
        return PathBuf::from(p);
    }
    PathBuf::from(".")
}

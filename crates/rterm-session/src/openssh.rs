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
//! ServerAliveInterval / ProxyJump / Include（顶层，递归 1 层）。
//!
//! 注意：`Match` 条件语法不在当前 Sprint 实现范围内 —— 遇到 Match 行会
//! 结束当前 Host 块，并忽略其后的非 Host 行。

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
pub fn parse_openssh_str(raw: &str, source_path: &Path, home: &Path) -> Result<Vec<Session>> {
    parse_openssh_inner(raw, source_path, home, true)
}

/// `allow_include = false` 时不再解析 Include，保证只递归 1 层。
fn parse_openssh_inner(
    raw: &str,
    source_path: &Path,
    home: &Path,
    allow_include: bool,
) -> Result<Vec<Session>> {
    let (includes, blocks) = split_into_host_blocks(raw);
    let mut sessions = Vec::new();

    // 顶层 Include（出现在任何 Host 之前）：解析被包含文件并追加会话。
    // 相对路径按 source_path 的父目录解析，失败只记日志。
    if allow_include {
        for include in includes {
            let inc_path = resolve_include_path(&include, source_path, home);
            let inc_raw = match std::fs::read_to_string(&inc_path) {
                Ok(r) => r,
                Err(e) => {
                    log::warn!("读取 Include 文件失败 {}: {e}", inc_path.display());
                    continue;
                }
            };
            match parse_openssh_inner(&inc_raw, &inc_path, home, false) {
                Ok(mut incs) => sessions.append(&mut incs),
                Err(e) => log::warn!("解析 Include 文件失败 {}: {e:#}", inc_path.display()),
            }
        }
    }

    for (host_pattern, kw_list) in blocks {
        // 同一 Host 块内重复关键字取第一个值（first-wins）
        let mut kv: HashMap<String, String> = HashMap::new();
        for (k, v) in kw_list {
            kv.entry(k).or_insert(v);
        }

        // Host 块内的 Include（兼容旧行为，同样只递归 1 层）
        if allow_include {
            if let Some(include) = kv.remove("include") {
                let inc_path = resolve_include_path(&include, source_path, home);
                match std::fs::read_to_string(&inc_path) {
                    Ok(inc_raw) => match parse_openssh_inner(&inc_raw, &inc_path, home, false) {
                        Ok(mut incs) => sessions.append(&mut incs),
                        Err(e) => log::warn!(
                            "解析 Include 文件失败 {}: {e:#}",
                            inc_path.display()
                        ),
                    },
                    Err(e) => log::warn!("读取 Include 文件失败 {}: {e}", inc_path.display()),
                }
            }
        }

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
            // 只取链上第一跳，解析 [user@]host[:port]；缺省端口 22。
            let first = pj.split(',').next().unwrap_or(pj).trim();
            let (jump_user, jump_host, jump_port) = parse_proxy_jump(first);
            let mut j = Session::new(jump_host.clone(), jump_host, jump_user);
            j.port = jump_port;
            j.folder = Some("OpenSSH Import".to_string());
            j.auth = AuthMethod::Agent { key_comment: String::new() };
            Box::new(j)
        });

        // `Host a b` 多模式：每个非通配模式各生成一个 Session，共享同一组关键字
        for pattern in host_pattern.split_whitespace() {
            // 跳过通配 Host（Host * / Host prod-*），只吃明确名称
            if pattern.contains('*') || pattern.contains('?') {
                continue;
            }

            // HostName 默认就是 Host
            let host = kv
                .get("hostname")
                .cloned()
                .unwrap_or_else(|| pattern.to_string());

            let s = Session {
                id: uuid::Uuid::new_v4(),
                name: pattern.to_string(),
                host,
                port,
                username: username.clone(),
                auth: auth.clone(),
                proxy_jump: proxy_jump.clone(),
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
    }

    Ok(sessions)
}

/// 单个 Host 块：模式串（如 `a b`）+ 关键字列表（保持文件顺序，便于 first-wins）。
type HostBlock = (String, Vec<(String, String)>);

/// 按 `Host xxx` 行切块，并收集顶层 Include（出现在任何 Host 之前）。
/// 返回全所有权 String（避免 &str.leak()）。
fn split_into_host_blocks(raw: &str) -> (Vec<String>, Vec<HostBlock>) {
    let mut blocks: Vec<HostBlock> = Vec::new();
    let mut includes: Vec<String> = Vec::new();
    let mut current: Option<HostBlock> = None;
    // 遇到 Match 后忽略后续非 Host 行，直到下一个 Host。
    let mut in_match = false;

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

        match key_lower.as_str() {
            "host" => {
                // 切换 block
                if let Some(prev) = current.take() {
                    blocks.push(prev);
                }
                in_match = false;
                current = Some((value.to_string(), Vec::new()));
            }
            "match" => {
                // Match 条件语法暂不支持：结束当前 Host 块并忽略后续非 Host 行
                if let Some(prev) = current.take() {
                    blocks.push(prev);
                }
                in_match = true;
            }
            _ => {
                if in_match {
                    continue;
                }
                match current.as_mut() {
                    Some((_, list)) => list.push((key_lower, value.to_string())),
                    None => {
                        // 还没有 Host 块：只接受顶层 Include
                        if key_lower == "include" {
                            includes.push(value.to_string());
                        }
                    }
                }
            }
        }
    }

    if let Some(last) = current {
        blocks.push(last);
    }

    (includes, blocks)
}

/// 解析 `ProxyJump` 的 `[user@]host[:port]` 片段，缺省端口 22。
fn parse_proxy_jump(spec: &str) -> (String, String, u16) {
    let (user, rest) = match spec.rsplit_once('@') {
        Some((u, r)) => (u.trim().to_string(), r.trim()),
        None => (String::new(), spec.trim()),
    };
    match rest.rsplit_once(':') {
        Some((host, port)) if !host.is_empty() => match port.parse::<u16>() {
            Ok(port) if port >= 1 => (user, host.to_string(), port),
            _ => (user, rest.to_string(), 22),
        },
        _ => (user, rest.to_string(), 22),
    }
}

/// Include 路径：`~/` 展开 home，相对路径按 source_path 的父目录解析。
fn resolve_include_path(include: &str, source_path: &Path, home: &Path) -> PathBuf {
    let expanded = expand_path(include, home);
    if expanded.is_absolute() {
        expanded
    } else {
        match source_path.parent() {
            Some(parent) if !parent.as_os_str().is_empty() => parent.join(expanded),
            _ => expanded,
        }
    }
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

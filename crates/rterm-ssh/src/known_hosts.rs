//! known_hosts — 主机密钥校验与首次信任
//!
//! 使用 libssh2 自带的 KnownHosts 实现（兼容 OpenSSH 明文格式）。
//! S5 采用 TOFU（Trust On First Use）：首次连接自动追加，
//! 已存在但密钥变化 → 返回 `Changed`，由调用方拒绝连接。
//! 完整的三选一确认弹窗 + 持久化收口在 S10 9.1。

use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use base64::{engine::general_purpose::STANDARD_NO_PAD, Engine as _};
use ssh2::{CheckResult, HashType, HostKeyType, KnownHostFileKind};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HostKeyPolicy {
    /// 未记录的主机直接拒绝（不写入）。
    Strict,
    /// 首次连接自动记录（Trust On First Use）。
    TrustOnFirstUse,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HostKeyDecision {
    Match,
    Unknown,
    Changed,
}

#[derive(Clone, Debug)]
pub struct HostKeyInfo {
    pub host: String,
    pub key_type: String,
    pub fingerprint: String,
    pub decision: HostKeyDecision,
}

/// `~/.ssh/known_hosts`（Windows 用 USERPROFILE，其他平台用 HOME）。
/// 测试可用 `RTERM_KNOWN_HOSTS` 指向临时文件，避免污染真实 known_hosts。
pub fn default_path() -> PathBuf {
    if let Some(path) = std::env::var_os("RTERM_KNOWN_HOSTS") {
        return PathBuf::from(path);
    }
    let home = std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    home.join(".ssh").join("known_hosts")
}

/// 校验主机密钥；按 policy 决定是否写入新 host。
pub fn check_and_record(
    session: &ssh2::Session,
    host: &str,
    port: u16,
    path: &Path,
    policy: HostKeyPolicy,
) -> Result<HostKeyInfo> {
    let (key, key_type) = session
        .host_key()
        .context("无法获取服务器主机密钥")?;
    let fingerprint = session
        .host_key_hash(HashType::Sha256)
        .map(|hash| STANDARD_NO_PAD.encode(hash))
        .unwrap_or_default();

    let mut known_hosts = session.known_hosts().context("初始化 known_hosts 失败")?;
    if path.exists() {
        known_hosts
            .read_file(path, KnownHostFileKind::OpenSSH)
            .with_context(|| format!("读取 known_hosts 失败: {}", path.display()))?;
    }

    let decision = match known_hosts.check_port(host, port, key) {
        CheckResult::Match => HostKeyDecision::Match,
        CheckResult::Mismatch => HostKeyDecision::Changed,
        CheckResult::NotFound => HostKeyDecision::Unknown,
        CheckResult::Failure => bail!("known_hosts 校验失败（{}:{}）", host, port),
    };

    let info = HostKeyInfo {
        host: host.to_string(),
        key_type: key_type_name(key_type).to_string(),
        fingerprint,
        decision,
    };

    if decision == HostKeyDecision::Unknown && policy == HostKeyPolicy::TrustOnFirstUse {
        let entry_host = if port == 22 {
            host.to_string()
        } else {
            format!("[{host}]:{port}")
        };
        known_hosts
            .add(&entry_host, key, "rterm", key_type.into())
            .with_context(|| format!("写入主机密钥失败: {entry_host}"))?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("创建 .ssh 目录失败: {}", parent.display()))?;
        }
        known_hosts
            .write_file(path, KnownHostFileKind::OpenSSH)
            .with_context(|| format!("保存 known_hosts 失败: {}", path.display()))?;
        log::info!(
            "[known_hosts] 首次连接 {}:{}，已记录主机密钥 {}",
            host,
            port,
            info.fingerprint
        );
    }

    Ok(info)
}

fn key_type_name(key_type: HostKeyType) -> &'static str {
    match key_type {
        HostKeyType::Rsa => "ssh-rsa",
        HostKeyType::Dss => "ssh-dss",
        HostKeyType::Ecdsa256 => "ecdsa-sha2-nistp256",
        HostKeyType::Ecdsa384 => "ecdsa-sha2-nistp384",
        HostKeyType::Ecdsa521 => "ecdsa-sha2-nistp521",
        HostKeyType::Ed25519 => "ssh-ed25519",
        HostKeyType::Unknown => "unknown",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_path_ends_with_ssh_known_hosts() {
        let path = default_path();
        assert!(path.ends_with(Path::new(".ssh").join("known_hosts")));
    }

    #[test]
    fn key_type_names_are_openssh_compatible() {
        assert_eq!(key_type_name(HostKeyType::Ed25519), "ssh-ed25519");
        assert_eq!(key_type_name(HostKeyType::Rsa), "ssh-rsa");
    }
}

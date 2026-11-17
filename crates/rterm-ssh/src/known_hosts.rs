//! known_hosts — 主机密钥校验、三选一确认与原子写入
//!
//! 用 libssh2 自带的 KnownHosts 做 Match/Unknown/Changed 分类，
//! 写入改为手工构造 OpenSSH 明文行 + 同目录临时文件 rename，
//! 并用进程内互斥锁串行化，避免并发写坏文件。

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use anyhow::{bail, Context, Result};
use base64::{
    engine::general_purpose::{STANDARD, STANDARD_NO_PAD},
    Engine as _,
};
use hmac::{Hmac, Mac};
use sha1::Sha1;
use sha2::{Digest, Sha256};
use ssh2::{CheckResult, HashType, HostKeyType, KnownHostFileKind};

/// 所有 known_hosts 写入共享同一把锁（进程内串行）。
static KNOWN_HOSTS_WRITE_LOCK: Mutex<()> = Mutex::new(());
/// 临时文件序号，避免同一进程内并发写入使用相同文件名。
static TMP_FILE_COUNTER: AtomicU64 = AtomicU64::new(0);

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

/// 用户对未知 / 变更主机密钥的选择。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HostKeyChoice {
    /// 接受并写入 known_hosts（替换旧记录）。
    AcceptAndSave,
    /// 仅本次接受，不写文件。
    AcceptOnce,
    /// 拒绝连接。
    Reject,
}

#[derive(Clone, Debug)]
pub struct HostKeyInfo {
    pub host: String,
    pub key_type: String,
    pub fingerprint: String,
    /// Changed 时尝试从 known_hosts 解析出的旧指纹；hashed / 格式异常时为 None。
    pub old_fingerprint: Option<String>,
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

/// 校验主机密钥；`verifier` 用于未知 / 变更时让用户三选一。
pub fn verify_host_key(
    session: &ssh2::Session,
    host: &str,
    port: u16,
    path: &Path,
    policy: HostKeyPolicy,
    verifier: Option<&dyn Fn(&HostKeyInfo) -> HostKeyChoice>,
) -> Result<HostKeyInfo> {
    let (key, key_type) = session.host_key().context("无法获取服务器主机密钥")?;
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

    // Changed 时尽量给出旧指纹，便于用户判断是否真的换了服务器密钥。
    let old_fingerprint = if decision == HostKeyDecision::Changed {
        stored_fingerprint(path, host, port)
    } else {
        None
    };

    let mut info = HostKeyInfo {
        host: host.to_string(),
        key_type: key_type_name(key_type).to_string(),
        fingerprint,
        old_fingerprint,
        decision,
    };

    match decision {
        HostKeyDecision::Match => Ok(info),
        HostKeyDecision::Unknown | HostKeyDecision::Changed => {
            if let Some(verifier) = verifier {
                match verifier(&info) {
                    HostKeyChoice::AcceptAndSave => {
                        write_host_key(path, host, port, &info.key_type, key)?;
                        if decision == HostKeyDecision::Changed {
                            // 用户已确认替换，后续 lib.rs 不应再按 Changed 拒绝。
                            info.decision = HostKeyDecision::Match;
                        }
                        log::info!("[known_hosts] 已接受并保存 {}:{} 的主机密钥", host, port);
                        Ok(info)
                    }
                    HostKeyChoice::AcceptOnce => {
                        if decision == HostKeyDecision::Changed {
                            info.decision = HostKeyDecision::Match;
                        }
                        log::info!(
                            "[known_hosts] 仅本次接受 {}:{} 的主机密钥（未写入）",
                            host,
                            port
                        );
                        Ok(info)
                    }
                    HostKeyChoice::Reject => bail!("用户拒绝接受主机密钥"),
                }
            } else if decision == HostKeyDecision::Unknown
                && policy == HostKeyPolicy::TrustOnFirstUse
            {
                write_host_key(path, host, port, &info.key_type, key)?;
                log::info!(
                    "[known_hosts] 首次连接 {}:{}，已记录主机密钥 {}",
                    host,
                    port,
                    info.fingerprint
                );
                Ok(info)
            } else if decision == HostKeyDecision::Unknown {
                bail!("未知主机密钥（{}:{}），Strict 策略下拒绝连接", host, port)
            } else {
                // Changed 且无 verifier：返回 info，由调用方拒绝。
                Ok(info)
            }
        }
    }
}

/// 校验主机密钥；按 policy 决定是否写入新 host（无交互版本）。
pub fn check_and_record(
    session: &ssh2::Session,
    host: &str,
    port: u16,
    path: &Path,
    policy: HostKeyPolicy,
) -> Result<HostKeyInfo> {
    verify_host_key(session, host, port, path, policy, None)
}

fn entry_host(host: &str, port: u16) -> String {
    if port == 22 {
        host.to_string()
    } else {
        format!("[{host}]:{port}")
    }
}

/// 原子写入一条 host key 记录；同 host 的旧行会被替换。
fn write_host_key(path: &Path, host: &str, port: u16, key_type: &str, key: &[u8]) -> Result<()> {
    // poison 时继续持锁写入：known_hosts 的完整性比 panic 传播更重要。
    let _guard = KNOWN_HOSTS_WRITE_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());

    let entry = entry_host(host, port);
    let encoded = STANDARD_NO_PAD.encode(key);

    let existing = match std::fs::read_to_string(path) {
        Ok(content) => content,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(error) => {
            return Err(error).with_context(|| format!("读取 known_hosts 失败: {}", path.display()))
        }
    };

    // 保留注释 / 空行 / 其他 host 行，只替换当前 host。
    let mut lines: Vec<String> = existing
        .lines()
        .filter(|line| !line_matches_entry(line, &entry))
        .map(|line| line.to_string())
        .collect();
    lines.push(format!("{entry} {key_type} {encoded}"));

    let mut content = lines.join("\n");
    content.push('\n');

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("创建 .ssh 目录失败: {}", parent.display()))?;
    }

    let temp_path = sibling_path(path, "rterm-tmp");

    let write_result = (|| -> Result<()> {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp_path)
            .with_context(|| format!("创建临时文件失败: {}", temp_path.display()))?;
        file.write_all(content.as_bytes())
            .with_context(|| format!("写入临时文件失败: {}", temp_path.display()))?;
        file.sync_all()
            .with_context(|| format!("同步临时文件失败: {}", temp_path.display()))?;
        drop(file);

        replace_file(&temp_path, path)
    })();

    if write_result.is_err() {
        let _ = std::fs::remove_file(&temp_path);
    }
    write_result
}

/// 与 `path` 同目录的唯一兄弟文件名（临时文件 / 备份）。
fn sibling_path(path: &Path, tag: &str) -> PathBuf {
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("known_hosts");
    let sequence = TMP_FILE_COUNTER.fetch_add(1, Ordering::Relaxed);
    dir.join(format!(
        "{file_name}.{tag}-{}-{}",
        std::process::id(),
        sequence
    ))
}

/// 用 `temp` 替换 `target`。
///
/// 首选一次 `rename`（Rust 在 Windows 上使用
/// `MoveFileExW(MOVEFILE_REPLACE_EXISTING)`，通常可直接覆盖）。
/// 若因文件被占用 / 权限失败且目标存在，则走「备份 → 替换 → 失败回滚」，
/// 保证任何时刻原内容都不会丢失（不再有"先删旧文件"的空窗）。
fn replace_file(temp: &Path, target: &Path) -> Result<()> {
    match std::fs::rename(temp, target) {
        Ok(()) => return Ok(()),
        Err(error) if !target.exists() => {
            return Err(error).with_context(|| format!("替换文件失败: {}", target.display()));
        }
        Err(_) => {}
    }

    let backup = sibling_path(target, "rterm-bak");
    std::fs::rename(target, &backup)
        .with_context(|| format!("备份原文件失败: {}", target.display()))?;
    match std::fs::rename(temp, target) {
        Ok(()) => {
            let _ = std::fs::remove_file(&backup);
            Ok(())
        }
        Err(error) => {
            // 回滚：把备份放回原位，避免目标文件凭空消失。
            let _ = std::fs::rename(&backup, target);
            Err(error).with_context(|| format!("替换文件失败: {}", target.display()))
        }
    }
}

/// 判断一行 known_hosts 是否属于 `entry`
/// （兼容逗号分隔的 host 列表与 `|1|` hashed host）。
fn line_matches_entry(line: &str, entry: &str) -> bool {
    let trimmed = line.trim_start();
    if trimmed.is_empty() || trimmed.starts_with('#') {
        return false;
    }
    let Some(hosts) = trimmed.split_whitespace().next() else {
        return false;
    };
    host_field_matches(hosts, entry)
}

/// host 字段（可能是逗号分隔列表）中任一项匹配 `entry` 即视为命中。
fn host_field_matches(field: &str, entry: &str) -> bool {
    field.split(',').any(|candidate| {
        candidate.eq_ignore_ascii_case(entry) || hashed_host_matches(candidate, entry)
    })
}

/// OpenSSH hashed hostname：`|1|BASE64(salt)|BASE64(HMAC-SHA1(salt, hostname))`。
///
/// 哈希是对 hostname 字节串做的，因此必须按原始字符串比对（不区分大小写
/// 的放宽只对明文行有效）；部分客户端写入前会转小写，这里两种都试。
fn hashed_host_matches(hashed: &str, entry: &str) -> bool {
    let Some(rest) = hashed.strip_prefix("|1|") else {
        return false;
    };
    let mut parts = rest.split('|');
    let (Some(salt), Some(expected)) = (parts.next(), parts.next()) else {
        return false;
    };
    let (Some(salt), Some(expected)) = (decode_base64(salt), decode_base64(expected)) else {
        return false;
    };

    if verify_hmac_sha1(&salt, entry.as_bytes(), &expected) {
        return true;
    }
    let lowercase = entry.to_ascii_lowercase();
    lowercase != entry && verify_hmac_sha1(&salt, lowercase.as_bytes(), &expected)
}

fn verify_hmac_sha1(key: &[u8], message: &[u8], expected: &[u8]) -> bool {
    let Ok(mut mac) = Hmac::<Sha1>::new_from_slice(key) else {
        return false;
    };
    mac.update(message);
    // verify_slice 是常量时间比较，避免指纹探测。
    mac.verify_slice(expected).is_ok()
}

/// 从 known_hosts 解析该 host 已保存的 key blob 并计算 SHA256 指纹。
/// 支持 hashed host（`|1|...`）；格式异常的行跳过。
pub fn stored_fingerprint(path: &Path, host: &str, port: u16) -> Option<String> {
    let entry = entry_host(host, port);
    let content = std::fs::read_to_string(path).ok()?;
    for line in content.lines() {
        let trimmed = line.trim_start();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let mut parts = trimmed.split_whitespace();
        let Some(hosts) = parts.next() else {
            continue;
        };
        // 跳过 @cert-authority / @revoked 等标记行。
        if hosts.starts_with('@') {
            continue;
        }
        if !host_field_matches(hosts, &entry) {
            continue;
        }
        let _key_type = parts.next();
        let Some(encoded) = parts.next() else {
            continue;
        };
        let Some(blob) = decode_base64(encoded) else {
            continue;
        };
        return Some(sha256_fingerprint(&blob));
    }
    None
}

fn decode_base64(encoded: &str) -> Option<Vec<u8>> {
    STANDARD_NO_PAD
        .decode(encoded)
        .or_else(|_| STANDARD.decode(encoded))
        .ok()
}

fn sha256_fingerprint(blob: &[u8]) -> String {
    STANDARD_NO_PAD.encode(Sha256::digest(blob))
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

    fn temp_path(tag: &str) -> PathBuf {
        let sequence = TMP_FILE_COUNTER.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "rterm-known-hosts-{}-{}-{}",
            std::process::id(),
            tag,
            sequence
        ))
    }

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

    #[test]
    fn write_new_host_creates_entry() {
        let path = temp_path("new");
        write_host_key(&path, "example.com", 22, "ssh-ed25519", b"blob").expect("写入新 host");

        let content = std::fs::read_to_string(&path).expect("读取 known_hosts");
        assert!(content.lines().any(|line| line
            == format!(
                "example.com ssh-ed25519 {}",
                STANDARD_NO_PAD.encode(b"blob")
            )));

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn replace_existing_host_keeps_single_line() {
        let path = temp_path("replace");
        std::fs::write(
            &path,
            "# 保留这条注释\nother.example.com ssh-ed25519 b3RoZXI=\n\
             example.com,alias.example.com ssh-ed25519 b2xk\n\n",
        )
        .expect("准备 known_hosts");

        write_host_key(&path, "example.com", 22, "ssh-ed25519", b"new-blob").expect("替换 host");

        let content = std::fs::read_to_string(&path).expect("读取 known_hosts");
        let matches: Vec<&str> = content
            .lines()
            .filter(|line| line.starts_with("example.com"))
            .collect();
        assert_eq!(matches.len(), 1);
        assert!(matches[0].contains(&STANDARD_NO_PAD.encode(b"new-blob")));
        // 注释和其他 host 必须保留。
        assert!(content.contains("# 保留这条注释"));
        assert!(content.contains("other.example.com"));

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn stored_fingerprint_is_stable_for_known_blob() {
        let path = temp_path("fingerprint");
        let blob = b"known-blob-for-fingerprint";
        write_host_key(&path, "fp.example.com", 2200, "ssh-rsa", blob).expect("写入 host");

        let expected = sha256_fingerprint(blob);
        let first = stored_fingerprint(&path, "fp.example.com", 2200).expect("解析指纹");
        let second = stored_fingerprint(&path, "fp.example.com", 2200).expect("再次解析指纹");
        assert_eq!(first, expected);
        assert_eq!(second, expected);

        let _ = std::fs::remove_file(&path);
    }

    /// 构造 OpenSSH hashed host 行（与 ssh-keygen -H 输出格式一致）。
    fn hashed_entry(salt: &[u8], host: &str) -> String {
        let mut mac = Hmac::<Sha1>::new_from_slice(salt).expect("HMAC key");
        mac.update(host.as_bytes());
        let digest = mac.finalize().into_bytes();
        format!(
            "|1|{}|{}",
            STANDARD_NO_PAD.encode(salt),
            STANDARD_NO_PAD.encode(digest)
        )
    }

    #[test]
    fn hashed_host_lines_are_matched() {
        let salt = b"0123456789abcdefghij";
        let hashed = hashed_entry(salt, "hashed.example.com");

        assert!(line_matches_entry(
            &format!("{hashed} ssh-ed25519 AAAA"),
            "hashed.example.com"
        ));
        assert!(!line_matches_entry(
            &format!("{hashed} ssh-ed25519 AAAA"),
            "other.example.com"
        ));
        // 带端口的入口同样是普通字符串哈希
        let hashed_port = hashed_entry(salt, "[hashed.example.com]:2222");
        assert!(line_matches_entry(
            &format!("{hashed_port} ssh-ed25519 AAAA"),
            "[hashed.example.com]:2222"
        ));
        // 非 |1| 前缀 / 结构损坏不 panic
        assert!(!line_matches_entry("|1|bad ssh-ed25519 AAAA", "h"));
        assert!(!line_matches_entry("|2|x|y ssh-ed25519 AAAA", "h"));
    }

    #[test]
    fn hashed_stored_fingerprint_is_resolved() {
        let path = temp_path("hashed-fp");
        let blob = b"hashed-blob";
        let hashed = hashed_entry(b"salt-salt-salt-salt-", "hashed-fp.example.com");
        std::fs::write(
            &path,
            format!("{hashed} ssh-ed25519 {}\n", STANDARD_NO_PAD.encode(blob)),
        )
        .expect("准备 hashed known_hosts");

        let fingerprint = stored_fingerprint(&path, "hashed-fp.example.com", 22).expect("解析指纹");
        assert_eq!(fingerprint, sha256_fingerprint(blob));

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn accepting_key_replaces_hashed_line() {
        let path = temp_path("hashed-replace");
        let hashed = hashed_entry(b"another-salt-abcdefgh", "hash-replace.example.com");
        std::fs::write(
            &path,
            format!("{hashed} ssh-ed25519 b2xk\nother.example.com ssh-ed25519 b3RoZXI=\n"),
        )
        .expect("准备 known_hosts");

        write_host_key(&path, "hash-replace.example.com", 22, "ssh-ed25519", b"new")
            .expect("写入新密钥");

        let content = std::fs::read_to_string(&path).expect("读取 known_hosts");
        assert!(
            !content.contains("|1|"),
            "hashed 旧行应被删除，避免重复提示"
        );
        assert_eq!(
            content
                .lines()
                .filter(|line| line.starts_with("hash-replace.example.com "))
                .count(),
            1
        );
        assert!(content.contains("other.example.com"));

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn replace_file_overwrites_target() {
        let target = temp_path("replace-file");
        std::fs::write(&target, b"old").expect("写旧文件");
        let temp = sibling_path(&target, "rterm-tmp");
        std::fs::write(&temp, b"new").expect("写临时文件");

        replace_file(&temp, &target).expect("替换成功");

        assert_eq!(std::fs::read(&target).expect("读目标"), b"new");
        assert!(!temp.exists(), "临时文件应已被 rename 消耗");

        let _ = std::fs::remove_file(&target);
    }

    #[test]
    fn concurrent_writes_keep_both_hosts() {
        let path = temp_path("concurrent");
        let path_a = path.clone();
        let path_b = path.clone();

        let first = std::thread::spawn(move || {
            write_host_key(&path_a, "host-a.example.com", 22, "ssh-ed25519", b"blob-a")
        });
        let second = std::thread::spawn(move || {
            write_host_key(&path_b, "host-b.example.com", 22, "ssh-ed25519", b"blob-b")
        });

        first.join().expect("线程 A").expect("写入 host-a");
        second.join().expect("线程 B").expect("写入 host-b");

        let content = std::fs::read_to_string(&path).expect("读取 known_hosts");
        assert!(content
            .lines()
            .any(|line| line.starts_with("host-a.example.com ")));
        assert!(content
            .lines()
            .any(|line| line.starts_with("host-b.example.com ")));

        let _ = std::fs::remove_file(&path);
    }
}

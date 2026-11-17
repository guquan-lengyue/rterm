//! 终端 / SFTP 路径同步核心。
//!
//! `SyncBridge` 只做纯状态机：
//! - 从 PTY 输出里解析当前 shell 路径（PS1）
//! - 把 SFTP 导航转换成终端 `cd` 命令
//! - 用 `last_terminal` / `last_sftp` / `ignore_list` 防止双向回环
//!
//! 它不持有 PTY、SFTP 或 UI 句柄；调用方负责把返回值转换成事件/命令。

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// 同步模式，与 `Settings.sftp_sync_mode` 的字符串一一对应。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SyncMode {
    Off,
    TerminalToSftp,
    SftpToTerminal,
    Bidirectional,
}

impl SyncMode {
    /// 从设置字符串解析；未知值回退双向同步（与设置默认值一致）。
    pub fn from_setting(value: &str) -> Self {
        match value {
            "off" => Self::Off,
            "term_to_sftp" => Self::TerminalToSftp,
            "sftp_to_term" => Self::SftpToTerminal,
            "bidirectional" => Self::Bidirectional,
            _ => Self::Bidirectional,
        }
    }

    fn terminal_to_sftp(self) -> bool {
        matches!(self, Self::TerminalToSftp | Self::Bidirectional)
    }

    fn sftp_to_terminal(self) -> bool {
        matches!(self, Self::SftpToTerminal | Self::Bidirectional)
    }
}

/// 路径来源，便于 UI/日志区分 PS1 解析与后续 pwd 兜底。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CwdSource {
    Ps1,
    PwdFallback,
    SftpNav,
}

/// 从终端输出中识别出的路径。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DetectedCwd {
    pub path: PathBuf,
    pub source: CwdSource,
}

/// 双向路径同步状态机。
pub struct SyncBridge {
    mode: SyncMode,
    /// 最近若干字节的 PTY 输出，用于处理跨 chunk 的提示符。
    tail: Vec<u8>,
    last_terminal: Option<PathBuf>,
    last_sftp: Option<PathBuf>,
    /// 由 SFTP → 终端注入的 `cd` 路径；终端回显该路径时不再反向触发。
    ignore_list: VecDeque<PathBuf>,
    /// 由终端 → SFTP 主动导航产生的 SftpNavigated 数量；这些事件不应反向注入。
    pending_sftp_nav: usize,
    /// PS1 无法识别、且末尾看起来像提示符时，允许注入一次 pwd 探测。
    need_pwd: bool,
    last_pwd_probe: Option<Instant>,
    /// 远端 home（由 SFTP 连接提供），用于把 PS1 的 `~` 解析成绝对路径。
    remote_home: Option<PathBuf>,
}

const TAIL_LIMIT: usize = 4096;
const IGNORE_LIMIT: usize = 10;
const MAX_PATH_LEN: usize = 1024;
const PWD_PROBE_INTERVAL: Duration = Duration::from_secs(2);
/// 行首空格避免污染常见 HISTCONTROL=ignorespace 的历史记录。
const PWD_PROBE: &str = " printf '\\033]9;rterm-cwd:%s\\007' \"$PWD\"\r";

/// 从一段（可含 ANSI 控制码的）PTY 输出中解析 PS1 末尾的 cwd。
///
/// 这是 `SyncBridge` 内部使用的纯函数，单独公开便于测试/诊断。
pub fn parse_ps1_prompt(ansi_bytes: &[u8]) -> Option<PathBuf> {
    extract_prompt_cwd(&strip_ansi(ansi_bytes)).map(PathBuf::from)
}

impl SyncBridge {
    pub fn new(mode: SyncMode) -> Self {
        Self {
            mode,
            tail: Vec::new(),
            last_terminal: None,
            last_sftp: None,
            ignore_list: VecDeque::new(),
            pending_sftp_nav: 0,
            need_pwd: false,
            last_pwd_probe: None,
            remote_home: None,
        }
    }

    pub fn mode(&self) -> SyncMode {
        self.mode
    }

    /// 设置变更时即时切换模式。
    pub fn set_mode(&mut self, mode: SyncMode) {
        self.mode = mode;
    }

    /// 终端有新的 PTY 输出时调用。只在识别到新的 cwd 时返回 Some。
    pub fn on_terminal_output(&mut self, bytes: &[u8]) -> Option<DetectedCwd> {
        if !self.mode.terminal_to_sftp() {
            return None;
        }

        self.tail.extend_from_slice(bytes);
        if self.tail.len() > TAIL_LIMIT {
            let drain = self.tail.len() - TAIL_LIMIT;
            self.tail.drain(..drain);
        }

        // 优先处理 pwd 兜底标记（OSC 9），它在 strip_ansi 后会被删除。
        if let Some(path) = extract_osc9_cwd(&self.tail) {
            self.need_pwd = false;
            return self.accept_cwd(PathBuf::from(path), CwdSource::PwdFallback);
        }

        let text = strip_ansi(&self.tail);
        if let Some(raw) = extract_prompt_cwd(&text) {
            self.need_pwd = false;
            return self.accept_cwd(PathBuf::from(raw), CwdSource::Ps1);
        }

        // 命令输出以换行结束：之前的兜底请求已过期，等新提示符再判断。
        if text.ends_with('\n') {
            self.need_pwd = false;
        } else if looks_like_unknown_prompt(&text) {
            // 末尾像提示符但没有可识别 cwd：标记为需要 pwd 兜底。
            self.need_pwd = true;
        }
        None
    }

    /// 若 PS1 解析失败，返回一次 pwd 探测命令（2s 内不重复）。
    pub fn take_pwd_probe(&mut self) -> Option<&'static str> {
        if !self.mode.terminal_to_sftp() || !self.need_pwd {
            return None;
        }
        if let Some(last) = self.last_pwd_probe {
            if last.elapsed() < PWD_PROBE_INTERVAL {
                return None;
            }
        }
        self.last_pwd_probe = Some(Instant::now());
        Some(PWD_PROBE)
    }

    /// SFTP 连接成功后设置远端 home；同时把已有的 `~` 表示刷新为绝对路径。
    pub fn set_remote_home(&mut self, home: impl Into<PathBuf>) {
        let home = normalize_path(&home.into());
        self.remote_home = Some(home);
        if let Some(last) = self.last_terminal.clone() {
            self.last_terminal = Some(self.resolve_path(last));
        }
        if let Some(last) = self.last_sftp.clone() {
            self.last_sftp = Some(self.resolve_path(last));
        }
    }

    /// SFTP 首次连接/打开时已知的当前路径，避免初始化导航反向注入 cd。
    pub fn mark_sftp_synced(&mut self, path: &Path) {
        self.last_sftp = Some(self.resolve_path(normalize_path(path)));
    }

    fn resolve_path(&self, path: PathBuf) -> PathBuf {
        let Some(home) = &self.remote_home else {
            return path;
        };
        let raw = path.to_string_lossy();
        if raw == "~" {
            return home.clone();
        }
        if let Some(rest) = raw.strip_prefix("~/") {
            return home.join(rest);
        }
        path
    }

    fn accept_cwd(&mut self, path: PathBuf, source: CwdSource) -> Option<DetectedCwd> {
        let path = self.resolve_path(path);
        // SFTP → 终端注入的 cd 回显：消费 ignore 项，不反向导航。
        if let Some(index) = self.ignore_list.iter().position(|entry| entry == &path) {
            self.ignore_list.remove(index);
            self.last_terminal = Some(path);
            return None;
        }

        if self.last_terminal.as_ref() == Some(&path) {
            return None;
        }

        self.last_terminal = Some(path.clone());
        Some(DetectedCwd { path, source })
    }

    /// 终端 → SFTP 主动导航前调用，告诉桥接器接下来会有 `count` 个 SftpNavigated 是被动事件。
    pub fn expect_sftp_navigation(&mut self, count: usize) {
        self.pending_sftp_nav = self.pending_sftp_nav.saturating_add(count);
    }

    /// SFTP 导航成功后调用。若需要同步到终端，返回要注入的 `cd` 命令。
    pub fn on_sftp_navigated(&mut self, path: &Path) -> Option<String> {
        if !self.mode.sftp_to_terminal() {
            return None;
        }

        let path = normalize_path(path);

        // 由终端 → SFTP 触发的导航：更新状态但不反向注入。
        if self.pending_sftp_nav > 0 {
            self.pending_sftp_nav -= 1;
            self.last_sftp = Some(path);
            return None;
        }

        if let Some(index) = self.ignore_list.iter().position(|entry| entry == &path) {
            self.ignore_list.remove(index);
            self.last_sftp = Some(path);
            return None;
        }

        // 终端已经在同一路径（常见于绝对路径 PS1）：不重复注入 cd。
        if self.last_terminal.as_ref() == Some(&path) {
            self.last_sftp = Some(path);
            return None;
        }

        if self.last_sftp.as_ref() == Some(&path) {
            return None;
        }

        self.last_sftp = Some(path.clone());
        self.push_ignore(path.clone());
        Some(format!("cd {}", shell_path(&path)))
    }

    /// 记录一个终端路径，避免 SFTP 导航回到同一位置时重复注入。
    pub fn remember_terminal_path(&mut self, path: PathBuf) {
        self.last_terminal = Some(path);
    }

    fn push_ignore(&mut self, path: PathBuf) {
        self.ignore_list.push_back(path);
        while self.ignore_list.len() > IGNORE_LIMIT {
            self.ignore_list.pop_front();
        }
    }
}

/// 去掉常见 ANSI CSI/OSC 转义与 `\r`，保留可读文本。
fn strip_ansi(bytes: &[u8]) -> String {
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == 0x1b {
            i += 1;
            if i >= bytes.len() {
                break;
            }
            match bytes[i] {
                b'[' => {
                    // CSI：吃到 @..~ 的终止字节。
                    i += 1;
                    while i < bytes.len() {
                        let b = bytes[i];
                        i += 1;
                        if (0x40..=0x7e).contains(&b) {
                            break;
                        }
                    }
                }
                b']' => {
                    // OSC：吃到 BEL 或 ST（ESC \）。
                    i += 1;
                    while i < bytes.len() {
                        if bytes[i] == 0x07 {
                            i += 1;
                            break;
                        }
                        if bytes[i] == 0x1b && bytes.get(i + 1) == Some(&b'\\') {
                            i += 2;
                            break;
                        }
                        i += 1;
                    }
                }
                _ => {
                    // 两字节 ESC 序列。
                    i += 1;
                }
            }
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }

    String::from_utf8_lossy(&out).replace('\r', "")
}

/// 从 pwd 兜底探测的 OSC 9 标记中提取路径。
fn extract_osc9_cwd(bytes: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(bytes);
    let marker = "\x1b]9;rterm-cwd:";
    let start = text.rfind(marker)? + marker.len();
    let rest = &text[start..];
    let end = rest.find('\x07').or_else(|| rest.find("\x1b\\"))?;
    let path = &rest[..end];
    if path.starts_with('/') || path.starts_with('~') {
        Some(path.to_string())
    } else {
        None
    }
}

/// 末尾没有换行、且以常见提示符结尾，说明可能是无法识别的 PS1。
fn looks_like_unknown_prompt(text: &str) -> bool {
    if text.ends_with('\n') {
        return false;
    }
    let line = text.rsplit('\n').next().unwrap_or("").trim_end();
    if line.is_empty() || line.len() > 120 {
        return false;
    }
    // 不把 `>` 当作提示符：Python REPL / PowerShell 等交互程序也以 `>` 结尾，
    // 误注入 pwd 探测会污染输入。远程 Linux shell 的常见提示符是 $/#/%/❯。
    if !matches!(line.chars().last(), Some('$' | '#' | '%' | '❯')) {
        return false;
    }
    // 降低误判：`echo $` 这类正在输入的命令也以 $ 结尾，但通常含空格且没有 @/:。
    line.contains('@') || line.contains(':') || !line.contains(' ')
}

/// 从文本最后一行提取提示符中的 cwd。
///
/// 支持：
/// - bash：`user@host:/var/log$`、`user@host:~#`
/// - zsh：`user@host /var/log %`、`user@host ~ %`
/// - 简单自定义：`/var/log$`、`(venv) user@host:~/proj$`
fn extract_prompt_cwd(text: &str) -> Option<String> {
    let line = text.rsplit('\n').next()?.trim_end();
    if line.is_empty() {
        return None;
    }

    let marker = line.chars().last()?;
    if !matches!(marker, '$' | '#' | '%' | '❯') {
        return None;
    }

    let marker_byte = line.len() - marker.len_utf8();
    let mut prefix = line[..marker_byte].trim_end();

    // 去掉常见包装符：`[user@host ~]`、`(user@host ~)`、`<...>`。
    prefix = prefix.trim_end_matches([']', ')', '>', '❯']);
    prefix = prefix.trim_end();

    // 优先取最后一个 `:` 之后（user@host:path）；否则取最后一个空白之后。
    let candidate = if let Some(index) = prefix.rfind(':') {
        prefix[index + 1..].trim()
    } else if let Some((index, _)) = prefix
        .char_indices()
        .rev()
        .find(|(_, ch)| ch.is_whitespace())
    {
        prefix[index..].trim()
    } else {
        prefix
    };

    if candidate.is_empty() || candidate.len() > MAX_PATH_LEN {
        return None;
    }
    if candidate.chars().any(|ch| ch.is_whitespace()) {
        return None;
    }
    if !(candidate.starts_with('/') || candidate.starts_with('~')) {
        return None;
    }

    Some(candidate.to_string())
}

/// 统一去掉尾部 `/`（根目录除外），避免 `/a/` 与 `/a` 被当成不同路径。
fn normalize_path(path: &Path) -> PathBuf {
    let raw = path.to_string_lossy();
    if raw == "/" {
        return PathBuf::from("/");
    }
    PathBuf::from(raw.trim_end_matches('/'))
}

/// 生成 shell 安全的路径参数；`~` 必须保持未引用才能被 shell 展开。
fn shell_path(path: &Path) -> String {
    let raw = path.to_string_lossy();
    if raw == "~" {
        return "~".to_string();
    }
    if let Some(rest) = raw.strip_prefix("~/") {
        return format!("~/{}", shell_quote(rest));
    }
    shell_quote(&raw)
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn detect(bridge: &mut SyncBridge, text: &str) -> Option<PathBuf> {
        bridge
            .on_terminal_output(text.as_bytes())
            .map(|detected| detected.path)
    }

    #[test]
    fn parses_bash_prompt() {
        let mut bridge = SyncBridge::new(SyncMode::Bidirectional);
        assert_eq!(
            detect(&mut bridge, "ubuntu@ubuntu:~$ "),
            Some(PathBuf::from("~"))
        );
        assert_eq!(
            detect(&mut bridge, "ubuntu@ubuntu:/var/log$ "),
            Some(PathBuf::from("/var/log"))
        );
    }

    #[test]
    fn parses_zsh_and_custom_prompts() {
        let mut bridge = SyncBridge::new(SyncMode::TerminalToSftp);
        assert_eq!(
            detect(&mut bridge, "user@host /var/log % "),
            Some(PathBuf::from("/var/log"))
        );
        assert_eq!(
            detect(&mut bridge, "(venv) user@host:~/proj# "),
            Some(PathBuf::from("~/proj"))
        );
    }

    #[test]
    fn strips_ansi_before_parsing() {
        let mut bridge = SyncBridge::new(SyncMode::Bidirectional);
        assert_eq!(
            detect(&mut bridge, "\x1b[32muser@host\x1b[0m:/tmp$ "),
            Some(PathBuf::from("/tmp"))
        );
        assert_eq!(
            parse_ps1_prompt(b"\x1b[32muser@host\x1b[0m:/tmp$ "),
            Some(PathBuf::from("/tmp"))
        );
    }

    #[test]
    fn pwd_fallback_marker_is_detected() {
        let mut bridge = SyncBridge::new(SyncMode::Bidirectional);
        let detected = bridge
            .on_terminal_output(b"\x1b]9;rterm-cwd:/opt/app\x07")
            .expect("pwd marker should be parsed");
        assert_eq!(detected.path, PathBuf::from("/opt/app"));
        assert_eq!(detected.source, CwdSource::PwdFallback);
    }

    #[test]
    fn unknown_prompt_requests_pwd_probe_once() {
        let mut bridge = SyncBridge::new(SyncMode::Bidirectional);
        assert_eq!(detect(&mut bridge, "custom-prompt$ "), None);
        assert!(bridge.take_pwd_probe().is_some());
        assert_eq!(bridge.take_pwd_probe(), None);
    }

    #[test]
    fn handles_prompt_split_across_chunks() {
        let mut bridge = SyncBridge::new(SyncMode::Bidirectional);
        assert_eq!(detect(&mut bridge, "user@host:/var"), None);
        assert_eq!(
            detect(&mut bridge, "/log$ "),
            Some(PathBuf::from("/var/log"))
        );
    }

    #[test]
    fn ignores_command_output_and_dedupes() {
        let mut bridge = SyncBridge::new(SyncMode::Bidirectional);
        assert_eq!(detect(&mut bridge, "hello$\n"), None);
        assert_eq!(
            detect(&mut bridge, "user@host:/tmp$ "),
            Some(PathBuf::from("/tmp"))
        );
        assert_eq!(detect(&mut bridge, "user@host:/tmp$ "), None);
    }

    #[test]
    fn modes_gate_both_directions() {
        let mut off = SyncBridge::new(SyncMode::Off);
        assert_eq!(detect(&mut off, "user@host:/tmp$ "), None);
        assert_eq!(off.on_sftp_navigated(Path::new("/tmp")), None);

        let mut term_only = SyncBridge::new(SyncMode::TerminalToSftp);
        assert!(detect(&mut term_only, "user@host:/tmp$ ").is_some());
        assert_eq!(term_only.on_sftp_navigated(Path::new("/var")), None);

        let mut sftp_only = SyncBridge::new(SyncMode::SftpToTerminal);
        assert_eq!(detect(&mut sftp_only, "user@host:/tmp$ "), None);
        assert!(sftp_only.on_sftp_navigated(Path::new("/var")).is_some());
    }

    #[test]
    fn sftp_navigation_injects_and_ignores_echo() {
        let mut bridge = SyncBridge::new(SyncMode::Bidirectional);
        assert_eq!(
            bridge.on_sftp_navigated(Path::new("/var/log/")),
            Some("cd '/var/log'".to_string())
        );
        // 终端回显 `cd '/var/log'` 后的提示符不应再次触发 SFTP 导航。
        assert_eq!(detect(&mut bridge, "user@host:/var/log$ "), None);
        // 同一个 SFTP 路径也不重复注入。
        assert_eq!(bridge.on_sftp_navigated(Path::new("/var/log")), None);
    }

    #[test]
    fn remote_home_resolves_tilde_paths() {
        let mut bridge = SyncBridge::new(SyncMode::Bidirectional);
        bridge.set_remote_home("/home/ubuntu");
        assert_eq!(
            detect(&mut bridge, "user@host:~$ "),
            Some(PathBuf::from("/home/ubuntu"))
        );
        assert_eq!(
            detect(&mut bridge, "user@host:~/proj$ "),
            Some(PathBuf::from("/home/ubuntu/proj"))
        );
    }

    #[test]
    fn initial_sftp_path_can_be_marked_as_synced() {
        let mut bridge = SyncBridge::new(SyncMode::Bidirectional);
        bridge.set_remote_home("/home/ubuntu");
        bridge.mark_sftp_synced(Path::new("/home/ubuntu"));
        assert_eq!(bridge.on_sftp_navigated(Path::new("/home/ubuntu")), None);
    }

    #[test]
    fn expected_sftp_navigation_is_not_injected_back() {
        let mut bridge = SyncBridge::new(SyncMode::Bidirectional);
        assert!(detect(&mut bridge, "user@host:/tmp$ ").is_some());
        bridge.expect_sftp_navigation(1);
        assert_eq!(bridge.on_sftp_navigated(Path::new("/tmp")), None);
        assert_eq!(
            bridge.on_sftp_navigated(Path::new("/var")),
            Some("cd '/var'".into())
        );
    }

    #[test]
    fn quotes_paths_but_preserves_tilde() {
        assert_eq!(shell_path(Path::new("~/a b")), "~/'a b'");
        assert_eq!(shell_path(Path::new("/tmp/a'b")), "'/tmp/a'\\''b'");
        assert_eq!(shell_path(Path::new("~")), "~");
    }
}

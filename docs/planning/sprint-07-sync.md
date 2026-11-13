# Sprint 7 — 路径同步

> **状态**：—
> **前置**：[Sprint 5](./sprint-05-terminal-render.md)（Terminal PTY 输出）+ [Sprint 6](./sprint-06-sftp.md)（SFTP 导航）
> **目标**：SyncBridge + PS1 正则 + ignore 防回环
> **定位**：rterm 独有杀手功能

---

### 7.1 SyncBridge

| # | 粒度 | 文件 | 要写的 | 验收 |
|---|------|------|--------|------|
| 7.1.1 | 0.5h | `crates/rterm-sync/src/lib.rs` | `pub enum SyncMode { Off, TerminalToSftp, SftpToTerminal, Bidirectional }` + `pub enum CwdSource { Ps1, PwdFallback, SftpNav }` + `pub struct SyncBridge { mode, last_terminal, last_sftp, ignore_list }` | 能构造 Bidirectional 默认 |
| 7.1.2 | 0.5h | `crates/rterm-sync/src/lib.rs` | PS1 正则（3 套模板）：bash default `w@h:p$` → 提取路径；zsh default；generic 带 ANSI 控制码剥离；`pub fn parse_ps1_prompt(ansi_bytes: &[u8]) -> Option<PathBuf>` | 给它 bash 真实 PS1 字节 → 提取 cwd |
| 7.1.3 | 0.5h | `crates/rterm-sync/src/lib.rs` | `pub fn on_terminal_output(&mut self, bytes: &[u8]) -> Option<PathBuf>` — PS1 正则 → 有则返回；无则标记 need_pwd；对比 last_terminal → 不同才返回 | 同一个 prompt 多次调用只返回一次 cwd |
| 7.1.4 | 0.5h | `crates/rterm-sync/src/lib.rs` | Pwd 兜底：on_terminal_output 返回 None 但 need_pwd=true → 后台注入命令 + 等待回复；超时 1s 忽略 | PS1 异常时 pwd 兜底能工作 |
| 7.1.5 | 0.5h | `crates/rterm-sync/src/lib.rs` | `pub fn on_sftp_navigated(&mut self, new_path: &Path) -> Option<InjectCommand>` — ignore_list 检查 → 有则返回 None；无则 `Some(format!("cd {}", shell_escape(new_path)))` + push ignore | ignore_list 生效；注入命令里路径被正确 shell 转义 |
| 7.1.6 | 0.25h | `crates/rterm-sync/src/lib.rs` | ignore_list 老化：VecDeque（max 10）FIFO；每次 push 自动 pop 最老 | 第 11 次 push 时第 1 个没了 |

### 7.2 集成

| # | 粒度 | 文件 | 要写的 | 验收 |
|---|------|------|--------|------|
| 7.2.1 | 0.5h | `crates/rterm-app/src/main.rs` | 每次 TerminalView 喂完 PTY 字节 → `sync_bridge.on_terminal_output(bytes)` → 若 Some(cwd) → broadcast `SftpNavigated` | 终端 cd /var/log → SFTP 自动跳 |
| 7.2.2 | 0.5h | `crates/rterm-app/src/main.rs` | 每次 SFTP 用户导航 → broadcast `UserRequestedCwd { path }` → sync_bridge.on_sftp_navigated → 组装命令 → PTY channel.write | SFTP 导航 → 终端自动 cd |

### 7.3 验收

| # | 粒度 | 验收 |
|---|------|------|
| 7.3.1 | 0.25h | Bidirectional 模式：`cd /var/log && pwd` → SFTP 跳 /var/log；SFTP 手动导航 /etc → 终端 prompt 变；**不**无限循环；切 Off 模式 → 完全独立 |

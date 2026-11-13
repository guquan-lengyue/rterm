# Sprint 5 — SSH 连接 & 终端核心

> **状态**：—（依赖 S3）
> **前置**：[Sprint 2](./sprint-02-session-data.md)（Session 模型）+ [Sprint 3](./sprint-03-session-tree.md)（会话树能触发 SessionConnectRequested）
> **目标**：rterm-ssh 封装 + 重连 + PTY；rterm-terminal TerminalEngine + alacritty_terminal glue
> **难度**：⚠️ 整个项目最具技术挑战的 Sprint

---

### 4.0 Perl 验证 ✅ 已完成（2026-11-13）

| # | 粒度 | 文件 | 要写的 | 验收 |
|---|------|------|--------|------|
| 4.0.1 | 0.25h | ✅ | perl 5.42.3 已装（`C:\Strawberry\perl\bin\perl.exe`）；已加 `scripts/msvc-env.ps1`；完整 `cargo check --workspace` 过（9m13s） | ✅ ssh2 vendored-openssl 编过 |

### 4.1 rterm-ssh — 连接与认证

| # | 粒度 | 文件 | 要写的 | 验收 |
|---|------|------|--------|------|
| 4.1.1 | 0.5h | `crates/rterm-ssh/src/lib.rs` | `pub struct SshSession { inner: Option<ssh2::Session>, state: ConnectionState }` + `pub enum ConnectionState { Disconnected, Connecting, Connected, Reconnecting(attempt) }` | 能 new() + 默认 Disconnected |
| 4.1.2 | 0.5h | `crates/rterm-ssh/src/auth.rs` | `pub fn authenticate(session: &mut ssh2::Session, session_config: &Session, decrypt: impl FnMut(&str) -> Result<Vec<u8>>) -> Result<()>` — Password → `userauth_password`；PrivateKey → `userauth_pubkey_file`；Agent → `userauth_agent` | 三种认证方式都能返回 Ok(()) |
| 4.1.3 | 0.5h | `crates/rterm-ssh/src/lib.rs` | `pub fn connect(&mut self, session: &Session) -> Result<()>` — tcp connect + `handshake()` + host_key check + authenticate；失败 map 为 anyhow 带上下文 | 对真实服务器能 connect |
| 4.1.4 | 0.5h | `crates/rterm-ssh/src/known_hosts.rs` | `pub fn check_and_record(session: &ssh2::Session, known_hosts_path: &Path) -> Result<HostKeyDecision>` — 读 `~/.ssh/known_hosts`；首次 → 广播 AppEvent::HostKeyUnknown（UI 弹窗三选一）；变更 → 广播 AppEvent::HostKeyChanged | 能 append 新 host；变更能检测 |

### 4.2 rterm-ssh — PTY + 后台 IO

| # | 粒度 | 文件 | 要写的 | 验收 |
|---|------|------|--------|------|
| 4.2.1 | 0.5h | `crates/rterm-ssh/src/pty.rs` | `pub fn open_pty(session: &mut SshSession, cols: u32, rows: u32) -> Result<(ssh2::Channel, mpsc::Receiver<Vec<u8>>)>` — `session.inner.channel_session()` + `request_pty("xterm-256color", cols, rows, 0, 0)` + `channel.exec("bash -l")` | 返回 Channel + Receiver |
| 4.2.2 | 0.5h | `crates/rterm-ssh/src/pty.rs` | `tokio::spawn` 后台 loop：`channel.read(4096)` → `tx.send(bytes)` → 读到 None 断开；所有 IO 放 `spawn_blocking` | 真实服务器 bash 输出字节流能发回 |
| 4.2.3 | 0.25h | `crates/rterm-ssh/src/pty.rs` | `pub fn resize_pty(channel: &mut ssh2::Channel, cols: u32, rows: u32) -> Result<()>` | resize 后远程终端更新 |

### 4.3 rterm-terminal — TerminalEngine

| # | 粒度 | 文件 | 要写的 | 验收 |
|---|------|------|--------|------|
| 4.3.1 | 0.5h | `crates/rterm-terminal/src/engine.rs` | `pub struct TerminalEngine { term: Term<SizeInfo>, scrollback_lines: usize }` + `impl TerminalEngine { pub fn new(cols, rows, scrollback) -> Self }` | `new(80, 24, 10000)` 构造不崩 |
| 4.3.2 | 0.5h | `crates/rterm-terminal/src/engine.rs` | `pub fn input_bytes(&mut self, bytes: &[u8])` — vte::Parser → feed → Term.input() | 喂 "hello\r\n" → Grid 里有 h,e,l,l,o |
| 4.3.3 | 0.5h | `crates/rterm-terminal/src/engine.rs` | `pub fn grid(&self) -> &Grid<Cell>` + `pub fn cursor(&self) -> Cursor` + `pub fn resize(&mut self, cols, rows)` + `pub fn active_colors(&self) -> &[Rgb; 256]` | 能读 Grid / Cursor / Colors |
| 4.3.4 | 0.5h | `crates/rterm-terminal/src/engine.rs` | `pub fn input_key(&mut self, key: TerminalKey) -> Vec<u8>` — TerminalKey enum (Enter, Backspace, Up, Down, CtrlC, ...) → xterm 序列；同时写 `term.paste_bytes(&seq)` 回显 | input_key(Enter) 返回 `b"\r"`；Grid 光标下移 |

### 4.4 rterm-ssh — KeepAlive + 重连

| # | 粒度 | 文件 | 要写的 | 验收 |
|---|------|------|--------|------|
| 4.4.1 | 0.5h | `crates/rterm-ssh/src/keepalive.rs` | `pub fn start_keepalive(session_id: Uuid, interval_secs: u32, mut session: SshSession) -> Task<()>` — tokio interval → `channel.exec("echo _PING_ > /dev/null")` → 失败计次 | 30s 发一次 echo |
| 4.4.2 | 0.5h | `crates/rterm-ssh/src/reconnect.rs` | 重连状态机：Disconnected → Connecting → Connected；失败 → 指数退避 sleep(1s/2s/4s/8s/16s)；每步广播 `SessionReconnecting { attempt }`；超过 5 次 → `SessionDisconnected` | 真实网络断开 → 观察自动重连 |

### 4.5 集成到 rterm-app

| # | 粒度 | 文件 | 要写的 | 验收 |
|---|------|------|--------|------|
| 4.5.1 | 0.5h | `crates/rterm-app/src/main.rs` | 订阅 SessionConnectRequested → 后台 spawn SshSession.connect → 成功 open_pty → 发字节到 TerminalEngine | 双击会话 → SSH 连接成功 → PTY 字节开始流动 |
| 4.5.2 | 0.25h | `crates/rterm-app/src/views/session_tree.rs` | 会话连接状态图标（Disconnected/Connecting/Connected/Failed）；EventBus 订阅更新；Ctrl+P 命令面板搜会话 | 真实服务器连接时看到图标变绿 |
| 4.5.3 | 0.25h | `crates/rterm-app/src/views/main_window.rs` | 中间区显示 TerminalPlaceholder：每次收到 TerminalData 事件 → `engine.input_bytes()` → `cx.notify()` → 显示 mock Grid 内容（先用 placeholder 占位不做 Canvas） | 看到终端输出纯文本或 placeholder |

### 4.6 验收

| # | 粒度 | 验收 |
|---|------|------|
| 4.6.1 | 0.5h | 连接真实 Linux → bash 能输入命令 → `pwd` 回显正确 → `vim` 能进 → tmux 能起 → 断网重连自动恢复 |

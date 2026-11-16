# Sprint 5 — SSH 连接 & 终端核心

> **状态**：✅ 已完成（2026-11-14）
> **前置**：[Sprint 2](./sprint-02-session-data.md)（Session 模型）+ [Sprint 3](./sprint-03-session-tree.md)（会话树已能触发 SessionConnectRequested）+ [Sprint 4](./sprint-04-settings-io.md)（Settings 默认值 + 主题/Modal 资产）
> **目标**：rterm-ssh 封装 + 重连 + PTY；rterm-terminal TerminalEngine + alacritty_terminal glue
> **难度**：⚠️ 整个项目最具技术挑战的 Sprint
>
> **实现说明（与计划的偏差）**：
> - 5.1.4 host key 采用 **TOFU 自动记录** + EventBus 广播；三选一确认 Modal 延后到 S10 9.1
> - 5.2 PTY IO：ssh2 blocking `channel.read` 会锁住整个 Session，导致输入写不出去 → shell 打开后 `set_blocking(false)`，单线程轮询 read / write / resize / keepalive
> - 5.4.1 KeepAlive 不单独起 tokio task，由 PTY IO 循环按间隔调用 `Session::keepalive_send()`
> - 5.5.2 状态图标 + 右键「打开终端」已完成；Ctrl+P 命令面板归 S10 9.3
> - 5.5.3 用文本行 placeholder 渲染（S6 替换为 Canvas 逐 Cell 渲染）
> - Windows 必须启用 ssh2 `openssl-on-win32`：默认 WINCNG 后端没有 curve25519-sha256 / ssh-ed25519
>
> **S3 已就绪的接入点**：
> - 双击会话 → 已 emit `AppEvent::SessionConnectRequested { session_id }`（当前无订阅者，本 Sprint 5.5.1 接管）
> - 会话数据：`AppState.session_store`（`Arc<Mutex<SessionStore>>`），可查 host/port/auth
> - 密码以 DPAPI 密文存于 `EncryptedString.data`，连接前需用 `crypto_bridge::decrypt_session_dpapi` 解密
> - 主题系统 / TextInput / 自绘 Modal 已可复用（错误提示、host key 确认弹窗可直接套 SessionEditDialog 模式）
>
> **S4 已就绪的接入点**：
> - Settings 已持久化到 `%APPDATA%\rterm\settings.json`：`default_port` / `default_username` / `keepalive_secs` / `connect_timeout_secs` 可直接用于新建会话与连接超时（见 5.5.4，当前 SessionEditDialog 仍硬编码 22/30/10）
> - 主题系统已有 4 套 UI 主题（Catppuccin Mocha / Windows 11 Light / Dracula / One Dark），但仅覆盖控件颜色，**不含终端 ANSI 16 色** → S6 扩展
> - `EventBus` 已定义 `HostKeyUnknown` / `HostKeyChanged` / `SessionConnected` / `SessionFailed` / `SessionReconnecting` / `SessionDisconnected`，本 Sprint 只需 emit
> - `Settings::save()` 已实现原子写盘；`rfd` 文件选择器已引入，可用于私钥选择（可选）
> - 自绘 Modal 的遮罩 / 卡片 / footer 按钮布局可直接复用于 host key 首次确认与变更告警

---

### 5.0 Perl 验证 ✅ 已完成（2026-11-13）

| # | 粒度 | 文件 | 要写的 | 验收 |
|---|------|------|--------|------|
| 5.0.1 | 0.25h | ✅ | perl 5.42.3 已装（`C:\Strawberry\perl\bin\perl.exe`）；已加 `scripts/msvc-env.ps1`；完整 `cargo check --workspace` 过（9m13s） | ✅ ssh2 vendored-openssl 编过 |

### 5.1 rterm-ssh — 连接与认证

| # | 粒度 | 文件 | 要写的 | 验收 |
|---|------|------|--------|------|
| 5.1.1 | 0.5h | `crates/rterm-ssh/src/lib.rs` | `pub struct SshSession { inner: Option<ssh2::Session>, state: ConnectionState }` + `pub enum ConnectionState { Disconnected, Connecting, Connected, Reconnecting(attempt) }` | 能 new() + 默认 Disconnected |
| 5.1.2 | 0.5h | `crates/rterm-ssh/src/auth.rs` | `pub fn authenticate(session: &mut ssh2::Session, session_config: &Session, decrypt: impl FnMut(&str) -> Result<Vec<u8>>) -> Result<()>` — Password → `userauth_password`；PrivateKey → `userauth_pubkey_file`；Agent → `userauth_agent` | 三种认证方式都能返回 Ok(()) |
| 5.1.3 | 0.5h | `crates/rterm-ssh/src/lib.rs` | `pub fn connect(&mut self, session: &Session) -> Result<()>` — tcp connect + `handshake()` + host_key check + authenticate；超时读 `session.connect_timeout_secs`；失败 map 为 anyhow 带上下文 | 对真实服务器能 connect；超时按会话配置生效 |
| 5.1.4 | 0.5h | `crates/rterm-ssh/src/known_hosts.rs` | `pub fn check_and_record(session: &ssh2::Session, known_hosts_path: &Path) -> Result<HostKeyDecision>` — 读 `~/.ssh/known_hosts`；首次 → 广播 AppEvent::HostKeyUnknown（复用 S4 自绘 Modal 弹三选一）；变更 → 广播 AppEvent::HostKeyChanged | 能 append 新 host；变更能检测 |

### 5.2 rterm-ssh — PTY + 后台 IO

| # | 粒度 | 文件 | 要写的 | 验收 |
|---|------|------|--------|------|
| 5.2.1 | 0.5h | `crates/rterm-ssh/src/pty.rs` | `pub fn open_pty(session: &mut SshSession, cols: u32, rows: u32) -> Result<(ssh2::Channel, mpsc::Receiver<Vec<u8>>)>` — `session.inner.channel_session()` + `request_pty("xterm-256color", cols, rows, 0, 0)` + `channel.exec("bash -l")` | 返回 Channel + Receiver |
| 5.2.2 | 0.5h | `crates/rterm-ssh/src/pty.rs` | `tokio::spawn` 后台 loop：`channel.read(4096)` → `tx.send(bytes)` → 读到 None 断开；所有 IO 放 `spawn_blocking` | 真实服务器 bash 输出字节流能发回 |
| 5.2.3 | 0.25h | `crates/rterm-ssh/src/pty.rs` | `pub fn resize_pty(channel: &mut ssh2::Channel, cols: u32, rows: u32) -> Result<()>` | resize 后远程终端更新 |

### 5.3 rterm-terminal — TerminalEngine

| # | 粒度 | 文件 | 要写的 | 验收 |
|---|------|------|--------|------|
| 5.3.1 | 0.5h | `crates/rterm-terminal/src/engine.rs` | `pub struct TerminalEngine { term: Term<SizeInfo>, scrollback_lines: usize }` + `impl TerminalEngine { pub fn new(cols, rows, scrollback) -> Self }` | `new(80, 24, 10000)` 构造不崩 |
| 5.3.2 | 0.5h | `crates/rterm-terminal/src/engine.rs` | `pub fn input_bytes(&mut self, bytes: &[u8])` — vte::Parser → feed → Term.input() | 喂 "hello\r\n" → Grid 里有 h,e,l,l,o |
| 5.3.3 | 0.5h | `crates/rterm-terminal/src/engine.rs` | `pub fn grid(&self) -> &Grid<Cell>` + `pub fn cursor(&self) -> Cursor` + `pub fn resize(&mut self, cols, rows)` + `pub fn active_colors(&self) -> &[Rgb; 256]` | 能读 Grid / Cursor / Colors |
| 5.3.4 | 0.5h | `crates/rterm-terminal/src/engine.rs` | `pub fn input_key(&mut self, key: TerminalKey) -> Vec<u8>` — TerminalKey enum (Enter, Backspace, Up, Down, CtrlC, ...) → xterm 序列；同时写 `term.paste_bytes(&seq)` 回显 | input_key(Enter) 返回 `b"\r"`；Grid 光标下移 |

### 5.4 rterm-ssh — KeepAlive + 重连

| # | 粒度 | 文件 | 要写的 | 验收 |
|---|------|------|--------|------|
| 5.4.1 | 0.5h | `crates/rterm-ssh/src/keepalive.rs` | `pub fn start_keepalive(session_id: Uuid, interval_secs: u32, mut session: SshSession) -> Task<()>` — interval 取 `session.keepalive_secs`（新建会话默认值来自 S4 Settings）→ tokio interval → `channel.exec("echo _PING_ > /dev/null")` → 失败计次 | 30s 发一次 echo |
| 5.4.2 | 0.5h | `crates/rterm-ssh/src/reconnect.rs` | 重连状态机：Disconnected → Connecting → Connected；失败 → 指数退避 sleep(1s/2s/4s/8s/16s)；每步广播 `SessionReconnecting { attempt }`；超过 5 次 → `SessionDisconnected` | 真实网络断开 → 观察自动重连 |

### 5.5 集成到 rterm-app

| # | 粒度 | 文件 | 要写的 | 验收 |
|---|------|------|--------|------|
| 5.5.1 | 0.5h | `crates/rterm-app/src/main.rs` | 订阅 SessionConnectRequested → 后台 spawn SshSession.connect → 成功 open_pty → 发字节到 TerminalEngine | 双击会话 → SSH 连接成功 → PTY 字节开始流动 |
| 5.5.2 | 0.25h | `crates/rterm-app/src/views/session_tree.rs` | 会话连接状态图标（Disconnected/Connecting/Connected/Failed）；EventBus 订阅更新；Ctrl+P 命令面板搜会话 | 真实服务器连接时看到图标变绿 |
| 5.5.3 | 0.25h | `crates/rterm-app/src/views/main_window.rs` | 中间区显示 TerminalPlaceholder：每次收到 TerminalData 事件 → `engine.input_bytes()` → `cx.notify()` → 显示 mock Grid 内容（先用 placeholder 占位不做 Canvas） | 看到终端输出纯文本或 placeholder |
| 5.5.4 | 0.25h | `crates/rterm-app/src/views/session_dialog.rs` | 新建会话默认值改从 `AppState.settings` 读取：`default_port` / `default_username` / `keepalive_secs` / `connect_timeout_secs`（S4 已持久化，当前硬编码 22/30/10） | 设置改默认端口 2222 → 新建会话表单预填 2222；已存会话不受影响 |

### 5.6 验收

| # | 粒度 | 验收 |
|---|------|------|
| 5.6.1 | 0.5h | 连接真实 Linux → 首次 host key 记录 → bash 能输入命令 → `pwd` 回显正确 → `vim` 能进 → tmux 能起 → 断网重连自动恢复 |

### 5.7 验收结果（2026-11-14）

- ✅ `cargo test --workspace`：43 passed / 0 failed（`live_ssh` 默认 ignored）
- ✅ 真实服务器 `192.168.1.95`：connect → 密码认证 → PTY → `pwd` 返回 `/home/ubuntu`
- ✅ GUI：双击 pw-test → 状态点变绿 / 标题「已连接」→ 键盘输入 `pwd` 正确
- ✅ GUI：`vim` 进入 Vim 9.1.241 → `:q!` 返回 shell；`tmux` 启动并显示状态栏
- ✅ 退出：Alt+F4 → SessionStore 保存 2 条成功
- ⏳ 真实断网重连未验证（指数退避/重试逻辑已通过认证失败场景触发验证）→ S10

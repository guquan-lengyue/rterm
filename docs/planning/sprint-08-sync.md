# Sprint 8 — 路径同步

> **状态**：✅ **已完成**（2026-11-17：7.1 核心 + 7.2 集成 + 7.3 真实 GUI E2E 全部通过）
> **前置**：[Sprint 6](./sprint-06-terminal-render.md)（Terminal PTY 输出 ✅）+ [Sprint 7](./sprint-07-sftp.md)（SFTP 导航；Perl ✅ 已完成）+ **[Sprint 7.5](./sprint-07.5-tech-debt.md)（技术债清偿；强制前置）** + [Sprint 4](./sprint-04-settings-io.md)（`Settings.sftp_sync_mode` 已持久化）
> **目标**：SyncBridge + PS1 正则 + ignore 防回环
> **定位**：rterm 独有杀手功能
>
> **当前实现（2026-11-17）**：
> - `SyncBridge`：四模式、bash/zsh/ANSI PS1 解析、跨 chunk、去重、`~` + remote home 解析、ignore FIFO(10)、pending SFTP nav 防回环、shell 转义（保留 `~`）、OSC 9 pwd 兜底
> - 集成：每个终端 Tab 一份 `Arc<Mutex<SyncBridge>>`；`TerminalView` 在 `handle_pty_event` 直接解析并 emit `TerminalCwdHint`；MainWindow 路由到同会话 SFTP / 终端；设置变更即时切换模式
> - 测试：`rterm-sync` 13 个单元测试；workspace 共 104 passed / 1 ignored；clippy 0 warning、fmt 通过
> - E2E：2026-11-17 真实 GUI 全量通过（见 7.3 验收记录）
>
> **S7.5 完成后的现状（已交接）**：
> - SFTP 导航已改为后台异步执行，但 `SftpNavigated` 仍在“列表加载成功后 emit”，语义与顺序不变；S8 不感知线程模型
> - `StatusBarMessage` 消费、关闭 Tab 状态点、设置默认值/data_dir 接线、删除/覆盖确认均已落地，S8 直接复用
> - **`AppEvent::TerminalData` 已在 S7.5 P3 作为死事件移除**（原实现只 emit、无订阅方）。S8 不要再新增全局高频 PTY 事件，直接在 `TerminalView::handle_pty_event` 内消费原始 bytes
>
> **已就绪的接入点**：
> - `TerminalView::handle_pty_event` 持有每个 PTY chunk 的 `bytes` → SyncBridge 直接在此调用，无需 EventBus 中转
> - `AppEvent::TerminalCwdHint` 保留，可用于 SyncBridge → 状态栏/其他 View 的轻量广播
> - **S6 已交付多 Tab 映射**：`MainWindow.tabs: Vec<TabEntry { id, session_id, title, kind, content }>`，S8 按 tab 隔离 SyncBridge 状态
> - **S7 已交付 SFTP 导航**：`SftpView` 每次成功导航（双击目录 / 面包屑 / 地址栏 / 刷新）都会 emit `SftpNavigated { tab_id, session_id, path }`；刷新也会触发，SyncBridge 需对比 `last_sftp` 去重，避免自激
> - **S7 已交付 Tab 内容枚举**：`TabContentView::{Terminal, Sftp}`；S8 通过 `MainWindow.tabs` 找到同 `session_id` 的终端 Tab
> - `SshSession::session_clone()` 可用于 SFTP 侧连接
>
> **S4 已就绪的接入点**：
> - 全局同步模式已在设置 → SFTP Tab 持久化为 `Settings.sftp_sync_mode`（`off` / `term_to_sftp` / `sftp_to_term` / `bidirectional`），本 Sprint 直接解析，不再新增全局配置项
> - `AppSettingsChanged` 已定义；模式变更后 SyncBridge 订阅刷新
> - `SftpNavigated` / `UserRequestedCwd` / `TerminalCwdHint` 事件已在 `event.rs` 定义

---

### 7.1 SyncBridge

| # | 粒度 | 文件 | 要写的 | 验收 |
|---|------|------|--------|------|
| 7.1.1 | 0.5h | `crates/rterm-sync/src/lib.rs` | `pub enum SyncMode { Off, TerminalToSftp, SftpToTerminal, Bidirectional }` + `pub enum CwdSource { Ps1, PwdFallback, SftpNav }` + `pub struct SyncBridge { mode, last_terminal, last_sftp, ignore_list }`；`SyncMode` 从 `Settings.sftp_sync_mode` 解析，缺省 Bidirectional | 能按设置构造模式；未知字符串回退 Bidirectional |
| 7.1.2 | 0.5h | `crates/rterm-sync/src/lib.rs` | PS1 正则（3 套模板）：bash default `w@h:p$` → 提取路径；zsh default；generic 带 ANSI 控制码剥离；`pub fn parse_ps1_prompt(ansi_bytes: &[u8]) -> Option<PathBuf>` | 给它 bash 真实 PS1 字节 → 提取 cwd |
| 7.1.3 | 0.5h | `crates/rterm-sync/src/lib.rs` | `pub fn on_terminal_output(&mut self, bytes: &[u8]) -> Option<PathBuf>` — PS1 正则 → 有则返回；无则标记 need_pwd；对比 last_terminal → 不同才返回 | 同一个 prompt 多次调用只返回一次 cwd |
| 7.1.4 | 0.5h | `crates/rterm-sync/src/lib.rs` | Pwd 兜底：on_terminal_output 返回 None 但 need_pwd=true → 后台注入命令 + 等待回复；超时 1s 忽略 | PS1 异常时 pwd 兜底能工作 |
| 7.1.5 | 0.5h | `crates/rterm-sync/src/lib.rs` | `pub fn on_sftp_navigated(&mut self, new_path: &Path) -> Option<InjectCommand>` — ignore_list 检查 → 有则返回 None；无则 `Some(format!("cd {}", shell_escape(new_path)))` + push ignore | ignore_list 生效；注入命令里路径被正确 shell 转义 |
| 7.1.6 | 0.25h | `crates/rterm-sync/src/lib.rs` | ignore_list 老化：VecDeque（max 10）FIFO；每次 push 自动 pop 最老 | 第 11 次 push 时第 1 个没了 |

### 7.2 集成

| # | 粒度 | 文件 | 要写的 | 验收 |
|---|------|------|--------|------|
| 7.2.1 | 0.5h | `crates/rterm-app/src/views/terminal.rs` + `main_window.rs` | 在 `TerminalView::handle_pty_event` 内、`engine.input_bytes(bytes)` 之后直接调用 `sync_bridge.on_terminal_output(bytes)`（不经过 EventBus）；若 Some(cwd) → emit `TerminalCwdHint` / `SftpNavigated`；SyncBridge 按 `tab_id` 隔离（映射来自 `MainWindow.tabs`） | 终端 cd /var/log → SFTP 自动跳 |
| 7.2.2 | 0.5h | `crates/rterm-app/src/views/main_window.rs` + SFTP View | SFTP 用户导航已在 `SftpView::navigate` 内 emit `SftpNavigated`；SyncBridge 处理后返回注入命令 → 找到同 `session_id` 的 `TabContentView::Terminal`，调用新增的 `TerminalView::inject_command`（内部 `PtyCommand::Write`） | SFTP 导航 → 终端自动 cd |

### 7.3 验收

| # | 粒度 | 验收 |
|---|------|------|
| 7.3.1 | 0.25h | Bidirectional 模式：`cd /var/log && pwd` → SFTP 跳 /var/log；SFTP 手动导航 /etc → 终端 prompt 变；**不**无限循环；在设置 → SFTP Tab 切 Off 模式 → 完全独立 |

> **交付状态**：7.1.1–7.1.6、7.2.1–7.2.2、7.3.1 全部完成。

### 7.3 验收记录（2026-11-17，真实 GUI / SSH 192.168.1.95）

| 场景 | 操作 | 结果 |
|------|------|------|
| 终端 → SFTP | 终端 `cd /var/log` | SFTP 远端自动跳 `/var/log`，列表 54 项 ✅ |
| SFTP → 终端 | SFTP 地址栏输入 `/etc` | 终端出现 `cd '/etc'`，prompt 变 `/etc` ✅ |
| 防回环 | 双向注入后继续操作 | 无重复注入、prompt 稳定 ✅ |
| Off 模式 | 设置切「关闭」并保存 | SFTP 跳 `/etc` 时终端仍为 `/home/ubuntu`；终端 `cd /tmp` 时 SFTP 仍停在 `/etc` ✅ |
| 即时生效 | 不重启应用，切回「双向同步」并保存 | SFTP 导航 `/var` → 终端立即 `cd '/var'` ✅ |
| 持久化 | 保存后读取 settings.json | `sftp_sync_mode` 在 `off` / `bidirectional` 间正确往返 ✅ |
| 初始化 | 打开 SFTP（远端 home） | 不反向注入 `cd`，终端 prompt 保持原状 ✅ |

自动化门禁：`cargo fmt --check` 通过、`cargo clippy --workspace --all-targets` 0 warning、`cargo test --workspace` 104 passed / 1 ignored（其中 `rterm-sync` 13 个）。

已知限制（移至 S10 打磨，不阻塞 S8）：
- SFTP 顶部「已连接 · 远端路径」标签停留在打开时的 home，不随导航刷新（纯展示）。
- pwd 兜底仅在 PS1 无法识别时启用，2 秒节流；对 `read -p` 等自定义提示可能误判，已通过 `$ # % ❯` 白名单收敛。
- Windows 下 GUI 自动化工具用 Unicode 注入时终端收不到字符（gpui 对 VK_PACKET 的处理），需用扫描码按键；与业务代码无关。

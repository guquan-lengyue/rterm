# Sprint 7 — SFTP 文件管理

> **状态**：⏳ **下一个**（S6 已完成，Tab 入口已就绪）
> **前置**：[Sprint 5](./sprint-05-ssh-terminal.md)（SSH Session + sftp 子系统；Perl ✅ 已完成）
> **目标**：SftpService + 双栏 View + 传输队列
>
> **S5 已就绪的接入点**：
> - `SshSession::session_clone()` 可用于在独立线程建立 `sftp()` 子系统
> - `FileTransfer*` 事件已定义
> - **Tab 入口已就绪（S6 5.4）**：`views/tab_bar.rs` + `MainWindow.tabs`（`TabEntry { id, session_id, view }`，`TabKind::Sftp` 已预留）；新增 Tab 直接复用 TabBar / Ctrl+T / Ctrl+W
> - **S6 遗留的改造点**：`MainWindow.tabs` 当前固定 `Entity<TerminalView>`，接入 SFTP 前需泛化为 `TabContentView::{Terminal, Sftp, Monitor}`（或在 MainWindow 内按 `TabKind` 维护多张表）；`TabBar` / 快捷键无需改动
>
> **S4 已就绪的接入点**：
> - `FileTransferStarted/Progress/Completed/Failed` 事件已在 `event.rs` 定义，本 Sprint 只需 emit
> - `Settings.sftp_sync_mode` 已持久化（S8 消费，本 Sprint 不读）
> - 传输进度弹窗 / chmod / 属性面板可复用 S4 自绘 Modal + TextInput 模式
> - `rfd` 文件选择器已引入，本地路径选择可直接用

---

### 6.1 SftpService

| # | 粒度 | 文件 | 要写的 | 验收 |
|---|------|------|--------|------|
| 6.1.1 | 0.5h | `crates/rterm-sftp/src/service.rs` | `pub struct SftpService` — 持有 ssh2::Session + sftp_subsystem | 能 new() |
| 6.1.2 | 0.5h | `crates/rterm-sftp/src/service.rs` | `pub fn listdir(&self, path: &Path) -> Result<Vec<FileEntry>>` — lstat + 解析 → FileEntry(name, size, mode, mtime, is_dir, symlink_target) | 远程 `/tmp` 能列出 |
| 6.1.3 | 0.5h | `crates/rterm-sftp/src/service.rs` | `pub fn mkdir` / `pub fn rmdir` / `pub fn unlink` / `pub fn rename` / `pub fn chmod` / `pub fn stat` | 目录创建删除正常 |
| 6.1.4 | 0.25h | `crates/rterm-sftp/src/service.rs` | 本地文件浏览（`walkdir::WalkDir`）+ 重命名 / 删除调系统 shell | 左栏能用 |

### 6.2 传输队列

| # | 粒度 | 文件 | 要写的 | 验收 |
|---|------|------|--------|------|
| 6.2.1 | 0.5h | `crates/rterm-sftp/src/queue.rs` | `pub struct TransferTask { pub id, direction, remote, local, total, transferred, state }` + `pub struct TransferQueue { tasks, concurrent }` | 能 enqueue / start |
| 6.2.2 | 1h | `crates/rterm-sftp/src/queue.rs` | 传输后台：tokio 并发（默认 2）+ 64KB chunk read/write + 实时 progress + 失败重试 2 次 | 1MB 文件 md5 往返一致 |
| 6.2.3 | 0.25h | `crates/rterm-sftp/src/queue.rs` | EventBus 广播：FileTransferStarted/Progress/Completed/Failed | 每个任务 4 次广播 |

### 6.3 Sftp View

| # | 粒度 | 文件 | 要写的 | 验收 |
|---|------|------|--------|------|
| 6.3.1 | 0.5h | `crates/rterm-app/src/views/sftp.rs` | 双栏 SplitView：左本地 + 右远程；各有地址栏 + 面包屑 + 刷新按钮 | 两栏能独立导航 |
| 6.3.2 | 0.5h | `crates/rterm-app/src/views/sftp.rs` | 文件列表：名字 / 大小 / 权限 / 时间；列头排序；目录图标 / 文件图标 | 列表清晰可点击 |
| 6.3.3 | 0.5h | `crates/rterm-app/src/views/sftp.rs` | 右键菜单：远程（上传/下载/mkdir/rename/delete/chmod/属性）；本地（同样但调 shell） | 菜单全功能 |
| 6.3.4 | 0.5h | `crates/rterm-app/src/views/sftp.rs` | 拖拽（gpui begin_drag + on_drop）：左→右=上传；右→左=下载 | 拖个文件能传 |
| 6.3.5 | 0.25h | `crates/rterm-app/src/views/sftp.rs` | 传输进度显示：状态栏进度条 + 弹窗（暂停/恢复/取消） | 传输中看到实时进度 |

### 6.4 SFTP Tab 打开

| # | 粒度 | 文件 | 要写的 | 验收 |
|---|------|------|--------|------|
| 6.4.1 | 0.5h | `crates/rterm-app/src/views/main_window.rs` + `views/tab_bar.rs` | 扩展 `TabEntry.view` 为多类型（Terminal / Sftp / Monitor）；SessionTree 右键「打开 SFTP」→ `MainWindow::open_sftp_tab`（复用 TabBar / Ctrl+W）；TabKind::Sftp 图标 📁 | 会话右键 → SFTP Tab 出现，可与终端 Tab 共存 |

### 6.5 验收

| # | 粒度 | 验收 |
|---|------|------|
| 6.5.1 | 0.25h | 新建远程目录 → 拖 50MB 文件上传 → md5 一致 → 右键重命名 → 下载 → md5 一致 → 删除 |

# Sprint 6 — SFTP 文件管理

> **状态**：—
> **前置**：[Sprint 4](./sprint-04-ssh-terminal.md)（SSH Session + sftp 子系统；Perl ✅ 已完成）
> **目标**：SftpService + 双栏 View + 传输队列

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
| 6.4.1 | 0.25h | `crates/rterm-app/src/main.rs` | TabKind::Sftp + SessionTree 右键 "打开 SFTP" → 新建 SFTP Tab + 复用 SSH Session | 会话右键 → SFTP Tab 出现 |

### 6.5 验收

| # | 粒度 | 验收 |
|---|------|------|
| 6.5.1 | 0.25h | 新建远程目录 → 拖 50MB 文件上传 → md5 一致 → 右键重命名 → 下载 → md5 一致 → 删除 |

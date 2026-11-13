# Sprint 3 — 会话 UI 层

> **状态**：—
> **前置**：[Sprint 1](./sprint-01-infrastructure.md)（AppState）+ [Sprint 2](./sprint-02-session-data.md)（SessionStore）
> **目标**：SessionTree View + 编辑对话框 + 设置界面 + 导入导出对话框
> **注意**：本 Sprint 的 View **全部 stub 后端连接**（还没有 SSH），只调 SessionStore

---

### 3.1 SessionTree View

| # | 粒度 | 文件 | 要写的 | 验收 |
|---|------|------|--------|------|
| 3.1.1 | 0.5h | `crates/rterm-app/src/views/session_tree.rs` | 替换 S1 占位为真实树形：按 `session.folder.split('/')` 建树；每个节点显示 session.name；图标区分文件夹 / 会话 | 构造 5 个 Session（2 个 folder）→ 树形显示 |
| 3.1.2 | 0.5h | `crates/rterm-app/src/views/session_tree.rs` | 右键菜单：新建 / 编辑 / 复制 / 删除 / 导出 / 打开终端（灰）/ 打开 SFTP（灰）—— gpui 的 MenuItem + `window.open_menu()` 模式 | 右键能弹菜单；点删除会调 SessionStore.delete + 广播 |
| 3.1.3 | 0.5h | `crates/rterm-app/src/views/session_tree.rs` | 双击会话 → `EventBus.emit(AppEvent::SessionConnectRequested { id })`；回车 → 同样 | debug log 能看到事件发出 |
| 3.1.4 | 0.25h | `crates/rterm-app/src/views/session_tree.rs` | `cx.subscribe` SessionCreated/Updated/Deleted → 自动 rebuild tree → `cx.notify()` | upsert 一个 Session → 左侧树立刻刷新 |

### 3.2 会话编辑对话框（Modal）

| # | 粒度 | 文件 | 要写的 | 验收 |
|---|------|------|--------|------|
| 3.2.1 | 0.5h | `crates/rterm-app/src/views/session_dialog.rs` | `pub fn show_edit_dialog(app: &mut App, session: Option<Session>)` — gpui Modal + 基础字段：名称 / 主机 / 端口(default 22) / 用户名 / 分组 folder | 对话框能弹出；空字段有 placeholder |
| 3.2.2 | 0.5h | `crates/rterm-app/src/views/session_dialog.rs` | 认证字段：下拉选 Password / PrivateKey / Agent；Password → 输入框 + 保存时调 `dpapi_encrypt`；PrivateKey → 文件选择 + 口令；Agent → `ssh-add -l` 枚举填充 | 三种认证方式都能填 + 保存 |
| 3.2.3 | 0.5h | `crates/rterm-app/src/views/session_dialog.rs` | 跳板机可选 + 全局默认 checkbox（不勾则继承 AppState.settings）| 勾上 default → 字段灰显 |
| 3.2.4 | 0.5h | `crates/rterm-app/src/views/session_dialog.rs` | 保存按钮：DPAPI 加密密码 → SessionStore.upsert → broadcast SessionUpdated → 关闭 Modal；取消按钮直接关 | 保存后 tree 刷新；密码字段 DPAPI 加密 |
| 3.2.5 | 0.5h | `crates/rterm-app/src/views/session_dialog.rs` | 测试连接按钮：`cx.background_spawn(async move { tcp connect() + ssh handshake() })` → 结果 broadcast `SessionTestResult` → 弹窗显示 | 对真实服务器能返回成功 / 失败原因 |

### 3.3 全局设置界面

| # | 粒度 | 文件 | 要写的 | 验收 |
|---|------|------|--------|------|
| 3.3.1 | 0.5h | `crates/rterm-app/src/views/settings.rs` | 主设置 View：左侧 TabList（常规 / SSH / 终端 / SFTP / 监控 / 高级）+ 右侧表单面板 | 能切 Tab |
| 3.3.2 | 0.5h | `crates/rterm-app/src/views/settings.rs` | 每个 Tab 的具体字段：DefaultPort, DefaultUsername, DefaultTheme, TerminalScrollbackLines, SftpSyncMode, MonitorInterval, etc. —— 绑定到 Settings 结构体 | 改一个字段 → 立刻 broadcast AppSettingsChanged + 保存 JSON |
| 3.3.3 | 0.5h | `crates/rterm-app/src/views/settings.rs` | 终端外观 Tab：主题下拉（4 套）+ 字体选择 + 字号 | 切主题 → 右侧预览面板立即变色 |

### 3.4 导入导出对话框

| # | 粒度 | 文件 | 要写的 | 验收 |
|---|------|------|--------|------|
| 3.4.1 | 0.5h | `crates/rterm-app/src/views/import_export.rs` | 导出：路径选择 + Scope 下拉（全部 / 文件夹 / 选择）+ 口令输入（可空） | 能调 `rterm_session::export_json` → 写文件 |
| 3.4.2 | 0.5h | `crates/rterm-app/src/views/import_export.rs` | 导入：路径选择 + ConflictPolicy 下拉 + 预览 diff（列出将要 added/skipped 的会话名）→ 执行 → 显示 ImportReport | 导出 → 清内存 → 导入 → 恢复完整 |

### 3.5 应用启动 / 退出

| # | 粒度 | 文件 | 要写的 | 验收 |
|---|------|------|--------|------|
| 3.5.1 | 0.25h | `crates/rterm-app/src/main.rs` | 启动时：`dirs::data_dir()` → `appdata_path` → `SessionStore::load()` + `Settings::load()`；Session store 空则新建默认 | 重启 app → 会话还在 |
| 3.5.2 | 0.25h | `crates/rterm-app/src/main.rs` | 退出时（on_app_quit）：SessionStore.save() + Settings.save() + 定时器 stop | Ctrl+C 退出后 JSON 更新时间戳是新的 |

### 3.6 验收

| # | 粒度 | 验收 |
|---|------|------|
| 3.6.1 | 0.25h | 启动 → 左侧 SessionTree（空）→ 右键新建 → 填 db-prod → 保存 → 出现树节点 → 退出重启 → 还在 → 右键导出 JSON → 清库 → 导入 → 恢复 |

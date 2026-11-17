# Sprint 10 — 稳定性收尾

> **状态**：进行中（S7.5 ✅、S8 ✅、S9 ✅ 已完成）
>
> **进度（2026-11-17）**：已完成 9.1b / 9.1c / 9.2 / 9.4 / 9.5 / 9.8 /
> 9.9 / 9.16 / 9.16b / 9.20 / 9.22（workspace 测试 146 passed / 1 ignored）。
> 待办：9.3 命令面板、9.6 OSC 52、9.7 vi 模式、9.11 诊断包、
> 9.13 自绘滚动、9.14 系统字体、9.17 / 9.18 / 9.19 / 9.21，
> 以及 9.1 / 9.10 / 9.15 回归与 9.12.1 全量 smoke test。
> **前置**：[Sprint 6](./sprint-06-terminal-render.md) ✅ + [Sprint 7](./sprint-07-sftp.md) + [Sprint 7.5](./sprint-07.5-tech-debt.md) ✅ + [Sprint 8](./sprint-08-sync.md) ✅ + [Sprint 9](./sprint-09-monitor.md)（Perl ✅ 已完成）
> **目标**：known_hosts 回归 + 错误映射 + 命令面板 + 全局快捷键 + 日志 + 备份 + 全量 smoke test
>
> **S3/S4 遗留（本 Sprint 收口）**：
> - 设置选择列表 / 导入预览超过上限无滚动（S4 遗留）→ 9.13
> - 终端字体仅 4 个预设，未枚举系统字体（S4 遗留）→ 9.14
> - 删除会话无二次确认 → 已由 S7.5 7.5.3.10 交付；9.15 改为回归 + 边界补漏
> - `data_dir` 接线 → 已由 S7.5 7.5.3.3 交付；9.16 只补 `terminal_log_dir` 接线
> - SessionStore 退出保存（S3）与 Settings 保存时原子写盘（S4）已完成 → 9.10 只需回归验证
>
> **S6 遗留（本 Sprint 收口）**：
> - 主题切换 / 高 DPI 鼠标 E2E 未覆盖 → 9.12.1 全量回归
> - 光标方块对比度、滚动条拖拽、双击选词 / 三击整行 → 9.17
> - 多类型 Tab（S7/S9 接入后）回归 → 9.12.1
>
> **S7.5 已提前收口（本 Sprint 只回归）**：
> - 会话 CRUD 即时落盘、损坏文件保护、单条解密容错、DPAPI 用户级、单实例锁 → 9.10 回归
> - 已知/变更主机密钥三选一 Modal → 9.1 回归
> - 删除/覆盖二次确认 → 9.15 回归
> - `data_dir` 接线 → 9.16 回归
>
> **S7.5 遗留（本 Sprint 收口）**：
> - hashed known_hosts 的旧指纹展示与变更替换 → 9.1b
> - Windows known_hosts 真正的原子替换（ReplaceFileW / 备份回滚）→ 9.1c
> - 修改 data_dir 后旧 sessions.json 迁移 → 9.16b
> - 长空闲 PTY 首键回显最坏 50ms → 9.19
>
> **S7 遗留（本 Sprint 收口）**：
> - 拖拽上传/下载、暂停/继续/取消 GUI E2E、空白区右键菜单 → 9.18
> - mkdir / rename / delete / chmod 逐项 E2E → 9.12.1
> - 文件列表虚拟化（大目录）→ 9.18
> - SFTP 本地文件系统操作仍在 UI 线程（远端已后台化）→ 9.18 评估
>
> **S8 遗留（本 Sprint 收口）**：
> - SFTP 顶部「已连接 · 远端路径」标签不随导航刷新 → 9.20
> - SyncBridge 注入的 `cd` 命令仍会在终端回显 → 9.21
> - pwd 兜底对自定义提示符（`read -p` 等）仍有误判可能，已用 `$ # % ❯` 白名单收敛 → 记录为已知限制，不新增任务
>
> **S9 遗留（本 Sprint 收口）**：
> - 关闭窗口时 bezel-gpui Windows 后端在 HWND 销毁后清理 OLE/TSF，打出 4 行 `window not found` + `无效的窗口句柄 (0x80040102 / 0x80070578)` 日志噪音（不影响关闭与退出保存，纯 idle 窗口关闭即可复现，与 Sprint 9 代码无关）→ 9.22

---

### 9.1 功能任务

| # | 粒度 | 文件 | 要写的 | 验收 |
|---|------|------|--------|------|
| 9.1 | 0.25h | `crates/rterm-ssh/src/known_hosts.rs` + `views/main_window.rs` | 已由 S7.5 7.5.4.1 交付首次连接三选一 Modal 与变更告警；本任务只回归 + 补错误文案边界 | 首次连接弹窗；拒绝后不写 known_hosts |
| 9.1b | 0.5h | `crates/rterm-ssh/src/known_hosts.rs` | hashed known_hosts（`\|1\|`）兼容：解析 salt/hash 匹配 host，展示旧指纹；AcceptAndSave 时删除匹配的 hashed 行，避免重复提示 | hashed known_hosts 变更后接受一次即可继续连接 |
| 9.1c | 0.5h | `crates/rterm-ssh/src/known_hosts.rs` | Windows 原子替换：优先 `ReplaceFileW`（或写临时文件 + 备份 + 失败回滚），消除“删旧再 rename”窗口 | 模拟 rename 失败时原 known_hosts 不丢 |
| 9.2 | 0.5h | `crates/rterm-ssh/src/errors.rs` | ssh2 error_code → 用户友好文案 map（auth_failed, timeout, host_key_changed, network_error）| 错误弹窗显示中文 |
| 9.3 | 1h | `crates/rterm-app/src/views/command_palette.rs` | Ctrl+P 命令面板：fuzzy-find 会话 + 设置入口 + 操作；↑/↓ 导航 Enter 执行 | 能搜 "db" 找到 db-prod |
| 9.4 | 0.5h | `crates/rterm-app/src/main.rs` | 全局快捷键：Ctrl+, 设置 / Ctrl+Shift+N 新会话 / F11 全屏 | 6 个快捷键生效 |
| 9.5 | 0.5h | `crates/rterm-terminal/src/engine.rs` | 会话日志：per-tab 开关；目录优先 `Settings.terminal_log_dir`，空则 `%APPDATA%\rterm\logs\`；可选时间戳前缀 | log 文件有内容；自定义目录生效 |
| 9.6 | 0.25h | `crates/rterm-terminal/src/engine.rs` | OSC 52 远程剪贴板支持 | 远程 tmux copy 能到本地剪贴板 |
| 9.7 | 0.25h | `crates/rterm-terminal/src/engine.rs` | vi 模式（alacritty_terminal 自带 ViMode 包装 + 入口）| Toggle 后 vi 导航可用 |
| 9.8 | 0.25h | `crates/rterm-session/src/openssh.rs` | S7.5 7.5.3.12 已修基础解析（顶层 Include / 多模式 Host / ProxyJump 端口 / Match 跳过）；本任务补 Include 递归 2 层 + Match 条件匹配 | 复杂 config 能解析 |
| 9.9 | 0.25h | `crates/rterm-session/src/backup.rs` | 24h 自动备份 + 保留 7 份轮转 | 手动触发能生成 backup 文件 |
| 9.10 | 0.25h | `crates/rterm-app/src/main.rs` | S7.5 7.5.1.1 已改为 CRUD 即时落盘 + 退出兜底；本任务回归：新建/编辑/删除后强杀 → 重启数据在；正常退出 → sessions/settings 都保留 | 强杀与正常退出两种路径数据一致 |
| 9.11 | 0.25h | `crates/rterm-app/src/main.rs` | 帮助 → 导出诊断包（logs + 版本 + 会话数 + OS）→ zip 导出 | 能生成 diag.zip |
| 9.13 | 0.5h | `crates/rterm-app/src/views/settings.rs` + `import_export.rs` | 自绘滚动容器（gpui 0.3 无原生滚动）：鼠标滚轮 + 可视区裁剪 + 滚动条；用于 S4 的选择会话列表 / 导入预览列表 | 24+ 条会话能滚动选择；预览 12+ 条能滚动 |
| 9.14 | 0.25h | `crates/rterm-app/src/views/settings.rs` | 字体选择改为枚举系统字体（DirectWrite / gpui 字体列表），保留现有 4 个预设作为 fallback | 能选到系统已装字体 |
| 9.15 | 0.15h | `crates/rterm-app/src/views/main_window.rs` + `session_tree.rs` | 已由 S7.5 7.5.3.10 交付删除二次确认；本任务只回归：删除后 emit SessionDeleted 并刷新树 | 点删除 → 确认 → 才真删 |
| 9.16 | 0.15h | `crates/rterm-app/src/main.rs` + `state.rs` | `data_dir` 已由 S7.5 7.5.3.3 接线；本任务只补 `terminal_log_dir` 供 9.5 使用，并回归自定义数据目录 | 改数据目录 → 重启后新路径生效 |
| 9.16b | 0.5h | `crates/rterm-app/src/state.rs` + `views/settings.rs` | data_dir 迁移：设置保存时若目录变化，提示并复制/移动 sessions.json（失败回滚），避免切换后看似丢会话 | 改 data_dir → 旧会话仍可见；失败保持原目录 |
| 9.17 | 0.5h | `crates/rterm-app/src/views/terminal.rs` | 终端交互打磨（S6 遗留）：方块光标改用反色/半透明提升对比度；滚动条支持鼠标拖拽；双击选中单词 / 三击选中整行 | 光标在任意配色下可辨认；滚动条可拖；双击选词 |
| 9.18 | 1h | `crates/rterm-app/src/views/sftp.rs` | SFTP 交互补全（S7 遗留）：Pane 间拖拽上传/下载；暂停/继续/取消 GUI E2E；空白区右键菜单；列表虚拟化（大目录不卡）；评估本地 fs 操作后台化 | 拖拽可传输；暂停后进度停止；1000 项目录滚动流畅 |
| 9.19 | 0.5h | `crates/rterm-ssh/src/pty.rs` | PTY 空闲唤醒：用 socket readiness / 可中断等待替代最长 50ms 轮询，恢复首键低延迟 | 空闲 1 分钟后按键回显 ≤ 当前活跃路径 + 10ms |
| 9.20 | 0.25h | `crates/rterm-app/src/views/sftp.rs` | SFTP 顶部状态标签随 `SftpNavigated` 刷新远端路径（当前停留在打开时的 home） | 任意导航后标签与地址栏一致 |
| 9.21 | 0.5h | `crates/rterm-app/src/views/terminal.rs` + `rterm-sync` | 评估注入命令回显抑制：标记内部命令，输入行不上屏（或短暂高亮/淡出），不影响真实用户输入 | 双向同步时不出现可见的 `cd '/path'` 行 |
| 9.22 | 0.25h | `crates/rterm-app/src/main.rs`（或 vendored gpui 评估） | 关闭窗口的 teardown 日志噪音：`App::update_window_erased` 在 HWND 销毁后仍被调用（`window not found`），随后 OLE/TSF 清理报 `0x80040102` / `0x80070578`。先评估在退出路径提前 `deactivate IME / 关闭 a11y` 是否消除；否则降级为 `log` filter 静音（保持功能日志完整），必要时给 bezel-gpui 上游提 issue | 关闭窗口日志不再出现 `window not found` / `无效的窗口句柄`；会话仍正常落盘 |

### 9.12 全量 Smoke Test

| # | 粒度 | 验收 |
|---|------|------|
| 9.12.1 | 1h | 逐个重跑：Sprint 3 会话管理端到端 + Sprint 4 设置持久化/导入导出 + Sprint 5 SSH+终端核心 + Sprint 6 终端渲染（含 vim/htop、Shift+PageUp、选择复制、切主题、字号缩放、高 DPI）+ Sprint 7 SFTP + Sprint 8 路径同步 + Sprint 9 监控；多类型 Tab 共存回归 |

**✅ 全部 Smoke Test 通过 → 进入 Sprint 11 Release 打磨**

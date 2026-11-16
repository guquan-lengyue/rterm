# Sprint 10 — 稳定性收尾

> **状态**：待开始（前置 S6–S9 完成）
> **前置**：[Sprint 6](./sprint-06-terminal-render.md) + [Sprint 7](./sprint-07-sftp.md) + [Sprint 8](./sprint-08-sync.md) + [Sprint 9](./sprint-09-monitor.md) 全部（Perl ✅ 已完成）
> **目标**：known_hosts 持久化 + 错误映射 + 命令面板 + 全局快捷键 + 日志 + 备份 + 全量 smoke test
>
> **S3/S4 遗留（本 Sprint 收口）**：
> - 设置选择列表 / 导入预览超过上限无滚动（S4 遗留）→ 9.13
> - 终端字体仅 4 个预设，未枚举系统字体（S4 遗留）→ 9.14
> - 删除会话无二次确认（S3 遗留）→ 9.15
> - `terminal_log_dir` / `data_dir` 仅存储未接线（S4 遗留）→ 9.16
> - SessionStore 退出保存（S3）与 Settings 保存时原子写盘（S4）已完成 → 9.10 只需回归验证

---

### 9.1 功能任务

| # | 粒度 | 文件 | 要写的 | 验收 |
|---|------|------|--------|------|
| 9.1 | 0.5h | `crates/rterm-ssh/src/known_hosts.rs` + `views/main_window.rs` | S5 已完成 TOFU 记录 / 变更检测；本任务补首次连接三选一 Modal（仅本次 / 保存 / 拒绝）与变更告警 UX | 首次连接弹窗；拒绝后不写 known_hosts |
| 9.2 | 0.5h | `crates/rterm-ssh/src/errors.rs` | ssh2 error_code → 用户友好文案 map（auth_failed, timeout, host_key_changed, network_error）| 错误弹窗显示中文 |
| 9.3 | 1h | `crates/rterm-app/src/views/command_palette.rs` | Ctrl+P 命令面板：fuzzy-find 会话 + 设置入口 + 操作；↑/↓ 导航 Enter 执行 | 能搜 "db" 找到 db-prod |
| 9.4 | 0.5h | `crates/rterm-app/src/main.rs` | 全局快捷键：Ctrl+, 设置 / Ctrl+Shift+N 新会话 / F11 全屏 | 6 个快捷键生效 |
| 9.5 | 0.5h | `crates/rterm-terminal/src/engine.rs` | 会话日志：per-tab 开关；目录优先 `Settings.terminal_log_dir`，空则 `%APPDATA%\rterm\logs\`；可选时间戳前缀 | log 文件有内容；自定义目录生效 |
| 9.6 | 0.25h | `crates/rterm-terminal/src/engine.rs` | OSC 52 远程剪贴板支持 | 远程 tmux copy 能到本地剪贴板 |
| 9.7 | 0.25h | `crates/rterm-terminal/src/engine.rs` | vi 模式（alacritty_terminal 自带 ViMode 包装 + 入口）| Toggle 后 vi 导航可用 |
| 9.8 | 0.25h | `crates/rterm-session/src/openssh.rs` | Include 递归 2 层 + Match 语法 | 复杂 config 能解析 |
| 9.9 | 0.25h | `crates/rterm-session/src/backup.rs` | 24h 自动备份 + 保留 7 份轮转 | 手动触发能生成 backup 文件 |
| 9.10 | 0.25h | `crates/rterm-app/src/main.rs` | ✅ 已由 S3（SessionStore 退出保存）+ S4（Settings 保存时原子写盘）落地；本任务只做回归：退出 → 重启 → sessions/settings 都保留 | 退出后 JSON mtime 更新 |
| 9.11 | 0.25h | `crates/rterm-app/src/main.rs` | 帮助 → 导出诊断包（logs + 版本 + 会话数 + OS）→ zip 导出 | 能生成 diag.zip |
| 9.13 | 0.5h | `crates/rterm-app/src/views/settings.rs` + `import_export.rs` | 自绘滚动容器（gpui 0.3 无原生滚动）：鼠标滚轮 + 可视区裁剪 + 滚动条；用于 S4 的选择会话列表 / 导入预览列表 | 24+ 条会话能滚动选择；预览 12+ 条能滚动 |
| 9.14 | 0.25h | `crates/rterm-app/src/views/settings.rs` | 字体选择改为枚举系统字体（DirectWrite / gpui 字体列表），保留现有 4 个预设作为 fallback | 能选到系统已装字体 |
| 9.15 | 0.25h | `crates/rterm-app/src/views/main_window.rs` + `session_tree.rs` | 删除会话二次确认 Modal（S3 遗留）；删除后 emit SessionDeleted 并刷新树 | 点删除 → 确认 → 才真删 |
| 9.16 | 0.25h | `crates/rterm-app/src/main.rs` + `state.rs` | `Settings.data_dir` 接线：sessions.json / settings.json / 备份统一到自定义目录；`terminal_log_dir` 供 9.5 使用 | 改数据目录 → 重启后新路径生效 |

### 9.12 全量 Smoke Test

| # | 粒度 | 验收 |
|---|------|------|
| 9.12.1 | 1h | 逐个重跑：Sprint 3 会话管理端到端 + Sprint 4 设置持久化/导入导出 + Sprint 5 SSH+终端核心 + Sprint 6 终端渲染 + Sprint 7 SFTP + Sprint 8 路径同步 + Sprint 9 监控 |

**✅ 全部 Smoke Test 通过 → 进入 Sprint 11 Release 打磨**

# Sprint 10 — 稳定性收尾

> **状态**：—
> **前置**：[Sprint 6](./sprint-06-terminal-render.md) + [Sprint 7](./sprint-07-sftp.md) + [Sprint 8](./sprint-08-sync.md) + [Sprint 9](./sprint-09-monitor.md) 全部（Perl ✅ 已完成）
> **目标**：known_hosts 持久化 + 错误映射 + 命令面板 + 全局快捷键 + 日志 + 备份 + 全量 smoke test

---

### 9.1 功能任务

| # | 粒度 | 文件 | 要写的 | 验收 |
|---|------|------|--------|------|
| 9.1 | 0.5h | `crates/rterm-ssh/src/known_hosts.rs` 完成 | known_hosts 持久化 OpenSSH 兼容格式 | 重启 app → known_hosts 保留 |
| 9.2 | 0.5h | `crates/rterm-ssh/src/errors.rs` | ssh2 error_code → 用户友好文案 map（auth_failed, timeout, host_key_changed, network_error）| 错误弹窗显示中文 |
| 9.3 | 1h | `crates/rterm-app/src/views/command_palette.rs` | Ctrl+P 命令面板：fuzzy-find 会话 + 设置入口 + 操作；↑/↓ 导航 Enter 执行 | 能搜 "db" 找到 db-prod |
| 9.4 | 0.5h | `crates/rterm-app/src/main.rs` | 全局快捷键：Ctrl+, 设置 / Ctrl+Shift+N 新会话 / F11 全屏 | 6 个快捷键生效 |
| 9.5 | 0.5h | `crates/rterm-terminal/src/engine.rs` | 会话日志：per-tab 开关；默认 `%APPDATA%\rterm\logs\`；可选时间戳前缀 | log 文件有内容 |
| 9.6 | 0.25h | `crates/rterm-terminal/src/engine.rs` | OSC 52 远程剪贴板支持 | 远程 tmux copy 能到本地剪贴板 |
| 9.7 | 0.25h | `crates/rterm-terminal/src/engine.rs` | vi 模式（alacritty_terminal 自带 ViMode 包装 + 入口）| Toggle 后 vi 导航可用 |
| 9.8 | 0.25h | `crates/rterm-session/src/openssh.rs` | Include 递归 2 层 + Match 语法 | 复杂 config 能解析 |
| 9.9 | 0.25h | `crates/rterm-session/src/backup.rs` | 24h 自动备份 + 保留 7 份轮转 | 手动触发能生成 backup 文件 |
| 9.10 | 0.25h | `crates/rterm-app/src/main.rs` | 退出钩子（WM_CLOSE + Ctrl+C）→ 持久化 SessionStore + Settings | 退出后 JSON mtime 更新 |
| 9.11 | 0.25h | `crates/rterm-app/src/main.rs` | 帮助 → 导出诊断包（logs + 版本 + 会话数 + OS）→ zip 导出 | 能生成 diag.zip |

### 9.12 全量 Smoke Test

| # | 粒度 | 验收 |
|---|------|------|
| 9.12.1 | 1h | 逐个重跑：S3.6（会话管理端到端）+ S4.6（SSH+终端核心）+ S5.6（终端渲染）+ S6.5（SFTP）+ S7.3（路径同步）+ S8.3（监控） |

**✅ 全部 Smoke Test 通过 → 进入 Sprint 10 Release 打磨**

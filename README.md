# rterm

一个用 **Rust + bezel-gpui** 构建的 Windows 远程终端工作台：SSH 终端、SFTP 文件管理、
终端 ↔ SFTP 路径双向同步、远程性能监控，全部在同一个窗口里以多标签页共存。

> **状态：v1.0 发布候选**。Windows 已完成全量回归；macOS / Linux 源码可构建（未验证发布）。

---

## 功能一览

| 模块 | 能力 |
|------|------|
| 会话管理 | 会话树 / 分组 / DPAPI 加密保存密码 / 搜索 / 导入导出（DPAPI 或 AES-GCM 口令）/ OpenSSH config 导入（含 Include 递归与 Match 条件）/ 自动备份（24h，保留 7 份） |
| SSH 终端 | xterm-256color、主题切换、字号缩放、滚动缓冲、选中即复制、右键粘贴（bracketed paste）、OSC 52 远程复制、vi 导航模式、会话日志（Ctrl+Shift+L） |
| SFTP | 双栏浏览、跨面板拖拽上传/下载、传输队列（暂停/继续/取消）、1000 项目录流畅滚动、右键菜单（重命名/删除/chmod/属性）、本地目录后台读取 |
| 路径同步 | 终端变更目录 → SFTP 跟随；SFTP 导航 → 终端 `cd`（四种模式，注入命令不回显） |
| 性能监控 | CPU / 内存 / Swap / 磁盘 / 网络 / Load 实时折线图、Top 进程（/proc 直读）、阈值告警、Kill 进程 |
| 稳定性 | 主机密钥三选一确认（含 hashed known_hosts）、自动重连（指数退避）、命令面板（Ctrl+P）、诊断包导出、崩溃安全的数据落盘 |

## 安装

### 安装包（推荐）

1. 下载 `rterm-1.0.0-setup.exe`
2. 双击安装（默认安装到 `Program Files\rterm`），可选桌面快捷方式

### 便携版

直接运行 `rterm-app.exe` 即可，无需安装。

## 首次使用

1. 左侧「＋ 新建会话」→ 填写名称 / 主机 / 端口 / 用户名 / 密码或私钥
2. 双击会话 → 打开终端
3. 首次连接会提示主机指纹：确认后「接受并保存」写入 known_hosts
4. 会话右键「打开 SFTP」「打开监控」可新建对应标签页

数据默认保存在 `%APPDATA%\rterm\`（会话、设置、日志、备份），可在
「设置 → 高级」中修改数据目录（切换时会自动迁移 sessions.json）。

## 快捷键

| 快捷键 | 功能 |
|--------|------|
| `Ctrl+T` / `Ctrl+Tab` / `Ctrl+W` | 新标签 / 切换标签 / 关闭标签 |
| `Ctrl+Shift+N` | 新建会话 |
| `Ctrl+,` | 设置 |
| `Ctrl+P` | 命令面板（搜索会话与操作） |
| `Ctrl+Shift+L` | 开关当前终端会话日志 |
| `Ctrl+Shift+Space` | 进入 / 退出 vi 导航模式（hjkl / w b / 0 $ / g G，Esc 退出） |
| `Shift+PageUp` / `Shift+PageDown` | 终端回滚 |
| `Ctrl+=` / `Ctrl+-` / `Ctrl+0` | 字号放大 / 缩小 / 复位 |
| `F11` | 全屏 |

## 常见问题

- **启动后立即退出？** 请从资源管理器 / 快捷方式启动。通过某些自动化环境（沙箱进程）
  启动时，DPAPI 凭据读取会被系统拒绝，程序会提示后退出。
- **密码保存在哪里？** 密码经 Windows DPAPI 加密后写入 `sessions.json`，仅当前 Windows
  用户可解密；导出时可选 DPAPI 或独立口令（PBKDF2 + AES-GCM）。
- **换电脑后会话丢失？** 用「导出」生成加密备份，在新机器「导入」即可。
- **命令面板搜不到中文会话？** 当前搜索框只支持 ASCII 输入（无 IME 组合输入），
  可先用拼音或英文命名。
- **终端出现重复的 `cd` 行？** 仅在远端提示符非常规（readline 补全、多行 PS2）时可能发生，
  正常情况下路径同步的注入命令不会回显。

## 故障排查

- 按 `Ctrl+P` 打开命令面板，执行「导出诊断包…」：打包版本、系统、设置摘要与最近日志
- 关闭窗口时的 `window not found` 日志属于已知的 gpui Windows 后端清理噪音，已被日志过滤器静音

## 从源码构建

依赖：Rust 1.85+（MSVC）、VS 2022 Build Tools（C++ workload）、CMake 3+、Strawberry Perl。

```powershell
. .\scripts\msvc-env.ps1          # 每次新 shell 先加载 MSVC 环境
cargo build -p rterm-app          # 调试版
cargo build --release -p rterm-app  # 发布版
.\target\release\rterm-app.exe
```

安装包：`iscc installer\rterm.iss`（需 Inno Setup 6，产物在 `dist\`）。

## 项目结构

```
crates/
├── rterm-app      # 入口 + gpui 主窗口 + AppState + EventBus + 各 View
├── rterm-session  # Session 模型 + CRUD + 持久化 + 导入导出 + 自动备份
├── rterm-ssh      # ssh2 封装 + 重连 + keepalive + known_hosts + 错误分类
├── rterm-terminal # alacritty_terminal glue + 会话日志
├── rterm-sftp     # SFTP 浏览 + 传输队列
├── rterm-monitor  # 远程采集 + 时序聚合 + 告警
├── rterm-sync     # 终端 ↔ SFTP 路径同步
└── rterm-crypto   # DPAPI + AES-GCM 加解密
```

设计与规划文档见 [docs/README.md](docs/README.md)；开发路线与 Sprint 状态见
[docs/planning/README.md](docs/planning/README.md)。

---

## 参考

- [bezel-gpui on crates.io](https://crates.io/crates/bezel-gpui) — Zed gpui 的活跃 fork，Windows/macOS/Linux 三平台构建
- [alacritty_terminal](https://docs.rs/alacritty_terminal/latest/alacritty_terminal/) — 终端仿真核心库
- [ssh2 crate](https://docs.rs/ssh2/latest/ssh2/) — libssh2 bindings

## 许可证

MIT

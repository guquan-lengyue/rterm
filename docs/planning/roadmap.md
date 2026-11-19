# rterm — 开发路线图 (Roadmap)

> **更细任务拆分 + 当前状态见 [planning/README.md](./README.md)**（每个 Sprint 一个独立文件，任务粒度 0.25–1h）。
> 本文档保留里程碑概览和高层任务分类，两份文档互为补充。

## 里程碑概览

| Phase | 交付 | 预估 | 关键依赖 |
|-------|------|------|----------|
| **P0 环境 & 骨架** | gpui 空窗口可运行 + 所有 crate 骨架 + MSVC 环境脚本 | ✅ 已完成 | VS Build Tools、perl、bezel-gpui 0.3 |
| **P1 会话管理** | 会话树 View + SessionStore CRUD + 持久化 JSON + 导入导出 + 全局设置界面 | ✅ 已完成（S2–S4） | serde / uuid / rterm-crypto |
| **P2 SSH 连接 & 终端** | ssh2 Session 封装 + PTY + alacritty_terminal glue + gpui Canvas 渲染 + 基础主题 | ✅ 已完成（S5 + S6） | ssh2 vendored-openssl / alacritty_terminal |
| **P3 SFTP 文件管理** | SFTP 双栏浏览 + 传输队列 + 进度条 + 拖拽 | ✅ 已完成（S7 + S10 收口） | ssh2 Sftp / walkdir |
| **P4 路径同步** | SyncBridge + PS1 正则 + ignore 防回环 + 双向开关 | ✅ 已完成（S8） | ssh2 / 手写解析（无 regex 依赖） |
| **P5 远程性能监控** | MonitorCollector + 折线图 + Top 进程 + 阈值告警 | ✅ 已完成（S9） | tokio 定时 + Canvas 折线图 |
| **P6 稳定性收尾** | 主机密钥管理 + 重连策略 + 命令面板 + 快捷键 + 日志导出 + 自动备份 | ✅ 已完成（S10） | winreg / dirs |
| **P7 打磨 & v1.0** | 全量回归 + Release 构建 + 安装包 + 文档 + 压力测试 | 🚧 进行中（S11） | — |
| **P8 v1.1 增强** | 终端 Tab 内嵌 SFTP 侧栏 + 路径跟随 | 📋 待开始（S12） | S8 + S10 + S11 |
| **P9 v1.2 会话体验** | 分组/图标/顶部菜单栏/菜单重构 + SSH 跳板代理与端口转发 + 导入导出完善 | ✅ 代码完成（S13，待真实远端 smoke） | S2–S5 + S12 |

**注**：预估时间相对粗略，按全职进度。P2（终端）是最具技术挑战的 Phase，也是整个 MVP 的核心。

---

## Phase 0 — 环境 & 骨架 ✅

### 已交付

- [x] VS 2022 Build Tools + C++ workload（MSVC 19.51）
- [x] MSVC 环境脚本 `scripts/msvc-env.ps1`
- [x] CMake 4.3.1（VS 自带）
- [x] Cargo proxy 配置（`~/.cargo/config.toml`）
- [x] Cargo workspace（8 个 crate 骨架 + bezel-gpui 依赖）
- [x] gpui 空窗口程序编译通过（`rterm-app.exe` 36MB）
- [x] `.gitignore` 整理完成
- [x] 架构文档 01–04

### 遗留（P2 前补）

- [ ] Strawberry Perl 装完后：启用 `rterm-app/Cargo.toml` 里被注释的 `rterm-*` 依赖，完整 workspace `cargo build` 验证
- [ ] 写 smoke test：最小 ssh2 Session 连接一个已知服务器，确认 vendored-openssl 工作

---

## Phase 1 — 会话管理 ✅（S2–S4）

### 任务清单

#### 1.1 rterm-crypto 实现 ✅（S1）
- [x] `dpapi_encrypt` / `dpapi_decrypt`（Windows）
- [x] `aesgcm_encrypt` / `aesgcm_decrypt`（PBKDF2 + 随机 salt + 非重用 iv）
- [x] 单元测试：加密→解密往返；错误口令解密抛错

#### 1.2 rterm-session 实现 ✅（S2）
- [x] `Session` 结构体 + serde（加密字段序列化为 `{ version, base64(data) }`）
- [x] `SessionStore::load` / `save`（JSON 文件）
- [x] `SessionStore` CRUD 方法（list / get / upsert / delete）
- [x] 文件夹分组逻辑（按 `session.folder.split('/')` 建树）
- [x] `ConflictPolicy`（覆盖 / 跳过 / 重命名）
- [x] `ExportScope`（全部 / 文件夹 / 选择）
- [x] 导入导出 JSON schema（带 `version` 字段）
- [ ] 自动备份逻辑（24h 定时器 + 保留 7 份）→ S10 9.9

#### 1.3 rterm-app 设置与持久化 ✅（S3 + S4）
- [x] `dirs::data_dir()` → `%APPDATA%\rterm\` 路径
- [x] `Settings` 结构体 + 默认值（`serde(default)` 向后兼容）
- [x] `AppState { session_store, settings, event_bus, theme, startup_error }`
- [x] 启动时自动 load sessions.json + settings.json
- [x] 退出时自动 save SessionStore（S3）；Settings 保存时原子写盘（S4）

#### 1.4 SessionTree View（gpui）——S3 已完成主体
- [x] 左侧 `SessionTreeView`：树形（folder → session），带图标
- [x] 右键菜单：新建 / 编辑 / 删除 / 导入 / 导出（S3 + S4）
- [x] 右键菜单：打开终端 / 打开 SFTP（复制未排期）
- [x] 双击会话 → 广播 `AppEvent::SessionConnectRequested`
- [x] 会话状态图标（未连接 / 连接中 / 在线 / 失败）+ 关闭最后 Tab 复位
- [ ] 收藏夹区域（顶部）→ 未排期

#### 1.5 Session 编辑对话框——S3 已完成主体
- [x] gpui Modal 风格表单（自绘 Dialog）
- [x] 基础字段：名称 / 主机 / 端口 / 用户名 / 分组
- [x] 认证字段：Password / PrivateKey / Agent 三选一
- [x] Password：DPAPI 加密存储（编辑时掩码、未修改沿用原密码）
- [x] PrivateKey：路径 + 口令（可选，可换 `rfd` 文件选择器）
- [ ] Agent：读取 `ssh-add -l` 输出枚举 key → S5 5.1.2 先做认证
- [ ] 跳板机：模型已支持 `proxy_jump`，连接链待 S5 接
- [x] 连接测试按钮：后台异步 TCP（S5 升级为 SSH 握手）
- [x] 全局默认配置界面：端口/用户名/keepalive/timeout 已接入新建会话；data_dir 已接入 sessions/导出路径

#### 1.6 导入导出对话框 ✅（S4，OpenSSH 导入 S2）
- [x] 导出：`rfd` 路径选择 + 范围（全部 / 文件夹 / 多选）+ DPAPI 或 AES-GCM 口令
- [x] 导入：路径选择 + 口令 + 冲突策略 + 预览 diff + ImportReport
- [x] OpenSSH config 导入（解析 `Host` / `HostName` / `Port` / `User` / `IdentityFile` / `ProxyJump`）

#### 1.7 全局设置界面 ✅（S4）
- [x] 7 个 Tab：常规 / SSH 默认值 / 终端外观 / SFTP / 监控 / 高级 / 导入导出
- [x] 即时预览（主题全应用换肤、字号联动预览）
- [x] 设置变更 → `AppSettingsChanged` / `AppThemeChanged`（消费方 S6 起接入）

---

## Phase 2 — SSH 连接 & 终端 ✅（S5 + S6）

### 前置条件
- [x] P0（gpui 空窗口）
- [x] perl 可用（vendored-openssl 编译，S5 5.0 已验证）
- [x] P1（SessionStore + Session 编辑，S2–S4）
- [x] S5（SSH + PTY + TerminalEngine + 文本 TerminalView）

### 任务清单

#### 2.1 rterm-ssh 核心 ✅（S5）
- [x] `SshSession::connect(session: &Session)`：密码 / 密钥 / agent（跳板机链式待补）
- [x] 主机密钥校验：未知/变更三选一 Modal（接受并保存 / 仅本次 / 拒绝）；known_hosts 原子写 + 进程内串行
- [x] KeepAlive：PTY IO 循环调用 `keepalive_send()`
- [x] 连接状态机：Disconnected → Connecting → Connected → Reconnecting（退避）
- [x] 重连策略：5 次指数退避 + 全部失败广播 `SessionDisconnected`

#### 2.2 PTY 管理 ✅（S5）
- [x] `SshSession::open_pty(cols, rows) → ssh2::Channel`
- [x] `resize_pty(channel, cols, rows)`
- [x] PTY 输出字节流读取（后台线程 non-blocking 轮询）
- [x] 字节流通过 futures mpsc 发回主线程

#### 2.3 rterm-terminal glue ✅（S5）
- [x] `TerminalEngine::new(cols, rows, scrollback_lines)`：Term<SizeInfo>
- [x] `input_bytes()`：喂 PTY 字节 → vte::Parser → Term
- [x] `input_key()`：TerminalKey → xterm 字节序列
- [x] `resize()`：Term 同步
- [x] `TerminalKey` enum 映射（gpui KeyDown → xterm 序列）
- [x] PS1 解析（bash / zsh / generic + ANSI 剥离 + pwd 兜底）→ S8
- [ ] 终端输入回显抑制（SyncBridge 注入 cd 时）→ S8 未做，注入命令仍可见，S10 评估

#### 2.4 Terminal View（gpui Canvas 渲染）✅（S6）
- [x] S5 文本 placeholder（实时显示 PTY 输出）
- [x] Canvas 渲染 `Grid<Cell>`：逐 Cell 画文字 + 背景矩形
- [x] 光标渲染（方块 / 下划线 / 竖线，blink 动画）
- [x] 滚动缓冲绘制 + 滚动条 + 鼠标滚轮
- [x] 鼠标选择（按住左键拖动画选区 → 复制系统剪贴板）
- [x] 右键粘贴（从系统剪贴板读 → PTY 注入）
- [x] 键盘事件：终端按键 → TerminalEngine.input_key → PTY
- [x] 主题切换（fg / bg / 16 ANSI + 256 + 真彩色）
- [x] 字体 + Nerd Font 字形支持（gpui 自带 DirectWrite）

#### 2.5 标签页框架 ✅（S6）
- [x] 标签模型：`MainWindow.tabs: Vec<TabEntry>` + active_index（`TabRegistry` 已移除）
- [x] TabBar View：水平标签 + 关闭按钮 + 关闭菜单
- [x] 标签 kinds：Terminal / Sftp / Monitor
- [x] 快捷键：Ctrl+T 新标签 / Ctrl+Tab 切换 / Ctrl+W 关闭

---

## Phase 3 — SFTP 文件管理 ✅（S7 + S10 收口，2026-11-17）

> **S7.5 技术债清偿已完成（2026-11-16）**：[sprint-07.5-tech-debt.md](./sprint-07.5-tech-debt.md) 已完成数据安全、线程生命周期、状态一致性与主机密钥收口；自动化门禁 92 passed / 1 ignored、clippy 0 warning、fmt 通过。Phase 4 可开始。
> **S10 9.18 已收口**：跨面板拖拽传输、暂停/继续/取消 GUI、空白区右键菜单、列表虚拟化、本地 listdir 后台化；真实远端 E2E 见 S11 11.1。

### 前置条件
- [x] P1（SessionStore）
- [x] P2（SSH 连接）

### 任务清单

#### 3.1 rterm-sftp 核心 ✅
- [x] `SftpService::sftp()`：基于 SSH Session 的 sftp 子系统
- [x] `listdir(path) → Vec<FileEntry>`：解析 stat / lstat
- [x] `mkdir(path)` / `rmdir(path)` / `unlink(path)` / `rename(from, to)`
- [x] `chmod(path, mode)` / `stat(path)`
- [x] 本地文件浏览（std::fs 实现，未用 walkdir/shell；功能等价）

#### 3.2 传输队列 ✅
- [x] `TransferQueue`：并发数可配（默认 2）
- [x] 上传：分块读取本地文件 → SFTP create
- [x] 下载：SFTP open → 分块写入本地
- [x] 每个任务进度实时更新（~100ms 节流）
- [ ] 速度（bytes / sec）显示
- [x] 暂停 / 恢复 / 取消 / 失败重试（代码完成；GUI E2E 见 S11 11.1）

#### 3.3 Sftp View（gpui）✅
- [x] 双栏 Split View（左本地，右远程）
- [x] 地址栏 + 面包屑 + 刷新
- [ ] 前进 / 后退
- [x] 文件列表图标（目录 / 文件 / 符号链接）
- [x] 拖拽上传 / 下载（S10 9.18，文件级；目录拖拽转 v1.1）
- [x] 列表虚拟化（S10 9.18，只渲染可视行）
- [x] 右键菜单：上传 / 下载 / 重命名 / 删除 / chmod / 属性（新建目录在顶部工具栏）；空白区菜单（S10）
- [x] 传输进度条（底部传输面板）

---

## Phase 4 — 路径同步（✅ 完成，2026-11-17）

> 交付记录见 [CHANGELOG.md](./CHANGELOG.md) Sprint 8；验收记录见 [sprint-08-sync.md](./sprint-08-sync.md)。

### 前置条件
- [x] P2（Terminal 输出字节流 + PS1 解析）
- [x] P3（SFTP 文件浏览 + 导航事件）
- [x] S7.5 技术债清偿（S8 前强制门禁）

### 任务清单

#### 4.1 rterm-sync SyncBridge
- [x] `SyncBridge::on_terminal_output(bytes) → Option<DetectedCwd>`
- [x] 3 套 PS1 解析（bash `w@h:p$`、zsh `w@h p %`、ANSI 剥离 + 跨 chunk）
- [x] 兜底 pwd 触发：正则匹配失败 → 注入 OSC 9 标记的 pwd 探测 → 2s 节流忽略
- [x] `SyncBridge::on_sftp_navigated(path) → Option<String>`（注入 `cd`）
- [x] ignore_list（VecDeque, max 10）+ pending SFTP nav + 初始 home 标记：防止双向回环
- [x] 方向开关（off / term→sftp / sftp→term / bidirectional），设置变更即时生效
- [ ] per-session 可覆盖默认方向（未排期；全局设置已满足当前需求）

#### 4.2 集成到 Terminal + SFTP
- [x] TerminalView 每次 PTY 字节流喂 Term 后 → SyncBridge.on_terminal_output
- [x] SFTP View 每次导航（双击 / 面包屑 / 地址栏）→ `SftpNavigated`
- [x] SyncBridge.on_sftp_navigated → 组装注入命令 → TerminalView.inject_command（shell 转义）
- [x] 回显抑制：注入命令的回显由 engine 在输出流中吞掉（S10 9.21）

---

## Phase 5 — 远程性能监控 ✅（S9，2026-11-17）

> 交付记录见 [CHANGELOG.md](./CHANGELOG.md)（Sprint 9）与 [sprint-09-monitor.md](./sprint-09-monitor.md)；9.12.1 多 Tab 回归见 S11 11.1。

### 前置条件
- [x] P2（SSH exec）

### 任务清单

#### 5.1 rterm-monitor MonitorCollector
- [x] `start(session_id, interval_secs=2)`：后台线程 + 独立 SSH 会话
- [x] Linux 采集脚本：单次 exec 多段输出（`@@RT_<SECTION>@@` 标记）
- [x] macOS 采集脚本：ps 回退路径
- [x] 分段解析 → ResourceSample（增量 CPU / 网络速率）
- [x] 最近 60 个采样点 VecDeque（每指标独立）
- [x] 阈值比较（rolling avg 避免尖刺误报）
- [x] 告警触发：EventBus + 状态栏

#### 5.2 Monitor View（gpui）
- [x] 6 个卡片：CPU% / 内存% / Swap% / 磁盘% / rx_kbps / tx_kbps / load_avg_1
- [x] 每张卡片内嵌 canvas 折线图（最近 60 点）
- [x] 暂停 / 恢复 / 重置按钮
- [x] Top 进程表：PID / USER / CPU% / MEM% / COMMAND，可排序（/proc 直读 awk，CPU% 按 tick 差值）
- [x] 右键进程 Kill / Force Kill（二次确认）
- [x] 阈值设置入口（全局设置 → 监控）

---

## Phase 6 — 稳定性收尾 ✅（S10，2026-11-17）

### 任务清单

- [x] 主机密钥 known_hosts 持久化（兼容格式 + 原子写 + 三选一确认；hashed 匹配与 Windows 原子替换见 S10 9.1b/9.1c）
- [x] 连接失败错误码映射（auth / timeout / host_key_changed / network）→ 面向用户的文案（9.2）
- [x] 命令面板 Ctrl+P + 模糊搜索（9.3，搜索仅 ASCII，见已知限制）
- [x] 全局快捷键全部生效（Ctrl+, 设置 / Ctrl+Shift+N 新会话 / F11 全屏 / Ctrl+T 新标签 / Ctrl+W 关标签 / Ctrl+Shift+L 会话日志 / Ctrl+Shift+Space vi）
- [x] 导出诊断包（logs + 版本 + 会话数 → zip，9.11）
- [x] 自动备份定时器启动（24h + 保留 7 份，9.9）
- [x] 会话日志文件写入（per-tab 可开 / 关，9.5）
- [x] OSC 52 远程剪贴板支持（9.6，只读方向）
- [x] vi 模式（9.7，导航 + 选词）
- [x] OpenSSH config 导入完善（Include 递归 2 层 + Match 条件，9.8）
- [ ] `.ppk` / PuTTY reg 文件导入（可选，未排期）

---

## Phase 7 — v1.0 打磨 🚧（S11 进行中）

> 任务拆分与 S10 交接项见 [sprint-11-release.md](./sprint-11-release.md)；S10 已完成 Release profile 预配置与 CHANGELOG 主体。

### 任务清单

- [ ] Release 构建验证（lto=thin / codegen-units=1 / strip=true 已配好）
- [ ] Windows 安装包（Inno Setup：rterm.exe + 图标 + 文件关联）
- [ ] macOS .dmg 构建流程（后续）
- [ ] Linux AppImage / deb（后续）
- [ ] 国际化（i18n，可选）
- [ ] 完整 README.md（用户版 + 开发者版）
- [ ] CHANGELOG.md v1.0 发布说明（补齐 Sprint 9 条目）
- [ ] 全量回归测试（基础连接 + 终端仿真 + SFTP 上传 + 路径同步 + 监控）

---

## Phase 8 — v1.1 增强（S12 终端内嵌 SFTP 侧栏）

> 任务拆分与验收见 [sprint-12-terminal-sftp-dock.md](./sprint-12-terminal-sftp-dock.md)；不阻塞 v1.0 发布。

### 前置条件
- [x] S8 路径同步（`TerminalCwdHint` / SyncBridge 防回环）
- [x] S10 SFTP 打磨（拖拽 / 虚拟化 / 后台读取）
- [ ] S11 v1.0 发布基线

### 任务清单

- [ ] `SftpView` Dock 模式（隐藏工具条、自适应宽度、延迟连接）
- [ ] `TerminalTabView` 组合视图：终端 + 右侧 SFTP 侧栏 + 可拖拽分隔条（280–720px）
- [ ] 路径跟随：`TerminalCwdHint` → 侧栏 `navigate_to_remote`（按同步模式门控、`pending_sftp_nav` 防回环）
- [ ] 开关入口：终端头部按钮 + `Ctrl+Shift+S`（`ToggleSftpDock` action）
- [ ] 设置持久化：默认显示侧栏 + 侧栏宽度
- [ ] 生命周期：关闭 Tab 同时释放 SFTP；侧栏错误不污染终端
- [ ] 回归：多 Tab / 主题 / 高 DPI / 拖拽传输 / 暂停取消 / vi 模式；README + CHANGELOG 更新

---

## Phase 9 — v1.2 会话管理增强 + SSH 跳板代理（S13）

> 任务拆分与验收见 [sprint-13-session-management-proxy.md](./sprint-13-session-management-proxy.md)；
> 可与 S12 并行，不阻塞 v1.0 / v1.1 发布。

### 前置条件
- [x] S2 会话数据层（Session / SessionStore / 导入导出 schema）
- [x] S3 会话树 + 编辑对话框
- [x] S4 设置 + 导入导出
- [x] S5 SSH 连接（`proxy_jump` 已建模，未接入连接链）
- [ ] S12 内嵌 SFTP 侧栏（仅 `main_window.rs` 合并顺序约定）

### 任务清单

- [x] 会话文件 schema v2：`Session.icon`、`SessionFile.folders`（支持空分组），v1 向后兼容
- [x] SSH 跳板代理：`build_chain` 环路/深度校验；`channel_direct_tcpip` 逐跳转发，终端 / SFTP / 监控生效
- [x] SSH 端口转发：新 `rterm-forward` crate，本地 `-L` / 远程 `-R` / 动态 `-D`；独立连接、规则启停与自动启动、转发 Tab
- [x] 分组：新建 / 重命名 / 删除空分组；会话编辑改为可搜索分组下拉 + 内联新建
- [x] 会话图标：预设图标选择（编辑对话框 + 右键「更改图标…」），树渲染并持久化
- [x] 空白区右键菜单：新建会话 / 新建分组 / 导入会话 / 展开折叠
- [x] 顶部菜单栏（会话 / 连接 / 视图 / 帮助）：自绘、F10/悬停切换、动作与右键/快捷键统一
- [x] 右键菜单职责分离：空白 / 分组 / 会话三类独立动作枚举与分区布局，全局功能移出会话菜单
- [x] 导入导出完善：统一入口、按分组导出、OpenSSH config 导入 UI、导出包含新字段
- [ ] 回归与文档：真实远端手工 smoke（跳板链 / 端口转发 E2E）；自动化门禁已通过

---

## 风险与缓解

| 风险 | 概率 | 影响 | 缓解 |
|------|------|------|------|
| alacritty_terminal 版本升级破坏 API | 中 | 中 | 锁定精确版本 `alacritty_terminal = "=0.26.0"`；glue 层隔离 |
| ssh2 vendored-openssl 编译失败 | 低 | 高 | perl 环境强制校验；fallback 用 openssl-sys + vcpkg |
| gpui 事件循环与 tokio Runtime 冲突 | 中 | 高 | bezel-gpui 已内置 tokio Runtime，统一用 `cx.spawn` / `background_spawn` |
| PS1 正则匹配率低 | 中 | 中 | 至少 3 套正则模板；允许用户自定义；兜底走 pwd |
| 传输大文件时阻塞 SSH 通道 | 低 | 中 | 传输用独立 SFTP channel，不占 PTY channel |
| gpui Canvas 字体渲染字形问题（emoji / CJK） | 中 | 低 | gpui 自带 DirectWrite / CoreText / FreeType；实测后补 fallback 字体列表 |

---

## 完成定义 (v1.0)

- [x] 会话：增删改查 + 持久化 + DPAPI 加密 + 导入导出 + OpenSSH config
- [x] 终端：xterm-256color + 主题 + 滚动缓冲 + 复制粘贴 + 快捷键
- [x] SFTP：双栏浏览 + 批量传输 + 进度 + 拖拽 + 列表虚拟化（S10 收口；GUI E2E 见 S11 11.1）
- [x] 路径同步：PS1 + pwd 兜底 + 双向 + ignore 防回环 + 回显抑制（S8 + S10）
- [x] 监控：CPU / MEM / NET / DISK / Load + 折线图 + Kill（S9）
- [x] 稳定性：重连 + known_hosts + 命令面板 + 自动备份 + 日志 + 诊断包（S10）
- [x] 工程门禁：`cargo fmt --check` / `cargo clippy --all-targets` 0 warning / `cargo test --workspace` 174 passed
- [ ] Windows 安装包 + 用户版 README + CHANGELOG 定稿（S11）

**全部满足 → 发布 v1.0.0**

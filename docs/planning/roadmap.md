# rterm — 开发路线图 (Roadmap)

> **更细任务拆分 + 当前状态见 [planning/README.md](./README.md)**（每个 Sprint 一个独立文件，任务粒度 0.25–1h）。
> 本文档保留里程碑概览和高层任务分类，两份文档互为补充。

## 里程碑概览

| Phase | 交付 | 预估 | 关键依赖 |
|-------|------|------|----------|
| **P0 环境 & 骨架** | gpui 空窗口可运行 + 所有 crate 骨架 + MSVC 环境脚本 | ✅ 已完成 | VS Build Tools、perl、bezel-gpui 0.3 |
| **P1 会话管理** | 会话树 View + SessionStore CRUD + 持久化 JSON + 导入导出 + 全局设置界面 | ✅ 已完成（S2–S4） | serde / uuid / rterm-crypto |
| **P2 SSH 连接 & 终端** | ssh2 Session 封装 + PTY + alacritty_terminal glue + gpui Canvas 渲染 + 基础主题 | ✅ 已完成（S5 + S6） | ssh2 vendored-openssl / alacritty_terminal |
| **P3 SFTP 文件管理** | SFTP 双栏浏览 + 传输队列 + 进度条 + 拖拽 | — | ssh2 Sftp / walkdir |
| **P4 路径同步** | SyncBridge + PS1 正则 + ignore 防回环 + 双向开关 | — | regex |
| **P5 远程性能监控** | MonitorCollector + 折线图 + Top 进程 + 阈值告警 | — | tokio 定时 + Canvas 折线图 |
| **P6 稳定性收尾** | 主机密钥管理 + 重连策略 + 命令面板 + 快捷键 + 日志导出 + 自动备份 | — | winreg / dirs |
| **P7 打磨 & v1.0** | 主题扩展 + vi 模式 + 会话日志 + 命令面板完善 + OpenSSH config 导入 + 安装包 | — | — |

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
- [ ] PS1 解析正则集合（bash / zsh / generic）→ S8
- [ ] 终端输入回显抑制（SyncBridge 注入 cd 时）→ S8

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

## Phase 3 — SFTP 文件管理 🚧（S7 核心完成）

> **S7.5 技术债清偿已完成（2026-11-16）**：[sprint-07.5-tech-debt.md](./sprint-07.5-tech-debt.md) 已完成数据安全、线程生命周期、状态一致性与主机密钥收口；自动化门禁 92 passed / 1 ignored、clippy 0 warning、fmt 通过。Phase 4 可开始。

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
- [x] 暂停 / 恢复 / 取消 / 失败重试（代码完成；暂停取消 E2E 留 S10）

#### 3.3 Sftp View（gpui）🚧
- [x] 双栏 Split View（左本地，右远程）
- [x] 地址栏 + 面包屑 + 刷新
- [ ] 前进 / 后退
- [x] 文件列表图标（目录 / 文件 / 符号链接）
- [ ] 拖拽上传 / 下载（S10 9.18）
- [x] 右键菜单：上传 / 下载 / 重命名 / 删除 / chmod / 属性（新建目录在顶部工具栏）
- [x] 传输进度条（底部传输面板）

---

## Phase 4 — 路径同步

### 前置条件
- [x] P2（Terminal 输出字节流 + PS1 解析）
- [x] P3（SFTP 文件浏览 + 导航事件）
- [x] S7.5 技术债清偿（S8 前强制门禁）

### 任务清单

#### 4.1 rterm-sync SyncBridge
- [ ] `SyncBridge::on_terminal_output(bytes) → Option<PathBuf>`
- [ ] 3 套 PS1 正则模板（bash default `w@h:p$`、zsh `w@h p $`、generic 带颜色控制码）
- [ ] 兜底 pwd 触发：正则匹配失败 → 后台注入 `pwd 2>/dev/null` → 1s 超时忽略
- [ ] `SyncBridge::on_sftp_navigated(path) → Option<InjectCommand>`
- [ ] ignore_list（VecDeque, max 10）：防止 SFTP→Terminal 注入的 cd 又被反同步
- [ ] 方向开关（off / term→sftp / sftp→term / bidirectional）
- [ ] per-session 可覆盖默认方向

#### 4.2 集成到 Terminal + SFTP
- [ ] TerminalView 每次 PTY 字节流喂 Term 后 → SyncBridge.on_terminal_output
- [ ] SFTP View 每次导航（双击 / 面包屑 / 右键 cd）→ 广播 UserRequestedCwd
- [ ] SyncBridge.on_sftp_navigated → 组装注入命令 → TerminalService 注入 PTY
- [ ] 回显抑制：注入命令前加 escape，Term 侧识别为内部命令不显示给用户

---

## Phase 5 — 远程性能监控

### 前置条件
- [x] P2（SSH exec）

### 任务清单

#### 5.1 rterm-monitor MonitorCollector
- [ ] `start(session_id, interval_secs=2)`：tokio interval loop
- [ ] Linux 采集脚本：`cat /proc/stat; cat /proc/meminfo; df -P / | tail -1; cat /proc/net/dev; uptime`
- [ ] macOS 采集脚本：`sysctl -n vm.loadavg; sysctl -n hw.memsize; vm_stat; netstat -ib`
- [ ] 单行解析 → ResourceSample
- [ ] 最近 60 个采样点 VecDeque（每指标独立）
- [ ] 阈值比较（rolling avg 避免尖刺误报）
- [ ] 告警触发：EventBus + 系统通知

#### 5.2 Monitor View（gpui）
- [ ] 6 个卡片：CPU% / 内存% / Swap% / 磁盘% / rx_kbps / tx_kbps / load_avg_1
- [ ] 每张卡片内嵌 canvas 折线图（最近 60 点）
- [ ] 暂停 / 恢复 / 重置按钮
- [ ] Top 进程表：PID / USER / CPU% / MEM% / COMMAND，可排序
- [ ] 右键进程 Kill / Force Kill（二次确认）
- [ ] 阈值设置入口（per-session 或全局）

---

## Phase 6 — 稳定性收尾

### 任务清单

- [ ] 主机密钥 known_hosts 持久化（`~/.ssh/known_hosts` 兼容格式）
- [ ] 连接失败错误码映射（auth / timeout / host_key_changed / network）→ 面向用户的文案
- [ ] 命令面板 Ctrl+P（gpui 自带 keybinding 框架）
- [ ] 全局快捷键全部生效（Ctrl+, 设置 / Ctrl+Shift+N 新会话 / F11 全屏 / Ctrl+T 新标签 / Ctrl+W 关标签）
- [ ] 帮助 → 导出诊断包（logs + 版本 + 会话数）
- [ ] 自动备份定时器启动（24h）
- [ ] 会话日志文件写入（per-tab 可开 / 关）
- [ ] OSC 52 远程剪贴板支持
- [ ] vi 模式（alacritty_terminal 自带）
- [ ] OpenSSH config 导入完善（Include / Match 语法）
- [ ] `.ppk` / PuTTY reg 文件导入（可选）

---

## Phase 7 — v1.0 打磨

### 任务清单

- [ ] Release profile 优化（lto=thin / codegen-units=1 / strip=true 已配好）
- [ ] Windows 安装包（MSI 或 Inno Setup：rterm.exe + 图标 + 文件关联）
- [ ] macOS .dmg 构建流程（后续）
- [ ] Linux AppImage / deb（后续）
- [ ] 国际化（i18n，可选）
- [ ] 完整 README.md（用户版 + 开发者版）
- [ ] CHANGELOG.md（v0.1 → v1.0 历史）
- [ ] 全量回归测试（基础连接 + 终端仿真 + SFTP 上传 + 路径同步 + 监控）

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
- [x] SFTP：双栏浏览 + 拖拽 + 批量传输 + 进度
- [x] 路径同步：PS1 + pwd 兜底 + 双向 + ignore 防回环
- [x] 监控：CPU / MEM / NET / DISK / Load + 折线图 + Kill
- [x] 稳定性：重连 + known_hosts + 命令面板 + 自动备份 + 日志
- [x] Windows MSI 安装包 + README + CHANGELOG

**全部满足 → 发布 v1.0.0**

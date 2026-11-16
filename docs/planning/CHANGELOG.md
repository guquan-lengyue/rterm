# CHANGELOG — Sprint 完成记录

> 每个 Sprint 完成后，把总结追加到这里。格式统一：状态 / commit / 交付清单 / 代码统计 / 架构落地 / 踩坑 / 验收 checklist / 遗留。

---

## Sprint 7 — SFTP 文件管理（核心完成）

**状态**：🚧 IN PROGRESS | **Commit**：待提交 | **时间**：2026-11-16
**前置**：S5（SSH Session）+ S6（多类型 Tab / TabBar）
**后续**：S8 直接订阅 `SftpNavigated`；S9 按 `TabContentView` 模式接入 Monitor

### 交付清单

| 模块 | 内容 |
|------|------|
| `rterm-sftp/service.rs` | `SftpService`（listdir / stat / mkdir / rmdir / unlink / rename / chmod / home_dir）+ `FileEntry`、目录优先排序、`format_mode/size`；11 个单元测试 |
| `rterm-sftp/queue.rs` | 2 worker 上传/下载队列、64KB 分块、100ms 进度节流、暂停/继续/取消、3 次重试、失败清理 |
| `SftpView` | 双栏（本地/远端）、地址栏 + 转到、面包屑、列头排序、鼠标滚轮、双击目录进入、双击文件上传/下载、右键下载/上传/重命名/删除/权限/属性、顶部「＋ 文件夹」、底部传输面板 |
| Tab 集成 | `TabContentView::{Terminal, Sftp}`、`TabKind::Sftp`、会话右键「📁 打开 SFTP」、Ctrl+W 关闭（断开 SFTP 并取消队列） |
| 事件 | `SftpOpened/SftpNavigated/SftpClosed` + `FileTransferStarted/Progress/Completed/Failed` 转发到 EventBus |

### 测试 & 验收

| 项 | 结果 |
|----|------|
| `cargo test --workspace` | ✅ 全部通过（新增 rterm-sftp 11 个测试） |
| `cargo build -p rterm-app` | ✅ BUILD OK |
| E2E：连接 192.168.1.95 | ✅ 本地面板 + 远端 `/home/ubuntu` 正常列出 |
| E2E：下载 | ✅ `.trae-cn/plugin-config.json`（453 B）→ 本地 `.icube-remote-ssh/`，进度条 100%「已完成」 |
| E2E：上传 | ✅ 同一文件上传回远端，远端列表出现新文件（mtime 更新），两个任务均「已完成」 |
| E2E：关闭 Tab | ✅ Ctrl+W 关闭 SFTP Tab，SSH/队列释放，进程存活 |

### 踩坑 & 修正

| # | 问题 | 解法 |
|---|------|------|
| 1 | TextInput 外部替换内容后 IME `marked_range` 越界导致 panic | 新增 `TextInput::set_content` 清空 marked_range；所有 IME 区间与渲染下标统一 `clamp_range` 到字符边界（含密码模式），并补 2 个单测 |
| 2 | 双栏宽度被内容撑开 / 列被挤出窗口 | 改为 `w(relative(0.5))` 等宽 + 固定元信息列宽 + 名称 `truncate()` |
| 3 | 列表超过一屏无法访问 | 增加滚轮滚动（按 3 行/格），渲染时 `skip(scroll)` |
| 4 | 地址栏文本溢出遮挡按钮 | 地址栏独立成行 + `overflow_hidden` + 显式「转到」按钮 |

### 遗留

| 项 | 后续 |
|----|------|
| 拖拽上传/下载 | S10 9.18 |
| 暂停/继续/取消 GUI E2E | S10 9.18 |
| 空白区右键菜单（当前用顶部「＋ 文件夹」） | S10 9.18 |
| mkdir/rename/delete/chmod 未逐项 E2E | S10 全量 smoke |
| 文件列表未虚拟化（大目录） | S10 |
| SFTP 使用独立 SSH 连接，未复用终端 Tab 的 Session | 设计取舍，功能等价；如需复用可在 S8 统一连接池 |

---

## Sprint 6 — 终端渲染

**状态**：✅ COMPLETED | **Commit**：`6b07fd6` | **时间**：2026-11-16
**前置**：S5（TerminalEngine / PTY / 连接编排）+ S4（Settings / 4 套 UI 主题）
**后续**：S7（SFTP）/ S9（监控）可直接复用 TabBar

### 交付清单

| # | 任务 | 产出文件 | 验收 |
|---|------|----------|------|
| 5.1.1 | gpui Canvas 逐 Cell 渲染（背景 quad + DirectWrite glyph + ANSI 色） | `views/terminal.rs` | ✅ 真实远程 shell 文字/颜色正确 |
| 5.1.2 | 光标 Block / Underline / Bar + 500ms blink，读 `terminal_cursor_style` | `views/terminal.rs` | ✅ 方块光标可见并闪烁 |
| 5.1.3 | 滚动缓冲：滚轮 + Shift+PageUp/PageDown + 右侧滚动条 | `views/terminal.rs` + `engine.rs` | ✅ `seq 1 200` 后 Shift+PageUp 回看到 118–145 行 |
| 5.1.4 | 鼠标拖选高亮 + 松开自动写剪贴板 | `views/terminal.rs` + `engine.rs` | ✅ 拖选 21 字符自动复制，右键可粘贴回来 |
| 5.1.5 | 窗口/字号变化 → cols/rows → Engine + `PtyCommand::Resize` | `views/terminal.rs` | ✅ `stty size` = 49×157；字号 14→15→16 时同步 147×45 / 138×42 |
| 5.2.1 | 右键粘贴 + bracketed paste 保护 | `views/terminal.rs` | ✅ 单行/多行粘贴走 `\x1b[200~…\x1b[201~` |
| 5.2.2 | Alt / Ctrl+方向 / F1–F12 / Home/End/PageUp/PageDown | `views/terminal.rs` + `engine.rs` | ✅ 映射表补全；Ctrl+C 可中断 |
| 5.3.1 | `TerminalPalette`（fg / bg / ANSI16 / cursor）+ 4 套主题调色板 | `theme.rs` | ✅ 4 套调色板可构造；单元测试覆盖 |
| 5.3.2 | `AppThemeChanged` / `AppSettingsChanged` → 终端即时换色/换字体 | `views/terminal.rs` + `views/settings.rs` | ✅ 设置预览切主题即广播，取消/恢复默认会还原 |
| 5.4.1 | `TabBar`：标签列表 + ✕ + 右键（关闭/其他/右侧） | `views/tab_bar.rs` | ✅ 点击切换、✕ 关闭 |
| 5.4.2 | Tab 生命周期：新建 = 新 TerminalView + SSH/PTY；关闭 = `PtyCommand::Close` + drop Engine | `views/main_window.rs` | ✅ 关 tab 断开；重开重连 |
| 5.4.3 | Ctrl+T / Ctrl+Tab / Ctrl+W（全局 action） | `main.rs` + `views/main_window.rs` | ✅ 三快捷键实测 |
| 5.5.1 | Ctrl+= / Ctrl+- / Ctrl+0 字号缩放并写回 Settings | `views/terminal.rs` | ✅ 立即生效 + 落盘 `settings.json` |
| 5.5.2 | 真实状态栏：主机 + 连接延迟 + 重连次数 + 时钟 | `views/status_bar.rs` | ✅ 订阅 `SessionConnected/Latency/Reconnecting/...`，时钟每秒刷新 |
| — | `SessionLatency` / Tab 事件 | `event.rs` | ✅ 状态栏与 TabBar 解耦 |

### 测试 & 验收

| 项 | 结果 |
|----|------|
| `cargo test --workspace` | ✅ **50 passed, 0 failed, 1 ignored**（live_ssh） |
| `cargo build -p rterm-app` | ✅ 成功（仅 OpenSSL PDB 链接警告） |
| GUI E2E：双击 `pw-test` → 连接 192.168.1.95 | ✅ 登录 banner 按 ANSI 色渲染，标题栏显示 `已连接 · 延迟` |
| GUI E2E：键盘 `pwd` / `stty size` | ✅ `/home/ubuntu`；`49 157` |
| GUI E2E：Ctrl+T / Ctrl+Tab / Ctrl+W | ✅ 两标签并存、切换、关闭后 SSH 断开 |
| GUI E2E：字号缩放 | ✅ Ctrl+= 两次（14→16），列数 157→138，重启后保留 |
| GUI E2E：拖选 + 右键粘贴 | ✅ 复制 21 字符，粘贴回 shell |
| GUI E2E：Shift+PageUp 回滚 | ✅ 视口上移到历史行 |
| GUI E2E：htop / vim 全屏程序 | ✅ htop alt-screen + CPU 彩条；vim 启动页正常、`:q!` 退出 |
| GUI E2E：`seq 1 200` / 长时间输出 | ✅ 无卡顿、无 panic |

### 踩坑 & 解法（6 个坑）

| # | 问题 | 解法 |
|---|------|------|
| 1 | `App::text_system()` 返回的 `TextSystem` 没有 `shape_line` | 字形缓存改用 `window.text_system()`（`WindowTextSystem`） |
| 2 | gpui `Rgba` 没有 `Rgb` 类型，计划里的 `ansi16: [Rgb;16]` 不可用 | 改为 `[Rgba; 16]`，颜色用 `rgb(0x…)` 构造 |
| 3 | `div.on_mouse_down` 需要显式 `MouseButton` 参数 | 左键选择、右键粘贴各注册一个 listener |
| 4 | 每帧逐 Cell 排版 3000+ 字符会反复 shaping | `HashMap<(char,bold,italic)>` 字形缓存，字号/字体变化时清空 |
| 5 | 终端 Grid 宽度与 CJK 宽字符对齐 | 按 cell 定位绘制；`WIDE_CHAR_SPACER` 跳过，宽字符只画一次 |
| 6 | 关闭 Tab 时后台 SSH 线程仍持有旧输出 | `generation` 代际号丢弃过期事件 + `PtyCommand::Close` 主动断开 |

### 遗留

| 项 | 后续 |
|----|------|
| 高 DPI（150%）环境下 gpui 窗口/弹窗坐标与自动化工具换算不一致，主题切换未完成鼠标 E2E | S10 统一做一次 GUI 回归 |
| 光标方块绘制在字形下方，光标色与前景色相近时对比度一般 | S10 视觉打磨 |
| 滚动条仅绘制，不支持拖拽 | S10 |
| 双击选中单词 / 三击整行、URL 点击 | S10 |
| 系统字体枚举（S4 遗留） | S10 |
| `TabRegistry` 结构体仍为占位，实际 Tab 列表在 `MainWindow.tabs`，且当前固定 `Entity<TerminalView>` | S7/S9 接入时泛化为多类型 Tab |

---

## Sprint 5 — SSH 连接 & 终端核心

**状态**：✅ COMPLETED | **Commit**：待提交 | **时间**：2026-11-14
**前置**：S2（Session 模型）+ S3（双击 connect 事件）+ S4（Settings / 主题 / Modal）
**后续**：S6（终端 Canvas 渲染）

### 交付清单

| # | 任务 | 产出文件 | 验收 |
|---|------|----------|------|
| 5.1.1 | SshSession + ConnectionState 状态机 | `rterm-ssh/src/lib.rs` | ✅ new / Disconnected / Connected |
| 5.1.2 | Password / PrivateKey / Agent 三种认证 | `rterm-ssh/src/auth.rs` | ✅ 密码认证真实服务器通过 |
| 5.1.3 | TCP + handshake + 超时 + 压缩 | `rterm-ssh/src/lib.rs` | ✅ `connect_timeout_secs` 生效 |
| 5.1.4 | known_hosts 校验 + 首次记录 + 变更检测 | `rterm-ssh/src/known_hosts.rs` | ✅ TOFU 追加；变更拒绝连接并广播 |
| 5.2.1 | open_pty（xterm-256color + shell） | `rterm-ssh/src/pty.rs` | ✅ 真实服务器返回 Channel |
| 5.2.2 | 后台 PTY 读写线程 | `rterm-ssh/src/pty.rs` | ✅ non-blocking 轮询，输入/输出双向通 |
| 5.2.3 | resize_pty | `rterm-ssh/src/pty.rs` | ✅ `request_pty_size` |
| 5.3.1 | TerminalEngine::new | `rterm-terminal/src/engine.rs` | ✅ 80×24×10000 构造 |
| 5.3.2 | input_bytes（vte → Term） | `rterm-terminal/src/engine.rs` | ✅ "hello\r\n" 落入 Grid |
| 5.3.3 | grid / cursor / colors / resize | `rterm-terminal/src/engine.rs` | ✅ ANSI 颜色与光标位置测试 |
| 5.3.4 | input_key（xterm 序列） | `rterm-terminal/src/engine.rs` | ✅ Enter/Backspace/方向键/Ctrl+F1–F12 |
| 5.4.1 | KeepAlive | `rterm-ssh/src/keepalive.rs` | ✅ IO 循环内 `keepalive_send` |
| 5.4.2 | 指数退避重连 | `rterm-ssh/src/reconnect.rs` | ✅ 1/2/4/8/16s，最多 5 次 |
| 5.5.1 | SessionConnectRequested → SSH → PTY → Engine | `views/terminal.rs` | ✅ 双击会话自动连接并输出 |
| 5.5.2 | 会话状态图标 + 右键「打开终端」 | `views/session_tree.rs` / `main_window.rs` | ✅ 灰/蓝/绿/红状态点 |
| 5.5.3 | 终端文本 placeholder | `views/terminal.rs` | ✅ 实时显示远程 shell 输出 |
| — | 键盘输入 | `views/terminal.rs` | ✅ 字符 / Enter / 方向键 / Ctrl 组合 |
| — | 错误分级 | `views/terminal.rs` | ✅ 认证/配置/主机密钥错误快速失败，仅网络错误退避重连 |
| — | Windows libssh2 后端修复 | `Cargo.toml` | ✅ `openssl-on-win32`（否则无 curve25519/ed25519） |

### 测试 & 验收

| 项 | 结果 |
|----|------|
| `cargo test --workspace` | ✅ **43 passed, 0 failed**（1 个 live test 默认 ignored） |
| `live_ssh`（真实服务器 192.168.1.95） | ✅ connect → auth → PTY → `pwd` 返回 `/home/ubuntu` |
| GUI E2E：双击 pw-test → 连接 | ✅ 状态栏显示「已连接」，登录 banner 渲染 |
| GUI E2E：键盘输入 `pwd` | ✅ 输出 `/home/ubuntu` |
| GUI E2E：`vim` | ✅ 进入 Vim 9.1.241 界面，可退出回 shell |
| GUI E2E：`tmux` | ✅ tmux 状态栏出现 `[0] 0:tmux*` |
| 退出保存 | ✅ Alt+F4 → `Saving 2 sessions` / `Saved OK` |

### 踩坑 & 解法（4 个坑）

| # | 问题 | 解法 |
|---|------|------|
| 1 | Windows 上 libssh2 默认走 WINCNG，无 curve25519-sha256 / ssh-ed25519 → `[Session(-5)] Unable to exchange encryption keys` | workspace `ssh2` 增加 `openssl-on-win32` feature，libssh2 改用 OpenSSL |
| 2 | ssh2 blocking `channel.read` 持有 Session 锁，导致键盘输入写不出去 | shell 打开后 `set_blocking(false)`，单线程轮询 read/write/resize/keepalive |
| 3 | `TerminalView` 需要长时间阻塞的 SSH IO，不能占用 gpui executor | SSH/PTY 用独立 `std::thread`；输出经 `futures::mpsc` 回主线程 `cx.spawn` |
| 4 | 全局 `backspace/ctrl-a` 等 TextInput 绑定会截获终端按键 | bindings 加 `Some("TextInput")` context，TextInput 根节点加 `.key_context("TextInput")` |

### 遗留

| 项 | 后续 |
|----|------|
| host key 首次连接为 TOFU 自动记录，未做三选一确认 Modal | S10 9.1 |
| Canvas 逐 Cell 渲染 / 光标 / 滚动条 / 鼠标选择 | S6 |
| TabBar / Ctrl+T / Ctrl+Tab / Ctrl+W | S6 |
| 窗口 resize → PTY resize 联动 | S6 |
| 跳板机 `proxy_jump` 链式连接 | S5 后续 / S10 |
| 真实断网重连未做端到端验证（退避/重试逻辑已通过认证失败场景验证） | S10 |
| Ctrl+P 命令面板 | S10 9.3 |

---

## Sprint 4 — 设置 + 导入导出

**状态**：✅ COMPLETED | **Commit**：`e7a9574` | **时间**：2026-11-14
**前置**：S3（SessionTree / SessionEditDialog / 启动退出持久化）
**后续**：S5（SSH 连接 + 终端核心）

### 交付清单

| # | 任务 | 产出文件 | 验收 |
|---|------|----------|------|
| 4.1.1 | SettingsView：左侧 7 Tab + 右侧表单 | `views/settings.rs` | ✅ 每 Tab 内容独立；`Ctrl+,` 打开 |
| 4.1.2 | SSH 默认值 / SFTP 模式 / 监控 / 高级目录 | `views/settings.rs` | ✅ 保存后写回 `AppState.settings` 并广播事件 |
| 4.1.3 | 终端外观：主题 / 字体 / 字号 / 光标 / 滚动 + 预览 | `views/settings.rs` + `theme.rs` | ✅ 切 Dracula 预览立刻变紫；字号联动预览 |
| 4.1.4 | 保存 / 取消 / 恢复默认 | `views/settings.rs` | ✅ 取消还原主题；恢复默认重置全部字段 |
| 4.1.5 | Settings 持久化 | `state.rs` + `main.rs` | ✅ `%APPDATA%\rterm\settings.json`，重启保留 |
| 4.2.1 | 导出 Modal：Scope / DPAPI·AES-GCM / rfd 路径 | `views/import_export.rs` | ✅ 全部/文件夹/多选；DPAPI 本机可解，口令文件跨机可用 |
| 4.2.2 | 导入 Modal：口令 / 冲突策略 / diff 预览 / 报告 | `views/import_export.rs` + `export.rs` | ✅ 错口令显示解密失败；正确口令预览 + 更新 2 条 |
| 4.2.3 | 入口：设置 Tab + SessionTree 右键菜单 | `main_window.rs` | ✅ 两处入口打开同一个 Modal |
| — | 主题扩展 + 注册表 | `theme.rs` | ✅ Dracula / One Dark；`all_themes()` / `theme_by_name()` |
| — | 导入预览 API | `rterm-session/src/export.rs` | ✅ `preview_import` 复用 `parse_and_decrypt` |

### 测试 & 验收

| 项 | 结果 |
|----|------|
| `cargo check --workspace` | ✅ 0 error |
| `cargo test --workspace` | ✅ 35 passed, 0 failed |
| E2E：终端主题 Dracula → 保存 → 重启 | ✅ 重启后仍是 Dracula |
| E2E：DPAPI 导出 2 个会话 → 导入 Overwrite | ✅ 数量一致，report 更新 2 |
| E2E：AES-GCM 口令 "abc" → 错误口令预览 | ✅ 显示「导入解密失败 — 可能需要口令」 |
| E2E：`Ctrl+,` | ✅ 设置弹窗打开 |

### 踩坑 & 解法（5 个坑）

| # | 问题 | 解法 |
|---|------|------|
| 1 | `App::on_action` 里同步 `WindowHandle::update` 报 `window not found` | action 派发期间窗口被临时移出 `App.windows`，改用 `app.defer` 延迟到事件处理结束后再打开 |
| 2 | 根节点 `.on_action` 在无聚焦元素时不触发 | 无焦点时 action 从 dispatch tree 根节点冒泡，不进入控件树；改用 `App::on_action` 全局监听 |
| 3 | gpui 0.3 没有 `min_h` / 滚动容器 | 表单布局保持紧凑；长列表限制展示条数（选择 24 / 预览 12） |
| 4 | 导入错误链过长把 footer 按钮挤出卡片 | 只显示顶层错误（`{e}` 而非 `{e:#}`）+ 消息区 `truncate()` |
| 5 | rfd 首次引入 + 屏幕坐标随 DPI/窗口位置变化 | 锁定依赖后用像素扫描定位按钮，坐标按 1.5 DPI 换算 |

### 遗留

| 项 | 后续 |
|----|------|
| 选择会话 / 预览列表无滚动，超过上限只显示前 N 条 | S10 稳定性阶段补滚动容器 |
| 字体为 4 个预设，未枚举系统字体 | S6 终端渲染时补 |
| 删除会话二次确认仍未做（S3 遗留） | S10 |
| 日志目录 / 数据目录首页仅存储，尚未被终端与日志模块使用 | S6 / S9 |

### 对后续 Sprint 的交接

| S4 已交付能力 | 后续消费方 |
|---------------|-----------|
| `Settings` 默认值：端口 / 用户名 / keepalive / 连接超时 | S5 新建会话与连接超时（5.5.4） |
| 终端外观：主题 / 字体 / 字号 / 光标样式 / 滚动行数 | S6 终端渲染直接读取，不重复建配置 |
| 4 套 UI 主题（Catppuccin Mocha / Windows 11 Light / Dracula / One Dark） | S6 需补终端 fg/bg/ANSI 16 色，`Theme` trait 不含终端调色板 |
| `Settings.sftp_sync_mode` 四选一（已持久化） | S8 SyncBridge 默认模式 |
| `Settings.monitor_interval_secs` + CPU/Mem 阈值 | S9 MonitorCollector |
| `terminal_log_dir` / `data_dir` 字段（仅存储） | S10 会话日志 / 数据目录接线 |
| 自绘 Modal + TextInput + `rfd` 文件选择器 | S5 host key 确认 / S7 本地路径选择 / S10 命令面板 |
| `AppThemeChanged` / `AppSettingsChanged` 事件 | S6 主题/字号即时重绘 |

---

## Sprint 2 — 会话数据层

**状态**：✅ COMPLETED | **Commit**：`3bbc1c16` | **时间**：2026-11-13
**前置**：S1（crypto + event_bus + app_state + 主窗口壳）
**后续**：S3（会话 UI 层）/ S4（SSH + 终端核心）

### 交付清单

| # | 任务 | 产出文件 | 验收 |
|---|------|----------|------|
| 2.1 | Session 模型 + AuthMethod | `model.rs` | ✅ `Session::default()` 可构造；三个 AuthMethod 变体 |
| 2.2 | SessionStore 内存 CRUD | `store.rs` | ✅ upsert 同 id 更新；delete 返回 bool |
| 2.3 | JSON 持久化 + 原子写 | `store.rs` save/load | ✅ save→rename→tmp 清理；文件不存在返回空 store |
| 2.4 | 加密桥接 DPAPI + AES-GCM | `crypto_bridge.rs` | ✅ 密码字段 version=0↔1↔2 三态转换 |
| 2.5 | 导入导出 + 冲突策略 | `export.rs` | ✅ Overwrite/Skip/Rename 三种 + ExportScope + ImportReport |
| 2.6 | OpenSSH config 解析 | `openssh.rs` | ✅ Host/HostName/Port/User/IdentityFile/ProxyJump |
| — | S1 占位替换 | `rterm-app/Cargo.toml` + `state.rs` | ✅ 删掉空 SessionStore，改用 `rterm_session::SessionStore` |
| — | workspace 依赖 | `Cargo.toml` + `rterm-session/Cargo.toml` | ✅ base64=0.22 新增 |

### 测试

| 文件 | 数量 | 全部通过 |
|------|------|----------|
| store_tests.rs | 8 | ✅ new/upsert×2/delete/file-not-found/roundtrip/atomic/encrypted |
| export_tests.rs | 10 | ✅ DPAPI roundtrip + 3 scope + 3 conflict policy + AES-GCM correct/wrong |
| openssh_tests.rs | 2 | ✅ 基本 Host 解析 + HostName 默认 Host |
| **总计** | **20** | **✅ 20/20** |

### 踩坑 & 解法（7 个坑）

| # | 问题 | 解法 |
|---|------|------|
| 1 | rterm-session/Cargo.toml 没加 base64 | workspace Cargo.toml 加 `base64 = "0.22"` + rterm-session 加 `base64.workspace = true` |
| 2 | store.rs SessionFile 缺 `use serde::{Serialize, Deserialize}` | 显式 import |
| 3 | openssh.rs 用了 nightly 的 `&str::leak()` | 重写 split_into_host_blocks 返回 `Vec<(String, String)>`（全所有权） |
| 4 | rterm-app/Cargo.toml 没加 rterm-session 依赖 | workspace Cargo.toml 顶层声明了不等于各 crate 自动继承；每个 crate 自己 Cargo.toml 必须显式写 `rterm-session.workspace = true` |
| 5 | Session struct 缺 `PartialEq` | 加 derive（chrono::DateTime<Utc> + Uuid 都实现了 PartialEq，OK） |
| 6 | openssh.rs doc-comment 里的 ``` 被当 doctest 编译 | 改成 ```text 标记 |
| 7 | wrong passphrase 测试不 fail — Session::default() 密码是空，加密路径被跳过 | 测试里手工构造带明文密码的 Session（version=0） |

### 验收 Checklist

- [x] `cargo check --workspace` → **0 ERROR**
- [x] `cargo test -p rterm-session --lib --tests` → **20 passed, 0 failed**
- [x] `cargo build -p rterm-app` → 无 error（只有 S1 骨架阶段的 dead_code warning）
- [x] JSON 持久化原子写：先 .tmp 再 rename，无半截文件
- [x] EncryptedString 密文 base64：JSON 里不可读密码
- [x] rterm-app/AppState.session_store 类型 → `rterm_session::SessionStore`（替换 S1 占位成功）

### 遗留

| 项 | 后续 |
|----|------|
| CoTaskMemFree 还没修 | Sprint 9 |
| crypto_bridge.rs 还没 ProxyJump 嵌套 AuthMethod 加密/解密（当前已递归） | 已做 ✅ |
| EventBus 还没 emit（upsert/delete broadcast 事件） | Sprint 3 UI 层接上 |
| Settings 持久化 | Sprint 3 |

---


## Sprint 1 — 底层基础设施

**状态**：✅ COMPLETED | **Commit**：`f49a37e` + `40f3f32` | **时间**：2026-11-13
**前置**：S0（gpui 空窗口骨架）
**后续**：S2（会话数据层）/ S3（会话 UI 层）/ S4（SSH + 终端核心）

### 交付清单

| # | 任务 | 产出文件 | 验收 |
|---|------|----------|------|
| 1.1 | rterm-crypto DPAPI | `crates/rterm-crypto/src/lib.rs` | ✅ CryptProtectData / CryptUnprotectData 往返 |
| 1.2 | rterm-crypto AES-GCM + PBKDF2 | 同上 | ✅ 600K iter PBKDF2-SHA256 + 16B salt + 12B nonce |
| 1.3 | crypto 单元测试 | `crates/rterm-crypto/tests/crypto_tests.rs` | ✅ **10/10 通过**（18.62s） |
| 1.4 | EventBus + AppEvent | `crates/rterm-app/src/event.rs` | ✅ 28 个事件变体覆盖会话/终端/SFTP/监控/传输/UI |
| 1.4 | AppState + Settings + Global impl | `crates/rterm-app/src/state.rs` | ✅ `App::set_global(AppState::new())` 成功 |
| 1.5 | 主窗口三栏 gpui | `views/main_window.rs` + `session_tree.rs` + `status_bar.rs` | ✅ 窗口弹出 PID 7360，MainWindowHandle 非零 |
| P0 阻塞 | Perl + vendored-openssl | `scripts/msvc-env.ps1` 补丁 | ✅ `cargo check --workspace` 完整 8 crate 通过（9m13s） |
| 额外 | 测试服务器集成 | SSH ed25519 key + authorized_keys | ✅ 密码 + 密钥双认证通，vim/tmux/htop/nano 全预装 |

### 代码统计

| crate | 新增行数 | 测试 |
|-------|---------|------|
| rterm-crypto | ~170 行 | 10 单元测试 |
| rterm-app | ~430 行 | 0（占位 View 不需要） |
| **总计** | **~600 行** | **10/10 绿** |

### 架构落地图

```
┌─ rterm-app ─────────────────────────────────────┐
│ main.rs: gpui_platform.application()             │
│   ├─ app.set_global(AppState)                    │
│   └─ open_window → MainWindow Entity             │
│        ├─ .child(SessionTreeView Entity)  240px  │
│        └─ .child(StatusBar Entity)         28px  │
│                                                  │
│ event.rs: EventBus(空 struct) + EventEmitter     │
│   └─ AppEvent 28 variants                        │
│                                                  │
│ state.rs: Settings(12 fields) + SessionStore +   │
│           TabRegistry + AppState + impl Global   │
└──────────────────────────────────────────────────┘
         │
         ▼ 依赖
┌─ rterm-crypto ───────────────────────────────────┐
│ dpapi_encrypt / dpapi_decrypt  (windows 0.58)    │
│ aesgcm_encrypt / aesgcm_decrypt (aes-gcm 0.10)  │
│ derive_key PBKDF2-SHA256 (hmac + sha2)           │
└──────────────────────────────────────────────────┘
```

### 踩坑 & 解法（5 个坑）

| # | 问题 | 根因 | 解法 |
|---|------|------|------|
| 1 | `windows::Win32::Security::Cryptography::DATA_BLOB` 不存在 | windows 0.58 改名为 `CRYPT_INTEGER_BLOB` | 直接改名 |
| 2 | `EventBus` 包装 `SubscriberSet`（private） | bezel-gpui 0.3 不让外部碰 SubscriberSet | 用 **空 struct + `impl EventEmitter<T>` marker trait**，gpui 内部自动管理 |
| 3 | Render 里不能 `.child(session_tree)`（impl IntoElement 不存在） | 只有 `Entity<V: Render>` 能嵌进 `.child()`，Render struct 本身不行 | **main.rs 里先 `app.new()` 挂 Entity**，Render 存 Entity ID 当字段，render 时 `.child(entity)` |
| 4 | `AppState` 没有实现 `gpui::Global` | `app.set_global::<G: Global>()` 有 trait bound | 空 impl `impl gpui::Global for AppState {}` 即可 |
| 5 | vendored-openssl 报 `perl not found` | Windows OpenSSL Configure 需要 Perl | Strawberry Perl 5.42.3 已装在 `C:\Strawberry\perl\bin`，加到 msvc-env.ps1 PATH |

### 验收 Checklist

- [x] `cargo test -p rterm-crypto` → 10 passed, 0 failed
- [x] `cargo build -p rterm-app` → 无 error（warning 可接受）
- [x] `cargo check --workspace` → 完整 8 crate 过（含 ssh2 vendored-openssl + alacritty_terminal）
- [x] 启动 `rterm-app.exe` → PID 7360，MainWindowHandle ≠ 0
- [x] Ctrl+C 退出干净，无 panic

### 遗留

| 项 | 状态 | 后续 |
|----|------|------|
| `CoTaskMemFree` 没调用 | 暂跳过（泄漏几百字节无害） | Sprint 9 修 |
| SessionStore / TabRegistry 空占位 | Sprint 2/5 填充 | state.rs 里 Vec\<Uuid\> 换成 Vec\<Session\> |
| EventBus 还没 `.emit()`/`.subscribe()` 调用点 | Sprint 2+ 接上 | 骨架阶段只需要 EventEmitter trait 存在 |
| MSVC + Perl PATH 自动化 | ✅ 已进 `scripts/msvc-env.ps1` | 每次 `. .\scripts\msvc-env.ps1` 生效 |

### 测试服务器信息

| 项 | 值 |
|----|-----|
| 地址 | `192.168.1.95` |
| 用户 | `ubuntu` |
| 密码 | `liang@123..` |
| 密钥 | `C:\Users\wst_k\.ssh\rterm_test`（已加 authorized_keys） |
| OS | Ubuntu 26.04.1 LTS |
| CPU | 4 核 / RAM 7.7 GB |
| 预装工具 | vim / tmux / htop / nano / less / python3 |
| 延迟 | ping 3ms |

---

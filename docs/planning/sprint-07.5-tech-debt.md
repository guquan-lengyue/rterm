# Sprint 7.5 — 技术债清偿（S8 前插入）

> **状态**：**进行中**（P0 数据可靠性 + P1 线程/资源 + P2 状态/交互已完成；P3 与主机密钥收口待续）
> **前置**：[Sprint 7](./sprint-07-sftp.md)（SFTP 核心完成）
> **后续**：[Sprint 8](./sprint-08-sync.md)（路径同步）
> **目标**：在进入路径同步之前，清偿会直接影响**数据安全、UI 阻塞、连接/线程生命周期**的技术债；不新增功能、不改视觉设计。
>
> **依据**：里程碑代码走查（commit `44a7234`，8 个 crate / 约 11.6k 行 Rust）。
> 当前基线：`cargo test --workspace` = 75 passed / 0 failed / 1 ignored（P0/P1/P2 落地后）；`cargo clippy --workspace --all-targets` 仍受既有的 `text_input.rs` reversed range 阻塞。
>
> **当前进度**：
>
> | 分组 | 状态 |
> |------|------|
> | 7.5.1 会话数据可靠性（P0） | 已完成 |
> | 7.5.2 线程、阻塞与资源（P1） | 已完成 |
> | 7.5.3 状态与交互正确性（P2） | 已完成 |
> | 7.5.4 主机密钥安全收口 | 待开始 |
> | 7.5.5 工程质量门禁 | 待开始 |
> | 7.5.6 文档同步与验收 | 待开始 |
>
> **完成定义**：
> 1. P0 与 P1 任务全部通过验收；
> 2. P2 必修项通过；
> 3. `cargo test --workspace` + `cargo clippy --workspace --all-targets` + `cargo fmt --check` 全绿；
> 4. 文末回归清单无阻塞项；
> 5. 未完成项必须明确转入 S8/S9/S10，不允许“口头遗留”。

> **执行纪律**：
> - 不改 UI 布局、主题、文案与交互流程（除本 Sprint 明确列出的修复项）。
> - 不顺手做新功能（拖拽、监控、同步、安装包）。
> - 每个任务先补/更新对应测试，再改实现；P0 / P1 未全绿不得进入 S8。

---

## 7.5.1 会话数据可靠性（P0，必须全绿）

| # | 文件 | 要做的 | 验收 |
|---|------|--------|------|
| 7.5.1.1 | `rterm-app/src/main.rs`、`views/session_dialog.rs`、`views/main_window.rs`、`views/import_export.rs` | 抽出 `persist_sessions()`（复用 `SessionStore::save` 的 tmp→rename）；新建/编辑/删除/导入完成后**立即落盘**；退出保存保留为兜底。`SessionStore.dirty` 不再作为唯一信号 | 新建/编辑/删除/导入后 `taskkill /F` 进程 → 重启状态与操作后一致；退出时不产生重复副作用 |
| 7.5.1.2 | `rterm-app/src/main.rs`、`rterm-session/src/store.rs` | 加载失败保护：读/解密失败时**不得用空库覆盖原文件**；将原文件复制为 `sessions.corrupt-<UTC>.json`，本次启动空库并展示可关闭错误提示；退出保存必须跳过或写独立恢复文件 | 手工破坏 `sessions.json` → 启动不崩溃、原文件保留、备份存在；退出后原文件仍未被空库覆盖 |
| 7.5.1.3 | `rterm-session/src/store.rs`、`crypto_bridge.rs`、`rterm-app/src/main.rs` | 单条会话解密失败不拖垮整个 store：逐条 `decrypt`，失败条目收集为 `LoadError`，其余照常加载；UI 提示“N 条会话无法解密” | 手工替换一条密码密文 → 其他会话可用；提示 1 条失败；不允许静默丢弃 |
| 7.5.1.4 | `rterm-app/src/views/session_dialog.rs` | 私钥口令复用密码框的 sentinel 机制：编辑时显示掩码；未修改则保存时沿用旧值；显式清空才视为删除口令 | 编辑私钥会话、不改口令 → 连接仍成功；清空后保存 → 口令确实被移除 |
| 7.5.1.5 | `rterm-crypto/src/lib.rs` | DPAPI 改为**当前用户**作用域（去掉 `CRYPTPROTECT_LOCAL_MACHINE`）；`CryptProtectData` / `CryptUnprotectData` 输出的 `pbData` 用 `CoTaskMemFree` 释放；补迁移说明 | 同用户可解密；另一 Windows 用户不可解密（手工验证）；代码中不再有“故意泄漏”注释 |
| 7.5.1.6 | `rterm-app/src/main.rs` | 单实例保护：命名互斥体（如 `Local\rterm-single-instance`）或 `%APPDATA%\rterm\rterm.lock` 独占锁；第二个实例提示后退出 | 双击第二个实例 → 提示并退出；第一个实例退出后可再次启动；不出现 `sessions.json` 互相覆盖 |

---

## 7.5.2 线程、阻塞与资源（P1）

| # | 文件 | 要做的 | 验收 |
|---|------|--------|------|
| 7.5.2.1 | `rterm-app/src/views/sftp.rs`（可抽 `sftp_task.rs`） | SFTP 远端 I/O 全部后台化：新增命令通道（Navigate / Refresh / Mkdir / Rename / Delete / Chmod / Home）与事件回传；`SftpView` 只发命令、收事件，不再直接调用 `SftpService` | 连接慢速/不可达主机时 UI 仍可切换 Tab、时钟继续走；操作完成后列表正确 |
| 7.5.2.2 | `views/sftp.rs`、`rterm-sftp/src/queue.rs` | 修 SFTP Tab 关闭泄漏：事件转发任务不得持有 `Arc<TransferQueue>`；`shutdown()` 释放 sender 并用后台回收线程 join worker（避免 UI 卡在 libssh2 阻塞调用上）；`SftpView::shutdown` 清理事件任务 | 连续开关 10 个 SFTP Tab（含 0 个和 2 个传输），线程数回到基线；无残留 SSH 连接 |
| 7.5.2.3 | `rterm-sftp/src/queue.rs` | 真并发：worker 不再在 `Mutex<Receiver>` 临界区内阻塞 `recv`；改为每 worker 独立 receiver 或单 dispatch + 线程池；默认并发 2 必须真实生效 | 两个 100MB 文件同时传输，两条进度均持续增长（不是先后完成） |
| 7.5.2.4 | `rterm-sftp/src/queue.rs` | 安全提交与取消：下载写 `*.part`，成功后 rename；上传写远端临时名，成功后 rename；取消/失败只清理临时文件；覆盖目标前必须确认 | 取消下载不破坏本地原文件；取消上传不删除远端原文件；失败后无半截目标文件 |
| 7.5.2.5 | `rterm-ssh/src/pty.rs`、`views/terminal.rs` | PTY 输出改有界队列或按帧合并，明确 UI 消费慢时的策略（优先保证尾部和输入响应）；禁止 unbounded 无限堆积 | `yes \| head -c 100M` 或 `cat 50MB` 时内存稳定、UI 不冻结、Ctrl+C 仍可中断 |
| 7.5.2.6 | `rterm-ssh/src/pty.rs` | 用 libssh2 socket readiness（`block_directions` + poll/select）或可中断等待替代固定 8ms 轮询；保留 Close/输入的及时响应 | 空闲终端 Tab CPU 接近 0；按键回显延迟不高于现状 |
| 7.5.2.7 | `views/session_tree.rs` | `cx.subscribe` 从 `Render::render` 移到 `SessionTreeView::new()`，只订阅一次 | 100 次重绘后，单个事件只触发 1 次处理（加计数日志或测试） |
| 7.5.2.8 | `views/terminal.rs` | `Render::render` 只读快照；`measure_font` / `sync_grid_size` 移到 layout/prepaint 或显式 resize 处理；禁止在 render 内 emit 事件 | resize 不再重入 notify；`stty size` 与实际网格一致 |

---

## 7.5.3 状态与交互正确性（P2 必修）

| # | 文件 | 要做的 | 验收 |
|---|------|--------|------|
| 7.5.3.1 | `views/status_bar.rs` | 订阅 `StatusBarMessage`：左侧消息按 `duration_ms` 自动清除；连接/断开消息优先于临时提示 | SFTP 下载/删除等操作能在底部看到提示并自动消失 |
| 7.5.3.2 | `views/terminal.rs` / `views/main_window.rs` | 关闭 Tab 时复位会话状态点：emit `SessionDisconnected`（如 `reason = "标签已关闭"`）或由 MainWindow 处理 `TerminalClosed` | 关闭终端 Tab → 会话树状态点从实心绿变回空心 |
| 7.5.3.3 | `views/session_dialog.rs`、`state.rs` | 设置接线：新建会话默认端口/用户名读取 `Settings`；`data_dir` 作用于 sessions/导出路径（settings.json 固定配置目录，避免 bootstrap 循环）；`sftp_sync_mode` / 监控阈值在 UI 上标注“S8/S9 生效” | 设默认端口 2222 → 新建对话框为 2222；设 data_dir → sessions.json 写入该目录 |
| 7.5.3.4 | `views/session_dialog.rs` | 会话表单校验：名称/主机必填，端口 1–65535，错误内联显示；取消静默回退 22 | 空名称/空主机/非法端口无法保存，并显示明确错误 |
| 7.5.3.5 | `views/sftp.rs` | `~` 解析改用 `SftpService::home_dir()`（或缓存 home）；`~` 与 `~/sub` 都相对 home | 地址栏输入 `~` → 远端 home；`~/tmp` → home/tmp |
| 7.5.3.6 | `views/terminal.rs`、`rterm-terminal/src/engine.rs` | 滚动条使用真实历史总量（`history_size` / `display_offset` 上限），不再用配置的 scrollback 代替 | 5 行历史与 1000 行历史下 thumb 尺寸/位置均正确 |
| 7.5.3.7 | `views/terminal.rs` | 重连不重建 `TerminalEngine`：保留当前屏幕与 scrollback，仅新连接时重置 | `seq 1 100` 后断网重连，历史仍可回滚 |
| 7.5.3.8 | `views/text_input.rs` | 补 Ctrl+V；鼠标点击定位光标；IME `new_selected_range_utf16` 生效；选区颜色走主题，不再硬编码 `rgba(0x3311ff30)` | 地址栏可粘贴路径；点击可定位光标；中文输入候选位置正确 |
| 7.5.3.9 | `views/main_window.rs`、`views/sftp.rs` | 快捷键尊重内部弹窗：SFTP 重命名/新建目录对话框打开时，Ctrl+T / Ctrl+W 不生效 | 对话框内按 Ctrl+W 不会关闭整个 Tab |
| 7.5.3.10 | `views/main_window.rs`、`views/sftp.rs` | 破坏性操作二次确认：会话删除、SFTP 本地/远端删除、上传/下载覆盖同名文件 | 取消确认后目标不变；确认后才执行 |
| 7.5.3.11 | `rterm-terminal/src/engine.rs`、`rterm-ssh/src/pty.rs` | 终端协议补全：DECCKM 应用光标模式下方向键发 SS3；`EventProxy` 处理 `Event::PtyWrite`（DSR/DA 等应答写回 PTY） | `vim` 方向键正常；`printf '\033[6n'` 能得到光标位置响应 |
| 7.5.3.12 | `rterm-session/src/openssh.rs` | OpenSSH 解析修正：顶层 `Include`、同块重复键取第一个、`Host a b` 多模式、ProxyJump 用跳板机自己的 host/port、`Match` 块跳过 | 用真实 `~/.ssh/config` 样例导入，Host/Port/User/IdentityFile/ProxyJump 正确 |
| 7.5.3.13 | `views/session_tree.rs` | 文件夹按 `folder.split('/')` 建树，支持“生产/数据库”多级嵌套 | 设置 `生产/数据库` → 树中显示两级文件夹 |

---

## 7.5.4 主机密钥安全收口（P0/P1）

| # | 文件 | 要做的 | 验收 |
|---|------|--------|------|
| 7.5.4.1 | `rterm-ssh/src/known_hosts.rs`、`views/terminal.rs`、`views/main_window.rs` | 未知主机不再静默 TOFU：弹窗展示 host + fingerprint，提供“接受并保存 / 仅本次 / 拒绝”；密钥变更必须阻断并展示旧/新指纹，提供明确的处理入口 | 首次连接出现确认弹窗；拒绝后不写入 known_hosts；变更时无法静默连接 |
| 7.5.4.2 | `rterm-ssh/src/known_hosts.rs` | 写入原子化 + 进程内互斥：tmp→rename；多 Tab 并发首次连接不互相覆盖；保留原文件内容与格式 | 两个 Tab 同时首次连接两个新主机 → 两条记录都存在；写入失败不破坏原文件 |

---

## 7.5.5 工程质量门禁

| # | 文件 | 要做的 | 验收 |
|---|------|--------|------|
| 7.5.5.1 | 全 `rterm-app` | 清理 23 处 `.lock().unwrap()`：统一 `lock_store()` 辅助，poison 时记录并恢复/重建；增加 panic hook，在崩溃前尝试落盘会话 | `rg "\.lock\(\)\.unwrap\(\)" crates/rterm-app` 为 0（或仅白名单）；故意污染锁不导致进程直接退出 |
| 7.5.5.2 | workspace | 质量门禁：修复 `text_input.rs` 的 `5..2` reversed range；`cargo fmt --all`；新增 CI（GitHub Actions 或本地 `scripts/check.ps1`）跑 test + clippy + fmt | `cargo test --workspace`、`cargo clippy --workspace --all-targets`、`cargo fmt --check` 全绿 |
| 7.5.5.3 | `rterm-app/src/event.rs`、`state.rs`、`rterm-monitor`、`rterm-sync` | 死代码/死事件处理：删除或接入 `TabRegistry`；为 `TerminalData` / `MonitorSample` / `HostKey*` 标注消费方或移除；monitor/sync 占位 crate 保持可编译但不得在 README 宣称已实现 | 代码中不再有“无人订阅且无计划”的事件；`TabRegistry` 不再悬挂 |
| 7.5.5.4 | workspace `Cargo.toml` / 各 crate | 清理未使用依赖：`thiserror`（session/ssh/sftp）、`bytes`（ssh）、未直接使用的 `tokio`、`parking_lot`、`tokio-util` | `cargo build --workspace` 通过，`cargo udeps`（或人工核对）无未使用项 |
| 7.5.5.5 | `views/text_input.rs`、`views/sftp.rs`、`views/terminal.rs`、`theme.rs` | 硬编码颜色/尺寸收敛：选区色、透明行背景、终端哨兵色进入主题或局部常量；侧栏/工具栏/标签栏尺寸统一从 `views/mod.rs` 读取 | 非 `theme.rs` 中不再出现裸 `rgb/rgba` 颜色值（终端调色板除外） |
| 7.5.5.6 | 各模块 `tests/` / `#[cfg(test)]` | 补回归测试：损坏 sessions.json 恢复、单条解密失败、口令 sentinel、队列取消不动原文件、`normalize_remote("~")`、SessionTree 单订阅、TransferQueue 并发、重连保留 scrollback | 新增测试全部通过；测试数与 CHANGELOG 记录一致 |

---

## 7.5.6 文档同步与验收

| # | 文件 | 要做的 | 验收 |
|---|------|--------|------|
| 7.5.6.1 | `README.md`、`docs/planning/README.md`、`roadmap.md`、`CHANGELOG.md`、`architecture/decisions.md` | 文档与实现对齐：修正 README 功能状态、roadmap 勾选、测试数量；ADR-006/007/009 要么按文档实现，要么改文档；记录本 Sprint 的取舍 | 文档中不再出现“已实现但实际为 stub”的描述；ADR 与代码一致 |
| 7.5.6.2 | 全项目 | 文末回归清单逐项执行并记录结果；未通过项转 S8/S9/S10 并写明原因 | 回归清单全绿或遗留项有明确去向 |

---

## 明确不在本 Sprint

| 项 | 去向 |
|----|------|
| SFTP 拖拽上传/下载、暂停/取消 GUI E2E、文件列表虚拟化（大目录性能） | S10 稳定性收尾 |
| `rterm-sync` 路径同步核心、`rterm-monitor` 采集与 UI | S8 / S9 |
| Windows 安装包、i18n、发布流程 | S11 |
| UI 视觉重设计、布局改版、新功能 | 不在本期，也不应顺手做 |

---

## 回归验收清单（Sprint 7.5 完成门禁）

- [ ] 新建 / 编辑 / 删除 / 导入会话后强杀进程，重启数据不丢
- [ ] 损坏 `sessions.json` 启动不覆盖原文件，且给出提示
- [ ] 私钥会话编辑不丢口令；DPAPI 用户级加解密可用
- [ ] 双开 rterm 不会互相覆盖会话文件
- [ ] 慢速/不可达主机下 SFTP 操作不冻结 UI
- [ ] 开关 SFTP Tab 无线程/连接泄漏；取消传输不破坏原文件
- [ ] 两个大文件可真实并行传输
- [ ] 大输出（`cat` / `yes`）内存稳定、Ctrl+C 可中断
- [ ] 空闲终端 CPU 接近 0；关闭 Tab 后状态点复位
- [ ] 设置中的默认端口/用户名/data_dir 真正生效
- [ ] `~` 导航、滚动条、重连 scrollback 正确
- [ ] 未知/变更主机密钥有明确确认与阻断
- [ ] `cargo test --workspace`、`cargo clippy --workspace --all-targets`、`cargo fmt --check` 全绿
- [ ] 文档、ADR、README 与实现一致

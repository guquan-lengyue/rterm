# Sprint 12 — 终端内嵌 SFTP 侧栏（v1.1）

> **状态**：🚧 进行中（12.1–12.7 已落地，待 12.8 回归）
> **前置**：[Sprint 8](./sprint-08-sync.md) ✅（SyncBridge / `TerminalCwdHint`）+
> [Sprint 10](./sprint-10-stability.md) ✅（SftpView 拖拽 / 虚拟化 / 后台读取）+
> [Sprint 11](./sprint-11-release.md) 基线
> **目标**：在**同一个终端 Tab 内**把 SFTP 面板停靠在终端右侧，并让 SFTP 远端路径
> **自动跟随终端当前目录**；不再需要为看文件单独开一个 SFTP Tab。
>
> **进度（2026-11-17）**
> - 12.1 ✅ `SftpDisplayMode::{Tab, Dock}`；Dock 隐藏顶部工具条、只显示远端栏、宽度由父容器驱动
> - 12.2 ✅ 侧栏首次显示才创建并连接
> - 12.3 ✅ `TerminalTabView` 组合视图 + 6px 可拖拽分隔条（280–720）+ 右上角开关
> - 12.4 ✅ 路径跟随：`TerminalCwdHint` 按 tab_id 过滤 + 同步模式门控 + `expect_sftp_navigation` 防回环；
>   侧栏连接完成后补跳最近 cwd，重新打开侧栏立即定位
> - 12.5 ✅ `MainWindow` 全面切换到 `TabContentView::Terminal(Entity<TerminalTabView>)`（focus / 关闭 / 快捷键阻塞转发）
> - 12.6 ✅ 入口：终端头部开关按钮 + `Ctrl+Shift+S`（`ToggleSftpDock` action，终端不再把该组合转发远端）
> - 12.7 ✅ 持久化：设置 → 终端外观「SFTP 侧栏」默认显隐；分隔条拖拽结束保存宽度，新 Tab 沿用
> - 12.8 ✅ 实机 UI 验证（2026-11-17，真实 Ubuntu 主机）：
>   侧栏布局 / 分隔条拖拽（460→620px 已落盘）/ `cd /tmp`、`cd /var/log` 跟随 /
>   隐藏后重开回到最近路径 / `Ctrl+Shift+S` 开合且不向远端发 Ctrl+S / 新建 Tab 按设置自动带侧栏 /
>   老 Tab 不受新设置影响 / 双向模式无 `cd` 回环注入。
>   期间修复：设置弹窗高度不足把 footer 挤出卡片导致「保存」不可点击（卡片 680 + 内容区 `min_h(0)`）。
> - README 用户文档已补充侧栏说明与 `Ctrl+Shift+S` 快捷键（S12 收尾）
> - 自适应布局（2026-11-17 追加）：`adaptive_dialog_size` 让设置 / 会话编辑 / 导入导出
>   三个弹窗随窗口收缩（四周留 24px），内容超出改由原生 `overflow_y_scroll` 滚动；
>   切换 Tab 复位滚动位置；终端 SFTP 侧栏宽度上限改为内容区 45%，窄窗口自动收窄
> - 滚动修复（2026-11-17 追加）：Taffy 默认 `flex-shrink: 1` 会把滚动容器的唯一子项压到
>   视口高度（`content_size == viewport_size` → 可滚距离 0），三个弹窗统一用
>   `div().w_full().flex_shrink_0()` 包裹内容；实机验证 600×433 / 900×600 窗口下
>   设置表单、会话表单、导入预览均可滚动且 footer 始终可见
> - 列对齐修复（2026-11-18）：SFTP 表头与数据行原先各自用「预计算 name_w + 可收缩定宽列」，
>   Dock 模式实际宽度被 45% 上限收窄后两者收缩比例不同 → 表头错位。改为表头/行共用
>   `COL_*_W` 常量：名称列 `flex_1 + min_w(0)` 填充、其余列 `flex_none` 不收缩；
>   同时修复 Dock 模式面板宽度写死 50% 只占侧栏一半的问题，并移除 `SftpView::set_dock_width`

---

## 需求与验收

| # | 需求 | 验收 |
|---|------|------|
| R1 | 终端 Tab 右侧可显示 SFTP 侧栏（可折叠），终端仍占主区域 | 打开终端 Tab → 侧栏出现在右侧；隐藏后终端占满 |
| R2 | SFTP 路径跟随终端 cwd | 终端 `cd /var/log` → 侧栏 ≤1s 跳到 `/var/log`；`cd ~` / 相对路径同样正确 |
| R3 | 跟随行为受现有「同步模式」约束 | `off` 不跟随；`term_to_sftp` / `bidirectional` 跟随；`sftp_to_term` 不跟随 |
| R4 | 每个终端 Tab 独立侧栏，互不串扰 | 两个 Tab 各自 `cd`，侧栏各自跳转；关闭一个不影响另一个 |
| R5 | 侧栏可完成常用文件操作 | 双击目录进入、双击文件下载、拖拽上传、右键菜单可用 |
| R6 | 按需连接，不额外浪费连接 | 侧栏未打开时不建立 SFTP 连接；首次打开才连接，失败只在侧栏提示 |
| R7 | 布局可调节并记忆 | 拖拽分隔条 280–720px；宽度与「默认显示侧栏」写入设置，重启生效 |

## 架构设计（分层）

```
TerminalTabView            ← 新增：组合视图（终端 + 可选 SFTP 侧栏 + 分隔条）
├── TerminalView           ← 现有：只负责终端渲染 / PTY / cwd 解析
└── SftpView (Dock 模式)   ← 现有：新增 compact 模式，隐藏自身工具条
```

- **职责边界**：`TerminalTabView` 只做布局、开关、宽度与事件路由；不改 Terminal / Sftp 内部逻辑。
- **路径跟随**：复用 `TerminalCwdHint { tab_id, cwd, .. }`（S8 已交付）。`TerminalTabView`
  按自己的终端 `tab_id` 过滤后调用 `SftpView::navigate_to_remote()`；导航前在
  `SyncBridge` 标记 `pending_sftp_nav`，避免侧栏自身的 `SftpNavigated` 反向注入 `cd` 造成回环。
- **Dock 模式**：`SftpView` 增加 `SftpDisplayMode::{Tab, Dock}`；Dock 隐藏顶部「SFTP 文件管理」
  工具条与连接状态大标题，保留面板头 / 地址栏 / 面包屑 / 文件列表 / 传输面板。
- **连接策略**：`SftpView` 延迟 `connect()`，由 `TerminalTabView` 在首次显示侧栏时触发；
  关闭 Tab 时按现有 `shutdown()` 释放。
- **焦点**：终端与侧栏各自 FocusHandle；`TerminalTabView::focus_handle()` 默认转发给终端，
  侧栏地址栏获得焦点时不劫持终端按键。
- **事件**：仍走 EventBus；`MainWindow` 的 `TabContentView::Terminal` 改为持有
  `Entity<TerminalTabView>`，focus / blocks_global_shortcuts / close 统一转发。

## 任务拆分

| # | 粒度 | 文件 | 要写的 | 验收 |
|---|------|------|--------|------|
| 12.1 | 0.5h | `views/sftp.rs` | 新增 `SftpDisplayMode::{Tab, Dock}` 与 `with_mode()`；Dock 模式隐藏工具条（保留 panel 头 / 地址栏 / 传输面板） | 现有 SFTP Tab 行为不变；Dock 宽度跟随父容器 |
| 12.2 | 0.5h | `views/sftp.rs` | 延迟连接：抽出 `connect_if_needed()`，首次显示侧栏时由外部调用；错误只在侧栏内显示 | 未打开侧栏不产生 SFTP 连接（日志可验证） |
| 12.3 | 1h | `views/terminal_tab.rs`（新） | `TerminalTabView`：`Entity<TerminalView>` + `Option<Entity<SftpView>>` + 宽度；水平 split + 可拖拽分隔条（clamp 280–720） | 终端 + 侧栏布局正确；分隔条可拖 |
| 12.4 | 0.5h | `views/terminal_tab.rs` | 路径跟随：订阅 `TerminalCwdHint`（按 tab_id 过滤）→ 模式门控 → `navigate_to_remote` + `pending_sftp_nav` | `cd /var/log` 侧栏跳转；off 模式不跳；无回环 `cd` |
| 12.5 | 0.5h | `views/terminal_tab.rs` + `main_window.rs` | `TabContentView::Terminal` 改为组合视图；focus / 关闭 / 阻塞快捷键转发 | 关闭 Tab 时终端与侧栏都释放；侧栏弹窗可阻塞全局快捷键 |
| 12.6 | 0.5h | `views/terminal_tab.rs` + `main_window.rs` + `main.rs` | 开关入口：终端头部按钮 + `Ctrl+Shift+S`；`ToggleSftpDock` action + keybinding | 按钮 / 快捷键都能开合侧栏，终端按键不受影响 |
| 12.7 | 0.5h | `state.rs` + `views/settings.rs` | 设置项：`terminal_sftp_dock_default`（默认关）+ `terminal_sftp_dock_width`；保存后新 Tab 生效 | 重启后默认显示与宽度保持 |
| 12.8 | 1h | `views/terminal_tab.rs` + 文档 | 回归：多 Tab / 主题切换 / 高 DPI / 拖拽上传 / 下载 / 暂停取消 / vi 模式；补 smoke 清单与 CHANGELOG | 全量 smoke 通过；workspace 测试全绿 |

## 风险与规避

| 风险 | 影响 | 规避 |
|------|------|------|
| 每 Tab 两条 SSH 连接（PTY + SFTP） | 服务器连接数上限 / 资源 | 侧栏按需连接；关闭侧栏可 `shutdown` 释放；文档说明 |
| `TerminalCwdHint` 全局广播串扰 | 侧栏跳错目录 | 严格按 `tab_id` 过滤，`TerminalTabView` 只处理自己的终端事件 |
| 侧栏导航反向注入 `cd` | 双向模式回环 | 沿用 `pending_sftp_nav` + `ignore` 列表；Dock 默认只做 terminal→SFTP |
| 分隔条与终端鼠标选择冲突 | 拖拽误触发选区 | 分隔条独立 6px 热区，按下时捕获鼠标、不传给终端 |
| SftpView 内部 emit 的全局事件 | 其他 Tab 误解 | 事件带 `tab_id`，消费方按 tab 过滤（沿用现有约定） |

## 交付定义

- 终端 Tab 右侧可开合 SFTP 侧栏，路径跟随终端 cwd（受同步模式门控、无回环）
- 多 Tab 独立、按需连接、宽度与默认显示可持久化
- `cargo fmt` / `cargo clippy --workspace --all-targets`（0 warning）/ `cargo test --workspace` 全绿
- README 增加「终端内嵌 SFTP 侧栏」使用说明；CHANGELOG 记录 Sprint 12

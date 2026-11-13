# rterm — AI 开发手册（README）

> **这份文档的读者是你——负责写 rterm 代码的 AI。** 它不是给人看的，是给你在每次 tool call 前快速扫一眼避免走歪的。

---

## 1. 立即要读的 3 份

开工顺序：

1. **[`planning/README.md`](./planning/README.md)** — 状态总览 + 依赖图 + 当前 Sprint 快速跳转
2. **[`architecture/architecture.md`](./architecture/architecture.md)** — 核心数据模型、事件总线枚举、crate 职责
3. **[`product/overview.md`](./product/overview.md)** — 功能边界，确认你没做 MVP 外的东西

ADR 决策记录 `architecture/decisions.md` 是排错时翻的，不是必读。

---

## 2. 项目快照

```
rterm/
├── Cargo.toml              # workspace root, resolver=2
├── scripts/msvc-env.ps1    # ⚠️ 每次 cargo build 前必须先 . 这个
└── crates/
    ├── rterm-app          # 入口 binary：gpui Application、主窗口、AppState、EventBus
    ├── rterm-crypto       # lib：dpapi_encrypt/decrypt, aesgcm_encrypt/decrypt
    ├── rterm-session      # lib：Session 模型、SessionStore、导入导出、OpenSSH config
    ├── rterm-ssh          # lib：SshSession, OpenPtyResult, keepalive, 重连状态机
    ├── rterm-terminal     # lib：TerminalEngine 包装 alacritty_terminal
    ├── rterm-sftp         # lib：SftpService, TransferQueue, TransferTask
    ├── rterm-monitor      # lib：MonitorCollector, ResourceSample
    └── rterm-sync         # lib：SyncBridge, SyncMode, CwdSource
```

**关键约束**：

| 约束 | 原因 | 违反后果 |
|------|------|----------|
| `alacritty_terminal::Term` 是 `!Send + !Sync` | Rc 内部状态 | 必须在 gpui 主线程 |
| `ssh2::Session` / `Channel` 全是同步的 | C library | 所有 IO 必须 `spawn_blocking` |
| gpui 渲染在 `Render::render(&mut self, window, cx)` | bezel-gpui 0.3 trait | 所有 UI 更新靠 `cx.notify()` 触发重绘 |
| EventBus 是**空 struct + `impl EventEmitter<AppEvent>`**（S1 实际落地） | gpui marker trait，内部 SubscriberSet 自动管理 | emit: `app.emit(&state.event_bus, ev)`；subscribe: `cx.subscribe(&state.event_bus, handler)` |

---

## 3. Shell 命令（每次 build 前）

```powershell
# 激活 MSVC + CMake 环境（PATH 里要有 cl.exe, cmake）
. .\scripts\msvc-env.ps1

# 构建（按 crate）
cargo build -p rterm-app
cargo build -p rterm-crypto --tests

# 构建（全部）
cargo build

# 清理 + 全量重编
cargo clean; cargo build

# 单元测试
cargo test -p rterm-crypto

# 运行
.\target\debug\rterm-app.exe
```

---

## 4. bezel-gpui 0.3 常用 API 速查

### 入口

```rust
// main.rs
gpui_platform::application().run(|app: &mut App| {
    app.open_window(WindowOptions::default(), |_window, app| {
        app.new(|cx| MyRootView)
    });
});
```

### Render trait

```rust
use gpui::prelude::*;                     // 必须，否则 Styled/ParentElement trait 不在作用域

impl Render for MyView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div().size_full().bg(rgb(0x1e1e1e))
            .child(div().size_full().flex()
                .items_center().justify_center()
                .text_color(rgb(0xcccccc))
                .text_size(px(18.))
                .child("Hello"))
    }
}
```

### 事件订阅

```rust
// View 订阅 EventBus
cx.subscribe(&event_bus, |app: &mut App, ev: &AppEvent| {
    if let AppEvent::SessionConnected { id } = ev { /* ... */ }
});
```

### 异步（后台线程 → gpui 主线程）

```rust
// 后台线程跑 ssh2
let rx = spawn_blocking(move || { ssh_session.open_pty().unwrap() });

// 发回主线程（在 gpui 里）
cx.spawn(async move {
    if let Ok(mut rx) = rx {
        while let Ok(bytes) = rx.recv().await {
            engine.input_bytes(&bytes);
            cx.notify();           // 触发 render
        }
    }
});
```

### 常用 trait import（每次 render 前检查）

```rust
use gpui::prelude::*;                       // IntoElement, Element, Styled, ParentElement
use gpui::{App, AppContext, Context, Window, WindowOptions, Render};
```

---

## 5. 编码规范（本项目）

- **错误处理**：Service 层用 `anyhow::Result`，View 层用 `AppEvent::{XxxFailed}`，两层之间 `.map_err()`
- **不阻塞主线程**：ssh2 IO、SFTP 传输、监控采集**必须** `spawn_blocking`
- **文件路径**：跨平台路径一律 `PathBuf`，用户配置目录用 `dirs::data_dir()`
- **加解密**：本地用 `dpapi_encrypt`；导出用 `aesgcm_encrypt` + 用户口令
- **日志**：`log::info!` / `log::debug!`（dev 模式 RUST_LOG=debug）
- **常量**：magic number（30s keepalive、10000 scrollback）提到 `Settings` 结构体里

---

## 6. 不要做的事

| ❌ 不要做 | 原因 |
|-----------|------|
| 在 View 里直接调用 ssh2 | View 层只订阅 EventBus，不碰业务逻辑 |
| 在 `Render::render` 里做耗时操作 | render 会被频繁调用（每帧），只能读状态 |
| 给 `Term<SizeInfo>` 加 Send/Sync 包装 | alacritty_terminal 故意 !Send，尊重它 |
| MVP 阶段加 X11 转发 / 多用户协同 / AI 助手 | docs/product/overview.md 1.2 明确砍掉了 |
| 改 EventBus 的事件枚举时不更新所有 View 的订阅 | 一次改全，留孤儿事件会忘 |

---

## 7. docs/ 目录结构

```
docs/                                ← 你在这里
├── README.md                        ← 本文件
├── product/                         ← WHAT 做什么
│   ├── README.md                    ← product 目录索引
│   └── overview.md                  ← 项目定位 + M1–M6 功能列表
├── architecture/                    ← HOW 怎么实现
│   ├── README.md                    ← architecture 目录索引 + 快速查找表
│   ├── architecture.md              ← 分层架构 + crate 职责 + 数据流
│   └── decisions.md                 ← 10 条 ADR 技术决策
└── planning/                         ← WHEN 交付节奏
    ├── README.md                    ← ⭐ 每次开工先读：状态总览 + 依赖图
    ├── CHANGELOG.md                 ← 每个 Sprint 完成后追加总结
    ├── roadmap.md                   ← Phase 0–7 里程碑 + 风险缓解
    ├── sprint-01-infrastructure.md  ← 每个 Sprint 一个独立文件
    ├── sprint-02-session-data.md
    ├── sprint-03-session-tree.md
    ├── sprint-04-settings-io.md
    ├── sprint-05-ssh-terminal.md
    ├── sprint-06-terminal-render.md
    ├── sprint-07-sftp.md
    ├── sprint-08-sync.md
    ├── sprint-09-monitor.md
    ├── sprint-10-stability.md
    └── sprint-11-release.md
```

## 8. 变更记录

- **2026-11-13 v2.0**：sprints.md（35KB 大文件）拆成 10 个 per-sprint 独立文件；新增 3 个子目录 README 索引；根 README 加完整目录树
- **2026-11-13 v1.0**：首次创建，面向 AI 开发者

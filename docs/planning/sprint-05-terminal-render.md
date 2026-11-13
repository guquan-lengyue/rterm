# Sprint 5 — 终端渲染

> **状态**：—
> **前置**：[Sprint 4](./sprint-04-ssh-terminal.md)（TerminalEngine + PTY 字节流；Perl ✅ 已完成）
> **目标**：gpui Canvas 渲染 Grid + 主题 + 复制粘贴 + 标签页

---

### 5.1 gpui Canvas 渲染

| # | 粒度 | 文件 | 要写的 | 验收 |
|---|------|------|--------|------|
| 5.1.1 | 1h | `crates/rterm-app/src/views/terminal.rs` | `pub struct TerminalView { engine: TerminalEngine, font_size: f32 }` + `impl Render` 逐 Grid<Cell> 画背景矩形 + 文字（DirectWrite）；font fallback 处理 CJK / Nerd Font | 能显示真实远程 shell 文字 |
| 5.1.2 | 0.5h | `crates/rterm-app/src/views/terminal.rs` | 光标：方块 / 下划线 / 竖线；blink（tokio 500ms）；覆盖当前 Cell | 光标闪烁位置正确 |
| 5.1.3 | 0.5h | `crates/rterm-app/src/views/terminal.rs` | 滚动缓冲：保留 scrollback_lines；滚动条 + 鼠标滚轮 + Shift+PageUp/PageDown | 回滚能看历史 |
| 5.1.4 | 0.5h | `crates/rterm-app/src/views/terminal.rs` | 文本选择：鼠标左键拖 → 高亮选区 → 松开自动复制 `gpui::App::set_clipboard` | 拖一段 → 粘贴到别的终端能看到 |

### 5.2 粘贴 + 键盘

| # | 粒度 | 文件 | 要写的 | 验收 |
|---|------|------|--------|------|
| 5.2.1 | 0.25h | `crates/rterm-app/src/views/terminal.rs` | 右键粘贴：`App::get_clipboard()` → PTY channel.write；bracketed paste mode 保护 | 粘贴多行不破坏 |
| 5.2.2 | 0.5h | `crates/rterm-app/src/views/terminal.rs` | 键盘映射：gpui KeyDown → TerminalKey enum（Enter, Backspace, Arrow, CtrlC/D/Z, F1–F12, Home/End, PageUp/Down）→ engine.input_key → PTY write | Ctrl+C 能中断；方向键能移动 vim 光标 |

### 5.3 主题

| # | 粒度 | 文件 | 要写的 | 验收 |
|---|------|------|--------|------|
| 5.3.1 | 0.5h | `crates/rterm-app/src/theme.rs` | `pub struct Theme { pub name, pub fg, pub bg, pub ansi16: [Rgb; 16], pub cursor }` + 内置 4 套：Solarized Dark/Light, Dracula, One Dark | 4 个 Theme 都能构造 |
| 5.3.2 | 0.25h | `crates/rterm-app/src/theme.rs` | Theme 切换：`EventBus::AppThemeChanged { name }` → TerminalView 订阅 → 重绘 | 设置里切 Dracula → 终端立刻变色 |

### 5.4 标签页

| # | 粒度 | 文件 | 要写的 | 验收 |
|---|------|------|--------|------|
| 5.4.1 | 0.5h | `crates/rterm-app/src/views/tab_bar.rs` | TabBar View：水平 `TabInfo` 列表 + 关闭按钮（✕）+ 激活高亮；右键菜单：关闭 / 关闭其他 / 关闭右侧 | 能 tab 切换 |
| 5.4.2 | 0.5h | `crates/rterm-app/src/main.rs` | TabRegistry 管理：SessionConnectRequested → 新建 Terminal Tab + 打开 SSH + PTY；Tab 关闭 → channel.disconnect + engine drop | 关 tab → SSH 断开；重开 tab → 重连 |
| 5.4.3 | 0.25h | `crates/rterm-app/src/main.rs` | 快捷键：Ctrl+T 新标签（同会话）/ Ctrl+Tab 切换 / Ctrl+W 关闭；gpui keybinding 框架 | 快捷键生效 |

### 5.5 字体缩放 + 状态栏

| # | 粒度 | 文件 | 要写的 | 验收 |
|---|------|------|--------|------|
| 5.5.1 | 0.25h | `crates/rterm-app/src/views/terminal.rs` | Ctrl+/Ctrl-/Ctrl+0 → font_size 加减 / 重置；保存到 settings.json | 改字号不重连立刻生效 |
| 5.5.2 | 0.25h | `crates/rterm-app/src/views/status_bar.rs` | 真实状态栏：当前主机 + 延迟（ping）+ 重连次数 + 时钟 | 底栏实时显示 |

### 5.6 验收

| # | 粒度 | 验收 |
|---|------|------|
| 5.6.1 | 0.5h | vim / nano / htop / tmux / less 全部能操作；三标签 Ctrl+Tab 切换；Shift+PageUp 回滚；右键复制粘贴；切 Dracula 主题；Ctrl+T 新标签 Ctrl+W 关闭 |

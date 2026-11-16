# Sprint 6 — 终端渲染

> **状态**：⏳ **下一个**（S5 已完成，2026-11-14）
> **前置**：[Sprint 5](./sprint-05-ssh-terminal.md)（TerminalEngine + PTY 字节流 + TerminalView 连接编排；Perl ✅ 已完成）+ [Sprint 4](./sprint-04-settings-io.md)（终端外观 Settings + UI 主题）
> **目标**：gpui Canvas 渲染 Grid + 主题 + 复制粘贴 + 标签页
>
> **S5 已就绪 / 本 Sprint 只替换渲染层**：
> - `TerminalEngine` 已提供 `grid()` / `cursor()` / `colors()` / `visible_lines()` / `input_key()` / `resize()`
> - `TerminalView` 已有完整连接编排（SSH 线程、PTY 事件、重连、状态、键盘 → `PtyCommand`）→ 保留，只把文本 body 换成 Canvas
> - 会话树已有连接状态点；S6 不需要重复实现
>
> **S4 已就绪 / 本 Sprint 需补的缺口**：
> - `Settings` 已有 `default_theme` / `default_font_family` / `default_font_size` / `terminal_cursor_style` / `terminal_scrollback_lines` → 直接读取，不再新增配置项
> - 已有 4 套 **UI 主题**（Catppuccin Mocha / Windows 11 Light / Dracula / One Dark），但 `Theme` trait 只有控件颜色 → 本 Sprint 新增 `TerminalPalette`（fg / bg / ANSI 16 / cursor），**不要重造控件主题**
> - `AppThemeChanged` / `AppSettingsChanged` 已定义，设置保存时会 emit；`Settings::save()` 已可原子写盘
> - 字体仍是 4 个预设，系统字体枚举未做（S4 遗留，可维持预设或在 5.5 补）
> - 字号快捷键改完需写回 `Settings.default_font_size` 并调用 `Settings::save()`

---

### 5.1 gpui Canvas 渲染

| # | 粒度 | 文件 | 要写的 | 验收 |
|---|------|------|--------|------|
| 5.1.1 | 1h | `crates/rterm-app/src/views/terminal.rs` | 在 S5 现有 `TerminalView` 上把文本 body 替换为 gpui Canvas：逐 Grid<Cell> 画背景矩形 + 文字（DirectWrite）；字体族 / 字号读 `Settings.default_font_family` / `default_font_size`；font fallback 处理 CJK / Nerd Font | 能显示真实远程 shell 文字（颜色正确） |
| 5.1.2 | 0.5h | `crates/rterm-app/src/views/terminal.rs` | 光标：方块 / 下划线 / 竖线，样式读 `Settings.terminal_cursor_style`；blink（tokio 500ms）；覆盖当前 Cell | 光标闪烁位置正确；设置改光标样式后生效 |
| 5.1.3 | 0.5h | `crates/rterm-app/src/views/terminal.rs` | 滚动缓冲：行数读 `Settings.terminal_scrollback_lines`；滚动条 + 鼠标滚轮 + Shift+PageUp/PageDown | 回滚能看历史 |
| 5.1.4 | 0.5h | `crates/rterm-app/src/views/terminal.rs` | 文本选择：鼠标左键拖 → 高亮选区 → 松开自动复制 `gpui::App::set_clipboard` | 拖一段 → 粘贴到别的终端能看到 |

### 5.2 粘贴 + 键盘

| # | 粒度 | 文件 | 要写的 | 验收 |
|---|------|------|--------|------|
| 5.2.1 | 0.25h | `crates/rterm-app/src/views/terminal.rs` | 右键粘贴：`App::get_clipboard()` → PTY channel.write；bracketed paste mode 保护 | 粘贴多行不破坏 |
| 5.2.2 | 0.5h | `crates/rterm-app/src/views/terminal.rs` | 键盘映射：gpui KeyDown → TerminalKey enum（Enter, Backspace, Arrow, CtrlC/D/Z, F1–F12, Home/End, PageUp/Down）→ engine.input_key → PTY write | Ctrl+C 能中断；方向键能移动 vim 光标 |

### 5.3 主题

| # | 粒度 | 文件 | 要写的 | 验收 |
|---|------|------|--------|------|
| 5.3.1 | 0.5h | `crates/rterm-app/src/theme.rs` | 新增 `pub struct TerminalPalette { pub name, pub fg, pub bg, pub ansi16: [Rgb; 16], pub cursor }` + 为 S4 已有 4 套 UI 主题补齐终端调色板（Catppuccin Mocha / Windows 11 Light / Dracula / One Dark）；Solarized Dark/Light 可选追加，不重造控件主题 | 4 套调色板都能构造；终端背景/文字/ANSI 色与主题匹配 |
| 5.3.2 | 0.25h | `crates/rterm-app/src/theme.rs` | 主题切换：`AppThemeChanged { name }`（S4 设置保存时已 emit）→ TerminalView 订阅 → 换 `TerminalPalette` 重绘；`AppSettingsChanged` 同步字体/字号/光标 | 设置里切 Dracula → 终端立刻变色 |

### 5.4 标签页

| # | 粒度 | 文件 | 要写的 | 验收 |
|---|------|------|--------|------|
| 5.4.1 | 0.5h | `crates/rterm-app/src/views/tab_bar.rs` | TabBar View：水平 `TabInfo` 列表 + 关闭按钮（✕）+ 激活高亮；右键菜单：关闭 / 关闭其他 / 关闭右侧 | 能 tab 切换 |
| 5.4.2 | 0.5h | `crates/rterm-app/src/main.rs` | TabRegistry 管理：SessionConnectRequested → 新建 Terminal Tab + 打开 SSH + PTY；Tab 关闭 → channel.disconnect + engine drop | 关 tab → SSH 断开；重开 tab → 重连 |
| 5.4.3 | 0.25h | `crates/rterm-app/src/main.rs` | 快捷键：Ctrl+T 新标签（同会话）/ Ctrl+Tab 切换 / Ctrl+W 关闭；gpui keybinding 框架 | 快捷键生效 |

### 5.5 字体缩放 + 状态栏

| # | 粒度 | 文件 | 要写的 | 验收 |
|---|------|------|--------|------|
| 5.5.1 | 0.25h | `crates/rterm-app/src/views/terminal.rs` | Ctrl+/Ctrl-/Ctrl+0 → font_size 加减 / 重置；写回 `Settings.default_font_size` + `Settings::save()`（S4 已实现原子写盘） | 改字号不重连立刻生效；重启后保留 |
| 5.5.2 | 0.25h | `crates/rterm-app/src/views/status_bar.rs` | 真实状态栏：当前主机 + 延迟（ping）+ 重连次数 + 时钟 | 底栏实时显示 |

### 5.6 验收

| # | 粒度 | 验收 |
|---|------|------|
| 5.6.1 | 0.5h | vim / nano / htop / tmux / less 全部能操作；三标签 Ctrl+Tab 切换；Shift+PageUp 回滚；右键复制粘贴；切 Dracula 主题；Ctrl+T 新标签 Ctrl+W 关闭 |

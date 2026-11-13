# rterm — 技术决策记录 (ADR)

> 本文件记录所有影响项目长期演进的关键技术决策，每条带日期、上下文、选项、结论和后果。

## ADR-001：UI 框架选型 — 选用 bezel-gpui（Zed gpui 活跃 fork）

- **日期**：2026-11-13
- **状态**：已接受
- **上下文**：项目目标是跨平台原生 GPU 加速桌面 GUI。Zed 编辑器的 gpui 库是 Rust 桌面 UI 中唯一成熟的 GPU 加速方案，但其主仓库不在 crates.io，且上游已停止独立发布。
- **选项**：
  - A. bezel-gpui + bezel-gpui-platform 0.3
  - B. 自己 vendoring Zed 主仓库 gpui 目录
  - C. 用 iced / egui / eframe（成熟但 CPU 渲染，非 GPU）
  - D. 用 Tauri + Web（Web 技术栈而非 Rust 原生）
- **决策**：选 **A**。bezel fork 专注独立发布、保持 Windows/macOS/Linux 三平台构建、持续同步 Zed 上游。我们已在本机 MSVC 工具链上成功编译空窗口。
- **后果**：
  - 正面：GPU 加速渲染（DirectX / Metal / Vulkan）、与 Zed 用户共享库生态、gpui-macros / prelude 简化开发
  - 风险：bezel 是第三方 fork，未来若停更则需要切换回 vendoring 或 eframe
  - 缓解：`bezel-gpui` 每月有发布；我们的封装层在 `rterm-app`，未来替换影响面可控

## ADR-002：SSH 协议栈 — 选用 ssh2 (libssh2 bindings) + vendored-openssl

- **日期**：2026-11-13
- **状态**：已接受
- **上下文**：SSH 客户端需要密码/密钥认证、PTY 分配、SFTP 子系统、端口转发、Agent 支持。
- **选项**：
  - A. `ssh2` crate（libssh2）
  - B. `openssh` crate（调用系统 ssh 命令）
  - C. 纯 Rust `async-ssh2` / `russh`
- **决策**：选 **A**。`openssh` crate 需要系统装 ssh，不独立；纯 Rust SSH 库普遍成熟度不足（SFTP / keepalive / 跳板机覆盖不全）。libssh2 是业界标准，vendored-openssl 可避免系统 OpenSSL 依赖。
- **后果**：
  - 正面：成熟稳定、SFTP 原生支持、PTY 全功能、压缩/keepalive/agent 完整
  - 风险：需要 C 工具链（cmake + perl 编译 OpenSSL）、ssh2 crate 是同步的
  - 缓解：在 `rterm-ssh` 里用 `tokio::task::spawn_blocking` 包装所有 ssh2 调用，通过 mpsc channel 与 gpui 主线程桥接

## ADR-003：终端仿真 — 选用 alacritty_terminal 作为后端

- **日期**：2026-11-13
- **状态**：已接受
- **上下文**：需要完整 xterm-256color 兼容（vim / nano / tmux / htop 正常）、真彩色、滚动缓冲、光标形状切换、交替缓冲区。
- **选项**：
  - A. `alacritty_terminal`
  - B. `vt100` crate（功能不全）
  - C. 自写（估计 ~5000 行 + 半年以上测试）
- **决策**：选 **A**。Alacritty 项目的终端核心库，已在多个生产终端（Alacritty 本身、WezTerm 的早期版本）被实机验证。我们只需写 ~300 行 glue：① PTY 字节喂 Term；② Term 的 Grid<Cell> 喂 gpui Canvas 绘制。
- **后果**：
  - 正面：省半年工作量；ANSI 转义序列兼容度 ~99%
  - 风险：`alacritty_terminal::Term` 是 `!Send + !Sync`，必须在 gpui 主线程运行
  - 缓解：SSH PTY IO 在后台线程，读到的字节通过 `cx.spawn()` 回到主线程喂给 Term

## ADR-004：SFTP ↔ 终端路径同步实现 — 方案 C（两者结合 + 方向可配）

- **日期**：2026-11-13
- **状态**：已接受
- **上下文**：用户核心需求——SFTP 文件浏览器与终端 shell 的当前工作目录自动保持同步。
- **选项**：
  - A. 仅 PS1 提示符正则提取
  - B. 后台周期性 `pwd` 命令
  - C. 两者结合（PS1 正则为主，pwd 兜底）+ 方向开关（Off / Terminal→SFTP / SFTP→Terminal / Bidirectional）
- **决策**：选 **C**。PS1 正则绝大多数 shell 配置能命中，但极个别场景（如 tty 内嵌套的 python REPL 或 tmux 后路径不暴露）匹配失败时用 `pwd` 兜底。方向开关给用户完全控制权。
- **后果**：
  - 正面：路径变化感知率接近 100%；用户可按需关闭；SFTP→Terminal 方向我们有 ignore 列表防回环
  - 风险：PS1 正则本身是启发式，复杂 PS1（颜色控制码多）可能漏匹配
  - 缓解：至少有 3 种正则模板（bash default / zsh default / generic），允许用户自定义正则

## ADR-005：敏感字段加密 — Windows DPAPI + 导出文件 AES-GCM

- **日期**：2026-11-13
- **状态**：已接受
- **上下文**：密码、私钥口令、跳板机凭证不能明文存盘。
- **选项**：
  - A. Windows DPAPI（绑定当前登录用户 + 本机）
  - B. 主密码派生 AES 密钥（跨平台统一）
  - C. 两者结合：本地用 DPAPI，导出用 AES-GCM + 用户输入口令
- **决策**：选 **C**。
  - **本地存储**（sessions.json 内的密码）：DPAPI，用户无感，系统级安全
  - **导出文件**（rterm-export.json）：AES-256-GCM + 用户输入口令 + PBKDF2-SHA256 + 16 字节随机 salt
- **后果**：
  - 正面：日常零摩擦；导出文件可跨机器安全迁移
  - 风险：DPAPI 在另一台 Windows 机器上无法解密；主密码丢失无法恢复
  - 缓解：导入导出时强制要求口令；UI 有明显警告提示

## ADR-006：错误处理策略 — Service 层 anyhow + View 层 thiserror 映射

- **日期**：2026-11-13
- **状态**：已接受
- **上下文**：分层架构下错误如何传播。
- **决策**：
  - Service / Infrastructure 层：`anyhow::Result<T>` 方便写
  - Application 层和事件总线：定义 `rterm-app::AppError`（thiserror），在 Service → App 边界做 `.map_err()` 转换
  - View 层：只接收到已经格式好的 `AppEvent::{XxxFailed}`，不再接触原始 Error 对象
- **后果**：
  - 正面：View 层不关心底层 ssh2 错误细节，只看面向用户的错误文案
  - 错误链完整保留，Debug 时用 `.with_context()` 可追溯

## ADR-007：异步线程模型 — ssh2 blocking + tokio mpsc 桥接 gpui 主线程

- **日期**：2026-11-13
- **状态**：已接受
- **上下文**：ssh2 crate 所有调用都是同步的，gpui 有自己的事件循环不能阻塞。
- **决策**：
  - ssh2 操作（connect / open_pty / exec / sftp.put_file）：`tokio::task::spawn_blocking`
  - 后台 blocking 任务与 gpui 主线程之间：`tokio::sync::mpsc::channel` 发消息
  - gpui 侧用 `App::spawn` 或 `Window::spawn` 接收 channel 消息（`await` 在 gpui 主线程的 tokio Runtime 上）
  - **TerminalEngine / SyncBridge** 必须在 gpui 主线程（因为 alacritty_terminal::Term 是 !Send），收到 PTY 字节后直接 `engine.input_bytes()`
- **后果**：
  - 正面：充分利用多核，UI 不卡顿，符合 gpui 推荐模式
  - 注意：gpui_platform::application() 内部自带 tokio Runtime（从 bezel-gpui 0.3 起默认创建），不需要我们自己建 Runtime

## ADR-008：跨平台支持策略 — MVP 优先 Windows，macOS/Linux 源码可构建

- **日期**：2026-11-13
- **状态**：已接受
- **上下文**：用户当前开发环境是 Windows，目标是"类似 sshxterm"，而 sshxterm 主要跑在 Windows 上。
- **决策**：
  - v1.0 MVP：Windows 优先完整验证
  - macOS：bezel-gpui 支持，但 DPAPI 换成 keychain；monitor 采集脚本换成 sysctl 版本
  - Linux：bezel-gpui 需要 Wayland/X11 headers；monitor 脚本保持 /proc 版本
- **后果**：
  - 正面：MVP 焦点集中，不摊薄资源
  - 跨平台差异集中在 `rterm-crypto`（DPAPI → keychain）和 `rterm-monitor`（采集脚本）两个模块，通过 `#[cfg]` 隔离

## ADR-009：会话导入导出格式 — 自定义 JSON + 加密

- **日期**：2026-11-13
- **状态**：已接受
- **上下文**：需要跨机器、跨版本的可迁移会话格式。
- **选项**：
  - A. 自定义 JSON + 加密（自描述版本号）
  - B. PuTTY 注册表格式导入
  - C. OpenSSH ~/.ssh/config 导入
- **决策**：v1.0 A + C（OpenSSH 导入作为补充），B 延后。JSON schema 带 `version` 字段，未来迁移用 serde 的 default / tag。
- **示例 JSON（未加密结构）**：
  ```json
  {
    "rterm-export": {
      "version": 1,
      "exported_at": "2026-11-13T10:00:00Z",
      "passphrase": { "kdf": "pbkdf2-sha256", "salt": "...", "iter": 600_000 },
      "sessions": [
        {
          "id": "019f...",
          "name": "db-prod",
          "host": "10.0.0.5",
          "port": 22,
          "username": "deploy",
          "auth": { "$type": "password", "ciphertext_base64": "<AES-GCM>" }
        }
      ]
    }
  }
  ```

## ADR-010：事件总线实现 — 内嵌 AppState 的 SubscriberSet

- **日期**：2026-11-13
- **状态**：已接受
- **上下文**：gpui 提供了 `gpui::SubscriberSet<T>` 作为全局发布/订阅原语。
- **决策**：在 AppState 里持有一个 `SubscriberSet<AppEvent>`，每个 View 通过 `cx.subscribe(&event_bus, |app, ev| { ... })` 订阅。不造轮子，直接复用 gpui 的 pub/sub 基础设施。
- **后果**：简单、自动跟随 App 生命周期；缺点是所有事件统一订阅后 View 要自行 match 过滤——这与 gpui 推荐的模式一致。

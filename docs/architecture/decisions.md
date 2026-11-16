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
  - 缓解：在 `rterm-ssh` 里用独立 `std::thread` 包装 ssh2 调用，通过 mpsc / futures channel 与 gpui 主线程桥接（实际实现见 ADR-007 修订）

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

## ADR-006：错误处理策略 — Service 层 anyhow + View 层字符串边界

- **日期**：2026-11-13
- **状态**：已接受（2026-11-16 修订：暂不引入 `AppError`）
- **上下文**：分层架构下错误如何传播。
- **决策**：
  - Service / Infrastructure 层：`anyhow::Result<T>` + `.context()` 保留错误链
  - 应用边界：当前直接在后台任务/事件构造点把错误格式化为中文文案，通过 `PtyEvent::{Failed,Closed}`、`AppEvent::{SessionFailed,...}` 传给 View
  - View 层：只接收面向用户的字符串，不接触原始 ssh2 / io Error
  - `rterm-app::AppError`（thiserror）暂不实现：当前错误主要一次性展示，没有分类重试/告警需求；等 S9/S10 出现结构化错误消费方再引入
- **后果**：
  - 正面：实现简单，View 层只关心文案；错误链在 log 中完整保留
  - 风险：跨层字符串边界丢失错误类型，未来若需要按错误类型重试需补 `AppError` 映射

## ADR-007：异步线程模型 — ssh2 blocking + 独立线程 + channel 桥接 gpui 主线程

- **日期**：2026-11-13
- **状态**：已接受（2026-11-16 修订：实际使用 std::thread，而非 tokio spawn_blocking）
- **上下文**：ssh2 crate 所有调用都是同步的，gpui 有自己的事件循环不能阻塞。
- **决策（实际实现）**：
  - 每个 SSH 连接 / PTY IO / SFTP 传输 worker 使用独立 `std::thread`，不占用 gpui 主线程
  - 后台 → UI：`futures::channel::mpsc`（PTY 输出，有界背压）或 `std::sync::mpsc`（SFTP 导航/操作结果、主机密钥应答）
  - UI → 后台：`std::sync::mpsc` 命令通道（输入 / resize / close / transfer command）
  - gpui 侧用 `cx.spawn` 驱动 receiver（在 gpui 自带 Runtime 上 await），收到消息后 `this.update(...)`
  - **TerminalEngine / SyncBridge** 必须在 gpui 主线程（`alacritty_terminal::Term` 是 `!Send`），收到 PTY 字节后直接 `engine.input_bytes()`
- **后果**：
  - 正面：UI 不阻塞；不需要自己建 tokio Runtime；线程生命周期可控（Tab 关闭时 shutdown/join）
  - 风险：阻塞的 libssh2 网络调用不可中断，关闭 Tab 时回收线程可能等待当前调用返回（已用后台 reaper 线程避免卡 UI）
  - 注意：`gpui_platform::application()` 内部自带 Runtime（bezel-gpui 0.3 起）

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
- **实际格式（2026-11-16 修订）**：
  - 顶层为 `ExportWrapper { tag: ExportTag { version, exported_at, sessions } }`
  - `sessions` 内每个敏感字段是 `EncryptedString { version, data(base64) }`：
    - `version=0` 明文（仅内存）
    - `version=1` DPAPI（本机导出）
    - `version=2` AES-256-GCM（口令导出）
  - AES-GCM 的 salt/nonce 编码在对应字段的 `data` 内；KDF 参数当前固定为 PBKDF2-SHA256 600k，未写入文件
  - 未实现 ADR 初稿中的全局 `passphrase.kdf` 元数据块
- **后果**：
  - 正面：字段级加密，导入时逐字段解密，单条失败可保留其余
  - 风险：KDF 参数/算法没有文件级版本元数据，未来升级参数时需要兼容旧数据（按 `EncryptedString.version` 或探测数据长度处理）
- **示例 JSON（结构示意，敏感字段已加密）**：
  ```json
  {
    "tag": {
      "version": 1,
      "exported_at": "2026-11-13T10:00:00Z",
      "sessions": [
        {
          "id": "019f...",
          "name": "db-prod",
          "host": "10.0.0.5",
          "port": 22,
          "username": "deploy",
          "auth": { "Password": { "password": { "version": 2, "data": "<base64>" } } }
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

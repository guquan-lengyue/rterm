# rterm — 架构设计

## 1. 总体分层

```
┌─────────────────────────────────────────────────────────────────────┐
│                    Presentation (gpui Views)                        │
│  ┌───────────┬───────────┬───────────┬───────────────────────────┐  │
│  │ SessionTree│Terminal   │  SFTP     │ ResourceMonitor          │  │
│  │   View     │  View     │  View     │       View               │  │
│  └───────────┴───────────┴───────────┴───────────────────────────┘  │
│  ┌────────────────────────┬─────────────────────────────────────┐  │
│  │ TabBar / Split / Menu  │      SettingsView / CommandPanel    │  │
│  └────────────────────────┴─────────────────────────────────────┘  │
├─────────────────────────────────────────────────────────────────────┤
│                  Application Layer (rterm-app)                       │
│   AppState · EventBus · TabRegistry · Dialogs · ThemeStore           │
├─────────────────────────────────────────────────────────────────────┤
│                 Domain / Service Layer (各业务 crate)                │
│  ┌──────────┬───────────┬──────────┬──────────┬──────────────────┐ │
│  │Session   │Terminal   │ SFTP     │Monitor   │ SyncBridge       │ │
│  │ Service  │ Service   │ Service  │ Service  │                  │ │
│  └──────────┴───────────┴──────────┴──────────┴──────────────────┘ │
│  ┌──────────┬───────────┬──────────┬──────────┬──────────────────┐ │
│  │ Import/  │ Auth      │ Config   │ Host     │ FileTransferQueue│ │
│  │ Export   │ Manager   │ Store    │ KeyStore │                  │ │
│  └──────────┴───────────┴──────────┴──────────┴──────────────────┘ │
├─────────────────────────────────────────────────────────────────────┤
│                  Infrastructure Layer                                │
│   ssh2 (libssh2) · alacritty_terminal · tokio async · serde          │
│   DPAPI/AES-GCM · winreg · JSON storage · glob · walkdir             │
└─────────────────────────────────────────────────────────────────────┘
```

**分层原则**：
- Presentation 层不做业务逻辑，只订阅事件 + 渲染
- Application 层是胶水，负责跨 Service 的编排（如「用户点击连接」→ SSH Service → EventBus 广播 → Terminal View 订阅并开 PTY）
- Domain Service 层内部独立，通过 EventBus 相互解耦，不直接调用
- Infrastructure 层是纯依赖封装，可单测

---

## 2. Crate 划分

```
rterm/
├── Cargo.toml                       # workspace root (resolver = 2)
├── docs/
│   ├── README.md
│   ├── product/
│   │   ├── README.md
│   │   └── overview.md
│   ├── architecture/
│   │   ├── README.md
│   │   ├── architecture.md
│   │   └── decisions.md
│   └── planning/
│       ├── README.md
│       ├── roadmap.md
│       └── sprint-*.md
├── scripts/
│   └── msvc-env.ps1                 # Windows MSVC + CMake 环境加载
└── crates/
    ├── rterm-app/                   # 入口 + gpui 主窗口 + AppState + EventBus
    ├── rterm-session/               # Session 模型 + CRUD + 持久化 + 导入导出
    ├── rterm-ssh/                   # 对 ssh2 crate 的薄封装（异步化 + 连接池 + keepalive）
    ├── rterm-terminal/              # PTY 管理 + alacritty_terminal glue + 输入输出桥接
    ├── rterm-sftp/                  # SFTP 文件浏览 + 传输队列 + 进度追踪
    ├── rterm-monitor/               # 远程命令采集 + 时序聚合 + 告警
    ├── rterm-sync/                  # 终端 ↔ SFTP 路径同步核心逻辑（SyncBridge）
    └── rterm-crypto/                # Windows DPAPI + AES-GCM 加解密
```

### 2.1 各 Crate 职责

#### `rterm-crypto`（最底层）

- **依赖**：`windows`（DPAPI）、`aes-gcm`、`rand`
- **公开 API**：
  ```rust
  pub fn dpapi_encrypt(plaintext: &[u8]) -> Result<Vec<u8>>;
  pub fn dpapi_decrypt(ciphertext: &[u8]) -> Result<Vec<u8>>;
  pub fn aesgcm_encrypt(plaintext: &[u8], passphrase: &str, salt: &[u8]) -> Result<Vec<u8>>;
  pub fn aesgcm_decrypt(ciphertext: &[u8], passphrase: &str, salt: &[u8]) -> Result<Vec<u8>>;
  ```
- **注意**：DPAPI 绑定当前 Windows 登录用户 + 当前机器；跨机器迁移必须用 AES-GCM（导出文件格式）

#### `rterm-session`

- **依赖**：`serde`、`serde_json`、`uuid`、`chrono`、`rterm-crypto`
- **公开类型**：见「核心数据模型」章节
- **公开服务**：
  ```rust
  pub struct SessionStore { /* 持有 Vec<Session> + 文件路径 */ }
  impl SessionStore {
      pub fn load(path: &Path) -> Result<Self>;
      pub fn save(&self, path: &Path) -> Result<()>;
      pub fn list(&self) -> &[Session];
      pub fn get(&self, id: Uuid) -> Option<&Session>;
      pub fn upsert(&mut self, session: Session);
      pub fn delete(&mut self, id: Uuid);
      pub fn import_json(&mut self, json: &str, policy: ConflictPolicy) -> Result<ImportReport>;
      pub fn export_json(&self, scope: ExportScope, passphrase: Option<&str>) -> Result<String>;
  }
  ```

#### `rterm-ssh`

- **依赖**：`ssh2`（vendored-openssl）、`tokio`、`futures`
- **核心类型**：
  ```rust
  pub struct SshSession { /* ssh2::Session + 连接状态 + keepalive task handle */ }
  pub struct OpenPtyResult { pub channel: ssh2::Channel, pub cols: u32, pub rows: u32 }
  ```
- **公开 API**：
  ```rust
  impl SshSession {
      pub fn connect(&mut self, session: &Session) -> Result<()>;
      pub fn disconnect(&mut self);
      pub fn is_connected(&self) -> bool;
      pub fn open_pty(&mut self, cols: u32, rows: u32) -> Result<OpenPtyResult>;
      pub fn exec(&mut self, command: &str) -> Result<ExecResult>;
      pub fn sftp(&self) -> Result<ssh2::Sftp>;
      pub fn resize_pty(&mut self, channel: &mut ssh2::Channel, cols: u32, rows: u32) -> Result<()>;
  }
  ```
- **线程模型**：`SshSession` 在后台线程通过 `spawn_blocking` 执行 ssh2 同步调用，用 `mpsc::channel` 与 UI 线程桥接

#### `rterm-terminal`

- **依赖**：`alacritty_terminal`
- **核心类型**：
  ```rust
  pub struct TerminalEngine {
      term: Term<SizeInfo>,           // alacritty_terminal Term
      scroll_history: VecDeque<Line>,  // 滚动缓冲
      cursor_blink_timer: Option<Instant>,
  }
  impl TerminalEngine {
      pub fn new(cols: u16, rows: u16, scrollback_lines: usize) -> Self;
      pub fn input_bytes(&mut self, bytes: &[u8]);          // 喂 PTY 输出
      pub fn input_key(&mut self, key: TerminalKey);        // 用户按键 → Term.paste_bytes
      pub fn resize(&mut self, cols: u16, rows: u16);
      pub fn grid(&self) -> &Grid<Cell>;                    // gpui View 渲染时读取
      pub fn cursor(&self) -> Cursor;
      pub fn active_colors(&self) -> &[Rgb; 256];
  }
  ```
- **注意**：`alacritty_terminal::Term` 是 `!Send + !Sync`，所以 `TerminalEngine` 必须在 gpui 主线程中。SSH PTY 的字节读取在后台线程进行，读到的字节通过 `cx.spawn()` 发回主线程喂给 `input_bytes()`

#### `rterm-sftp`

- **依赖**：`ssh2`、`futures`
- **核心类型**：
  ```rust
  pub struct SftpService { /* ssh2::Sftp + 本地 walker */ }
  pub struct TransferTask { pub id: Uuid, pub direction: TransferDir, pub remote_path: PathBuf, pub local_path: PathBuf, pub progress: TransferProgress, pub state: TransferState }
  pub struct TransferQueue { tasks: VecDeque<TransferTask> }
  ```
- **公开 API**：文件浏览用「纯 stat 递归 + 按需加载文件列表」；传输用后台 blocking

#### `rterm-monitor`

- **依赖**：`tokio`
- **核心类型**：
  ```rust
  pub struct MonitorCollector {
      samples: HashMap<String, VecDeque<Sample>>,  // 指标名 → 最近 N 个采样
      thresholds: HashMap<String, Threshold>,
      session_id: Uuid,
  }
  pub struct ResourceSample {
      pub cpu_usage: f32,
      pub mem_used_pct: f32,
      pub swap_used_pct: f32,
      pub disk_total_gb: f64,
      pub disk_used_pct: f32,
      pub net_rx_kbps: f64,
      pub net_tx_kbps: f64,
      pub load_avg_1: f32,
      pub load_avg_5: f32,
      pub timestamp: SystemTime,
  }
  ```
- **采集脚本**：Linux 端是一条压缩 shell 命令，一行 `cat /proc/stat; cat /proc/meminfo; df -P / | tail -1; cat /proc/net/dev; uptime`，在 SSH 通道上异步执行并解析

#### `rterm-sync`

- **依赖**：`regex`（PS1 匹配）
- **核心类型**：
  ```rust
  pub struct SyncBridge {
      mode: SyncMode,                    // 方向开关
      last_terminal_cwd: Option<PathBuf>,
      last_sftp_cwd: Option<PathBuf>,
      ignore_list: VecDeque<PathBuf>,    // 防止回环
  }
  pub enum SyncMode { Off, TerminalToSftpOnly, SftpToTerminalOnly, Bidirectional }
  ```
- **核心方法**：
  ```rust
  impl SyncBridge {
      pub fn on_terminal_output(&mut self, bytes: &[u8]) -> Option<PathBuf>;
          // 尝试 PS1 正则 + 兜底 pwd；若检测到 cwd 变化则返回新路径

      pub fn on_sftp_navigated(&mut self, new_path: &Path) -> Option<InjectCommand>;
          // 若 SFTP 用户手动导航，返回需注入 PTY 的 "cd <path> && pwd" 命令
  }
  ```

#### `rterm-app`（入口 + 胶水）

- **依赖**：全部 `rterm-*` crate、`gpui`（bezel-gpui）、`gpui_platform`
- **核心类型**：
  ```rust
  pub struct AppState {                        // impl gpui::Global
      session_store: SessionStore,             // Sprint 1 占位：Vec<Uuid>; Sprint 2 替换为 rterm_session::SessionStore
      tab_registry: TabRegistry,
      event_bus: EventBus,                     // 空 struct + impl EventEmitter<AppEvent>（marker trait）
      settings: Settings,
      themes: ThemeStore,
  }

  // ✅ S1 已落地的真实签名（2026-11-13）：
  pub struct EventBus;   // empty struct
  impl gpui::EventEmitter<AppEvent> for EventBus {}

  // emit:  app.emit(&app.global::<AppState>().event_bus, AppEvent::Xxx { ... });
  // subscribe: cx.subscribe(&app_state.event_bus, |app, ev| { ... });
  ```
- **公开事件**（见下「事件总线」）

---

## 3. 关键子系统设计

### 3.1 SSH 连接与重连

```
用户点击「连接」
    │
    ▼
SessionService::connect(session_id)
    │
    ▼ (async spawn_blocking)
ssh2::Session::connect(host:port)
ssh2::Session::userauth_password / userauth_publickey / userauth_agent
    │
    ├── 成功 → app.emit(&state.event_bus, AppEvent::SessionConnected)
    │           → 启动 KeepAlive tokio task（每 N 秒发送 "echo ping"）
    │           → 启动 PTY + app.emit(&state.event_bus, AppEvent::TerminalReady)
    │
    └── 失败 → app.emit(&state.event_bus, AppEvent::SessionFailed { reason })
                → SessionStore.last_error = Some(reason)
```

**重连策略**：KeepAlive task 收到 SSH 错误（ChannelError / Disconnected）时启动重连循环：
- 间隔：1s → 2s → 4s → 8s → 16s（指数退避，最多 5 次）
- 每次重连先重置 `SshSession`，然后完整走一遍 connect + auth + open_pty
- 全部失败后广播 `AppEvent::SessionDisconnected { reason: "max_reconnect_exceeded" }`

### 3.2 终端数据流

```
┌────────────────────────────────────────────────────┐
│  后台线程（SshSession 持有 ssh2::Channel）        │
│  loop { channel.read() → bytes → tx.send(bytes) } │
└──────────────────────┬─────────────────────────────┘
                       │ mpsc::channel
                       ▼
┌────────────────────────────────────────────────────┐
│  gpui 主线程（TerminalView）                      │
│  cx.spawn(async move { rx.recv().await })?;        │
│  loop {                                             │
│      match channel.recv().await {                   │
│          Some(bytes) =>                             │
│              engine.input_bytes(bytes);             │
│              bridge.on_terminal_output(bytes);      │
│              cx.notify();  // 触发 render          │
│          None => break;                             │
│      }                                              │
│  }                                                  │
│  // render 时读 engine.grid() 画 Cell              │
└────────────────────────────────────────────────────┘

用户按键 → TerminalView.on_key_event
    → engine.input_key(key)
    → PTY.write_bytes(key_to_bytes))  // 反向通道
```

### 3.3 路径同步（SyncBridge）

```
方案 A — 终端 → SFTP（主方案）
┌──────────────────────────────────────────────────┐
│  Terminal PTY output bytes                       │
│       │                                          │
│       ▼                                          │
│  SyncBridge.on_terminal_output(bytes)            │
│    ├── 1) PS1 正则 "~/$ #" 后路径提取             │
│    ├── 2) 兜底：没匹配到？则触发后台              │
│    │      "pwd" 注入 + 等待回复（超时 1s）        │
│    └── 3) 解析出 new_cwd → 对比 last_terminal    │
│          ├── 相同 → 忽略                         │
│          └── 不同 → 更新 + 广播                    │
│               AppEvent::SftpNavigated { path }   │
│               // SFTP 侧订阅并刷新右栏           │
└──────────────────────────────────────────────────┘

方案 B — SFTP → 终端（仅 Bidirectional / SftpToTerminal 模式）
┌──────────────────────────────────────────────────┐
│  用户在 SFTP 右栏双击 / 面包屑导航                │
│       │                                          │
│       ▼                                          │
│  SftpView.emit(AppEvent::UserRequestedCwd)       │
│       │                                          │
│       ▼                                          │
│  SyncBridge.on_sftp_navigated(path)              │
│    ├── 检查 ignore_list 内是否有这个 path         │
│    │   （避免刚收到 terminal 来的 cd 又被反注）    │
│    └── 无 → 组装注入命令                          │
│           "cd /absolute/path 2>/dev/null"        │
│           // 不输出 pwd，抑制回显                 │
│           TerminalService::inject_command(cmd)  │
│           ignore_list.push_back(path)            │
│           // 下次 terminal 也发同样的 cwd 时忽略 │
└──────────────────────────────────────────────────┘

回环预防：ignore_list 是一个 VecDeque，每次最多保留 10 个路径条目，FIFO。
```

### 3.4 性能监控数据采集

```
MonitorCollector.start(session_id, interval_secs=2)
    │
    ▼
loop every 2s (tokio::interval)
    ssh2.exec("cat /proc/stat; cat /proc/meminfo; ...")
    parse → ResourceSample
    samples[metric].push_front(sample)          // 保留 60 个
    if threshold_hit { broadcast Alert }
    broadcast AppEvent::MonitorSample { sample } // 触发折线图 render
```

**阈值判断伪代码**：
```rust
if rolling_avg(&samples["cpu_usage"], last=5) > threshold.cpu_pct {
    if !threshold.alerted {
        broadcast Alert::CpuHigh;
        threshold.alerted = true;
    }
}
```

### 3.5 传输队列（SFTP）

```
User drags file → TransferQueue.enqueue(TransferTask)
    │
    ▼
queue worker (tokio 1 per concurrent transfer, 默认 2 并发)
    ssh2.sftp().put_file / get_file
    loop {
        read/write chunk 64KB
        bytes_transferred += chunk.len()
        queue.update_progress(task.id, bytes_transferred)
        broadcast AppEvent::FileTransferProgress
    }
    broadcast AppEvent::FileTransferCompleted
```

---

## 4. 核心数据模型

```rust
// === rterm-session ===

pub struct Session {
    pub id: Uuid,
    pub name: String,
    pub host: String,
    pub port: u16,                                  // default 22
    pub username: String,

    pub auth: AuthMethod,
    pub proxy_jump: Option<Box<Session>>,          // 多级嵌套

    pub keepalive_secs: u32,                        // default 30
    pub connect_timeout_secs: u32,                  // default 10
    pub compression: bool,                          // default true

    pub folder: Option<String>,                     // 分组路径，例 "prod/db"
    pub favorite: bool,
    pub tags: Vec<String>,

    pub theme_override: Option<String>,             // 覆盖全局主题
    pub terminal_cols: Option<u16>,
    pub terminal_rows: Option<u16>,
    pub shell: Option<String>,                      // 不指定则用用户登录 shell

    pub created_at: DateTime<Utc>,
    pub last_connected: Option<DateTime<Utc>>,
}

pub enum AuthMethod {
    Password(EncryptedString),                      // DPAPI 加密
    PrivateKey {
        path: PathBuf,
        passphrase: Option<EncryptedString>,
    },
    Agent {
        key_comment: String,                        // ssh-add -l 返回的 comment
    },
}

// 不直接存 Vec<u8>，而是存 (version + data)，便于未来换加密方式
pub struct EncryptedString {
    pub version: u8,                                // 1 = DPAPI, 2 = AES-GCM
    pub data: Vec<u8>,
}

// === rterm-app ===

pub struct AppState {
    pub session_store: SessionStore,
    pub tab_registry: TabRegistry,
    pub settings: Settings,
    pub themes: ThemeStore,
}

pub struct Settings {
    pub default_port: u16,
    pub default_username: Option<String>,
    pub default_auth: AuthMethod,
    pub default_theme: String,
    pub default_font: String,
    pub default_font_size: f32,

    pub terminal_scrollback_lines: usize,           // default 10000
    pub terminal_cursor_style: CursorStyle,
    pub terminal_auto_log: bool,
    pub terminal_log_dir: Option<PathBuf>,

    pub sftp_sync_mode: SyncMode,                   // 默认 Bidirectional
    pub sftp_sync_terminal_source: Ps1OrPwd,        // 默认两者结合

    pub monitor_interval_secs: u32,                 // default 2
    pub monitor_cpu_threshold: f32,                 // default 90.0
    pub monitor_mem_threshold: f32,

    pub export_auto_backup_enabled: bool,
    pub export_backup_dir: PathBuf,
}

pub struct TabRegistry {
    pub tabs: Vec<TabInfo>,                         // 当前所有标签
    pub active: usize,
}

pub struct TabInfo {
    pub id: Uuid,
    pub session_id: Uuid,
    pub kind: TabKind,
    pub title: String,
}

pub enum TabKind { Terminal, Sftp, Monitor }
```

---

## 5. 事件总线

**设计原则**：各 Service 和各 View 之间**不直接调用**，只通过 EventBus 广播/订阅。解耦后任何一方都可以被替换、重放、或增加观察者（比如加个 debug logger）。

```rust
pub enum AppEvent {
    // ---- 会话 ----
    SessionCreated { session: Session },
    SessionUpdated { session: Session },
    SessionDeleted { id: Uuid },
    SessionImported { report: ImportReport },
    SessionConnected { id: Uuid },
    SessionFailed    { id: Uuid, reason: String },
    SessionDisconnected { id: Uuid, reason: String },
    SessionReconnecting { id: Uuid, attempt: u8 },

    // ---- 终端 ----
    TerminalReady    { tab_id: Uuid, session_id: Uuid },
    TerminalData     { tab_id: Uuid, session_id: Uuid, bytes: Vec<u8> },
    TerminalCwdHint  { tab_id: Uuid, session_id: Uuid, cwd: PathBuf, source: CwdSource },
    TerminalResize   { tab_id: Uuid, cols: u16, rows: u16 },
    TerminalClosed   { tab_id: Uuid },

    // ---- SFTP ----
    SftpOpened       { tab_id: Uuid, session_id: Uuid },
    SftpNavigated    { tab_id: Uuid, session_id: Uuid, path: PathBuf, triggered_by: NavSource },
    UserRequestedCwd { tab_id: Uuid, path: PathBuf },

    // ---- 监控 ----
    MonitorSample    { session_id: Uuid, sample: ResourceSample },
    MonitorAlert     { session_id: Uuid, alert: Alert },

    // ---- 传输 ----
    FileTransferStarted   { id: Uuid, direction: TransferDir, remote: String },
    FileTransferProgress  { id: Uuid, bytes: u64, total: u64 },
    FileTransferCompleted { id: Uuid },
    FileTransferFailed    { id: Uuid, reason: String },

    // ---- UI ----
    AppSettingsChanged,
    AppThemeChanged { name: String },
    StatusBarMessage { text: String, timeout_ms: u64 },
}
```

**订阅示例**（gpui View 里）：
```rust
cx.subscribe(&event_bus, |app: &mut App, ev: &AppEvent| {
    if let AppEvent::SessionConnected { id } = ev {
        app.open_tab(*id, TabKind::Terminal);
    }
});
```

---

## 6. 存储路径

| 用途 | Windows | macOS | Linux |
|------|---------|-------|-------|
| 会话库 | `%APPDATA%\rterm\sessions.json` | `~/Library/Application Support/rterm/sessions.json` | `~/.config/rterm/sessions.json` |
| 设置 | `%APPDATA%\rterm\settings.json` | 同上 | 同上 |
| 自动备份 | `%APPDATA%\rterm\backup\` | 同上 | `~/.config/rterm/backup/` |
| 终端日志 | 用户配置目录 | 用户配置目录 | 用户配置目录 |
| 缓存 | `%LOCALAPPDATA%\rterm\cache\` | `~/Library/Caches/rterm/` | `~/.cache/rterm/` |

路径解析用 `dirs` crate（不依赖 gpui，在 service 层做）。

---

## 7. 配置 Cargo.toml 依赖概览（workspace）

| crate | 版本 | 用途 |
|-------|------|------|
| `bezel-gpui` | 0.3 | UI 框架（gpui 包名） |
| `bezel-gpui-platform` | 0.3 | 平台初始化（gpui_platform 包名） |
| `ssh2` | 0.9, vendored-openssl + openssl-on-win32 | SSH 协议（Windows 必须 OpenSSL 后端才支持 curve25519/ed25519） |
| `alacritty_terminal` | 0.25.1 | 终端仿真后端 |
| `tokio` | 1 full | 异步 runtime（SSH IO + 监控定时 + 传输队列） |
| `futures` | 0.3 | async traits |
| `serde` / `serde_json` | 1 | 会话 / 设置 JSON |
| `uuid` | 1 v4+serde | 实体 ID |
| `chrono` | 0.4 serde | 时间戳 |
| `anyhow` / `thiserror` | 1 / 2 | 错误处理 |
| `windows` (crate) | 0.57+ | DPAPI 加密 |
| `aes-gcm` | 0.10 | 导出文件加密 |
| `rand` | 0.8 | salt 生成 |
| `regex` | 1 | PS1 路径匹配 |
| `dirs` | 5 | 跨平台存储路径 |
| `walkdir` | 2 | 本地文件浏览 |
| `bytes` | 1 | 零拷贝缓冲 |
| `parking_lot` | 0.12 | 高性能锁 |
| `log` / `env_logger` | 0.4 / 0.11 | 日志 |

# Sprint 1 — 底层基础设施

> **状态**：⏳ 下一个
> **前置**：S0（gpui 空窗口可编译）
> **目标**：rterm-crypto + EventBus + AppState + 主窗口壳
> **必读**：[planning/README.md](./README.md) 的执行纪律

---

### 1.1 rterm-crypto — DPAPI

| # | 粒度 | 文件 | 要写的 | 验收 |
|---|------|------|--------|------|
| 1.1.1 | 0.25h | `crates/rterm-crypto/Cargo.toml` | 加依赖 `windows = { version = "0.57", features = ["Win32_Security_Cryptography"] }` + `rand = "0.8"` + `aes-gcm = "0.10"` | `cargo check -p rterm-crypto` 过 |
| 1.1.2 | 0.5h | `crates/rterm-crypto/src/lib.rs` | `pub fn dpapi_encrypt(plaintext: &[u8]) -> Result<Vec<u8>>` — `CryptProtectData` + 当前 user scope + 空 entropy | 返回 bytes 非空；doc comment |
| 1.1.3 | 0.5h | `crates/rterm-crypto/src/lib.rs` | `pub fn dpapi_decrypt(ciphertext: &[u8]) -> Result<Vec<u8>>` — `CryptUnprotectData`；失败 map 为 `anyhow::anyhow!("DPAPI decrypt failed: {e}")` | 加密→解密往返等于原文 |
| 1.1.4 | 0.25h | `crates/rterm-crypto/src/lib.rs` | 加 `#[cfg(not(target_os = "windows"))]` stub（返回 Err "DPAPI only on Windows"）| macOS/Linux 编译不崩 |

### 1.2 rterm-crypto — AES-GCM

| # | 粒度 | 文件 | 要写的 | 验收 |
|---|------|------|--------|------|
| 1.2.1 | 0.5h | `crates/rterm-crypto/src/lib.rs` | `pub fn derive_key(passphrase: &str, salt: &[u8]) -> Result<[u8; 32]>` — PBKDF2-HMAC-SHA256, 600_000 iter | 相同输入产出相同 key |
| 1.2.2 | 0.5h | `crates/rterm-crypto/src/lib.rs` | `pub fn aesgcm_encrypt(plaintext: &[u8], passphrase: &str) -> Result<Vec<u8>>` — 16 字节随机 salt + 12 字节 nonce + ciphertext + tag；返回 `[salt(16) \|\| nonce(12) \|\| ciphertext+tag]` 拼接 | 输出非空；salt 每次不同 |
| 1.2.3 | 0.5h | `crates/rterm-crypto/src/lib.rs` | `pub fn aesgcm_decrypt(packed: &[u8], passphrase: &str) -> Result<Vec<u8>>` — 拆分 salt/nonce/ciphertext；derive_key；decrypt | 加密→解密往返；错误口令抛 Err |

### 1.3 rterm-crypto — 单元测试

| # | 粒度 | 文件 | 要写的 | 验收 |
|---|------|------|--------|------|
| 1.3.1 | 0.5h | `crates/rterm-crypto/tests/crypto_tests.rs` | `#[test] fn dpapi_roundtrip()` + `#[test] fn aesgcm_roundtrip()` + `#[test] fn aesgcm_bad_passphrase()` | `cargo test -p rterm-crypto` 三个过 |
| 1.3.2 | 0.25h | `crates/rterm-crypto/tests/crypto_tests.rs` | `#[test] fn aesgcm_empty_input()` + `#[test] fn aesgcm_large_input_1mb()` | 边界过 |

### 1.4 EventBus + AppState

| # | 粒度 | 文件 | 要写的 | 验收 |
|---|------|------|--------|------|
| 1.4.1 | 0.5h | `crates/rterm-app/src/event.rs` | `#[derive(Clone, Debug)] pub enum AppEvent { ... }` — 完整事件枚举（见 architecture.md 第 5 节抄） | `cargo check -p rterm-app` 过 |
| 1.4.2 | 0.25h | `crates/rterm-app/src/event.rs` | `pub struct EventBus(pub gpui::SubscriberSet<AppEvent>)` + `impl EventBus { pub fn new() -> Self }` | 空实现，能 new() |
| 1.4.3 | 0.5h | `crates/rterm-app/src/state.rs` | `pub struct Settings` — 所有默认值（见 architecture.md 核心数据模型）；`impl Default for Settings` | `let s = Settings::default();` 不崩 |
| 1.4.4 | 0.25h | `crates/rterm-app/src/state.rs` | `pub struct SessionStore` + `pub struct TabRegistry`（空 struct，Sprint 2 填充）+ `pub struct AppState { pub event_bus, pub settings, pub session_store, pub tab_registry }` | `AppState::new()` 不崩 |

### 1.5 主窗口壳（gpui）

| # | 粒度 | 文件 | 要写的 | 验收 |
|---|------|------|--------|------|
| 1.5.1 | 0.5h | `crates/rterm-app/src/views/mod.rs` | 模块声明 + `pub struct MainWindow { app_state: AppState }` + `impl Render for MainWindow` | 显示 `div().size_full().bg(rgb(0x1e1e1e))` |
| 1.5.2 | 0.5h | `crates/rterm-app/src/views/main_window.rs` | 三栏：左 `SessionTreePlaceholder`（宽 240px）+ 中间 `div().size_full()` + 底 `StatusBarPlaceholder`（高 28px） | 窗口能显示三栏灰底 |
| 1.5.3 | 0.5h | `crates/rterm-app/src/views/session_tree.rs` | `pub struct SessionTreeView` 渲染 "Session Tree（待填充）" 占位文字；订阅空 EventBus | 左侧栏看到占位文字 |
| 1.5.4 | 0.25h | `crates/rterm-app/src/views/status_bar.rs` | `pub struct StatusBar { message: String }` + 显示 message 默认 "Ready" | 底部状态栏显示 Ready |
| 1.5.5 | 0.25h | `crates/rterm-app/src/main.rs` | 替换空窗口：`App::new()` → 打开 MainWindow → 把 AppState 存进 `App::set_global()` | 启动看到三栏主窗口 |

### 1.6 验收

| # | 粒度 | 验收 |
|---|------|------|
| 1.6.1 | 0.25h | `cargo test -p rterm-crypto` 全过；`cargo build -p rterm-app` 无 warning；启动 → 看到三栏主窗口；无 panic；resize 正常；Ctrl+C 退出干净 |

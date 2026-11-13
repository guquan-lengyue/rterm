# Sprint 2 — 会话数据层

> **状态**：—
> **前置**：[Sprint 1](./sprint-01-infrastructure.md)（crypto + event_bus + app_state + 主窗口壳）
> **目标**：Session 模型 + SessionStore CRUD + JSON 持久化 + 导入导出
> **注意**：本 Sprint **不写任何 gpui View**（UI 在 S3）

---

### 2.1 Session 模型

| # | 粒度 | 文件 | 要写的 | 验收 |
|---|------|------|--------|------|
| 2.1.1 | 0.5h | `crates/rterm-session/src/model.rs` | `#[derive(Clone, Serialize, Deserialize)] pub struct Session` + 所有字段（见 architecture.md 核心数据模型）；字段默认值用 `#[serde(default)]` | 能构造 `let s = Session::default();` |
| 2.1.2 | 0.25h | `crates/rterm-session/src/model.rs` | `#[derive(Clone, Serialize, Deserialize)] pub enum AuthMethod { Password(EncryptedString), PrivateKey { path, passphrase }, Agent { key_comment } }` | 三个变体都能构造 |
| 2.1.3 | 0.25h | `crates/rterm-session/src/model.rs` | `#[derive(Clone, Serialize, Deserialize)] pub struct EncryptedString { pub version: u8, pub data: Vec<u8> }` | 能序列化；JSON 里有 `{version, data}` |

### 2.2 SessionStore — 内存 CRUD

| # | 粒度 | 文件 | 要写的 | 验收 |
|---|------|------|--------|------|
| 2.2.1 | 0.5h | `crates/rterm-session/src/store.rs` | `pub struct SessionStore { sessions: Vec<Session>, dirty: bool }` + `pub fn list(&self) -> &[Session]` + `pub fn get(&self, id: Uuid) -> Option<&Session>` | 能 new() + list() 为空 |
| 2.2.2 | 0.5h | `crates/rterm-session/src/store.rs` | `pub fn upsert(&mut self, session: Session)` — 如果 session.id 存在则 update（更新 `last_modified`）否则 push；`self.dirty = true` | 同 id 调两次 upsert 只有一份 |
| 2.2.3 | 0.25h | `crates/rterm-session/src/store.rs` | `pub fn delete(&mut self, id: Uuid) -> bool` + `pub fn count(&self) -> usize` | 返回被删前 count |

### 2.3 SessionStore — JSON 持久化

| # | 粒度 | 文件 | 要写的 | 验收 |
|---|------|------|--------|------|
| 2.3.1 | 0.5h | `crates/rterm-session/src/store.rs` | `pub fn save<F: FnMut(&str)>(&mut self, path: &Path, encrypt_fn: F) -> Result<()>` — serialize → 加密每个 EncryptedString → write 原子文件（先 tmp 再 rename） | 文件存在且 JSON 合法 |
| 2.3.2 | 0.5h | `crates/rterm-session/src/store.rs` | `pub fn load<F: FnMut(&str)>(path: &Path, decrypt_fn: F) -> Result<Self>` — 文件不存在返回空 SessionStore；读到后批量解密 EncryptedString | save→load 往返字段一致 |

### 2.4 SessionStore — 加密字段桥接

| # | 粒度 | 文件 | 要写的 | 验收 |
|---|------|------|--------|------|
| 2.4.1 | 0.5h | `crates/rterm-session/src/crypto_bridge.rs` | `pub fn encrypt_session(s: &Session) -> Result<Session>` — 深拷贝 + 每个 EncryptedString::data 用 dpapi_encrypt；`pub fn decrypt_session(s: &Session) -> Result<Session>` — 反向 | 调用后 EncryptedString.data 是密文长度 ≠ 明文长度 |
| 2.4.2 | 0.25h | `crates/rterm-session/src/lib.rs` | 模块声明 + `pub use store::*; pub use model::*; pub use crypto_bridge::*;` | `use rterm_session::SessionStore;` 可用 |

### 2.5 导入导出

| # | 粒度 | 文件 | 要写的 | 验收 |
|---|------|------|--------|------|
| 2.5.1 | 0.5h | `crates/rterm-session/src/export.rs` | `pub fn export_json(sessions: &[Session], scope: ExportScope, passphrase: Option<&str>) -> Result<String>` — 导出 JSON schema：`{"rterm-export":{"version":1,"exported_at":"...","kdf":...?, "sessions":[...]}}`；passphrase None → EncryptedString 直接 DPAPI；passphrase Some → 转 AES-GCM | 返回 String；能 serde_json::from_str 解析 |
| 2.5.2 | 0.5h | `crates/rterm-session/src/export.rs` | `pub fn import_json(json: &str, policy: ConflictPolicy, decrypt_fn: impl FnMut(&str)) -> Result<ImportReport>` — 解析 JSON；遍历 sessions；按 policy 处理冲突（覆盖/跳过/重命名）；调用 decrypt_fn 解密密码字段 | 导出→导入往返 Session 数量 + 字段一致 |
| 2.5.3 | 0.25h | `crates/rterm-session/src/export.rs` | `pub enum ExportScope { All, Folder(String), Selection(Vec<Uuid>) }` + `pub enum ConflictPolicy { Overwrite, Skip, Rename }` + `pub struct ImportReport { pub added: usize, pub updated: usize, pub skipped: usize, pub renamed: usize }` | 三个 enum 能序列化；ImportReport 能 print |

### 2.6 OpenSSH config 解析

| # | 粒度 | 文件 | 要写的 | 验收 |
|---|------|------|--------|------|
| 2.6.1 | 0.5h | `crates/rterm-session/src/openssh.rs` | `pub fn parse_openssh_config(path: &Path) -> Result<Vec<Session>>` — 解析 `Host xxx` 块 + HostName / Port / User / IdentityFile / ServerAliveInterval / Include（递归 1 层） | 给它 `~/.ssh/config` 样例 → 产出 Vec<Session> 字段正确 |

### 2.7 单元测试

| # | 粒度 | 文件 | 要写的 | 验收 |
|---|------|------|--------|------|
| 2.7.1 | 0.5h | `crates/rterm-session/tests/store_tests.rs` | save→load 往返；加密字段；upsert 更新；delete；文件不存在 → 空 store | 5 个测试过 |
| 2.7.2 | 0.5h | `crates/rterm-session/tests/export_tests.rs` | 导出 AES-GCM 加密 → 错误口令解密抛 Err → 正确口令往返；ConflictPolicy Overwrite/Skip/Rename 三种行为 | 8 个测试过 |
| 2.7.3 | 0.25h | `crates/rterm-session/tests/openssh_tests.rs` | 内联 openssh config 样例 → parse → 字段断言 | 1 个测试过 |

### 2.8 验收

| # | 粒度 | 验收 |
|---|------|------|
| 2.8.1 | 0.25h | `cargo test -p rterm-session` 全过；`cargo build -p rterm-app` 无 warning；所有密码字段在 JSON 里是 DPAPI 密文 base64（不可读） |

//! Session 数据模型 — 会话连接配置 + 认证方式 + 加密字符串容器
//!
//! 所有字段都 `#[derive(Serialize, Deserialize)]`，用 `#[serde(default)]` 给可选字段默认值，
//! 这样旧版本 JSON 升级到新版本不会因新字段缺失而反序列化失败。

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use uuid::Uuid;

/// 加密字符串容器 — 存密码或密钥口令。
///
/// 序列化格式：`{ "version": 1, "data": "base64..." }`
/// - version=0 表示明文（迁移用，正常不应出现）
/// - version=1 表示 DPAPI / AES-GCM 密文（取决于谁加密的）
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct EncryptedString {
    pub version: u8,
    #[serde(with = "serde_bytes")]
    pub data: Vec<u8>,
}

impl EncryptedString {
    pub fn new(plaintext: &str) -> Self {
        // 正常不应存明文；这里留给迁移/临时调试。
        Self { version: 0, data: plaintext.as_bytes().to_vec() }
    }

    pub fn empty() -> Self {
        Self { version: 1, data: Vec::new() }
    }

    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }
}

/// SSH 认证方式。
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AuthMethod {
    Password {
        password: EncryptedString,
    },
    PrivateKey {
        path: PathBuf,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        passphrase: Option<EncryptedString>,
    },
    Agent {
        #[serde(default)]
        key_comment: String,
    },
}

impl Default for AuthMethod {
    fn default() -> Self {
        Self::Password { password: EncryptedString::empty() }
    }
}

/// SSH 会话完整配置。
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Session {
    #[serde(default = "Uuid::new_v4")]
    pub id: Uuid,

    #[serde(default)]
    pub name: String,

    #[serde(default)]
    pub host: String,

    #[serde(default = "default_port")]
    pub port: u16,

    #[serde(default)]
    pub username: String,

    #[serde(default)]
    pub auth: AuthMethod,

    /// 跳板机代理链（None 表示直连）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proxy_jump: Option<Box<Session>>,

    #[serde(default = "default_keepalive")]
    pub keepalive_secs: u32,

    #[serde(default = "default_timeout")]
    pub connect_timeout_secs: u32,

    /// 分组路径，如 "生产/数据库"。None 表示根目录。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub folder: Option<String>,

    #[serde(default)]
    pub tags: Vec<String>,

    #[serde(default)]
    pub compress: bool,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub theme_override: Option<String>,

    #[serde(default = "Utc::now")]
    pub created_at: DateTime<Utc>,

    #[serde(default = "Utc::now")]
    pub last_modified: DateTime<Utc>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_connected: Option<DateTime<Utc>>,
}

fn default_port() -> u16 { 22 }
fn default_keepalive() -> u32 { 30 }
fn default_timeout() -> u32 { 10 }

impl Default for Session {
    fn default() -> Self {
        Self {
            id: Uuid::new_v4(),
            name: String::new(),
            host: String::new(),
            port: 22,
            username: String::new(),
            auth: AuthMethod::default(),
            proxy_jump: None,
            keepalive_secs: 30,
            connect_timeout_secs: 10,
            folder: None,
            tags: Vec::new(),
            compress: true,
            theme_override: None,
            created_at: Utc::now(),
            last_modified: Utc::now(),
            last_connected: None,
        }
    }
}

impl Session {
    pub fn new<S: Into<String>>(name: S, host: S, username: S) -> Self {
        Self {
            name: name.into(),
            host: host.into(),
            username: username.into(),
            ..Self::default()
        }
    }

    pub fn display_label(&self) -> String {
        if self.name.is_empty() {
            format!("{}@{}:{}", self.username, self.host, self.port)
        } else {
            self.name.clone()
        }
    }
}

// 自定义 bytes 序列化：EncryptedString.data 用 base64 存 JSON，人类不可读。
// ---- 这比 serde_bytes crate 更可控。
mod serde_bytes {
    use base64::{engine::general_purpose::STANDARD, Engine as _};
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S>(data: &Vec<u8>, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let b64 = STANDARD.encode(data);
        serializer.serialize_str(&b64)
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Vec<u8>, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        STANDARD.decode(&s).map_err(serde::de::Error::custom)
    }
}

// base64 crate 用哪个版本？让我查 workspace Cargo.toml 里有没有，没有的话 rterm-session 单独加。
// 先放这里，等会 Cargo check 如果说找不到再加依赖。

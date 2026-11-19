//! Session 数据模型 — 会话连接配置 + 认证方式 + 加密字符串容器
//!
//! 所有字段都 `#[derive(Serialize, Deserialize)]`，用 `#[serde(default)]` 给可选字段默认值，
//! 这样旧版本 JSON 升级到新版本不会因新字段缺失而反序列化失败。

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use uuid::Uuid;

/// 默认会话图标（未设置 / 非法值时回退）。
pub const DEFAULT_SESSION_ICON: &str = "🖥️";

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
        Self {
            version: 0,
            data: plaintext.as_bytes().to_vec(),
        }
    }

    pub fn empty() -> Self {
        Self {
            version: 1,
            data: Vec::new(),
        }
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
        Self::Password {
            password: EncryptedString::empty(),
        }
    }
}

/// SSH 端口转发类型（v1 支持本地 / 远程，动态转发为可选延伸）。
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ForwardKind {
    /// 本地转发：本地监听 → 经 SSH 访问远端目标（`ssh -L`）
    #[default]
    Local,
    /// 远程转发：远端监听 → 经 SSH 访问本地目标（`ssh -R`）
    Remote,
    /// 动态转发：本地 SOCKS5（`ssh -D`）
    Dynamic,
}

impl ForwardKind {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Local => "本地 -L",
            Self::Remote => "远程 -R",
            Self::Dynamic => "动态 -D",
        }
    }

    /// 该类型是否需要填写目标地址（动态转发不需要）。
    pub fn needs_target(&self) -> bool {
        !matches!(self, Self::Dynamic)
    }
}

/// 单条端口转发规则。
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct PortForward {
    #[serde(default = "Uuid::new_v4")]
    pub id: Uuid,
    #[serde(default)]
    pub kind: ForwardKind,
    /// 监听地址；默认只绑定回环，避免误暴露到局域网。
    #[serde(default = "default_bind_host")]
    pub bind_host: String,
    #[serde(default)]
    pub bind_port: u16,
    #[serde(default = "default_target_host")]
    pub target_host: String,
    #[serde(default)]
    pub target_port: u16,
    #[serde(default)]
    pub auto_start: bool,
}

fn default_bind_host() -> String {
    "127.0.0.1".to_string()
}

fn default_target_host() -> String {
    "127.0.0.1".to_string()
}

impl Default for PortForward {
    fn default() -> Self {
        Self {
            id: Uuid::new_v4(),
            kind: ForwardKind::Local,
            bind_host: default_bind_host(),
            bind_port: 0,
            target_host: default_target_host(),
            target_port: 0,
            auto_start: false,
        }
    }
}

impl PortForward {
    /// 单条规则校验：端口范围、目标必填、绑定地址非空。
    pub fn validate(&self) -> Result<(), String> {
        if self.bind_host.trim().is_empty() {
            return Err("监听地址不能为空".into());
        }
        if self.bind_port == 0 {
            return Err("监听端口必须是 1–65535".into());
        }
        if self.kind.needs_target() {
            if self.target_host.trim().is_empty() {
                return Err("目标地址不能为空".into());
            }
            if self.target_port == 0 {
                return Err("目标端口必须是 1–65535".into());
            }
        }
        Ok(())
    }

    /// 监听端点（用于同会话内端口冲突检测）。
    pub fn listener_endpoint(&self) -> (String, u16) {
        (self.bind_host.trim().to_string(), self.bind_port)
    }

    /// 单行展示文案，例如 `127.0.0.1:13306 → 10.0.0.5:3306`。
    pub fn summary(&self) -> String {
        match self.kind {
            ForwardKind::Local => format!(
                "{}:{} → {}:{}",
                self.bind_host, self.bind_port, self.target_host, self.target_port
            ),
            ForwardKind::Remote => format!(
                "远端 {}:{} → 本地 {}:{}",
                self.bind_host, self.bind_port, self.target_host, self.target_port
            ),
            ForwardKind::Dynamic => format!("SOCKS5 {}:{}", self.bind_host, self.bind_port),
        }
    }
}

/// 校验一个会话的全部转发规则（单条合法性 + 监听端口不冲突）。
pub fn validate_port_forwards(rules: &[PortForward]) -> Result<(), String> {
    let mut seen: Vec<(String, u16)> = Vec::new();
    for rule in rules {
        rule.validate()?;
        let endpoint = rule.listener_endpoint();
        if seen.contains(&endpoint) {
            return Err(format!("监听端口冲突: {}:{}", endpoint.0, endpoint.1));
        }
        seen.push(endpoint);
    }
    Ok(())
}

/// SSH 会话完整配置。
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Session {
    #[serde(default = "Uuid::new_v4")]
    pub id: Uuid,

    #[serde(default)]
    pub name: String,

    /// 会话图标（预设 emoji）；None 使用默认图标。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,

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

    /// 端口转发规则（可空）。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub port_forwards: Vec<PortForward>,

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

fn default_port() -> u16 {
    22
}
fn default_keepalive() -> u32 {
    30
}
fn default_timeout() -> u32 {
    10
}

/// 供外部 crate 获取当前时间（避免每个 crate 都直接依赖 chrono）
pub fn default_now() -> DateTime<Utc> {
    Utc::now()
}

impl Default for Session {
    fn default() -> Self {
        Self {
            id: Uuid::new_v4(),
            name: String::new(),
            icon: None,
            host: String::new(),
            port: 22,
            username: String::new(),
            auth: AuthMethod::default(),
            proxy_jump: None,
            port_forwards: Vec::new(),
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

    /// 会话图标：未设置或空白时回退默认图标。
    pub fn icon_or_default(&self) -> &str {
        match self.icon.as_deref() {
            Some(icon) if !icon.trim().is_empty() => icon,
            _ => DEFAULT_SESSION_ICON,
        }
    }

    /// 保存 / 连接前的整体校验（分组名、代理链、端口转发）。
    ///
    /// 只做纯数据校验，不触碰存储；调用方负责在失败时展示错误。
    pub fn validate(&self) -> Result<(), String> {
        if self.name.trim().is_empty() {
            return Err("会话名称不能为空".into());
        }
        if self.host.trim().is_empty() {
            return Err("主机地址不能为空".into());
        }
        if self.port == 0 {
            return Err("端口必须是 1–65535".into());
        }
        if let Some(folder) = &self.folder {
            crate::store::normalize_folder_path(folder)
                .map_err(|error| format!("分组路径非法: {error}"))?;
        }
        crate::proxy::build_chain(self).map_err(|error| format!("{error}"))?;
        validate_port_forwards(&self.port_forwards)?;
        Ok(())
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

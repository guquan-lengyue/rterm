//! rterm-session — 会话数据层
//!
//! 职责：Session 模型 + SessionStore CRUD + JSON 持久化 + 导入导出 + OpenSSH 解析
//! 本 crate 不依赖 gpui，纯数据层，可独立测试。

pub mod crypto_bridge;
pub mod export;
pub mod model;
pub mod openssh;
pub mod store;

pub use export::{
    ConflictPolicy, ExportScope, ImportPreviewItem, ImportPreviewStatus, ImportReport,
};
pub use model::{default_now, AuthMethod, EncryptedString, Session};
pub use store::{LoadIssue, SessionStore, UpsertResult};

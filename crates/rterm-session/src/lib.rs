//! rterm-session — 会话数据层
//!
//! 职责：Session 模型 + SessionStore CRUD + JSON 持久化 + 导入导出 + OpenSSH 解析
//! 本 crate 不依赖 gpui，纯数据层，可独立测试。

pub mod model;
pub mod store;
pub mod crypto_bridge;
pub mod export;
pub mod openssh;

pub use model::{AuthMethod, EncryptedString, Session, default_now};
pub use store::{LoadIssue, SessionStore, UpsertResult};
pub use export::{
    ConflictPolicy, ExportScope, ImportPreviewItem, ImportPreviewStatus, ImportReport,
};

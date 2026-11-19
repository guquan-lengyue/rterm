//! rterm-session — 会话数据层
//!
//! 职责：Session 模型 + SessionStore CRUD + JSON 持久化 + 导入导出 + OpenSSH 解析
//! 本 crate 不依赖 gpui，纯数据层，可独立测试。

pub mod backup;
pub mod crypto_bridge;
pub mod export;
pub mod model;
pub mod openssh;
pub mod proxy;
pub mod store;

pub use export::{
    ConflictPolicy, ExportScope, ImportPreviewItem, ImportPreviewStatus, ImportReport,
};
pub use model::{
    default_now, validate_port_forwards, AuthMethod, EncryptedString, ForwardKind, PortForward,
    Session, DEFAULT_SESSION_ICON,
};
pub use proxy::{
    build_chain, can_be_proxy_of, chain_labels, describe_chain, selection_forms_cycle, MAX_JUMPS,
};
pub use store::{normalize_folder_path, LoadIssue, SessionStore, UpsertResult};

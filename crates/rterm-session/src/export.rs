//! export — 会话导入导出
//!
//! 导出格式（自描述 JSON）：
//! ```json
//! {
//!   "rterm-export": {
//!     "version": 1,
//!     "exported_at": "2026-11-13T...Z",
//!     "sessions": [...]
//!   }
//! }
//! ```
//!
//! passphrase=None  → EncryptedString 保持 DPAPI 密文（仅本机/本用户可解）
//! passphrase=Some  → 全部转 AES-GCM 密文（跨机器可解）

use anyhow::{Context, Result};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::crypto_bridge;
use crate::model::Session;

// ============================================================================
// 公共类型
// ============================================================================

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ExportScope {
    /// 所有会话
    All,
    /// 某个文件夹下的会话
    Folder(String),
    /// 明确选中的若干 id
    Selection(Vec<Uuid>),
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq)]
pub enum ConflictPolicy {
    /// 用导入的覆盖已有的（id 相同）
    Overwrite,
    /// 跳过已有的，只新增不存在的
    Skip,
    /// 已有的改名（后缀 _imported_时间戳）后新增
    Rename,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ImportReport {
    pub added: usize,
    pub updated: usize,
    pub skipped: usize,
    pub renamed: usize,
}

/// 导入预览：单个会话将要执行的动作。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImportPreviewStatus {
    Added,
    Updated,
    Skipped,
    Renamed,
}

#[derive(Clone, Debug)]
pub struct ImportPreviewItem {
    pub id: Uuid,
    pub name: String,
    pub status: ImportPreviewStatus,
}

// ============================================================================
// 导出
// ============================================================================

const EXPORT_VERSION: u32 = 1;

#[derive(Serialize)]
struct ExportWrapper {
    #[serde(rename = "rterm-export")]
    tag: ExportTag,
}

#[derive(Serialize)]
struct ExportTag {
    version: u32,
    exported_at: chrono::DateTime<Utc>,
    sessions: Vec<Session>,
}

pub fn export_json(
    sessions: &[Session],
    scope: ExportScope,
    passphrase: Option<&str>,
) -> Result<String> {
    let picked = apply_scope(sessions, scope);

    let encrypted: Result<Vec<_>> = if let Some(pw) = passphrase {
        picked
            .iter()
            .map(|s| crypto_bridge::encrypt_session_aesgcm(s, pw))
            .collect()
    } else {
        picked
            .iter()
            .map(|s| crypto_bridge::encrypt_session_dpapi(s))
            .collect()
    };
    let encrypted = encrypted.context("导出加密失败")?;

    let wrapper = ExportWrapper {
        tag: ExportTag {
            version: EXPORT_VERSION,
            exported_at: Utc::now(),
            sessions: encrypted,
        },
    };

    serde_json::to_string_pretty(&wrapper).context("序列化导出 JSON 失败")
}

fn apply_scope(sessions: &[Session], scope: ExportScope) -> Vec<&Session> {
    match scope {
        ExportScope::All => sessions.iter().collect(),
        ExportScope::Folder(ref folder) => sessions
            .iter()
            .filter(|s| s.folder.as_deref() == Some(folder.as_str()))
            .collect(),
        ExportScope::Selection(ids) => {
            let set: std::collections::HashSet<Uuid> = ids.into_iter().collect();
            sessions.iter().filter(|s| set.contains(&s.id)).collect()
        }
    }
}

// ============================================================================
// 导入
// ============================================================================

#[derive(Deserialize)]
struct ImportWrapper {
    #[serde(rename = "rterm-export")]
    tag: Option<ImportTag>,
}

#[derive(Deserialize)]
struct ImportTag {
    #[allow(dead_code)]
    version: u32,
    #[allow(dead_code)]
    exported_at: chrono::DateTime<Utc>,
    sessions: Vec<Session>,
}

/// 读取导出 JSON 并按冲突策略预演每条会话的处理结果（不修改任何数据）。
pub fn preview_import(
    json: &str,
    existing: &[Session],
    policy: ConflictPolicy,
    passphrase: Option<&str>,
) -> Result<Vec<ImportPreviewItem>> {
    let import = parse_and_decrypt(json, passphrase)?;

    Ok(import
        .into_iter()
        .map(|s| {
            let status = if existing.iter().any(|e| e.id == s.id) {
                match policy {
                    ConflictPolicy::Overwrite => ImportPreviewStatus::Updated,
                    ConflictPolicy::Skip => ImportPreviewStatus::Skipped,
                    ConflictPolicy::Rename => ImportPreviewStatus::Renamed,
                }
            } else {
                ImportPreviewStatus::Added
            };
            ImportPreviewItem {
                id: s.id,
                name: s.display_label(),
                status,
            }
        })
        .collect())
}

/// 解析导出 JSON 并解密所有会话。导入 / 预览共用。
fn parse_and_decrypt(json: &str, passphrase: Option<&str>) -> Result<Vec<Session>> {
    let wrapper: ImportWrapper = serde_json::from_str(json).context("解析导出 JSON 失败")?;
    let tag = wrapper.tag.context("缺少 rterm-export 标记")?;

    let decrypted: Result<Vec<_>> = if let Some(pw) = passphrase {
        tag.sessions
            .iter()
            .map(|s| crypto_bridge::decrypt_session_aesgcm(s, pw))
            .collect()
    } else {
        tag.sessions
            .iter()
            .map(crypto_bridge::decrypt_session_dpapi)
            .collect()
    };
    decrypted.context("导入解密失败 — 可能需要口令")
}

pub fn import_json(
    json: &str,
    existing: &[Session],
    policy: ConflictPolicy,
    passphrase: Option<&str>,
) -> Result<(Vec<Session>, ImportReport)> {
    let decrypted = parse_and_decrypt(json, passphrase)?;

    let mut report = ImportReport::default();
    let mut merged: Vec<Session> = existing.to_vec();

    for incoming in decrypted {
        let existing_idx = merged.iter().position(|s| s.id == incoming.id);

        match (existing_idx, policy) {
            (None, _) => {
                merged.push(incoming);
                report.added += 1;
            }
            (Some(_), ConflictPolicy::Overwrite) => {
                // replace
                if let Some(idx) = existing_idx {
                    merged[idx] = incoming;
                }
                report.updated += 1;
            }
            (Some(_), ConflictPolicy::Skip) => {
                report.skipped += 1;
            }
            (Some(_), ConflictPolicy::Rename) => {
                let mut renamed = incoming.clone();
                renamed.name = format!(
                    "{}_imported_{}",
                    renamed.name,
                    Utc::now().format("%Y%m%d%H%M%S")
                );
                renamed.id = Uuid::new_v4();
                merged.push(renamed);
                report.renamed += 1;
            }
        }
    }

    Ok((merged, report))
}

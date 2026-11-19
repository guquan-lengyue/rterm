//! export — 会话导入导出（schema v2）
//!
//! 导出格式（自描述 JSON）：
//! ```json
//! {
//!   "rterm-export": {
//!     "version": 2,
//!     "exported_at": "2026-11-19T...Z",
//!     "folders": ["生产", "测试/空分组"],
//!     "sessions": [...]
//!   }
//! }
//! ```
//!
//! passphrase=None  → EncryptedString 保持 DPAPI 密文（仅本机/本用户可解）
//! passphrase=Some  → 全部转 AES-GCM 密文（跨机器可解）
//!
//! v2 新增 `folders`（显式分组）；v1 文件缺少该字段，按空列表读取。

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
    /// 某个文件夹及其子分组下的会话
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

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
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
    /// 分组路径（None = 根目录）
    pub folder: Option<String>,
    /// 会话图标（None = 默认图标）
    pub icon: Option<String>,
    /// 是否配置了跳板代理
    pub has_proxy: bool,
    /// 端口转发规则数量
    pub port_forwards: usize,
}

// ============================================================================
// 导出
// ============================================================================

const EXPORT_VERSION: u32 = 2;

#[derive(Serialize)]
struct ExportWrapper {
    #[serde(rename = "rterm-export")]
    tag: ExportTag,
}

#[derive(Serialize)]
struct ExportTag {
    version: u32,
    exported_at: chrono::DateTime<Utc>,
    folders: Vec<String>,
    sessions: Vec<Session>,
}

/// 导出选中范围的会话与分组。
///
/// `folders` 传入全部显式分组；函数会按 scope 裁剪，并补齐导出会话的父路径。
pub fn export_json(
    sessions: &[Session],
    folders: &[String],
    scope: ExportScope,
    passphrase: Option<&str>,
) -> Result<String> {
    let picked = apply_scope(sessions, &scope);
    let picked_folders = scope_folders(sessions, folders, &scope, &picked);

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
            folders: picked_folders,
            sessions: encrypted,
        },
    };

    serde_json::to_string_pretty(&wrapper).context("序列化导出 JSON 失败")
}

fn apply_scope<'a>(sessions: &'a [Session], scope: &ExportScope) -> Vec<&'a Session> {
    match scope {
        ExportScope::All => sessions.iter().collect(),
        ExportScope::Folder(folder) => {
            let folder = folder.trim_matches('/');
            let prefix = format!("{folder}/");
            sessions
                .iter()
                .filter(|s| {
                    s.folder
                        .as_deref()
                        .map(|f| f == folder || f.starts_with(&prefix))
                        .unwrap_or(false)
                })
                .collect()
        }
        ExportScope::Selection(ids) => {
            let set: std::collections::HashSet<Uuid> = ids.iter().copied().collect();
            sessions.iter().filter(|s| set.contains(&s.id)).collect()
        }
    }
}

/// 裁剪要导出的显式分组，并补齐导出会话引用到的分组路径。
fn scope_folders(
    _sessions: &[Session],
    all_folders: &[String],
    scope: &ExportScope,
    picked: &[&Session],
) -> Vec<String> {
    let mut result: Vec<String> = Vec::new();
    match scope {
        ExportScope::All => result.extend(all_folders.iter().cloned()),
        ExportScope::Folder(folder) => {
            let folder = folder.trim_matches('/');
            let prefix = format!("{folder}/");
            result.extend(
                all_folders
                    .iter()
                    .filter(|f| f.as_str() == folder || f.starts_with(&prefix))
                    .cloned(),
            );
        }
        // 勾选导出不带显式空分组，只保留会话实际引用到的路径。
        ExportScope::Selection(_) => {}
    }

    for session in picked {
        if let Some(folder) = session.folder.as_deref() {
            extend_ancestor_paths(&mut result, folder);
        }
    }

    result.sort();
    result.dedup();
    result
}

/// 追加 `path` 及其全部父路径。
fn extend_ancestor_paths(out: &mut Vec<String>, path: &str) {
    let path = path.trim().trim_matches('/');
    if path.is_empty() {
        return;
    }
    let mut acc = String::new();
    for segment in path.split('/').filter(|segment| !segment.is_empty()) {
        if !acc.is_empty() {
            acc.push('/');
        }
        acc.push_str(segment);
        out.push(acc.clone());
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
    #[serde(default)]
    #[allow(dead_code)]
    version: u32,
    #[serde(default)]
    #[allow(dead_code)]
    exported_at: Option<chrono::DateTime<Utc>>,
    #[serde(default)]
    folders: Vec<String>,
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
    Ok(preview_sessions(&import.sessions, existing, policy))
}

/// 对一批已解密的会话生成导入预览（OpenSSH config 导入复用）。
pub fn preview_sessions(
    incoming: &[Session],
    existing: &[Session],
    policy: ConflictPolicy,
) -> Vec<ImportPreviewItem> {
    incoming
        .iter()
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
                folder: s.folder.clone(),
                icon: s.icon.clone(),
                has_proxy: s.proxy_jump.is_some(),
                port_forwards: s.port_forwards.len(),
            }
        })
        .collect()
}

/// 解析导出 JSON 并解密所有会话。导入 / 预览共用。
fn parse_and_decrypt(json: &str, passphrase: Option<&str>) -> Result<ImportData> {
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
    Ok(ImportData {
        folders: tag.folders,
        sessions: decrypted.context("导入解密失败 — 可能需要口令")?,
    })
}

struct ImportData {
    folders: Vec<String>,
    sessions: Vec<Session>,
}

/// 导入会话并合并分组。
///
/// 返回 `(合并后的会话, 合并后的显式分组, 统计)`；调用方负责写回
/// `SessionStore` 并持久化。
pub fn import_json(
    json: &str,
    existing: &[Session],
    existing_folders: &[String],
    policy: ConflictPolicy,
    passphrase: Option<&str>,
) -> Result<(Vec<Session>, Vec<String>, ImportReport)> {
    let imported = parse_and_decrypt(json, passphrase)?;
    Ok(merge_sessions(
        imported.sessions,
        existing,
        &merge_folders(existing_folders, &imported.folders),
        policy,
    ))
}

/// 合并导入会话与分组（`import_json` 与 OpenSSH 导入共用）。
///
/// `existing_folders` 应已包含导入文件中声明的显式分组。
pub fn merge_sessions(
    incoming: Vec<Session>,
    existing: &[Session],
    existing_folders: &[String],
    policy: ConflictPolicy,
) -> (Vec<Session>, Vec<String>, ImportReport) {
    let mut report = ImportReport::default();
    let mut merged: Vec<Session> = existing.to_vec();
    let mut folders: Vec<String> = existing_folders.to_vec();

    for incoming in incoming {
        let existing_idx = merged.iter().position(|s| s.id == incoming.id);
        let stored = match (existing_idx, policy) {
            (None, _) => {
                merged.push(incoming.clone());
                report.added += 1;
                incoming
            }
            (Some(_), ConflictPolicy::Overwrite) => {
                if let Some(idx) = existing_idx {
                    merged[idx] = incoming.clone();
                }
                report.updated += 1;
                incoming
            }
            (Some(_), ConflictPolicy::Skip) => {
                report.skipped += 1;
                continue;
            }
            (Some(_), ConflictPolicy::Rename) => {
                let mut renamed = incoming;
                renamed.name = format!(
                    "{}_imported_{}",
                    renamed.name,
                    Utc::now().format("%Y%m%d%H%M%S")
                );
                renamed.id = Uuid::new_v4();
                merged.push(renamed.clone());
                report.renamed += 1;
                renamed
            }
        };

        if let Some(folder) = stored.folder.as_deref() {
            extend_ancestor_paths(&mut folders, folder);
        }
    }

    folders.sort();
    folders.dedup();
    (merged, folders, report)
}

/// 合并两组显式分组：保留双方并补齐父路径。
pub fn merge_folders(existing: &[String], incoming: &[String]) -> Vec<String> {
    let mut folders = existing.to_vec();
    for folder in incoming {
        extend_ancestor_paths(&mut folders, folder);
    }
    folders.sort();
    folders.dedup();
    folders
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{AuthMethod, EncryptedString};

    fn session(name: &str, folder: Option<&str>) -> Session {
        Session {
            name: name.into(),
            icon: Some("🐧".into()),
            host: "example.com".into(),
            username: "ops".into(),
            auth: AuthMethod::Password {
                password: EncryptedString::new("secret"),
            },
            folder: folder.map(str::to_string),
            ..Session::default()
        }
    }

    #[test]
    fn export_scope_all_contains_folders_and_icon() {
        let sessions = vec![session("web", Some("生产/数据库"))];
        let folders = vec!["生产".to_string(), "生产/数据库".to_string()];
        let json = export_json(&sessions, &folders, ExportScope::All, Some("pw")).expect("导出");

        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        let tag = &value["rterm-export"];
        assert_eq!(tag["version"], 2);
        assert_eq!(tag["folders"].as_array().unwrap().len(), 2);
        assert_eq!(tag["sessions"][0]["icon"], "🐧");
    }

    #[test]
    fn folder_scope_includes_descendants() {
        let sessions = vec![
            session("a", Some("生产")),
            session("b", Some("生产/数据库")),
            session("c", Some("测试")),
        ];
        let json = export_json(
            &sessions,
            &["生产".into(), "生产/数据库".into(), "测试".into()],
            ExportScope::Folder("生产".into()),
            Some("pw"),
        )
        .expect("导出");
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(
            value["rterm-export"]["sessions"].as_array().unwrap().len(),
            2
        );
        assert_eq!(
            value["rterm-export"]["folders"].as_array().unwrap().len(),
            2,
            "只导出该分组及其子分组"
        );
    }

    #[test]
    fn import_v1_file_without_folders() {
        // v1：没有 folders 字段
        let legacy = session("legacy", Some("旧分组"));
        let json = serde_json::json!({
            "rterm-export": {
                "version": 1,
                "exported_at": "2026-01-01T00:00:00Z",
                "sessions": [serde_json::to_value(&legacy).unwrap()],
            }
        })
        .to_string();

        let preview = preview_import(&json, &[], ConflictPolicy::Skip, None).expect("预览");
        assert_eq!(preview.len(), 1);
        assert_eq!(preview[0].folder.as_deref(), Some("旧分组"));

        let (merged, folders, report) =
            import_json(&json, &[], &[], ConflictPolicy::Skip, None).expect("导入");
        assert_eq!(merged.len(), 1);
        assert_eq!(report.added, 1);
        assert!(folders.contains(&"旧分组".to_string()));
    }

    #[test]
    fn conflict_policies_behave_as_expected() {
        let existing = vec![session("web", None)];
        let same_id = existing[0].clone();
        let json = serde_json::json!({
            "rterm-export": {
                "version": 2,
                "folders": [],
                "sessions": [serde_json::to_value(&same_id).unwrap()],
            }
        })
        .to_string();

        let (_, _, skip) = import_json(&json, &existing, &[], ConflictPolicy::Skip, None).unwrap();
        assert_eq!(skip.skipped, 1);

        let (_, _, overwrite) =
            import_json(&json, &existing, &[], ConflictPolicy::Overwrite, None).unwrap();
        assert_eq!(overwrite.updated, 1);

        let (merged, _, rename) =
            import_json(&json, &existing, &[], ConflictPolicy::Rename, None).unwrap();
        assert_eq!(rename.renamed, 1);
        assert_eq!(merged.len(), 2);
        assert!(merged[1].name.contains("_imported_"));
    }

    #[test]
    fn preview_reports_icon_proxy_and_forward_count() {
        let mut s = session("jump-target", Some("a/b"));
        s.proxy_jump = Some(Box::new(session("jump", Some("a"))));
        s.port_forwards = vec![crate::model::PortForward {
            bind_port: 13306,
            target_port: 3306,
            ..Default::default()
        }];
        let json = serde_json::json!({
            "rterm-export": {
                "version": 2,
                "folders": ["a", "a/b"],
                "sessions": [serde_json::to_value(&s).unwrap()],
            }
        })
        .to_string();

        let preview = preview_import(&json, &[], ConflictPolicy::Skip, None).unwrap();
        assert!(preview[0].has_proxy);
        assert_eq!(preview[0].port_forwards, 1);
        assert_eq!(preview[0].icon.as_deref(), Some("🐧"));
    }
}

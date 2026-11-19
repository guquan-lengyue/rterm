//! SessionStore — 会话内存 CRUD + JSON 持久化
//!
//! 线程模型：SessionStore 通常被 gpui::Global(AppState) 持有，由 gpui 的单线程 UI 事件循环访问，
//! 所以不需要 Mutex。如果将来需要后台线程读写，外层用 parking_lot::RwLock 包一层即可。

use anyhow::{Context, Result};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use uuid::Uuid;

use crate::model::Session;

// ============================================================================
// Store struct
// ============================================================================

#[derive(Debug, Default, Clone)]
pub struct SessionStore {
    sessions: Vec<Session>,
    /// 显式分组（会话文件 v2）：允许空分组、重命名；与 `Session.folder` 派生分组合并展示。
    folders: Vec<String>,
    dirty: bool,
}

impl SessionStore {
    pub fn new() -> Self {
        Self {
            sessions: Vec::new(),
            folders: Vec::new(),
            dirty: false,
        }
    }

    /// 列出所有会话（不可变引用）。
    pub fn list(&self) -> &[Session] {
        &self.sessions
    }

    /// 按 id 查找会话。
    pub fn get(&self, id: Uuid) -> Option<&Session> {
        self.sessions.iter().find(|s| s.id == id)
    }

    /// 按 id 查找可变引用（Sprint 3+ UI 编辑时用）。
    pub fn get_mut(&mut self, id: Uuid) -> Option<&mut Session> {
        self.sessions.iter_mut().find(|s| s.id == id)
    }

    pub fn count(&self) -> usize {
        self.sessions.len()
    }

    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    // ------------------------------------------------------------------
    // CRUD
    // ------------------------------------------------------------------

    /// 插入或更新会话。
    /// - 若 id 已存在 → 更新 + 刷新 last_modified
    /// - 若 id 不存在 → 追加
    ///
    /// 返回本次操作类型（给上层发 EventBus 事件）。
    pub fn upsert(&mut self, mut session: Session) -> UpsertResult {
        let existing_idx = self.sessions.iter().position(|s| s.id == session.id);
        match existing_idx {
            Some(idx) => {
                session.last_modified = Utc::now();
                self.sessions[idx] = session;
                self.dirty = true;
                UpsertResult::Updated
            }
            None => {
                session.last_modified = Utc::now();
                self.sessions.push(session);
                self.dirty = true;
                UpsertResult::Created
            }
        }
    }

    /// 按 id 删除。返回 true 表示确实删了。
    pub fn delete(&mut self, id: Uuid) -> bool {
        let before = self.sessions.len();
        self.sessions.retain(|s| s.id != id);
        if self.sessions.len() != before {
            self.dirty = true;
            true
        } else {
            false
        }
    }

    pub fn clear(&mut self) {
        self.sessions.clear();
        self.dirty = true;
    }

    // ------------------------------------------------------------------
    // 分组 CRUD（v2）
    // ------------------------------------------------------------------

    /// 显式分组列表（不含会话派生）。
    pub fn explicit_folders(&self) -> &[String] {
        &self.folders
    }

    /// 全部已知分组：显式分组 ∪ 会话 `folder` 派生路径（含各级父路径），已排序去重。
    pub fn list_folders(&self) -> Vec<String> {
        let mut folders: Vec<String> = self.folders.clone();
        for session in &self.sessions {
            if let Some(folder) = session.folder.as_deref() {
                extend_ancestor_paths(&mut folders, folder);
            }
        }
        folders.sort();
        folders.dedup();
        folders
    }

    /// 分组是否存在（显式或会话派生）。
    pub fn folder_exists(&self, path: &str) -> bool {
        match normalize_folder_path(path) {
            Ok(path) => self.list_folders().iter().any(|folder| folder == &path),
            Err(_) => false,
        }
    }

    /// 新建分组（允许空分组）。已存在时返回错误。
    pub fn create_folder(&mut self, path: &str) -> Result<()> {
        let path = normalize_folder_path(path)?;
        if self.folder_exists(&path) {
            anyhow::bail!("分组「{path}」已存在");
        }
        // 同时补齐父路径，使空分组的层级在重启后依旧完整。
        let mut ancestors = Vec::new();
        extend_ancestor_paths(&mut ancestors, &path);
        for folder in ancestors {
            if !self.folders.contains(&folder) {
                self.folders.push(folder);
            }
        }
        self.dirty = true;
        Ok(())
    }

    /// 重命名分组：批量更新显式分组前缀与会话 `folder`。
    ///
    /// `from` 必须存在；`to` 不能已存在（避免覆盖）；`to` 不能是 `from` 的子路径
    /// （避免把父分组搬进自己）。
    pub fn rename_folder(&mut self, from: &str, to: &str) -> Result<()> {
        let from = normalize_folder_path(from)?;
        let to = normalize_folder_path(to)?;
        if from == to {
            return Ok(());
        }
        if !self.folder_exists(&from) {
            anyhow::bail!("分组「{from}」不存在");
        }
        if self.folder_exists(&to) {
            anyhow::bail!("分组「{to}」已存在");
        }
        if to.starts_with(&format!("{from}/")) {
            anyhow::bail!("不能把分组移动到自身子路径");
        }

        let rename = |path: &str| -> Option<String> {
            if path == from {
                Some(to.clone())
            } else {
                path.strip_prefix(&format!("{from}/"))
                    .map(|suffix| format!("{to}/{suffix}"))
            }
        };

        for folder in &mut self.folders {
            if let Some(renamed) = rename(folder) {
                *folder = renamed;
            }
        }
        for session in &mut self.sessions {
            if let Some(folder) = session.folder.as_deref() {
                if let Some(renamed) = rename(folder) {
                    session.folder = Some(renamed);
                }
            }
        }
        self.folders.sort();
        self.folders.dedup();
        self.dirty = true;
        Ok(())
    }

    /// 删除空分组：仅当没有会话、也没有子分组时允许。
    pub fn delete_empty_folder(&mut self, path: &str) -> Result<()> {
        let path = normalize_folder_path(path)?;
        if !self.folder_exists(&path) {
            anyhow::bail!("分组「{path}」不存在");
        }
        let prefix = format!("{path}/");
        let has_child_folder = self
            .list_folders()
            .iter()
            .any(|folder| folder != &path && folder.starts_with(&prefix));
        if has_child_folder {
            anyhow::bail!("分组「{path}」下还有子分组，无法删除");
        }
        let has_session = self.sessions.iter().any(|session| {
            session
                .folder
                .as_deref()
                .map(|folder| folder == path || folder.starts_with(&prefix))
                .unwrap_or(false)
        });
        if has_session {
            anyhow::bail!("分组「{path}」下还有会话，无法删除");
        }

        self.folders.retain(|folder| folder != &path);
        self.dirty = true;
        Ok(())
    }

    /// 把会话移动到指定分组（None = 根目录）。
    pub fn move_session(&mut self, id: Uuid, folder: Option<&str>) -> Result<()> {
        let folder = match folder {
            Some(path) if !path.trim().is_empty() => {
                let path = normalize_folder_path(path)?;
                if !self.folder_exists(&path) {
                    anyhow::bail!("分组「{path}」不存在");
                }
                Some(path)
            }
            _ => None,
        };

        let session = self
            .sessions
            .iter_mut()
            .find(|session| session.id == id)
            .ok_or_else(|| anyhow::anyhow!("会话不存在"))?;
        if session.folder == folder {
            return Ok(());
        }
        session.folder = folder;
        session.last_modified = Utc::now();
        self.dirty = true;
        Ok(())
    }

    /// 整体替换显式分组（导入 v2 文件后使用），自动补齐父路径并排序去重。
    pub fn set_folders(&mut self, folders: Vec<String>) {
        let mut merged = Vec::new();
        for folder in folders {
            if let Ok(path) = normalize_folder_path(&folder) {
                extend_ancestor_paths(&mut merged, &path);
            }
        }
        merged.sort();
        merged.dedup();
        self.folders = merged;
        self.dirty = true;
    }

    // ------------------------------------------------------------------
    // 持久化
    // ------------------------------------------------------------------

    /// 保存到 JSON 文件。
    ///
    /// `encrypt_fn` 在写磁盘前对每个 EncryptedString 字段调用（DPAPI 或 AES-GCM）。
    /// 原子写：先 tmp 再 rename，避免写一半崩溃留下半截文件。
    pub fn save<F>(&mut self, path: &Path, encrypt_fn: F) -> Result<()>
    where
        F: FnMut(&Session) -> Result<Session>,
    {
        self.save_inner(path, encrypt_fn)?;
        self.dirty = false;
        Ok(())
    }

    fn save_inner<F>(&self, path: &Path, encrypt_fn: F) -> Result<()>
    where
        F: FnMut(&Session) -> Result<Session>,
    {
        // 加密后再序列化，避免明文落盘。
        let encrypted: Result<Vec<_>> = self.sessions.iter().map(encrypt_fn).collect();
        let encrypted = encrypted.context("加密会话失败")?;

        let wrapper = SessionFile {
            version: FILE_VERSION,
            saved_at: Utc::now(),
            folders: self.folders.clone(),
            sessions: encrypted,
        };

        let json = serde_json::to_string_pretty(&wrapper).context("序列化会话 JSON 失败")?;

        // 原子写：父目录 → tmp → rename
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent).ok();
            }
        }
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, &json)
            .with_context(|| format!("写临时文件失败: {}", tmp.display()))?;
        std::fs::rename(&tmp, path)
            .with_context(|| format!("重命名到目标失败: {}", path.display()))?;

        Ok(())
    }

    /// 从 JSON 文件加载。
    ///
    /// 文件不存在 → 返回空 SessionStore（首次启动场景）。
    /// 读盘后立刻对每个 EncryptedString 字段调用 `decrypt_fn` 解出明文。
    pub fn load<F>(path: &Path, decrypt_fn: F) -> Result<Self>
    where
        F: FnMut(&Session) -> Result<Session>,
    {
        if !path.exists() {
            log::info!("会话文件不存在，返回空 store: {}", path.display());
            return Ok(Self::new());
        }
        Self::load_inner(path, decrypt_fn)
    }

    fn load_inner<F>(path: &Path, mut decrypt_fn: F) -> Result<Self>
    where
        F: FnMut(&Session) -> Result<Session>,
    {
        let wrapper = read_session_file(path)?;
        let folders = sanitize_folders(&wrapper.folders);
        let decrypted: Result<Vec<_>> = wrapper.sessions.iter().map(&mut decrypt_fn).collect();
        let sessions = decrypted.context("解密会话失败 — 密码或密钥可能不匹配")?;

        Ok(Self {
            sessions,
            folders,
            dirty: false,
        })
    }

    /// 容错加载：逐条解密，单条失败只记录 `LoadIssue`，不整体失败。
    ///
    /// 解密失败的条目保留磁盘上的原始内容（仍是 version=1 密文），
    /// 因此后续 save 时不会被重复加密，也不会丢数据。
    pub fn load_lossy<F>(path: &Path, mut decrypt_fn: F) -> Result<(Self, Vec<LoadIssue>)>
    where
        F: FnMut(&Session) -> Result<Session>,
    {
        if !path.exists() {
            log::info!("会话文件不存在，返回空 store: {}", path.display());
            return Ok((Self::new(), Vec::new()));
        }

        let wrapper = read_session_file(path)?;

        let mut sessions = Vec::with_capacity(wrapper.sessions.len());
        let mut issues = Vec::new();
        for original in wrapper.sessions {
            match decrypt_fn(&original) {
                Ok(decrypted) => sessions.push(decrypted),
                Err(e) => {
                    log::error!(
                        "会话 {}（{}）解密失败，保留原密文: {e:#}",
                        original.name,
                        original.id
                    );
                    issues.push(LoadIssue {
                        id: original.id,
                        name: original.name.clone(),
                        error: format!("{e:#}"),
                    });
                    sessions.push(original);
                }
            }
        }

        Ok((
            Self {
                sessions,
                folders: sanitize_folders(&wrapper.folders),
                dirty: false,
            },
            issues,
        ))
    }

    /// save + load 往返验证（测试用）。
    #[cfg(test)]
    pub fn roundtrip<FE, FD>(&mut self, path: &Path, encrypt: FE, decrypt: FD) -> Result<bool>
    where
        FE: FnMut(&Session) -> Result<Session>,
        FD: FnMut(&Session) -> Result<Session>,
    {
        let original = self.sessions.clone();
        let original_folders = self.folders.clone();
        self.save(path, encrypt)?;
        let loaded = Self::load(path, decrypt)?;
        Ok(loaded.sessions == original && loaded.folders == original_folders)
    }
}

// ============================================================================
// 事件 & 辅助类型
// ============================================================================

pub enum UpsertResult {
    Created,
    Updated,
}

/// 单条会话解密失败的记录（`load_lossy` 用）。
#[derive(Debug, Clone)]
pub struct LoadIssue {
    pub id: Uuid,
    pub name: String,
    pub error: String,
}

const FILE_VERSION: u32 = 2;

/// 磁盘 JSON 包装格式（v2 起含显式分组；v1 文件无 `folders` 字段，默认为空）。
#[derive(Serialize, Deserialize)]
struct SessionFile {
    version: u32,
    saved_at: chrono::DateTime<Utc>,
    #[serde(default)]
    folders: Vec<String>,
    sessions: Vec<Session>,
}

/// 规范化分组路径：`/a/b/` → `a/b`；拒绝空路径与空段。
///
/// 分组路径是唯一键（不引入 Group ID），因此统一在这里做格式收敛。
pub fn normalize_folder_path(path: &str) -> Result<String> {
    let trimmed = path.trim().trim_matches('/');
    if trimmed.is_empty() {
        anyhow::bail!("分组名称不能为空");
    }
    let mut segments = Vec::new();
    for segment in trimmed.split('/') {
        let segment = segment.trim();
        if segment.is_empty() {
            anyhow::bail!("分组路径不能包含空层级");
        }
        segments.push(segment);
    }
    Ok(segments.join("/"))
}

/// 把 `path` 及其所有父路径追加进 `out`（未去重，调用方负责排序去重）。
fn extend_ancestor_paths(out: &mut Vec<String>, path: &str) {
    let Ok(path) = normalize_folder_path(path) else {
        return;
    };
    let mut acc = String::new();
    for segment in path.split('/') {
        if !acc.is_empty() {
            acc.push('/');
        }
        acc.push_str(segment);
        out.push(acc.clone());
    }
}

/// 读盘时清洗显式分组：规范化 + 补父路径 + 排序去重。
fn sanitize_folders(folders: &[String]) -> Vec<String> {
    let mut result = Vec::new();
    for folder in folders {
        extend_ancestor_paths(&mut result, folder);
    }
    result.sort();
    result.dedup();
    result
}

/// 读文件 + 解析 + 版本检查（load / load_lossy 共用）。
fn read_session_file(path: &Path) -> Result<SessionFile> {
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("读会话文件失败: {}", path.display()))?;

    let wrapper: SessionFile = serde_json::from_str(&raw)
        .with_context(|| format!("解析会话 JSON 失败: {}", path.display()))?;

    // 版本迁移：旧版本直接兼容，新版本才 bail。
    if wrapper.version > FILE_VERSION {
        anyhow::bail!(
            "会话文件版本 {} 高于当前支持的 {}，可能来自新版本 rterm",
            wrapper.version,
            FILE_VERSION
        );
    }

    Ok(wrapper)
}

// ============================================================================
// 便捷构造
// ============================================================================

impl SessionStore {
    /// 常用默认路径：`%APPDATA%\rterm\sessions.json`（Windows）
    pub fn default_path() -> PathBuf {
        // 让调用方传更好；这里只给测试用
        PathBuf::from("sessions.json")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Session;

    fn session(name: &str, folder: Option<&str>) -> Session {
        Session {
            name: name.into(),
            host: "example.com".into(),
            username: "ops".into(),
            folder: folder.map(str::to_string),
            ..Session::default()
        }
    }

    fn temp_path(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "rterm-store-{tag}-{}-{}.json",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ))
    }

    #[test]
    fn normalizes_folder_paths() {
        assert_eq!(
            normalize_folder_path(" /生产/数据库/ ").unwrap(),
            "生产/数据库"
        );
        assert!(normalize_folder_path("  ").is_err());
        assert!(normalize_folder_path("a//b").is_err());
    }

    #[test]
    fn create_folder_keeps_empty_groups_and_parents() {
        let mut store = SessionStore::new();
        store.create_folder("生产/数据库").expect("创建分组");
        assert_eq!(
            store.explicit_folders(),
            &["生产".to_string(), "生产/数据库".to_string()]
        );
        assert!(store.create_folder("生产").is_err(), "重复创建应报错");
        assert!(store.folder_exists("生产/数据库"));
    }

    #[test]
    fn rename_folder_updates_prefix_and_sessions() {
        let mut store = SessionStore::new();
        store.create_folder("生产/数据库").expect("创建分组");
        store.upsert(session("db-01", Some("生产/数据库")));
        store.upsert(session("web-01", Some("生产")));

        store.rename_folder("生产", "生产环境").expect("重命名");

        assert!(store.list_folders().iter().any(|f| f == "生产环境/数据库"));
        assert_eq!(
            store.get(store.list()[0].id).unwrap().folder.as_deref(),
            Some("生产环境/数据库")
        );
        assert_eq!(
            store.get(store.list()[1].id).unwrap().folder.as_deref(),
            Some("生产环境")
        );
    }

    #[test]
    fn rename_folder_rejects_conflicts_and_self_nesting() {
        let mut store = SessionStore::new();
        store.create_folder("a").unwrap();
        store.create_folder("b").unwrap();
        assert!(store.rename_folder("a", "b").is_err(), "目标已存在");
        assert!(store.rename_folder("a", "a/sub").is_err(), "不能移入自身");
        assert!(store.rename_folder("missing", "c").is_err());
    }

    #[test]
    fn delete_empty_folder_rejects_non_empty() {
        let mut store = SessionStore::new();
        store.create_folder("a/b").unwrap();
        assert!(store.delete_empty_folder("a").is_err(), "存在子分组");

        store.delete_empty_folder("a/b").expect("空分组可删");
        assert!(!store.folder_exists("a/b"));
        assert!(store.folder_exists("a"), "显式父分组保留");

        store.upsert(session("db", Some("a")));
        assert!(store.delete_empty_folder("a").is_err(), "存在会话");
    }

    #[test]
    fn move_session_changes_folder() {
        let mut store = SessionStore::new();
        store.create_folder("db").unwrap();
        let s = session("db-01", None);
        let id = s.id;
        store.upsert(s);

        store.move_session(id, Some("db")).expect("移动");
        assert_eq!(store.get(id).unwrap().folder.as_deref(), Some("db"));
        store.move_session(id, None).expect("移回根目录");
        assert_eq!(store.get(id).unwrap().folder, None);
        assert!(store.move_session(id, Some("missing")).is_err());
    }

    #[test]
    fn loads_v1_session_file_without_folders() {
        let path = temp_path("v1");
        let raw = r#"{
            "version": 1,
            "saved_at": "2026-01-01T00:00:00Z",
            "sessions": [{
                "id": "6f9d5a2e-0e8a-4a36-9d46-3fde3f4f1f0a",
                "name": "legacy",
                "host": "legacy.example.com",
                "username": "ops"
            }]
        }"#;
        std::fs::write(&path, raw).expect("写 v1 文件");

        let store = SessionStore::load(&path, |s| Ok(s.clone())).expect("v1 可加载");
        assert_eq!(store.count(), 1);
        assert!(store.explicit_folders().is_empty());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn saves_v2_and_roundtrips_folders() {
        let path = temp_path("v2");
        let mut store = SessionStore::new();
        store.create_folder("测试/空分组").unwrap();
        store.upsert(session("web", Some("生产")));
        store.set_folders(vec!["测试/空分组".into()]);

        let ok = store
            .roundtrip(&path, |s| Ok(s.clone()), |s| Ok(s.clone()))
            .expect("往返");
        assert!(ok, "会话与显式分组应完整往返");
        assert_eq!(
            store.explicit_folders(),
            &["测试".to_string(), "测试/空分组".into()]
        );
        let _ = std::fs::remove_file(&path);
    }
}

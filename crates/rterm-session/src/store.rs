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
    dirty: bool,
}

impl SessionStore {
    pub fn new() -> Self {
        Self {
            sessions: Vec::new(),
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
        let decrypted: Result<Vec<_>> = wrapper.sessions.iter().map(&mut decrypt_fn).collect();
        let sessions = decrypted.context("解密会话失败 — 密码或密钥可能不匹配")?;

        Ok(Self {
            sessions,
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
        self.save(path, encrypt)?;
        let loaded = Self::load(path, decrypt)?;
        Ok(loaded.sessions == original)
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

const FILE_VERSION: u32 = 1;

/// 磁盘 JSON 包装格式。
#[derive(Serialize, Deserialize)]
struct SessionFile {
    version: u32,
    saved_at: chrono::DateTime<Utc>,
    sessions: Vec<Session>,
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

//! SftpService — ssh2::Sftp 的阻塞封装。
//!
//! `ssh2::Sftp` 内部用 `Arc<Mutex<SessionInner>>` 串行化所有操作，且实现了
//! `Send + Sync`，因此可以被导航线程与多个传输线程共享。

use anyhow::{anyhow, Context, Result};
use ssh2::{FileStat, Session, Sftp};
use std::path::Path;

/// 远端目录项（与本地目录项统一的展示模型）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileEntry {
    pub name: String,
    /// 远端绝对路径（使用 `/` 分隔）
    pub path: String,
    pub size: u64,
    pub mode: u32,
    pub mtime: i64,
    pub is_dir: bool,
    pub is_symlink: bool,
    pub symlink_target: Option<String>,
}

impl FileEntry {
    fn from_stat(dir: &str, name: String, stat: &FileStat, link_target: Option<String>) -> Self {
        Self {
            path: remote_join(dir, &name),
            size: stat.size.unwrap_or(0),
            mode: stat.perm.unwrap_or(0),
            mtime: stat.mtime.unwrap_or(0) as i64,
            is_dir: stat.is_dir(),
            is_symlink: stat.file_type().is_symlink(),
            symlink_target: link_target,
            name,
        }
    }
}

/// 排序字段。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SortKey {
    Name,
    Size,
    Mode,
    Modified,
}

/// 一次 SFTP 连接（独立的 SSH 连接，生命周期与 SftpView 绑定）。
pub struct SftpService {
    sftp: Sftp,
    /// 保留 Session，避免 Sftp 的引用计数之外被提前断开。
    _session: Session,
}

impl SftpService {
    /// 在已认证的 SSH Session 上建立 SFTP 子系统。
    pub fn new(session: Session) -> Result<Self> {
        let sftp = session.sftp().context("无法建立 SFTP 子系统")?;
        Ok(Self {
            sftp,
            _session: session,
        })
    }

    /// 远端 home 目录（realpath "."）。
    pub fn home_dir(&self) -> Result<String> {
        let path = self
            .sftp
            .realpath(Path::new("."))
            .context("无法解析远端 home 目录")?;
        Ok(path_to_string(&path))
    }

    /// 列目录：目录优先，再按名称排序。
    pub fn listdir(&self, dir: &str) -> Result<Vec<FileEntry>> {
        let raw = self
            .sftp
            .readdir(Path::new(dir))
            .with_context(|| format!("无法读取远端目录 {dir}"))?;
        let mut entries = Vec::with_capacity(raw.len());
        for (path, stat) in raw {
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            if name.is_empty() || name == "." || name == ".." {
                continue;
            }
            let link_target = if stat.file_type().is_symlink() {
                self.sftp
                    .readlink(Path::new(&remote_join(dir, &name)))
                    .ok()
                    .map(|p| path_to_string(&p))
            } else {
                None
            };
            entries.push(FileEntry::from_stat(dir, name, &stat, link_target));
        }
        sort_entries(&mut entries, SortKey::Name, true);
        Ok(entries)
    }

    /// 单文件 / 目录状态。
    pub fn stat(&self, path: &str) -> Result<FileEntry> {
        let stat = self
            .sftp
            .stat(Path::new(path))
            .with_context(|| format!("无法获取远端状态 {path}"))?;
        let name = file_name_of(path);
        let link_target = if stat.file_type().is_symlink() {
            self.sftp
                .readlink(Path::new(path))
                .ok()
                .map(|p| path_to_string(&p))
        } else {
            None
        };
        let dir = remote_parent(path);
        Ok(FileEntry::from_stat(&dir, name, &stat, link_target))
    }

    pub fn mkdir(&self, path: &str) -> Result<()> {
        self.sftp
            .mkdir(Path::new(path), 0o755)
            .with_context(|| format!("创建目录失败 {path}"))
    }

    pub fn remove_file(&self, path: &str) -> Result<()> {
        self.sftp
            .unlink(Path::new(path))
            .with_context(|| format!("删除文件失败 {path}"))
    }

    pub fn remove_dir(&self, path: &str) -> Result<()> {
        self.sftp
            .rmdir(Path::new(path))
            .with_context(|| format!("删除目录失败 {path}"))
    }

    pub fn rename(&self, from: &str, to: &str) -> Result<()> {
        self.sftp
            .rename(Path::new(from), Path::new(to), None)
            .with_context(|| format!("重命名失败 {from} → {to}"))
    }

    /// chmod（0o000–0o7777）。
    pub fn chmod(&self, path: &str, mode: u32) -> Result<()> {
        if mode > 0o7777 {
            return Err(anyhow!("非法权限值 {mode:o}"));
        }
        let stat = FileStat {
            size: None,
            uid: None,
            gid: None,
            perm: Some(mode),
            atime: None,
            mtime: None,
        };
        self.sftp
            .setstat(Path::new(path), stat)
            .with_context(|| format!("修改权限失败 {path}"))
    }

    /// 供 TransferQueue 使用的底层句柄。
    pub fn sftp(&self) -> &Sftp {
        &self.sftp
    }

    /// 远端指定目录下是否存在 `.` / `..` 之外的条目（用于删除目录前判断）。
    pub fn is_dir_empty(&self, path: &str) -> Result<bool> {
        Ok(self.listdir(path)?.is_empty())
    }
}

/// 拼接远端路径，始终使用 `/`。
pub fn remote_join(dir: &str, name: &str) -> String {
    if dir.is_empty() || dir == "." {
        return name.to_string();
    }
    if dir.ends_with('/') {
        format!("{dir}{name}")
    } else {
        format!("{dir}/{name}")
    }
}

/// 远端父目录；根目录保持 `/`。
pub fn remote_parent(path: &str) -> String {
    let trimmed = path.trim_end_matches('/');
    if trimmed.is_empty() {
        return "/".to_string();
    }
    match trimmed.rfind('/') {
        Some(0) => "/".to_string(),
        Some(index) => trimmed[..index].to_string(),
        None => ".".to_string(),
    }
}

fn file_name_of(path: &str) -> String {
    let trimmed = path.trim_end_matches('/');
    trimmed
        .rsplit('/')
        .next()
        .filter(|name| !name.is_empty())
        .unwrap_or("/")
        .to_string()
}

fn path_to_string(path: &Path) -> String {
    let text = path.to_string_lossy().replace('\\', "/");
    if text.is_empty() {
        "/".to_string()
    } else {
        text
    }
}

/// 目录优先，再按指定字段排序。
pub fn sort_entries(entries: &mut [FileEntry], key: SortKey, ascending: bool) {
    entries.sort_by(|a, b| {
        // 目录永远在文件前面（与原设计一致）
        if a.is_dir != b.is_dir {
            return if a.is_dir {
                std::cmp::Ordering::Less
            } else {
                std::cmp::Ordering::Greater
            };
        }
        let ordering = match key {
            SortKey::Name => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
            SortKey::Size => a.size.cmp(&b.size),
            SortKey::Mode => a.mode.cmp(&b.mode),
            SortKey::Modified => a.mtime.cmp(&b.mtime),
        };
        let ordering = if ascending {
            ordering
        } else {
            ordering.reverse()
        };
        if ordering == std::cmp::Ordering::Equal {
            a.name.to_lowercase().cmp(&b.name.to_lowercase())
        } else {
            ordering
        }
    });
}

/// `-rw-r--r--` 形式。
pub fn format_mode(entry: &FileEntry) -> String {
    let mut out = String::with_capacity(10);
    out.push(if entry.is_dir {
        'd'
    } else if entry.is_symlink {
        'l'
    } else {
        '-'
    });
    for shift in [6, 3, 0] {
        let bits = (entry.mode >> shift) & 0o7;
        out.push(if bits & 0o4 != 0 { 'r' } else { '-' });
        out.push(if bits & 0o2 != 0 { 'w' } else { '-' });
        out.push(if bits & 0o1 != 0 { 'x' } else { '-' });
    }
    out
}

/// 人类可读大小：`512 B` / `1.2 KB` / `3.4 MB`。
pub fn format_size(size: u64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = KB * 1024.0;
    const GB: f64 = MB * 1024.0;
    let value = size as f64;
    if value >= GB {
        format!("{:.1} GB", value / GB)
    } else if value >= MB {
        format!("{:.1} MB", value / MB)
    } else if value >= KB {
        format!("{:.1} KB", value / KB)
    } else {
        format!("{size} B")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(name: &str, is_dir: bool, size: u64, mode: u32) -> FileEntry {
        FileEntry {
            name: name.into(),
            path: format!("/tmp/{name}"),
            size,
            mode,
            mtime: 0,
            is_dir,
            is_symlink: false,
            symlink_target: None,
        }
    }

    #[test]
    fn remote_join_handles_roots_and_trailing_slash() {
        assert_eq!(remote_join("/", "etc"), "/etc");
        assert_eq!(remote_join("/home/ubuntu", "file"), "/home/ubuntu/file");
        assert_eq!(remote_join("/home/ubuntu/", "file"), "/home/ubuntu/file");
        assert_eq!(remote_join("", "file"), "file");
        assert_eq!(remote_join(".", "file"), "file");
    }

    #[test]
    fn remote_parent_walks_up_to_root() {
        assert_eq!(remote_parent("/home/ubuntu/file"), "/home/ubuntu");
        assert_eq!(remote_parent("/home"), "/");
        assert_eq!(remote_parent("/"), "/");
        assert_eq!(remote_parent("/home/"), "/");
        assert_eq!(remote_parent("file"), ".");
    }

    #[test]
    fn sort_keeps_directories_first_and_name_ascending() {
        let mut entries = vec![
            entry("b.txt", false, 20, 0o644),
            entry("zeta", true, 0, 0o755),
            entry("A.txt", false, 10, 0o644),
            entry("alpha", true, 0, 0o755),
        ];
        sort_entries(&mut entries, SortKey::Name, true);
        let names: Vec<_> = entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["alpha", "zeta", "A.txt", "b.txt"]);
    }

    #[test]
    fn sort_by_size_descending() {
        let mut entries = vec![
            entry("small", false, 1, 0o644),
            entry("large", false, 100, 0o644),
            entry("dir", true, 0, 0o755),
        ];
        sort_entries(&mut entries, SortKey::Size, false);
        let names: Vec<_> = entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["dir", "large", "small"]);
    }

    #[test]
    fn mode_is_rendered_like_ls() {
        let mut file = entry("a.sh", false, 0, 0o755);
        assert_eq!(format_mode(&file), "-rwxr-xr-x");
        file.mode = 0o644;
        assert_eq!(format_mode(&file), "-rw-r--r--");
        let dir = entry("etc", true, 0, 0o755);
        assert_eq!(format_mode(&dir), "drwxr-xr-x");
        let link = FileEntry {
            is_symlink: true,
            mode: 0o777,
            ..entry("link", false, 0, 0o777)
        };
        assert_eq!(format_mode(&link), "lrwxrwxrwx");
    }

    #[test]
    fn size_is_human_readable() {
        assert_eq!(format_size(512), "512 B");
        assert_eq!(format_size(1024), "1.0 KB");
        assert_eq!(format_size(1536), "1.5 KB");
        assert_eq!(format_size(1024 * 1024), "1.0 MB");
        assert_eq!(format_size(3 * 1024 * 1024 * 1024), "3.0 GB");
    }

    #[test]
    fn file_name_and_parent_roundtrip() {
        assert_eq!(file_name_of("/home/ubuntu/a.txt"), "a.txt");
        assert_eq!(file_name_of("/home/ubuntu/"), "ubuntu");
        assert_eq!(remote_join("/home/ubuntu", "a.txt"), "/home/ubuntu/a.txt");
    }
}

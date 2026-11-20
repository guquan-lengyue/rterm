//! single_instance — 跨平台单实例保护
//!
//! Windows：命名互斥体 `Local\rterm-single-instance`
//! POSIX（Linux / macOS）：`flock` 锁文件（`$XDG_CACHE_HOME/rterm/single-instance.lock`
//! 或 `$HOME/.cache/rterm/single-instance.lock`）
//!
//! guard 在 main 整个生命周期内持有，进程退出时 Drop → 锁释放。
//! flock 与命名互斥体在「进程崩溃后自动释放」这点上行为一致。

#[cfg(target_os = "windows")]
mod imp {
    use windows::core::w;
    use windows::Win32::Foundation::{
        CloseHandle, GetLastError, SetLastError, ERROR_ALREADY_EXISTS, ERROR_SUCCESS, HANDLE, HWND,
    };
    use windows::Win32::System::Threading::CreateMutexW;
    use windows::Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_ICONINFORMATION, MB_OK};

    /// 持有命名互斥体句柄；Drop 时关闭。
    pub struct SingleInstanceGuard {
        handle: HANDLE,
    }

    impl Drop for SingleInstanceGuard {
        fn drop(&mut self) {
            if !self.handle.is_invalid() {
                unsafe {
                    let _ = CloseHandle(self.handle);
                }
            }
        }
    }

    /// 尝试获取单实例锁。
    ///
    /// - 已有实例 → 弹窗提示并返回 None
    /// - 创建互斥体失败 → 记日志并继续启动（返回一个空 guard）
    pub fn acquire() -> Option<SingleInstanceGuard> {
        unsafe {
            // CreateMutexW 失败/成功都可能沿用旧的 last error，先清零。
            SetLastError(ERROR_SUCCESS);
            match CreateMutexW(None, false, w!("Local\\rterm-single-instance")) {
                Ok(handle) => {
                    if GetLastError() == ERROR_ALREADY_EXISTS {
                        let _ = CloseHandle(handle);
                        MessageBoxW(
                            HWND::default(),
                            w!("rterm 已在运行"),
                            w!("rterm"),
                            MB_OK | MB_ICONINFORMATION,
                        );
                        None
                    } else {
                        Some(SingleInstanceGuard { handle })
                    }
                }
                Err(e) => {
                    log::error!("创建单实例互斥体失败，继续启动: {e}");
                    Some(SingleInstanceGuard {
                        handle: HANDLE::default(),
                    })
                }
            }
        }
    }
}

#[cfg(not(target_os = "windows"))]
mod imp {
    use std::fs::{File, OpenOptions};
    use std::io::ErrorKind;
    use std::path::{Path, PathBuf};

    use nix::fcntl::{flock, FlockArg};

    /// `flock` 持有的锁文件；进程退出（Drop）时关闭 fd → 内核自动释放锁。
    pub struct SingleInstanceGuard {
        _file: File,
        path: PathBuf,
    }

    impl Drop for SingleInstanceGuard {
        fn drop(&mut self) {
            // 主动 unlock + 删除锁文件，避免留下脏文件；文件被另一个进程接管时删除失败。
            let _ = flock(self._file.as_raw_fd(), FlockArg::Unlock);
            let _ = std::fs::remove_file(&self.path);
        }
    }

    /// 锁文件路径：`$XDG_CACHE_HOME/rterm/single-instance.lock`
    /// 回退 `$HOME/.cache/rterm/single-instance.lock`。
    fn lock_path() -> PathBuf {
        let dir = std::env::var_os("XDG_CACHE_HOME")
            .map(PathBuf::from)
            .or_else(|| {
                std::env::var_os("HOME").map(|h| {
                    let mut p = PathBuf::from(h);
                    p.push(".cache");
                    p
                })
            })
            .unwrap_or_else(|| PathBuf::from("."));
        dir.join("rterm").join("single-instance.lock")
    }

    /// 尝试获取单实例锁。
    ///
    /// - 已有实例持有锁 → 返回 None
    /// - 锁文件无法创建 → 记日志并继续启动
    pub fn acquire() -> Option<SingleInstanceGuard> {
        let path = lock_path();
        if let Some(parent) = path.parent() {
            if let Err(e) = std::fs::create_dir_all(parent) {
                log::error!(
                    "[single_instance] 创建锁目录 {} 失败，继续启动: {e}",
                    parent.display()
                );
            }
        }

        let file = match OpenOptions::new().create(true).write(true).open(&path) {
            Ok(f) => f,
            Err(e) => {
                log::error!(
                    "[single_instance] 打开锁文件 {} 失败，继续启动: {e}",
                    path.display()
                );
                return None;
            }
        };

        // 写 pid 方便排查（调试时 `cat` 锁文件就能知道谁持有的）
        let _ = std::fs::write(&path, format!("{}\n", std::process::id()));

        match flock(file.as_raw_fd(), FlockArg::LockExclusive | FlockArg::LockNonBlocking) {
            Ok(()) => Some(SingleInstanceGuard { _file: file, path }),
            Err(nix::Error::EWOULDBLOCK | nix::Error::EAGAIN) => {
                log::info!("[single_instance] 已有 rterm 实例在运行（PID 锁文件 {}），本次启动退出", path.display());
                None
            }
            Err(e) => {
                log::error!(
                    "[single_instance] flock 失败（PID={}，路径={}），继续启动: {e}",
                    std::process::id(),
                    path.display()
                );
                Some(SingleInstanceGuard { _file: file, path })
            }
        }
    }
}

pub use imp::{acquire, SingleInstanceGuard};

// 本模块全是平台 FFI（Windows 命名互斥体 / POSIX flock），
// 单元测试里无法可靠模拟「第二个进程已持有锁」的场景，因此省略独立测试。
// 手动验证步骤：
//   1. 启动一个 rterm 实例 → 第二个 rterm 应该直接退出（Win 弹窗提示，Linux 静默返回）
//   2. 杀掉第一个 → 再启动 → 正常进入
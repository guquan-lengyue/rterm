//! single_instance — Windows 单实例保护
//!
//! 用命名互斥体 `Local\rterm-single-instance` 保证同一登录会话只跑一个 rterm，
//! 避免两个进程同时读写 sessions.json 相互覆盖。
//! guard 在 main 整个生命周期内持有，进程退出（Drop）时释放。

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
    /// 非 Windows 平台不限制单实例。
    pub struct SingleInstanceGuard;

    pub fn acquire() -> Option<SingleInstanceGuard> {
        Some(SingleInstanceGuard)
    }
}

pub use imp::{acquire, SingleInstanceGuard};

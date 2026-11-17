//! SSH 错误分类与中文文案
//!
//! `ssh2` 的 `Error::code()` 是 `LIBSSH2_ERROR_*` 原始码（负数），
//! 直接透传给用户可读性差。这里把错误链里的 `ssh2::Error` /
//! `std::io::Error` 归一成几类，供 UI 展示与重连策略判断。

use std::io::ErrorKind;

use ssh2::ErrorCode;

/// 用户可理解的错误类别。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SshErrorKind {
    /// 用户名 / 密码 / 私钥 / Agent 认证失败。
    AuthFailed,
    /// TCP 或 SSH 层超时。
    Timeout,
    /// known_hosts 中记录的主机密钥与当前不一致。
    HostKeyChanged,
    /// 用户拒绝接受未知 / 变更的主机密钥。
    HostKeyRejected,
    /// 无法建立连接、连接被重置等网络问题。
    NetworkError,
    /// 其它未归类错误。
    Unknown,
}

impl SshErrorKind {
    /// 中文文案（不含原始错误细节）。
    pub fn message(self) -> &'static str {
        match self {
            Self::AuthFailed => "认证失败：请检查用户名、密码、私钥或 SSH Agent 配置",
            Self::Timeout => "连接超时：请检查主机地址、端口与网络连通性",
            Self::HostKeyChanged => {
                "主机密钥已变更：可能是服务器重装或存在中间人风险，确认指纹后再连接"
            }
            Self::HostKeyRejected => "已拒绝该主机密钥，连接已中止",
            Self::NetworkError => "网络错误：无法连接到服务器（主机不可达或端口未开放）",
            Self::Unknown => "连接失败",
        }
    }
}

// LIBSSH2_ERROR_* 原始码（libssh2.h）。
const ERROR_TIMEOUT: i32 = -9;
const ERROR_SOCKET_SEND: i32 = -7;
const ERROR_SOCKET_DISCONNECT: i32 = -13;
const ERROR_PASSWORD_EXPIRED: i32 = -15;
const ERROR_AUTHENTICATION_FAILED: i32 = -18;
const ERROR_PUBLICKEY_UNRECOGNIZED: i32 = -19;
const ERROR_SOCKET_TIMEOUT: i32 = -30;
const ERROR_SOCKET_RECV: i32 = -43;
const ERROR_BAD_SOCKET: i32 = -45;
const ERROR_KEYFILE_AUTH_FAILED: i32 = -48;
const ERROR_SOCKET_NONE: i32 = -1;

/// 从 anyhow 错误链分类。
pub fn classify(error: &anyhow::Error) -> SshErrorKind {
    for cause in error.chain() {
        if let Some(ssh_error) = cause.downcast_ref::<ssh2::Error>() {
            if let ErrorCode::Session(code) = ssh_error.code() {
                let kind = classify_libssh2_code(code);
                if kind != SshErrorKind::Unknown {
                    return kind;
                }
            }
        }
        if let Some(io_error) = cause.downcast_ref::<std::io::Error>() {
            let kind = classify_io_kind(io_error.kind());
            if kind != SshErrorKind::Unknown {
                return kind;
            }
        }
        let kind = classify_message(&cause.to_string());
        if kind != SshErrorKind::Unknown {
            return kind;
        }
    }
    SshErrorKind::Unknown
}

/// 分类纯文本错误（用于已转成 `String` 的场景，如重连策略判断）。
pub fn classify_message(message: &str) -> SshErrorKind {
    if message.contains("主机密钥已变更") {
        SshErrorKind::HostKeyChanged
    } else if message.contains("拒绝接受主机密钥")
        || message.contains("拒绝该主机密钥")
        || message.contains("Strict 策略下拒绝连接")
    {
        SshErrorKind::HostKeyRejected
    } else if message.contains("认证失败")
        || message.contains("认证未通过")
        || message.contains("密码为空")
        || message.contains("私钥认证失败")
        || message.contains("SSH Agent 认证失败")
    {
        SshErrorKind::AuthFailed
    } else if message.contains("超时") || message.contains("timed out") {
        SshErrorKind::Timeout
    } else {
        SshErrorKind::Unknown
    }
}

/// 生成用户可读的错误描述（含类别文案；未归类时附原始错误）。
pub fn describe(error: &anyhow::Error) -> String {
    let kind = classify(error);
    if kind == SshErrorKind::Unknown {
        format!("{}：{error:#}", kind.message())
    } else {
        kind.message().to_string()
    }
}

fn classify_libssh2_code(code: i32) -> SshErrorKind {
    match code {
        ERROR_AUTHENTICATION_FAILED
        | ERROR_PUBLICKEY_UNRECOGNIZED
        | ERROR_PASSWORD_EXPIRED
        | ERROR_KEYFILE_AUTH_FAILED => SshErrorKind::AuthFailed,
        ERROR_TIMEOUT | ERROR_SOCKET_TIMEOUT => SshErrorKind::Timeout,
        ERROR_SOCKET_NONE
        | ERROR_SOCKET_DISCONNECT
        | ERROR_SOCKET_RECV
        | ERROR_SOCKET_SEND
        | ERROR_BAD_SOCKET => SshErrorKind::NetworkError,
        _ => SshErrorKind::Unknown,
    }
}

fn classify_io_kind(kind: ErrorKind) -> SshErrorKind {
    match kind {
        ErrorKind::TimedOut | ErrorKind::WouldBlock => SshErrorKind::Timeout,
        ErrorKind::ConnectionRefused
        | ErrorKind::ConnectionReset
        | ErrorKind::ConnectionAborted
        | ErrorKind::NotConnected
        | ErrorKind::BrokenPipe
        | ErrorKind::HostUnreachable
        | ErrorKind::NetworkUnreachable
        | ErrorKind::NetworkDown => SshErrorKind::NetworkError,
        _ => SshErrorKind::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn io_error(kind: ErrorKind) -> std::io::Error {
        std::io::Error::from(kind)
    }

    #[test]
    fn classifies_timeout_from_io_chain() {
        let error = anyhow::Error::new(io_error(ErrorKind::TimedOut)).context("TCP 连接失败");
        assert_eq!(classify(&error), SshErrorKind::Timeout);
        assert!(describe(&error).contains("连接超时"));
    }

    #[test]
    fn classifies_network_errors() {
        let error = anyhow::Error::new(io_error(ErrorKind::ConnectionRefused));
        assert_eq!(classify(&error), SshErrorKind::NetworkError);
    }

    #[test]
    fn classifies_host_key_messages() {
        let changed = anyhow::anyhow!("主机密钥已变更（h:22，ssh-ed25519 AAA），已拒绝连接");
        assert_eq!(classify(&changed), SshErrorKind::HostKeyChanged);

        let rejected = anyhow::anyhow!("用户拒绝接受主机密钥");
        assert_eq!(classify(&rejected), SshErrorKind::HostKeyRejected);
    }

    #[test]
    fn classifies_auth_messages() {
        let error = anyhow::anyhow!("密码认证失败").context("连接 1.2.3.4:22");
        assert_eq!(classify(&error), SshErrorKind::AuthFailed);
        assert!(describe(&error).contains("认证失败"));
    }

    #[test]
    fn unknown_keeps_original_detail() {
        let error = anyhow::anyhow!("something odd happened");
        assert_eq!(classify(&error), SshErrorKind::Unknown);
        assert!(describe(&error).contains("something odd happened"));
    }

    #[test]
    fn classify_message_is_usable_for_plain_strings() {
        assert_eq!(
            classify_message("SSH 握手失败: 主机密钥已变更"),
            SshErrorKind::HostKeyChanged
        );
        assert_eq!(
            classify_message("TCP 连接失败: 操作超时"),
            SshErrorKind::Timeout
        );
        assert_eq!(classify_message("其它错误"), SshErrorKind::Unknown);
    }
}

//! SSH 认证 — Password / PrivateKey / Agent
//!
//! 传入的 `Session` 必须是**已解密**的（EncryptedString.version == 0）。

use anyhow::{bail, Context, Result};
use rterm_session::{AuthMethod, EncryptedString, Session};

/// 依次尝试会话配置里的认证方式。
pub fn authenticate(session: &ssh2::Session, config: &Session) -> Result<()> {
    let username = config.username.trim();
    if username.is_empty() {
        bail!("用户名为空");
    }

    match &config.auth {
        AuthMethod::Password { password } => {
            let password = plaintext(password).context("读取密码失败")?;
            if password.is_empty() {
                bail!("密码为空");
            }
            session
                .userauth_password(username, &password)
                .context("密码认证失败")?;
        }
        AuthMethod::PrivateKey { path, passphrase } => {
            let passphrase = match passphrase {
                Some(pp) => Some(plaintext(pp).context("读取私钥口令失败")?),
                None => None,
            };
            session
                .userauth_pubkey_file(username, None, path, passphrase.as_deref())
                .with_context(|| format!("私钥认证失败: {}", path.display()))?;
        }
        AuthMethod::Agent { .. } => {
            session
                .userauth_agent(username)
                .context("SSH Agent 认证失败（请确认 agent 已运行并加载密钥）")?;
        }
    }

    if !session.authenticated() {
        bail!("SSH 认证未通过");
    }
    Ok(())
}

fn plaintext(value: &EncryptedString) -> Result<String> {
    if value.version != 0 {
        bail!("字段仍是密文（version={}），请先解密会话", value.version);
    }
    String::from_utf8(value.data.clone()).context("字段不是 UTF-8 明文")
}

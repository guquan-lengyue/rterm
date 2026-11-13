//! crypto_bridge — Session ↔ rterm-crypto 的加密/解密桥
//!
//! Session 里的 EncryptedString 字段存的是**密文**（save 时加密、load 时解密）。
//! 这层把 dpapi_encrypt/dpapi_decrypt 或 aesgcm_encrypt/aesgcm_decrypt 映射到
//! Session 的 AuthMethod::Password / PrivateKey.passphrase 里。

use anyhow::{Context, Result};

use crate::model::{AuthMethod, Session};

// ============================================================================
// DPAPI 桥 — 本地存储（save/load 时用）
// ============================================================================

/// 深拷贝 Session，对 EncryptedString::data 做 DPAPI 加密。
pub fn encrypt_session_dpapi(s: &Session) -> Result<Session> {
    let mut out = s.clone();
    encrypt_auth_dpapi(&mut out.auth)?;
    if let Some(jump) = out.proxy_jump.as_deref_mut() {
        encrypt_session_dpapi_inplace(jump)?;
    }
    Ok(out)
}

fn encrypt_session_dpapi_inplace(s: &mut Session) -> Result<()> {
    encrypt_auth_dpapi(&mut s.auth)?;
    if let Some(jump) = s.proxy_jump.as_deref_mut() {
        encrypt_session_dpapi_inplace(jump)?;
    }
    Ok(())
}

fn encrypt_auth_dpapi(auth: &mut AuthMethod) -> Result<()> {
    match auth {
        AuthMethod::Password { password } => {
            if !password.is_empty() && password.version == 0 {
                let cipher = rterm_crypto::dpapi_encrypt(&password.data)
                    .context("DPAPI 加密密码失败")?;
                password.data = cipher;
                password.version = 1;
            }
        }
        AuthMethod::PrivateKey { passphrase, .. } => {
            if let Some(pp) = passphrase.as_mut() {
                if !pp.is_empty() && pp.version == 0 {
                    let cipher = rterm_crypto::dpapi_encrypt(&pp.data)
                        .context("DPAPI 加密密钥口令失败")?;
                    pp.data = cipher;
                    pp.version = 1;
                }
            }
        }
        AuthMethod::Agent { .. } => {}
    }
    Ok(())
}

pub fn decrypt_session_dpapi(s: &Session) -> Result<Session> {
    let mut out = s.clone();
    decrypt_session_dpapi_inplace(&mut out)?;
    Ok(out)
}

fn decrypt_session_dpapi_inplace(s: &mut Session) -> Result<()> {
    decrypt_auth_dpapi(&mut s.auth)?;
    if let Some(jump) = s.proxy_jump.as_deref_mut() {
        decrypt_session_dpapi_inplace(jump)?;
    }
    Ok(())
}

fn decrypt_auth_dpapi(auth: &mut AuthMethod) -> Result<()> {
    match auth {
        AuthMethod::Password { password } => {
            if !password.is_empty() && password.version == 1 {
                let plain = rterm_crypto::dpapi_decrypt(&password.data)
                    .context("DPAPI 解密密码失败 — 可能文件来自不同 Windows 用户或机器")?;
                password.data = plain;
                password.version = 0;
            }
        }
        AuthMethod::PrivateKey { passphrase, .. } => {
            if let Some(pp) = passphrase.as_mut() {
                if !pp.is_empty() && pp.version == 1 {
                    let plain = rterm_crypto::dpapi_decrypt(&pp.data)
                        .context("DPAPI 解密密钥口令失败")?;
                    pp.data = plain;
                    pp.version = 0;
                }
            }
        }
        AuthMethod::Agent { .. } => {}
    }
    Ok(())
}

// ============================================================================
// AES-GCM 桥 — 导出/导入文件时用
// ============================================================================

pub fn encrypt_session_aesgcm(s: &Session, passphrase: &str) -> Result<Session> {
    let mut out = s.clone();
    encrypt_session_aesgcm_inplace(&mut out, passphrase)?;
    Ok(out)
}

fn encrypt_session_aesgcm_inplace(s: &mut Session, passphrase: &str) -> Result<()> {
    encrypt_auth_aesgcm(&mut s.auth, passphrase)?;
    if let Some(jump) = s.proxy_jump.as_deref_mut() {
        encrypt_session_aesgcm_inplace(jump, passphrase)?;
    }
    Ok(())
}

fn encrypt_auth_aesgcm(auth: &mut AuthMethod, passphrase: &str) -> Result<()> {
    match auth {
        AuthMethod::Password { password } => {
            if !password.is_empty() {
                // 导出前先 DPAPI 解密（假设当前是 DPAPI 密文），然后 AES-GCM 加密
                let plain = if password.version == 1 {
                    rterm_crypto::dpapi_decrypt(&password.data)
                        .context("导出时先 DPAPI 解密失败")?
                } else {
                    password.data.clone()
                };
                let cipher = rterm_crypto::aesgcm_encrypt(&plain, passphrase)
                    .context("AES-GCM 加密导出密码失败")?;
                password.data = cipher;
                password.version = 2;  // version=2 表示 AES-GCM
            }
        }
        AuthMethod::PrivateKey { passphrase: Some(pp), .. } if !pp.is_empty() => {
            let plain = if pp.version == 1 {
                rterm_crypto::dpapi_decrypt(&pp.data).context("导出时 DPAPI 解密钥口令失败")?
            } else {
                pp.data.clone()
            };
            let cipher = rterm_crypto::aesgcm_encrypt(&plain, passphrase)?;
            pp.data = cipher;
            pp.version = 2;
        }
        _ => {}
    }
    Ok(())
}

pub fn decrypt_session_aesgcm(s: &Session, passphrase: &str) -> Result<Session> {
    let mut out = s.clone();
    decrypt_session_aesgcm_inplace(&mut out, passphrase)?;
    Ok(out)
}

fn decrypt_session_aesgcm_inplace(s: &mut Session, passphrase: &str) -> Result<()> {
    decrypt_auth_aesgcm(&mut s.auth, passphrase)?;
    if let Some(jump) = s.proxy_jump.as_deref_mut() {
        decrypt_session_aesgcm_inplace(jump, passphrase)?;
    }
    Ok(())
}

fn decrypt_auth_aesgcm(auth: &mut AuthMethod, passphrase: &str) -> Result<()> {
    match auth {
        AuthMethod::Password { password } => {
            if !password.is_empty() && password.version == 2 {
                let plain = rterm_crypto::aesgcm_decrypt(&password.data, passphrase)
                    .context("AES-GCM 解密导入密码失败 — 口令错误或数据被篡改")?;
                password.data = plain;
                password.version = 0;
            }
        }
        AuthMethod::PrivateKey { passphrase: Some(pp), .. }
            if !pp.is_empty() && pp.version == 2 =>
        {
            let plain = rterm_crypto::aesgcm_decrypt(&pp.data, passphrase)?;
            pp.data = plain;
            pp.version = 0;
        }
        _ => {}
    }
    Ok(())
}

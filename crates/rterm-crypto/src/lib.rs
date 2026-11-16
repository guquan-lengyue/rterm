//! rterm-crypto — 敏感数据加解密
//!
//! 两种加密方式：
//! - **DPAPI**（本地存储）：绑定当前 Windows 登录用户 + 当前机器，用户无感
//! - **AES-256-GCM**（导出文件）：PBKDF2-HMAC-SHA256 派生密钥 + 用户输入口令
//!
//! Windows 外平台 DPAPI 返回 Err（调用方应 fallback 到 AES-GCM）。

use anyhow::{Context, Result};

// ============================================================================
// DPAPI (Windows only)
// ============================================================================

/// Windows DPAPI protect — 绑定当前登录用户 + 当前机器。
#[cfg(target_os = "windows")]
pub fn dpapi_encrypt(plaintext: &[u8]) -> Result<Vec<u8>> {
    use windows::Win32::Foundation::{LocalFree, HLOCAL};
    use windows::Win32::Security::Cryptography::{CryptProtectData, CRYPT_INTEGER_BLOB};

    let input = CRYPT_INTEGER_BLOB {
        cbData: plaintext.len() as u32,
        pbData: plaintext.as_ptr() as *mut u8,
    };
    let mut output = CRYPT_INTEGER_BLOB::default();

    unsafe {
        CryptProtectData(
            &input,
            None,  // description
            None,  // entropy
            None,  // reserved
            None,  // prompt
            0,     // 用户级作用域（不带 CRYPTPROTECT_LOCAL_MACHINE）
            &mut output,
        )
        .context("CryptProtectData failed")?;
    }

    let result = unsafe {
        std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec()
    };
    // DPAPI 用 LocalAlloc 分配输出缓冲区，复制后立即用 LocalFree 释放。
    unsafe {
        if !output.pbData.is_null() {
            let _ = LocalFree(HLOCAL(output.pbData as *mut core::ffi::c_void));
        }
    }
    Ok(result)
}

/// Windows DPAPI unprotect。
#[cfg(target_os = "windows")]
pub fn dpapi_decrypt(ciphertext: &[u8]) -> Result<Vec<u8>> {
    use windows::Win32::Foundation::{LocalFree, HLOCAL};
    use windows::Win32::Security::Cryptography::{
        CryptUnprotectData, CRYPT_INTEGER_BLOB,
    };

    let input = CRYPT_INTEGER_BLOB {
        cbData: ciphertext.len() as u32,
        pbData: ciphertext.as_ptr() as *mut u8,
    };
    let mut output = CRYPT_INTEGER_BLOB::default();

    unsafe {
        CryptUnprotectData(
            &input,
            None,  // description out
            None,  // entropy
            None,  // reserved
            None,  // prompt
            0,
            &mut output,
        )
        .context("CryptUnprotectData failed — ciphertext invalid or from different user/machine")?;
    }

    let result = unsafe {
        std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec()
    };
    // 输出缓冲区由 LocalAlloc 分配，复制后用 LocalFree 释放，避免内存泄漏。
    unsafe {
        if !output.pbData.is_null() {
            let _ = LocalFree(HLOCAL(output.pbData as *mut core::ffi::c_void));
        }
    }
    Ok(result)
}

// Non-Windows stubs
#[cfg(not(target_os = "windows"))]
pub fn dpapi_encrypt(_plaintext: &[u8]) -> Result<Vec<u8>> {
    anyhow::bail!("DPAPI is Windows-only; use aesgcm_encrypt instead")
}
#[cfg(not(target_os = "windows"))]
pub fn dpapi_decrypt(_ciphertext: &[u8]) -> Result<Vec<u8>> {
    anyhow::bail!("DPAPI is Windows-only; ciphertext cannot be decrypted on this platform")
}

// ============================================================================
// AES-256-GCM (cross-platform, for exported files)
// ============================================================================

use aes_gcm::{
    aead::{Aead, KeyInit},
    Aes256Gcm, Nonce,
};
use rand::RngCore;

const SALT_LEN: usize = 16;
const NONCE_LEN: usize = 12;
const KDF_ITER: u32 = 600_000;

/// PBKDF2-HMAC-SHA256 派生 32 字节 AES-256 key。
pub fn derive_key(passphrase: &str, salt: &[u8]) -> Result<[u8; 32]> {
    use hmac::{Hmac, Mac};
    use sha2::Sha256;

    type HmacSha256 = Hmac<Sha256>;

    let mut key = [0u8; 32];
    let mut u = [0u8; 32];
    let mut t = [0u8; 32];

    let blocks_needed = (key.len() as u32 + 31) / 32;

    for block_idx in 1..=blocks_needed {
        // U_1 = HMAC-SHA256(password, salt || INT_32_BE(block_idx))
        let mut mac = <HmacSha256 as Mac>::new_from_slice(passphrase.as_bytes())
            .context("HMAC-SHA256 key derive failed")?;
        mac.update(salt);
        mac.update(&block_idx.to_be_bytes());
        u = mac.finalize().into_bytes().into();
        t = u;

        for _ in 1..KDF_ITER {
            let mut mac = <HmacSha256 as Mac>::new_from_slice(passphrase.as_bytes())?;
            mac.update(&u);
            u = mac.finalize().into_bytes().into();
            for (ti, ui) in t.iter_mut().zip(u.iter()) {
                *ti ^= ui;
            }
        }

        let start = ((block_idx - 1) as usize) * 32;
        let end = key.len().min(start + 32);
        key[start..end].copy_from_slice(&t[..end - start]);
    }

    Ok(key)
}

/// AES-GCM 加密。返回格式：`[salt(16) || nonce(12) || ciphertext+tag]`。
pub fn aesgcm_encrypt(plaintext: &[u8], passphrase: &str) -> Result<Vec<u8>> {
    let mut salt = [0u8; SALT_LEN];
    let mut nonce = [0u8; NONCE_LEN];
    rand::thread_rng().fill_bytes(&mut salt);
    rand::thread_rng().fill_bytes(&mut nonce);

    let key = derive_key(passphrase, &salt)?;
    let cipher = Aes256Gcm::new_from_slice(&key)
        .map_err(|e| anyhow::anyhow!("AES key error: {e}"))?;
    let ciphertext = cipher
        .encrypt(Nonce::from_slice(&nonce), plaintext)
        .map_err(|e| anyhow::anyhow!("AES-GCM encrypt failed: {e}"))?;

    let mut packed = Vec::with_capacity(SALT_LEN + NONCE_LEN + ciphertext.len());
    packed.extend_from_slice(&salt);
    packed.extend_from_slice(&nonce);
    packed.extend_from_slice(&ciphertext);
    Ok(packed)
}

/// AES-GCM 解密。输入格式：`[salt(16) || nonce(12) || ciphertext+tag]`。
pub fn aesgcm_decrypt(packed: &[u8], passphrase: &str) -> Result<Vec<u8>> {
    if packed.len() < SALT_LEN + NONCE_LEN + 16 {
        anyhow::bail!(
            "ciphertext too short (salt+nonce+tag minimum = {})",
            SALT_LEN + NONCE_LEN + 16
        );
    }
    let (salt, rest) = packed.split_at(SALT_LEN);
    let (nonce, ciphertext) = rest.split_at(NONCE_LEN);

    let key = derive_key(passphrase, salt)?;
    let cipher = Aes256Gcm::new_from_slice(&key)
        .map_err(|e| anyhow::anyhow!("AES key error: {e}"))?;
    cipher
        .decrypt(Nonce::from_slice(nonce), ciphertext)
        .map_err(|e| anyhow::anyhow!("AES-GCM decrypt failed — wrong passphrase or tampered data: {e}"))
}

use rterm_crypto::{aesgcm_decrypt, aesgcm_encrypt, dpapi_decrypt, dpapi_encrypt};

// ---------- DPAPI tests ----------

#[test]
fn dpapi_roundtrip() {
    let plain = b"hello rterm";
    let enc = dpapi_encrypt(plain).expect("encrypt should succeed");
    assert_ne!(
        enc,
        plain.as_slice(),
        "ciphertext must differ from plaintext"
    );
    assert!(!enc.is_empty());
    let dec = dpapi_decrypt(&enc).expect("decrypt should succeed");
    assert_eq!(dec, plain);
}

#[test]
fn dpapi_empty_input() {
    let enc = dpapi_encrypt(b"").expect("encrypt empty should succeed");
    let dec = dpapi_decrypt(&enc).expect("decrypt empty should succeed");
    assert!(dec.is_empty());
}

#[test]
fn dpapi_large_input() {
    let big = vec![0x42u8; 1_000_000];
    let enc = dpapi_encrypt(&big).expect("encrypt 1MB");
    let dec = dpapi_decrypt(&enc).expect("decrypt 1MB");
    assert_eq!(dec.len(), big.len());
    assert_eq!(dec, big);
}

// ---------- AES-GCM tests ----------

const TEST_PASS: &str = "rterm-super-secret-2026";

#[test]
fn aesgcm_roundtrip() {
    let plain = b"exported session data";
    let enc = aesgcm_encrypt(plain, TEST_PASS).expect("encrypt");
    let dec = aesgcm_decrypt(&enc, TEST_PASS).expect("decrypt");
    assert_eq!(dec, plain);
}

#[test]
fn aesgcm_bad_passphrase() {
    let enc = aesgcm_encrypt(b"secret", TEST_PASS).expect("encrypt");
    let err = aesgcm_decrypt(&enc, "wrong-pass").unwrap_err();
    assert!(err.to_string().contains("wrong passphrase") || err.to_string().contains("tampered"));
}

#[test]
fn aesgcm_different_plaintext_produces_different_ciphertext() {
    let a = aesgcm_encrypt(b"aaa", TEST_PASS).unwrap();
    let b = aesgcm_encrypt(b"aaa", TEST_PASS).unwrap();
    // salt and nonce are random each time — ciphertexts must differ even for same input
    assert_ne!(a, b);
}

#[test]
fn aesgcm_empty_input() {
    let enc = aesgcm_encrypt(b"", TEST_PASS).unwrap();
    let dec = aesgcm_decrypt(&enc, TEST_PASS).unwrap();
    assert!(dec.is_empty());
}

#[test]
fn aesgcm_large_input_1mb() {
    let big = vec![0x7Au8; 1_000_000];
    let enc = aesgcm_encrypt(&big, TEST_PASS).unwrap();
    let dec = aesgcm_decrypt(&enc, TEST_PASS).unwrap();
    assert_eq!(dec, big);
}

#[test]
fn aesgcm_tampered_ciphertext_fails() {
    let mut enc = aesgcm_encrypt(b"data", TEST_PASS).unwrap();
    // flip last byte of ciphertext+tag (triggers GCM auth failure)
    if let Some(b) = enc.last_mut() {
        *b ^= 0xff;
    }
    let err = aesgcm_decrypt(&enc, TEST_PASS).unwrap_err();
    assert!(err.to_string().contains("wrong passphrase") || err.to_string().contains("tampered"));
}

#[test]
fn aesgcm_too_short_input() {
    let result = aesgcm_decrypt(&[0u8; 10], TEST_PASS);
    assert!(result.is_err());
}

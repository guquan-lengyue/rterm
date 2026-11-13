//! store_tests — SessionStore CRUD + save/load 往返 + 加密字段

use rterm_session::{Session, SessionStore, AuthMethod, EncryptedString};
use std::path::PathBuf;

fn tmp_path(name: &str) -> PathBuf {
    let mut p = std::env::temp_dir();
    let unique = format!("rterm_s2_{}_{}", name, std::process::id());
    p.push(unique);
    p
}

// ---------- 2.7.1 ----------

#[test]
fn store_new_is_empty() {
    let store = SessionStore::new();
    assert_eq!(store.count(), 0);
    assert!(store.list().is_empty());
}

#[test]
fn upsert_creates_new() {
    let mut store = SessionStore::new();
    let s = Session::new("web", "192.168.1.10", "root");
    let id = s.id;
    store.upsert(s);

    assert_eq!(store.count(), 1);
    let found = store.get(id).expect("should find");
    assert_eq!(found.name, "web");
    assert!(store.is_dirty());
}

#[test]
fn upsert_same_id_updates() {
    let mut store = SessionStore::new();
    let mut s = Session::new("web", "192.168.1.10", "root");
    store.upsert(s.clone());

    s.name = "web-renamed".into();
    s.host = "192.168.1.20".into();
    store.upsert(s);

    assert_eq!(store.count(), 1);
    assert_eq!(store.list()[0].name, "web-renamed");
    assert_eq!(store.list()[0].host, "192.168.1.20");
}

#[test]
fn delete_removes_by_id() {
    let mut store = SessionStore::new();
    let s1 = Session::new("a", "1.1.1.1", "x");
    let s2 = Session::new("b", "2.2.2.2", "x");
    store.upsert(s1.clone());
    store.upsert(s2.clone());
    assert_eq!(store.count(), 2);

    let ok = store.delete(s1.id);
    assert!(ok);
    assert_eq!(store.count(), 1);
    assert_eq!(store.list()[0].name, "b");

    let miss = store.delete(s1.id);
    assert!(!miss);
}

#[test]
fn file_not_found_loads_empty() {
    let path = tmp_path("nonexistent_load");
    let store = SessionStore::load(&path, |s| Ok(s.clone())).expect("ok");
    assert_eq!(store.count(), 0);
}

#[test]
fn save_load_roundtrip_plaintext() {
    let path = tmp_path("save_load_plain");
    let mut store = SessionStore::new();
    let s = Session::new("roundtrip", "10.0.0.5", "admin");
    store.upsert(s.clone());

    // 加密：把 AuthMethod::Password 里 EncryptedString 转 version=1 (DPAPI 密文)
    // 这里我们用 identity 函数，模拟 save/load 时未加密的场景（测试 plaintext 往返）
    store.save(&path, |s| Ok(s.clone())).expect("save");
    assert!(!store.is_dirty());

    let loaded = SessionStore::load(&path, |s| Ok(s.clone())).expect("load");
    assert_eq!(loaded.count(), 1);
    let got = loaded.get(s.id).unwrap();
    assert_eq!(got.name, "roundtrip");
    assert_eq!(got.host, "10.0.0.5");
    assert_eq!(got.username, "admin");

    // 清理
    let _ = std::fs::remove_file(&path);
}

#[test]
fn encrypted_password_survives_roundtrip() {
    // 构造一个带 Password 的 Session
    let mut store = SessionStore::new();
    let mut s = Session::new("enc", "127.0.0.1", "me");
    s.auth = AuthMethod::Password {
        password: EncryptedString::new("hunter2"),
    };
    store.upsert(s.clone());

    let path = tmp_path("enc_roundtrip");
    // 实际 DPAPI 加密 + 解密往返（只在 Windows 上真实生效；非 Windows 跳过）
    #[cfg(target_os = "windows")]
    {
        store
            .save(&path, |s| rterm_session::crypto_bridge::encrypt_session_dpapi(s))
            .expect("save with dpapi");

        let cipher_json = std::fs::read_to_string(&path).unwrap();
        // 密码字节应该是 DPAPI 密文（base64），不是明文 "hunter2"
        assert!(!cipher_json.contains("hunter2"), "密码明文不应出现在 JSON 中");

        let loaded = SessionStore::load(&path, |s| rterm_session::crypto_bridge::decrypt_session_dpapi(s))
            .expect("load with dpapi");
        let got = loaded.get(s.id).unwrap();
        if let AuthMethod::Password { password } = &got.auth {
            assert_eq!(password.data, b"hunter2");
            assert_eq!(password.version, 0);
        } else {
            panic!("expected Password variant");
        }
    }
    #[cfg(not(target_os = "windows"))]
    {
        // 非 Windows：identity 函数往返
        store.save(&path, |s| Ok(s.clone())).unwrap();
        let loaded = SessionStore::load(&path, |s| Ok(s.clone())).unwrap();
        assert_eq!(loaded.count(), 1);
    }

    let _ = std::fs::remove_file(&path);
}

#[test]
fn atomic_write_leaves_no_tmp() {
    let path = tmp_path("atomic");
    let mut store = SessionStore::new();
    store.upsert(Session::new("a", "1.1.1.1", "x"));
    store.save(&path, |s| Ok(s.clone())).unwrap();

    assert!(path.exists());
    let tmp = path.with_extension("tmp");
    assert!(!tmp.exists(), "tmp 文件应被 rename 掉");
    let _ = std::fs::remove_file(&path);
}

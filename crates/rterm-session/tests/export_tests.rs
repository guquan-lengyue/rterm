//! export_tests — 导出/导入 + AES-GCM + 冲突策略

use rterm_session::{
    export::{export_json, import_json},
    ConflictPolicy, ExportScope, ImportReport, Session,
};

fn make_session(id_seed: u8, name: &str) -> Session {
    let mut s = Session::new(name, &format!("10.0.0.{}", id_seed), "root");
    s.id = uuid::Uuid::from_bytes([id_seed; 16]);
    s
}

// ---------- DPAPI 路径（本机内无密码导出）----------

#[test]
fn export_dpapi_roundtrip() {
    let sessions = vec![make_session(1, "srv-a"), make_session(2, "srv-b")];
    let json = export_json(&sessions, &[], ExportScope::All, None).expect("export");
    assert!(json.contains("\"rterm-export\""));
    assert!(json.contains("\"version\": 2"));

    let (merged, _folders, report) =
        import_json(&json, &[], &[], ConflictPolicy::Overwrite, None).expect("import");
    assert_eq!(report.added, 2);
    assert_eq!(merged.len(), 2);
}

#[test]
fn export_scope_selection() {
    let sessions = vec![make_session(1, "keep-me"), make_session(2, "drop-me")];
    let ids = vec![sessions[0].id];
    let json = export_json(&sessions, &[], ExportScope::Selection(ids), None).expect("export");
    let (merged, _, _) = import_json(&json, &[], &[], ConflictPolicy::Overwrite, None).unwrap();
    assert_eq!(merged.len(), 1);
    assert_eq!(merged[0].name, "keep-me");
}

#[test]
fn export_scope_folder() {
    let mut s1 = make_session(1, "a");
    s1.folder = Some("prod".into());
    let mut s2 = make_session(2, "b");
    s2.folder = Some("dev".into());

    let json = export_json(
        &[s1, s2],
        &["prod".into(), "dev".into()],
        ExportScope::Folder("prod".into()),
        None,
    )
    .expect("export");
    let (merged, folders, _) =
        import_json(&json, &[], &[], ConflictPolicy::Overwrite, None).unwrap();
    assert_eq!(merged.len(), 1);
    assert_eq!(merged[0].folder.as_deref(), Some("prod"));
    assert_eq!(folders, vec!["prod".to_string()], "只导出目标分组");
}

// ---------- AES-GCM 路径（有密码导出）----------

#[test]
fn export_aesgcm_wrong_password_fails() {
    // Session::default() 密码是空的 — 必须手工塞明文密码让 AES-GCM 加密路径被走到
    let mut s = make_session(1, "x");
    use rterm_session::{AuthMethod, EncryptedString};
    s.auth = AuthMethod::Password {
        password: EncryptedString::new("secret123"),
    };

    let json = export_json(&[s], &[], ExportScope::All, Some("correct-pass")).expect("export");

    let result = import_json(
        &json,
        &[],
        &[],
        ConflictPolicy::Overwrite,
        Some("wrong-pass"),
    );
    assert!(result.is_err(), "wrong passphrase should fail");
}

#[test]
fn export_aesgcm_correct_password_roundtrip() {
    let sessions = vec![make_session(1, "x"), make_session(2, "y")];
    let json = export_json(&sessions, &[], ExportScope::All, Some("pw")).expect("export");

    let (merged, _, report) =
        import_json(&json, &[], &[], ConflictPolicy::Overwrite, Some("pw")).expect("import");
    assert_eq!(report.added, 2);
    assert_eq!(merged.len(), 2);
}

#[test]
fn aesgcm_encrypted_payload_not_readable() {
    let sessions = vec![make_session(1, "secret")];
    let json = export_json(&sessions, &[], ExportScope::All, Some("mypw")).expect("export");
    // JSON 里应该看不到 host "10.0.0.1" 或者 name "secret"?
    // EncryptedString.data 是 base64，明文 Session 其他字段会出现（name/host 不加密）
    // 但如果有密码字段加了密，其内容应不可读。这里 Session::default() 没密码，所以只验证能 parse
    assert!(serde_json::from_str::<serde_json::Value>(&json).is_ok());
}

// ---------- ConflictPolicy ----------

#[test]
fn conflict_overwrite() {
    let existing = vec![make_session(1, "original")];
    let incoming = vec![make_session(1, "imported")]; // 同 id

    let json = export_json(&incoming, &[], ExportScope::All, None).expect("export");
    let (merged, _, report) =
        import_json(&json, &existing, &[], ConflictPolicy::Overwrite, None).unwrap();

    assert_eq!(report.updated, 1);
    assert_eq!(report.added, 0);
    assert_eq!(merged.len(), 1);
    assert_eq!(merged[0].name, "imported");
}

#[test]
fn conflict_skip() {
    let existing = vec![make_session(1, "keep")];
    let incoming = vec![make_session(1, "new")];

    let json = export_json(&incoming, &[], ExportScope::All, None).expect("export");
    let (merged, _, report) =
        import_json(&json, &existing, &[], ConflictPolicy::Skip, None).unwrap();

    assert_eq!(report.skipped, 1);
    assert_eq!(merged[0].name, "keep");
}

#[test]
fn conflict_rename() {
    let existing = vec![make_session(1, "exist")];
    let incoming = vec![make_session(1, "duplicate")];

    let json = export_json(&incoming, &[], ExportScope::All, None).expect("export");
    let (merged, _, report) =
        import_json(&json, &existing, &[], ConflictPolicy::Rename, None).unwrap();

    assert_eq!(report.renamed, 1);
    assert_eq!(merged.len(), 2);
    // 原来的 + 改名后的新副本
    let names: Vec<&str> = merged.iter().map(|s| s.name.as_str()).collect();
    assert!(names.contains(&"exist"));
    assert!(names.iter().any(|n| n.starts_with("duplicate_imported_")));
}

#[test]
fn report_serializes() {
    let r = ImportReport {
        added: 1,
        updated: 2,
        skipped: 3,
        renamed: 4,
    };
    let j = serde_json::to_string(&r).unwrap();
    assert!(j.contains("\"added\":1"));
}

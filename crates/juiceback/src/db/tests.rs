use rusqlite::Connection;

use super::*;

#[test]
fn session_storage_uses_only_token_hash() {
    let conn = Connection::open_in_memory().unwrap();
    init_db(&conn).unwrap();
    let token = crate::auth::new_session_token();
    let hash = crate::auth::session_token_hash(&token);
    create_session(&conn, &hash, "user-1", 10, 20).unwrap();

    let stored: String = conn
        .query_row("SELECT token_hash FROM sessions", [], |row| row.get(0))
        .unwrap();
    assert_eq!(stored, hash);
    assert_ne!(stored, token);
    assert_eq!(
        resolve_session(&conn, &hash, 11)
            .unwrap()
            .map(|(user_id, _)| user_id)
            .as_deref(),
        Some("user-1")
    );
    assert!(resolve_session(&conn, &hash, 20).unwrap().is_none());
}

#[test]
fn session_touch_is_coalesced_within_interval() {
    let conn = Connection::open_in_memory().unwrap();
    init_db(&conn).unwrap();
    let token = crate::auth::new_session_token();
    let hash = crate::auth::session_token_hash(&token);

    create_session(&conn, &hash, "user-1", 1000, 10_000).unwrap();

    let (user_id, last_seen) = resolve_session(&conn, &hash, 1010).unwrap().unwrap();
    assert_eq!(user_id, "user-1");
    assert_eq!(last_seen, 1000);

    let (_, last_seen) = resolve_session(
        &conn,
        &hash,
        1000 + crate::constants::SESSION_TOUCH_INTERVAL_SECS - 1,
    )
    .unwrap()
    .unwrap();
    assert_eq!(last_seen, 1000);

    let (_, last_seen) = resolve_session(
        &conn,
        &hash,
        1000 + crate::constants::SESSION_TOUCH_INTERVAL_SECS,
    )
    .unwrap()
    .unwrap();
    assert_eq!(
        last_seen,
        1000 + crate::constants::SESSION_TOUCH_INTERVAL_SECS
    );
}

#[test]
fn legacy_import_verifies_local_token_and_is_idempotent() {
    let conn = Connection::open_in_memory().unwrap();
    init_db(&conn).unwrap();
    let local = FileRecord::new(
        "local".into(),
        "a.txt".into(),
        "text/plain".into(),
        1,
        "valid-token".into(),
        10,
        20,
        None,
        None,
    );
    let remote = FileRecord::new(
        "remote".into(),
        "b.txt".into(),
        "text/plain".into(),
        1,
        "remote-token".into(),
        10,
        20,
        None,
        Some("https://elsewhere.test".into()),
    );
    insert_file(&conn, &local).unwrap();
    insert_file(&conn, &remote).unwrap();
    let records = vec![
        ClientFileRecord {
            id: "local".into(),
            token: "valid-token".into(),
            n: "ignored".into(),
            m: String::new(),
            s: 0,
            u: 0,
            e: 0,
        },
        ClientFileRecord {
            id: "remote".into(),
            token: "remote-token".into(),
            n: String::new(),
            m: String::new(),
            s: 0,
            u: 0,
            e: 0,
        },
        ClientFileRecord {
            id: "local".into(),
            token: "wrong".into(),
            n: String::new(),
            m: String::new(),
            s: 0,
            u: 0,
            e: 0,
        },
    ];

    assert_eq!(
        import_verified_client_files(&conn, "user-1", &records).unwrap(),
        1
    );
    assert_eq!(
        import_verified_client_files(&conn, "user-1", &records).unwrap(),
        0
    );
    let imported = list_client_files(&conn, "user-1").unwrap();
    assert_eq!(imported.len(), 1);
    assert_eq!(imported[0].n, "a.txt");
    assert!(
        !serde_json::to_string(&imported)
            .unwrap()
            .contains("valid-token")
    );
}

#[test]
fn removed_device_is_immediately_revoked() {
    let conn = Connection::open_in_memory().unwrap();
    init_db(&conn).unwrap();
    conn.execute(
        "INSERT INTO devices (id, user_id, device_name, paired_at, last_seen_at) VALUES ('d', 'u', 'test', 1, 1)",
        [],
    ).unwrap();
    assert!(device_belongs_to_user(&conn, "d", "u").unwrap());
    conn.execute("DELETE FROM devices WHERE id = 'd'", [])
        .unwrap();
    assert!(!device_belongs_to_user(&conn, "d", "u").unwrap());
}

#[test]
fn file_record_new_sets_fields() {
    let record = FileRecord::new(
        "id1".into(),
        "test.txt".into(),
        "text/plain".into(),
        1024,
        "token1".into(),
        100,
        200,
        Some("1.2.3.4".into()),
        Some("https://files.example.com".into()),
    );
    assert_eq!(record.id, "id1");
    assert_eq!(record.filename, "test.txt");
    assert_eq!(record.mime_type, "text/plain");
    assert_eq!(record.size_bytes, 1024);
    assert_eq!(record.storage_path, "remote-id1");
    assert_eq!(record.delete_token, "token1");
    assert_eq!(record.uploaded_at, 100);
    assert_eq!(record.expires_at, 200);
    assert_eq!(record.uploader_ip.as_deref(), Some("1.2.3.4"));
    assert_eq!(
        record.storage_host.as_deref(),
        Some("https://files.example.com")
    );
}

#[test]
fn file_record_from_upload_with_token_ttl() {
    let before = chrono::Utc::now().timestamp();
    let record = FileRecord::from_upload_with_token(
        "id2".into(),
        "file.bin".into(),
        "application/octet-stream".into(),
        2048,
        24.0,
        "tok2".into(),
        None,
        None,
    );
    let after = chrono::Utc::now().timestamp();
    assert_eq!(record.id, "id2");
    assert_eq!(record.size_bytes, 2048);
    assert_eq!(record.delete_token, "tok2");
    assert!(record.uploaded_at >= before);
    assert!(record.expires_at <= after + 24 * 3600 + 2);
    assert!(record.expires_at >= before + 24 * 3600 - 2);
    assert!(record.uploader_ip.is_none());
    assert!(record.storage_host.is_none());
}

#[test]
fn file_record_from_upload_with_token_clamped_high() {
    let before = chrono::Utc::now().timestamp();
    let record = FileRecord::from_upload_with_token(
        "id3".into(),
        "file.bin".into(),
        "application/octet-stream".into(),
        100,
        99999.0,
        "tok3".into(),
        None,
        None,
    );
    let expected_ttl = (99999.0 * crate::constants::SECONDS_PER_HOUR_F64).round() as i64;
    assert!(record.expires_at >= before + expected_ttl - 2);
    assert!(record.expires_at <= before + expected_ttl + 3);
}

#[test]
fn file_record_from_upload_with_token_clamped_low() {
    let before = chrono::Utc::now().timestamp();
    let record = FileRecord::from_upload_with_token(
        "id4".into(),
        "file.bin".into(),
        "application/octet-stream".into(),
        100,
        0.0001,
        "tok4".into(),
        None,
        None,
    );
    let expected_ttl = (0.0001 * crate::constants::SECONDS_PER_HOUR_F64).round() as i64;
    assert!(record.expires_at >= before + expected_ttl - 2);
    assert!(record.expires_at <= before + expected_ttl + 3);
}

fn setup_db() -> Connection {
    let conn = Connection::open_in_memory().unwrap();
    init_db(&conn).unwrap();
    conn
}

#[test]
fn test_init_db_idempotent() {
    let conn = setup_db();
    init_db(&conn).unwrap();
}

#[test]
fn test_insert_get_file() {
    let conn = setup_db();
    let record = FileRecord::new(
        "test1".into(),
        "file.txt".into(),
        "text/plain".into(),
        100,
        "tok1".into(),
        1000,
        2000,
        Some("127.0.0.1".into()),
        None,
    );
    insert_file(&conn, &record).unwrap();
    let got = get_file(&conn, "test1").unwrap().unwrap();
    assert_eq!(got.id, "test1");
    assert_eq!(got.filename, "file.txt");
    assert_eq!(got.size_bytes, 100);
}

#[test]
fn test_get_file_not_found() {
    let conn = setup_db();
    assert!(get_file(&conn, "nonexistent").unwrap().is_none());
}

#[test]
fn finish_reservation_requires_token_and_uploading_status() {
    let conn = setup_db();
    let mut pending = FileRecord::new(
        "reserved".into(),
        "placeholder.txt".into(),
        "text/plain".into(),
        0,
        "secret".into(),
        100,
        200,
        None,
        None,
    );
    pending.status = "uploading".into();
    insert_file(&conn, &pending).unwrap();

    assert!(matches!(
        finish_reservation(
            &conn,
            "final.txt",
            "text/plain",
            4,
            "reserved",
            "wrong",
            None
        )
        .unwrap(),
        FinishReservationResult::InvalidToken
    ));
    assert_eq!(
        get_file(&conn, "reserved").unwrap().unwrap().status,
        "uploading"
    );
    let completed = finish_reservation(
        &conn,
        "final.txt",
        "text/plain",
        4,
        "reserved",
        "secret",
        None,
    )
    .unwrap();
    assert!(matches!(completed, FinishReservationResult::Completed(_)));

    assert!(matches!(
        finish_reservation(
            &conn,
            "again.txt",
            "text/plain",
            5,
            "reserved",
            "secret",
            None
        )
        .unwrap(),
        FinishReservationResult::NotUploading
    ));
}

#[test]
fn finish_reservation_propagates_database_errors() {
    let conn = setup_db();
    conn.execute("DROP TABLE files", []).unwrap();
    assert!(
        finish_reservation(
            &conn,
            "final.txt",
            "text/plain",
            4,
            "reserved",
            "secret",
            None,
        )
        .is_err()
    );
}

#[test]
fn test_get_files_by_ids() {
    let conn = setup_db();
    for i in 0..3 {
        let record = FileRecord::new(
            format!("id{i}"),
            "f.txt".into(),
            "text/plain".into(),
            10,
            format!("tok{i}"),
            100,
            200,
            None,
            None,
        );
        insert_file(&conn, &record).unwrap();
    }
    let files = get_files_by_ids(&conn, &["id0".into(), "id2".into()]).unwrap();
    assert_eq!(files.len(), 2);
    assert!(files.iter().any(|f| f.id == "id0"));
    assert!(files.iter().any(|f| f.id == "id2"));
}

#[test]
fn test_delete_file() {
    let conn = setup_db();
    let record = FileRecord::new(
        "del1".into(),
        "f.txt".into(),
        "text/plain".into(),
        10,
        "tok".into(),
        100,
        200,
        None,
        None,
    );
    insert_file(&conn, &record).unwrap();
    assert!(delete_file(&conn, "del1").unwrap());
    assert!(get_file(&conn, "del1").unwrap().is_none());
}

#[test]
fn test_list_all_files_ordered() {
    let conn = setup_db();
    for i in 0..3 {
        let record = FileRecord::new(
            format!("id{i}"),
            "f.txt".into(),
            "text/plain".into(),
            10,
            "tok".into(),
            1000 + i,
            2000,
            None,
            None,
        );
        insert_file(&conn, &record).unwrap();
    }
    let files = list_all_files(&conn).unwrap();
    assert_eq!(files.len(), 3);
    assert!(files[0].uploaded_at >= files[1].uploaded_at);
    assert!(files[1].uploaded_at >= files[2].uploaded_at);
}

#[test]
fn test_renew_file_id() {
    let conn = setup_db();
    let record = FileRecord::new(
        "old_id".into(),
        "f.txt".into(),
        "text/plain".into(),
        10,
        "tok".into(),
        100,
        200,
        None,
        None,
    );
    insert_file(&conn, &record).unwrap();
    assert!(renew_file_id(&conn, "old_id", "new_id", "remote-new_id").unwrap());
    assert!(get_file(&conn, "old_id").unwrap().is_none());
    let renewed = get_file(&conn, "new_id").unwrap().unwrap();
    assert_eq!(renewed.delete_token, "tok");
    assert_eq!(renewed.storage_path, "remote-new_id");
}

#[test]
fn test_list_expired() {
    let conn = setup_db();
    let past = FileRecord::new(
        "expired".into(),
        "f.txt".into(),
        "text/plain".into(),
        10,
        "tok".into(),
        100,
        1,
        None,
        None,
    );
    let future = FileRecord::new(
        "active".into(),
        "f.txt".into(),
        "text/plain".into(),
        10,
        "tok".into(),
        100,
        9_999_999_999,
        None,
        None,
    );
    insert_file(&conn, &past).unwrap();
    insert_file(&conn, &future).unwrap();
    let expired = list_expired_batch(&conn, "", 500).unwrap();
    assert_eq!(expired.len(), 1);
    assert_eq!(expired[0].id, "expired");
}

#[test]
fn expired_batch_paginates_with_cursor_and_skips_deleted() {
    let conn = setup_db();
    for i in 0..1200 {
        let mut record = FileRecord::new(
            format!("file-{i:05}"),
            "f.txt".into(),
            "text/plain".into(),
            10,
            "tok".into(),
            100,
            1,
            None,
            None,
        );

        if i % 10 == 0 {
            record.expires_at = 9_999_999_999;
        }
        insert_file(&conn, &record).unwrap();
    }
    delete_files_by_ids(
        &conn,
        &(0..100).map(|i| format!("file-{i:05}")).collect::<Vec<_>>(),
    )
    .unwrap();

    let mut seen = Vec::new();
    let mut cursor = String::new();
    loop {
        let batch = list_expired_batch(&conn, &cursor, 500).unwrap();
        if batch.is_empty() {
            break;
        }
        assert!(batch.len() <= 500);
        cursor = batch.last().unwrap().id.clone();
        seen.extend(batch.into_iter().map(|r| r.id));
    }

    assert_eq!(seen.len(), 990);
    let mut sorted = seen.clone();
    sorted.sort();
    sorted.dedup();
    assert_eq!(sorted.len(), 990);
    assert!(seen.iter().all(|id| !id.ends_with('0')));
}

#[test]
fn abandoned_batch_filters_status_and_paginates() {
    let conn = setup_db();
    for i in 0..30 {
        let mut record = FileRecord::new(
            format!("ab-{i:02}"),
            "f.txt".into(),
            "text/plain".into(),
            0,
            "tok".into(),
            50,
            9_999_999_999,
            None,
            None,
        );
        if i % 3 == 0 {
            record.status = "uploading".into();
        }
        insert_file(&conn, &record).unwrap();
    }
    let first = list_abandoned_uploads_batch(&conn, 0, "", 4).unwrap();
    assert_eq!(first.len(), 4);
    assert!(first.iter().all(|r| r.status == "uploading"));
    let cursor = first.last().unwrap().id.clone();
    let second = list_abandoned_uploads_batch(&conn, 0, &cursor, 100).unwrap();
    assert_eq!(second.len(), 6);
    assert!(second.iter().all(|r| r.status == "uploading"));
    let end = second.last().unwrap().id.clone();
    assert!(
        list_abandoned_uploads_batch(&conn, 0, &end, 100)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn cleanup_indexes_exist() {
    let conn = setup_db();
    for name in [
        "idx_expires_at",
        "idx_files_status_uploaded",
        "idx_pairing_codes_user",
        "idx_pairing_codes_ip",
        "idx_failed_logins_user_ip",
        "idx_client_files_client_uploaded",
        "idx_fetch_jobs_status_updated",
    ] {
        let found: String = conn
            .query_row(
                "SELECT name FROM sqlite_master WHERE type = 'index' AND name = ?1",
                [name],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(found, name);
    }
}

#[test]
fn test_delete_files_by_ids() {
    let conn = setup_db();
    for i in 0..3 {
        let record = FileRecord::new(
            format!("bulk{i}"),
            "f.txt".into(),
            "text/plain".into(),
            10,
            "tok".into(),
            100,
            200,
            None,
            None,
        );
        insert_file(&conn, &record).unwrap();
    }
    let deleted = delete_files_by_ids(&conn, &["bulk0".into(), "bulk2".into()]).unwrap();
    assert_eq!(deleted, 2);
    assert!(get_file(&conn, "bulk0").unwrap().is_none());
    assert!(get_file(&conn, "bulk1").unwrap().is_some());
    assert!(get_file(&conn, "bulk2").unwrap().is_none());
}

#[test]
fn test_insert_list_delete_report() {
    let conn = setup_db();
    let id = insert_report(
        &conn,
        "http://example.com",
        "spam",
        "details",
        Some("1.2.3.4"),
        Some("a@b.com"),
    )
    .unwrap();
    assert!(id > 0);
    let reports = list_reports(&conn).unwrap();
    assert_eq!(reports.len(), 1);
    assert_eq!(reports[0].file_url, "http://example.com");
    assert!(delete_report(&conn, id).unwrap());
    assert!(list_reports(&conn).unwrap().is_empty());
}

#[test]
fn test_insert_list_delete_feedback() {
    let conn = setup_db();
    let id = insert_feedback(&conn, "great app", Some("a@b.com"), Some("1.2.3.4")).unwrap();
    assert!(id > 0);
    let feedback = list_feedback(&conn).unwrap();
    assert_eq!(feedback.len(), 1);
    assert_eq!(feedback[0].message, "great app");
    assert!(delete_feedback(&conn, id).unwrap());
    assert!(list_feedback(&conn).unwrap().is_empty());
}

#[test]
fn cleanup_ephemeral_data_removes_only_expired_or_retained_rows() {
    let conn = setup_db();
    create_session(&conn, "expired", "u", 1, 10).unwrap();
    create_session(&conn, "active", "u", 1, 200).unwrap();
    conn.execute(
        "INSERT INTO pairing_codes (code_hash, user_id, created_at, expires_at, used) VALUES
         ('expired', 'u', 1, 10, 0), ('used', 'u', 1, 200, 1), ('active', 'u', 1, 200, 0)",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO reports (file_url, reason, details, created_at) VALUES ('old', 'r', '', 10), ('new', 'r', '', 90)",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO feedback (message, created_at) VALUES ('old', 10), ('new', 90)",
        [],
    )
    .unwrap();

    assert_eq!(cleanup_ephemeral_data(&conn, 100, 50, 50).unwrap(), 5);
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM sessions", [], |row| row
            .get::<_, i64>(0))
            .unwrap(),
        1
    );
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM pairing_codes", [], |row| row
            .get::<_, i64>(0))
            .unwrap(),
        1
    );
    assert_eq!(list_reports(&conn).unwrap().len(), 1);
    assert_eq!(list_feedback(&conn).unwrap().len(), 1);
}

#[test]
fn failed_login_counts_are_scoped_to_username_and_ip() {
    let conn = setup_db();
    insert_failed_login_for_ip(&conn, "admin", "ip-a").unwrap();
    insert_failed_login_for_ip(&conn, "admin", "ip-a").unwrap();
    insert_failed_login_for_ip(&conn, "admin", "ip-b").unwrap();
    insert_failed_login_for_ip(&conn, "other", "ip-a").unwrap();

    assert_eq!(
        count_failed_logins_for_ip(&conn, "admin", "ip-a", 60).unwrap(),
        2
    );
    assert_eq!(
        count_failed_logins_for_ip(&conn, "admin", "ip-b", 60).unwrap(),
        1
    );
    delete_failed_logins_for_ip(&conn, "admin", "ip-a").unwrap();
    assert_eq!(
        count_failed_logins_for_ip(&conn, "admin", "ip-a", 60).unwrap(),
        0
    );
    assert_eq!(
        count_failed_logins_for_ip(&conn, "other", "ip-a", 60).unwrap(),
        1
    );
}

#[test]
fn test_ban_unban_ip() {
    let conn = setup_db();
    ban_ip(&conn, "10.0.0.1", "spam", "admin").unwrap();
    let record = get_ban_record(&conn, "10.0.0.1").unwrap().unwrap();
    assert_eq!(record.ip, "10.0.0.1");
    assert_eq!(record.reason, "spam");
    assert!(unban_ip(&conn, "10.0.0.1").unwrap());
    assert!(get_ban_record(&conn, "10.0.0.1").unwrap().is_none());
}

#[test]
fn test_load_banned_ips_set() {
    let conn = setup_db();
    ban_ip(&conn, "1.1.1.1", "r1", "admin").unwrap();
    ban_ip(&conn, "2.2.2.2", "r2", "admin").unwrap();
    let set = load_banned_ips_set(&conn).unwrap();
    assert_eq!(set.len(), 2);
    assert!(set.contains("1.1.1.1"));
    assert!(set.contains("2.2.2.2"));
}

#[test]
fn test_announcement_upsert() {
    let conn = setup_db();
    let a1 = upsert_announcement(&conn, "Hello", "https://example.com", "info", true).unwrap();
    assert!(a1.is_active);
    let a2 = upsert_announcement(&conn, "Updated", "https://new.com", "warning", true).unwrap();
    assert_eq!(a2.message, "Updated");
    let active = get_active_announcement(&conn).unwrap().unwrap();
    assert_eq!(active.id, a2.id);
}

#[test]
fn test_get_admin_by_username() {
    let conn = setup_db();
    let result = get_admin_by_username(&conn, "admin").unwrap();
    assert!(result.is_none());
}

#[test]
fn test_update_client_file_id_swaps_key() {
    let conn = setup_db();
    let rec = ClientFileRecord {
        id: "old_id".into(),
        token: "tok".into(),
        n: "f.txt".into(),
        m: "text/plain".into(),
        s: 10,
        u: 100,
        e: 200,
    };
    replace_client_files(&conn, "client-1", &[rec]).unwrap();
    assert!(update_client_file_id(&conn, "client-1", "old_id", "new_id").unwrap());
    let files = list_client_files(&conn, "client-1").unwrap();
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].id, "new_id");
    assert_eq!(files[0].token, "tok");

    assert!(!update_client_file_id(&conn, "client-1", "missing", "x").unwrap());
}

#[test]
fn test_update_client_file_id_clears_new_id_clash() {
    let conn = setup_db();
    let recs = vec![
        ClientFileRecord {
            id: "old_id".into(),
            token: "tok".into(),
            n: "f.txt".into(),
            m: String::new(),
            s: 10,
            u: 100,
            e: 200,
        },
        ClientFileRecord {
            id: "new_id".into(),
            token: "tok2".into(),
            n: "g.txt".into(),
            m: String::new(),
            s: 20,
            u: 100,
            e: 200,
        },
    ];
    replace_client_files(&conn, "client-1", &recs).unwrap();
    assert!(update_client_file_id(&conn, "client-1", "old_id", "new_id").unwrap());
    let files = list_client_files(&conn, "client-1").unwrap();
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].id, "new_id");
    assert_eq!(files[0].token, "tok");
}

#[test]
fn migrate_twice_is_stable() {
    let conn = Connection::open_in_memory().unwrap();
    init_db(&conn).unwrap();
    let version = schema_version(&conn).unwrap();
    assert_eq!(version, migration_count());
    assert!(version >= 1);
    init_db(&conn).unwrap();
    assert_eq!(schema_version(&conn).unwrap(), version);
}

#[test]
fn legacy_files_table_gains_new_columns() {
    let conn = Connection::open_in_memory().unwrap();

    conn.execute_batch(
        "CREATE TABLE files (
            id            TEXT PRIMARY KEY,
            filename      TEXT NOT NULL,
            mime_type     TEXT NOT NULL,
            size_bytes    INTEGER NOT NULL,
            storage_path  TEXT NOT NULL UNIQUE,
            delete_token  TEXT NOT NULL,
            uploaded_at   INTEGER NOT NULL,
            expires_at    INTEGER NOT NULL,
            uploader_ip   TEXT
        );",
    )
    .unwrap();
    init_db(&conn).unwrap();
    let cols: Vec<String> = conn
        .prepare("PRAGMA table_info(files)")
        .unwrap()
        .query_map([], |row| row.get(1))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    assert!(cols.contains(&"storage_host".to_string()));
    assert!(cols.contains(&"status".to_string()));
    assert_eq!(schema_version(&conn).unwrap(), migration_count());
}

#[test]
fn renew_ids_are_atomic_in_transaction() {
    let conn = Connection::open_in_memory().unwrap();
    init_db(&conn).unwrap();

    conn.execute(
        "INSERT INTO files (id, filename, mime_type, size_bytes, storage_path, delete_token, uploaded_at, expires_at) VALUES ('old', 'a.txt', 'text/plain', 10, 'remote-old', 'tok', 1000, 2000)",
        [],
    )
    .unwrap();

    conn.unchecked_transaction()
        .map(|tx| {
            insert_alias(&tx, "old", "new").unwrap();
            assert!(renew_file_id(&tx, "old", "new", "remote-new").unwrap());
            tx.commit().unwrap();
        })
        .unwrap();
    assert_eq!(resolve_alias(&conn, "old").unwrap().as_deref(), Some("new"));

    let tx = conn.unchecked_transaction().unwrap();
    insert_alias(&tx, "new", "newer").unwrap();
    tx.execute("INSERT INTO files (id) VALUES (NULL)", [])
        .unwrap_err();
    tx.rollback().unwrap();
    assert!(resolve_alias(&conn, "new").unwrap().is_none());
}

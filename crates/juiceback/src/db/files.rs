use rusqlite::{Connection, OptionalExtension, Result, params};

use super::types::{FileRecord, FinishReservationResult};

pub fn finish_reservation(
    db: &Connection,
    filename: &str,
    mime_type: &str,
    size_bytes: i64,
    id: &str,
    delete_token: &str,
    uploader_ip: Option<&str>,
) -> Result<FinishReservationResult> {
    let affected = if let Some(ip) = uploader_ip {
        db.execute(
            "UPDATE files SET filename = ?1, mime_type = ?2, size_bytes = ?3, uploader_ip = ?6, status = 'ready', \
             expires_at = expires_at + CASE WHEN uploaded_at > 0 AND unixepoch() > uploaded_at THEN unixepoch() - uploaded_at ELSE 0 END \
             WHERE id = ?4 AND delete_token = ?5 AND status = 'uploading'",
            params![filename, mime_type, size_bytes, id, delete_token, ip],
        )?
    } else {
        db.execute(
            "UPDATE files SET filename = ?1, mime_type = ?2, size_bytes = ?3, status = 'ready', \
             expires_at = expires_at + CASE WHEN uploaded_at > 0 AND unixepoch() > uploaded_at THEN unixepoch() - uploaded_at ELSE 0 END \
             WHERE id = ?4 AND delete_token = ?5 AND status = 'uploading'",
            params![filename, mime_type, size_bytes, id, delete_token],
        )?
    };
    if affected == 1 {
        return Ok(FinishReservationResult::Completed(
            get_file(db, id)?.ok_or(rusqlite::Error::QueryReturnedNoRows)?,
        ));
    }

    let Some(record) = get_file(db, id)? else {
        return Ok(FinishReservationResult::NotFound);
    };
    if !juiceutils::constant_time_eq(&record.delete_token, delete_token) {
        return Ok(FinishReservationResult::InvalidToken);
    }
    if record.status != FileRecord::STATUS_UPLOADING {
        return Ok(FinishReservationResult::NotUploading);
    }
    Ok(FinishReservationResult::NotUploading)
}

/// Column order must match `row_to_file_record`.
pub(crate) const FILE_COLUMNS: &str = "id, filename, mime_type, size_bytes, storage_path, \
    delete_token, uploaded_at, expires_at, uploader_ip, storage_host, status, \
    password_hash, is_encrypted, enc_header, dek_wrapped, dek_salt, dek_escrow, key_version, \
    tier, r2_key, tier_pinned, title, author, peer_device_id, last_seen";

/// Missing values from older rows fall back to ready/standard/0.
pub(crate) fn row_to_file_record(row: &rusqlite::Row) -> rusqlite::Result<FileRecord> {
    let raw_host: String = row.get(9)?;
    let status: String = row
        .get(10)
        .unwrap_or_else(|_| FileRecord::STATUS_READY.to_string());
    Ok(FileRecord {
        id: row.get(0)?,
        filename: row.get(1)?,
        mime_type: row.get(2)?,
        size_bytes: row.get(3)?,
        storage_path: row.get(4)?,
        delete_token: row.get(5)?,
        uploaded_at: row.get(6)?,
        expires_at: row.get(7)?,
        uploader_ip: row.get(8)?,
        storage_host: if raw_host.is_empty() {
            None
        } else {
            Some(raw_host)
        },
        status,
        password_hash: row.get(11)?,
        is_encrypted: row.get::<_, i64>(12).unwrap_or(0) != 0,
        enc_header: row.get(13)?,
        dek_wrapped: row.get(14).unwrap_or(None),
        dek_salt: row.get(15).unwrap_or(None),
        dek_escrow: row.get(16).unwrap_or(None),
        key_version: row.get(17).unwrap_or(0),
        tier: row
            .get(18)
            .unwrap_or_else(|_| FileRecord::TIER_STANDARD.to_string()),
        r2_key: row.get(19).unwrap_or_default(),
        tier_pinned: row.get::<_, i64>(20).unwrap_or(0) != 0,
        title: row.get(21).unwrap_or_default(),
        author: row.get(22).unwrap_or_default(),
        peer_device_id: row.get(23).unwrap_or_default(),
        last_seen: row.get(24).unwrap_or(0),
    })
}

pub fn insert_file(conn: &Connection, record: &FileRecord) -> Result<()> {
    conn.execute(
        &format!(
            "INSERT INTO files ({FILE_COLUMNS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25)"
        ),
        params![
            record.id,
            record.filename,
            record.mime_type,
            record.size_bytes,
            record.storage_path,
            record.delete_token,
            record.uploaded_at,
            record.expires_at,
            record.uploader_ip,
            record.storage_host.as_deref().unwrap_or(""),
            record.status,
            record.password_hash.as_deref(),
            i64::from(record.is_encrypted),
            record.enc_header.as_deref(),
            record.dek_wrapped.as_deref(),
            record.dek_salt.as_deref(),
            record.dek_escrow.as_deref(),
            record.key_version,
            record.tier,
            record.r2_key,
            i64::from(record.tier_pinned),
            record.title,
            record.author,
            record.peer_device_id,
            record.last_seen,
        ],
    )?;
    Ok(())
}

pub async fn insert_new_file(
    state: &std::sync::Arc<crate::state::AppState>,
    record: FileRecord,
) -> Result<FileRecord, crate::error::AppError> {
    let r2 = record.clone();
    state
        .db_call("insert_file", move |db| insert_file(db, &r2))
        .await?;
    Ok(record)
}

/// Forces `uploading` so an incomplete upload is never served as ready.
pub async fn insert_pending_file(
    state: &std::sync::Arc<crate::state::AppState>,
    mut record: FileRecord,
) -> Result<FileRecord, crate::error::AppError> {
    record.status = FileRecord::STATUS_UPLOADING.to_string();
    let r2 = record.clone();
    state
        .db_call("insert_pending_file", move |db| insert_file(db, &r2))
        .await?;
    Ok(record)
}

/// Errors if the row is not `uploading`; extends expiry by upload time.
pub async fn finish_file(
    state: &std::sync::Arc<crate::state::AppState>,
    id: String,
) -> Result<(), crate::error::AppError> {
    state
        .db_call("finish_file", move |db| {
            let affected = db.execute(
                "UPDATE files SET status = 'ready', \
                 expires_at = expires_at + CASE WHEN uploaded_at > 0 AND unixepoch() > uploaded_at THEN unixepoch() - uploaded_at ELSE 0 END \
                 WHERE id = ?1 AND status = 'uploading'",
                params![id],
            )?;
            if affected == 0 {
                Err(rusqlite::Error::QueryReturnedNoRows)
            } else {
                Ok(())
            }
        })
        .await
}

pub async fn delete_pending_file(
    state: &std::sync::Arc<crate::state::AppState>,
    id: String,
) -> Result<(), crate::error::AppError> {
    state
        .db_call("delete_pending_file", move |db| {
            db.execute(
                "DELETE FROM files WHERE id = ?1 AND status = 'uploading'",
                params![id],
            )?;
            Ok(())
        })
        .await
}

pub fn get_file(conn: &Connection, id: &str) -> Result<Option<FileRecord>> {
    let mut stmt = conn.prepare(&format!("SELECT {FILE_COLUMNS} FROM files WHERE id = ?1"))?;

    let mut rows = stmt.query(params![id])?;

    let Some(row) = rows.next()? else {
        return Ok(None);
    };
    Ok(Some(row_to_file_record(row)?))
}

pub fn get_files_by_ids(conn: &Connection, ids: &[String]) -> Result<Vec<FileRecord>> {
    let mut out = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for chunk in ids.chunks(crate::constants::SQL_PARAM_CHUNK_SIZE) {
        let unique: Vec<&String> = chunk.iter().filter(|id| seen.insert(id.as_str())).collect();
        if unique.is_empty() {
            continue;
        }
        let placeholders: String = (1..=unique.len())
            .map(|i| format!("?{i}"))
            .collect::<Vec<_>>()
            .join(", ");
        let mut stmt = conn.prepare(&format!(
            "SELECT {FILE_COLUMNS} FROM files WHERE id IN ({placeholders})"
        ))?;
        let params: Vec<&dyn rusqlite::ToSql> =
            unique.iter().map(|s| *s as &dyn rusqlite::ToSql).collect();
        let rows = stmt.query_map(params.as_slice(), row_to_file_record)?;
        for row in rows {
            out.push(row?);
        }
    }
    Ok(out)
}

/// No shared transaction; orphaned stats are pruned later.
pub fn delete_file(conn: &Connection, id: &str) -> Result<bool> {
    let affected = conn.execute("DELETE FROM files WHERE id = ?1", params![id])?;
    if affected > 0 {
        super::stats::delete_file_stats(conn, id)?;
    }
    Ok(affected > 0)
}

pub fn set_protection(
    conn: &Connection,
    id: &str,
    material: &super::types::ProtectionMaterial,
) -> Result<bool> {
    let affected = conn.execute(
        "UPDATE files SET password_hash = ?1, is_encrypted = ?2, enc_header = ?3, \
         dek_wrapped = ?4, dek_salt = ?5, dek_escrow = ?6, key_version = ?7 WHERE id = ?8",
        params![
            material.password_hash,
            i64::from(material.is_encrypted),
            material.enc_header,
            material.dek_wrapped,
            material.dek_salt,
            material.dek_escrow,
            material.key_version,
            id
        ],
    )?;
    Ok(affected > 0)
}

pub fn list_all_files(conn: &Connection) -> Result<Vec<FileRecord>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {FILE_COLUMNS} FROM files ORDER BY uploaded_at DESC"
    ))?;
    let rows = stmt.query_map([], row_to_file_record)?;
    rows.collect()
}

/// Rewrites the id, storage path, and optional filename, carrying the stats
/// row along.
pub fn renew_file_id(
    conn: &Connection,
    old_id: &str,
    new_id: &str,
    new_storage_path: &str,
    new_filename: Option<&str>,
) -> Result<bool> {
    let affected = conn.execute(
        "UPDATE files SET id = ?1, storage_path = ?2, filename = COALESCE(?4, filename) WHERE id = ?3",
        params![new_id, new_storage_path, old_id, new_filename],
    )?;
    if affected > 0 {
        conn.execute(
            "UPDATE file_stats SET file_id = ?1 WHERE file_id = ?2",
            params![new_id, old_id],
        )?;
    }
    Ok(affected > 0)
}

/// Renames in place and keeps the denormalized client copy in step.
pub fn set_filename(conn: &Connection, id: &str, filename: &str) -> Result<bool> {
    let affected = conn.execute(
        "UPDATE files SET filename = ?1 WHERE id = ?2",
        params![filename, id],
    )?;
    if affected > 0 {
        conn.execute(
            "UPDATE client_files SET filename = ?1 WHERE file_id = ?2",
            params![filename, id],
        )?;
    }
    Ok(affected > 0)
}

pub fn insert_alias(conn: &Connection, old_id: &str, new_id: &str) -> Result<()> {
    conn.execute(
        "INSERT OR REPLACE INTO aliases (old_id, new_id) VALUES (?1, ?2)",
        params![old_id, new_id],
    )?;
    Ok(())
}

pub fn delete_alias(conn: &Connection, old_id: &str) -> Result<bool> {
    let affected = conn.execute("DELETE FROM aliases WHERE old_id = ?1", params![old_id])?;
    Ok(affected > 0)
}

/// Follows up to 10 alias hops; `None` when already canonical.
pub fn resolve_alias(conn: &Connection, old_id: &str) -> Result<Option<String>> {
    let mut current = old_id.to_string();
    for _ in 0..10 {
        let result: Option<String> = conn
            .query_row(
                "SELECT new_id FROM aliases WHERE old_id = ?1",
                params![current],
                |row| row.get(0),
            )
            .optional()?;
        match result {
            Some(next) if next != current => current = next,

            _ => {
                return Ok(if current == old_id {
                    None
                } else {
                    Some(current)
                });
            }
        }
    }
    Ok(Some(current))
}

/// Keyset pagination; pass the previous batch's last id as `after_id`.
pub fn list_expired_batch(
    conn: &Connection,
    after_id: &str,
    limit: usize,
) -> Result<Vec<FileRecord>> {
    // Peer rows hold no bytes on a storage host; the host-deletion cleanup path
    // must never see them. They are purged by `delete_expired_peer_files`.
    let mut stmt = conn.prepare(&format!(
        "SELECT {FILE_COLUMNS} FROM files WHERE expires_at < unixepoch() AND status != 'uploading' \
         AND tier != ?3 AND id > ?1 ORDER BY id LIMIT ?2"
    ))?;
    let rows = stmt.query_map(
        params![after_id, limit as i64, FileRecord::TIER_PEER],
        row_to_file_record,
    )?;
    rows.collect()
}

/// Removes expired peer rows without any storage-host interaction. Called
/// opportunistically by peer endpoints since the host cleanup skips them.
pub fn delete_expired_peer_files(conn: &Connection, now: i64) -> Result<usize> {
    conn.execute(
        "DELETE FROM files WHERE tier = ?1 AND expires_at < ?2",
        params![FileRecord::TIER_PEER, now],
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PeerRevokeResult {
    Revoked,
    NotFound,
    NotOwner,
}

/// Deletes a peer row only for the device that announced it.
pub fn revoke_peer_file(conn: &Connection, id: &str, device_id: &str) -> Result<PeerRevokeResult> {
    let owner: Option<String> = conn
        .query_row(
            "SELECT peer_device_id FROM files WHERE id = ?1 AND tier = ?2",
            params![id, FileRecord::TIER_PEER],
            |row| row.get(0),
        )
        .optional()?;
    let Some(owner) = owner else {
        return Ok(PeerRevokeResult::NotFound);
    };
    if !crate::utils::constant_time_eq(&owner, device_id) {
        return Ok(PeerRevokeResult::NotOwner);
    }
    conn.execute("DELETE FROM files WHERE id = ?1", params![id])?;
    Ok(PeerRevokeResult::Revoked)
}

/// Refreshes `last_seen` for a device's peer rows, optionally limited to ids.
pub fn touch_peer_files(
    conn: &Connection,
    device_id: &str,
    ids: Option<&[String]>,
    now: i64,
) -> Result<usize> {
    if let Some(ids) = ids {
        if ids.is_empty() {
            return Ok(0);
        }
        // Fixed params occupy ?1..?3, so the id list starts at ?4.
        let placeholders: String = (4..=ids.len() + 3)
            .map(|i| format!("?{i}"))
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
            "UPDATE files SET last_seen = ?1 WHERE tier = ?2 AND peer_device_id = ?3 AND id IN ({placeholders})"
        );
        let tier = FileRecord::TIER_PEER.to_string();
        let device = device_id.to_string();
        let mut params: Vec<&dyn rusqlite::ToSql> = vec![&now, &tier, &device];
        params.extend(ids.iter().map(|id| id as &dyn rusqlite::ToSql));
        return conn.execute(&sql, params.as_slice());
    }
    conn.execute(
        "UPDATE files SET last_seen = ?1 WHERE tier = ?2 AND peer_device_id = ?3",
        params![now, FileRecord::TIER_PEER, device_id],
    )
}

/// Metadata-only replacement for a device's peer row. `INSERT OR REPLACE`
/// keeps re-registration idempotent without ever touching storage.
pub fn replace_peer_file(conn: &Connection, record: &FileRecord) -> Result<()> {
    conn.execute(
        &format!(
            "INSERT OR REPLACE INTO files ({FILE_COLUMNS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25)"
        ),
        params![
            record.id,
            record.filename,
            record.mime_type,
            record.size_bytes,
            record.storage_path,
            record.delete_token,
            record.uploaded_at,
            record.expires_at,
            record.uploader_ip,
            record.storage_host.as_deref().unwrap_or(""),
            record.status,
            record.password_hash.as_deref(),
            i64::from(record.is_encrypted),
            record.enc_header.as_deref(),
            record.dek_wrapped.as_deref(),
            record.dek_salt.as_deref(),
            record.dek_escrow.as_deref(),
            record.key_version,
            record.tier,
            record.r2_key,
            i64::from(record.tier_pinned),
            record.title,
            record.author,
            record.peer_device_id,
            record.last_seen,
        ],
    )?;
    Ok(())
}

pub fn list_abandoned_uploads_batch(
    conn: &Connection,
    older_than_secs: i64,
    after_id: &str,
    limit: usize,
) -> Result<Vec<FileRecord>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {FILE_COLUMNS} FROM files WHERE status = 'uploading' AND uploaded_at < unixepoch() - ?1 AND id > ?2 ORDER BY id LIMIT ?3"
    ))?;
    let rows = stmt.query_map(
        params![older_than_secs, after_id, limit as i64],
        row_to_file_record,
    )?;
    rows.collect()
}

/// Returns the file-row count, not the stats count.
pub fn delete_files_by_ids(conn: &Connection, ids: &[String]) -> Result<usize> {
    if ids.is_empty() {
        return Ok(0);
    }
    let placeholders: String = (1..=ids.len())
        .map(|i| format!("?{i}"))
        .collect::<Vec<_>>()
        .join(", ");
    let sql = format!("DELETE FROM files WHERE id IN ({placeholders})");
    let params: Vec<&dyn rusqlite::ToSql> = ids.iter().map(|s| s as &dyn rusqlite::ToSql).collect();
    let affected = conn.execute(&sql, params.as_slice())?;
    if affected > 0 {
        let stats_sql = format!("DELETE FROM file_stats WHERE file_id IN ({placeholders})");
        conn.execute(&stats_sql, params.as_slice())?;
    }
    Ok(affected)
}

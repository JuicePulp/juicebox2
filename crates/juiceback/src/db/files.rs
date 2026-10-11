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
            "UPDATE files SET filename = ?1, mime_type = ?2, size_bytes = ?3, uploader_ip = ?6, status = 'ready' \
             WHERE id = ?4 AND delete_token = ?5 AND status = 'uploading'",
            params![filename, mime_type, size_bytes, id, delete_token, ip],
        )?
    } else {
        db.execute(
            "UPDATE files SET filename = ?1, mime_type = ?2, size_bytes = ?3, status = 'ready' \
             WHERE id = ?4 AND delete_token = ?5 AND status = 'uploading'",
            params![filename, mime_type, size_bytes, id, delete_token],
        )?
    };
    if affected == 1 {
        return Ok(FinishReservationResult::Completed(Box::new(
            get_file(db, id)?.ok_or(rusqlite::Error::QueryReturnedNoRows)?,
        )));
    }

    let Some(record) = get_file(db, id)? else {
        return Ok(FinishReservationResult::NotFound);
    };
    if record.status != "uploading" {
        return Ok(FinishReservationResult::NotUploading);
    }
    Ok(FinishReservationResult::InvalidToken)
}

pub(crate) const FILE_COLUMNS: &str = "id, filename, mime_type, size_bytes, storage_path, \
    delete_token, uploaded_at, expires_at, uploader_ip, storage_host, status, \
    password_hash, is_encrypted, enc_header, dek_wrapped, dek_salt, dek_escrow, key_version";

pub(crate) fn row_to_file_record(row: &rusqlite::Row) -> rusqlite::Result<FileRecord> {
    let raw_host: String = row.get(9)?;
    let status: String = row.get(10).unwrap_or_else(|_| "ready".to_string());
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
    })
}

pub fn insert_file(conn: &Connection, record: &FileRecord) -> Result<()> {
    conn.execute(
        &format!(
            "INSERT INTO files ({FILE_COLUMNS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18)"
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

pub async fn insert_pending_file(
    state: &std::sync::Arc<crate::state::AppState>,
    mut record: FileRecord,
) -> Result<FileRecord, crate::error::AppError> {
    record.status = "uploading".to_string();
    let r2 = record.clone();
    state
        .db_call("insert_pending_file", move |db| insert_file(db, &r2))
        .await?;
    Ok(record)
}

pub async fn finish_file(
    state: &std::sync::Arc<crate::state::AppState>,
    id: String,
) -> Result<(), crate::error::AppError> {
    state
        .db_call("finish_file", move |db| {
            let affected = db.execute(
                "UPDATE files SET status = 'ready' WHERE id = ?1 AND status = 'uploading'",
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
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let placeholders: String = (1..=ids.len())
        .map(|i| format!("?{i}"))
        .collect::<Vec<_>>()
        .join(", ");
    let mut stmt = conn.prepare(&format!(
        "SELECT {FILE_COLUMNS} FROM files WHERE id IN ({placeholders})"
    ))?;
    let params: Vec<&dyn rusqlite::ToSql> = ids.iter().map(|s| s as &dyn rusqlite::ToSql).collect();
    let rows = stmt.query_map(params.as_slice(), row_to_file_record)?;
    rows.collect()
}

pub fn delete_file(conn: &Connection, id: &str) -> Result<bool> {
    let affected = conn.execute("DELETE FROM files WHERE id = ?1", params![id])?;
    if affected > 0 {
        super::stats::delete_file_stats(conn, id)?;
    }
    Ok(affected > 0)
}

/// Record password-gate and encryption metadata for a completed upload.
///
/// # Errors
///
/// Returns [`rusqlite::Error`] on database failure.
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

pub fn renew_file_id(
    conn: &Connection,
    old_id: &str,
    new_id: &str,
    new_storage_path: &str,
) -> Result<bool> {
    let affected = conn.execute(
        "UPDATE files SET id = ?1, storage_path = ?2 WHERE id = ?3",
        params![new_id, new_storage_path, old_id],
    )?;
    if affected > 0 {
        // Keep accumulated viewers on the new id (old URLs alias to it).
        conn.execute(
            "UPDATE file_stats SET file_id = ?1 WHERE file_id = ?2",
            params![new_id, old_id],
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

pub fn list_expired_batch(
    conn: &Connection,
    after_id: &str,
    limit: usize,
) -> Result<Vec<FileRecord>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {FILE_COLUMNS} FROM files WHERE expires_at < unixepoch() AND id > ?1 ORDER BY id LIMIT ?2"
    ))?;
    let rows = stmt.query_map(params![after_id, limit as i64], row_to_file_record)?;
    rows.collect()
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

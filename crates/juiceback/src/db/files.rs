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
    delete_token, uploaded_at, expires_at, uploader_ip, storage_host, status";

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
    })
}

pub fn insert_file(conn: &Connection, record: &FileRecord) -> Result<()> {
    conn.execute(
        &format!(
            "INSERT INTO files ({FILE_COLUMNS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)"
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

    if let Some(row) = rows.next()? {
        Ok(Some(row_to_file_record(row)?))
    } else {
        Ok(None)
    }
}

/// Look up multiple file records by their IDs.
pub fn get_files_by_ids(conn: &Connection, ids: &[String]) -> Result<Vec<FileRecord>> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let placeholders: String = ids
        .iter()
        .enumerate()
        .map(|(i, _)| format!("?{}", i + 1))
        .collect::<Vec<_>>()
        .join(", ");
    let mut stmt = conn.prepare(&format!(
        "SELECT {FILE_COLUMNS} FROM files WHERE id IN ({placeholders})"
    ))?;
    let params: Vec<&dyn rusqlite::ToSql> = ids.iter().map(|s| s as &dyn rusqlite::ToSql).collect();
    let rows = stmt.query_map(params.as_slice(), row_to_file_record)?;
    rows.collect()
}

/// Delete a file record by ID and return true if anything was actually removed
pub fn delete_file(conn: &Connection, id: &str) -> Result<bool> {
    let affected = conn.execute("DELETE FROM files WHERE id = ?1", params![id])?;
    Ok(affected > 0)
}

/// List every file record, newest first.
pub fn list_all_files(conn: &Connection) -> Result<Vec<FileRecord>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {FILE_COLUMNS} FROM files ORDER BY uploaded_at DESC"
    ))?;
    let rows = stmt.query_map([], row_to_file_record)?;
    rows.collect()
}

/// Give a file a new ID and URL while keeping the `delete_token` filename and
/// expiry the same
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
    Ok(affected > 0)
}

/// Record that `old_id` now redirects to `new_id`.
pub fn insert_alias(conn: &Connection, old_id: &str, new_id: &str) -> Result<()> {
    conn.execute(
        "INSERT OR REPLACE INTO aliases (old_id, new_id) VALUES (?1, ?2)",
        params![old_id, new_id],
    )?;
    Ok(())
}

/// Remove a file alias by its `old_id`.
pub fn delete_alias(conn: &Connection, old_id: &str) -> Result<bool> {
    let affected = conn.execute("DELETE FROM aliases WHERE old_id = ?1", params![old_id])?;
    Ok(affected > 0)
}

/// Resolve a previous file ID to its current ID.
///
/// Follows alias chains up to 10 hops to avoid infinite loops.
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
            // Chain end (or self-loop): only an alias if we moved. Callers
            // that want identity fallback apply `.unwrap_or(id)` themselves.
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

/// One bounded page of expired files for the cleanup loop. Keyset on `id`
/// (rather than OFFSET) so concurrent deletes can't shift rows between pages;
/// rows whose juicehost delete fails stay for the next cycle because the
/// cursor advances past them. An empty result (or a short page) ends the
/// sweep.
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

/// Bounded page of quick link uploads that were reserved but never completed
/// so we know they're safe to delete. Same keyset contract as
/// `list_expired_batch`; served by `idx_files_status_uploaded`.
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

/// Bulk-delete file records by their IDs and return the number of removed rows.
pub fn delete_files_by_ids(conn: &Connection, ids: &[String]) -> Result<usize> {
    if ids.is_empty() {
        return Ok(0);
    }
    let placeholders: String = ids
        .iter()
        .enumerate()
        .map(|(i, _)| format!("?{}", i + 1))
        .collect::<Vec<_>>()
        .join(", ");
    let sql = format!("DELETE FROM files WHERE id IN ({placeholders})");
    let params: Vec<&dyn rusqlite::ToSql> = ids.iter().map(|s| s as &dyn rusqlite::ToSql).collect();
    let affected = conn.execute(&sql, params.as_slice())?;
    Ok(affected)
}

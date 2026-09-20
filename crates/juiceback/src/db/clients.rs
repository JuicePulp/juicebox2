use rusqlite::{Connection, OptionalExtension, Result, Transaction, params};

use super::types::{ClientFileRecord, FileRecord};

pub fn device_belongs_to_user(conn: &Connection, device_id: &str, user_id: &str) -> Result<bool> {
    Ok(conn
        .query_row(
            "SELECT 1 FROM devices WHERE id = ?1 AND user_id = ?2",
            params![device_id, user_id],
            |_| Ok(()),
        )
        .optional()?
        .is_some())
}

pub fn client_owns_file(conn: &Connection, user_id: &str, file_id: &str) -> Result<bool> {
    Ok(conn
        .query_row(
            "SELECT 1 FROM client_files WHERE client_key = ?1 AND file_id = ?2",
            params![user_id, file_id],
            |_| Ok(()),
        )
        .optional()?
        .is_some())
}

pub fn add_client_file(conn: &Connection, user_id: &str, record: &FileRecord) -> Result<()> {
    conn.execute(
        "INSERT INTO client_files
         (client_key, file_id, filename, mime_type, size_bytes, uploaded_at, expires_at, delete_token)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
         ON CONFLICT(client_key, file_id) DO UPDATE SET
           filename = excluded.filename,
           mime_type = excluded.mime_type,
           size_bytes = excluded.size_bytes,
           uploaded_at = excluded.uploaded_at,
           expires_at = excluded.expires_at",
        params![
            user_id,
            record.id,
            record.filename,
            record.mime_type,
            record.size_bytes,
            record.uploaded_at,
            record.expires_at,
            record.delete_token
        ],
    )?;
    Ok(())
}

pub fn remove_client_file(conn: &Connection, user_id: &str, file_id: &str) -> Result<()> {
    conn.execute(
        "DELETE FROM client_files WHERE client_key = ?1 AND file_id = ?2",
        params![user_id, file_id],
    )?;
    Ok(())
}

pub fn import_verified_client_files(
    conn: &Connection,
    user_id: &str,
    records: &[ClientFileRecord],
) -> Result<usize> {
    let tx = conn.unchecked_transaction()?;
    let mut imported = 0;
    for record in records {
        let verified: bool = tx
            .query_row(
                "SELECT 1 FROM files WHERE id = ?1 AND storage_host = '' AND delete_token = ?2",
                params![record.id, record.token],
                |_| Ok(true),
            )
            .optional()?
            .unwrap_or(false);
        if verified {
            imported += tx.execute(
                "INSERT OR IGNORE INTO client_files
                 (client_key, file_id, filename, mime_type, size_bytes, uploaded_at, expires_at, delete_token)
                 SELECT ?1, id, filename, mime_type, size_bytes, uploaded_at, expires_at, delete_token
                 FROM files WHERE id = ?2",
                params![user_id, record.id],
            )?;
        }
    }
    tx.commit()?;
    Ok(imported)
}

pub fn replace_client_files(
    conn: &Connection,
    client_key: &str,
    records: &[ClientFileRecord],
) -> Result<usize> {
    let tx = conn.unchecked_transaction()?;
    tx.execute(
        "DELETE FROM client_files WHERE client_key = ?1",
        params![client_key],
    )?;
    for r in records {
        tx.execute(
            "INSERT OR REPLACE INTO client_files
             (client_key, file_id, filename, mime_type, size_bytes, uploaded_at, expires_at, delete_token)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![client_key, r.id, r.n, r.m, r.s, r.u, r.e, r.token],
        )?;
    }
    tx.commit()?;
    Ok(records.len())
}

/// Fetch a client's stored upload records, newest first.
pub fn list_client_files(conn: &Connection, client_key: &str) -> Result<Vec<ClientFileRecord>> {
    let mut stmt = conn.prepare(
        "SELECT file_id, delete_token, filename, mime_type, size_bytes, uploaded_at, expires_at
         FROM client_files WHERE client_key = ?1 ORDER BY uploaded_at DESC",
    )?;
    let rows = stmt.query_map(params![client_key], |row| {
        Ok(ClientFileRecord {
            id: row.get(0)?,
            token: row.get(1)?,
            n: row.get(2)?,
            m: row.get(3)?,
            s: row.get(4)?,
            u: row.get(5)?,
            e: row.get(6)?,
        })
    })?;
    rows.collect()
}

/// Swap a client's registered file ID after a server-side rename.
///
/// Renames keep the delete token and metadata, so only `file_id` changes. Any
/// row already under `new_id` is removed first so the primary key can't clash.
/// Returns true when a row was actually renamed.
pub fn update_client_file_id(
    conn: &Connection,
    client_key: &str,
    old_id: &str,
    new_id: &str,
) -> Result<bool> {
    let tx = conn.unchecked_transaction()?;
    let renamed = update_client_file_id_in_tx(&tx, client_key, old_id, new_id)?;
    tx.commit()?;
    Ok(renamed)
}

/// Core of [`update_client_file_id`] that joins an existing transaction
/// instead of opening its own (SQLite has no nested `BEGIN`).
pub fn update_client_file_id_in_tx(
    tx: &Transaction,
    client_key: &str,
    old_id: &str,
    new_id: &str,
) -> Result<bool> {
    tx.execute(
        "DELETE FROM client_files WHERE client_key = ?1 AND file_id = ?2",
        params![client_key, new_id],
    )?;
    let affected = tx.execute(
        "UPDATE client_files SET file_id = ?1 WHERE client_key = ?2 AND file_id = ?3",
        params![new_id, client_key, old_id],
    )?;
    Ok(affected > 0)
}

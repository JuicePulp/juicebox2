use rusqlite::{Connection, Result, params};

use super::types::ReportRecord;

pub fn insert_report(
    conn: &Connection,
    file_url: &str,
    reason: &str,
    details: &str,
    reporter_ip: Option<&str>,
    email: Option<&str>,
    password: Option<&str>,
) -> Result<i64> {
    conn.execute(
        "INSERT INTO reports (file_url, reason, details, reporter_ip, email, password) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![file_url, reason, details, reporter_ip, email, password],
    )?;
    Ok(conn.last_insert_rowid())
}

pub fn list_reports(conn: &Connection) -> Result<Vec<ReportRecord>> {
    let mut stmt = conn.prepare(
        "SELECT id, file_url, reason, details, reporter_ip, email, created_at, password FROM reports ORDER BY created_at DESC",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok(ReportRecord {
            id: row.get(0)?,
            file_url: row.get(1)?,
            reason: row.get(2)?,
            details: row.get(3)?,
            reporter_ip: row.get(4)?,
            email: row.get(5)?,
            created_at: row.get(6)?,
            password: row.get(7)?,
        })
    })?;
    rows.collect()
}

pub fn delete_report(conn: &Connection, id: i64) -> Result<bool> {
    let affected = conn.execute("DELETE FROM reports WHERE id = ?1", params![id])?;
    Ok(affected > 0)
}

/// Fetch one report row by id.
///
/// # Errors
///
/// Returns [`rusqlite::Error`] on database failure.
pub fn get_report(conn: &Connection, id: i64) -> Result<Option<ReportRecord>> {
    let mut stmt = conn.prepare(
        "SELECT id, file_url, reason, details, reporter_ip, email, created_at, password FROM reports WHERE id = ?1",
    )?;
    let mut rows = stmt.query_map(params![id], |row| {
        Ok(ReportRecord {
            id: row.get(0)?,
            file_url: row.get(1)?,
            reason: row.get(2)?,
            details: row.get(3)?,
            reporter_ip: row.get(4)?,
            email: row.get(5)?,
            created_at: row.get(6)?,
            password: row.get(7)?,
        })
    })?;
    rows.next().transpose()
}

/// File id from a `/f/<id>[.<ext>]` URL path, if present.
#[must_use]
pub fn file_id_from_url(file_url: &str) -> Option<String> {
    let path = file_url.split('?').next()?;
    let last = path.trim_end_matches('/').rsplit('/').next()?;
    let id = last.split('.').next().unwrap_or("");
    if crate::utils::is_valid_id(id) {
        Some(id.to_string())
    } else {
        None
    }
}

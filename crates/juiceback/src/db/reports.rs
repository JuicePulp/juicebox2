use rusqlite::{Connection, Result, params};

use super::types::ReportRecord;

pub fn insert_report(
    conn: &Connection,
    file_url: &str,
    reason: &str,
    details: &str,
    reporter_ip: Option<&str>,
    email: Option<&str>,
) -> Result<i64> {
    conn.execute(
        "INSERT INTO reports (file_url, reason, details, reporter_ip, email) VALUES (?1, ?2, ?3, ?4, ?5)",
        params![file_url, reason, details, reporter_ip, email],
    )?;
    Ok(conn.last_insert_rowid())
}

/// List all reports, newest first.
pub fn list_reports(conn: &Connection) -> Result<Vec<ReportRecord>> {
    let mut stmt = conn.prepare(
        "SELECT id, file_url, reason, details, reporter_ip, email, created_at FROM reports ORDER BY created_at DESC",
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
        })
    })?;
    rows.collect()
}

/// Delete a report by ID and return true if anything was actually removed
pub fn delete_report(conn: &Connection, id: i64) -> Result<bool> {
    let affected = conn.execute("DELETE FROM reports WHERE id = ?1", params![id])?;
    Ok(affected > 0)
}

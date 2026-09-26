use rusqlite::{Connection, Result, params};

use super::types::FeedbackRecord;

pub fn insert_feedback(
    conn: &Connection,
    message: &str,
    email: Option<&str>,
    reporter_ip: Option<&str>,
) -> Result<i64> {
    conn.execute(
        "INSERT INTO feedback (message, email, reporter_ip) VALUES (?1, ?2, ?3)",
        params![message, email, reporter_ip],
    )?;
    Ok(conn.last_insert_rowid())
}

pub fn list_feedback(conn: &Connection) -> Result<Vec<FeedbackRecord>> {
    let mut stmt = conn.prepare(
        "SELECT id, message, email, reporter_ip, created_at FROM feedback ORDER BY created_at DESC",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok(FeedbackRecord {
            id: row.get(0)?,
            message: row.get(1)?,
            email: row.get(2)?,
            reporter_ip: row.get(3)?,
            created_at: row.get(4)?,
        })
    })?;
    rows.collect()
}

pub fn delete_feedback(conn: &Connection, id: i64) -> Result<bool> {
    let affected = conn.execute("DELETE FROM feedback WHERE id = ?1", params![id])?;
    Ok(affected > 0)
}

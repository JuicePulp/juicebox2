use rusqlite::{Connection, Result, params};

use super::types::FetchJob;

fn row_to_fetch_job(row: &rusqlite::Row) -> rusqlite::Result<FetchJob> {
    Ok(FetchJob {
        id: row.get(0)?,
        user_id: row.get(1)?,
        source_url: row.get(2)?,
        status: row.get(3)?,
        error: row.get(4)?,
        file_id: row.get(5)?,
        created_at: row.get(6)?,
        updated_at: row.get(7)?,
        stage: row.get(8)?,
        bytes_received: row.get(9)?,
    })
}

pub fn insert_fetch_job(
    conn: &Connection,
    id: &str,
    user_id: &str,
    source_url: &str,
) -> Result<()> {
    conn.execute(
        "INSERT INTO fetch_jobs (id, user_id, source_url) VALUES (?1, ?2, ?3)",
        params![id, user_id, source_url],
    )?;
    Ok(())
}

pub fn get_fetch_job(conn: &Connection, id: &str) -> Result<Option<FetchJob>> {
    let mut stmt = conn.prepare(
        "SELECT id, user_id, source_url, status, error, file_id, created_at, updated_at, stage, bytes_received FROM fetch_jobs WHERE id = ?1",
    )?;
    let mut rows = stmt.query(params![id])?;
    match rows.next()? {
        Some(row) => Ok(Some(row_to_fetch_job(row)?)),
        None => Ok(None),
    }
}

pub fn finish_fetch_job(
    conn: &Connection,
    id: &str,
    status: &str,
    error: &str,
    file_id: &str,
) -> Result<bool> {
    let affected = conn.execute(
        "UPDATE fetch_jobs SET status = ?2, error = ?3, file_id = ?4, updated_at = unixepoch() \
         WHERE id = ?1 AND status IN ('pending', 'processing', 'downloading')",
        params![id, status, error, file_id],
    )?;
    Ok(affected == 1)
}

/// Record live progress for a running fetch job. Best-effort by design:
/// callers ignore the result, since losing a progress tick must never fail
/// the transfer itself.
pub fn update_fetch_job_progress(
    conn: &Connection,
    id: &str,
    stage: &str,
    status: &str,
    bytes_received: i64,
) -> Result<()> {
    conn.execute(
        "UPDATE fetch_jobs SET stage = ?2, status = ?3, bytes_received = ?4, \
         updated_at = unixepoch() \
         WHERE id = ?1 AND status IN ('pending', 'processing', 'downloading')",
        params![id, stage, status, bytes_received],
    )?;
    Ok(())
}

pub fn cleanup_fetch_jobs(conn: &Connection, now: i64) -> Result<usize> {
    let mut deleted = conn.execute(
        "DELETE FROM fetch_jobs WHERE status != 'pending' AND updated_at <= ?1",
        params![now - crate::constants::FETCH_JOB_RETENTION_SECS],
    )?;
    deleted += conn.execute(
        "UPDATE fetch_jobs SET status = 'failed', error = 'fetch job timed out', \
         updated_at = unixepoch() WHERE status = 'pending' AND updated_at <= ?1",
        params![now - crate::constants::STALE_FETCH_JOB_TIMEOUT_SECS],
    )?;
    Ok(deleted)
}

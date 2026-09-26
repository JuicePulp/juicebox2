use rusqlite::{Connection, OptionalExtension, Result, params};

use super::fetch_jobs::cleanup_fetch_jobs;

pub fn resolve_session(
    conn: &Connection,
    token_hash: &str,
    now: i64,
) -> Result<Option<(String, i64)>> {
    let row: Option<(String, i64)> = conn
        .query_row(
            "SELECT user_id, last_seen_at FROM sessions WHERE token_hash = ?1 AND expires_at > ?2",
            params![token_hash, now],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    let Some((user_id, last_seen)) = row else {
        return Ok(None);
    };

    if now - last_seen >= crate::constants::SESSION_TOUCH_INTERVAL_SECS {
        conn.execute(
            "UPDATE sessions SET last_seen_at = ?2 WHERE token_hash = ?1",
            params![token_hash, now],
        )?;
        Ok(Some((user_id, now)))
    } else {
        Ok(Some((user_id, last_seen)))
    }
}

pub fn create_session(
    conn: &Connection,
    token_hash: &str,
    user_id: &str,
    now: i64,
    expires_at: i64,
) -> Result<()> {
    conn.execute(
        "INSERT INTO sessions (token_hash, user_id, created_at, expires_at, last_seen_at)
         VALUES (?1, ?2, ?3, ?4, ?3)",
        params![token_hash, user_id, now, expires_at],
    )?;
    Ok(())
}

pub fn cleanup_ephemeral_data(
    conn: &Connection,
    now: i64,
    report_cutoff: i64,
    feedback_cutoff: i64,
) -> Result<usize> {
    let tx = conn.unchecked_transaction()?;
    let mut deleted = tx.execute("DELETE FROM sessions WHERE expires_at <= ?1", params![now])?;
    deleted += tx.execute(
        "DELETE FROM pairing_codes WHERE used != 0 OR expires_at <= ?1",
        params![now],
    )?;
    deleted += tx.execute(
        "DELETE FROM reports WHERE created_at < ?1",
        params![report_cutoff],
    )?;
    deleted += tx.execute(
        "DELETE FROM feedback WHERE created_at < ?1",
        params![feedback_cutoff],
    )?;
    deleted += tx.execute(
        "DELETE FROM failed_logins WHERE attempt_at < ?1",
        params![now - crate::constants::SECONDS_PER_DAY],
    )?;
    match cleanup_fetch_jobs(&tx, now) {
        Ok(n) => deleted += n,
        Err(e) => tracing::warn!("fetch_jobs cleanup failed: {e}"),
    }
    tx.commit()?;
    Ok(deleted)
}

//! Sessions keyed by a hash of the bearer token; the token is never stored.

use rusqlite::{Connection, OptionalExtension, Result, params};

/// Touches `last_seen_at` at most once per `SESSION_TOUCH_INTERVAL_SECS`;
/// expired or unknown tokens yield `None`.
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

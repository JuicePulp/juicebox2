use std::collections::HashMap;

use rusqlite::{Connection, Result, params};

#[derive(Debug, Clone)]
pub struct PromoCandidate {
    pub id: String,
    pub storage_host: String,
    pub size_bytes: i64,
    pub filename: String,
}

pub fn list_promo_candidates(
    conn: &Connection,
    min_expires_at: i64,
    limit: i64,
) -> Result<Vec<PromoCandidate>> {
    // Manually Warm/Cool pins the tier (tier_pinned=1): the auto-promoter
    // skips pinned files so a manual choice stays until the next manual change.
    let mut stmt = conn.prepare(
        "SELECT id, storage_host, size_bytes, filename FROM files
         WHERE status = 'ready' AND tier = 'standard'
           AND COALESCE(tier_pinned, 0) = 0 AND expires_at > ?1
         ORDER BY uploaded_at DESC LIMIT ?2",
    )?;
    let rows = stmt.query_map(params![min_expires_at, limit], |row| {
        Ok(PromoCandidate {
            id: row.get(0)?,
            storage_host: row.get(1)?,
            size_bytes: row.get(2)?,
            filename: row.get(3)?,
        })
    })?;
    rows.collect()
}

pub fn read_promo_state(
    conn: &Connection,
    ids: &[String],
) -> Result<HashMap<String, (i64, i64, i64)>> {
    let mut out = HashMap::new();
    if ids.is_empty() {
        return Ok(out);
    }
    let placeholders: String = (1..=ids.len())
        .map(|i| format!("?{i}"))
        .collect::<Vec<_>>()
        .join(", ");
    let sql = format!(
        "SELECT file_id, views_seen, downloads_seen, checked_at FROM promo_state WHERE file_id IN ({placeholders})"
    );
    let params: Vec<&dyn rusqlite::ToSql> = ids.iter().map(|s| s as &dyn rusqlite::ToSql).collect();
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params.as_slice(), |row| {
        Ok((
            row.get::<_, String>(0)?,
            (
                row.get::<_, i64>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, i64>(3)?,
            ),
        ))
    })?;
    for row in rows {
        let (id, state) = row?;
        out.insert(id, state);
    }
    Ok(out)
}

pub fn upsert_promo_state(conn: &Connection, rows: &[(String, i64, i64, i64)]) -> Result<()> {
    for (id, views, downloads, checked_at) in rows {
        conn.execute(
            "INSERT INTO promo_state (file_id, views_seen, downloads_seen, checked_at)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(file_id) DO UPDATE SET
                 views_seen = excluded.views_seen,
                 downloads_seen = excluded.downloads_seen,
                 checked_at = excluded.checked_at",
            params![id, views, downloads, checked_at],
        )?;
    }
    Ok(())
}

pub fn prune_promo_state(conn: &Connection) -> Result<usize> {
    Ok(conn.execute(
        "DELETE FROM promo_state WHERE file_id NOT IN (SELECT id FROM files)",
        [],
    )? as usize)
}

/// Embed-promote cooldown: true when no attempt was recorded within
/// `cooldown_secs`. Missing column (pre-m16 DBs) means due.
pub fn embed_attempt_due(
    conn: &Connection,
    file_id: &str,
    now: i64,
    cooldown_secs: i64,
) -> Result<bool> {
    let last: Option<i64> = conn
        .query_row(
            "SELECT embed_attempt_at FROM promo_state WHERE file_id = ?1",
            params![file_id],
            |row| row.get(0),
        )
        .ok();
    match last {
        None => Ok(true),
        Some(ts) => Ok(now.saturating_sub(ts) >= cooldown_secs.max(0)),
    }
}

/// Record an embed-promote attempt (creates the row when absent, preserving
/// velocity counters otherwise managed by the sweeper).
pub fn record_embed_attempt(conn: &Connection, file_id: &str, now: i64) -> Result<()> {
    conn.execute(
        "INSERT INTO promo_state (file_id, views_seen, downloads_seen, checked_at, embed_attempt_at)
         VALUES (?1, 0, 0, ?2, ?2)
         ON CONFLICT(file_id) DO UPDATE SET embed_attempt_at = excluded.embed_attempt_at",
        params![file_id, now],
    )?;
    Ok(())
}

pub fn set_file_tier(conn: &Connection, id: &str, tier: &str, r2_key: &str) -> Result<usize> {
    Ok(conn.execute(
        "UPDATE files SET tier = ?1, r2_key = ?2 WHERE id = ?3",
        params![tier, r2_key, id],
    )? as usize)
}

/// Manual Warm/Cool: set the tier and pin it so the auto-promoter leaves it
/// alone until the next manual change.
pub fn set_file_tier_manual(
    conn: &Connection,
    id: &str,
    tier: &str,
    r2_key: &str,
) -> Result<usize> {
    Ok(conn.execute(
        "UPDATE files SET tier = ?1, r2_key = ?2, tier_pinned = 1 WHERE id = ?3",
        params![tier, r2_key, id],
    )? as usize)
}

pub fn hot_usage(conn: &Connection) -> Result<(i64, i64)> {
    conn.query_row(
        "SELECT COUNT(*), COALESCE(SUM(size_bytes), 0) FROM files WHERE tier = 'hot'",
        [],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )
}

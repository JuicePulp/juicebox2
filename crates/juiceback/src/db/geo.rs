//! IP geolocation cache + resolver queue.
//!
//! Raw IPs are never stored here: rows are keyed by ban-hash, exactly like
//! the rest of the codebase. A background worker resolves unknown hashes
//! through one rate-limited batch call per minute (ip-api.com allows 45).

use std::collections::{HashMap, HashSet};

use rusqlite::{Connection, Result, params};

pub struct GeoLite {
    pub country: String,
    pub region: String,
    pub city: String,
}

pub fn enqueue_geo_ip(conn: &Connection, enc_ip: &str, ip_hash: &str, now: i64) -> Result<()> {
    conn.execute(
        "INSERT OR IGNORE INTO geo_queue(enc_ip, ip_hash, attempts, enqueued_at)
         VALUES (?1, ?2, 0, ?3)",
        params![enc_ip, ip_hash, now],
    )?;
    Ok(())
}

pub struct QueuedIp {
    pub id: i64,
    pub enc_ip: String,
    pub ip_hash: String,
    pub attempts: i64,
}

pub fn dequeue_geo_batch(conn: &Connection, limit: i64) -> Result<Vec<QueuedIp>> {
    let mut stmt = conn.prepare(
        "SELECT id, enc_ip, ip_hash, attempts FROM geo_queue
         ORDER BY enqueued_at ASC, id ASC LIMIT ?1",
    )?;
    let rows = stmt.query_map(params![limit], |row| {
        Ok(QueuedIp {
            id: row.get(0)?,
            enc_ip: row.get(1)?,
            ip_hash: row.get(2)?,
            attempts: row.get(3)?,
        })
    })?;
    rows.collect()
}

pub fn delete_geo_queue(conn: &Connection, ids: &[i64]) -> Result<()> {
    if ids.is_empty() {
        return Ok(());
    }
    let placeholders: String = (1..=ids.len())
        .map(|i| format!("?{i}"))
        .collect::<Vec<_>>()
        .join(", ");
    let sql = format!("DELETE FROM geo_queue WHERE id IN ({placeholders})");
    let params: Vec<&dyn rusqlite::ToSql> = ids.iter().map(|i| i as &dyn rusqlite::ToSql).collect();
    conn.execute(&sql, params.as_slice())?;
    Ok(())
}

/// Failures stay queued for retry; poison rows die after 10 attempts.
pub fn bump_geo_attempts(conn: &Connection, ids: &[i64]) -> Result<()> {
    if ids.is_empty() {
        return Ok(());
    }
    let placeholders: String = (1..=ids.len())
        .map(|i| format!("?{i}"))
        .collect::<Vec<_>>()
        .join(", ");
    let sql = format!("UPDATE geo_queue SET attempts = attempts + 1 WHERE id IN ({placeholders})");
    let params: Vec<&dyn rusqlite::ToSql> = ids.iter().map(|i| i as &dyn rusqlite::ToSql).collect();
    conn.execute(&sql, params.as_slice())?;
    conn.execute("DELETE FROM geo_queue WHERE attempts >= 10", [])?;
    Ok(())
}

pub fn upsert_ip_location(
    conn: &Connection,
    ip_hash: &str,
    country: &str,
    region: &str,
    city: &str,
    lat: f64,
    lon: f64,
    now: i64,
) -> Result<()> {
    conn.execute(
        "INSERT INTO ip_location(ip_hash, country, region, city, lat, lon, resolved_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
         ON CONFLICT(ip_hash) DO UPDATE SET
           country = excluded.country, region = excluded.region, city = excluded.city,
           lat = excluded.lat, lon = excluded.lon, resolved_at = excluded.resolved_at",
        params![ip_hash, country, region, city, lat, lon, now],
    )?;
    Ok(())
}

pub fn ip_locations_for_hashes(
    conn: &Connection,
    hashes: &[String],
) -> Result<HashMap<String, GeoLite>> {
    let mut out = HashMap::new();
    if hashes.is_empty() {
        return Ok(out);
    }
    let placeholders: String = (1..=hashes.len())
        .map(|i| format!("?{i}"))
        .collect::<Vec<_>>()
        .join(", ");
    let sql = format!(
        "SELECT ip_hash, country, region, city FROM ip_location WHERE ip_hash IN ({placeholders})"
    );
    let params: Vec<&dyn rusqlite::ToSql> =
        hashes.iter().map(|h| h as &dyn rusqlite::ToSql).collect();
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params.as_slice(), |row| {
        Ok((
            row.get::<_, String>(0)?,
            GeoLite {
                country: row.get(1)?,
                region: row.get(2)?,
                city: row.get(3)?,
            },
        ))
    })?;
    for row in rows {
        let (hash, geo) = row?;
        out.insert(hash, geo);
    }
    Ok(out)
}

/// Distinct uploader IPs (still encrypted) for the backfill pass.
pub fn uploader_ips_sample(conn: &Connection, limit: i64) -> Result<Vec<String>> {
    let mut stmt = conn.prepare(
        "SELECT DISTINCT uploader_ip FROM files
         WHERE uploader_ip IS NOT NULL AND uploader_ip != ''
         ORDER BY uploaded_at DESC LIMIT ?1",
    )?;
    let rows = stmt.query_map(params![limit], |row| row.get::<_, String>(0))?;
    rows.collect()
}

pub fn known_geo_hashes(conn: &Connection, hashes: &[String]) -> Result<HashSet<String>> {
    let mut out = HashSet::new();
    if hashes.is_empty() {
        return Ok(out);
    }
    let placeholders: String = (1..=hashes.len())
        .map(|i| format!("?{i}"))
        .collect::<Vec<_>>()
        .join(", ");
    let sql = format!("SELECT ip_hash FROM ip_location WHERE ip_hash IN ({placeholders})");
    let params: Vec<&dyn rusqlite::ToSql> =
        hashes.iter().map(|h| h as &dyn rusqlite::ToSql).collect();
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params.as_slice(), |row| row.get::<_, String>(0))?;
    for row in rows {
        out.insert(row?);
    }
    Ok(out)
}

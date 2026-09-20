use rusqlite::{Connection, Result, params};

use super::types::{BanRecord, ImportBan};

pub fn get_ban_record(conn: &Connection, ip: &str) -> Result<Option<BanRecord>> {
    let mut stmt =
        conn.prepare("SELECT ip, reason, banned_by, banned_at FROM banned_ips WHERE ip = ?1")?;
    let mut rows = stmt.query(params![ip])?;
    if let Some(row) = rows.next()? {
        Ok(Some(BanRecord {
            ip: row.get(0)?,
            reason: row.get(1)?,
            banned_by: row.get(2)?,
            banned_at: row.get(3)?,
        }))
    } else {
        Ok(None)
    }
}

/// Ban an IP address (no shit sherlock)
pub fn ban_ip(conn: &Connection, ip: &str, reason: &str, banned_by: &str) -> Result<()> {
    conn.execute(
        "INSERT OR REPLACE INTO banned_ips (ip, reason, banned_by) VALUES (?1, ?2, ?3)",
        params![ip, reason, banned_by],
    )?;
    Ok(())
}

/// Unban an IP address and return true if anything was actually removed
pub fn unban_ip(conn: &Connection, ip: &str) -> Result<bool> {
    let affected = conn.execute("DELETE FROM banned_ips WHERE ip = ?1", params![ip])?;
    Ok(affected > 0)
}

pub fn insert_banned_ips(conn: &Connection, bans: &[ImportBan]) -> Result<usize> {
    let tx = conn.unchecked_transaction()?;
    for b in bans {
        tx.execute(
            "INSERT OR REPLACE INTO banned_ips (ip, reason, banned_by) VALUES (?1, ?2, ?3)",
            params![b.ip, b.reason, b.banned_by],
        )?;
    }
    tx.commit()?;
    Ok(bans.len())
}

/// List all banned IPs
pub fn list_banned_ips(conn: &Connection) -> Result<Vec<BanRecord>> {
    let mut stmt = conn.prepare(
        "SELECT ip, reason, banned_by, banned_at FROM banned_ips ORDER BY banned_at DESC",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok(BanRecord {
            ip: row.get(0)?,
            reason: row.get(1)?,
            banned_by: row.get(2)?,
            banned_at: row.get(3)?,
        })
    })?;
    rows.collect()
}

/// Load all banned IPs into a HashSet cuz it's fast as shi
pub fn load_banned_ips_set(conn: &Connection) -> Result<std::collections::HashSet<String>> {
    let mut stmt = conn.prepare("SELECT ip FROM banned_ips")?;
    let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
    let mut set = std::collections::HashSet::new();
    for row in rows {
        set.insert(row?);
    }
    Ok(set)
}

use rusqlite::{Connection, Result, params};

use super::types::AdminUser;

pub fn insert_failed_login(conn: &Connection, username: &str) -> Result<()> {
    conn.execute(
        "INSERT INTO failed_logins (username, attempt_at) VALUES (?1, unixepoch())",
        params![username],
    )?;
    Ok(())
}

pub fn insert_failed_login_for_ip(conn: &Connection, username: &str, ip_hash: &str) -> Result<()> {
    conn.execute(
        "INSERT INTO failed_logins (username, ip_hash, attempt_at) VALUES (?1, ?2, unixepoch())",
        params![username, ip_hash],
    )?;
    Ok(())
}

/// Count failed login attempts within the last `window_secs` seconds.
pub fn count_failed_logins(conn: &Connection, username: &str, window_secs: i64) -> Result<u32> {
    let count: u32 = conn.query_row(
        "SELECT COUNT(*) FROM failed_logins WHERE username = ?1 AND attempt_at > unixepoch() - ?2",
        params![username, window_secs],
        |row| row.get(0),
    )?;
    Ok(count)
}

pub fn count_failed_logins_for_ip(
    conn: &Connection,
    username: &str,
    ip_hash: &str,
    window_secs: i64,
) -> Result<u32> {
    conn.query_row(
        "SELECT COUNT(*) FROM failed_logins WHERE username = ?1 AND ip_hash = ?2 AND attempt_at > unixepoch() - ?3",
        params![username, ip_hash, window_secs],
        |row| row.get(0),
    )
}

/// Clear failed login attempts for the given username.
pub fn delete_failed_logins(conn: &Connection, username: &str) -> Result<()> {
    conn.execute(
        "DELETE FROM failed_logins WHERE username = ?1",
        params![username],
    )?;
    Ok(())
}

pub fn delete_failed_logins_for_ip(conn: &Connection, username: &str, ip_hash: &str) -> Result<()> {
    conn.execute(
        "DELETE FROM failed_logins WHERE username = ?1 AND ip_hash = ?2",
        params![username, ip_hash],
    )?;
    Ok(())
}

/// Look up an admin by username.
pub fn get_admin_by_username(conn: &Connection, username: &str) -> Result<Option<AdminUser>> {
    let mut stmt = conn.prepare(
        "SELECT id, username, password_hash, created_at FROM admins WHERE username = ?1",
    )?;
    let mut rows = stmt.query(params![username])?;
    if let Some(row) = rows.next()? {
        Ok(Some(AdminUser {
            id: row.get(0)?,
            username: row.get(1)?,
            password_hash: row.get(2)?,
            created_at: row.get(3)?,
        }))
    } else {
        Ok(None)
    }
}

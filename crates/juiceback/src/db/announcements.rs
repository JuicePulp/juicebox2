use rusqlite::{Connection, Result, params};

use super::types::Announcement;

fn row_to_announcement(row: &rusqlite::Row) -> rusqlite::Result<Announcement> {
    Ok(Announcement {
        id: row.get(0)?,
        message: row.get(1)?,
        link_url: row.get(2)?,
        mode: row.get(3)?,
        is_active: row.get::<_, i64>(4)? != 0,
        created_at: row.get(5)?,
        updated_at: row.get(6)?,
    })
}

pub fn get_active_announcement(conn: &Connection) -> Result<Option<Announcement>> {
    let mut stmt = conn.prepare(
        "SELECT id, message, link_url, mode, is_active, created_at, updated_at \
         FROM announcements WHERE is_active = 1 LIMIT 1",
    )?;
    let mut rows = stmt.query([])?;
    let Some(row) = rows.next()? else {
        return Ok(None);
    };
    Ok(Some(row_to_announcement(row)?))
}

pub fn get_latest_announcement(conn: &Connection) -> Result<Option<Announcement>> {
    let mut stmt = conn.prepare(
        "SELECT id, message, link_url, mode, is_active, created_at, updated_at \
         FROM announcements ORDER BY is_active DESC, updated_at DESC LIMIT 1",
    )?;
    let mut rows = stmt.query([])?;
    let Some(row) = rows.next()? else {
        return Ok(None);
    };
    Ok(Some(row_to_announcement(row)?))
}

pub fn upsert_announcement(
    conn: &Connection,
    message: &str,
    link_url: &str,
    mode: &str,
    is_active: bool,
) -> Result<Announcement> {
    conn.execute("UPDATE announcements SET is_active = 0", [])?;

    let existing: Option<i64> = {
        let mut stmt = conn.prepare("SELECT id FROM announcements LIMIT 1")?;
        let mut rows = stmt.query([])?;
        rows.next()?.map(|r| r.get(0)).transpose()?
    };

    if let Some(id) = existing {
        conn.execute(
            "UPDATE announcements SET message = ?1, link_url = ?2, mode = ?3, is_active = ?4, updated_at = unixepoch() WHERE id = ?5",
            params![message, link_url, mode, is_active as i64, id],
        )?;
        let row = conn.query_row(
            "SELECT id, message, link_url, mode, is_active, created_at, updated_at FROM announcements WHERE id = ?1",
            params![id],
            row_to_announcement,
        )?;
        Ok(row)
    } else {
        conn.execute(
            "INSERT INTO announcements (message, link_url, mode, is_active) VALUES (?1, ?2, ?3, ?4)",
            params![message, link_url, mode, is_active as i64],
        )?;
        let id = conn.last_insert_rowid();
        let row = conn.query_row(
            "SELECT id, message, link_url, mode, is_active, created_at, updated_at FROM announcements WHERE id = ?1",
            params![id],
            row_to_announcement,
        )?;
        Ok(row)
    }
}

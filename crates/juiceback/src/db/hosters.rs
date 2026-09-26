use rusqlite::{Connection, OptionalExtension, Result, params};

use super::{common::sort_direction, types::HosterRecord};

pub fn upsert_hoster(conn: &Connection, host: &str, status: &str, error: &str) -> Result<()> {
    conn.execute(
        "INSERT INTO hosters (host, first_seen_at, last_seen_at, last_status, last_error)
         VALUES (?1, unixepoch(), unixepoch(), ?2, ?3)
         ON CONFLICT(host) DO UPDATE SET
            last_seen_at = unixepoch(),
            last_status = excluded.last_status,
            last_error = excluded.last_error",
        params![host, status, error],
    )?;
    Ok(())
}

pub fn get_hoster(conn: &Connection, host: &str) -> Result<Option<HosterRecord>> {
    conn.query_row(
        "SELECT host, first_seen_at, last_seen_at, last_status, last_error, \
         banned, banned_reason, banned_by, banned_at FROM hosters WHERE host = ?1",
        params![host],
        |row| {
            Ok(HosterRecord {
                host: row.get(0)?,
                first_seen_at: row.get(1)?,
                last_seen_at: row.get(2)?,
                last_status: row.get(3)?,
                last_error: row.get(4)?,
                banned: row.get::<_, i64>(5)? != 0,
                banned_reason: row.get(6)?,
                banned_by: row.get(7)?,
                banned_at: row.get(8)?,
            })
        },
    )
    .optional()
}

pub fn update_hoster_banned(
    conn: &Connection,
    host: &str,
    banned: bool,
    reason: &str,
    banned_by: &str,
) -> Result<bool> {
    let affected = conn.execute(
        "UPDATE hosters SET banned = ?1, banned_reason = ?2, banned_by = ?3, \
         banned_at = CASE WHEN ?1 = 1 THEN unixepoch() ELSE NULL END WHERE host = ?4",
        params![i64::from(banned), reason, banned_by, host],
    )?;
    Ok(affected > 0)
}

fn hoster_sort_column(sort: &str) -> &str {
    match sort {
        "host" => "host",
        "first_seen" => "first_seen_at",
        "last_status" => "last_status",
        "banned" => "banned",
        _ => "last_seen_at",
    }
}

pub fn list_hosters_paginated(
    conn: &Connection,
    search: &str,
    sort: &str,
    dir: &str,
    offset: i64,
    limit: i64,
) -> Result<(Vec<HosterRecord>, i64)> {
    let col = hoster_sort_column(sort);
    let d = sort_direction(dir);

    let (where_clause, search_param): (&str, String) = if search.is_empty() {
        ("", String::new())
    } else {
        let pattern = format!("%{search}%");
        (
            "WHERE (host LIKE ?1 OR last_status LIKE ?1 OR last_error LIKE ?1 OR banned_reason LIKE ?1)",
            pattern,
        )
    };

    let sql = format!(
        "SELECT host, first_seen_at, last_seen_at, last_status, last_error, \
         banned, banned_reason, banned_by, banned_at, COUNT(*) OVER() AS total FROM hosters {} \
         ORDER BY {} {} LIMIT ?{} OFFSET ?{}",
        where_clause,
        col,
        d,
        if search.is_empty() { "1" } else { "2" },
        if search.is_empty() { "2" } else { "3" },
    );

    let mut stmt = conn.prepare(&sql)?;
    let mut total = 0;
    let rows = if search.is_empty() {
        let r = stmt.query_map(params![limit, offset], |row| {
            total = row.get(9)?;
            Ok(HosterRecord {
                host: row.get(0)?,
                first_seen_at: row.get(1)?,
                last_seen_at: row.get(2)?,
                last_status: row.get(3)?,
                last_error: row.get(4)?,
                banned: row.get::<_, i64>(5)? != 0,
                banned_reason: row.get(6)?,
                banned_by: row.get(7)?,
                banned_at: row.get(8)?,
            })
        })?;
        r.collect::<Result<Vec<_>>>()?
    } else {
        let r = stmt.query_map(params![search_param, limit, offset], |row| {
            Ok(HosterRecord {
                host: row.get(0)?,
                first_seen_at: row.get(1)?,
                last_seen_at: row.get(2)?,
                last_status: row.get(3)?,
                last_error: row.get(4)?,
                banned: row.get::<_, i64>(5)? != 0,
                banned_reason: row.get(6)?,
                banned_by: row.get(7)?,
                banned_at: row.get(8)?,
            })
        })?;
        r.collect::<Result<Vec<_>>>()?
    };

    Ok((rows, total))
}

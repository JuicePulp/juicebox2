use rusqlite::{Connection, Result, params};

use super::{
    common::sort_direction,
    files::{FILE_COLUMNS, row_to_file_record},
    types::{BanRecord, FeedbackRecord, FileRecord, ReportRecord},
};

fn file_sort_column(sort: &str) -> &str {
    match sort {
        "filename" => "filename",
        "size_bytes" => "size_bytes",
        "mime_type" => "mime_type",
        "uploaded_at" => "uploaded_at",
        "expires_at" => "expires_at",
        _ => "uploaded_at",
    }
}

fn report_sort_column(sort: &str) -> &str {
    match sort {
        "reason" => "reason",
        "id" => "id",
        _ => "created_at",
    }
}

fn feedback_sort_column(sort: &str) -> &str {
    match sort {
        "id" => "id",
        _ => "created_at",
    }
}

fn ban_sort_column(sort: &str) -> &str {
    match sort {
        "reason" => "reason",
        "banned_by" => "banned_by",
        _ => "banned_at",
    }
}

pub fn list_files_paginated(
    conn: &Connection,
    search: &str,
    sort: &str,
    dir: &str,
    offset: i64,
    limit: i64,
) -> Result<(Vec<FileRecord>, i64)> {
    let col = file_sort_column(sort);
    let d = sort_direction(dir);

    let (where_clause, search_param): (&str, String) = if search.is_empty() {
        ("", String::new())
    } else {
        let pattern = format!("%{search}%");
        (
            "WHERE (id LIKE ?1 OR filename LIKE ?1 OR mime_type LIKE ?1 OR uploader_ip LIKE ?1)",
            pattern,
        )
    };

    let sql = format!(
        "SELECT {}, COUNT(*) OVER() AS total FROM files {} ORDER BY {} {} LIMIT ?{} OFFSET ?{}",
        FILE_COLUMNS,
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
            total = row.get(11)?;
            row_to_file_record(row)
        })?;
        r.collect::<Result<Vec<_>>>()?
    } else {
        let r = stmt.query_map(params![search_param, limit, offset], |row| {
            total = row.get(11)?;
            row_to_file_record(row)
        })?;
        r.collect::<Result<Vec<_>>>()?
    };

    Ok((rows, total))
}

pub fn list_reports_paginated(
    conn: &Connection,
    search: &str,
    sort: &str,
    dir: &str,
    offset: i64,
    limit: i64,
) -> Result<(Vec<ReportRecord>, i64)> {
    let col = report_sort_column(sort);
    let d = sort_direction(dir);

    let (where_clause, search_param): (&str, String) = if search.is_empty() {
        ("", String::new())
    } else {
        let pattern = format!("%{search}%");
        (
            "WHERE (file_url LIKE ?1 OR reason LIKE ?1 OR details LIKE ?1 OR email LIKE ?1)",
            pattern,
        )
    };

    let sql = format!(
        "SELECT id, file_url, reason, details, reporter_ip, email, created_at, COUNT(*) OVER() AS total FROM reports {} ORDER BY {} {} LIMIT ?{} OFFSET ?{}",
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
            total = row.get(7)?;
            Ok(ReportRecord {
                id: row.get(0)?,
                file_url: row.get(1)?,
                reason: row.get(2)?,
                details: row.get(3)?,
                reporter_ip: row.get(4)?,
                email: row.get(5)?,
                created_at: row.get(6)?,
            })
        })?;
        r.collect::<Result<Vec<_>>>()?
    } else {
        let r = stmt.query_map(params![search_param, limit, offset], |row| {
            total = row.get(7)?;
            Ok(ReportRecord {
                id: row.get(0)?,
                file_url: row.get(1)?,
                reason: row.get(2)?,
                details: row.get(3)?,
                reporter_ip: row.get(4)?,
                email: row.get(5)?,
                created_at: row.get(6)?,
            })
        })?;
        r.collect::<Result<Vec<_>>>()?
    };

    Ok((rows, total))
}

pub fn list_feedback_paginated(
    conn: &Connection,
    search: &str,
    sort: &str,
    dir: &str,
    offset: i64,
    limit: i64,
) -> Result<(Vec<FeedbackRecord>, i64)> {
    let col = feedback_sort_column(sort);
    let d = sort_direction(dir);

    let (where_clause, search_param): (&str, String) = if search.is_empty() {
        ("", String::new())
    } else {
        let pattern = format!("%{search}%");
        ("WHERE (message LIKE ?1 OR email LIKE ?1)", pattern)
    };

    let sql = format!(
        "SELECT id, message, email, reporter_ip, created_at, COUNT(*) OVER() AS total FROM feedback {} ORDER BY {} {} LIMIT ?{} OFFSET ?{}",
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
            total = row.get(5)?;
            Ok(FeedbackRecord {
                id: row.get(0)?,
                message: row.get(1)?,
                email: row.get(2)?,
                reporter_ip: row.get(3)?,
                created_at: row.get(4)?,
            })
        })?;
        r.collect::<Result<Vec<_>>>()?
    } else {
        let r = stmt.query_map(params![search_param, limit, offset], |row| {
            total = row.get(5)?;
            Ok(FeedbackRecord {
                id: row.get(0)?,
                message: row.get(1)?,
                email: row.get(2)?,
                reporter_ip: row.get(3)?,
                created_at: row.get(4)?,
            })
        })?;
        r.collect::<Result<Vec<_>>>()?
    };

    Ok((rows, total))
}

pub fn list_banned_ips_paginated(
    conn: &Connection,
    search: &str,
    sort: &str,
    dir: &str,
    offset: i64,
    limit: i64,
) -> Result<(Vec<BanRecord>, i64)> {
    let col = ban_sort_column(sort);
    let d = sort_direction(dir);

    let (where_clause, search_param): (&str, String) = if search.is_empty() {
        ("", String::new())
    } else {
        let pattern = format!("%{search}%");
        (
            "WHERE (ip LIKE ?1 OR reason LIKE ?1 OR banned_by LIKE ?1)",
            pattern,
        )
    };

    let sql = format!(
        "SELECT ip, reason, banned_by, banned_at, COUNT(*) OVER() AS total FROM banned_ips {} ORDER BY {} {} LIMIT ?{} OFFSET ?{}",
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
            total = row.get(4)?;
            Ok(BanRecord {
                ip: row.get(0)?,
                reason: row.get(1)?,
                banned_by: row.get(2)?,
                banned_at: row.get(3)?,
            })
        })?;
        r.collect::<Result<Vec<_>>>()?
    } else {
        let r = stmt.query_map(params![search_param, limit, offset], |row| {
            total = row.get(4)?;
            Ok(BanRecord {
                ip: row.get(0)?,
                reason: row.get(1)?,
                banned_by: row.get(2)?,
                banned_at: row.get(3)?,
            })
        })?;
        r.collect::<Result<Vec<_>>>()?
    };

    Ok((rows, total))
}

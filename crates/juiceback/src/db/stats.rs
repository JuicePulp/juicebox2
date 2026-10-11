use rusqlite::{Connection, Result, params};

/// Which serving action a file hit counts toward. Views are inline serves
/// (`/f/`, preview pages, protected `/content`); downloads are forced
/// attachments (`/d/`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HitKind {
    View,
    Download,
}

impl HitKind {
    #[must_use]
    pub fn from_str(kind: &str) -> Option<Self> {
        match kind {
            "view" => Some(Self::View),
            "download" => Some(Self::Download),
            _ => None,
        }
    }
}

/// Aggregated counters for one file, for the admin files list.
pub struct FileStatsAggregate {
    pub file_id: String,
    pub viewers: i64,
    pub views: i64,
    pub downloaders: i64,
    pub downloads: i64,
    pub last_seen_at: i64,
}

/// Admin dashboard totals.
#[derive(Debug, Default)]
pub struct SiteOverview {
    /// Unique people ever seen on the main site (by IP hash).
    pub site_visitors: i64,
    /// Total main-site page hits across all visitors.
    pub site_visits: i64,
    /// Distinct uploader identities with at least one file on record.
    pub uploaders: i64,
    /// Unique people who viewed at least one file.
    pub file_viewers: i64,
    /// Total file view hits.
    pub file_views: i64,
    /// Total file download hits.
    pub file_downloads: i64,
}

/// Record one serving hit. `ip_hash` must already be the peppered HMAC, so
/// raw IPs never touch these tables. Range-request chunks from the same
/// viewer collapse into their existing row.
pub fn record_file_hit(
    conn: &Connection,
    file_id: &str,
    ip_hash: &str,
    kind: HitKind,
    now: i64,
) -> Result<()> {
    let (views, downloads) = match kind {
        HitKind::View => (1, 0),
        HitKind::Download => (0, 1),
    };
    conn.execute(
        "INSERT INTO file_stats (file_id, ip_hash, first_seen_at, last_seen_at, views, downloads)
         VALUES (?1, ?2, ?3, ?3, ?4, ?5)
         ON CONFLICT(file_id, ip_hash) DO UPDATE SET
             views = views + excluded.views,
             downloads = downloads + excluded.downloads,
             last_seen_at = excluded.last_seen_at",
        params![file_id, ip_hash, now, views, downloads],
    )?;
    Ok(())
}

/// Record one main-site page hit for a (hashed) visitor.
pub fn record_site_visit(conn: &Connection, ip_hash: &str, now: i64) -> Result<()> {
    conn.execute(
        "INSERT INTO site_visitors (ip_hash, first_seen_at, last_seen_at, visits)
         VALUES (?1, ?2, ?2, 1)
         ON CONFLICT(ip_hash) DO UPDATE SET
             visits = visits + 1,
             last_seen_at = excluded.last_seen_at",
        params![ip_hash, now],
    )?;
    Ok(())
}

/// Aggregated stats for exactly the given file ids (one row per file that
/// has any stats; files without stats are absent and default to zeros).
pub fn file_stats_for_ids(conn: &Connection, ids: &[String]) -> Result<Vec<FileStatsAggregate>> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let placeholders: String = (1..=ids.len())
        .map(|i| format!("?{i}"))
        .collect::<Vec<_>>()
        .join(", ");
    let sql = format!(
        "SELECT file_id,
            COALESCE(SUM(views > 0), 0),
            COALESCE(SUM(views), 0),
            COALESCE(SUM(downloads > 0), 0),
            COALESCE(SUM(downloads), 0),
            MAX(last_seen_at)
         FROM file_stats WHERE file_id IN ({placeholders}) GROUP BY file_id"
    );
    let params: Vec<&dyn rusqlite::ToSql> = ids.iter().map(|s| s as &dyn rusqlite::ToSql).collect();
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params.as_slice(), |row| {
        Ok(FileStatsAggregate {
            file_id: row.get(0)?,
            viewers: row.get(1)?,
            views: row.get(2)?,
            downloaders: row.get(3)?,
            downloads: row.get(4)?,
            last_seen_at: row.get(5)?,
        })
    })?;
    rows.collect()
}

/// Dashboard totals for the admin panel. Never exposed publicly.
pub fn site_overview(conn: &Connection) -> Result<SiteOverview> {
    conn.query_row(
        "SELECT
            (SELECT COUNT(*) FROM site_visitors),
            (SELECT COALESCE(SUM(visits), 0) FROM site_visitors),
            (SELECT COUNT(DISTINCT client_key) FROM client_files),
            (SELECT COUNT(DISTINCT ip_hash) FROM file_stats WHERE views > 0),
            (SELECT COALESCE(SUM(views), 0) FROM file_stats),
            (SELECT COALESCE(SUM(downloads), 0) FROM file_stats)",
        [],
        |row| {
            Ok(SiteOverview {
                site_visitors: row.get(0)?,
                site_visits: row.get(1)?,
                uploaders: row.get(2)?,
                file_viewers: row.get(3)?,
                file_views: row.get(4)?,
                file_downloads: row.get(5)?,
            })
        },
    )
}

/// Drop stats rows for one file (called whenever the file itself is
/// deleted, so counters never outlive their file in admin views).
pub fn delete_file_stats(conn: &Connection, file_id: &str) -> Result<()> {
    conn.execute(
        "DELETE FROM file_stats WHERE file_id = ?1",
        params![file_id],
    )?;
    Ok(())
}

/// Drop stats rows whose file no longer exists (belt-and-braces for rows
/// predating the delete cascade, e.g. manual DB edits).
pub fn delete_orphan_file_stats(conn: &Connection) -> Result<usize> {
    let affected = conn.execute(
        "DELETE FROM file_stats WHERE file_id NOT IN (SELECT id FROM files)",
        [],
    )?;
    Ok(affected)
}

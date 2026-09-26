use std::collections::HashSet;

use rusqlite::{Connection, Result, params};

pub fn init_db(conn: &Connection) -> Result<()> {
    create_base_schema(conn)?;
    apply_migrations(conn)?;
    Ok(())
}

/// Number of ordered migrations in [`MIGRATIONS`]. Bump by appending; never
/// reorder or edit an applied migration.
pub fn migration_count() -> u32 {
    MIGRATIONS.len() as u32
}

/// Highest applied migration version recorded for this database.
pub fn schema_version(conn: &Connection) -> Result<u32> {
    conn.execute(
        "CREATE TABLE IF NOT EXISTS schema_migrations (
            version    INTEGER PRIMARY KEY,
            applied_at INTEGER NOT NULL DEFAULT (unixepoch())
        )",
        [],
    )?;
    let max: Option<u32> =
        conn.query_row("SELECT MAX(version) FROM schema_migrations", [], |row| {
            row.get(0)
        })?;
    Ok(max.unwrap_or(0))
}

/// Ordered schema migrations. Each entry runs once per database, tracked in
/// `schema_migrations`; every statement is safe to re-run (IF NOT EXISTS or
/// existence-checked ALTERs), so concurrent startups converge harmlessly.
const MIGRATIONS: &[(&str, fn(&Connection) -> Result<()>)] = &[
    ("failed_logins.ip_hash", m01_failed_logins_ip_hash),
    ("files.storage_host", m02_files_storage_host),
    ("reports.email", m03_reports_email),
    ("announcements.mode", m04_announcements_mode),
    ("files.status", m05_files_status),
    ("pairing_codes.ip_hash", m06_pairing_codes_ip_hash),
    ("fetch_jobs.progress", m07_fetch_jobs_progress),
    ("client_files.legacy_index", m08_client_files_legacy_index),
];

fn apply_migrations(conn: &Connection) -> Result<()> {
    schema_version(conn)?;
    let applied: HashSet<u32> = conn
        .prepare("SELECT version FROM schema_migrations")?
        .query_map([], |row| row.get(0))?
        .collect::<Result<_, _>>()?;
    for (idx, (name, migrate)) in MIGRATIONS.iter().enumerate() {
        let version = idx as u32 + 1;
        if applied.contains(&version) {
            continue;
        }
        tracing::debug!("applying schema migration v{version}: {name}");
        migrate(conn)?;
        // OR IGNORE: a concurrent startup may have applied it first; the
        // statements above are all idempotent.
        conn.execute(
            "INSERT OR IGNORE INTO schema_migrations (version) VALUES (?1)",
            params![version],
        )?;
    }
    Ok(())
}

fn has_column(conn: &Connection, table: &str, column: &str) -> Result<bool> {
    Ok(conn
        .prepare(&format!("PRAGMA table_info({table})"))?
        .query_map([], |row| row.get::<_, String>(1))?
        .any(|name| name.is_ok_and(|n| n == column)))
}

fn m01_failed_logins_ip_hash(conn: &Connection) -> Result<()> {
    if !has_column(conn, "failed_logins", "ip_hash")? {
        conn.execute(
            "ALTER TABLE failed_logins ADD COLUMN ip_hash TEXT NOT NULL DEFAULT ''",
            [],
        )?;
    }
    Ok(())
}

fn m02_files_storage_host(conn: &Connection) -> Result<()> {
    if !has_column(conn, "files", "storage_host")? {
        conn.execute(
            "ALTER TABLE files ADD COLUMN storage_host TEXT NOT NULL DEFAULT ''",
            [],
        )?;
    }
    Ok(())
}

fn m03_reports_email(conn: &Connection) -> Result<()> {
    if !has_column(conn, "reports", "email")? {
        conn.execute("ALTER TABLE reports ADD COLUMN email TEXT", [])?;
    }
    Ok(())
}

fn m04_announcements_mode(conn: &Connection) -> Result<()> {
    if !has_column(conn, "announcements", "mode")? {
        conn.execute(
            "ALTER TABLE announcements ADD COLUMN mode TEXT NOT NULL DEFAULT 'warning'",
            [],
        )?;
    }
    Ok(())
}

fn m05_files_status(conn: &Connection) -> Result<()> {
    if !has_column(conn, "files", "status")? {
        conn.execute(
            "ALTER TABLE files ADD COLUMN status TEXT NOT NULL DEFAULT 'ready'",
            [],
        )?;
    }
    // Serves the abandoned-uploads cleanup sweep (status filter) and its
    // companion ordered scan. Created here so the column always exists first.
    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_files_status_uploaded ON files(status, uploaded_at)",
        [],
    )?;
    Ok(())
}

fn m06_pairing_codes_ip_hash(conn: &Connection) -> Result<()> {
    if !has_column(conn, "pairing_codes", "ip_hash")? {
        conn.execute(
            "ALTER TABLE pairing_codes ADD COLUMN ip_hash TEXT NOT NULL DEFAULT ''",
            [],
        )?;
    }
    Ok(())
}

fn m07_fetch_jobs_progress(conn: &Connection) -> Result<()> {
    // Progress columns (added after the table shipped); ignore errors when
    // they already exist on upgraded databases.
    let _ = conn.execute(
        "ALTER TABLE fetch_jobs ADD COLUMN stage TEXT NOT NULL DEFAULT ''",
        [],
    );
    let _ = conn.execute(
        "ALTER TABLE fetch_jobs ADD COLUMN bytes_received INTEGER NOT NULL DEFAULT 0",
        [],
    );
    Ok(())
}

fn m08_client_files_legacy_index(conn: &Connection) -> Result<()> {
    // The PK (client_key, file_id) already covers bare client_key lookups,
    // so the old single-column index is redundant.
    conn.execute("DROP INDEX IF EXISTS idx_client_files_client", [])?;
    Ok(())
}

fn create_base_schema(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS sessions (
            token_hash   TEXT PRIMARY KEY,
            user_id      TEXT NOT NULL,
            created_at   INTEGER NOT NULL,
            expires_at   INTEGER NOT NULL,
            last_seen_at INTEGER NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_sessions_expires ON sessions(expires_at);",
    )?;
    conn.execute(
        "CREATE TABLE IF NOT EXISTS files (
            id            TEXT PRIMARY KEY,
            filename      TEXT NOT NULL,
            mime_type     TEXT NOT NULL,
            size_bytes    INTEGER NOT NULL,
            storage_path  TEXT NOT NULL UNIQUE,
            delete_token  TEXT NOT NULL,
            uploaded_at   INTEGER NOT NULL,
            expires_at    INTEGER NOT NULL,
            uploader_ip   TEXT
        )",
        [],
    )?;

    conn.execute(
        "CREATE TABLE IF NOT EXISTS admins (
            id            INTEGER PRIMARY KEY AUTOINCREMENT,
            username      TEXT NOT NULL UNIQUE,
            password_hash TEXT NOT NULL,
            created_at    INTEGER NOT NULL DEFAULT (unixepoch())
        )",
        [],
    )?;

    conn.execute(
        "CREATE TABLE IF NOT EXISTS reports (
            id            INTEGER PRIMARY KEY AUTOINCREMENT,
            file_url      TEXT NOT NULL,
            reason        TEXT NOT NULL,
            details       TEXT NOT NULL DEFAULT '',
            reporter_ip   TEXT,
            email         TEXT,
            created_at    INTEGER NOT NULL DEFAULT (unixepoch())
        )",
        [],
    )?;

    conn.execute(
        "CREATE TABLE IF NOT EXISTS banned_ips (
            ip          TEXT PRIMARY KEY,
            reason      TEXT NOT NULL DEFAULT '',
            banned_by   TEXT NOT NULL DEFAULT 'admin',
            banned_at   INTEGER NOT NULL DEFAULT (unixepoch())
        )",
        [],
    )?;

    conn.execute(
        "CREATE TABLE IF NOT EXISTS announcements (
            id          INTEGER PRIMARY KEY AUTOINCREMENT,
            message     TEXT NOT NULL DEFAULT '',
            link_url    TEXT NOT NULL DEFAULT '',
            mode        TEXT NOT NULL DEFAULT 'warning',
            is_active   INTEGER NOT NULL DEFAULT 0,
            created_at  INTEGER NOT NULL DEFAULT (unixepoch()),
            updated_at  INTEGER NOT NULL DEFAULT (unixepoch())
        )",
        [],
    )?;

    conn.execute(
        "CREATE TABLE IF NOT EXISTS feedback (
            id            INTEGER PRIMARY KEY AUTOINCREMENT,
            message       TEXT NOT NULL,
            email         TEXT,
            reporter_ip   TEXT,
            created_at    INTEGER NOT NULL DEFAULT (unixepoch())
        )",
        [],
    )?;

    // Known custom hosts users have pointed the UI at. First use is tracked so
    // admins can see who's been used and ban offenders.
    conn.execute(
        "CREATE TABLE IF NOT EXISTS hosters (
            host          TEXT PRIMARY KEY,
            first_seen_at INTEGER NOT NULL DEFAULT (unixepoch()),
            last_seen_at  INTEGER NOT NULL DEFAULT (unixepoch()),
            last_status   TEXT NOT NULL DEFAULT 'unknown',
            last_error    TEXT NOT NULL DEFAULT '',
            banned        INTEGER NOT NULL DEFAULT 0,
            banned_reason TEXT NOT NULL DEFAULT '',
            banned_by     TEXT NOT NULL DEFAULT '',
            banned_at     INTEGER
        )",
        [],
    )?;
    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_hosters_last_seen ON hosters(last_seen_at)",
        [],
    )?;

    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_expires_at ON files(expires_at)",
        [],
    )?;

    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_reports_created_at ON reports(created_at)",
        [],
    )?;

    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_feedback_created_at ON feedback(created_at)",
        [],
    )?;

    // Table for persisting admin failed login attempts (survives restarts)
    conn.execute(
        "CREATE TABLE IF NOT EXISTS failed_logins (
            username   TEXT NOT NULL,
            ip_hash    TEXT NOT NULL DEFAULT '',
            attempt_at INTEGER NOT NULL DEFAULT (unixepoch())
        )",
        [],
    )?;

    // Maps previous file IDs to their current ID so old URLs can redirect.
    conn.execute(
        "CREATE TABLE IF NOT EXISTS aliases (
            old_id    TEXT PRIMARY KEY,
            new_id    TEXT NOT NULL,
            created_at INTEGER NOT NULL DEFAULT (unixepoch())
        )",
        [],
    )?;
    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_failed_logins_username ON failed_logins(username)",
        [],
    )?;
    // Serve the per-login attempt COUNT guard.
    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_failed_logins_user_ip ON failed_logins(username, ip_hash, attempt_at)",
        [],
    )?;

    // Pairing codes: short-lived codes for juicebox-plus device registration
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS pairing_codes (
            code_hash TEXT PRIMARY KEY,
            user_id TEXT NOT NULL,
            created_at INTEGER NOT NULL,
            expires_at INTEGER NOT NULL,
            used INTEGER NOT NULL DEFAULT 0,
            ip_hash TEXT NOT NULL DEFAULT ''
        );
        CREATE INDEX IF NOT EXISTS idx_pairing_codes_expires ON pairing_codes(expires_at);",
    )?;
    // Serve the per-generate COUNT guards (user and IP quotas).
    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_pairing_codes_user ON pairing_codes(user_id, used, expires_at)",
        [],
    )?;
    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_pairing_codes_ip ON pairing_codes(ip_hash, used, expires_at)",
        [],
    )?;

    // Registered devices (juicebox-plus instances paired to user accounts)
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS devices (
            id TEXT PRIMARY KEY,
            user_id TEXT NOT NULL,
            device_name TEXT NOT NULL,
            paired_at INTEGER NOT NULL,
            last_seen_at INTEGER NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_devices_user ON devices(user_id);",
    )?;

    // Per-client upload registry: the browser's upload list mirrored server-side
    // (keyed by the `jb_uid` user id), so no-JS /files can render the full list
    // without the ~4KB cookie cap and without the files table knowing the owner.
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS client_files (
            client_key   TEXT NOT NULL,
            file_id      TEXT NOT NULL,
            filename     TEXT NOT NULL DEFAULT '',
            mime_type    TEXT NOT NULL DEFAULT '',
            size_bytes   INTEGER NOT NULL DEFAULT 0,
            uploaded_at  INTEGER NOT NULL DEFAULT 0,
            expires_at   INTEGER NOT NULL DEFAULT 0,
            delete_token TEXT NOT NULL,
            PRIMARY KEY (client_key, file_id)
        );
        CREATE INDEX IF NOT EXISTS idx_client_files_client_uploaded ON client_files(client_key, uploaded_at);",
    )?;

    // JuiceBox x Cobalt.Tools URL fetch jobs. Tracks async cobalt processing
    // so the browser can poll for completion; file_id links to a normal
    // record in the files table once the transfer succeeds.
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS fetch_jobs (
            id          TEXT PRIMARY KEY,
            user_id     TEXT NOT NULL,
            source_url  TEXT NOT NULL,
            status      TEXT NOT NULL DEFAULT 'pending',
            error       TEXT NOT NULL DEFAULT '',
            file_id     TEXT NOT NULL DEFAULT '',
            created_at  INTEGER NOT NULL DEFAULT (unixepoch()),
            updated_at  INTEGER NOT NULL DEFAULT (unixepoch())
        );
        CREATE INDEX IF NOT EXISTS idx_fetch_jobs_user ON fetch_jobs(user_id);
        CREATE INDEX IF NOT EXISTS idx_fetch_jobs_updated ON fetch_jobs(updated_at);
        CREATE INDEX IF NOT EXISTS idx_fetch_jobs_status_updated ON fetch_jobs(status, updated_at);",
    )?;

    Ok(())
}

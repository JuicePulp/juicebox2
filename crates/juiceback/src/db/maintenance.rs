use rusqlite::Connection;

pub fn migrate_raw_ips_to_encrypted(
    conn: &Connection,
    encryption_key: &str,
    pepper: &str,
) -> rusqlite::Result<()> {
    fn looks_like_raw_ip(val: &str) -> bool {
        let trimmed = val.trim();
        if trimmed.is_empty() {
            return false;
        }
        // Already encrypted values contain ':' separator between nonce and ciphertext
        // and are long hex strings
        if trimmed.len() > 40 && trimmed.contains(':') {
            return false;
        }
        // Already HMAC hashes are 64-char hex
        if trimmed.len() == 64 && trimmed.chars().all(|c| c.is_ascii_hexdigit()) {
            return false;
        }
        if trimmed.split('.').all(|part| part.parse::<u8>().is_ok()) {
            return true;
        }
        if trimmed.contains(':') && trimmed.len() < 40 {
            return true;
        }
        false
    }

    let mut encrypted_count = 0u64;
    let mut hashed_count = 0u64;

    // Migrate files.uploader_ip -> encrypted
    {
        let mut stmt =
            conn.prepare("SELECT id, uploader_ip FROM files WHERE uploader_ip IS NOT NULL")?;
        let rows: Vec<(String, String)> = stmt
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect::<rusqlite::Result<_>>()?;

        for (id, ip) in rows {
            if looks_like_raw_ip(&ip) {
                let encrypted = crate::utils::encrypt_ip(&ip, encryption_key);
                conn.execute(
                    "UPDATE files SET uploader_ip = ?1 WHERE id = ?2",
                    rusqlite::params![encrypted, id],
                )?;
                encrypted_count += 1;
            }
        }
    }

    // Migrate reports.reporter_ip -> encrypted
    {
        let mut stmt =
            conn.prepare("SELECT id, reporter_ip FROM reports WHERE reporter_ip IS NOT NULL")?;
        let rows: Vec<(i64, String)> = stmt
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect::<rusqlite::Result<_>>()?;

        for (id, ip) in rows {
            if looks_like_raw_ip(&ip) {
                let encrypted = crate::utils::encrypt_ip(&ip, encryption_key);
                conn.execute(
                    "UPDATE reports SET reporter_ip = ?1 WHERE id = ?2",
                    rusqlite::params![encrypted, id],
                )?;
                encrypted_count += 1;
            }
        }
    }

    // Migrate feedback.reporter_ip -> encrypted
    {
        let mut stmt =
            conn.prepare("SELECT id, reporter_ip FROM feedback WHERE reporter_ip IS NOT NULL")?;
        let rows: Vec<(i64, String)> = stmt
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect::<rusqlite::Result<_>>()?;

        for (id, ip) in rows {
            if looks_like_raw_ip(&ip) {
                let encrypted = crate::utils::encrypt_ip(&ip, encryption_key);
                conn.execute(
                    "UPDATE feedback SET reporter_ip = ?1 WHERE id = ?2",
                    rusqlite::params![encrypted, id],
                )?;
                encrypted_count += 1;
            }
        }
    }

    // Migrate banned_ips.ip -> HMAC hash
    {
        let mut stmt = conn.prepare("SELECT ip FROM banned_ips")?;
        let rows: Vec<String> = stmt
            .query_map([], |row| row.get(0))?
            .collect::<rusqlite::Result<_>>()?;

        for ip in rows {
            if looks_like_raw_ip(&ip) {
                let hashed = crate::utils::hash_ip_for_ban(&ip, pepper);
                conn.execute(
                    "UPDATE banned_ips SET ip = ?1 WHERE ip = ?2",
                    rusqlite::params![hashed, ip],
                )?;
                hashed_count += 1;
            }
        }
    }

    if encrypted_count > 0 || hashed_count > 0 {
        tracing::info!(
            "ip migration: {} IPs encrypted, {} ban entries hashed",
            encrypted_count,
            hashed_count
        );
    }

    Ok(())
}

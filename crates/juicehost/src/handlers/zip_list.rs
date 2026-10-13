







use std::sync::Arc;

use bytes::Bytes;
use futures::StreamExt;

use crate::storage::StorageBackend;

pub struct ZipEntry {
    pub name: String,
    pub size: Option<u64>,
    pub is_dir: bool,
    /// DOS modification stamp as `"YYYY-MM-DD HH:MM"`, if the entry carries
    /// a sane one.
    pub modified: Option<String>,
    /// Bit 0 of the entry flags: contents are encrypted.
    pub encrypted: bool,

    pub method: u16,

    pub comp_size: u64,

    pub header_offset: u32,
}


const MAX_CD_BYTES: u64 = 8 * 1024 * 1024;

const TAIL_WINDOW: u64 = 66_000;

const MAX_ENTRIES: usize = 5_000;

fn u16le(b: &[u8]) -> u16 {
    u16::from_le_bytes([b[0], b[1]])
}

fn u32le(b: &[u8]) -> u32 {
    u32::from_le_bytes([b[0], b[1], b[2], b[3]])
}

pub(crate) async fn read_range_capped(
    storage: &Arc<dyn StorageBackend>,
    id: &str,
    start: u64,
    end_inclusive: u64,
    cap: u64,
) -> Option<Bytes> {
    if end_inclusive < start || end_inclusive - start + 1 > cap {
        return None;
    }
    let mut stream = storage
        .get_range_stream(id, start, end_inclusive)
        .await
        .ok()?;
    let mut out = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.ok()?;
        if out.len() as u64 + chunk.len() as u64 > cap {
            return None;
        }
        out.extend_from_slice(&chunk);
    }
    Some(Bytes::from(out))
}



fn find_eocd(tail: &[u8], file_size: u64) -> Option<(u64, u64, usize)> {
    if tail.len() < 22 {
        return None;
    }
    let mut pos = tail.len() - 22;
    loop {
        if tail[pos..pos + 4] == *b"PK\x05\x06" {
            let comment_len = u16le(&tail[pos + 20..pos + 22]) as usize;
            if pos + 22 + comment_len == tail.len() {
                let disk_no = u16le(&tail[pos + 4..pos + 6]);
                let cd_disk = u16le(&tail[pos + 6..pos + 8]);
                let disk_entries = u16le(&tail[pos + 8..pos + 10]);
                let total_entries = u16le(&tail[pos + 10..pos + 12]);
                let cd_size = u32le(&tail[pos + 12..pos + 16]) as u64;
                let cd_offset = u32le(&tail[pos + 16..pos + 20]) as u64;
           
           
                if disk_no == 0
                    && cd_disk == 0
                    && disk_entries == total_entries
                    && total_entries > 0
                    && total_entries != 0xFFFF
                    && (total_entries as usize) < MAX_ENTRIES
                    && cd_size > 0
                    && cd_size <= MAX_CD_BYTES
                    && cd_offset.checked_add(cd_size)? <= file_size
                {
                    return Some((cd_offset, cd_size, total_entries as usize));
                }
            }
        }
        if pos == 0 {
            break;
        }
        pos -= 1;
    }
    None
}

fn decode_name(raw: &[u8]) -> Option<String> {
    if raw.is_empty() {
        return None;
    }
    
    let text = String::from_utf8_lossy(raw);
    if text.chars().any(|c| c.is_control() && c != '\t') {
        return None;
    }
    Some(text.into_owned())
}



fn dos_datetime(time: u16, date: u16) -> Option<String> {
    let secs = u32::from(time & 0x1F) * 2;
    let mins = u32::from((time >> 5) & 0x3F);
    let hours = u32::from((time >> 11) & 0x1F);
    let day = u32::from(date & 0x1F);
    let month = u32::from((date >> 5) & 0x0F);
    let year = u32::from((date >> 9) & 0x7F) + 1980;
    if day == 0 || day > 31 || month == 0 || month > 12 || hours > 23 || mins > 59 || secs > 59
    {
        return None;
    }
    Some(format!("{year:04}-{month:02}-{day:02} {hours:02}:{mins:02}"))
}


fn parse_entries(cd: &[u8], count: usize) -> Option<Vec<ZipEntry>> {
    let mut out = Vec::with_capacity(count.min(512));
    let mut pos = 0usize;
    for _ in 0..count {
        if pos + 46 > cd.len() || cd[pos..pos + 4] != *b"PK\x01\x02" {
            return None;
        }
        let comp_size = u32le(&cd[pos + 20..pos + 24]) as u64;
        let uncomp_size = u32le(&cd[pos + 24..pos + 28]) as u64;
        let method = u16le(&cd[pos + 10..pos + 12]);
        let header_offset = u32le(&cd[pos + 42..pos + 46]);
        let mod_time = u16le(&cd[pos + 12..pos + 14]);
        let mod_date = u16le(&cd[pos + 14..pos + 16]);
        let enc_flag = u16le(&cd[pos + 8..pos + 10]) & 0x01 != 0;
        let name_len = u16le(&cd[pos + 28..pos + 30]) as usize;
        let extra_len = u16le(&cd[pos + 30..pos + 32]) as usize;
        let comment_len = u16le(&cd[pos + 32..pos + 34]) as usize;
        let total = 46usize
            .checked_add(name_len)?
            .checked_add(extra_len)?
            .checked_add(comment_len)?;
        if pos + total > cd.len() {
            return None;
        }
        if let Some(name) = decode_name(&cd[pos + 46..pos + 46 + name_len]) {
            let is_dir = name.ends_with('/');
            
            let size = if is_dir || comp_size == 0xFFFF_FFFF || uncomp_size == 0xFFFF_FFFF {
                None
            } else {
                Some(uncomp_size)
            };
            out.push(ZipEntry {
                name,
                size,
                is_dir,
                modified: dos_datetime(mod_time, mod_date),
                encrypted: enc_flag,
                method,
                comp_size,
                header_offset,
            });
        }
        pos += total;
    }
    if out.is_empty() { None } else { Some(out) }
}



pub async fn load_zip_entries(
    storage: &Arc<dyn StorageBackend>,
    id: &str,
    file_size: u64,
) -> Option<Vec<ZipEntry>> {
    if file_size < 22 {
        return None;
    }
    let tail_len = TAIL_WINDOW.min(file_size);
    let tail =
        read_range_capped(storage, id, file_size - tail_len, file_size - 1, TAIL_WINDOW).await?;
    let (cd_offset, cd_size, count) = find_eocd(&tail, file_size)?;
    let cd =
        read_range_capped(storage, id, cd_offset, cd_offset + cd_size - 1, MAX_CD_BYTES).await?;
    parse_entries(&cd, count)
}

/// Whole-file cap for 7z listings (the format needs random access, so the
/// bytes must be buffered, not streamed).
const MAX_7Z_BYTES: u64 = 32 * 1024 * 1024;
/// Streaming tar scan budget and entry cap.
const MAX_TAR_SCAN_BYTES: u64 = 16 * 1024 * 1024;
const MAX_TAR_ENTRIES: usize = 1_000;
const TAR_CHUNK: u64 = 65_536;

/// List archive contents by filename extension. ZIP reads its central
/// directory, 7z buffers for random access, tar streams from the front,
/// gzip exposes its single member. Anything else (rar, …) falls back to
/// the plain download hero.
pub struct ArchiveListing {
    pub entries: Vec<ZipEntry>,
    /// True when the archive is password-locked (as opposed to corrupt or
    /// unsupported): the page can say so instead of failing silently.
    pub locked: bool,
}

pub async fn load_archive_entries(
    storage: &Arc<dyn StorageBackend>,
    id: &str,
    filename: &str,
    file_size: u64,
) -> Option<ArchiveListing> {
    let ext = filename
        .rsplit('.')
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();
    match ext.as_str() {
        "7z" => load_7z_entries(storage, id, file_size).await,
        "tar" => load_tar_entries(storage, id, file_size)
            .await
            .map(|entries| ArchiveListing {
                entries,
                locked: false,
            }),
        "gz" | "tgz" => load_gz_entry(storage, id, filename, file_size)
            .await
            .map(|entries| ArchiveListing {
                entries,
                locked: false,
            }),
        _ => {
            let entries = load_zip_entries(storage, id, file_size).await?;
            let locked = entries.iter().any(|e| e.encrypted);
            Some(ArchiveListing { entries, locked })
        }
    }
}

async fn load_7z_entries(
    storage: &Arc<dyn StorageBackend>,
    id: &str,
    file_size: u64,
) -> Option<ArchiveListing> {
    if file_size < 32 || file_size > MAX_7Z_BYTES {
        return None;
    }
    let bytes = read_range_capped(storage, id, 0, file_size - 1, MAX_7Z_BYTES).await?;
    let parsed = tokio::task::spawn_blocking(move || {
        let cursor = std::io::Cursor::new(bytes.to_vec());
        let reader =
            sevenz_rust::SevenZReader::new(cursor, file_size, sevenz_rust::Password::empty());
        reader.map(|reader| {
            let mut out = Vec::new();
            for f in reader.archive().files.iter() {
                if f.name.is_empty() || f.is_anti_item {
                    continue;
                }
                out.push(ZipEntry {
                    name: f.name.clone(),
                    size: if f.is_directory { None } else { Some(f.size) },
                    is_dir: f.is_directory,
                    modified: None,
                    encrypted: false,
                    method: 0xFFFF,
                    comp_size: 0,
                    header_offset: 0,
                });
                if out.len() >= MAX_ENTRIES {
                    break;
                }
            }
            out
        })
    })
    .await
    .ok()?;
    match parsed {
        Ok(entries) if !entries.is_empty() => Some(ArchiveListing {
            entries,
            locked: false,
        }),
        Err(sevenz_rust::Error::PasswordRequired) => Some(ArchiveListing {
            entries: Vec::new(),
            locked: true,
        }),
        _ => None,
    }
}

fn parse_octal(raw: &[u8]) -> Option<u64> {
    let text = std::str::from_utf8(raw).ok()?;
    let text = text.trim_matches(['\0', ' ']);
    if text.is_empty() {
        return None;
    }
    u64::from_str_radix(text, 8).ok()
}

async fn load_tar_entries(
    storage: &Arc<dyn StorageBackend>,
    id: &str,
    file_size: u64,
) -> Option<Vec<ZipEntry>> {
    let mut data: Vec<u8> = Vec::new();
    let mut off: usize = 0;
    let mut fetched: u64 = 0;
    let mut out: Vec<ZipEntry> = Vec::new();
    let mut zero_streak: u8 = 0;
    loop {
        while data.len() - off < 512 && fetched < file_size && fetched < MAX_TAR_SCAN_BYTES {
            let end = (fetched + TAR_CHUNK).min(file_size) - 1;
            let chunk = read_range_capped(storage, id, fetched, end, TAR_CHUNK).await?;
            fetched += chunk.len() as u64;
            data.extend_from_slice(&chunk);
        }
        if data.len() - off < 512 {
            break;
        }
        // Compact consumed prefix so memory stays bounded.
        if off > 1_048_576 {
            data.drain(..off);
            off = 0;
        }
        let hdr = &data[off..off + 512];
        if hdr.iter().all(|&b| b == 0) {
            zero_streak += 1;
            off += 512;
            if zero_streak >= 2 {
                break;
            }
            continue;
        }
        zero_streak = 0;
        let ustar = hdr.len() >= 262 && &hdr[257..262] == b"ustar";
        let name = String::from_utf8_lossy(split_nul(&hdr[..100])).into_owned();
        let size = parse_octal(split_blank(&hdr[124..136]));
        let typeflag = hdr[156] as char;
        if name.trim().is_empty() {
            break;
        }
        if !ustar && off > 0 {
            // Trailing garbage after real entries: stop, keep what we have.
            break;
        }
        if !ustar && size.is_none() {
            // Not a tar at all.
            return None;
        }
        let size = size.unwrap_or(0);
        let is_dir = typeflag == '5' || name.ends_with('/');
        // Pax/special headers carry no content of their own here; the size
        // field still tells how far to skip.
        if matches!(typeflag, 'x' | 'g') {
            off += 512 + pad512(size);
            continue;
        }
        out.push(ZipEntry {
            name,
            size: if is_dir { None } else { Some(size) },
            is_dir,
            modified: None,
            encrypted: false,
            method: 0xFFFF,
            comp_size: 0,
            header_offset: 0,
        });
        if out.len() >= MAX_TAR_ENTRIES {
            break;
        }
        off += 512 + pad512(size);
        if off as u64 > fetched && fetched >= file_size {
            break;
        }
    }
    if out.is_empty() { None } else { Some(out) }
}

fn split_nul(field: &[u8]) -> &[u8] {
    match field.iter().position(|&b| b == 0) {
        Some(pos) => &field[..pos],
        None => field,
    }
}

fn split_blank(field: &[u8]) -> &[u8] {
    let mut end = field.len();
    while end > 0 && (field[end - 1] == 0 || field[end - 1] == b' ') {
        end -= 1;
    }
    &field[..end]
}

fn pad512(size: u64) -> usize {
    (size as usize).div_ceil(512) * 512
}

async fn load_gz_entry(
    storage: &Arc<dyn StorageBackend>,
    id: &str,
    filename: &str,
    file_size: u64,
) -> Option<Vec<ZipEntry>> {
    if file_size < 18 {
        return None;
    }
    let head = read_range_capped(storage, id, 0, 4095.min(file_size - 1), 4096).await?;
    if head.len() < 10 || &head[0..2] != b"\x1f\x8b" || head[2] != 8 {
        return None;
    }
    let flags = head[3];
    let mut pos = 10usize;
    if flags & 0x04 != 0 {
        let xlen = u16::from_le_bytes([*head.get(pos)?, *head.get(pos + 1)?]) as usize;
        pos += 2 + xlen;
    }
    let mut name: Option<String> = None;
    if flags & 0x08 != 0 {
        let rest = head.get(pos..)?;
        let end = rest.iter().position(|&b| b == 0)?;
        let raw = &rest[..end];
        if !raw.is_empty() {
            let text = String::from_utf8_lossy(raw);
            if !text.chars().any(|c| c.is_control()) {
                name = Some(text.into_owned());
            }
        }
    }
    let tail = read_range_capped(storage, id, file_size - 4, file_size - 1, 4).await?;
    if tail.len() < 4 {
        return None;
    }
    let isize = u32::from_le_bytes([tail[0], tail[1], tail[2], tail[3]]) as u64;
    let display = match name {
        Some(n) if !n.trim().is_empty() => n,
        _ => {
            let stem = filename.rsplit_once('.').map(|(s, _)| s).unwrap_or(filename);
            if stem.trim().is_empty() {
                "archive contents".to_string()
            } else {
                format!("{stem} (decompressed)")
            }
        }
    };
    Some(vec![ZipEntry {
        name: display,
        size: Some(isize),
        is_dir: false,
        modified: None,
        encrypted: false,
        method: 0xFFFF,
        comp_size: 0,
        header_offset: 0,
    }])
}

#[cfg(test)]
mod tests {
    use super::*;

    
    fn sample_zip() -> Vec<u8> {
        let mut v = Vec::new();
        
        v.extend_from_slice(b"PK\x03\x04");
        v.extend_from_slice(&20u16.to_le_bytes()); 
        v.extend_from_slice(&0u16.to_le_bytes()); 
        v.extend_from_slice(&0u16.to_le_bytes()); 
        v.extend_from_slice(&0u16.to_le_bytes()); 
        v.extend_from_slice(&0u16.to_le_bytes()); 
        v.extend_from_slice(&0u32.to_le_bytes()); 
        v.extend_from_slice(&5u32.to_le_bytes()); 
        v.extend_from_slice(&5u32.to_le_bytes()); 
        v.extend_from_slice(&6u16.to_le_bytes()); 
        v.extend_from_slice(&0u16.to_le_bytes()); 
        v.extend_from_slice(b"hi.txt");
        v.extend_from_slice(b"hello");
        let cd_offset = v.len() as u32;
        
        v.extend_from_slice(b"PK\x01\x02");
        v.extend_from_slice(&20u16.to_le_bytes()); 
        v.extend_from_slice(&20u16.to_le_bytes()); 
        v.extend_from_slice(&0u16.to_le_bytes()); 
        v.extend_from_slice(&0u16.to_le_bytes()); 
        v.extend_from_slice(&0u16.to_le_bytes()); 
        v.extend_from_slice(&0u16.to_le_bytes()); 
        v.extend_from_slice(&0u32.to_le_bytes()); 
        v.extend_from_slice(&5u32.to_le_bytes()); 
        v.extend_from_slice(&5u32.to_le_bytes()); 
        v.extend_from_slice(&6u16.to_le_bytes()); 
        v.extend_from_slice(&0u16.to_le_bytes()); 
        v.extend_from_slice(&0u16.to_le_bytes()); 
        v.extend_from_slice(&0u16.to_le_bytes()); 
        v.extend_from_slice(&0u16.to_le_bytes()); 
        v.extend_from_slice(&0u32.to_le_bytes()); 
        v.extend_from_slice(&0u32.to_le_bytes()); 
        v.extend_from_slice(b"hi.txt");
        let cd_size = v.len() as u32 - cd_offset;
        
        v.extend_from_slice(b"PK\x05\x06");
        v.extend_from_slice(&0u16.to_le_bytes()); 
        v.extend_from_slice(&0u16.to_le_bytes()); 
        v.extend_from_slice(&1u16.to_le_bytes()); 
        v.extend_from_slice(&1u16.to_le_bytes()); 
        v.extend_from_slice(&cd_size.to_le_bytes());
        v.extend_from_slice(&cd_offset.to_le_bytes());
        v.extend_from_slice(&0u16.to_le_bytes()); 
        v
    }

    #[test]
    fn parses_single_entry() {
        let zip = sample_zip();
        let (off, size, count) = find_eocd(&zip, zip.len() as u64).unwrap();
        let entries = parse_entries(&zip[off as usize..(off + size) as usize], count).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name, "hi.txt");
        assert_eq!(entries[0].size, Some(5));
        assert!(!entries[0].is_dir);
        
        assert_eq!(entries[0].modified, None);
    }

    #[test]
    fn dos_stamps_format() {
        
        assert_eq!(dos_datetime((13u16 << 11) | (5u16 << 5), (44u16 << 9) | (1u16 << 5) | 15).as_deref(), Some("2024-01-15 13:05"));
        assert_eq!(dos_datetime(0, 0), None);
        assert_eq!(dos_datetime(0xFFFF, 0xFFFF), None);
    }

    #[test]
    fn rejects_garbage_and_truncation() {
        assert!(find_eocd(b"definitely not a zip file at all......", 40).is_none());
        let mut zip = sample_zip();
        zip.truncate(zip.len() - 10);
        assert!(find_eocd(&zip, 10_000).is_none());
        assert!(parse_entries(b"PK\x01\x02short", 1).is_none());
    }
}

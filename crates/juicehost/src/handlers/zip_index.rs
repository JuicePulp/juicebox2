use std::{collections::BTreeSet, sync::Arc, time::Instant};

use dashmap::DashMap;

use super::zip_list::{self, ZipEntry};
use crate::{state::AppState, storage::FileMetadata};

const MAX_CACHED_INDEXES: usize = 256;
const INDEX_TTL_SECS: u64 = 600;
const MAX_RATE_BUCKETS: usize = 4_096;

#[derive(Clone)]
pub(crate) struct IndexedEntry {
    pub index: usize,
    pub entry: ZipEntry,
    pub basename: String,
}

pub(crate) struct ZipIndex {
    pub archive_name: String,
    pub files: Vec<IndexedEntry>,
    pub folders: BTreeSet<String>,
}

pub(crate) enum Resolved {
    Root,
    Dir(String),
    File(IndexedEntry),
    Ambiguous(Vec<IndexedEntry>),
    NotFound,
}

pub(crate) struct CachedIndex {
    pub loaded: Instant,
    pub size: u64,
    pub etag: String,
    pub index: Arc<ZipIndex>,
}

pub(crate) type ZipIndexCache = DashMap<String, CachedIndex>;

impl ZipIndex {
    fn from_entries(archive_name: String, entries: Vec<ZipEntry>) -> Self {
        let mut files = Vec::new();
        let mut folders = BTreeSet::new();
        for entry in entries {
            let name = entry.name.trim_end_matches('/').to_string();
            if name.is_empty() {
                continue;
            }
            let segments: Vec<&str> = name.split('/').filter(|s| !s.is_empty()).collect();
            for i in 1..segments.len() {
                folders.insert(segments[..i].join("/"));
            }
            if entry.is_dir || entry.name.ends_with('/') {
                folders.insert(name);
                continue;
            }
            let basename = segments.last().copied().unwrap_or(&name).to_string();
            files.push(IndexedEntry {
                index: files.len() + 1,
                entry: ZipEntry { name, ..entry },
                basename,
            });
        }
        Self {
            archive_name,
            files,
            folders,
        }
    }

    pub(crate) fn total_size(&self) -> u64 {
        self.files.iter().filter_map(|f| f.entry.size).sum()
    }

    pub(crate) fn children(&self, folder: &str) -> (Vec<String>, Vec<&IndexedEntry>) {
        let folder = folder.trim_end_matches('/');
        let mut dirs = Vec::new();
        for candidate in &self.folders {
            let parent = candidate.rsplit_once('/').map_or("", |(parent, _)| parent);
            if parent == folder {
                dirs.push(
                    candidate
                        .rsplit('/')
                        .next()
                        .unwrap_or(candidate)
                        .to_string(),
                );
            }
        }
        let files = self
            .files
            .iter()
            .filter(|file| {
                file.entry
                    .name
                    .rsplit_once('/')
                    .map_or("", |(parent, _)| parent)
                    == folder
            })
            .collect();
        (dirs, files)
    }
}

pub(crate) async fn load_index(
    state: &Arc<AppState>,
    id: &str,
    meta: &FileMetadata,
) -> Option<Arc<ZipIndex>> {
    if let Some(cached) = state.zip_index_cache.get(id)
        && cached.size == meta.size
        && cached.etag == meta.etag
        && cached.loaded.elapsed().as_secs() < INDEX_TTL_SECS
    {
        return Some(Arc::clone(&cached.index));
    }

    let entries = zip_list::load_zip_entries(&state.storage, id, meta.size).await?;
    if entries.is_empty() {
        return None;
    }
    let archive_name = if meta.extension.is_empty() {
        id.to_string()
    } else {
        format!("{id}.{}", meta.extension)
    };
    let index = Arc::new(ZipIndex::from_entries(archive_name, entries));
    state.zip_index_cache.insert(
        id.to_string(),
        CachedIndex {
            loaded: Instant::now(),
            size: meta.size,
            etag: meta.etag.clone(),
            index: Arc::clone(&index),
        },
    );
    cleanup_cache(state);
    Some(index)
}

fn cleanup_cache(state: &AppState) {
    if state.zip_index_cache.len() <= MAX_CACHED_INDEXES {
        return;
    }
    let keys: Vec<String> = state
        .zip_index_cache
        .iter()
        .filter(|entry| entry.loaded.elapsed().as_secs() >= INDEX_TTL_SECS)
        .map(|entry| entry.key().clone())
        .collect();
    for key in keys {
        state.zip_index_cache.remove(&key);
    }
    while state.zip_index_cache.len() > MAX_CACHED_INDEXES {
        let Some(oldest) = state
            .zip_index_cache
            .iter()
            .min_by_key(|entry| entry.loaded)
            .map(|entry| entry.key().clone())
        else {
            break;
        };
        state.zip_index_cache.remove(&oldest);
    }
}

/// Parses `<archive>.zip/z/<selector>` out of a wildcard request path.
pub(crate) fn parse_zip_selector(path: &str) -> Option<(String, String)> {
    if let Some((archive, selector)) = path.split_once("/z/") {
        if archive.is_empty() {
            return None;
        }
        return Some((archive.to_string(), selector.to_string()));
    }
    path.strip_suffix("/z")
        .filter(|archive| !archive.is_empty())
        .map(|archive| (archive.to_string(), String::new()))
}

pub(crate) fn archive_id_from_url_path(archive_path: &str) -> Option<(String, String)> {
    let (id, ext) = match archive_path.split_once('.') {
        Some((id, ext)) if !id.is_empty() && !ext.is_empty() => (id, ext),
        _ => (archive_path, ""),
    };
    if !crate::storage::valid_component(id) {
        return None;
    }
    Some((id.to_string(), ext.to_string()))
}

pub(crate) fn valid_selector(selector: &str) -> bool {
    if selector.is_empty() {
        return true;
    }
    if selector.len() > 4096
        || selector.starts_with('/')
        || selector.contains('\\')
        || selector.chars().any(char::is_control)
    {
        return false;
    }
    let trimmed = selector.strip_suffix('/').unwrap_or(selector);
    if trimmed.is_empty() {
        return false;
    }
    !trimmed
        .split('/')
        .any(|segment| segment.is_empty() || segment == "." || segment == "..")
}

pub(crate) fn resolve(index: &ZipIndex, selector: &str) -> Resolved {
    if selector.is_empty() {
        return Resolved::Root;
    }
    if !valid_selector(selector) {
        return Resolved::NotFound;
    }

    if selector.chars().all(|c| c.is_ascii_digit()) {
        let Ok(number) = selector.parse::<usize>() else {
            return Resolved::NotFound;
        };
        return index
            .files
            .get(number.saturating_sub(1))
            .filter(|_| number > 0)
            .cloned()
            .map_or(Resolved::NotFound, Resolved::File);
    }

    let is_dir_request = selector.ends_with('/');
    let selector = selector.trim_end_matches('/');
    if is_dir_request {
        return if index.folders.contains(selector) {
            Resolved::Dir(selector.to_string())
        } else {
            Resolved::NotFound
        };
    }

    if let Some(entry) = index.files.iter().find(|e| e.entry.name == selector) {
        return Resolved::File(entry.clone());
    }
    let basename_matches: Vec<&IndexedEntry> = index
        .files
        .iter()
        .filter(|e| e.basename == selector)
        .collect();
    match basename_matches.len() {
        1 => return Resolved::File(basename_matches[0].clone()),
        n if n > 1 => return Resolved::Ambiguous(clone_entries(&basename_matches)),
        _ => {}
    }

    let extensionless: Vec<&IndexedEntry> = index
        .files
        .iter()
        .filter(|e| extensionless_match(&e.entry.name, selector))
        .collect();
    match extensionless.len() {
        1 => return Resolved::File(extensionless[0].clone()),
        n if n > 1 => return Resolved::Ambiguous(clone_entries(&extensionless)),
        _ => {}
    }

    let by_basename: Vec<&IndexedEntry> = index
        .files
        .iter()
        .filter(|e| extensionless_match(&e.basename, selector))
        .collect();
    match by_basename.len() {
        1 => return Resolved::File(by_basename[0].clone()),
        n if n > 1 => return Resolved::Ambiguous(clone_entries(&by_basename)),
        _ => {}
    }

    let lower = selector.to_ascii_lowercase();
    if let Some(entry) = index
        .files
        .iter()
        .find(|e| e.entry.name.to_ascii_lowercase() == lower)
    {
        return Resolved::File(entry.clone());
    }
    let lower_matches: Vec<&IndexedEntry> = index
        .files
        .iter()
        .filter(|e| e.basename.to_ascii_lowercase() == lower)
        .collect();
    match lower_matches.len() {
        1 => return Resolved::File(lower_matches[0].clone()),
        n if n > 1 => return Resolved::Ambiguous(clone_entries(&lower_matches)),
        _ => {}
    }

    if index.folders.contains(selector) {
        return Resolved::Dir(selector.to_string());
    }
    Resolved::NotFound
}

fn clone_entries(entries: &[&IndexedEntry]) -> Vec<IndexedEntry> {
    entries.iter().take(50).map(|e| (*e).clone()).collect()
}

fn extensionless_match(name: &str, selector: &str) -> bool {
    let Some(rest) = name.strip_prefix(selector) else {
        return false;
    };
    let Some(ext) = rest.strip_prefix('.') else {
        return false;
    };
    !ext.is_empty() && !ext.contains('/')
}

pub(crate) fn encode_entry_path(path: &str) -> String {
    use percent_encoding::{AsciiSet, CONTROLS, utf8_percent_encode};
    const SEGMENT: &AsciiSet = &CONTROLS
        .add(b' ')
        .add(b'"')
        .add(b'#')
        .add(b'%')
        .add(b'<')
        .add(b'>')
        .add(b'?')
        .add(b'`')
        .add(b'{')
        .add(b'}')
        .add(b'/')
        .add(b'\\');
    path.split('/')
        .map(|segment| utf8_percent_encode(segment, SEGMENT).to_string())
        .collect::<Vec<_>>()
        .join("/")
}

pub(crate) fn entry_urls(
    index: &ZipIndex,
    entry: &IndexedEntry,
) -> (String, String, String, String) {
    let encoded = encode_entry_path(&entry.entry.name);
    let base = format!("/{}", encode_entry_path(&index.archive_name));
    (
        format!("/v{base}/z/{encoded}"),
        format!("/f{base}/z/{encoded}"),
        format!("/d{base}/z/{encoded}"),
        format!("/v{base}/z/{}", entry.index),
    )
}

pub(crate) struct ZipRateLimiter {
    buckets: DashMap<String, RateBucket>,
}

struct RateBucket {
    tokens: f64,
    last: Instant,
}

impl ZipRateLimiter {
    pub(crate) const RATE_PER_SEC: f64 = 1.0;
    pub(crate) const BURST: f64 = 10.0;

    pub(crate) fn new() -> Self {
        Self {
            buckets: DashMap::new(),
        }
    }

    /// Returns `None` when allowed, or the retry delay in seconds.
    pub(crate) fn check(&self, key: Option<&str>) -> Option<u64> {
        let key = key?;
        if self.buckets.len() > MAX_RATE_BUCKETS {
            self.buckets
                .retain(|_, bucket| bucket.last.elapsed().as_secs() < 600);
        }
        let now = Instant::now();
        let mut bucket = self.buckets.entry(key.to_string()).or_insert(RateBucket {
            tokens: Self::BURST,
            last: now,
        });
        let elapsed = now.duration_since(bucket.last).as_secs_f64();
        bucket.tokens = (bucket.tokens + elapsed * Self::RATE_PER_SEC).min(Self::BURST);
        bucket.last = now;
        if bucket.tokens >= 1.0 {
            bucket.tokens -= 1.0;
            None
        } else {
            Some(((1.0 - bucket.tokens) / Self::RATE_PER_SEC).ceil().max(1.0) as u64)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(name: &str) -> ZipEntry {
        ZipEntry {
            name: name.to_string(),
            size: Some(10),
            is_dir: false,
            modified: None,
            encrypted: false,
            method: 0,
            comp_size: 10,
            header_offset: 0,
        }
    }

    fn index() -> ZipIndex {
        ZipIndex::from_entries(
            "abc.zip".into(),
            vec![
                entry("photo_01.jpg"),
                entry("clip_02.mp4"),
                entry("sub/photo_03.jpg"),
                entry("sub/deep/photo_04.png"),
                ZipEntry {
                    name: "empty/".into(),
                    size: None,
                    is_dir: true,
                    modified: None,
                    encrypted: false,
                    method: 0,
                    comp_size: 0,
                    header_offset: 0,
                },
            ],
        )
    }

    #[test]
    fn numeric_and_name_resolution() {
        let idx = index();
        assert!(matches!(resolve(&idx, "1"), Resolved::File(e) if e.entry.name == "photo_01.jpg"));
        assert!(
            matches!(resolve(&idx, "photo_01"), Resolved::File(e) if e.entry.name == "photo_01.jpg")
        );
        assert!(
            matches!(resolve(&idx, "sub/photo_03"), Resolved::File(e) if e.entry.name == "sub/photo_03.jpg")
        );
        assert!(matches!(resolve(&idx, "sub/"), Resolved::Dir(d) if d == "sub"));
        assert!(matches!(resolve(&idx, "empty/"), Resolved::Dir(d) if d == "empty"));
    }

    #[test]
    fn ambiguous_basename() {
        let idx =
            ZipIndex::from_entries("abc.zip".into(), vec![entry("a/x.jpg"), entry("b/x.jpg")]);
        assert!(matches!(resolve(&idx, "x.jpg"), Resolved::Ambiguous(v) if v.len() == 2));
    }

    #[test]
    fn selector_parser_and_traversal() {
        assert_eq!(
            parse_zip_selector("abc.zip/z/1"),
            Some(("abc.zip".into(), "1".into()))
        );
        assert_eq!(
            parse_zip_selector("abc.zip/z"),
            Some(("abc.zip".into(), String::new()))
        );
        assert!(!valid_selector("../x"));
        assert!(!valid_selector("a//b"));
        assert!(valid_selector("a/b/"));
    }

    #[test]
    fn urls_encode_segments() {
        assert_eq!(encode_entry_path("sub/a b#.jpg"), "sub/a%20b%23.jpg");
    }
}

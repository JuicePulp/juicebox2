
pub const DIR: &str = "<svg viewBox=\"0 0 24 24\" fill=\"currentColor\" aria-hidden=\"true\"><path d=\"M20 20H4v-2h16v2ZM4 18H2V6h2v12Zm18 0h-2V8h2v10ZM20 8H10V6H4V4h8v2h8v2Z\"/></svg>";
pub const FILE: &str = "<svg viewBox=\"0 0 24 24\" fill=\"currentColor\" aria-hidden=\"true\"><path d=\"M18 22H6v-2h12v2ZM6 20H4V4h2v16ZM16 4h-2v4h4V6h2v14h-2V10h-6V4H6V2h10v2Zm2 2h-2V4h2v2Z\"/></svg>";
pub const TEXT: &str = "<svg viewBox=\"0 0 24 24\" fill=\"currentColor\" aria-hidden=\"true\"><path d=\"M18 22H6v-2h12v2ZM6 20H4V4h2v16ZM16 4h-2v4h4V6h2v14h-2V10h-6V4H6V2h10v2Zm0 14H8v-2h8v2Zm0-4H8v-2h8v2Zm-6-4H8V8h2v2Zm8-4h-2V4h2v2Z\"/></svg>";
pub const IMAGE: &str = "<svg viewBox=\"0 0 24 24\" fill=\"currentColor\" aria-hidden=\"true\"><path d=\"M8 20h6v-2h2v2h4v2H4v-2h2v-2h2v2Zm-4 0H2V4h2v16Zm18 0h-2v-4h-2v-2h2V4h2v16Zm-12-2H8v-2h2v2Zm4 0h-2v-2h2v2Zm-2-2h-2v-2h2v2Zm4 0h-2v-2h2v2Zm2-2h-2v-2h2v2Zm-8-2H8v-2h2v2Zm-2-2H6V8h2v2Zm4 0h-2V8h2v2Zm-2-2H8V6h2v2Zm10-4H4V2h16v2Z\"/></svg>";
pub const VIDEO: &str = "<svg viewBox=\"0 0 24 24\" fill=\"currentColor\" aria-hidden=\"true\"><path d=\"M16 19H4v-2h12v2ZM4 17H2V7h2v10Zm14-8h2V7h2v10h-2v-2h-2v2h-2V7h2v2Zm-2-2H4V5h12v2Z\"/></svg>";
pub const AUDIO: &str = "<svg viewBox=\"0 0 24 24\" fill=\"currentColor\" aria-hidden=\"true\"><path d=\"M3 7h2v5H3zm4 0h2v13H7zm4-3h2v16h-2zm4 0h2v13h-2zM5 5h2v2H5zm4 15h2v2H9zm4-18h2v2h-2zm4 15h2v2h-2zm2-5h2v5h-2zm2-2h2v2h-2zM1 12h2v2H1z\"/></svg>";
pub const ARCHIVE: &str = "<svg viewBox=\"0 0 24 24\" fill=\"currentColor\" aria-hidden=\"true\"><path d=\"M3 2h18v2H3zm0 5h18v2H3zM1 4h2v3H1zm20 0h2v3h-2zm-2 5h2v11h-2zM3 9h2v11H3zm2 11h14v2H5zm4-9h6v2H9z\"/></svg>";
pub const DOWNLOAD_ART: &str = "<svg viewBox=\"0 0 24 24\" fill=\"currentColor\" aria-hidden=\"true\"><g><path d=\"M6 4H4v16h2zm10-2H6v2h10zm4 4h-2v14h2zm-2 14H6v2h12zM16 4h2v2h-2zm-4 0h2v6h-2z\"/><path d=\"M12 8h6v2h-6z\"/></g></svg>";

pub fn file_icon(name: &str, is_dir: bool) -> &'static str {
    if is_dir {
        return DIR;
    }
    match name.rsplit('.').next().unwrap_or("").to_ascii_lowercase().as_str() {
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "avif" | "bmp" | "ico" | "svg" => IMAGE,
        "mp4" | "webm" | "mov" | "mkv" | "avi" | "m4v" => VIDEO,
        "mp3" | "wav" | "ogg" | "oga" | "flac" | "m4a" | "opus" => AUDIO,
        "txt" | "md" | "json" | "xml" | "html" | "css" | "js" | "ts" | "rs" | "py" | "sh"
        | "yml" | "yaml" | "toml" | "log" | "csv" | "ini" | "cfg" => TEXT,
        "zip" | "rar" | "7z" | "tar" | "gz" | "bz2" | "xz" | "zst" => ARCHIVE,
        _ => FILE,
    }
}

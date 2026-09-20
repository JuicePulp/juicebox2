use sha2::{Digest, Sha256};

#[must_use]
pub fn extract_extension(filename: &str) -> &str {
    std::path::Path::new(filename)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("bin")
}

pub fn valid_component(value: &str) -> bool {
    juiceutils::ids::is_valid_id_component(value)
}

pub fn capability_hash(capability: &str) -> String {
    hex::encode(Sha256::digest(capability.as_bytes()))
}

pub fn safe_extension(filename: &str) -> String {
    let ext = extract_extension(filename);
    if valid_component(ext) {
        ext.to_ascii_lowercase()
    } else {
        "bin".into()
    }
}

/// Guess the MIME type from a file extension.
#[must_use]
pub fn guess_mime(ext: &str) -> String {
    let mime = mime_guess::from_path(format!("file.{ext}")).first_or_octet_stream();
    if mime.type_() == mime_guess::mime::TEXT {
        format!("{mime}; charset=utf-8")
    } else {
        mime.to_string()
    }
}

pub const REPO_URL: &str = "https://github.com/JuicePulp/juicebox2";

/// Git ref baked in at compile time; `unknown` when the build did not set it.
pub fn commit_ref() -> &'static str {
    option_env!("JUICEFRONT_COMMIT_REF").unwrap_or("unknown")
}

pub fn commit_hash() -> &'static str {
    option_env!("JUICEFRONT_COMMIT_HASH").unwrap_or("unknown")
}

pub fn commit_short_hash() -> String {
    let hash = commit_hash();
    if hash == "unknown" {
        return "unknown".to_owned();
    }
    hash.chars().take(7).collect()
}

pub fn commit_label() -> String {
    commit_ref().to_owned()
}

/// schema.org `SoftwareSourceCode` JSON-LD; contains no user input.
pub fn structured_data_json() -> String {
    serde_json::json!({
        "@context": "https://schema.org",
        "@type": "SoftwareSourceCode",
        "name": "Juicebox2",
        "version": commit_short_hash(),
        "programmingLanguage": "Rust",
        "codeRepository": REPO_URL,
        "license": "https://opensource.org/licenses/MIT",
        "source": {
            "@type": "URL",
            "@id": REPO_URL,
        },
    })
    .to_string()
}

pub const REPO_URL: &str = "https://github.com/create-juicey-app/juicebox2";

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

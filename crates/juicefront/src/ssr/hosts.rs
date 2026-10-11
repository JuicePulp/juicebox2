use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HostNode {
    pub name: String,
    pub url: String,
    pub region: String,
    pub official: bool,
    #[serde(default = "default_host_kind")]
    pub kind: String,
}

fn default_host_kind() -> String {
    String::from("storage")
}

const OFFICIAL_DEFAULTS: &[(&str, &str, &str, &str)] = &[
    ("Default", "", "", "storage"),
    ("Router", "https://f.juicey.dev", "", "router"),
    ("Europe-1", "https://f-eu1.juicey.dev", "Europe", "storage"),
    ("Europe-2", "https://f-eu2.juicey.dev", "Europe", "storage"),
];

fn normalize_url(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    let with_scheme = if trimmed.contains("://") {
        trimmed.to_owned()
    } else {
        format!("https://{trimmed}")
    };
    let parsed: url::Url = with_scheme.parse().ok()?;
    if parsed.scheme() != "http" && parsed.scheme() != "https" {
        return None;
    }
    Some(format!("{}://{}", parsed.scheme(), parsed.host_str()?))
}

fn parse_env_list(raw: Option<String>, official: bool) -> Vec<HostNode> {
    let Some(raw) = raw else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for entry in raw.split(',') {
        let mut parts = entry.split('|').map(str::trim);
        let name = parts.next().unwrap_or("");
        let url = parts.next().unwrap_or("");
        let region = parts.next().unwrap_or("").to_owned();
        let kind = parts.next().unwrap_or("").to_owned();
        let Some(base) = normalize_url(url) else {
            continue;
        };
        out.push(HostNode {
            name: if name.is_empty() {
                base.clone()
            } else {
                name.to_owned()
            },
            url: base,
            region,
            official,
            kind: if kind == "router" {
                kind
            } else {
                default_host_kind()
            },
        });
    }
    out
}

fn merge_nodes(base: Vec<HostNode>, extra: Vec<HostNode>) -> Vec<HostNode> {
    let mut seen: Vec<String> = base.iter().map(|node| node.url.clone()).collect();
    let mut out = base;
    for node in extra {
        if seen.iter().any(|url| url == &node.url) {
            continue;
        }
        seen.push(node.url.clone());
        out.push(node);
    }
    out
}

pub fn official_nodes() -> Vec<HostNode> {
    let base: Vec<HostNode> = OFFICIAL_DEFAULTS
        .iter()
        .map(|(name, url, region, kind)| HostNode {
            name: (*name).to_owned(),
            url: (*url).to_owned(),
            region: (*region).to_owned(),
            official: true,
            kind: (*kind).to_owned(),
        })
        .collect();
    let extra = juiceutils::config::optional_secret("PUBLIC_OFFICIAL_HOSTS")
        .or_else(|| juiceutils::config::optional_secret("JUICEFRONT_OFFICIAL_HOSTS"));
    merge_nodes(base, parse_env_list(extra, true))
}

pub fn unofficial_nodes() -> Vec<HostNode> {
    let extra = juiceutils::config::optional_secret("PUBLIC_UNOFFICIAL_HOSTS")
        .or_else(|| juiceutils::config::optional_secret("JUICEFRONT_UNOFFICIAL_HOSTS"));
    merge_nodes(Vec::new(), parse_env_list(extra, false))
}

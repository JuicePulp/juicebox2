use juiceutils::host::normalize_host;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HostNode {
    pub name: String,
    pub url: String,
    pub region: String,
    #[serde(default = "default_host_kind")]
    pub kind: String,
}

fn default_host_kind() -> String {
    String::from("storage")
}

const OFFICIAL_DEFAULTS: &[(&str, &str, &str, &str)] = &[
    ("Europe-1", "https://f-eu1.juicey.dev", "Europe", "storage"),
    ("Europe-2", "https://f-eu2.juicey.dev", "Europe", "storage"),
    (
        "US-1",
        "https://f-us1.juicey.dev",
        "United States",
        "storage",
    ),
];

// Entries are comma-separated `name|url|region|kind`; bad URLs are dropped.
fn parse_env_list(raw: Option<String>) -> Vec<HostNode> {
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
        let Some(base) = normalize_host(url) else {
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
            kind: if kind == "router" {
                kind
            } else {
                default_host_kind()
            },
        });
    }
    out
}

// Dedupe by URL so env-supplied hosts cannot shadow built-ins.
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
            kind: (*kind).to_owned(),
        })
        .collect();
    let extra = juiceutils::config::optional_secret("PUBLIC_OFFICIAL_HOSTS")
        .or_else(|| juiceutils::config::optional_secret("JUICEFRONT_OFFICIAL_HOSTS"));
    merge_nodes(base, parse_env_list(extra))
}

pub fn unofficial_nodes() -> Vec<HostNode> {
    let extra = juiceutils::config::optional_secret("PUBLIC_UNOFFICIAL_HOSTS")
        .or_else(|| juiceutils::config::optional_secret("JUICEFRONT_UNOFFICIAL_HOSTS"));
    merge_nodes(Vec::new(), parse_env_list(extra))
}

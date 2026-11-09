use std::collections::HashSet;

use crate::config::Config;

/// The deployment's public identities, derived once from `Config`.
#[derive(Debug, Clone, Default)]
pub struct DeploymentHosts {
    official: HashSet<String>,
    ban_targets: Vec<String>,
}

fn trimmed(raw: &str) -> Option<String> {
    let value = raw.trim().trim_end_matches('/');
    (!value.is_empty()).then(|| value.to_owned())
}

impl DeploymentHosts {
    #[must_use]
    pub fn from_config(config: &Config) -> Self {
        let mut official = HashSet::new();
        for raw in [
            &config.juicehost_url,
            &config.public_base_url,
            &config.public_juicehost_url,
        ] {
            if let Some(host) = trimmed(raw) {
                official.insert(host);
            }
        }
        if let Some(host) = config.router_base_url.as_deref().and_then(trimmed) {
            official.insert(host);
        }

        let mut ban_targets = Vec::new();
        if let Some(host) = trimmed(&config.juicehost_url) {
            ban_targets.push(host);
        }
        let mut regions: Vec<String> = config
            .region_public_juicehosts
            .values()
            .filter_map(|raw| trimmed(raw))
            .collect();
        regions.sort_unstable();
        regions.dedup();
        for host in regions {
            official.insert(host.clone());
            if !ban_targets.contains(&host) {
                ban_targets.push(host);
            }
        }

        Self {
            official,
            ban_targets,
        }
    }

    /// Empty hosts are not trusted; used to scope internal auth headers.
    #[must_use]
    pub fn is_trusted_target(&self, host: &str) -> bool {
        trimmed(host).is_some_and(|host| self.official.contains(&host))
    }

    /// A missing or empty storage host means "use the public host"; used for
    /// share links.
    #[must_use]
    pub fn is_share_host(&self, host: Option<&str>) -> bool {
        match host.and_then(trimmed) {
            None => true,
            Some(host) => self.official.contains(&host),
        }
    }

    /// Hosts that receive ban snapshots: the primary juicehost and every
    /// region, without the public frontend or router.
    #[must_use]
    pub fn ban_targets(&self) -> &[String] {
        &self.ban_targets
    }
}

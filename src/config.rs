//! Configuration loaded from a `key = value` file (default
//! `/etc/monit/monit.conf`, overridable via `MONIT_CONFIG`). Keeps all
//! site-specific values (hostnames, SSH targets) out of the source tree so the
//! repository can be public. Environment variables still override the file.
//!
//! Recognized keys (with their env override):
//!   ai_host    MONIT_AI_HOST     ssh target for the remote GPU host
//!   pve_label  MONIT_PVE_LABEL   panel label for the local host
//!   ai_label   MONIT_AI_LABEL    panel label for the remote host
//!   refresh    MONIT_REFRESH     data refresh seconds
//!   page_secs  MONIT_PAGE_SECS   seconds per page before rotating
//!   top        MONIT_TOP         rows of top consumers per host
//!   temp_unit  MONIT_TEMP_UNIT   C or F

use std::collections::HashMap;
use std::fs;

pub struct Config {
    map: HashMap<String, String>,
}

impl Config {
    pub fn load() -> Config {
        let path = std::env::var("MONIT_CONFIG").unwrap_or_else(|_| "/etc/monit/monit.conf".to_string());
        let mut map = HashMap::new();
        if let Ok(text) = fs::read_to_string(&path) {
            for line in text.lines() {
                let line = line.trim();
                if line.is_empty() || line.starts_with('#') {
                    continue;
                }
                if let Some((k, v)) = line.split_once('=') {
                    map.insert(k.trim().to_lowercase(), v.trim().to_string());
                }
            }
        }
        Config { map }
    }

    /// File/env lookup: env override wins, then the config file.
    fn lookup(&self, key: &str, env: &str) -> Option<String> {
        std::env::var(env).ok().filter(|s| !s.is_empty()).or_else(|| self.map.get(key).cloned())
    }

    pub fn string(&self, key: &str, env: &str, default: &str) -> String {
        self.lookup(key, env).unwrap_or_else(|| default.to_string())
    }

    pub fn opt(&self, key: &str, env: &str) -> Option<String> {
        self.lookup(key, env)
    }

    pub fn parse<T: std::str::FromStr>(&self, key: &str, env: &str, default: T) -> T {
        self.lookup(key, env).and_then(|s| s.parse().ok()).unwrap_or(default)
    }
}

/// The local system hostname, used as the default label for the local panel.
pub fn hostname() -> String {
    fs::read_to_string("/proc/sys/kernel/hostname")
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|_| "localhost".to_string())
}

/// Host portion of an ssh target ("root@host" -> "host").
pub fn host_part(ssh_target: &str) -> String {
    ssh_target.rsplit('@').next().unwrap_or(ssh_target).to_string()
}

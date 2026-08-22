//! Read-only active Agent names from the W2-04 installed session bridge.
//!
//! The lease writer remains `scripts/bus-demux.py`. STT only consumes a bounded,
//! expiring snapshot: no helper process, lock, deletion, or heartbeat mutation.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use directories::BaseDirs;
use serde::Deserialize;

const LEASE_SCHEMA: &str = "codescribe.agent-bridge.lease.v1";
const BRIDGE_HOME_ENV: &str = "CODESCRIBE_AGENT_BRIDGE_HOME";
const LEASE_TTL_SECONDS: f64 = 120.0;
const MAX_LEASE_FILES: usize = 64;
const MAX_LEASE_BYTES: u64 = 16 * 1024;
const MAX_ACTIVE_NAMES: usize = 16;
const CACHE_FOR: Duration = Duration::from_secs(1);

#[derive(Debug, Deserialize)]
struct SessionLease {
    schema: String,
    name: Option<String>,
    active: bool,
    heartbeat_unix: f64,
}

#[derive(Default)]
struct NameCache {
    root: PathBuf,
    refreshed_at: Option<Instant>,
    names: Vec<String>,
}

static ACTIVE_NAMES: OnceLock<Mutex<NameCache>> = OnceLock::new();

/// Current bounded active-name snapshot. Errors and stale leases fail open.
pub fn active_names() -> Vec<String> {
    let root = bridge_home();
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs_f64())
        .unwrap_or(0.0);
    let cache = ACTIVE_NAMES.get_or_init(|| Mutex::new(NameCache::default()));
    let mut cache = cache.lock().unwrap_or_else(|error| error.into_inner());
    if cache.root == root
        && cache
            .refreshed_at
            .is_some_and(|refreshed| refreshed.elapsed() < CACHE_FOR)
    {
        return cache.names.clone();
    }
    cache.root = root.clone();
    cache.names = read_active_names_at(&root, now, LEASE_TTL_SECONDS);
    cache.refreshed_at = Some(Instant::now());
    cache.names.clone()
}

fn bridge_home() -> PathBuf {
    if let Ok(value) = std::env::var(BRIDGE_HOME_ENV) {
        let value = value.trim();
        if !value.is_empty() {
            return expand_tilde(value);
        }
    }
    BaseDirs::new()
        .map(|dirs| dirs.home_dir().join(".codescribe/agent-bridge"))
        .unwrap_or_else(|| PathBuf::from(".codescribe/agent-bridge"))
}

fn expand_tilde(path: &str) -> PathBuf {
    if path == "~" {
        return BaseDirs::new()
            .map(|dirs| dirs.home_dir().to_path_buf())
            .unwrap_or_else(|| PathBuf::from(path));
    }
    if let Some(relative) = path.strip_prefix("~/") {
        return BaseDirs::new()
            .map(|dirs| dirs.home_dir().join(relative))
            .unwrap_or_else(|| PathBuf::from(path));
    }
    PathBuf::from(path)
}

fn canonical_name(value: &str) -> Option<String> {
    let value = value.trim();
    let count = value.chars().count();
    if !(2..=32).contains(&count) || !value.chars().all(char::is_alphabetic) {
        return None;
    }
    let mut chars = value.chars();
    let first = chars.next()?;
    Some(first.to_uppercase().chain(chars).collect())
}

fn read_active_names_at(root: &Path, now: f64, ttl_seconds: f64) -> Vec<String> {
    let Ok(entries) = fs::read_dir(root.join("leases")) else {
        return Vec::new();
    };
    let mut paths = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "json")
        })
        .collect::<Vec<_>>();
    paths.sort();
    paths.truncate(MAX_LEASE_FILES);

    let mut seen = HashSet::new();
    let mut names = Vec::new();
    for path in paths {
        let Ok(metadata) = fs::metadata(&path) else {
            continue;
        };
        if metadata.len() > MAX_LEASE_BYTES {
            continue;
        }
        let Ok(bytes) = fs::read(&path) else {
            continue;
        };
        let Ok(lease) = serde_json::from_slice::<SessionLease>(&bytes) else {
            continue;
        };
        let age = now - lease.heartbeat_unix;
        if lease.schema != LEASE_SCHEMA
            || !lease.active
            || !age.is_finite()
            || age < 0.0
            || age > ttl_seconds
        {
            continue;
        }
        let Some(name) = lease.name.as_deref().and_then(canonical_name) else {
            continue;
        };
        if seen.insert(name.to_lowercase()) {
            names.push(name);
            if names.len() == MAX_ACTIVE_NAMES {
                break;
            }
        }
    }
    names
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_lease(root: &Path, file: &str, name: &str, active: bool, heartbeat: f64) {
        let dir = root.join("leases");
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join(file),
            serde_json::to_vec(&serde_json::json!({
                "schema": LEASE_SCHEMA,
                "name": name,
                "active": active,
                "heartbeat_unix": heartbeat,
            }))
            .unwrap(),
        )
        .unwrap();
    }

    #[test]
    fn active_names_are_bounded_deduplicated_and_expire() {
        let temp = tempfile::tempdir().unwrap();
        write_lease(temp.path(), "a.json", "iwo", true, 990.0);
        write_lease(temp.path(), "b.json", "IWO", true, 995.0);
        write_lease(temp.path(), "c.json", "stary", true, 800.0);
        write_lease(temp.path(), "d.json", "zamkniety", false, 999.0);
        write_lease(temp.path(), "e.json", "piwo trzy", true, 999.0);

        assert_eq!(read_active_names_at(temp.path(), 1_000.0, 120.0), ["Iwo"]);
    }

    #[test]
    fn malformed_or_future_lease_fails_open() {
        let temp = tempfile::tempdir().unwrap();
        fs::create_dir_all(temp.path().join("leases")).unwrap();
        fs::write(temp.path().join("leases/bad.json"), b"not json").unwrap();
        write_lease(temp.path(), "future.json", "Iwo", true, 1_001.0);
        assert!(read_active_names_at(temp.path(), 1_000.0, 120.0).is_empty());
    }
}

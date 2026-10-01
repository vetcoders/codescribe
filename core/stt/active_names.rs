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
/// Shared follower-liveness window: a heartbeat older than this is dead.
/// Mirrors the helper's `DEFAULT_LEASE_TTL_SECONDS`.
pub const LEASE_TTL_SECONDS: f64 = 120.0;
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
    #[serde(default)]
    provider: Option<String>,
    #[serde(default)]
    provider_session_id: Option<String>,
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

/// Runtime agent-bridge home: `CODESCRIBE_AGENT_BRIDGE_HOME`, otherwise
/// `~/.codescribe/agent-bridge`. The audience binding file sits here, beside
/// the leases the Bus followers already use. Test builds default under the
/// per-process temporary config root instead, so tests never observe the
/// account's real bridge leases.
pub fn bridge_home() -> PathBuf {
    if let Ok(value) = std::env::var(BRIDGE_HOME_ENV) {
        let value = value.trim();
        if !value.is_empty() {
            return expand_tilde(value);
        }
    }
    #[cfg(any(test, feature = "test-isolation"))]
    {
        crate::test_isolation::test_process_config_root().join("agent-bridge")
    }
    #[cfg(not(any(test, feature = "test-isolation")))]
    {
        BaseDirs::new()
            .map(|dirs| dirs.home_dir().join(".codescribe/agent-bridge"))
            .unwrap_or_else(|| PathBuf::from(".codescribe/agent-bridge"))
    }
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
    Some(
        first
            .to_uppercase()
            .chain(chars.flat_map(char::to_lowercase))
            .collect(),
    )
}

/// Provider sessions whose follower is live right now: lease schema, `active`,
/// and a heartbeat no older than the shared TTL — the same convention the
/// helper's `--status` reports. Pairs are `(provider, provider_session_id)`
/// with the provider casefolded, matching what the lease writer stores.
pub fn live_follower_sessions_at(
    root: &Path,
    now: f64,
    ttl_seconds: f64,
) -> HashSet<(String, String)> {
    let mut sessions = HashSet::new();
    for_each_fresh_lease(root, now, ttl_seconds, |lease| {
        if let (Some(provider), Some(session)) = (
            lease.provider.as_deref().map(str::trim),
            lease.provider_session_id.as_deref().map(str::trim),
        ) && !provider.is_empty()
            && !session.is_empty()
        {
            sessions.insert((provider.to_lowercase(), session.to_string()));
        }
    });
    sessions
}

/// Bounded scan of the lease directory, invoking `visit` for every lease that
/// is well-formed, active, and heartbeat-fresh. Everything else fails open.
fn for_each_fresh_lease(
    root: &Path,
    now: f64,
    ttl_seconds: f64,
    mut visit: impl FnMut(&SessionLease),
) {
    let Ok(entries) = fs::read_dir(root.join("leases")) else {
        return;
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
        visit(&lease);
    }
}

fn read_active_names_at(root: &Path, now: f64, ttl_seconds: f64) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut names = Vec::new();
    for_each_fresh_lease(root, now, ttl_seconds, |lease| {
        if names.len() == MAX_ACTIVE_NAMES {
            return;
        }
        let Some(name) = lease.name.as_deref().and_then(canonical_name) else {
            return;
        };
        if seen.insert(name.to_lowercase()) {
            names.push(name);
        }
    });
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

    fn write_session_lease(
        root: &Path,
        file: &str,
        provider: &str,
        session: &str,
        active: bool,
        heartbeat: f64,
    ) {
        let dir = root.join("leases");
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join(file),
            serde_json::to_vec(&serde_json::json!({
                "schema": LEASE_SCHEMA,
                "name": "iwo",
                "active": active,
                "heartbeat_unix": heartbeat,
                "provider": provider,
                "provider_session_id": session,
            }))
            .unwrap(),
        )
        .unwrap();
    }

    #[test]
    fn a_live_follower_session_is_reported_and_a_stale_one_is_not() {
        let temp = tempfile::tempdir().unwrap();
        write_session_lease(
            temp.path(),
            "live.json",
            "Claude-Code",
            "sess-1",
            true,
            990.0,
        );
        write_session_lease(temp.path(), "stale.json", "codex", "sess-2", true, 800.0);
        write_session_lease(temp.path(), "closed.json", "codex", "sess-3", false, 999.0);
        // A lease without provider fields (pre-W2 shape) is skipped, not an error.
        write_lease(temp.path(), "nameless.json", "iwo", true, 999.0);

        let live = live_follower_sessions_at(temp.path(), 1_000.0, 120.0);
        assert_eq!(
            live,
            HashSet::from([("claude-code".to_string(), "sess-1".to_string())])
        );
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

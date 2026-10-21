//! Read-only active Agent names from the W2-04 installed session bridge.
//!
//! The lease writer remains `scripts/bus-demux.py`. STT only consumes a bounded,
//! expiring snapshot: no helper process, lock, deletion, or heartbeat mutation.

use std::collections::{HashMap, HashSet};
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
/// A lease is not just a name: the helper also persists the follower's delivery
/// mailbox and its unclosed channel documents in the same file, so a healthy
/// lease routinely reaches hundreds of kilobytes. The bound mirrors the
/// helper's own mailbox bound and the Swift overlay reader
/// (`OverlayChannelDeliveryReader.object(at:)`); a smaller cap would make a
/// live follower read as absent instead of rejecting a corrupt file.
const MAX_LEASE_BYTES: u64 = 16 * 1024 * 1024;
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
    #[serde(default)]
    lease_id: Option<String>,
    /// The follower's mailbox. Acknowledged envelopes linger here until its
    /// next sweep, so this list alone overstates the backlog; the marker
    /// store under `acknowledgments/<lease_id>/` is the receipt truth.
    #[serde(default)]
    pending: Vec<PendingDelivery>,
    /// The vc-frame pane the follower inherited at attach (`frame_context`
    /// in the helper), or null when its process held no pane coordinates.
    #[serde(default)]
    frame: Option<serde_json::Value>,
    /// The iTerm2 or Terminal.app tab hosting the session (`terminal_context`
    /// in the helper), or null when neither terminal was identified.
    #[serde(default)]
    terminal: Option<serde_json::Value>,
    /// The Codex app thread of a Codex follower (`codex_context` in the
    /// helper): `thread_id`, `app_bundle_id`, `rollout`, `remote`; null for
    /// every other provider.
    #[serde(default)]
    codex: Option<serde_json::Value>,
}

impl SessionLease {
    /// A pane the follower can focus: a numeric `pane_id` and a session name.
    fn has_frame_pane(&self) -> bool {
        self.frame.as_ref().is_some_and(|frame| {
            frame.get("pane_id").is_some_and(serde_json::Value::is_u64)
                && frame
                    .get("session")
                    .and_then(serde_json::Value::as_str)
                    .is_some_and(|session| !session.trim().is_empty())
        })
    }

    /// A terminal tab the follower can focus: a non-empty `host_bundle_id`.
    fn has_terminal_host(&self) -> bool {
        self.terminal.as_ref().is_some_and(|terminal| {
            terminal
                .get("host_bundle_id")
                .and_then(serde_json::Value::as_str)
                .is_some_and(|bundle| !bundle.trim().is_empty())
        })
    }

    /// The Codex app thread the lease names. The id goes into a
    /// `codex://threads/<id>` link, so only the helper's own id alphabet
    /// (8-128 letters, digits, `-`, `_`) is accepted.
    fn codex_thread(&self) -> Option<CodexThread> {
        let codex = self.codex.as_ref()?;
        let thread_id = codex.get("thread_id")?.as_str()?.trim();
        let well_formed = (8..=128).contains(&thread_id.len())
            && thread_id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_');
        if !well_formed {
            return None;
        }
        Some(CodexThread {
            thread_id: thread_id.to_string(),
            rollout: codex
                .get("rollout")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false),
        })
    }
}

/// A Codex follower's thread in the Codex desktop app. Unlike the host
/// facts, it does not need a live follower: the app opens the thread itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodexThread {
    /// The Codex thread id, which is the follower's provider session id.
    pub thread_id: String,
    /// The local Codex store holds the thread's rollout, so the app can open
    /// it. A Codex Cloud thread has none and the app answers "no rollout
    /// found" for its link.
    pub rollout: bool,
}

/// Where a live follower can bring its agent to the front. Both facts come
/// from the same lease; neither changes whether the follower is alive.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FollowerHosts {
    /// The lease names a vc-frame pane.
    pub frame_pane: bool,
    /// The lease names an iTerm2 or Terminal.app tab.
    pub terminal_tab: bool,
}

#[derive(Debug, Deserialize)]
struct PendingDelivery {
    #[serde(default)]
    delivery_id: Option<String>,
    #[serde(default)]
    kind: Option<String>,
    #[serde(default)]
    emitted_at: Option<String>,
}

/// Envelope kinds that wait for a reader: sealed takes and typed messages.
/// Draft revisions are not deliveries (the helper's `TERMINAL_KINDS`).
const TERMINAL_KINDS: [&str; 2] = ["seal", "message"];

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
    live_follower_hosts_at(root, now, ttl_seconds)
        .into_keys()
        .collect()
}

/// Every live follower session (same convention as
/// [`live_follower_sessions_at`]) mapped to whether its lease names a vc-frame
/// pane and a terminal tab. One scan serves the liveness and the host facts
/// the roster shows.
pub fn live_follower_hosts_at(
    root: &Path,
    now: f64,
    ttl_seconds: f64,
) -> HashMap<(String, String), FollowerHosts> {
    let mut sessions: HashMap<(String, String), FollowerHosts> = HashMap::new();
    for_each_fresh_lease(root, now, ttl_seconds, |lease| {
        if let (Some(provider), Some(session)) = (
            lease.provider.as_deref().map(str::trim),
            lease.provider_session_id.as_deref().map(str::trim),
        ) && !provider.is_empty()
            && !session.is_empty()
        {
            let hosts = sessions
                .entry((provider.to_lowercase(), session.to_string()))
                .or_default();
            hosts.frame_pane |= lease.has_frame_pane();
            hosts.terminal_tab |= lease.has_terminal_host();
        }
    });
    sessions
}

/// Every provider session whose newest lease (highest `heartbeat_unix`)
/// names a Codex app thread, whether or not its follower is alive: Codex app
/// followers are usually dead between turns, and opening the thread needs no
/// follower. Pairs are keyed like [`live_follower_hosts_at`].
pub fn codex_threads_at(root: &Path) -> HashMap<(String, String), CodexThread> {
    let mut newest: HashMap<(String, String), (f64, Option<CodexThread>)> = HashMap::new();
    for_each_lease_summary(root, |summary| {
        if newest
            .get(&summary.session)
            .is_some_and(|(heartbeat, _)| *heartbeat >= summary.heartbeat_unix)
        {
            return;
        }
        newest.insert(
            summary.session.clone(),
            (summary.heartbeat_unix, summary.codex_thread.clone()),
        );
    });
    newest
        .into_iter()
        .filter_map(|(session, (_, thread))| thread.map(|thread| (session, thread)))
        .collect()
}

/// For every live follower session, when its oldest unacknowledged delivery
/// was emitted (Unix seconds). A session with no backlog is absent. This is
/// the second liveness layer the overlay shows: a follower process can be
/// alive and heartbeating while the conversation behind it reads nothing
/// (Founder, 2026-10-14: "mogę do niego napisać, ale on nie odbiera").
/// Unacknowledged = no marker file for the delivery id; the helper's deeper
/// marker verification is receipt truth, this is a display hint.
pub fn unread_backlog_since_at(
    root: &Path,
    now: f64,
    ttl_seconds: f64,
) -> HashMap<(String, String), f64> {
    let mut oldest = HashMap::new();
    for_each_fresh_lease(root, now, ttl_seconds, |lease| {
        let (Some(provider), Some(session), Some(lease_id)) = (
            lease.provider.as_deref().map(str::trim),
            lease.provider_session_id.as_deref().map(str::trim),
            lease.lease_id.as_deref().map(str::trim),
        ) else {
            return;
        };
        if provider.is_empty() || session.is_empty() || lease_id.is_empty() {
            return;
        }
        let markers = root.join("acknowledgments").join(lease_id);
        let since = lease
            .pending
            .iter()
            .filter(|entry| {
                entry
                    .kind
                    .as_deref()
                    .is_some_and(|kind| TERMINAL_KINDS.contains(&kind))
            })
            .filter(|entry| {
                entry
                    .delivery_id
                    .as_deref()
                    .is_some_and(|id| !markers.join(format!("{id}.json")).is_file())
            })
            .filter_map(|entry| entry.emitted_at.as_deref())
            .filter_map(|stamp| {
                chrono::DateTime::parse_from_rfc3339(stamp)
                    .ok()
                    .map(|time| time.timestamp_millis() as f64 / 1000.0)
            })
            .fold(None, |oldest: Option<f64>, stamp| {
                Some(oldest.map_or(stamp, |current| current.min(stamp)))
            });
        if let Some(since) = since {
            oldest.insert((provider.to_lowercase(), session.to_string()), since);
        }
    });
    oldest
}

/// The bounded set of lease files: the first `MAX_LEASE_FILES` `*.json`
/// names in order, each no larger than the helper can write, with the
/// metadata read for that check.
fn lease_files(root: &Path) -> Vec<(PathBuf, fs::Metadata)> {
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
    paths
        .into_iter()
        .filter_map(|path| {
            let metadata = fs::metadata(&path).ok()?;
            (metadata.len() <= MAX_LEASE_BYTES).then_some((path, metadata))
        })
        .collect()
}

/// One lease file parsed as the helper wrote it, or `None` when it is
/// unreadable, malformed or of another schema.
fn read_lease(path: &Path) -> Option<SessionLease> {
    let bytes = fs::read(path).ok()?;
    let lease = serde_json::from_slice::<SessionLease>(&bytes).ok()?;
    (lease.schema == LEASE_SCHEMA).then_some(lease)
}

/// What [`codex_threads_at`] needs from one lease file, whatever its
/// heartbeat age.
struct LeaseSummary {
    session: (String, String),
    heartbeat_unix: f64,
    codex_thread: Option<CodexThread>,
}

/// A lease file's summary, valid while its length and modification time stay
/// the same.
struct CachedLeaseSummary {
    len: u64,
    modified: Option<SystemTime>,
    summary: Option<LeaseSummary>,
}

/// Summaries of the lease files last scanned, by path. The roster polls
/// twice a second and dead followers keep multi-megabyte mailboxes that never
/// change again; a file is parsed only when it was rewritten.
static LEASE_SUMMARIES: OnceLock<Mutex<HashMap<PathBuf, CachedLeaseSummary>>> = OnceLock::new();

/// Bounded scan of every well-formed lease with a provider session, live or
/// dead, invoking `visit` with its summary. Unchanged files are not parsed
/// again. Everything else fails open.
fn for_each_lease_summary(root: &Path, mut visit: impl FnMut(&LeaseSummary)) {
    let files = lease_files(root);
    let cache = LEASE_SUMMARIES.get_or_init(|| Mutex::new(HashMap::new()));
    let mut cache = cache.lock().unwrap_or_else(|error| error.into_inner());
    let scanned: HashSet<PathBuf> = files.iter().map(|(path, _)| path.clone()).collect();
    cache.retain(|path, _| !path.starts_with(root) || scanned.contains(path));
    for (path, metadata) in files {
        let len = metadata.len();
        let modified = metadata.modified().ok();
        let current = cache
            .get(&path)
            .is_some_and(|cached| cached.len == len && cached.modified == modified);
        if !current {
            let summary = read_lease(&path).and_then(|lease| {
                let provider = lease.provider.as_deref()?.trim();
                let session = lease.provider_session_id.as_deref()?.trim();
                if provider.is_empty() || session.is_empty() || !lease.heartbeat_unix.is_finite() {
                    return None;
                }
                Some(LeaseSummary {
                    session: (provider.to_lowercase(), session.to_string()),
                    heartbeat_unix: lease.heartbeat_unix,
                    codex_thread: lease.codex_thread(),
                })
            });
            cache.insert(
                path.clone(),
                CachedLeaseSummary {
                    len,
                    modified,
                    summary,
                },
            );
        }
        if let Some(summary) = cache.get(&path).and_then(|cached| cached.summary.as_ref()) {
            visit(summary);
        }
    }
}

/// Bounded scan of the lease directory, invoking `visit` for every lease that
/// is well-formed, active, and heartbeat-fresh. Everything else fails open.
fn for_each_fresh_lease(
    root: &Path,
    now: f64,
    ttl_seconds: f64,
    mut visit: impl FnMut(&SessionLease),
) {
    for (path, metadata) in lease_files(root) {
        // The helper rewrites the whole lease, `heartbeat_unix: time.time()`
        // included, so a file last modified outside the window cannot hold a
        // fresh heartbeat. Dead followers keep multi-megabyte mailboxes, and
        // the overlay roster polls this scan twice every 500 ms: parsing them
        // was the hottest Rust frame of the live app (sample 2026-10-15,
        // 45 stale leases, 7.2 MB per scan).
        if metadata
            .modified()
            .ok()
            .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
            .is_some_and(|modified| now - modified.as_secs_f64() > ttl_seconds)
        {
            continue;
        }
        let Some(lease) = read_lease(&path) else {
            continue;
        };
        let age = now - lease.heartbeat_unix;
        if !lease.active || !age.is_finite() || age < 0.0 || age > ttl_seconds {
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

    fn write_framed_lease(root: &Path, file: &str, session: &str, frame: serde_json::Value) {
        let dir = root.join("leases");
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join(file),
            serde_json::to_vec(&serde_json::json!({
                "schema": LEASE_SCHEMA,
                "name": "igor",
                "active": true,
                "heartbeat_unix": 995.0,
                "provider": "claude-code",
                "provider_session_id": session,
                "frame": frame,
            }))
            .unwrap(),
        )
        .unwrap();
    }

    #[test]
    fn only_a_lease_naming_a_frame_pane_reports_frame_detected() {
        let temp = tempfile::tempdir().unwrap();
        write_framed_lease(
            temp.path(),
            "framed.json",
            "in-pane",
            serde_json::json!({"session": "codescribe", "pane_id": 3}),
        );
        // A daemon-hosted session attaches without pane coordinates.
        write_framed_lease(
            temp.path(),
            "daemon.json",
            "daemon",
            serde_json::Value::Null,
        );
        write_framed_lease(
            temp.path(),
            "paneless.json",
            "paneless",
            serde_json::json!({"session": "codescribe"}),
        );
        // Pre-frame helpers never wrote the field at all.
        write_session_lease(temp.path(), "old.json", "claude-code", "old", true, 990.0);

        let hosts = live_follower_hosts_at(temp.path(), 1_000.0, 120.0);
        let frames: HashMap<_, _> = hosts
            .iter()
            .map(|(key, hosts)| (key.clone(), hosts.frame_pane))
            .collect();
        let key = |session: &str| ("claude-code".to_string(), session.to_string());
        assert_eq!(frames.get(&key("in-pane")), Some(&true));
        assert_eq!(frames.get(&key("daemon")), Some(&false));
        assert_eq!(frames.get(&key("paneless")), Some(&false));
        assert_eq!(frames.get(&key("old")), Some(&false));
        assert_eq!(
            live_follower_sessions_at(temp.path(), 1_000.0, 120.0).len(),
            4,
            "the pane fact never changes who is alive"
        );
    }

    fn write_terminal_lease(root: &Path, file: &str, session: &str, terminal: serde_json::Value) {
        let dir = root.join("leases");
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join(file),
            serde_json::to_vec(&serde_json::json!({
                "schema": LEASE_SCHEMA,
                "name": "igor",
                "active": true,
                "heartbeat_unix": 995.0,
                "provider": "codex",
                "provider_session_id": session,
                "frame": null,
                "terminal": terminal,
            }))
            .unwrap(),
        )
        .unwrap();
    }

    #[test]
    fn only_a_lease_naming_a_terminal_host_reports_terminal_detected() {
        let temp = tempfile::tempdir().unwrap();
        write_terminal_lease(
            temp.path(),
            "iterm.json",
            "in-iterm",
            serde_json::json!({
                "host": "iterm2",
                "host_bundle_id": "com.googlecode.iterm2",
                "tty": "/dev/ttys005",
                "session_id": "6F3C1A2B-0000-4000-8000-000000000005",
            }),
        );
        write_terminal_lease(
            temp.path(),
            "terminal.json",
            "in-terminal",
            serde_json::json!({
                "host": "apple-terminal",
                "host_bundle_id": "com.apple.Terminal",
                "tty": "/dev/ttys002",
                "session_id": null,
            }),
        );
        // Neither iTerm2 nor Terminal.app hosts this session.
        write_terminal_lease(
            temp.path(),
            "elsewhere.json",
            "elsewhere",
            serde_json::Value::Null,
        );
        write_terminal_lease(
            temp.path(),
            "blank.json",
            "blank",
            serde_json::json!({"host": "iterm2", "host_bundle_id": " "}),
        );
        // Pre-terminal helpers never wrote the field at all.
        write_session_lease(temp.path(), "old.json", "codex", "old", true, 990.0);

        let hosts = live_follower_hosts_at(temp.path(), 1_000.0, 120.0);
        let terminal = |session: &str| {
            hosts
                .get(&("codex".to_string(), session.to_string()))
                .map(|hosts| hosts.terminal_tab)
        };
        assert_eq!(terminal("in-iterm"), Some(true));
        assert_eq!(terminal("in-terminal"), Some(true));
        assert_eq!(terminal("elsewhere"), Some(false));
        assert_eq!(terminal("blank"), Some(false));
        assert_eq!(terminal("old"), Some(false));
        assert!(
            hosts.values().all(|hosts| !hosts.frame_pane),
            "a terminal tab is not a vc-frame pane"
        );
        assert_eq!(
            live_follower_sessions_at(temp.path(), 1_000.0, 120.0).len(),
            5,
            "the terminal fact never changes who is alive"
        );
    }

    const CODEX_THREAD: &str = "c0ffee01-0000-7000-8000-000000000001";

    fn write_codex_lease(
        root: &Path,
        file: &str,
        provider: &str,
        session: &str,
        heartbeat: f64,
        codex: serde_json::Value,
    ) {
        let dir = root.join("leases");
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join(file),
            serde_json::to_vec(&serde_json::json!({
                "schema": LEASE_SCHEMA,
                "name": "leon",
                "active": true,
                "heartbeat_unix": heartbeat,
                "provider": provider,
                "provider_session_id": session,
                "frame": null,
                "terminal": null,
                "codex": codex,
            }))
            .unwrap(),
        )
        .unwrap();
    }

    fn codex_facts(thread_id: &str, rollout: bool, remote: bool) -> serde_json::Value {
        serde_json::json!({
            "thread_id": thread_id,
            "app_bundle_id": "com.openai.codex",
            "rollout": rollout,
            "remote": remote,
        })
    }

    #[test]
    fn a_codex_thread_is_reported_whether_or_not_its_follower_is_alive() {
        let temp = tempfile::tempdir().unwrap();
        // Codex app followers are usually dead between turns.
        write_codex_lease(
            temp.path(),
            "stale.json",
            "codex",
            CODEX_THREAD,
            800.0,
            codex_facts(CODEX_THREAD, true, false),
        );
        write_codex_lease(
            temp.path(),
            "cloud.json",
            "Codex",
            "cloud-thread-0001",
            995.0,
            codex_facts("cloud-thread-0001", false, true),
        );
        write_codex_lease(
            temp.path(),
            "claude.json",
            "claude-code",
            "claude-session",
            995.0,
            serde_json::Value::Null,
        );
        // An id outside the helper's alphabet never becomes a link.
        write_codex_lease(
            temp.path(),
            "odd.json",
            "codex",
            "odd-session",
            995.0,
            codex_facts("../../etc", true, false),
        );
        // Pre-Codex-thread helpers never wrote the field at all.
        write_session_lease(temp.path(), "old.json", "codex", "old-session", true, 990.0);

        let threads = codex_threads_at(temp.path());
        let key = |session: &str| ("codex".to_string(), session.to_string());
        assert_eq!(
            threads.get(&key(CODEX_THREAD)),
            Some(&CodexThread {
                thread_id: CODEX_THREAD.to_string(),
                rollout: true,
            }),
            "a stale lease still names its thread"
        );
        assert_eq!(
            threads.get(&key("cloud-thread-0001")),
            Some(&CodexThread {
                thread_id: "cloud-thread-0001".to_string(),
                rollout: false,
            }),
            "a cloud thread without a local rollout is reported as such"
        );
        assert_eq!(threads.len(), 2, "{threads:?}");
        assert!(
            !live_follower_sessions_at(temp.path(), 1_000.0, 120.0).contains(&key(CODEX_THREAD)),
            "the thread fact never makes a dead follower alive"
        );
    }

    #[test]
    fn the_newest_lease_of_a_session_names_its_thread_and_a_rewrite_is_read_again() {
        let temp = tempfile::tempdir().unwrap();
        write_codex_lease(
            temp.path(),
            "a.json",
            "codex",
            CODEX_THREAD,
            900.0,
            codex_facts(CODEX_THREAD, true, false),
        );
        write_codex_lease(
            temp.path(),
            "b.json",
            "codex",
            CODEX_THREAD,
            950.0,
            codex_facts(CODEX_THREAD, false, false),
        );
        let key = ("codex".to_string(), CODEX_THREAD.to_string());
        assert_eq!(
            codex_threads_at(temp.path()).get(&key).map(|thread| thread.rollout),
            Some(false)
        );

        // The helper found the rollout later and rewrote the newer lease.
        write_codex_lease(
            temp.path(),
            "b.json",
            "codex",
            CODEX_THREAD,
            960.0,
            codex_facts(CODEX_THREAD, true, false),
        );
        assert_eq!(
            codex_threads_at(temp.path()).get(&key).map(|thread| thread.rollout),
            Some(true)
        );

        // A newer lease without Codex facts names no thread.
        write_codex_lease(
            temp.path(),
            "c.json",
            "codex",
            CODEX_THREAD,
            970.0,
            serde_json::Value::Null,
        );
        assert!(codex_threads_at(temp.path()).is_empty());
    }

    /// A lease shaped like the helper's: identity plus a mailbox of `padding`
    /// bytes of transcript-bearing envelopes this reader never looks at.
    fn write_lease_with_mailbox(root: &Path, file: &str, session: &str, padding: usize) -> u64 {
        let dir = root.join("leases");
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join(file);
        fs::write(
            &path,
            serde_json::to_vec(&serde_json::json!({
                "schema": LEASE_SCHEMA,
                "name": "igor",
                "active": true,
                "heartbeat_unix": 995.0,
                "provider": "claude-code",
                "provider_session_id": session,
                "pending": [{"kind": "revised", "text": "x".repeat(padding)}],
                "unclosed_channel_messages": {},
            }))
            .unwrap(),
        )
        .unwrap();
        fs::metadata(&path).unwrap().len()
    }

    #[test]
    fn a_follower_with_a_full_mailbox_is_still_listening() {
        let temp = tempfile::tempdir().unwrap();
        // 891 KB is the largest lease measured on a live session on 2026-10-06.
        let bytes = write_lease_with_mailbox(temp.path(), "busy.json", "sess-busy", 891 * 1024);
        assert!(bytes > 891 * 1024);

        assert_eq!(
            live_follower_sessions_at(temp.path(), 1_000.0, 120.0),
            HashSet::from([("claude-code".to_string(), "sess-busy".to_string())])
        );
        assert_eq!(read_active_names_at(temp.path(), 1_000.0, 120.0), ["Igor"]);
    }

    #[test]
    fn a_lease_larger_than_the_helper_can_write_is_not_read() {
        let temp = tempfile::tempdir().unwrap();
        let bytes = write_lease_with_mailbox(
            temp.path(),
            "runaway.json",
            "sess-runaway",
            MAX_LEASE_BYTES as usize,
        );
        assert!(bytes > MAX_LEASE_BYTES);

        assert!(live_follower_sessions_at(temp.path(), 1_000.0, 120.0).is_empty());
        assert!(read_active_names_at(temp.path(), 1_000.0, 120.0).is_empty());
    }

    /// The helper rewrites the lease on every heartbeat, so a file untouched
    /// past the window is dead before it is parsed, whatever heartbeat it
    /// still carries. A dead follower's mailbox can be megabytes.
    #[test]
    fn a_lease_file_untouched_past_the_window_is_not_parsed() {
        let temp = tempfile::tempdir().unwrap();
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs_f64();
        write_session_lease(
            temp.path(),
            "old.json",
            "codex",
            "sess-old",
            true,
            now - 5.0,
        );
        write_session_lease(
            temp.path(),
            "new.json",
            "codex",
            "sess-new",
            true,
            now - 5.0,
        );
        fs::File::options()
            .write(true)
            .open(temp.path().join("leases/old.json"))
            .unwrap()
            .set_modified(SystemTime::now() - Duration::from_secs(3_600))
            .unwrap();

        assert_eq!(
            live_follower_sessions_at(temp.path(), now, 120.0),
            HashSet::from([("codex".to_string(), "sess-new".to_string())])
        );
    }

    /// A live follower whose conversation stopped reading: the oldest
    /// unacknowledged seal or message dates the backlog. Acknowledged rows
    /// still listed in `pending`, draft revisions and dead leases never count.
    #[test]
    fn unread_backlog_dates_the_oldest_unacknowledged_delivery_of_a_live_follower() {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().join("leases");
        fs::create_dir_all(&dir).unwrap();
        let lease = |file: &str, session: &str, heartbeat: f64, pending: serde_json::Value| {
            fs::write(
                dir.join(file),
                serde_json::to_vec(&serde_json::json!({
                    "schema": LEASE_SCHEMA,
                    "lease_id": session,
                    "name": "iwo",
                    "active": true,
                    "heartbeat_unix": heartbeat,
                    "provider": "Claude-Code",
                    "provider_session_id": session,
                    "pending": pending,
                }))
                .unwrap(),
            )
            .unwrap();
        };
        lease(
            "stalled.json",
            "sess-stalled",
            990.0,
            serde_json::json!([
                {"kind": "seal", "delivery_id": "a".repeat(24), "emitted_at": "2026-10-14T13:00:00Z"},
                {"kind": "message", "delivery_id": "b".repeat(24), "emitted_at": "2026-10-14T12:30:00Z"},
                {"kind": "seal", "delivery_id": "c".repeat(24), "emitted_at": "2026-10-14T12:00:00Z"},
                {"kind": "draft", "delivery_id": "d".repeat(24), "emitted_at": "2026-10-14T11:00:00Z"},
                {"kind": "seal", "delivery_id": "e".repeat(24), "emitted_at": "garbage"}
            ]),
        );
        let markers = temp.path().join("acknowledgments/sess-stalled");
        fs::create_dir_all(&markers).unwrap();
        fs::write(markers.join(format!("{}.json", "c".repeat(24))), b"{}").unwrap();
        lease("drained.json", "sess-drained", 995.0, serde_json::json!([]));
        lease(
            "dead.json",
            "sess-dead",
            800.0,
            serde_json::json!([
                {"kind": "seal", "delivery_id": "f".repeat(24), "emitted_at": "2026-10-14T10:00:00Z"}
            ]),
        );

        let since = unread_backlog_since_at(temp.path(), 1_000.0, 120.0);
        let expected = chrono::DateTime::parse_from_rfc3339("2026-10-14T12:30:00Z")
            .unwrap()
            .timestamp() as f64;
        assert_eq!(
            since,
            HashMap::from([(
                ("claude-code".to_string(), "sess-stalled".to_string()),
                expected
            )])
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

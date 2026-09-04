//! macOS Keychain integration for API keys.
//!
//! Stores secrets in the system Keychain instead of plaintext .env files.

use anyhow::{Context, Result};
use base64::Engine as _;
use security_framework::passwords::{
    delete_generic_password, get_generic_password, set_generic_password,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::{Once, OnceLock, RwLock};
use tracing::{debug, info};

/// Keychain service identity for every Codescribe generic-password item.
const SERVICE: &str = "com.vetcoders.codescribe";
/// Account name of the single bundled secret item (all API keys together).
const BUNDLE_ACCOUNT: &str = "codescribe_keychain_bundle_v1";

/// Known API key accounts stored in Keychain.
pub const KEYCHAIN_ACCOUNTS: &[&str] = &[
    "LLM_API_KEY",
    "STT_API_KEY",
    "LLM_FORMATTING_API_KEY",
    "LLM_ASSISTIVE_API_KEY",
    "LLM_ANTHROPIC_API_KEY",
    "LLM_XAI_API_KEY",
    "GITHUB_TOKEN",
];

/// All Codescribe secrets in a single Keychain item.
///
/// One bundled item rather than one item per account: macOS evaluates the item
/// ACL per access, so N accounts meant N separate authorization decisions on
/// every launch.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct KeychainBundle {
    version: u8,
    keys: BTreeMap<String, String>,
}

impl Default for KeychainBundle {
    /// Empty bundle at schema version 1 — used before first Keychain load or write.
    fn default() -> Self {
        Self {
            version: 1,
            keys: BTreeMap::new(),
        }
    }
}

/// Process-wide decoded Keychain bundle cache; `None` means unloaded or deleted.
static BUNDLE_CACHE: OnceLock<RwLock<Option<KeychainBundle>>> = OnceLock::new();
/// Ledger of env values this process seeded from Keychain (not user-exported).
static PROCESS_ENV_SEEDS: OnceLock<RwLock<BTreeMap<String, String>>> = OnceLock::new();
/// Ensures `populate_env_from_keychain` mutates process env at most once.
static POPULATE_ONCE: Once = Once::new();

/// Process-wide cache of the decoded bundle, created on first use.
///
/// `None` inside the lock means "not loaded or deleted", which is distinct from
/// the lock not existing yet.
fn bundle_cache() -> &'static RwLock<Option<KeychainBundle>> {
    BUNDLE_CACHE.get_or_init(|| RwLock::new(None))
}

/// Read the cached bundle, recovering from a poisoned lock.
///
/// A panic elsewhere must not turn every later secret lookup into a panic; the
/// cached bytes are still valid, so the guard is taken via `into_inner()`.
fn read_bundle_cache() -> Option<KeychainBundle> {
    match bundle_cache().read() {
        Ok(guard) => guard.clone(),
        Err(poisoned) => poisoned.into_inner().clone(),
    }
}

/// Replace the cached bundle, recovering from a poisoned lock. `None` clears it.
fn write_bundle_cache(bundle: Option<KeychainBundle>) {
    match bundle_cache().write() {
        Ok(mut guard) => {
            *guard = bundle;
        }
        Err(poisoned) => {
            *poisoned.into_inner() = bundle;
        }
    }
}

/// Origin ledger for env vars this process set from Keychain during bootstrap.
///
/// Without it, a value copied into the environment is indistinguishable from one
/// the user exported, and a later Settings save or delete would be shadowed by
/// the stale process snapshot forever.
fn process_env_seeds() -> &'static RwLock<BTreeMap<String, String>> {
    PROCESS_ENV_SEEDS.get_or_init(|| RwLock::new(BTreeMap::new()))
}

/// Record that `account`'s current env value originated from Keychain, not from
/// the user's environment.
fn remember_process_env_seed(account: &str, secret: &str) {
    match process_env_seeds().write() {
        Ok(mut guard) => {
            guard.insert(account.to_string(), secret.to_string());
        }
        Err(poisoned) => {
            poisoned
                .into_inner()
                .insert(account.to_string(), secret.to_string());
        }
    }
}

/// The value this process seeded into `account`, if it seeded one.
fn process_env_seed(account: &str) -> Option<String> {
    match process_env_seeds().read() {
        Ok(guard) => guard.get(account).cloned(),
        Err(poisoned) => poisoned.into_inner().get(account).cloned(),
    }
}

/// Serialize the bundle to the on-disk form: JSON, base64, `b64:` prefixed.
///
/// The prefix is a format marker, not obfuscation — it lets [`decode_bundle`]
/// distinguish this encoding from the legacy bare-JSON items.
fn encode_bundle(bundle: &KeychainBundle) -> Result<Vec<u8>> {
    let json = serde_json::to_string(bundle).context("Failed to serialize bundle")?;
    let b64 = base64::engine::general_purpose::STANDARD.encode(json.as_bytes());
    Ok(format!("b64:{b64}").into_bytes())
}

/// Parse a stored bundle, accepting both the `b64:` form and legacy bare JSON.
///
/// Every failure collapses to `None`: an unreadable item is treated as "no
/// secrets stored" so a corrupt entry degrades to re-entry rather than an error
/// on every launch.
fn decode_bundle(bytes: &[u8]) -> Option<KeychainBundle> {
    let raw = String::from_utf8(bytes.to_vec()).ok()?;
    let json = if let Some(b64) = raw.strip_prefix("b64:") {
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(b64.as_bytes())
            .ok()?;
        String::from_utf8(decoded).ok()?
    } else {
        // Legacy: plain JSON (no prefix)
        raw
    };
    serde_json::from_str(&json).ok()
}

/// Return the bundle from cache, otherwise read it from the Keychain.
///
/// A miss can raise a macOS authorization prompt, so this belongs only on paths
/// where a prompt is acceptable — see [`cached_runtime_key`] for the silent one.
/// A successful read populates the cache; a decode failure does not.
fn load_bundle() -> Option<KeychainBundle> {
    if let Some(bundle) = read_bundle_cache() {
        return Some(bundle);
    }
    match get_generic_password(SERVICE, BUNDLE_ACCOUNT) {
        Ok(bytes) => {
            let bundle = decode_bundle(&bytes);
            if bundle.is_some() {
                write_bundle_cache(bundle.clone());
            }
            bundle
        }
        Err(e) => {
            // INFO on purpose: an unreadable bundle (absent OR ACL-denied
            // after a re-signed install) silently strips every lane of its
            // credential — at debug level that was invisible in the 2026-09-04
            // agent-chat 401 boot log.
            info!("Keychain bundle unreadable (absent or access denied): {e}");
            None
        }
    }
}

/// Write the bundle to the Keychain and refresh the cache on success.
fn save_bundle(bundle: &KeychainBundle) -> Result<()> {
    let payload = encode_bundle(bundle)?;
    set_generic_password(SERVICE, BUNDLE_ACCOUNT, &payload)
        .with_context(|| "Failed to save Keychain bundle")?;
    write_bundle_cache(Some(bundle.clone()));
    Ok(())
}

/// Returns true when running inside a test harness or when Keychain is explicitly disabled.
///
/// NOTE: `cfg!(test)` only works for **unit tests** within this crate. Integration tests
/// (`tests/`, `core/tests/`) and tests in other crates (e.g. `app/*`, `bridge/*`) compile
/// this library normally, so `cfg!(test)` is false there. Such tests are still detected via
/// the harness signals below (the `target/**/deps/` exe path and `RUST_TEST_THREADS`), and
/// may additionally set `CODESCRIBE_DISABLE_KEYCHAIN=1` — which the Makefile `TEST_SETUP` does.
/// `CODESCRIBE_DATA_DIR` does NOT skip Keychain: it is a production-valid data-dir override.
///
/// The Swift front-end suite is a THIRD harness shape and none of the Rust signals see it —
/// see `in_xctest_host` below.
fn is_test_env() -> bool {
    if cfg!(test) {
        return true;
    }
    if let Ok(exe_path) = std::env::current_exe() {
        let exe = exe_path.to_string_lossy();
        if exe.contains("/target/debug/deps/")
            || exe.contains("/target/release/deps/")
            || exe.contains("\\target\\debug\\deps\\")
            || exe.contains("\\target\\release\\deps\\")
        {
            return true;
        }
    }
    // app/* tests link codescribe-core as a dependency, so cfg!(test) is false there.
    // libtest sets RUST_TEST_THREADS for the harness process; use it as a
    // non-invasive signal to skip blocking Keychain calls during tests.
    if std::env::var_os("RUST_TEST_THREADS").is_some() {
        return true;
    }
    if in_xctest_host() {
        return true;
    }
    keychain_disabled_by_signals(
        std::env::var_os("CODESCRIBE_DISABLE_KEYCHAIN").is_some(),
        std::env::var_os("CODESCRIBE_DATA_DIR").is_some(),
        std::env::var_os("CI").is_some(),
    )
}

/// Pure Keychain-skip policy over explicit environment signals.
///
/// `CODESCRIBE_DISABLE_KEYCHAIN` is the ONLY user-facing kill switch. `CODESCRIBE_DATA_DIR`
/// (a documented data-directory override) and `CI` are accepted here purely so the policy is
/// explicit and regression-tested — they MUST NOT disable Keychain. Silently dropping
/// persisted API keys / OAuth tokens when a user merely relocates the data dir (or runs under
/// a CI flag) was the bug this function pins shut. Kept side-effect-free so the policy is
/// unit-testable without mutating process-global env vars (which race across the suite).
fn keychain_disabled_by_signals(disable_keychain: bool, data_dir_set: bool, ci_set: bool) -> bool {
    let _ = (data_dir_set, ci_set);
    disable_keychain
}

/// True when this process is an XCTest host — the Swift front-end suite.
///
/// This is the harness shape every other signal in `is_test_env` misses, and missing it is
/// not free. `make test-swift` hosts the tests inside the app itself, so the executable is
/// `Codescribe.app/Contents/MacOS/Codescribe` (not `target/**/deps/*`) and libtest never
/// runs, so `RUST_TEST_THREADS` is unset. The core therefore classified a test run as
/// production and issued real Keychain calls from a binary whose ad-hoc signature changes on
/// every rebuild, so macOS re-evaluated the item ACL instead of reusing a cached decision.
///
/// Measured 2026-08-08 on this host, identical tree and identical 317 tests, six runs:
/// without the bypass 47.5 / 28.2 / 4.5 s, with it 4.19 / 4.69 / 4.22 s — ~3 s of CPU in
/// every case, so the spread was blocking, not work. See `macos/CodescribeTests/README.md`.
///
/// Every other test lane in this repo already bypasses Keychain (`TEST_SETUP` in the
/// Makefile, `CODESCRIBE_DISABLE_KEYCHAIN` in `.github/workflows/rust.yml`, the harness
/// signals above). Detecting the host here rather than exporting the kill switch from the
/// Makefile keeps that guarantee attached to the RUN, not to the invocation: an XCTest host
/// launched from Xcode, from a script, or by a future CI job inherits it too.
fn in_xctest_host() -> bool {
    is_xctest_host_by_signals(
        std::env::var_os("XCTestConfigurationFilePath").is_some(),
        std::env::var_os("XCTestSessionIdentifier").is_some(),
        std::env::var_os("XCTestBundlePath").is_some(),
    )
}

/// Pure XCTest-host policy over environment signals — any one of them is proof.
///
/// Three markers rather than one because Xcode has moved which it exports across versions;
/// `swift_suite_env_markers_pin_the_signal_the_core_keys_on` (CodescribeTests) asserts from
/// inside a live host that at least one is still present, so this cannot rot silently into a
/// function that always returns false. Kept side-effect-free so the policy is unit-testable
/// without mutating process-global env vars (which race across the suite).
fn is_xctest_host_by_signals(config_file: bool, session_id: bool, bundle_path: bool) -> bool {
    config_file || session_id || bundle_path
}

/// Saves a secret to the macOS Keychain under the Codescribe service.
/// In test environments, sets the env var directly instead of touching Keychain.
pub fn save_key(account: &str, secret: &str) -> Result<()> {
    if is_test_env() {
        debug!("Test env: skipping Keychain save for {account}");
        unsafe { std::env::set_var(account, secret) };
        return Ok(());
    }
    let mut bundle = load_bundle().unwrap_or_default();
    bundle.keys.insert(account.to_string(), secret.to_string());
    save_bundle(&bundle)?;
    info!("Saved {account} to Keychain bundle");
    Ok(())
}

/// Loads a secret from the macOS Keychain. Returns `None` if not found.
pub fn load_key(account: &str) -> Option<String> {
    if is_test_env() {
        debug!("Test env: skipping Keychain load for {account}");
        return None;
    }
    let bundle = load_bundle()?;
    if let Some(value) = bundle.keys.get(account) {
        debug!("Loaded {account} from Keychain bundle");
        return Some(value.clone());
    }
    None
}

/// Resolve the current runtime secret without touching Keychain.
///
/// Explicit process environment values retain highest priority. Values copied
/// from Keychain during bootstrap are origin-tracked, so a later Settings save
/// or delete cannot be shadowed by that stale process snapshot.
pub fn cached_runtime_key(account: &str) -> Option<String> {
    let env_value = non_empty_env(account);
    let seeded_env_value = process_env_seed(account);
    if let Some(value) = explicit_env_value(env_value, seeded_env_value.as_deref()) {
        return Some(value);
    }
    non_empty_secret(read_bundle_cache().and_then(|bundle| bundle.keys.get(account).cloned()))
}

/// Resolve the current runtime secret, reading Keychain when the in-memory
/// cache has not been populated yet. Use this only on explicit secret-use
/// paths, where a macOS Keychain prompt is appropriate.
pub fn runtime_key(account: &str) -> Option<String> {
    let env_value = non_empty_env(account);
    let seeded_env_value = process_env_seed(account);
    if let Some(value) = explicit_env_value(env_value, seeded_env_value.as_deref()) {
        return Some(value);
    }
    non_empty_secret(load_key(account))
}

/// Read `account` from the process environment, trimmed, treating blank as unset.
fn non_empty_env(account: &str) -> Option<String> {
    std::env::var(account)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

/// The secret-precedence policy, lifted out of the env for testing.
///
/// Mirrors [`cached_runtime_key`] / [`runtime_key`] exactly: an explicit env
/// value wins, a value this process seeded from Keychain does not, and the
/// stored value is the fallback.
#[cfg(test)]
fn resolve_runtime_key(
    env_value: Option<String>,
    seeded_env_value: Option<String>,
    stored_value: Option<String>,
) -> Option<String> {
    explicit_env_value(env_value, seeded_env_value.as_deref())
        .or_else(|| non_empty_secret(stored_value))
}

/// Keep an env value only when it did *not* come from this process's own
/// Keychain bootstrap.
///
/// Equality against the seed is the discriminator: if they match, the env still
/// holds our stale copy and the current Keychain value must win instead.
fn explicit_env_value(env_value: Option<String>, seeded_env_value: Option<&str>) -> Option<String> {
    env_value.filter(|value| seeded_env_value != Some(value))
}

/// Trim a stored secret and treat a whitespace-only value as absent.
fn non_empty_secret(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

/// Deletes a secret from the macOS Keychain. Ignores "not found" errors.
pub fn delete_key(account: &str) -> Result<()> {
    if is_test_env() {
        debug!("Test env: skipping Keychain delete for {account}");
        return Ok(());
    }
    let mut bundle = load_bundle().unwrap_or_default();
    if bundle.keys.remove(account).is_some() {
        if bundle.keys.is_empty() {
            match delete_generic_password(SERVICE, BUNDLE_ACCOUNT) {
                Ok(()) => {
                    write_bundle_cache(None);
                    info!("Deleted Keychain bundle (last key removed)");
                    Ok(())
                }
                Err(e) => {
                    let desc = format!("{e}");
                    if desc.contains("not found") || desc.contains("-25300") {
                        write_bundle_cache(None);
                        debug!("Keychain bundle not found, nothing to delete");
                        Ok(())
                    } else {
                        Err(e).with_context(|| "Failed to delete Keychain bundle")
                    }
                }
            }
        } else {
            save_bundle(&bundle)?;
            info!("Deleted {account} from Keychain bundle");
            Ok(())
        }
    } else {
        debug!("Keychain bundle has no {account}, nothing to delete");
        Ok(())
    }
}

/// Populates environment variables from Keychain for any keys not already set.
///
/// This ensures `.env` values always take priority over Keychain entries.
pub fn populate_env_from_keychain() {
    if is_test_env() {
        debug!("Test env: skipping Keychain population");
        return;
    }
    POPULATE_ONCE.call_once(|| {
        let bundle = load_bundle();
        if bundle.is_none() {
            debug!("Keychain bundle missing; skipping population");
            return;
        }
        let bundle = bundle.unwrap();
        for &account in KEYCHAIN_ACCOUNTS {
            if std::env::var(account).is_err()
                && let Some(value) = bundle.keys.get(account)
            {
                // SAFETY: called during single-threaded init before spawning workers.
                unsafe {
                    std::env::set_var(account, value);
                }
                remember_process_env_seed(account, value);
                debug!("Set {account} from Keychain bundle");
            }
        }
    });
}

/// Keychain bypass and runtime-key priority regressions (no live Keychain I/O).
#[cfg(test)]
mod tests {
    use super::{is_xctest_host_by_signals, keychain_disabled_by_signals, resolve_runtime_key};

    /// Any single XCTest host marker is enough — Xcode has rotated which env it sets.
    #[test]
    fn any_xctest_marker_identifies_the_swift_test_host() {
        // The Swift suite is a test run that looks like production to every other signal in
        // `is_test_env`: an app binary, no libtest, no `target/**/deps/` path. Any one marker
        // is enough — Xcode has moved which of the three it exports between versions, and a
        // policy that required all three would go quietly false on the next Xcode bump.
        assert!(is_xctest_host_by_signals(true, false, false));
        assert!(is_xctest_host_by_signals(false, true, false));
        assert!(is_xctest_host_by_signals(false, false, true));
        assert!(is_xctest_host_by_signals(true, true, true));
    }

    /// Zero markers must never trip the XCTest host detector (would drop persisted keys).
    #[test]
    fn a_production_launch_is_never_mistaken_for_an_xctest_host() {
        // The whole value of this detector is that it cannot fire for a user launch: it would
        // drop persisted API keys / OAuth tokens on the floor. No markers, no bypass.
        assert!(!is_xctest_host_by_signals(false, false, false));
    }

    /// Explicit disable flag turns Keychain off regardless of data-dir or CI signals.
    #[test]
    fn disable_keychain_flag_is_honored() {
        // Explicit user opt-out disables Keychain regardless of other signals.
        assert!(keychain_disabled_by_signals(true, false, false));
        assert!(keychain_disabled_by_signals(true, true, true));
    }

    /// `CODESCRIBE_DATA_DIR` and CI alone must not silently disable Keychain persistence.
    #[test]
    fn data_dir_and_ci_do_not_disable_keychain() {
        // Regression: setting CODESCRIBE_DATA_DIR (a documented data-dir override) or CI
        // must NOT silently disable Keychain persistence of API keys / OAuth tokens.
        assert!(!keychain_disabled_by_signals(false, true, false));
        assert!(!keychain_disabled_by_signals(false, false, true));
        assert!(!keychain_disabled_by_signals(false, true, true));
        assert!(!keychain_disabled_by_signals(false, false, false));
    }

    /// User-exported env wins over a Keychain-stored secret for the same account.
    #[test]
    fn explicit_process_env_keeps_priority_over_keychain() {
        assert_eq!(
            resolve_runtime_key(
                Some("explicit".to_string()),
                None,
                Some("stored".to_string())
            )
            .as_deref(),
            Some("explicit")
        );
    }

    /// A later Keychain write must replace the bootstrap seed left in process env.
    #[test]
    fn updated_keychain_replaces_bootstrap_seed() {
        assert_eq!(
            resolve_runtime_key(
                Some("old".to_string()),
                Some("old".to_string()),
                Some("new".to_string())
            )
            .as_deref(),
            Some("new")
        );
    }

    /// Deleting the Keychain entry invalidates a matching bootstrap seed in process env.
    #[test]
    fn deleted_keychain_entry_invalidates_bootstrap_seed() {
        assert_eq!(
            resolve_runtime_key(Some("old".to_string()), Some("old".to_string()), None),
            None
        );
    }
}

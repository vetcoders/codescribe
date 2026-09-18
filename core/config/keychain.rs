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

use crate::llm::provider::is_custom_key_account;

type CredentialAttempts = std::rc::Rc<std::cell::RefCell<Vec<&'static str>>>;

thread_local! {
    static CREDENTIAL_ACQUISITION_ATTEMPTS: std::cell::RefCell<Option<CredentialAttempts>> = const { std::cell::RefCell::new(None) };
}

/// Synchronous credential-I/O tripwire, including dependency-mode witnesses.
/// Attempts panic before test bypasses or any credential-store operation.
#[doc(hidden)]
pub struct CredentialAcquisitionProbe {
    attempts: CredentialAttempts,
}

impl CredentialAcquisitionProbe {
    pub fn forbid() -> Self {
        let attempts = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        CREDENTIAL_ACQUISITION_ATTEMPTS.with(|slot| {
            let mut slot = slot.borrow_mut();
            assert!(
                slot.is_none(),
                "credential acquisition probe already installed"
            );
            *slot = Some(attempts.clone());
        });
        Self { attempts }
    }

    pub fn attempts(&self) -> Vec<&'static str> {
        self.attempts.borrow().clone()
    }
}

impl Drop for CredentialAcquisitionProbe {
    fn drop(&mut self) {
        CREDENTIAL_ACQUISITION_ATTEMPTS.with(|slot| *slot.borrow_mut() = None);
    }
}

fn note_credential_acquisition(operation: &'static str) {
    CREDENTIAL_ACQUISITION_ATTEMPTS.with(|slot| {
        if let Some(attempts) = slot.borrow().as_ref() {
            attempts.borrow_mut().push(operation);
            panic!("forbidden credential acquisition: {operation}");
        }
    });
}

/// Keychain service identity for every Codescribe generic-password item.
const SERVICE: &str = "com.vetcoders.codescribe";
/// Account name of the single bundled secret item (all API keys together).
const BUNDLE_ACCOUNT: &str = "codescribe_keychain_bundle_v1";

/// Static secret accounts: one per pinned vendor, plus STT and GitHub.
///
/// Only these are seeded into process env at bootstrap. Custom-provider keys
/// (`LLM_CUSTOM_<ID>_API_KEY`, see [`is_known_account`]) and OAuth token
/// records live in the same bundle but are read from it directly, never from
/// env.
pub const KEYCHAIN_ACCOUNTS: &[&str] = &[
    "LLM_LIBRAXIS_API_KEY",
    "LLM_OPENAI_API_KEY",
    "LLM_XAI_API_KEY",
    "LLM_ANTHROPIC_API_KEY",
    "STT_FILE_API_KEY",
    "STT_LIVE_API_KEY",
    "GITHUB_TOKEN",
];

/// A static account or a well-formed custom-provider key account.
pub fn is_known_account(account: &str) -> bool {
    KEYCHAIN_ACCOUNTS.contains(&account) || is_custom_key_account(account)
}

/// Whether a non-blank secret exists for `account` without touching Keychain.
///
/// Static accounts honour an explicit process env value; custom accounts read
/// the bundle cache only.
pub fn key_present(account: &str) -> bool {
    cached_runtime_key(account).is_some()
}

/// One legacy → current secret relocation inside the bundle.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeyMove {
    /// Legacy account to drain (for example `LLM_FORMATTING_API_KEY`).
    pub from: String,
    /// Account that owns the secret from now on (a provider `key_account`).
    pub to: String,
}

/// Apply [`KeyMove`]s to the bundle: copy `from` into `to` when `to` is absent,
/// then remove the copied `from`. Returns the number of bundle entries changed.
///
/// Idempotent — a second pass finds no `from` entries and changes nothing. A
/// `to` that already holds a secret keeps it: the user's current key is never
/// overwritten by a legacy one; a differing legacy secret remains recoverable.
/// Disabled Keychain access leaves relocation pending.
pub fn apply_key_moves(moves: &[KeyMove]) -> Result<usize> {
    note_credential_acquisition("relocate");
    if moves.is_empty() {
        return Ok(0);
    }
    anyhow::ensure!(
        !is_test_env(),
        "Keychain disabled; relocation must remain pending"
    );
    // Do not collapse denial/corruption into an empty bundle: that would erase
    // the durable retry intent. -25300 is Security.framework errSecItemNotFound.
    let mut bundle = match get_generic_password(SERVICE, BUNDLE_ACCOUNT) {
        Ok(bytes) => decode_bundle(&bytes).context("Keychain bundle cannot be decoded")?,
        Err(error) if error.code() == -25300 => KeychainBundle::default(),
        Err(error) => return Err(error).context("Keychain relocation read failed"),
    };
    let changed = relocate_bundle_keys(&mut bundle, moves);
    if changed > 0 {
        save_bundle(&bundle)?;
    } else {
        write_bundle_cache(Some(bundle));
    }
    Ok(changed)
}

fn relocate_bundle_keys(bundle: &mut KeychainBundle, moves: &[KeyMove]) -> usize {
    let mut changed = 0;
    for step in moves {
        let Some(secret) = bundle.keys.get(&step.from).cloned() else {
            continue;
        };
        if bundle
            .keys
            .get(&step.to)
            .is_some_and(|current| current != &secret)
        {
            info!(
                "Keychain: retained legacy {} ({} holds a different key)",
                step.from, step.to
            );
            continue;
        }
        bundle.keys.remove(&step.from);
        changed += 1;
        if !bundle.keys.contains_key(&step.to) {
            bundle.keys.insert(step.to.clone(), secret);
            changed += 1;
            info!("Keychain: moved {} into {}", step.from, step.to);
        } else {
            info!(
                "Keychain: removed copied legacy {} ({} already has the same key)",
                step.from, step.to
            );
        }
    }
    changed
}

/// Copy a legacy key to missing targets, then drain the source in one bundle write.
/// A failed write leaves the source/cache intact so a later load can retry.
pub fn fan_out_key(from: &str, targets: &[&str]) -> usize {
    note_credential_acquisition("fan out");
    if targets.is_empty() || targets.contains(&from) {
        return 0;
    }
    let bundle = if is_test_env() {
        read_bundle_cache()
    } else {
        load_bundle()
    };
    let Some(bundle) = bundle else {
        return 0;
    };
    persist_key_fan_out(bundle, from, targets, |bundle| {
        if is_test_env() {
            write_bundle_cache(Some(bundle.clone()));
            Ok(())
        } else {
            save_bundle(bundle)
        }
    })
}

fn persist_key_fan_out(
    mut bundle: KeychainBundle,
    from: &str,
    targets: &[&str],
    persist: impl FnOnce(&KeychainBundle) -> Result<()>,
) -> usize {
    let Some(secret) = bundle.keys.remove(from) else {
        return 0;
    };
    let mut changed = 1;
    for target in targets {
        if !bundle.keys.contains_key(*target) {
            bundle.keys.insert((*target).into(), secret.clone());
            changed += 1;
        }
    }
    if let Err(error) = persist(&bundle) {
        tracing::warn!(%error, "STT key fan-out failed; source retained");
        return 0;
    }
    changed
}

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
    note_credential_acquisition("read bundle");
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
    note_credential_acquisition("write bundle");
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
///
/// In test environments a static account is set as an env var instead; a
/// custom-provider account (never read from env) goes into the bundle cache.
pub fn save_key(account: &str, secret: &str) -> Result<()> {
    note_credential_acquisition("save");
    if is_test_env() {
        debug!("Test env: skipping Keychain save for {account}");
        if is_custom_key_account(account) {
            let mut bundle = read_bundle_cache().unwrap_or_default();
            bundle.keys.insert(account.to_string(), secret.to_string());
            write_bundle_cache(Some(bundle));
        } else {
            unsafe { std::env::set_var(account, secret) };
        }
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
    note_credential_acquisition("load");
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
/// Explicit process environment values retain highest priority for static
/// accounts. Values copied from Keychain during bootstrap are origin-tracked,
/// so a later Settings save or delete cannot be shadowed by that stale process
/// snapshot. Custom-provider accounts never consult env.
pub fn cached_runtime_key(account: &str) -> Option<String> {
    if let Some(value) = explicit_env_for(account) {
        return Some(value);
    }
    non_empty_secret(cached_bundle_secret(account))
}

/// The bundle-cache entry for `account`, untrimmed.
fn cached_bundle_secret(account: &str) -> Option<String> {
    read_bundle_cache().and_then(|bundle| bundle.keys.get(account).cloned())
}

/// The explicit (user-exported) env value for a static account; `None` for a
/// custom-provider account, whose key must never reach process env.
fn explicit_env_for(account: &str) -> Option<String> {
    if is_custom_key_account(account) {
        return None;
    }
    let seeded_env_value = process_env_seed(account);
    explicit_env_value(non_empty_env(account), seeded_env_value.as_deref())
}

/// Test-only view of a decoded Keychain bundle: what seal-time readers see when
/// secrets exist in Keychain and nowhere in the process environment.
#[cfg(test)]
pub(crate) mod test_support {
    use super::{KeychainBundle, read_bundle_cache, write_bundle_cache};

    /// Restores whatever bundle cache existed before the fixture on drop.
    pub(crate) struct BundleCacheGuard {
        previous: Option<KeychainBundle>,
    }

    /// Install `keys` as the process bundle cache, exactly as a decoded
    /// Keychain read would leave it.
    pub(crate) fn install_bundle(keys: &[(&str, &str)]) -> BundleCacheGuard {
        let previous = read_bundle_cache();
        let mut bundle = KeychainBundle::default();
        for (account, secret) in keys {
            bundle
                .keys
                .insert((*account).to_string(), (*secret).to_string());
        }
        write_bundle_cache(Some(bundle));
        BundleCacheGuard { previous }
    }

    /// Current bundle cache as plain account → secret pairs.
    pub(crate) fn snapshot_bundle() -> Option<std::collections::HashMap<String, String>> {
        read_bundle_cache().map(|bundle| bundle.keys.into_iter().collect())
    }

    impl Drop for BundleCacheGuard {
        fn drop(&mut self) {
            write_bundle_cache(self.previous.take());
        }
    }
}

/// Resolve the current runtime secret, reading Keychain when the in-memory
/// cache has not been populated yet. Use this only on explicit secret-use
/// paths, where a macOS Keychain prompt is appropriate.
pub fn runtime_key(account: &str) -> Option<String> {
    if let Some(value) = explicit_env_for(account) {
        return Some(value);
    }
    non_empty_secret(cached_bundle_secret(account).or_else(|| load_key(account)))
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
    note_credential_acquisition("delete");
    if is_test_env() {
        debug!("Test env: skipping Keychain delete for {account}");
        if is_custom_key_account(account)
            && let Some(mut bundle) = read_bundle_cache()
        {
            bundle.keys.remove(account);
            write_bundle_cache(Some(bundle));
        }
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

/// A retired `STT_API_KEY` still in the bundle means the STT lane migration
/// ran while the Keychain was unavailable. Finish the fan-out here, where the
/// bundle is already open, so no settings load ever has to.
pub(crate) fn retry_stt_key_fan_out() -> usize {
    use crate::stt::SttLane;
    fan_out_key(
        "STT_API_KEY",
        &[SttLane::File.key_account(), SttLane::Live.key_account()],
    )
}

/// Populates environment variables from Keychain for any static account not
/// already set. Custom-provider keys and OAuth records stay in the bundle.
///
/// This ensures `.env` values always take priority over Keychain entries.
/// A closed env-seeding window still permits authorized acquisition into cache;
/// it never permits writing the acquired values into the running process env.
pub fn populate_env_from_keychain(seed_process_env: bool) {
    note_credential_acquisition("populate");
    if is_test_env() {
        debug!("Test env: skipping Keychain population");
        return;
    }
    let Some(bundle) = load_bundle() else {
        debug!("Keychain bundle missing; skipping population");
        return;
    };
    retry_stt_key_fan_out();
    let bundle = read_bundle_cache().unwrap_or(bundle);
    seed_bundle_env(&bundle, seed_process_env);
}

fn seed_bundle_env(bundle: &KeychainBundle, seed_process_env: bool) {
    if !seed_process_env {
        return;
    }
    POPULATE_ONCE.call_once(|| {
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
    #[test]
    #[serial_test::serial]
    fn failed_fan_out_write_preserves_source_and_existing_destinations() {
        let _cache = super::test_support::install_bundle(&[
            ("STT_API_KEY", "synthetic-source"),
            ("STT_FILE_API_KEY", "synthetic-current"),
        ]);
        let before = super::read_bundle_cache().unwrap();
        let changed = super::persist_key_fan_out(
            before.clone(),
            "STT_API_KEY",
            &["STT_FILE_API_KEY", "STT_LIVE_API_KEY"],
            |candidate| {
                assert_eq!(
                    candidate.keys.get("STT_FILE_API_KEY").map(String::as_str),
                    Some("synthetic-current")
                );
                assert_eq!(
                    candidate.keys.get("STT_LIVE_API_KEY").map(String::as_str),
                    Some("synthetic-source")
                );
                anyhow::bail!("synthetic persistence refusal")
            },
        );
        assert_eq!(changed, 0);
        assert_eq!(super::read_bundle_cache().unwrap().keys, before.keys);
    }

    #[test]
    fn credential_probe_detects_attempts_before_harness_bypass() {
        let probe = super::CredentialAcquisitionProbe::forbid();
        let result = std::panic::catch_unwind(|| super::load_key("synthetic-account"));
        assert!(result.is_err());
        assert_eq!(probe.attempts(), ["load"]);
    }

    #[test]
    #[serial_test::serial]
    fn closed_env_window_keeps_newly_acquired_credentials_out_of_process_env() {
        let before = std::env::var_os("STT_FILE_API_KEY");
        let mut bundle = super::KeychainBundle::default();
        bundle
            .keys
            .insert("STT_FILE_API_KEY".into(), "synthetic-new-value".into());
        super::seed_bundle_env(&bundle, false);
        assert!(std::env::var_os("STT_FILE_API_KEY") == before);
    }

    use super::{
        KEYCHAIN_ACCOUNTS, cached_runtime_key, is_known_account, is_xctest_host_by_signals,
        key_present, keychain_disabled_by_signals, resolve_runtime_key, test_support,
    };
    use serial_test::serial;

    #[test]
    fn relocation_preserves_a_different_existing_key_and_is_retry_safe() {
        let mut bundle = super::KeychainBundle::default();
        bundle.keys.insert("old-a".into(), "legacy-a".into());
        bundle.keys.insert("new-a".into(), "current-a".into());
        bundle.keys.insert("old-b".into(), "legacy-b".into());
        let moves = [
            super::KeyMove {
                from: "old-a".into(),
                to: "new-a".into(),
            },
            super::KeyMove {
                from: "old-b".into(),
                to: "new-b".into(),
            },
        ];
        assert_eq!(super::relocate_bundle_keys(&mut bundle, &moves), 2);
        assert_eq!(bundle.keys.get("old-a").unwrap(), "legacy-a");
        assert_eq!(bundle.keys.get("new-a").unwrap(), "current-a");
        assert_eq!(bundle.keys.get("new-b").unwrap(), "legacy-b");
        assert!(!bundle.keys.contains_key("old-b"));
        assert_eq!(super::relocate_bundle_keys(&mut bundle, &moves), 0);
    }

    /// The static roster is one account per pinned vendor plus STT and GitHub;
    /// legacy lane accounts are gone and custom accounts are known by shape.
    #[test]
    fn static_accounts_are_per_vendor_and_custom_accounts_are_known_by_shape() {
        assert_eq!(
            KEYCHAIN_ACCOUNTS,
            &[
                "LLM_LIBRAXIS_API_KEY",
                "LLM_OPENAI_API_KEY",
                "LLM_XAI_API_KEY",
                "LLM_ANTHROPIC_API_KEY",
                "STT_FILE_API_KEY",
                "STT_LIVE_API_KEY",
                "GITHUB_TOKEN",
            ]
        );
        assert!(is_known_account("LLM_LIBRAXIS_API_KEY"));
        assert!(is_known_account("LLM_CUSTOM_MY_LOCAL_API_KEY"));
        assert!(!is_known_account("LLM_API_KEY"));
        assert!(!is_known_account("LLM_FORMATTING_API_KEY"));
        assert!(!is_known_account("LLM_ASSISTIVE_API_KEY"));
    }

    /// Effect witness: a custom-provider key is read from the bundle only. An
    /// env var of the same name (never legitimately set) is ignored, and
    /// `key_present` follows the same rule.
    #[test]
    #[serial]
    fn custom_provider_keys_come_from_the_bundle_never_from_env() {
        let account = "LLM_CUSTOM_MY_LOCAL_API_KEY";
        // SAFETY: serial test; the var is removed again below.
        unsafe { std::env::set_var(account, "from-env") };
        {
            let _bundle = test_support::install_bundle(&[]);
            assert_eq!(cached_runtime_key(account), None);
            assert!(!key_present(account));
        }
        {
            let _bundle = test_support::install_bundle(&[(account, " from-bundle ")]);
            assert_eq!(cached_runtime_key(account).as_deref(), Some("from-bundle"));
            assert!(key_present(account));
        }
        unsafe { std::env::remove_var(account) };
    }

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

#[cfg(test)]
mod stt_tests {
    use super::*;

    #[test]
    #[serial_test::serial]
    fn retry_fan_out_finishes_a_migration_the_keychain_outage_left_behind() {
        let _bundle = test_support::install_bundle(&[("STT_API_KEY", "retired")]);
        assert_eq!(super::retry_stt_key_fan_out(), 3);
        let bundle = read_bundle_cache().unwrap();
        assert_eq!(
            bundle.keys.get("STT_FILE_API_KEY").map(String::as_str),
            Some("retired")
        );
        assert_eq!(
            bundle.keys.get("STT_LIVE_API_KEY").map(String::as_str),
            Some("retired")
        );
        assert!(!bundle.keys.contains_key("STT_API_KEY"));
        assert_eq!(super::retry_stt_key_fan_out(), 0);
    }

    #[test]
    #[serial_test::serial]
    fn fan_out_key_copies_to_absent_targets_then_drains_source() {
        let _bundle =
            test_support::install_bundle(&[("legacy-stt", "old"), ("STT_LIVE_API_KEY", "current")]);
        assert_eq!(
            fan_out_key("legacy-stt", &["STT_FILE_API_KEY", "STT_LIVE_API_KEY"]),
            2
        );
        let bundle = read_bundle_cache().unwrap();
        assert_eq!(
            bundle.keys.get("STT_FILE_API_KEY").map(String::as_str),
            Some("old")
        );
        assert_eq!(
            bundle.keys.get("STT_LIVE_API_KEY").map(String::as_str),
            Some("current")
        );
        assert!(!bundle.keys.contains_key("legacy-stt"));
        assert_eq!(
            fan_out_key("legacy-stt", &["STT_FILE_API_KEY", "STT_LIVE_API_KEY"]),
            0
        );
    }
}

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

const SERVICE: &str = "com.vetcoders.codescribe";
const BUNDLE_ACCOUNT: &str = "codescribe_keychain_bundle_v1";

/// Known API key accounts stored in Keychain.
pub const KEYCHAIN_ACCOUNTS: &[&str] = &[
    "LLM_API_KEY",
    "STT_API_KEY",
    "LLM_FORMATTING_API_KEY",
    "LLM_ASSISTIVE_API_KEY",
    "LLM_ANTHROPIC_API_KEY",
    "GITHUB_TOKEN",
];

#[derive(Debug, Clone, Serialize, Deserialize)]
struct KeychainBundle {
    version: u8,
    keys: BTreeMap<String, String>,
}

impl Default for KeychainBundle {
    fn default() -> Self {
        Self {
            version: 1,
            keys: BTreeMap::new(),
        }
    }
}

static BUNDLE_CACHE: OnceLock<RwLock<Option<KeychainBundle>>> = OnceLock::new();
static POPULATE_ONCE: Once = Once::new();

fn bundle_cache() -> &'static RwLock<Option<KeychainBundle>> {
    BUNDLE_CACHE.get_or_init(|| RwLock::new(None))
}

fn read_bundle_cache() -> Option<KeychainBundle> {
    match bundle_cache().read() {
        Ok(guard) => guard.clone(),
        Err(poisoned) => poisoned.into_inner().clone(),
    }
}

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

fn encode_bundle(bundle: &KeychainBundle) -> Result<Vec<u8>> {
    let json = serde_json::to_string(bundle).context("Failed to serialize bundle")?;
    let b64 = base64::engine::general_purpose::STANDARD.encode(json.as_bytes());
    Ok(format!("b64:{b64}").into_bytes())
}

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
            debug!("No Keychain bundle entry: {e}");
            None
        }
    }
}

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
                debug!("Set {account} from Keychain bundle");
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::keychain_disabled_by_signals;

    #[test]
    fn disable_keychain_flag_is_honored() {
        // Explicit user opt-out disables Keychain regardless of other signals.
        assert!(keychain_disabled_by_signals(true, false, false));
        assert!(keychain_disabled_by_signals(true, true, true));
    }

    #[test]
    fn data_dir_and_ci_do_not_disable_keychain() {
        // Regression: setting CODESCRIBE_DATA_DIR (a documented data-dir override) or CI
        // must NOT silently disable Keychain persistence of API keys / OAuth tokens.
        assert!(!keychain_disabled_by_signals(false, true, false));
        assert!(!keychain_disabled_by_signals(false, false, true));
        assert!(!keychain_disabled_by_signals(false, true, true));
        assert!(!keychain_disabled_by_signals(false, false, false));
    }
}

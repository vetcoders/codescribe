//! Global configuration state management.
//!
//! Provides thread-safe global access to CodeScribe configuration.

use std::sync::{OnceLock, RwLock};

use super::types::Config;

/// Thread-safe global configuration instance
static GLOBAL_CONFIG: OnceLock<RwLock<Config>> = OnceLock::new();

/// Initialize global configuration.
///
/// Should be called once at application startup.
pub fn init() {
    let config = Config::load();
    GLOBAL_CONFIG.get_or_init(|| RwLock::new(config));
}

/// Get read access to global configuration.
///
/// # Panics
/// Panics if called before `init()`.
pub fn get() -> std::sync::RwLockReadGuard<'static, Config> {
    GLOBAL_CONFIG
        .get()
        .expect("Config not initialized - call config::init() first")
        .read()
        .expect("Config lock poisoned")
}

/// Update global configuration.
///
/// # Example
/// ```rust,no_run
/// use codescribe::config;
///
/// config::update(|c| {
///     c.beep_on_start = false;
///     c.sound_volume = 0.5;
/// });
/// ```
pub fn update<F>(f: F)
where
    F: FnOnce(&mut Config),
{
    let mut config = GLOBAL_CONFIG
        .get()
        .expect("Config not initialized - call config::init() first")
        .write()
        .expect("Config lock poisoned");

    f(&mut config);
    config.sanitize();
}

/// Save current global configuration to .env file.
pub fn save() -> anyhow::Result<()> {
    let config = get();
    config.save_all_to_env()
}

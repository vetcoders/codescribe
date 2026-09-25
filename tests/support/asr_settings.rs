//! Isolated settings authority for explicitly selected real-audio harness runs.
use codescribe_core::config::{Config, UserSettings};

pub struct IsolatedAsrSettings {
    _root: tempfile::TempDir,
    previous: Vec<(&'static str, Option<std::ffi::OsString>)>,
    pub settings: UserSettings,
}

impl IsolatedAsrSettings {
    /// Call before starting workers; run one real-audio test per process.
    pub fn from_requested_mode() -> Self {
        let mode = std::env::var("CODESCRIBE_ASR_MODE")
            .unwrap_or_else(|_| "apple_only".to_string());
        assert!(matches!(mode.as_str(), "apple_only" | "local_power"));
        // Preserve measured calibration bytes, never invent a capture profile.
        let calibration_path = codescribe_core::config::energy_calibration_path();
        let calibration = std::fs::read(&calibration_path).ok();
        let root = tempfile::tempdir().expect("isolated ASR data directory");
        let previous = ["CODESCRIBE_DATA_DIR", "CODESCRIBE_ENV_PATH"]
            .into_iter()
            .map(|key| (key, std::env::var_os(key)))
            .collect();
        let settings = UserSettings {
            asr_mode: Some(mode.clone()),
            ..UserSettings::default()
        };
        let isolated = Self {
            _root: root,
            previous,
            settings,
        };
        // SAFETY: ignored real-audio harness starts no workers before this call.
        unsafe {
            std::env::set_var("CODESCRIBE_DATA_DIR", isolated._root.path());
            std::env::set_var(
                "CODESCRIBE_ENV_PATH",
                isolated._root.path().join("absent.env"),
            );
        }
        if let Some(bytes) = calibration {
            std::fs::write(codescribe_core::config::energy_calibration_path(), bytes)
                .expect("copy measured calibration into isolated runtime");
        }
        isolated.settings.save().expect("save isolated ASR settings");
        let snapshot = Config::load_runtime_snapshot_without_keychain().expect("ASR snapshot");
        let (_, receipt) = codescribe_core::asr_session::layer1_decision(&snapshot);
        assert_eq!(receipt.asr_mode, mode);
        assert_eq!(
            receipt.refiner,
            if mode == "apple_only" { "off" } else { "local_tail_patch" }
        );
        isolated
    }
}

impl Drop for IsolatedAsrSettings {
    fn drop(&mut self) {
        for (key, value) in &self.previous {
            // SAFETY: harness workers have ended before the owner is dropped.
            unsafe {
                match value {
                    Some(value) => std::env::set_var(key, value),
                    None => std::env::remove_var(key),
                }
            }
        }
    }
}

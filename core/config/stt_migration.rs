//! Legacy URL inversion exists only at this one-shot migration boundary.
use super::settings::UserSettings;
use crate::stt::{SttLane, validate_stt_endpoint};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SttV2Legacy {
    pub cloud_transcription_endpoint: Option<String>,
    file_transcription_endpoint: Option<String>,
    live_transcription_endpoint: Option<String>,
}

fn optional_json_str(raw: &serde_json::Value, pointers: &[&str]) -> Option<String> {
    pointers
        .iter()
        .find_map(|pointer| raw.pointer(pointer))
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

impl SttV2Legacy {
    pub fn from_json(raw: &serde_json::Value) -> Self {
        Self {
            cloud_transcription_endpoint: optional_json_str(
                raw,
                &[
                    "/speech/engine/cloud_transcription_endpoint",
                    "/stt_endpoint",
                ],
            ),
            file_transcription_endpoint: optional_json_str(
                raw,
                &["/speech/engine/file_transcription_endpoint"],
            ),
            live_transcription_endpoint: optional_json_str(
                raw,
                &["/speech/engine/live_transcription_endpoint"],
            ),
        }
    }
    pub(crate) fn from_endpoint(raw: &str) -> Self {
        Self {
            cloud_transcription_endpoint: Some(raw.into()),
            ..Self::default()
        }
    }
    /// True only when the legacy URL is still present *and* at least one
    /// destination lane would be written. Presence of the retired key after
    /// file/live rows already exist is not a migration: launch repair can
    /// re-seed `cloud_transcription_endpoint` from the operator pack, and
    /// treating `is_some()` as the trigger saved the file on every load.
    pub fn needs_migration(&self) -> bool {
        let mut probe = UserSettings {
            stt_file_endpoint: self.file_transcription_endpoint.clone(),
            stt_live_endpoint: self.live_transcription_endpoint.clone(),
            ..UserSettings::default()
        };
        !migrate_legacy_stt_lanes(self, &mut probe).0.is_empty()
    }
}
pub struct SttMigrationStep {
    pub from: &'static str,
    pub to: &'static str,
    pub value: String,
}

fn invert_live_to_file(raw: &str) -> Option<String> {
    let mut url = reqwest::Url::parse(raw).ok()?;
    if !url.path().ends_with("/transcribe") {
        return None;
    }
    let scheme = match url.scheme() {
        "ws" => "http",
        "wss" => "https",
        _ => return None,
    };
    let loopback = crate::stt::tail_provider::stt_auth_mode(raw)
        == crate::stt::tail_provider::SttAuthMode::Unauthenticated;
    url.set_scheme(scheme).ok()?;
    url.set_path(&(url.path().trim_end_matches("transcribe").to_owned() + "transcriptions"));
    url.set_query(None);
    url.set_fragment(None);
    if loopback && url.port() == Some(8446) {
        url.set_port(Some(8444)).ok()?;
    }
    validate_stt_endpoint(SttLane::File, url.as_str()).ok()
}

/// R1–R4, without I/O. Existing explicit rows win over a legacy fallback.
pub fn migrate_legacy_stt_lanes(
    legacy: &SttV2Legacy,
    settings: &mut UserSettings,
) -> (Vec<SttMigrationStep>, Vec<&'static str>) {
    let mut steps = Vec::new();
    if let Some(raw) = legacy.cloud_transcription_endpoint.as_deref() {
        let (file, live) = if let Ok(live) = validate_stt_endpoint(SttLane::Live, raw) {
            (invert_live_to_file(&live), Some(live))
        } else if let Ok(file) = validate_stt_endpoint(SttLane::File, raw) {
            (Some(file), None)
        } else {
            tracing::warn!("Legacy STT endpoint is invalid; no lane synthesized");
            (None, None)
        };
        for (lane, target, value) in [
            (SttLane::File, &mut settings.stt_file_endpoint, file),
            (SttLane::Live, &mut settings.stt_live_endpoint, live),
        ] {
            if target.is_none()
                && let Some(value) = value
            {
                steps.push(SttMigrationStep {
                    from: "STT_ENDPOINT",
                    to: lane.wire_key(),
                    value: value.clone(),
                });
                *target = Some(value);
            }
        }
    }
    let mut targets: Vec<&'static str> = [
        (SttLane::File, &settings.stt_file_endpoint),
        (SttLane::Live, &settings.stt_live_endpoint),
    ]
    .into_iter()
    .filter(|(_, row)| row.as_deref().is_some_and(|v| !v.trim().is_empty()))
    .map(|(lane, _)| lane.key_account())
    .collect();
    if targets.is_empty() {
        targets.push(SttLane::File.key_account());
    }
    (steps, targets)
}

/// Retired `STT_ENDPOINT` alias split into `(file, live)` rows; warned once per process.
pub(crate) fn split_retired_stt_endpoint(raw: &str) -> (Option<String>, Option<String>) {
    static WARN: std::sync::Once = std::sync::Once::new();
    WARN.call_once(|| {
        tracing::warn!(
            "STT_ENDPOINT is retired; use STT_FILE_ENDPOINT / STT_LIVE_ENDPOINT (removed after 2026-10-15)"
        )
    });
    let mut settings = UserSettings::default();
    migrate_legacy_stt_lanes(&SttV2Legacy::from_endpoint(raw), &mut settings);
    (settings.stt_file_endpoint, settings.stt_live_endpoint)
}

/// Called with the settings transaction lock held, before legacy fields are serialized away.
pub fn migrate_legacy_stt_lanes_once(settings: &mut UserSettings) {
    let raw = std::fs::read(UserSettings::settings_path())
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or(serde_json::Value::Null);
    let legacy = SttV2Legacy::from_json(&raw);
    let (steps, _) = migrate_legacy_stt_lanes(&legacy, settings);
    if steps.is_empty() {
        // A write here would serialize SettingsV2 (no cloud key) and let the
        // next load's pack-seed repair put the key back — the 2026-09-08
        // `Saved settings` / `Migrated legacy STT lanes rows=0` storm.
        return;
    }
    // Settings projection never acquires credentials. The source account stays
    // intact until the authorized loader performs atomic STT key fan-out.
    match settings.save_unlocked() {
        Ok(()) => {
            *settings = UserSettings::from_v2(settings.to_v2());
            tracing::info!(rows = steps.len(), "Migrated legacy STT lanes");
        }
        Err(error) => tracing::warn!(%error, "Failed to persist STT lane migration"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FOUNDER: &str = "wss://api.libraxis.cloud/v1/audio/transcribe";

    #[test]
    fn migrates_founder_wss_socket_into_both_lanes() {
        let mut settings = UserSettings::default();
        let legacy = SttV2Legacy::from_json(
            &serde_json::json!({"speech":{"engine":{"cloud_transcription_endpoint":FOUNDER}}}),
        );
        let (steps, targets) = migrate_legacy_stt_lanes(&legacy, &mut settings);
        assert_eq!(settings.stt_live_endpoint.as_deref(), Some(FOUNDER));
        assert_eq!(
            settings.stt_file_endpoint.as_deref(),
            Some("https://api.libraxis.cloud/v1/audio/transcriptions")
        );
        assert_eq!(targets, ["STT_FILE_API_KEY", "STT_LIVE_API_KEY"]);
        assert_eq!(steps.len(), 2);
    }

    #[test]
    fn http_legacy_lands_in_file_lane_only() {
        let mut settings = UserSettings::default();
        let legacy =
            SttV2Legacy::from_json(&serde_json::json!({"stt_endpoint":"https://example.com/stt"}));
        let (_, targets) = migrate_legacy_stt_lanes(&legacy, &mut settings);
        assert_eq!(
            settings.stt_file_endpoint.as_deref(),
            Some("https://example.com/stt")
        );
        assert_eq!(settings.stt_live_endpoint, None);
        assert_eq!(targets, ["STT_FILE_API_KEY"]);
        for raw in [
            "not a URL",
            "wss://example.com/live",
            "ws://127.0.0.1:8446/v1/audio/transcribe?q=x#f",
        ] {
            let mut settings = UserSettings::default();
            let legacy = SttV2Legacy {
                cloud_transcription_endpoint: Some(raw.into()),
                ..SttV2Legacy::default()
            };
            migrate_legacy_stt_lanes(&legacy, &mut settings);
            if raw.contains("8446") {
                assert_eq!(
                    settings.stt_file_endpoint.as_deref(),
                    Some("http://127.0.0.1:8444/v1/audio/transcriptions")
                );
            } else {
                assert!(settings.stt_file_endpoint.is_none());
            }
        }
    }

    #[test]
    #[serial_test::serial]
    fn migration_is_idempotent_on_second_load() {
        let dir = tempfile::tempdir().unwrap();
        let previous = std::env::var_os("CODESCRIBE_DATA_DIR");
        // SAFETY: serialized test; restore the isolated data root after the witness.
        unsafe {
            std::env::set_var("CODESCRIBE_DATA_DIR", dir.path());
        }
        struct Restore(Option<std::ffi::OsString>);
        impl Drop for Restore {
            fn drop(&mut self) {
                // SAFETY: same serialized environment scope.
                unsafe {
                    match &self.0 {
                        Some(v) => std::env::set_var("CODESCRIBE_DATA_DIR", v),
                        None => std::env::remove_var("CODESCRIBE_DATA_DIR"),
                    }
                }
            }
        }
        let _restore = Restore(previous);
        std::fs::create_dir_all(UserSettings::settings_dir()).unwrap();
        for raw in [
            serde_json::json!({"stt_endpoint":FOUNDER}),
            serde_json::json!({"schema_version":3,"speech":{"engine":{"cloud_transcription_endpoint":FOUNDER},"llm_endpoint":"https://api.libraxis.cloud/v1/responses"}}),
        ] {
            std::fs::write(UserSettings::settings_path(), raw.to_string()).unwrap();
            let first = UserSettings::load();
            let bytes = std::fs::read(UserSettings::settings_path()).unwrap();
            let second = UserSettings::load();
            assert_eq!(first, second);
            assert_eq!(first.stt_live_endpoint.as_deref(), Some(FOUNDER));
            assert_eq!(
                first.stt_file_endpoint.as_deref(),
                Some("https://api.libraxis.cloud/v1/audio/transcriptions")
            );
            assert_eq!(bytes, std::fs::read(UserSettings::settings_path()).unwrap());
            assert!(
                !SttV2Legacy::from_json(&serde_json::from_slice(&bytes).unwrap()).needs_migration()
            );
        }
    }

    #[test]
    #[serial_test::serial]
    fn load_of_migrated_settings_never_opens_the_keychain() {
        // Bisect 2026-09-08: an unsigned `target/debug/codescribe transcribe`
        // hung in `SecItemCopyMatching` because every load retried the
        // `STT_API_KEY` fan-out. A migrated file must load without touching
        // the bundle; the retry belongs to the loader's Keychain step.
        let dir = tempfile::tempdir().unwrap();
        let previous = std::env::var_os("CODESCRIBE_DATA_DIR");
        // SAFETY: serialized test; restore the isolated data root after the witness.
        unsafe {
            std::env::set_var("CODESCRIBE_DATA_DIR", dir.path());
        }
        struct Restore(Option<std::ffi::OsString>);
        impl Drop for Restore {
            fn drop(&mut self) {
                // SAFETY: same serialized environment scope.
                unsafe {
                    match &self.0 {
                        Some(v) => std::env::set_var("CODESCRIBE_DATA_DIR", v),
                        None => std::env::remove_var("CODESCRIBE_DATA_DIR"),
                    }
                }
            }
        }
        let _restore = Restore(previous);
        std::fs::create_dir_all(UserSettings::settings_dir()).unwrap();
        let migrated = serde_json::json!({"schema_version":3,"speech":{"engine":{
            "file_transcription_endpoint":"https://api.libraxis.cloud/v1/audio/transcriptions",
            "live_transcription_endpoint":FOUNDER}}});
        std::fs::write(UserSettings::settings_path(), migrated.to_string()).unwrap();
        let _bundle =
            super::super::keychain::test_support::install_bundle(&[("STT_API_KEY", "retired")]);
        let loaded = UserSettings::load();
        assert_eq!(loaded.stt_live_endpoint.as_deref(), Some(FOUNDER));
        let bundle = super::super::keychain::test_support::snapshot_bundle().unwrap();
        assert_eq!(
            bundle.get("STT_API_KEY").map(String::as_str),
            Some("retired")
        );
        assert!(!bundle.contains_key("STT_FILE_API_KEY"));
        assert!(!bundle.contains_key("STT_LIVE_API_KEY"));
    }

    struct IsolatedSettings {
        _dir: tempfile::TempDir,
        previous_data: Option<std::ffi::OsString>,
        previous_pack: Option<std::ffi::OsString>,
    }

    impl IsolatedSettings {
        fn new() -> Self {
            let dir = tempfile::tempdir().unwrap();
            let previous_data = std::env::var_os("CODESCRIBE_DATA_DIR");
            let previous_pack = std::env::var_os("CODESCRIBE_VOICE_LAB_SRC");
            // SAFETY: serial tests; Drop restores both variables.
            unsafe {
                std::env::set_var("CODESCRIBE_DATA_DIR", dir.path());
                std::env::remove_var("CODESCRIBE_VOICE_LAB_SRC");
            }
            std::fs::create_dir_all(UserSettings::settings_dir()).unwrap();
            Self {
                _dir: dir,
                previous_data,
                previous_pack,
            }
        }

        fn path(&self) -> std::path::PathBuf {
            UserSettings::settings_path()
        }
    }

    impl Drop for IsolatedSettings {
        fn drop(&mut self) {
            // SAFETY: same serialized environment scope as `new`.
            unsafe {
                match &self.previous_data {
                    Some(value) => std::env::set_var("CODESCRIBE_DATA_DIR", value),
                    None => std::env::remove_var("CODESCRIBE_DATA_DIR"),
                }
                match &self.previous_pack {
                    Some(value) => std::env::set_var("CODESCRIBE_VOICE_LAB_SRC", value),
                    None => std::env::remove_var("CODESCRIBE_VOICE_LAB_SRC"),
                }
            }
        }
    }

    /// The 2026-09-08 storm shape: W2 lanes already populated, retired key still
    /// on disk (repair re-seeds it). A load must not rewrite the file.
    fn storm_settings_json() -> serde_json::Value {
        serde_json::json!({
            "schema_version": 3,
            "speech": {
                "engine": {
                    "cloud_transcription_endpoint": FOUNDER,
                    "file_transcription_endpoint": "https://api.libraxis.cloud/v1/audio/transcriptions",
                    "live_transcription_endpoint": FOUNDER
                }
            }
        })
    }

    fn with_save_log_count<R>(f: impl FnOnce() -> R) -> (R, usize) {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicUsize, Ordering};
        use tracing::field::{Field, Visit};

        struct Counter(Arc<AtomicUsize>);
        struct Message(String);
        impl Visit for Message {
            fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
                if field.name() == "message" {
                    self.0 = format!("{value:?}");
                }
            }
            fn record_str(&mut self, field: &Field, value: &str) {
                if field.name() == "message" {
                    self.0 = value.to_owned();
                }
            }
        }
        impl tracing::Subscriber for Counter {
            fn enabled(&self, _: &tracing::Metadata<'_>) -> bool {
                true
            }
            fn new_span(&self, _: &tracing::span::Attributes<'_>) -> tracing::span::Id {
                tracing::span::Id::from_u64(1)
            }
            fn record(&self, _: &tracing::span::Id, _: &tracing::span::Record<'_>) {}
            fn record_follows_from(&self, _: &tracing::span::Id, _: &tracing::span::Id) {}
            fn event(&self, event: &tracing::Event<'_>) {
                let mut message = Message(String::new());
                event.record(&mut message);
                if message.0.contains("Saved settings") {
                    self.0.fetch_add(1, Ordering::SeqCst);
                }
            }
            fn enter(&self, _: &tracing::span::Id) {}
            fn exit(&self, _: &tracing::span::Id) {}
        }

        let count = Arc::new(AtomicUsize::new(0));
        let subscriber = Counter(count.clone());
        let result = tracing::subscriber::with_default(subscriber, f);
        (result, count.load(Ordering::SeqCst))
    }

    #[test]
    fn empty_legacy_key_does_not_need_migration() {
        let blank = SttV2Legacy::from_json(&serde_json::json!({
            "speech": {"engine": {"cloud_transcription_endpoint": ""}}
        }));
        assert!(!blank.needs_migration());
        let already = SttV2Legacy::from_json(&storm_settings_json());
        assert!(
            !already.needs_migration(),
            "legacy present AND targets present is already migrated"
        );
    }

    #[test]
    #[serial_test::serial]
    fn migrate_legacy_stt_lanes_once_with_empty_steps_performs_no_write() {
        let isolated = IsolatedSettings::new();
        let path = isolated.path();
        std::fs::write(&path, storm_settings_json().to_string()).unwrap();
        let before = std::fs::metadata(&path).unwrap();
        let bytes_before = std::fs::read(&path).unwrap();
        let mut settings = UserSettings {
            stt_file_endpoint: Some("https://api.libraxis.cloud/v1/audio/transcriptions".into()),
            stt_live_endpoint: Some(FOUNDER.into()),
            ..UserSettings::default()
        };
        let (_, saves) = with_save_log_count(|| migrate_legacy_stt_lanes_once(&mut settings));
        assert_eq!(saves, 0);
        assert_eq!(bytes_before, std::fs::read(&path).unwrap());
        let after = std::fs::metadata(&path).unwrap();
        assert_eq!(before.modified().unwrap(), after.modified().unwrap());
    }

    #[test]
    #[serial_test::serial]
    fn load_of_legacy_key_with_targets_is_pure_on_second_pass() {
        let isolated = IsolatedSettings::new();
        let path = isolated.path();
        std::fs::write(&path, storm_settings_json().to_string()).unwrap();
        let (first, first_saves) = with_save_log_count(UserSettings::load);
        let bytes_after_first = std::fs::read(&path).unwrap();
        let mtime_after_first = std::fs::metadata(&path).unwrap().modified().unwrap();
        assert!(
            !SttV2Legacy::from_json(&serde_json::from_slice(&bytes_after_first).unwrap())
                .needs_migration()
        );
        assert_eq!(first.stt_live_endpoint.as_deref(), Some(FOUNDER));
        let (second, second_saves) = with_save_log_count(UserSettings::load);
        assert_eq!(first, second);
        assert_eq!(second_saves, 0, "second load must not emit Saved settings");
        assert_eq!(bytes_after_first, std::fs::read(&path).unwrap());
        assert_eq!(
            mtime_after_first,
            std::fs::metadata(&path).unwrap().modified().unwrap()
        );
        assert_eq!(
            first_saves, 0,
            "targets already present: first load must not save either"
        );
    }

    #[test]
    #[serial_test::serial]
    fn runtime_snapshot_load_loop_is_read_only_and_survives_unreadable_file() {
        let isolated = IsolatedSettings::new();
        let path = isolated.path();
        std::fs::write(&path, storm_settings_json().to_string()).unwrap();
        let _ = crate::config::Config::load_runtime_snapshot();
        let bytes = std::fs::read(&path).unwrap();
        let mtime = std::fs::metadata(&path).unwrap().modified().unwrap();
        for _ in 0..50 {
            let snapshot = crate::config::Config::load_runtime_snapshot();
            assert!(snapshot.is_ok());
        }
        assert_eq!(bytes, std::fs::read(&path).unwrap());
        assert_eq!(mtime, std::fs::metadata(&path).unwrap().modified().unwrap());

        let mode = std::fs::metadata(&path).unwrap().permissions();
        let mut locked = mode.clone();
        locked.set_readonly(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            locked.set_mode(0o000);
        }
        std::fs::set_permissions(&path, locked).unwrap();
        let result = std::panic::catch_unwind(|| {
            for _ in 0..50 {
                let _ = crate::config::Config::load_runtime_snapshot();
            }
        });
        std::fs::set_permissions(&path, mode).unwrap();
        assert!(
            result.is_ok(),
            "unreadable settings must not panic the loader"
        );
    }
}

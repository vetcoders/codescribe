pub use super::*;

/// Pins the bridge-side projection of a core key probe.
#[cfg(test)]
mod api_key_probe_tests {
    use super::{ApiKeyLivenessResult, ApiKeyLivenessStatus, CsApiKeyProbeResult};

    /// The endpoint the core actually probed must survive the FFI conversion:
    /// without it, "Invalid" on a self-hosted endpoint is indistinguishable
    /// from "Invalid" against the vendor cloud.
    #[test]
    fn bridge_probe_result_preserves_the_endpoint_used_by_core() {
        let result = CsApiKeyProbeResult::from(ApiKeyLivenessResult {
            account: "LLM_OPENAI_API_KEY".to_string(),
            status: ApiKeyLivenessStatus::Invalid,
            message: "provider rejected this key".to_string(),
            probed_endpoint: Some("https://api.libraxis.cloud/v1/responses".to_string()),
        });

        assert_eq!(
            result.probed_endpoint.as_deref(),
            Some("https://api.libraxis.cloud/v1/responses")
        );
    }
}

/// Covers the destructive reset path against real temp directories: what the
/// reset scope resolves to, that data lands recoverably in a Trash stand-in,
/// that the audit log only ever grows, and that the cross-volume fallback
/// copies before it removes.
#[cfg(test)]
mod reset_tests {
    use super::{
        CsResetPreview, ResetAuditEvent, agent_connector_secret_accounts, agent_env_keys,
        agent_reset_error, agent_reset_paths, agent_reset_preview_for_paths, agent_secret_accounts,
        app_data_dirs, append_reset_audit, capture_base_prompts, clear_mcp_configuration_to,
        create_reset_destination, move_path_recoverably, move_path_recoverably_with,
        move_reset_dirs_to_destination, remove_path_without_following_symlinks,
        reset_preview_for_dirs, restore_base_prompts,
    };
    use chrono::{DateTime, Utc};
    use codescribe_core::config::{Config, begin_app_data_reset};
    use serial_test::serial;
    use std::path::{Path, PathBuf};
    use std::time::{SystemTime, UNIX_EPOCH};

    /// Unique scratch directory under the OS temp dir (never the real home).
    fn scratch(tag: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        std::env::temp_dir().join(format!("cs_reset_{}_{tag}_{nanos}", std::process::id()))
    }

    use codescribe_core::test_isolation::EnvGuard;

    #[test]
    fn transcription_controls_bridge_projects_each_frozen_combination() {
        use codescribe_core::config::CapturedRuntimeInputs;

        for (ledger, apple) in [(false, false), (false, true), (true, false), (true, true)] {
            let mut input = CapturedRuntimeInputs::defaults_at(
                std::path::PathBuf::from("/fixture/transcription-controls"),
                1_700_000_000_000,
            );
            let original = Config::runtime_snapshot_from_captured(input.clone());
            input.user_settings.seal_ledger_enabled = Some(ledger);
            input.user_settings.apple_live_enabled = Some(apple);
            let snapshot = Config::runtime_snapshot_from_captured(input);
            let projected = super::CsSettings::from_runtime_snapshot(&snapshot);
            assert_eq!(projected.seal_ledger_enabled, ledger);
            assert_eq!(projected.apple_live_enabled, apple);
            let unchanged = super::CsSettings::from_runtime_snapshot(&original);
            assert!(!unchanged.seal_ledger_enabled);
            assert!(unchanged.apple_live_enabled);
        }
    }

    #[test]
    fn resolved_asr_projection_uses_the_same_seal_as_the_session_factory() {
        use super::CsSettings;
        use codescribe_core::config::CapturedRuntimeInputs;

        let cases = [
            (None, None, None, "apple_only"),
            (None, None, Some(true), "local_power"),
            (None, None, Some(false), "cloud"),
            (Some("apple_only"), None, Some(true), "apple_only"),
            (Some("local_power"), None, None, "local_power"),
            (Some("cloud"), None, None, "apple_only"),
            (Some("cloud"), Some("denied"), None, "apple_only"),
            (Some("cloud"), Some("granted"), None, "cloud"),
        ];
        for (mode, consent, local, expected) in cases {
            let mut input = CapturedRuntimeInputs::defaults_at(
                std::path::PathBuf::from("/fixture/asr-projection"),
                1_700_000_000_000,
            );
            input.user_settings.asr_mode = mode.map(str::to_owned);
            input.user_settings.cloud_consent = consent.map(str::to_owned);
            input.user_settings.use_local_stt = local;
            let snapshot = Config::runtime_snapshot_from_captured(input);
            let projected = CsSettings::from_runtime_snapshot(&snapshot);
            assert_eq!(projected.resolved_asr_mode, expected);
            assert_eq!(
                projected.resolved_asr_mode,
                snapshot.user_settings().resolved_asr_mode().mode.as_str()
            );
            assert_eq!(
                projected.asr_mode.as_deref(),
                mode,
                "retain editable intent"
            );
        }
    }

    /// Write a fixture file, creating its parents. Panics on failure: a fixture
    /// that did not materialize would make the assertions below meaningless.
    fn write(path: &Path, bytes: &[u8]) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("create fixture parent");
        }
        std::fs::write(path, bytes).expect("write fixture");
    }

    /// Frozen reset timestamp so audit-line assertions can compare the rendered
    /// RFC3339 string exactly instead of matching a shape.
    fn fixed_timestamp() -> DateTime<Utc> {
        "2026-07-16T16:15:00Z"
            .parse()
            .expect("valid fixed reset timestamp")
    }

    #[test]
    #[serial]
    fn agent_reset_scope_moves_only_agent_files_and_keeps_dictation_roots() {
        let sandbox = scratch("agent_scope");
        let root = sandbox.join("data");
        let trash = sandbox.join("trash");
        write(
            &root.join("threads/index.json"),
            br#"{"threads":[{"id":"one"}]}"#,
        );
        write(&root.join("threads/one.json"), b"agent conversation");
        write(&root.join("threads/blobs/image.png"), b"agent attachment");
        write(&root.join("mcp.json"), b"{\"mcpServers\":{}}");
        write(&root.join("tool_grants.json"), b"{\"always_allow\":{}}");
        write(
            &root.join("transcriptions/2026-08-15/voice.txt"),
            b"must survive",
        );
        write(&root.join("prompts/assistive.txt"), b"must survive");
        write(&root.join("dictionary/custom.json"), b"must survive");
        let _data_dir = EnvGuard::set("CODESCRIBE_DATA_DIR", &root);

        let paths = agent_reset_paths();
        let preview = agent_reset_preview_for_paths(&paths);
        assert_eq!(preview.threads, 1);
        assert_eq!(preview.files, 5);

        let destination = trash.join("codescribe-agent-reset-test");
        std::fs::create_dir_all(&destination).expect("create agent Trash destination");
        for source in &paths {
            if source.exists() {
                let name = source.file_name().expect("Agent fixture path name");
                move_path_recoverably(source, &destination.join(name))
                    .expect("move only Agent-owned path");
            }
        }

        assert!(destination.join("threads/one.json").is_file());
        assert!(destination.join("mcp.json").is_file());
        assert!(root.join("transcriptions/2026-08-15/voice.txt").is_file());
        assert!(root.join("prompts/assistive.txt").is_file());
        assert!(root.join("dictionary/custom.json").is_file());
        let _ = std::fs::remove_dir_all(&sandbox);
    }

    #[test]
    fn agent_reset_secret_scope_excludes_stt_and_non_agent_keys() {
        let accounts = agent_secret_accounts();
        assert!(accounts.contains(&"LLM_OPENAI_API_KEY"));
        assert!(accounts.contains(&"LLM_OPENAI_ACCOUNT_TOKENS"));
        assert!(!accounts.contains(&"STT_FILE_API_KEY"));
        assert!(!accounts.contains(&"STT_LIVE_API_KEY"));
        assert!(!accounts.contains(&"GITHUB_TOKEN"));
    }

    #[test]
    #[serial]
    fn agent_reset_connector_secret_scope_uses_only_managed_auth_refs() {
        let sandbox = scratch("agent_connector_secret_scope");
        write(
            &sandbox.join("mcp.json"),
            br#"{
              "mcpServers": {
                "managed": {
                  "command": "managed",
                  "args": [],
                  "auth_ref": "MCP_CONNECTOR_MANAGED_TOKEN"
                },
                "shared": {
                  "command": "shared",
                  "args": [],
                  "auth_ref": "GITHUB_TOKEN"
                },
                "handwritten": {
                  "command": "handwritten",
                  "args": [],
                  "auth_ref": "my_custom_secret"
                }
              }
            }"#,
        );
        let _data_dir = EnvGuard::set("CODESCRIBE_DATA_DIR", &sandbox);

        assert_eq!(
            agent_connector_secret_accounts().expect("read connector accounts"),
            vec!["MCP_CONNECTOR_MANAGED_TOKEN"]
        );
        let _ = std::fs::remove_dir_all(&sandbox);
    }

    #[test]
    fn agent_reset_marks_only_post_mutation_failures_for_relaunch() {
        assert!(
            !format!("{:?}", agent_reset_error(false, "prepare failed"))
                .contains("CODESCRIBE_AGENT_RESET_RELAUNCH_REQUIRED")
        );
        assert!(
            format!("{:?}", agent_reset_error(true, "Keychain failed"))
                .contains("CODESCRIBE_AGENT_RESET_RELAUNCH_REQUIRED")
        );
    }

    #[test]
    #[serial]
    fn agent_reset_env_scope_removes_only_agent_rows() {
        let sandbox = scratch("agent_env");
        let env_path = sandbox.join(".env");
        write(
            &env_path,
            b"# preserve this comment\nLLM_ASSISTIVE_MODEL=agent-model\nAGENT_WORKSPACE_ROOTS=~/work\nSTT_ENDPOINT=https://stt.example\nWHISPER_LANGUAGE=pl\nLLM_FORMATTING_MODEL=formatter\n",
        );
        let _env_path = EnvGuard::set("CODESCRIBE_ENV_PATH", &env_path);

        Config::remove_env_keys(agent_env_keys()).expect("remove only Agent env rows");
        let rewritten = std::fs::read_to_string(&env_path).expect("read rewritten env");
        assert!(!rewritten.contains("LLM_ASSISTIVE_MODEL="));
        assert!(!rewritten.contains("AGENT_WORKSPACE_ROOTS="));
        assert!(rewritten.contains("# preserve this comment"));
        assert!(rewritten.contains("STT_ENDPOINT=https://stt.example"));
        assert!(rewritten.contains("WHISPER_LANGUAGE=pl"));
        assert!(rewritten.contains("LLM_FORMATTING_MODEL=formatter"));
        let _ = std::fs::remove_dir_all(&sandbox);
    }

    /// The reset scope follows `CODESCRIBE_DATA_DIR`, previews live counts, and
    /// moves the complete source into a recoverable Trash destination.
    #[test]
    #[serial]
    fn reset_scope_follows_data_dir_and_moves_live_data_to_trash() {
        const CHILD_FLAG: &str = "CODESCRIBE_TEST_RESET_SCOPE_CHILD";
        const CHILD_WITNESS: &str = "CODESCRIBE_TEST_RESET_SCOPE_WITNESS";
        const WITNESS_BYTES: &[u8] = b"reset-scope-pass";
        if std::env::var_os(CHILD_FLAG).is_none() {
            let witness_dir = tempfile::TempDir::new().expect("child witness dir");
            let witness = witness_dir.path().join("passed");
            let status = std::process::Command::new(
                std::env::current_exe().expect("current bridge test executable"),
            )
            .args([
                "--exact",
                "config::config_tests::reset_tests::reset_scope_follows_data_dir_and_moves_live_data_to_trash",
                "--nocapture",
            ])
            .env(CHILD_FLAG, "1")
            .env(CHILD_WITNESS, &witness)
            .status()
            .expect("spawn isolated destructive reset regression");
            assert!(
                status.success(),
                "isolated destructive reset regression failed"
            );
            assert_eq!(
                std::fs::read(witness).expect("child completed exact reset-scope test"),
                WITNESS_BYTES,
                "child command exited successfully without executing the exact regression"
            );
            return;
        }

        let sandbox = scratch("scope");
        let root = sandbox.join("source");
        let trash = sandbox.join("trash");
        let audit = sandbox.join("logs/reset-audit.log");
        write(
            &root.join("transcriptions/2026-07-15/one.m4a"),
            b"audio-one",
        );
        write(&root.join("transcriptions/2026-07-15/one.txt"), b"hello");
        write(
            &root.join("transcriptions/2026-07-16/two.wav"),
            b"audio-two",
        );
        write(
            &root.join("threads/index.json"),
            br#"{"version":3,"threads":[{"id":"one"},{"id":"two"}]}"#,
        );
        write(&root.join("settings.json"), b"{}");
        write(
            &root.join("prompts/assistive.txt"),
            b"sacred assistive bytes\n",
        );
        write(
            &root.join("prompts/formatting.txt"),
            b"sacred formatting bytes\n",
        );
        write(
            &root.join("prompts/formatting-smart.txt"),
            b"sacred smart bytes\n",
        );
        write(
            &root.join("prompts/formatting-max.txt"),
            b"sacred max bytes\n",
        );
        let root_canon = root.canonicalize().expect("canonical reset root");
        let _data_dir = EnvGuard::set("CODESCRIBE_DATA_DIR", &root);
        let mut reset_guard = begin_app_data_reset().expect("own reset fence");

        let dirs = app_data_dirs();
        assert!(!dirs.is_empty(), "reset must target at least one dir");
        for dir in &dirs {
            let resolved = dir.canonicalize().unwrap_or_else(|_| dir.clone());
            assert_eq!(
                resolved, root_canon,
                "reset target escaped the data dir: {dir:?}"
            );
        }

        let preview = reset_preview_for_dirs(&dirs);
        assert_eq!(preview.audio_files, 2);
        assert_eq!(preview.transcript_days, 2);
        assert_eq!(preview.threads, 2);
        assert!(preview.total_bytes >= 25);

        let timestamp = fixed_timestamp();
        let destination =
            create_reset_destination(&trash, &timestamp).expect("create test Trash destination");
        let preserved_prompts = capture_base_prompts().expect("capture sacred prompts");
        let moved_paths =
            move_reset_dirs_to_destination(&mut reset_guard, &dirs, &destination, &trash)
                .expect("move reset scope to test Trash");
        restore_base_prompts(&reset_guard, &preserved_prompts).expect("restore sacred prompts");
        assert!(!root.join("settings.json").exists());
        assert_eq!(
            std::fs::read(root.join("prompts/assistive.txt")).expect("read restored assistive"),
            b"sacred assistive bytes\n"
        );
        assert_eq!(
            std::fs::read(root.join("prompts/formatting.txt")).expect("read restored formatting"),
            b"sacred formatting bytes\n"
        );
        assert_eq!(
            std::fs::read(root.join("prompts/formatting-smart.txt")).expect("read restored smart"),
            b"sacred smart bytes\n"
        );
        assert_eq!(
            std::fs::read(root.join("prompts/formatting-max.txt")).expect("read restored max"),
            b"sacred max bytes\n"
        );
        assert!(destination.join("codescribe-data/settings.json").is_file());
        assert!(
            destination
                .join("codescribe-data/transcriptions/2026-07-15/one.m4a")
                .is_file()
        );

        append_reset_audit(&ResetAuditEvent {
            audit_path: &audit,
            timestamp: &timestamp,
            status: "completed",
            source_paths: &dirs,
            moved_paths: &moved_paths,
            trash_path: &destination,
            preview: &preview,
            include_keys: false,
            include_prompts: false,
            preserved_prompt_files: preserved_prompts.len(),
            error: None,
            failed_source: None,
            failed_destination: None,
        })
        .expect("append reset audit");
        let line = std::fs::read_to_string(&audit).expect("read reset audit");
        let entry: serde_json::Value = serde_json::from_str(line.trim()).expect("parse audit JSON");
        assert_eq!(entry["timestamp"], "2026-07-16T16:15:00.000Z");
        assert_eq!(entry["action"], "reset_app_data");
        assert_eq!(entry["status"], "completed");
        assert_eq!(entry["audio_files"], 2);
        assert_eq!(entry["transcript_days"], 2);
        assert_eq!(entry["threads"], 2);
        assert_eq!(entry["include_keys"], false);
        assert_eq!(entry["include_prompts"], false);
        assert_eq!(entry["preserved_prompt_files"], 4);
        assert_eq!(entry["trash_path"], destination.to_string_lossy().as_ref());
        assert!(
            entry["source_paths"]
                .as_array()
                .is_some_and(|paths| paths.len() == 1)
        );
        assert_eq!(
            entry["moved_paths"][0]["source"],
            root_canon.to_string_lossy().as_ref()
        );
        assert_eq!(
            entry["moved_paths"][0]["destination"],
            destination
                .join("codescribe-data")
                .to_string_lossy()
                .as_ref()
        );

        remove_path_without_following_symlinks(&sandbox).expect("clean reset fixture");
        std::fs::write(
            std::env::var_os(CHILD_WITNESS).expect("child witness path"),
            WITNESS_BYTES,
        )
        .expect("write reset-scope child witness");
    }

    /// If a later root fails after an earlier one moved, recovery evidence must
    /// name the completed move and the failed pair, and the FFI error must force
    /// Swift to relaunch the now-latched process.
    #[test]
    #[serial]
    fn reset_partial_move_failure_reports_progress_and_requires_relaunch() {
        const CHILD_FLAG: &str = "CODESCRIBE_TEST_RESET_PARTIAL_CHILD";
        const CHILD_WITNESS: &str = "CODESCRIBE_TEST_RESET_PARTIAL_WITNESS";
        if std::env::var_os(CHILD_FLAG).is_none() {
            let witness_dir = tempfile::TempDir::new().expect("partial reset witness dir");
            let witness = witness_dir.path().join("passed");
            let status = std::process::Command::new(
                std::env::current_exe().expect("current bridge test executable"),
            )
            .args([
                "--exact",
                "config::config_tests::reset_tests::reset_partial_move_failure_reports_progress_and_requires_relaunch",
                "--nocapture",
            ])
            .env(CHILD_FLAG, "1")
            .env(CHILD_WITNESS, &witness)
            .status()
            .expect("spawn isolated partial-reset regression");
            assert!(status.success(), "isolated partial-reset regression failed");
            assert_eq!(
                std::fs::read(witness).expect("child executed exact partial-reset test"),
                b"partial-reset-pass"
            );
            return;
        }

        let sandbox = scratch("partial_move");
        let first = sandbox.join("first");
        let second = sandbox.join("second");
        let trash = sandbox.join("trash");
        let destination = trash.join("reset");
        write(&first.join("one.txt"), b"one");
        write(&second.join("two.txt"), b"two");
        std::fs::create_dir_all(&destination).expect("create reset destination");
        // Force only the second destination to collide. The first root must
        // move successfully before the structured failure is returned.
        std::fs::create_dir(destination.join("application-support-1"))
            .expect("block second destination");
        write(
            &destination.join("application-support-1/blocker"),
            b"occupied",
        );
        let mut reset = begin_app_data_reset().expect("own partial reset fence");
        let failure = move_reset_dirs_to_destination(
            &mut reset,
            &[first.clone(), second.clone()],
            &destination,
            &trash,
        )
        .expect_err("second root collision must fail after first move");

        assert!(reset.relaunch_required());
        assert_eq!(failure.moved_paths.len(), 1);
        assert_eq!(failure.moved_paths[0].0, first);
        assert_eq!(failure.failed_source, second);
        assert!(!first.exists());
        assert!(destination.join("codescribe-data/one.txt").is_file());
        assert!(second.join("two.txt").is_file());
        let ffi_error = super::reset_error(&reset, failure.to_string()).to_string();
        assert!(ffi_error.contains("CODESCRIBE_RESET_RELAUNCH_REQUIRED"));

        remove_path_without_following_symlinks(&sandbox).expect("clean partial-reset fixture");
        std::fs::write(
            std::env::var_os(CHILD_WITNESS).expect("partial reset witness path"),
            b"partial-reset-pass",
        )
        .expect("write partial-reset child witness");
    }

    /// Two entries for one reset must both survive in order. The log is the only
    /// evidence a reset started, so an implementation that truncated or
    /// overwrote would erase the `started` line whenever the reset completed.
    #[test]
    fn reset_audit_log_is_append_only_and_keeps_each_entry() {
        let sandbox = scratch("audit");
        let audit = sandbox.join("reset-audit.log");
        let timestamp = fixed_timestamp();
        let preview = CsResetPreview {
            audio_files: 7,
            transcript_days: 3,
            threads: 4,
            total_bytes: 42,
        };
        let sources = vec![sandbox.join("source")];
        let destination = sandbox.join("trash/reset");

        append_reset_audit(&ResetAuditEvent {
            audit_path: &audit,
            timestamp: &timestamp,
            status: "started",
            source_paths: &sources,
            moved_paths: &[],
            trash_path: &destination,
            preview: &preview,
            include_keys: true,
            include_prompts: false,
            preserved_prompt_files: 2,
            error: None,
            failed_source: None,
            failed_destination: None,
        })
        .expect("append first audit line");
        let moved_paths = vec![(sources[0].clone(), destination.join("codescribe-data"))];
        append_reset_audit(&ResetAuditEvent {
            audit_path: &audit,
            timestamp: &timestamp,
            status: "completed",
            source_paths: &sources,
            moved_paths: &moved_paths,
            trash_path: &destination,
            preview: &preview,
            include_keys: true,
            include_prompts: false,
            preserved_prompt_files: 2,
            error: None,
            failed_source: None,
            failed_destination: None,
        })
        .expect("append second audit line");

        let content = std::fs::read_to_string(&audit).expect("read append-only audit");
        assert_eq!(content.lines().count(), 2);
        assert!(
            content
                .lines()
                .all(|line| line.contains("\"include_keys\":true"))
        );
        assert!(
            content
                .lines()
                .next()
                .is_some_and(|line| line.contains("\"status\":\"started\""))
        );
        assert!(
            content
                .lines()
                .nth(1)
                .is_some_and(|line| line.contains("\"status\":\"completed\""))
        );
        remove_path_without_following_symlinks(&sandbox).expect("clean audit fixture");
    }

    /// With rename forced to fail (the cross-volume case), the tree must arrive
    /// complete at the destination *and* the source must be gone — proving the
    /// fallback is a real move, not a copy that leaves the data behind or a
    /// removal that runs before the copy is safe.
    #[test]
    fn reset_cross_volume_fallback_copies_before_source_removal() {
        let sandbox = scratch("fallback");
        let source = sandbox.join("source");
        let destination = sandbox.join("destination");
        write(&source.join("nested/audio.m4a"), b"recoverable-audio");

        move_path_recoverably_with(&source, &destination, |_source, _destination| {
            Err(std::io::Error::other("forced cross-volume rename failure"))
        })
        .expect("complete copy-and-remove fallback after rename failure");
        assert!(!source.exists());
        assert_eq!(
            std::fs::read(destination.join("nested/audio.m4a")).expect("read copied audio"),
            b"recoverable-audio"
        );
        assert!(destination.join("nested/audio.m4a").is_file());
        remove_path_without_following_symlinks(&sandbox).expect("clean fallback fixture");
    }

    /// "Clear MCP configuration" is a scalpel, not the reset: `mcp.json` moves
    /// to Trash intact while a sibling `settings.json` is left byte-identical.
    #[test]
    fn reset_clear_mcp_configuration_moves_only_mcp_json() {
        let sandbox = scratch("mcp");
        let data = sandbox.join("data");
        let trash = sandbox.join("trash");
        let mcp = data.join("mcp.json");
        let settings = data.join("settings.json");
        write(&mcp, br#"{"mcpServers":{"loctree":{}}}"#);
        write(&settings, br#"{"speech":{"language":"pl"}}"#);

        let destination = clear_mcp_configuration_to(&mcp, &trash)
            .expect("move MCP config to Trash")
            .expect("existing MCP config destination");

        assert!(!mcp.exists());
        assert_eq!(
            std::fs::read(&settings).expect("settings must survive MCP clear"),
            br#"{"speech":{"language":"pl"}}"#
        );
        assert_eq!(
            std::fs::read(destination).expect("read trashed MCP config"),
            br#"{"mcpServers":{"loctree":{}}}"#
        );
        remove_path_without_following_symlinks(&sandbox).expect("clean MCP fixture");
    }
}

/// Covers the settings read/write round trip through the real bridge object:
/// which store a UI write lands in (promoted settings.json vs `.env`), and that
/// a fresh handle reconstructs the same truth from disk with no in-memory
/// carryover. Every test here mutates process env, so all are `#[serial]`.
#[cfg(test)]
mod settings_snapshot_tests {
    use super::{
        CodescribeConfig, CsConfigEntry, CsError, CsPasteMode, CsSettings, SttLane,
        ensure_known_account, keychain, remove_path_without_following_symlinks, setting_string,
    };
    use codescribe_core::config::{Config, UserSettings};
    use serial_test::serial;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    /// Unique scratch data dir under the OS temp dir. Keyed by pid and
    /// nanosecond so a `#[serial]` test never inherits a previous run's
    /// settings.json.
    fn scratch(tag: &str) -> std::path::PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        std::env::temp_dir().join(format!(
            "cs_settings_snapshot_{}_{tag}_{nanos}",
            std::process::id()
        ))
    }

    fn custom_draft(name: &str, endpoint: &str) -> super::CsCustomProviderDraft {
        super::CsCustomProviderDraft {
            name: name.into(),
            wire: "responses".into(),
            endpoint: endpoint.into(),
            api_key: None,
        }
    }

    #[test]
    #[serial]
    fn agent_send_hold_bridge_round_trip_preserves_zero_and_env_precedence() {
        let root = tempfile::tempdir().unwrap();
        let _data = EnvGuard::set("CODESCRIBE_DATA_DIR", root.path());
        let _env = EnvGuard::set("CODESCRIBE_ENV_PATH", root.path().join("absent.env"));
        let _hold = EnvGuard::remove("CODESCRIBE_AGENT_SEND_HOLD_SEC");
        let config = CodescribeConfig::new();
        assert_eq!(config.load_settings().agent_send_hold_sec, 10);
        for seconds in [0, 7, 30] {
            config
                .update_config("CODESCRIBE_AGENT_SEND_HOLD_SEC".into(), seconds.to_string())
                .unwrap();
            assert_eq!(
                CodescribeConfig::new().load_settings().agent_send_hold_sec,
                seconds
            );
            let mut settings = UserSettings::load();
            settings.show_dock_icon = Some(true);
            settings.save().unwrap();
            assert_eq!(config.load_settings().agent_send_hold_sec, seconds);
        }
        assert!(!Config::env_path().exists());
        let _override = EnvGuard::set("CODESCRIBE_AGENT_SEND_HOLD_SEC", "0");
        assert_eq!(config.load_settings().agent_send_hold_sec, 0);
        assert_eq!(UserSettings::load().agent_send_hold_sec, Some(30));
    }

    #[test]
    #[serial]
    fn explicit_key_actions_cancel_pending_imports_and_stale_settings_cannot_reissue_them() {
        for replacement in [Some("synthetic-new-key"), None] {
            let root = tempfile::tempdir().unwrap();
            let _data = EnvGuard::set("CODESCRIBE_DATA_DIR", root.path());
            let source = root.path().join("selected.env");
            let _env = EnvGuard::set("CODESCRIBE_ENV_PATH", &source);
            let _key = EnvGuard::remove("STT_FILE_API_KEY");
            fs::write(&source, "STT_FILE_API_KEY=synthetic-old-import\n").unwrap();
            let values = Config::parse_env_file(&source).unwrap();
            codescribe_core::config::migrate::migrate_if_needed(Some(&values), false);
            let mut stale = UserSettings::load();
            assert_eq!(stale.pending_env_key_imports.len(), 1);
            let config = CodescribeConfig::new();
            match replacement {
                Some(secret) => config
                    .set_api_key("STT_FILE_API_KEY".into(), secret.into())
                    .unwrap(),
                None => config.clear_api_key("STT_FILE_API_KEY".into()).unwrap(),
            }
            stale.show_dock_icon = Some(true);
            stale.save().unwrap();
            assert!(UserSettings::load().pending_env_key_imports.is_empty());
            codescribe_core::config::migrate::migrate_if_needed(None, true);
            assert_eq!(
                keychain::cached_runtime_key("STT_FILE_API_KEY").as_deref(),
                replacement
            );
        }
    }

    #[test]
    #[serial]
    fn removing_pending_custom_provider_then_reusing_id_does_not_restore_its_old_key() {
        let root = tempfile::tempdir().unwrap();
        let _data = EnvGuard::set("CODESCRIBE_DATA_DIR", root.path());
        let source = root.path().join("selected.env");
        let _env = EnvGuard::set("CODESCRIBE_ENV_PATH", &source);
        fs::write(&source, "LLM_FORMATTING_ENDPOINT=https://synthetic.example/v1/responses\nLLM_FORMATTING_API_KEY=synthetic-old-provider-key\n").unwrap();
        let values = Config::parse_env_file(&source).unwrap();
        codescribe_core::config::migrate::migrate_if_needed(Some(&values), false);
        let before = UserSettings::load();
        let provider = before.llm_custom_providers.first().unwrap();
        let account = provider.key_account();
        assert!(
            before
                .pending_env_key_imports
                .iter()
                .any(|row| row.target == account)
        );
        let config = CodescribeConfig::new();
        config.remove_custom_provider(provider.id.clone()).unwrap();
        assert!(UserSettings::load().pending_env_key_imports.is_empty());
        let added = config
            .add_custom_provider(custom_draft(&provider.name, &provider.endpoint))
            .unwrap();
        assert_eq!(added.api_key_account, account);
        codescribe_core::config::migrate::migrate_if_needed(None, true);
        assert!(keychain::cached_runtime_key(&account).is_none());
    }

    #[test]
    #[serial]
    fn diagnostic_settings_refuse_unsupported_schema_without_exposing_source() {
        let root = tempfile::tempdir().unwrap();
        let _data = EnvGuard::set("CODESCRIBE_DATA_DIR", root.path());
        let _env = EnvGuard::set("CODESCRIBE_ENV_PATH", root.path().join("absent.env"));
        let source = br#"{"schema_version":999,"private_value":"do-not-copy"}"#;
        let path = UserSettings::settings_path();
        fs::write(&path, source).unwrap();
        let result = CodescribeConfig {}.load_diagnostic_settings();
        assert!(matches!(result, Err(CsError::Config { ref msg })
            if msg == "Configuration refusal recorded in this process"));
        assert_eq!(fs::read(path).unwrap(), source);
    }

    /// Swift reads the high-churn folder from the core resolver, never from a
    /// string it builds itself.
    #[test]
    #[serial]
    fn churn_dir_export_is_the_core_resolver_under_the_data_override() {
        let root = tempfile::tempdir().unwrap();
        let _data = EnvGuard::set("CODESCRIBE_DATA_DIR", root.path());
        let exported = CodescribeConfig {}.churn_dir();
        assert_eq!(exported, Config::churn_dir().to_string_lossy());
        assert_eq!(
            std::path::PathBuf::from(exported),
            root.path().canonicalize().unwrap().join("data.noindex")
        );
    }

    #[test]
    #[serial]
    fn diagnostic_settings_project_valid_saved_configuration() {
        let root = tempfile::tempdir().unwrap();
        let _data = EnvGuard::set("CODESCRIBE_DATA_DIR", root.path());
        let _env = EnvGuard::set("CODESCRIBE_ENV_PATH", root.path().join("absent.env"));
        UserSettings {
            audio_input_device: Some("Diagnostic fixture mic".into()),
            ..Default::default()
        }
        .save()
        .unwrap();
        let settings = CodescribeConfig {}.load_diagnostic_settings().unwrap();
        assert_eq!(
            settings.audio_input_device.as_deref(),
            Some("Diagnostic fixture mic")
        );
    }

    #[test]
    #[serial]
    fn remove_custom_provider_resets_lanes_and_deletes_key() {
        let root = tempfile::tempdir().unwrap();
        let _data = EnvGuard::set("CODESCRIBE_DATA_DIR", root.path());
        let config = CodescribeConfig::new();
        let mut draft = custom_draft("Remove witness", "http://localhost:8080");
        draft.api_key = Some("fixture-key".into());
        let row = config.add_custom_provider(draft).unwrap();
        assert!(row.api_key_set);
        for lane in [super::CsLlmLane::Formatting, super::CsLlmLane::Assistive] {
            config.set_lane_provider(lane, row.id.clone()).unwrap();
        }
        let removed = config
            .remove_custom_provider("remove-witness".into())
            .unwrap();
        assert_eq!(removed.id, "remove-witness");
        assert_eq!(
            removed.lanes_reset,
            [super::CsLlmLane::Formatting, super::CsLlmLane::Assistive]
        );
        assert!(super::keychain::cached_runtime_key(&row.api_key_account).is_none());
        let settings = UserSettings::load();
        assert!(settings.llm_custom_providers.is_empty());
        assert!(settings.llm_formatting_provider.is_none());
        assert!(settings.llm_assistive_provider.is_none());
        assert!(!config.available_providers().iter().any(|p| p.id == row.id));
    }

    #[test]
    #[serial]
    fn discover_models_accepts_custom_provider() {
        use std::io::{Read, Write};
        let root = tempfile::tempdir().unwrap();
        let _data = EnvGuard::set("CODESCRIBE_DATA_DIR", root.path());
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let config = CodescribeConfig::new();
        let row = config
            .add_custom_provider(custom_draft(
                "Discovery witness",
                &format!("http://{}", listener.local_addr().unwrap()),
            ))
            .unwrap();
        let server = std::thread::spawn(move || {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
            let (mut stream, _) = loop {
                match listener.accept() {
                    Ok(pair) => break pair,
                    Err(error)
                        if error.kind() == std::io::ErrorKind::WouldBlock
                            && std::time::Instant::now() < deadline =>
                    {
                        std::thread::sleep(std::time::Duration::from_millis(10))
                    }
                    Err(error) => panic!("discovery request did not arrive: {error}"),
                }
            };
            // An accepted socket inherits O_NONBLOCK from the listener on macOS,
            // and set_read_timeout has no effect on a nonblocking socket: a read
            // ahead of the client's request returns WouldBlock instead of waiting.
            stream.set_nonblocking(false).unwrap();
            stream
                .set_read_timeout(Some(std::time::Duration::from_secs(5)))
                .unwrap();
            let mut request = [0; 4096];
            let len = stream.read(&mut request).unwrap();
            let request = String::from_utf8_lossy(&request[..len]).to_lowercase();
            assert!(request.starts_with("get /v1/models "));
            assert!(!request.contains("authorization:"));
            let body = r#"{"data":[{"id":"fixture-model"}]}"#;
            write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
        });
        let result = config.discover_models(row.id.clone());
        server.join().unwrap();
        assert_eq!(result.provider_id, row.id);
        assert_eq!(result.status, "fresh", "{:?}", result.message);
        assert_eq!(result.models[0].id, "fixture-model");
    }

    #[test]
    #[serial]
    fn update_custom_provider_preserves_slug_and_key() {
        let root = tempfile::tempdir().unwrap();
        let _data = EnvGuard::set("CODESCRIBE_DATA_DIR", root.path());
        let config = CodescribeConfig::new();
        let mut draft = custom_draft("Update witness", "http://localhost:8080");
        draft.api_key = Some("fixture-key".into());
        let original = config.add_custom_provider(draft).unwrap();
        let mut draft = custom_draft("Renamed", "http://localhost:9090");
        draft.wire = "messages".into();
        let updated = config
            .update_custom_provider("update-witness".into(), draft)
            .unwrap();
        assert_eq!(updated.id, original.id);
        assert_eq!(updated.api_key_account, original.api_key_account);
        assert!(updated.api_key_set);
        assert_eq!(updated.endpoint, "http://localhost:9090/v1/messages");
        assert_eq!(updated.display_name, "Renamed");
        config
            .remove_custom_provider("update-witness".into())
            .unwrap();
    }

    #[test]
    #[serial]
    fn provider_catalog_validates_lanes_and_exposes_service_keys() {
        let root = tempfile::tempdir().unwrap();
        let _data = EnvGuard::set("CODESCRIBE_DATA_DIR", root.path());
        let config = CodescribeConfig::new();
        assert_eq!(config.service_key_accounts(), ["GITHUB_TOKEN"]);
        let providers = config.available_providers();
        assert_eq!(providers.len(), 5);
        let google = providers
            .iter()
            .find(|provider| provider.id == "google-gemini")
            .expect("Google is discoverable through the native provider catalog");
        assert_eq!(google.wire, "google-generate-content");
        assert_eq!(google.api_key_account, "LLM_GOOGLE_API_KEY");
        assert!(
            config
                .available_providers()
                .iter()
                .all(|p| p.kind == "vendor" && p.key_required)
        );
        assert!(
            config
                .set_lane_provider(super::CsLlmLane::Formatting, "custom:missing".into())
                .is_err()
        );
        config
            .set_lane_provider(super::CsLlmLane::Formatting, "xai-responses".into())
            .unwrap();
        assert_eq!(
            UserSettings::load().llm_formatting_provider.as_deref(),
            Some("xai-responses")
        );
        assert!(
            config
                .update_config("LLM_ENDPOINT".into(), "http://localhost".into())
                .is_err()
        );
        assert_eq!(
            config.discover_models("custom:missing".into()).status,
            "error"
        );
    }

    #[test]
    #[serial]
    fn stt_lanes_are_two_atomic_rows_in_order() {
        let root = tempfile::tempdir().unwrap();
        let _data = EnvGuard::set("CODESCRIBE_DATA_DIR", root.path());
        let config = CodescribeConfig::new();
        let rows = config.stt_lanes();
        assert_eq!(
            rows.iter().map(|row| row.id.as_str()).collect::<Vec<_>>(),
            ["file", "live"]
        );
        for (row, lane) in rows.iter().zip(SttLane::ALL) {
            assert_eq!(row.id, lane.id());
            assert_eq!(row.endpoint_wire_key, lane.wire_key());
            assert_eq!(row.key_account, lane.key_account());
            assert_eq!(row.api_key_set, keychain::key_present(lane.key_account()));
            ensure_known_account(&row.key_account).unwrap();
        }
    }

    #[test]
    #[serial]
    fn update_config_rejects_wss_on_file_lane() {
        let root = tempfile::tempdir().unwrap();
        let _data = EnvGuard::set("CODESCRIBE_DATA_DIR", root.path());
        let config = CodescribeConfig::new();
        for (lane, good, bad) in [
            (
                SttLane::File,
                "https://api.libraxis.cloud/v1/audio/transcriptions",
                "wss://api.libraxis.cloud/v1/audio/transcribe",
            ),
            (
                SttLane::Live,
                "wss://api.libraxis.cloud/v1/audio/transcribe",
                "https://api.libraxis.cloud/v1/audio/transcriptions",
            ),
        ] {
            config
                .update_config(lane.wire_key().into(), good.into())
                .unwrap();
            let before = fs::read(UserSettings::settings_path()).unwrap();
            assert!(matches!(
                config.update_config(lane.wire_key().into(), bad.into()),
                Err(CsError::Config { .. })
            ));
            assert!(matches!(
                config.update_config_many(vec![CsConfigEntry {
                    key: lane.wire_key().into(),
                    value: bad.into()
                }]),
                Err(CsError::Config { .. })
            ));
            assert_eq!(fs::read(UserSettings::settings_path()).unwrap(), before);
            assert_eq!(
                config
                    .stt_lanes()
                    .into_iter()
                    .find(|row| row.id == lane.id())
                    .unwrap()
                    .endpoint
                    .as_deref(),
                Some(good)
            );
            config
                .update_config(lane.wire_key().into(), "".into())
                .unwrap();
            assert!(
                config
                    .stt_lanes()
                    .into_iter()
                    .find(|row| row.id == lane.id())
                    .unwrap()
                    .endpoint
                    .is_none()
            );
        }
    }

    /// The assistive provider is a promoted key: a Settings write must land in
    /// settings.json (which survives a reinstall) and must NOT be mirrored into
    /// the `.env` power-user store, or the two would drift apart.
    #[test]
    #[serial]
    fn assistive_provider_ui_write_is_promoted_to_settings_json() {
        let root = scratch("assistive_provider");
        std::fs::create_dir_all(&root).unwrap();

        let _data_dir = EnvGuard::set("CODESCRIBE_DATA_DIR", &root);
        let _env_path = EnvGuard::remove("CODESCRIBE_ENV_PATH");
        let _provider = EnvGuard::remove("LLM_ASSISTIVE_PROVIDER");

        let config = CodescribeConfig::new();
        config
            .update_config(
                "LLM_ASSISTIVE_PROVIDER".to_string(),
                "anthropic-messages".to_string(),
            )
            .expect("persist provider through the UI bridge");

        let persisted: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(UserSettings::settings_path())
                .expect("read promoted settings.json"),
        )
        .expect("parse promoted settings.json");
        assert_eq!(
            persisted
                .get("speech")
                .and_then(|value| value.get("assistive"))
                .and_then(|value| value.get("provider"))
                .and_then(serde_json::Value::as_str),
            Some("anthropic-messages")
        );

        let env_path = Config::env_path();
        if env_path.exists() {
            let env = Config::parse_env_file(&env_path).expect("parse optional .env");
            assert!(
                !env.contains_key("LLM_ASSISTIVE_PROVIDER"),
                "promoted provider must not be written to .env"
            );
        }
        assert_eq!(
            config.load_settings().llm_assistive_provider.as_deref(),
            Some("anthropic-messages")
        );

        let _ = remove_path_without_following_symlinks(&root);
    }

    /// The tray must report persisted truth, never an optimistic echo. A normal
    /// two-step write/read proves the returned snapshot is disk-derived; atomic
    /// write failure itself is injected at the core rename seam.
    #[test]
    #[serial]
    fn tray_toggles_roundtrip_paste_mode_and_format_truth() {
        let root = scratch("tray_delivery_truth");
        std::fs::create_dir_all(&root).expect("create bridge scratch");
        let _data_dir = EnvGuard::set("CODESCRIBE_DATA_DIR", &root);
        let _env_path = EnvGuard::remove("CODESCRIBE_ENV_PATH");
        let _paste_mode = EnvGuard::remove("PASTE_MODE");
        let _formatting = EnvGuard::remove("FORMATTING_LEVEL");
        let config = CodescribeConfig::new();
        assert_eq!(config.tray_toggles().paste_mode, CsPasteMode::Safe);

        let comfort = config
            .set_paste_mode(CsPasteMode::Comfort)
            .expect("persist comfort");
        assert_eq!(comfort.paste_mode, CsPasteMode::Comfort);
        assert!(comfort.auto_paste_enabled);
        assert_eq!(comfort.formatting_level, "correction");
        assert_eq!(config.load_settings().paste_mode, CsPasteMode::Comfort);

        // A redundant "on" from a binary surface keeps the armed mode.
        let still_comfort = config
            .set_auto_paste_enabled(true)
            .expect("redundant on keeps comfort");
        assert_eq!(still_comfort.paste_mode, CsPasteMode::Comfort);

        let after_paste = config
            .set_auto_paste_enabled(false)
            .expect("persist paste off");
        assert_eq!(after_paste.paste_mode, CsPasteMode::Off);
        assert!(!after_paste.auto_paste_enabled);

        let after_format = config
            .set_auto_format_level("smart".to_string())
            .expect("persist normalized format level");
        assert_eq!(after_format.paste_mode, CsPasteMode::Off);
        assert_eq!(after_format.formatting_level, "smart");

        let delivered = config
            .set_auto_paste_enabled(true)
            .expect("re-arm from off");
        assert_eq!(delivered.paste_mode, CsPasteMode::Safe);
        let reread = config.tray_toggles();
        assert_eq!(reread.paste_mode, CsPasteMode::Safe);
        assert!(reread.auto_paste_enabled);
        assert_eq!(reread.formatting_level, "smart");

        let env_path = Config::env_path();
        if env_path.exists() {
            let env = Config::parse_env_file(&env_path).expect("parse optional env");
            assert!(!env.contains_key("PASTE_MODE"));
            assert!(!env.contains_key("FORMATTING_LEVEL"));
        }
        let _ = remove_path_without_following_symlinks(&root);
    }

    #[test]
    #[serial]
    fn notes_mode_roundtrip_preserves_each_paste_mode() {
        let root = scratch("notes_paste_independence");
        std::fs::create_dir_all(&root).unwrap();
        let _data_dir = EnvGuard::set("CODESCRIBE_DATA_DIR", &root);
        let _env_path = EnvGuard::remove("CODESCRIBE_ENV_PATH");
        let _paste_mode = EnvGuard::remove("PASTE_MODE");
        let _notes = EnvGuard::remove("QUICK_NOTES_ENABLED");
        let config = CodescribeConfig::new();
        for mode in [CsPasteMode::Safe, CsPasteMode::Comfort, CsPasteMode::Off] {
            config.set_paste_mode(mode).unwrap();
            for enabled in [true, false] {
                config.set_notes_mode(enabled).unwrap();
                let toggles = config.tray_toggles();
                assert_eq!(toggles.notes_mode_enabled, enabled);
                assert_eq!(toggles.paste_mode, mode);
                assert_eq!(config.load_settings().paste_mode, mode);
            }
        }
        let _ = remove_path_without_following_symlinks(&root);
    }

    #[test]
    #[serial]
    fn overlay_take_start_defaults_on_and_survives_new_handle() {
        let root = tempfile::tempdir().unwrap();
        let _data_dir = EnvGuard::set("CODESCRIBE_DATA_DIR", root.path());
        let _env_path = EnvGuard::set("CODESCRIBE_ENV_PATH", root.path().join("absent.env"));
        let config = CodescribeConfig::new();
        assert!(config.overlay_expanded_by_default());
        assert!(config.set_overlay_expanded_by_default(false));
        assert!(!CodescribeConfig::new().overlay_expanded_by_default());
        assert!(config.set_overlay_expanded_by_default(true));
        assert!(CodescribeConfig::new().overlay_expanded_by_default());
        let persisted: serde_json::Value =
            serde_json::from_slice(&fs::read(UserSettings::settings_path()).unwrap()).unwrap();
        assert_eq!(persisted["ui"]["show_transcript_at_take_start"], true);
        assert!(persisted["ui"].get("overlay_expanded_by_default").is_none());
    }

    #[test]
    #[serial]
    fn overlay_take_start_ignores_old_false_and_only_writes_new_key() {
        let root = tempfile::tempdir().unwrap();
        let _data_dir = EnvGuard::set("CODESCRIBE_DATA_DIR", root.path());
        let _env_path = EnvGuard::set("CODESCRIBE_ENV_PATH", root.path().join("absent.env"));
        UserSettings {
            overlay_expanded_by_default: Some(false),
            ..UserSettings::default()
        }
        .save()
        .unwrap();
        let config = CodescribeConfig::new();
        assert!(config.overlay_expanded_by_default());
        for enabled in [false, true] {
            assert!(config.set_overlay_expanded_by_default(enabled));
            assert_eq!(
                CodescribeConfig::new().overlay_expanded_by_default(),
                enabled
            );
            let saved = UserSettings::load();
            assert_eq!(saved.overlay_expanded_by_default, Some(false));
            assert_eq!(saved.show_transcript_at_take_start, Some(enabled));
        }
    }

    #[test]
    #[serial]
    fn overlay_take_start_defaults_on_after_settings_load_failure() {
        let root = tempfile::tempdir().unwrap();
        let _data_dir = EnvGuard::set("CODESCRIBE_DATA_DIR", root.path());
        let _env_path = EnvGuard::set("CODESCRIBE_ENV_PATH", root.path().join("absent.env"));
        let path = UserSettings::settings_path();
        // A directory cannot be read as settings JSON or repaired as a file.
        fs::create_dir(&path).unwrap();
        assert!(fs::read_to_string(&path).is_err());
        let expanded =
            std::panic::catch_unwind(|| CodescribeConfig::new().overlay_expanded_by_default());
        assert_eq!(expanded.ok(), Some(true));
        assert!(
            path.is_dir(),
            "The getter must not replace an unreadable source"
        );
    }

    #[test]
    #[serial]
    fn overlay_pin_defaults_off_and_survives_new_handle() {
        let root = scratch("overlay_pin_truth");
        std::fs::create_dir_all(&root).expect("create bridge scratch");
        let _data_dir = EnvGuard::set("CODESCRIBE_DATA_DIR", &root);
        let _env_path = EnvGuard::remove("CODESCRIBE_ENV_PATH");
        let config = CodescribeConfig::new();
        assert!(!config.overlay_keep_visible_between_takes());
        assert!(config.set_overlay_keep_visible_between_takes(true));
        assert!(CodescribeConfig::new().overlay_keep_visible_between_takes());
        assert!(config.set_overlay_keep_visible_between_takes(true));
        assert!(CodescribeConfig::new().overlay_keep_visible_between_takes());
        let _ = remove_path_without_following_symlinks(&root);
    }

    #[test]
    #[serial]
    fn overlay_auto_fold_seconds_defaults_to_ten_refuses_seven_and_survives_new_handle() {
        let root = scratch("overlay_auto_fold");
        std::fs::create_dir_all(&root).expect("create bridge scratch");
        let _data_dir = EnvGuard::set("CODESCRIBE_DATA_DIR", &root);
        let _env_path = EnvGuard::remove("CODESCRIBE_ENV_PATH");
        let config = CodescribeConfig::new();
        assert_eq!(config.overlay_auto_fold_seconds(), 10);
        assert!(!config.set_overlay_auto_fold_seconds(7));
        assert_eq!(config.overlay_auto_fold_seconds(), 10);
        assert_eq!(UserSettings::load().overlay_auto_fold_seconds, None);
        assert!(config.set_overlay_auto_fold_seconds(30));
        assert_eq!(CodescribeConfig::new().overlay_auto_fold_seconds(), 30);
        let path = UserSettings::settings_path();
        let written = fs::read(&path).expect("read auto-fold");
        assert!(config.set_overlay_auto_fold_seconds(30));
        assert_eq!(fs::read(&path).expect("unchanged auto-fold"), written);
        assert!(config.set_overlay_auto_fold_seconds(0));
        assert_eq!(CodescribeConfig::new().overlay_auto_fold_seconds(), 0);
        UserSettings {
            overlay_auto_fold_seconds: Some(7),
            ..UserSettings::default()
        }
        .save()
        .unwrap();
        assert_eq!(CodescribeConfig::new().overlay_auto_fold_seconds(), 10);
        let _ = remove_path_without_following_symlinks(&root);
    }

    /// Clearing the input device must persist a real absence, not `Some("")`.
    /// An empty override would read as a configured device named "" and pin the
    /// recorder away from the live system default forever.
    #[test]
    #[serial]
    fn audio_input_reset_persists_absence_not_an_empty_override() {
        let root = scratch("audio_input_reset");
        std::fs::create_dir_all(&root).expect("create audio reset scratch dir");

        let _data_dir = EnvGuard::set("CODESCRIBE_DATA_DIR", &root);
        let _env_path = EnvGuard::remove("CODESCRIBE_ENV_PATH");
        // An explicit launch override stays in the process; resetting the
        // saved preference must not depend on it.
        let _device = EnvGuard::set("AUDIO_INPUT_DEVICE", "USB Studio Mic");

        let mut settings = UserSettings {
            audio_input_device: Some("USB Studio Mic".to_string()),
            ..Default::default()
        };
        settings.save().expect("seed audio device preference");

        CodescribeConfig::new()
            .reset_audio_input_device()
            .expect("reset audio device through the bridge");

        settings = UserSettings::load();
        assert_eq!(settings.audio_input_device, None);
        assert_eq!(
            CodescribeConfig::new().load_settings().audio_input_device,
            None
        );
        let persisted = std::fs::read_to_string(UserSettings::settings_path())
            .expect("read settings after audio reset");
        assert!(!persisted.contains("USB Studio Mic"));
        assert!(!persisted.contains("\"input_device_id\": \"\""));

        let _ = remove_path_without_following_symlinks(&root);
    }

    /// Promoted keys read settings.json before process env. Launch-time
    /// bootstrap can leave a stale `LLM_FORMATTING_MODEL` in the environment; if env won,
    /// a fresh UI write would appear to save and then silently revert.
    #[test]
    #[serial]
    fn load_settings_prefers_persisted_model_over_stale_process_env() {
        let root = scratch("formatting_model");
        std::fs::create_dir_all(&root).unwrap();

        let previous_data_dir = std::env::var("CODESCRIBE_DATA_DIR").ok();
        let previous_model = std::env::var("LLM_FORMATTING_MODEL").ok();
        // SAFETY: serialized test body; no background workers are started.
        unsafe {
            std::env::set_var("CODESCRIBE_DATA_DIR", &root);
            std::env::set_var("LLM_FORMATTING_MODEL", "stale-bootstrap-model");
        }

        let settings = UserSettings {
            llm_formatting_model: Some("fresh-runtime-model".to_string()),
            ..Default::default()
        };
        settings.save().unwrap();

        let snapshot = CodescribeConfig::new().load_settings();
        assert_eq!(
            snapshot.llm_formatting_model.as_deref(),
            Some("fresh-runtime-model")
        );

        // SAFETY: restore prior env, same serialized single-thread context.
        unsafe {
            match previous_data_dir {
                Some(value) => std::env::set_var("CODESCRIBE_DATA_DIR", value),
                None => std::env::remove_var("CODESCRIBE_DATA_DIR"),
            }
            match previous_model {
                Some(value) => std::env::set_var("LLM_FORMATTING_MODEL", value),
                None => std::env::remove_var("LLM_FORMATTING_MODEL"),
            }
        }
        let _ = remove_path_without_following_symlinks(&root);
    }

    /// The picker receives its selected chord through `CsSettings`, not a
    /// second Swift-only store. A fresh bridge must therefore reconstruct the
    /// promoted settings.json choice and expose its canonical wire id.
    #[test]
    #[serial]
    fn load_settings_exports_persisted_deferred_insert_shortcut() {
        let root = scratch("deferred_insert_shortcut");
        std::fs::create_dir_all(&root).expect("create deferred-insert scratch");
        let _data_dir = EnvGuard::set("CODESCRIBE_DATA_DIR", &root);
        let _env_path = EnvGuard::remove("CODESCRIBE_ENV_PATH");
        let _process_shortcut = EnvGuard::remove("CODESCRIBE_DEFERRED_INSERT_SHORTCUT");

        CodescribeConfig::new()
            .update_config(
                "CODESCRIBE_DEFERRED_INSERT_SHORTCUT".to_string(),
                "command_shift_v".to_string(),
            )
            .expect("persist deferred-insert chord through the bridge");

        let fresh = CodescribeConfig::new().load_settings();
        assert_eq!(fresh.deferred_insert_shortcut, "command_shift_v");
        assert_eq!(
            UserSettings::load().deferred_insert_shortcut.as_deref(),
            Some("command_shift_v"),
            "the promoted picker choice belongs in settings.json"
        );

        let _ = remove_path_without_following_symlinks(&root);
    }

    /// Workspace roots must round-trip through durable settings.json and be
    /// rebuilt identically by a brand-new bridge handle. The failure this pins:
    /// a fresh instance falling back to the `["~/.codescribe"]` default and
    /// quietly discarding the operator's configured roots.
    #[test]
    #[serial]
    fn workspace_roots_persist_across_fresh_config_instances() {
        let root = scratch("workspace_roots");
        std::fs::create_dir_all(&root).expect("create workspace-root scratch");
        let _data_dir = EnvGuard::set("CODESCRIBE_DATA_DIR", &root);
        let _env_path = EnvGuard::remove("CODESCRIBE_ENV_PATH");
        let _process_roots = EnvGuard::remove("AGENT_WORKSPACE_ROOTS");
        let expected = vec![
            "~/Git".to_string(),
            "/Users/op/workspace/projects".to_string(),
            "/Users/op/workspace/tools".to_string(),
            "/Users/op/workspace/libs".to_string(),
        ];

        CodescribeConfig::new()
            .update_config("AGENT_WORKSPACE_ROOTS".to_string(), expected.join(":"))
            .expect("persist workspace roots through bridge");

        assert_eq!(
            UserSettings::load().agent_workspace_roots,
            Some(expected.clone()),
            "the UI write must land in durable settings.json"
        );
        let env_path = Config::env_path();
        if env_path.exists() {
            let dotenv = Config::parse_env_file(&env_path).expect("parse optional env file");
            assert!(
                !dotenv.contains_key("AGENT_WORKSPACE_ROOTS"),
                "promoted roots must not be rewritten to the reinstall-fragile .env store"
            );
        }

        // A new bridge object has no in-memory carryover; it must reconstruct
        // the exact list from settings.json, never the built-in default.
        let fresh = CodescribeConfig::new().load_settings();
        assert_eq!(fresh.agent_workspace_roots, expected);
        let second_fresh = CodescribeConfig::new().load_settings();
        assert_eq!(
            second_fresh.agent_workspace_roots,
            fresh.agent_workspace_roots
        );

        let _ = remove_path_without_following_symlinks(&root);
    }

    use codescribe_core::test_isolation::EnvGuard;

    #[test]
    #[serial_test::serial]
    fn engine_controls_project_only_the_captured_layered_override() {
        use codescribe_core::config::CapturedRuntimeInputs;

        let _ambient = EnvGuard::set("CODESCRIBE_LAYERED_TRANSCRIPTION", "off");
        let mut input = CapturedRuntimeInputs::defaults_at(
            std::path::PathBuf::from("/fixture/engine-controls"),
            1_700_000_000_000,
        );
        input.user_settings.asr_mode = Some("local_power".into());
        let normal = Config::runtime_snapshot_from_captured(input.clone());
        let projected = CsSettings::from_runtime_snapshot(&normal);
        assert_eq!(projected.asr_mode.as_deref(), Some("local_power"));
        assert_eq!(projected.layered_transcription, None);

        input
            .overrides
            .insert("CODESCRIBE_LAYERED_TRANSCRIPTION".into(), Ok("off".into()));
        let degraded = Config::runtime_snapshot_from_captured(input);
        let _changed = EnvGuard::set("CODESCRIBE_LAYERED_TRANSCRIPTION", "phase1");
        assert_eq!(
            CsSettings::from_runtime_snapshot(&degraded)
                .layered_transcription
                .as_deref(),
            Some("off")
        );
        assert_eq!(
            CsSettings::from_runtime_snapshot(&normal).layered_transcription,
            None
        );
    }

    /// Settings shows the level the formatting lane will send: `none` until
    /// the user opts in, and never a word that is not a level.
    #[test]
    fn formatting_reasoning_projects_its_level_with_none_as_default() {
        use codescribe_core::config::CapturedRuntimeInputs;

        let mut input = CapturedRuntimeInputs::defaults_at(
            std::path::PathBuf::from("/fixture/formatting-reasoning"),
            1_700_000_000_000,
        );
        for (stored, shown) in [
            (None, "none"),
            (Some("high"), "high"),
            (Some("turbo"), "none"),
        ] {
            input.user_settings.llm_formatting_reasoning = stored.map(str::to_string);
            let runtime = Config::runtime_snapshot_from_captured(input.clone());
            assert_eq!(
                CsSettings::from_runtime_snapshot(&runtime).llm_formatting_reasoning,
                shown,
                "{stored:?}"
            );
            let lane_level = runtime
                .llm_lanes()
                .formatting()
                .reasoning()
                .map(|effort| effort.as_str());
            assert_eq!(lane_level, Some(shown), "{stored:?}");
        }
    }

    /// The Agent shows `default` until a level is chosen, `none` included.
    #[test]
    fn assistive_reasoning_projects_default_until_chosen() {
        use codescribe_core::config::CapturedRuntimeInputs;

        let mut input = CapturedRuntimeInputs::defaults_at(
            std::path::PathBuf::from("/fixture/assistive-reasoning"),
            1_700_000_000_000,
        );
        for (stored, shown, lane) in [
            (None, "default", None),
            (Some("none"), "none", Some("none")),
            (Some("xhigh"), "xhigh", Some("xhigh")),
        ] {
            input.user_settings.llm_assistive_reasoning = stored.map(str::to_string);
            let runtime = Config::runtime_snapshot_from_captured(input.clone());
            assert_eq!(
                CsSettings::from_runtime_snapshot(&runtime).llm_assistive_reasoning,
                shown,
                "{stored:?}"
            );
            let lane_level = runtime
                .llm_lanes()
                .assistive()
                .reasoning()
                .map(|effort| effort.as_str());
            assert_eq!(lane_level, lane, "{stored:?}");
        }
    }

    /// C15D falsifier (source-level; W2 does not execute): Settings UI projection
    /// comes from one sealed snapshot without an env-file re-merge twin.
    #[test]
    fn settings_ui_projects_from_one_runtime_snapshot_without_env_remerge() {
        let runtime = Config::load_runtime_snapshot_without_keychain()
            .expect("seal runtime settings for projection");
        let projected = CsSettings::from_runtime_snapshot(&runtime);
        assert_eq!(
            projected.formatting_level.as_deref(),
            Some(runtime.formatting_policy().as_str())
        );
        assert_eq!(
            projected.hold_start_delay_ms,
            runtime.values().hold_start_delay_ms
        );
        // Persisted intent from the same seal wins over ambient process env.
        assert_eq!(
            projected.llm_formatting_model,
            setting_string(runtime.user_settings().llm_formatting_model.clone())
                .or_else(|| Some(runtime.llm_lanes().formatting().model().to_string()))
        );
    }
}

/// Every lane-unavailable variant crosses the bridge with its data intact and
/// without a sentence: Swift owns the wording.
#[cfg(test)]
mod lane_unavailable_projection_tests {
    use super::{CsLlmLane, CsLlmLaneUnavailable};
    use codescribe_core::config::{RuntimeLlmLaneKind, RuntimeLlmLaneUnavailable};

    #[test]
    fn every_variant_projects_its_data() {
        let cases = [
            (
                RuntimeLlmLaneUnavailable::CustomProviderMissing {
                    custom_id: "gone".into(),
                },
                CsLlmLaneUnavailable::CustomProviderMissing {
                    custom_id: "gone".into(),
                },
            ),
            (
                RuntimeLlmLaneUnavailable::ModelMissing {
                    provider: "My Box".into(),
                },
                CsLlmLaneUnavailable::ModelMissing {
                    provider: "My Box".into(),
                },
            ),
            (
                RuntimeLlmLaneUnavailable::CredentialMissing {
                    lane: RuntimeLlmLaneKind::Formatting,
                    provider: "OpenAI (Responses)".into(),
                    endpoint: "https://api.openai.com/v1/responses".into(),
                    key_account: "LLM_OPENAI_API_KEY".into(),
                },
                CsLlmLaneUnavailable::CredentialMissing {
                    lane: CsLlmLane::Formatting,
                    provider: "OpenAI (Responses)".into(),
                    endpoint: "https://api.openai.com/v1/responses".into(),
                    key_account: "LLM_OPENAI_API_KEY".into(),
                },
            ),
            (
                RuntimeLlmLaneUnavailable::CloudCodeAssistAccountRequired {
                    lane: RuntimeLlmLaneKind::Assistive,
                },
                CsLlmLaneUnavailable::CloudCodeAssistAccountRequired {
                    lane: CsLlmLane::Assistive,
                },
            ),
            (
                RuntimeLlmLaneUnavailable::AcpLaunchInvalid,
                CsLlmLaneUnavailable::AcpLaunchInvalid,
            ),
            (
                RuntimeLlmLaneUnavailable::AcpWorkspaceRootsMissing,
                CsLlmLaneUnavailable::AcpWorkspaceRootsMissing,
            ),
            (
                RuntimeLlmLaneUnavailable::AcpCwdOutsideWorkspace,
                CsLlmLaneUnavailable::AcpCwdOutsideWorkspace,
            ),
        ];
        for (core, bridge) in cases {
            assert_eq!(CsLlmLaneUnavailable::from(&core), bridge);
        }
    }
}

/// The last-good runtime snapshot is keyed on `settings.json` mtime. Pasting a
/// key writes the Keychain bundle (test env: process env / bundle cache) and
/// never touches that file, so a cached lane row must not keep saying "no key".
#[cfg(test)]
mod runtime_snapshot_cache_tests {
    use super::{CodescribeConfig, CsLlmLane, KEYCHAIN_ACCOUNTS, runtime_llm_lane, save_key};
    use codescribe_core::config::UserSettings;
    use serial_test::serial;
    use std::ffi::OsString;
    use std::fs;

    /// Same isolation shape as `core/config/stt_migration.rs` tests: a private
    /// data dir, with Drop restoring the process env the test mutated.
    struct IsolatedSettings {
        _dir: tempfile::TempDir,
        _bundle: codescribe_core::config::keychain::test_support::BundleCacheGuard,
        previous: Vec<(&'static str, Option<OsString>)>,
    }

    impl IsolatedSettings {
        fn new() -> Self {
            let dir = tempfile::tempdir().unwrap();
            let mut keys = vec![
                "CODESCRIBE_DATA_DIR",
                "CODESCRIBE_DISABLE_KEYCHAIN",
                "CODESCRIBE_VOICE_LAB_SRC",
                "CODESCRIBE_ENV_PATH",
                "LLM_API_KEY",
                "LLM_FORMATTING_API_KEY",
                "LLM_ASSISTIVE_API_KEY",
                "LLM_FORMATTING_PROVIDER",
                "LLM_ASSISTIVE_PROVIDER",
            ];
            keys.extend(KEYCHAIN_ACCOUNTS.iter().copied());
            let previous = keys
                .into_iter()
                .map(|key| {
                    let previous = std::env::var_os(key);
                    // SAFETY: `#[serial]` tests; Drop restores every key.
                    unsafe {
                        if key == "CODESCRIBE_DATA_DIR" {
                            std::env::set_var(key, dir.path());
                        } else if key == "CODESCRIBE_ENV_PATH" {
                            // Pin the .env path to a nonexistent file inside the
                            // sandbox. Removing the key instead would let loads
                            // fall back to the real ~/.codescribe/.env, whose
                            // retired keys (e.g. AGENT_WORKSPACE_ROOTS) make any
                            // concurrent non-serial config load run migration
                            // writes against whatever settings.json
                            // CODESCRIBE_DATA_DIR currently names — ours
                            // (the 2026-09-30 double-build flake).
                            std::env::set_var(key, dir.path().join("isolated.env"));
                        } else if key == "CODESCRIBE_DISABLE_KEYCHAIN" {
                            // A private data directory does not disable Keychain.
                            // Dependency builds must opt out explicitly, including
                            // custom target directories outside /target/debug/deps.
                            std::env::set_var(key, "1");
                        } else {
                            std::env::remove_var(key);
                        }
                    }
                    (key, previous)
                })
                .collect();
            fs::create_dir_all(UserSettings::settings_dir()).unwrap();
            super::invalidate_runtime_snapshot_cache();
            Self {
                _dir: dir,
                _bundle: codescribe_core::config::keychain::test_support::install_bundle(&[]),
                previous,
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
                for (key, previous) in &self.previous {
                    match previous {
                        Some(value) => std::env::set_var(key, value),
                        None => std::env::remove_var(key),
                    }
                }
            }
        }
    }

    /// File/live STT rows already populated — a load must not rewrite the file
    /// (the 2026-09-08 save-storm shape).
    fn settled_settings_json() -> serde_json::Value {
        serde_json::json!({
            "schema_version": 3,
            "speech": {
                "engine": {
                    "file_transcription_endpoint": "https://api.libraxis.cloud/v1/audio/transcriptions",
                    "live_transcription_endpoint": "wss://api.libraxis.cloud/v1/audio/transcribe"
                },
                "formatting": {
                    "llm_provider": "openai-responses",
                    "llm_model": "gpt-4.1"
                }
            }
        })
    }

    #[test]
    #[serial]
    fn runtime_llm_lane_sees_a_key_saved_after_the_snapshot_was_cached() {
        let isolated = IsolatedSettings::new();
        let path = isolated.path();
        fs::write(&path, settled_settings_json().to_string()).unwrap();

        let _settle = runtime_llm_lane(CsLlmLane::Formatting);
        let mtime = fs::metadata(&path).unwrap().modified().unwrap();
        let before = runtime_llm_lane(CsLlmLane::Formatting);
        assert!(
            !before.key_present,
            "isolated formatting lane must start without a key"
        );
        assert_eq!(
            before.key_account, "LLM_OPENAI_API_KEY",
            "pinned openai-responses formatting lane"
        );
        assert_eq!(
            before.unavailable,
            Some(super::CsLlmLaneUnavailable::CredentialMissing {
                lane: CsLlmLane::Formatting,
                provider: before.provider_display_name.clone(),
                endpoint: before.endpoint.clone(),
                key_account: "LLM_OPENAI_API_KEY".to_string(),
            }),
            "a keyless lane crosses the bridge as a typed reason, not a sentence"
        );
        assert_eq!(mtime, fs::metadata(&path).unwrap().modified().unwrap());

        // Test-env `save_key` writes a static account into process env (custom
        // accounts go to the bundle cache). Settings.json must not move.
        save_key(&before.key_account, "fixture-not-a-real-key").unwrap();
        // Settings paste goes through `set_api_key` (same Keychain write) and
        // must drop the last-good snapshot so the next projection is live.
        CodescribeConfig::new()
            .set_api_key(before.key_account.clone(), "fixture-not-a-real-key".into())
            .unwrap();

        let after = runtime_llm_lane(CsLlmLane::Formatting);
        assert_eq!(
            mtime,
            fs::metadata(&path).unwrap().modified().unwrap(),
            "saving a key must not rewrite settings.json"
        );
        assert!(
            after.key_present,
            "cached snapshot must not hide a key saved after the cache filled"
        );
        assert_eq!(after.unavailable, None);
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
    #[serial]
    fn runtime_llm_lane_loop_rebuilds_the_snapshot_once_and_does_not_save() {
        let isolated = IsolatedSettings::new();
        let path = isolated.path();
        fs::write(&path, settled_settings_json().to_string()).unwrap();
        let mtime = fs::metadata(&path).unwrap().modified().unwrap();
        super::RUNTIME_SNAPSHOT_BUILDS.store(0, std::sync::atomic::Ordering::SeqCst);
        let (_, saves) = with_save_log_count(|| {
            for _ in 0..50 {
                let _ = runtime_llm_lane(CsLlmLane::Formatting);
            }
        });
        assert_eq!(saves, 0, "lane projection must not emit Saved settings");
        assert_eq!(
            super::RUNTIME_SNAPSHOT_BUILDS.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "fifty reads without a credential mutation rebuild the snapshot once \
             (path={}, mtime_at_start={mtime:?}, mtime_now={:?}; an mtime move means \
             a concurrent non-serial load wrote this settings.json through the shared \
             CODESCRIBE_DATA_DIR/CODESCRIBE_ENV_PATH process state)",
            path.display(),
            fs::metadata(&path).and_then(|m| m.modified()).ok(),
        );
        assert_eq!(
            mtime,
            fs::metadata(&path).unwrap().modified().unwrap(),
            "repeated lane projection must not rewrite settings.json"
        );
    }
    #[test]
    #[serial]
    fn composer_cache_stamp_is_metadata_only_and_missing_metadata_refuses_reuse() {
        let isolated = IsolatedSettings::new();
        let config = CodescribeConfig::new();
        let probe = codescribe_core::config::keychain::CredentialAcquisitionProbe::forbid();
        assert!(config.composer_model_cache_stamp().is_none());
        fs::write(isolated.path(), settled_settings_json().to_string()).unwrap();
        let first = config.composer_model_cache_stamp().unwrap();
        assert_eq!(
            config.composer_model_cache_stamp().as_deref(),
            Some(first.as_str())
        );
        assert!(
            super::last_good_runtime_snapshot()
                .lock()
                .unwrap()
                .value
                .is_none()
        );
        fs::remove_file(isolated.path()).unwrap();
        assert!(config.composer_model_cache_stamp().is_none());
        assert!(probe.attempts().is_empty());
    }

    #[test]
    #[serial]
    fn composer_cache_stamp_changes_for_raw_metadata_credentials_and_runtime_invalidation() {
        let isolated = IsolatedSettings::new();
        fs::write(isolated.path(), settled_settings_json().to_string()).unwrap();
        let config = CodescribeConfig::new();
        let probe = codescribe_core::config::keychain::CredentialAcquisitionProbe::forbid();
        let first = config.composer_model_cache_stamp().unwrap();
        // An external writer can preserve bytes while publishing a new mtime.
        let file = fs::OpenOptions::new()
            .write(true)
            .open(isolated.path())
            .unwrap();
        let newer = fs::metadata(isolated.path()).unwrap().modified().unwrap()
            + std::time::Duration::from_secs(1);
        file.set_times(fs::FileTimes::new().set_modified(newer))
            .unwrap();
        let external = config.composer_model_cache_stamp().unwrap();
        assert_ne!(external, first);
        let _keys = codescribe_core::config::keychain::test_support::install_bundle(&[(
            "LLM_OPENAI_API_KEY",
            "synthetic-cache-stamp-key",
        )]);
        let credentials = config.composer_model_cache_stamp().unwrap();
        assert_ne!(credentials, external);
        super::invalidate_runtime_snapshot_cache();
        assert_ne!(config.composer_model_cache_stamp().unwrap(), credentials);
        assert!(probe.attempts().is_empty());
    }

    #[test]
    #[serial]
    fn runtime_invalidation_during_a_load_cannot_repopulate_earlier_generation() {
        let isolated = IsolatedSettings::new();
        fs::write(isolated.path(), settled_settings_json().to_string()).unwrap();
        struct ResetHook;
        impl Drop for ResetHook {
            fn drop(&mut self) {
                super::INVALIDATE_AFTER_RUNTIME_LOAD
                    .store(false, std::sync::atomic::Ordering::SeqCst);
            }
        }
        let _reset = ResetHook;
        super::INVALIDATE_AFTER_RUNTIME_LOAD.store(true, std::sync::atomic::Ordering::SeqCst);
        let _first = super::load_runtime_snapshot_for_lane();
        assert!(
            super::last_good_runtime_snapshot()
                .lock()
                .unwrap()
                .value
                .is_none(),
            "a loader begun before invalidation must not restore its old snapshot"
        );
        let _fresh = super::load_runtime_snapshot_for_lane();
        assert!(
            super::last_good_runtime_snapshot()
                .lock()
                .unwrap()
                .value
                .is_some()
        );
    }
}

mod lane_unavailable_projection_tests {
    use super::{CsLlmLane, CsLlmLaneUnavailable};
    use codescribe_core::config::{RuntimeLlmLaneKind, RuntimeLlmLaneUnavailable};

    #[test]
    fn every_variant_projects_its_data() {
        let cases = [
            (
                RuntimeLlmLaneUnavailable::CustomProviderMissing {
                    custom_id: "gone".into(),
                },
                CsLlmLaneUnavailable::CustomProviderMissing {
                    custom_id: "gone".into(),
                },
            ),
            (
                RuntimeLlmLaneUnavailable::ModelMissing {
                    provider: "My Box".into(),
                },
                CsLlmLaneUnavailable::ModelMissing {
                    provider: "My Box".into(),
                },
            ),
            (
                RuntimeLlmLaneUnavailable::CredentialMissing {
                    lane: RuntimeLlmLaneKind::Formatting,
                    provider: "OpenAI (Responses)".into(),
                    endpoint: "https://api.openai.com/v1/responses".into(),
                    key_account: "LLM_OPENAI_API_KEY".into(),
                },
                CsLlmLaneUnavailable::CredentialMissing {
                    lane: CsLlmLane::Formatting,
                    provider: "OpenAI (Responses)".into(),
                    endpoint: "https://api.openai.com/v1/responses".into(),
                    key_account: "LLM_OPENAI_API_KEY".into(),
                },
            ),
            (
                RuntimeLlmLaneUnavailable::CloudCodeAssistAccountRequired {
                    lane: RuntimeLlmLaneKind::Assistive,
                },
                CsLlmLaneUnavailable::CloudCodeAssistAccountRequired {
                    lane: CsLlmLane::Assistive,
                },
            ),
            (
                RuntimeLlmLaneUnavailable::AcpLaunchInvalid,
                CsLlmLaneUnavailable::AcpLaunchInvalid,
            ),
            (
                RuntimeLlmLaneUnavailable::AcpWorkspaceRootsMissing,
                CsLlmLaneUnavailable::AcpWorkspaceRootsMissing,
            ),
            (
                RuntimeLlmLaneUnavailable::AcpCwdOutsideWorkspace,
                CsLlmLaneUnavailable::AcpCwdOutsideWorkspace,
            ),
        ];
        for (core, bridge) in cases {
            assert_eq!(CsLlmLaneUnavailable::from(&core), bridge);
        }
    }
}

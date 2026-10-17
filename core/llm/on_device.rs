//! Host-owned on-device formatting (Apple FoundationModels).
//!
//! Core cannot speak to the system model — FoundationModels has no C ABI — so
//! the host (SwiftUI over the UniFFI bridge) registers a formatter here at
//! bootstrap, the same "host-owned execution" shape the Max rung already uses
//! through `FormattingAgent`. The formatting funnel consults this module
//! before the cloud wire: when the lane is selected and a formatter is
//! registered, one on-device attempt runs first and any failure falls back to
//! the configured cloud lane inside the same attempt, so refusal detection,
//! `AiNoop` detection and protected-lexicon handling stay in Rust either way.
//!
//! Selection is an explicit power knob (`CODESCRIBE_FORMAT_ON_DEVICE`), not a
//! default: Polish is contractually unsupported by the system model even
//! though it formats it in practice (probe 2026-09-25: ~3.2 s / 350 chars,
//! `supportsLocale(pl) == false`, no refusal), so the Founder opts in.

use std::sync::{Arc, OnceLock, RwLock};

/// Why one on-device attempt produced no text. The kinds mirror the SDK 27
/// `LanguageModelError` cases the funnel must treat differently only in logs —
/// every kind falls back to the cloud lane.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OnDeviceFormatError {
    Unavailable(String),
    UnsupportedLanguage(String),
    Refused(String),
    ContextExceeded(String),
    Other(String),
}

impl std::fmt::Display for OnDeviceFormatError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unavailable(detail) => write!(f, "on-device model unavailable: {detail}"),
            Self::UnsupportedLanguage(detail) => {
                write!(f, "language or locale unsupported: {detail}")
            }
            Self::Refused(detail) => write!(f, "guardrail or refusal: {detail}"),
            Self::ContextExceeded(detail) => write!(f, "context window exceeded: {detail}"),
            Self::Other(detail) => write!(f, "{detail}"),
        }
    }
}

impl std::error::Error for OnDeviceFormatError {}

/// Host capability: format `user_message` under the sealed `instructions` on
/// the device model. The prompt pair arrives exactly as the cloud lane would
/// send it — the sealed Smart/Correction prompt verbatim as instructions and
/// the `[Language: …]`-prefixed take as the user message.
#[async_trait::async_trait]
pub trait OnDeviceFormatter: Send + Sync {
    async fn format(
        &self,
        instructions: &str,
        user_message: &str,
    ) -> Result<String, OnDeviceFormatError>;
}

type SharedFormatter = RwLock<Option<Arc<dyn OnDeviceFormatter>>>;

fn shared_formatter() -> &'static SharedFormatter {
    static FORMATTER: OnceLock<SharedFormatter> = OnceLock::new();
    FORMATTER.get_or_init(|| RwLock::new(None))
}

/// Install (or replace) the host formatter. The bridge calls this once at
/// bootstrap; re-registration replaces, so a Swift relaunch cannot fork state.
pub fn register_on_device_formatter(formatter: Arc<dyn OnDeviceFormatter>) {
    let mut slot = shared_formatter()
        .write()
        .unwrap_or_else(|error| error.into_inner());
    *slot = Some(formatter);
}

/// The registered host formatter, if the host installed one.
pub fn on_device_formatter() -> Option<Arc<dyn OnDeviceFormatter>> {
    shared_formatter()
        .read()
        .unwrap_or_else(|error| error.into_inner())
        .as_ref()
        .map(Arc::clone)
}

/// Opt-in key: routes the (non-assistive) formatting lane to the on-device
/// model first. One key, two writers — the developer Lab toggle (settings
/// `speech.formatting.on_device`) and the `.env` power knob. The runtime
/// snapshot resolves them once per settings generation into
/// `Config::format_on_device`, the process value outranking Settings.
/// Doctrine 2026-09-25: opt-in defaults.
pub const FORMAT_ON_DEVICE_ENV: &str = "CODESCRIBE_FORMAT_ON_DEVICE";

#[cfg(test)]
mod tests {
    use super::*;

    struct RestoreFormatter(Option<Arc<dyn OnDeviceFormatter>>);

    impl Drop for RestoreFormatter {
        fn drop(&mut self) {
            *shared_formatter().write().unwrap() = self.0.take();
        }
    }

    async fn prompt_authority_case(
        raw: &str,
        answer: Result<String, OnDeviceFormatError>,
        expected_status: crate::llm::ai_formatting::AiFormatStatus,
        expected_text: &str,
    ) {
        use crate::config::{CapturedRuntimeInputs, Config};
        use crate::llm::ai_formatting::format_text_with_status_for_policy;
        use std::sync::Mutex;

        type Calls = Arc<Mutex<Vec<(String, String)>>>;
        struct Host {
            answer: Result<String, OnDeviceFormatError>,
            calls: Calls,
        }
        #[async_trait::async_trait]
        impl OnDeviceFormatter for Host {
            async fn format(
                &self,
                instructions: &str,
                message: &str,
            ) -> Result<String, OnDeviceFormatError> {
                self.calls
                    .lock()
                    .unwrap()
                    .push((instructions.into(), message.into()));
                self.answer.clone()
            }
        }
        let _restore = RestoreFormatter(shared_formatter().write().unwrap().take());
        let calls: Calls = Arc::default();
        register_on_device_formatter(Arc::new(Host {
            answer,
            calls: calls.clone(),
        }));
        let root = tempfile::tempdir().unwrap();
        for policy in ["correction", "smart"] {
            let mut input = CapturedRuntimeInputs::defaults_at(root.path().into(), 0);
            input.user_settings.llm_formatting_provider =
                Some("custom:unconfigured-formatter".into());
            input.user_settings.formatting_level = Some(policy.into());
            input.user_settings.format_on_device = Some(true);
            input.values.format_on_device = true;
            let snapshot = Config::runtime_snapshot_from_captured(input);
            assert!(!snapshot.llm_lanes().formatting().available());
            let prompt = snapshot
                .ai_execution()
                .formatter()
                .formatting_prompt()
                .unwrap()
                .composed_content()
                .to_string();
            let result = format_text_with_status_for_policy(raw, Some("pl"), &snapshot, None).await;
            assert_eq!(result.status, expected_status, "{policy}");
            assert_eq!(result.text, expected_text, "{policy}");
            let actual = calls.lock().unwrap().last().unwrap().clone();
            assert_eq!(actual, (prompt, format!("[Language: pl]\n\n{raw}")));
        }
        assert_eq!(calls.lock().unwrap().len(), 2);
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn prompt_authority_preserves_provider_input_and_accepts_lexical_edits() {
        prompt_authority_case(
            "Iwo Iwo Iwo plan był dobry i naprawdę ważny",
            Ok("Dobry plan był ważny.".into()),
            crate::llm::ai_formatting::AiFormatStatus::Applied,
            "Dobry plan był ważny.",
        )
        .await;
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn prompt_authority_publishes_paragraph_only_changes() {
        prompt_authority_case(
            "To jest pierwsze zdanie. To jest drugie zdanie.",
            Ok("To jest pierwsze zdanie.\n\nTo jest drugie zdanie.".into()),
            crate::llm::ai_formatting::AiFormatStatus::Applied,
            "To jest pierwsze zdanie.\n\nTo jest drugie zdanie.",
        )
        .await;
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn prompt_authority_does_not_censor_quoted_refusal_words() {
        prompt_authority_case(
            "Please quote the phrase I cannot help in this sentence",
            Ok("The phrase is: I cannot help.".into()),
            crate::llm::ai_formatting::AiFormatStatus::Applied,
            "The phrase is: I cannot help.",
        )
        .await;
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn prompt_authority_exact_echo_is_noop() {
        let raw = "Iwo Iwo Iwo plan był dobry i naprawdę ważny";
        prompt_authority_case(
            raw,
            Ok(raw.into()),
            crate::llm::ai_formatting::AiFormatStatus::AiNoop,
            raw,
        )
        .await;
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn prompt_authority_actual_provider_error_keeps_original_repetitions() {
        let raw = "Iwo Iwo Iwo plan był dobry i naprawdę ważny";
        prompt_authority_case(
            raw,
            Err(OnDeviceFormatError::Unavailable(
                "synthetic host failure".into(),
            )),
            crate::llm::ai_formatting::AiFormatStatus::Failed,
            raw,
        )
        .await;
    }

    struct Fixed(&'static str);

    #[async_trait::async_trait]
    impl OnDeviceFormatter for Fixed {
        async fn format(
            &self,
            _instructions: &str,
            _user_message: &str,
        ) -> Result<String, OnDeviceFormatError> {
            Ok(self.0.to_string())
        }
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn a_registered_formatter_is_shared_and_replaceable() {
        register_on_device_formatter(Arc::new(Fixed("first")));
        let first = on_device_formatter().expect("registered");
        assert_eq!(first.format("i", "u").await.unwrap(), "first");

        register_on_device_formatter(Arc::new(Fixed("second")));
        let second = on_device_formatter().expect("still registered");
        assert_eq!(second.format("i", "u").await.unwrap(), "second");
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn apple_formats_text_without_cloud_audio_or_seal_and_off_refuses_missing_cloud() {
        use crate::config::{CapturedRuntimeInputs, Config};
        use crate::llm::ai_formatting::{AiFormatStatus, format_text_with_status_for_policy};
        use std::sync::atomic::{AtomicUsize, Ordering};

        struct Host(Arc<AtomicUsize>);
        #[async_trait::async_trait]
        impl OnDeviceFormatter for Host {
            async fn format(
                &self,
                instructions: &str,
                user_message: &str,
            ) -> Result<String, OnDeviceFormatError> {
                assert!(!instructions.is_empty());
                assert!(user_message.contains("please preserve every word in this sentence"));
                self.0.fetch_add(1, Ordering::SeqCst);
                Ok("Please preserve every word in this sentence.".into())
            }
        }
        struct Restore(Option<Arc<dyn OnDeviceFormatter>>);
        impl Drop for Restore {
            fn drop(&mut self) {
                *shared_formatter().write().unwrap() = self.0.take();
            }
        }
        let root = tempfile::tempdir().unwrap();
        let _restore = Restore(shared_formatter().write().unwrap().take());
        let calls = Arc::new(AtomicUsize::new(0));
        register_on_device_formatter(Arc::new(Host(calls.clone())));
        // The routing contract consumes an immutable captured snapshot. Avoid
        // a process-global settings/env read racing other loader fixtures.
        let mut input = CapturedRuntimeInputs::defaults_at(root.path().to_path_buf(), 0);
        input.user_settings.llm_formatting_provider = Some("custom:unconfigured-formatter".into());
        input.user_settings.llm_assistive_provider = Some("custom:unconfigured-agent".into());
        let raw = "please preserve every word in this sentence";
        for policy in ["correction", "smart"] {
            input.user_settings.formatting_level = Some(policy.into());
            input.user_settings.format_on_device = Some(true);
            input.values.format_on_device = true;
            let snapshot = Config::runtime_snapshot_from_captured(input.clone());
            // Credential availability can refresh from other test fixtures;
            // this deliberately missing provider makes cloud requests refuse
            // regardless of any key present in the process.
            assert!(!snapshot.llm_lanes().formatting().available());
            assert!(
                snapshot
                    .llm_lanes()
                    .formatting()
                    .request_unavailable_reason()
                    .is_some()
            );
            assert!(
                snapshot
                    .llm_lanes()
                    .formatting()
                    .credential()
                    .api_key()
                    .is_none()
            );
            assert!(
                crate::llm::ai_formatting::text_formatting_unavailable_reason(&snapshot).is_none()
            );
            let output = format_text_with_status_for_policy(raw, Some("en"), &snapshot, None).await;
            assert_eq!(output.status, AiFormatStatus::Applied);
            assert_eq!(output.text, "Please preserve every word in this sentence.");
            input.user_settings.format_on_device = Some(false);
            input.values.format_on_device = false;
            let snapshot = Config::runtime_snapshot_from_captured(input.clone());
            let output = format_text_with_status_for_policy(raw, Some("en"), &snapshot, None).await;
            assert_eq!(output.status, AiFormatStatus::Failed);
            assert!(output.text.to_lowercase().contains(raw));
        }
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }
}

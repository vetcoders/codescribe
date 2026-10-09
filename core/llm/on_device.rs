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
        use crate::config::{Config, UserSettings};
        use crate::llm::ai_formatting::{AiFormatStatus, format_text_with_status_for_policy};
        use crate::test_isolation::EnvGuard;
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
        let _data = EnvGuard::set("CODESCRIBE_DATA_DIR", root.path().to_str().unwrap());
        let _keychain = EnvGuard::set("CODESCRIBE_DISABLE_KEYCHAIN", "1");
        let _selectors = [
            "LLM_FORMATTING_PROVIDER",
            "LLM_FORMATTING_MODEL",
            "LLM_ASSISTIVE_PROVIDER",
            "LLM_ASSISTIVE_MODEL",
        ]
        .map(EnvGuard::remove);
        let _apple = EnvGuard::remove(FORMAT_ON_DEVICE_ENV);
        let _restore = Restore(shared_formatter().write().unwrap().take());
        let calls = Arc::new(AtomicUsize::new(0));
        register_on_device_formatter(Arc::new(Host(calls.clone())));
        let mut settings = UserSettings {
            llm_formatting_provider: Some("custom:unconfigured-formatter".into()),
            llm_assistive_provider: Some("custom:unconfigured-agent".into()),
            ..Default::default()
        };
        let raw = "please preserve every word in this sentence";
        for policy in ["correction", "smart"] {
            settings.format_on_device = Some(true);
            settings.save().unwrap();
            let _policy = EnvGuard::set("FORMATTING_LEVEL", policy);
            let snapshot = Config::load_runtime_snapshot().unwrap();
            assert!(!snapshot.llm_lanes().formatting().request_available());
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
            settings.format_on_device = Some(false);
            settings.save().unwrap();
            let snapshot = Config::load_runtime_snapshot().unwrap();
            let output = format_text_with_status_for_policy(raw, Some("en"), &snapshot, None).await;
            assert_eq!(output.status, AiFormatStatus::Failed);
            assert!(output.text.to_lowercase().contains(raw));
        }
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }
}

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

/// Opt-in knob: truthy values route the (non-assistive) formatting lane to
/// the on-device model first. Doctrine 2026-09-25: opt-in defaults, power
/// knobs in `.env`.
pub const FORMAT_ON_DEVICE_ENV: &str = "CODESCRIBE_FORMAT_ON_DEVICE";

/// True when the knob selects the on-device lane. A selected lane without a
/// registered formatter still falls through to the cloud, loudly.
pub fn on_device_formatting_selected() -> bool {
    std::env::var(FORMAT_ON_DEVICE_ENV).is_ok_and(|value| {
        matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        )
    })
}

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
    async fn a_registered_formatter_is_shared_and_replaceable() {
        register_on_device_formatter(Arc::new(Fixed("first")));
        let first = on_device_formatter().expect("registered");
        assert_eq!(first.format("i", "u").await.unwrap(), "first");

        register_on_device_formatter(Arc::new(Fixed("second")));
        let second = on_device_formatter().expect("still registered");
        assert_eq!(second.format("i", "u").await.unwrap(), "second");
    }
}

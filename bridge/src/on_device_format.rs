//! On-device formatting bridge (W6): Swift implements the formatter over
//! Apple FoundationModels (`LanguageModelSession`); this module hops it into
//! `codescribe_core::llm::on_device`, where the formatting funnel consults it
//! before the cloud wire. Registration mirrors the other foreign listeners:
//! process-global, replace-on-re-register, installed once at bootstrap.
//!
//! The foreign call is synchronous on purpose — the model takes seconds and
//! Swift owns its own concurrency; core hops it off the async runtime with
//! `spawn_blocking`, so a slow host attempt never stalls the runtime.

use std::sync::Arc;

use codescribe_core::llm::on_device::{
    OnDeviceFormatError, OnDeviceFormatter, register_on_device_formatter,
};

use crate::hotkeys::CodescribeHotkeys;

/// Outcome of one host formatting attempt. Error kinds mirror the SDK 27
/// `LanguageModelError` cases the engine logs distinctly; every error falls
/// back to the configured cloud lane.
#[derive(uniffi::Enum, Debug, Clone)]
pub enum CsOnDeviceFormatOutcome {
    /// The model produced formatted text.
    Formatted { text: String },
    /// Model or Apple Intelligence unavailable (`availability != available`).
    Unavailable { message: String },
    /// `unsupportedLanguageOrLocale`.
    UnsupportedLanguage { message: String },
    /// `refusal` or `guardrailViolation`.
    Refused { message: String },
    /// `contextSizeExceeded` — the take does not fit the 8k window.
    ContextExceeded { message: String },
    /// Anything else (timeout, rate limit, unexpected error).
    Failed { message: String },
}

/// Foreign capability: format `user_message` under the sealed `instructions`
/// on the device model. Swift receives the sealed prompt VERBATIM as the
/// session instructions and must not amend it (operator-owned prompt).
#[uniffi::export(with_foreign)]
pub trait CsOnDeviceFormatter: Send + Sync {
    fn format(&self, instructions: String, user_message: String) -> CsOnDeviceFormatOutcome;
}

/// Adapter: the core-side trait over the registered Swift formatter.
struct HostOnDeviceFormatter(Arc<dyn CsOnDeviceFormatter>);

#[async_trait::async_trait]
impl OnDeviceFormatter for HostOnDeviceFormatter {
    async fn format(
        &self,
        instructions: &str,
        user_message: &str,
    ) -> Result<String, OnDeviceFormatError> {
        let host = Arc::clone(&self.0);
        let instructions = instructions.to_string();
        let user_message = user_message.to_string();
        let outcome = tokio::task::spawn_blocking(move || host.format(instructions, user_message))
            .await
            .map_err(|error| OnDeviceFormatError::Other(error.to_string()))?;
        match outcome {
            CsOnDeviceFormatOutcome::Formatted { text } => Ok(text),
            CsOnDeviceFormatOutcome::Unavailable { message } => {
                Err(OnDeviceFormatError::Unavailable(message))
            }
            CsOnDeviceFormatOutcome::UnsupportedLanguage { message } => {
                Err(OnDeviceFormatError::UnsupportedLanguage(message))
            }
            CsOnDeviceFormatOutcome::Refused { message } => {
                Err(OnDeviceFormatError::Refused(message))
            }
            CsOnDeviceFormatOutcome::ContextExceeded { message } => {
                Err(OnDeviceFormatError::ContextExceeded(message))
            }
            CsOnDeviceFormatOutcome::Failed { message } => Err(OnDeviceFormatError::Other(message)),
        }
    }
}

#[uniffi::export]
impl CodescribeHotkeys {
    /// Install the host on-device formatter (bootstrap; re-register replaces).
    /// Selection stays with the engine: the formatter only runs when the
    /// `CODESCRIBE_FORMAT_ON_DEVICE` knob picks the on-device lane.
    pub fn set_on_device_formatter(&self, formatter: Arc<dyn CsOnDeviceFormatter>) {
        register_on_device_formatter(Arc::new(HostOnDeviceFormatter(formatter)));
        tracing::info!("on-device formatter registered by the host");
    }
}

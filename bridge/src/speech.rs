//! On-demand Agent speech on the process-owned application runtime.
use crate::{CsError, application_runtime};
use codescribe_core::llm::speech;

/// Outcome of an explicitly requested spoken assistant turn.
#[derive(Clone, Debug, uniffi::Record)]
pub struct CsSpeechResult {
    /// `played` or `stopped`; failures use the bridge error channel.
    pub outcome: String,
    /// Duration of synthesized PCM (zero if cancelled during synthesis).
    pub duration_ms: u64,
    /// Whether synthesis used the disk cache.
    pub cached: bool,
}

/// None means locally configured; server permissions are checked by speak_text.
#[uniffi::export]
pub fn speech_availability() -> Option<String> {
    speech::speech_availability()
}

/// Cancel current playback and invalidate pending synthesis.
#[uniffi::export]
pub fn stop_speaking() {
    speech::playback::stop();
}

/// Speak the turn through the provider currently selected for the assistive lane.
#[uniffi::export]
pub async fn speak_text(text: String) -> Result<CsSpeechResult, CsError> {
    let ticket = speech::playback::begin();
    application_runtime::run(async move {
        let Some(audio) = synthesize_until_stopped(ticket, speech::synthesize(&text))
            .await
            .map_err(anyhow::Error::from)?
        else {
            return Ok(CsSpeechResult {
                outcome: "stopped".into(),
                duration_ms: 0,
                cached: false,
            });
        };
        let duration_ms = audio.duration_ms();
        let cached = audio.cached;
        let played = tokio::task::spawn_blocking(move || {
            speech::playback::play(audio.samples, audio.sample_rate, ticket)
        })
        .await
        .map_err(anyhow::Error::from)??;
        Ok(CsSpeechResult {
            outcome: if played { "played" } else { "stopped" }.into(),
            duration_ms,
            cached,
        })
    })
    .await?
}

async fn synthesize_until_stopped(
    ticket: u64,
    synthesis: impl std::future::Future<Output = Result<speech::SpeechAudio, speech::SpeechError>>,
) -> Result<Option<speech::SpeechAudio>, speech::SpeechError> {
    let cancelled = async {
        while speech::playback::current(ticket) {
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    };
    tokio::select! {
        biased;
        _ = cancelled => Ok(None),
        result = synthesis => result.map(Some),
    }
}

#[cfg(test)]
mod rc_w1_tests {
    use super::*;

    #[tokio::test]
    async fn stop_drops_pending_synthesis_and_wins_over_late_refusal() {
        let ticket = speech::playback::begin();
        let pending = async {
            speech::playback::stop();
            std::future::pending::<Result<speech::SpeechAudio, speech::SpeechError>>().await
        };
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(1),
            synthesize_until_stopped(ticket, pending),
        )
        .await
        .expect("stop must settle pending synthesis")
        .expect("stop is not a provider failure");
        assert!(result.is_none());
        let result =
            synthesize_until_stopped(ticket, async { Err(speech::SpeechError::Http(403)) })
                .await
                .expect("a stopped request must not publish its late refusal");
        assert!(result.is_none());
    }
}

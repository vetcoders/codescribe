//! Locked THE ENGINE contract for quality-report HTML and corpus JSON.
//!
//! This module exists so an agent cannot re-invent sealed/committed on every
//! session. The bars, the relay, the forbidden operations, and the product
//! goal are compile-time constants. Quality HTML embeds them. Corpus schema
//! v3 names them. Tests fail if anyone "simplifies" the doctrine back to
//! "the whole text is mutable until session seal".

use serde::{Deserialize, Serialize};

/// Schema id carried by every `codescribe-corpus` report that honours this lock.
pub const CORPUS_REPORT_SCHEMA: &str = "codescribe-corpus-parity/v3";

/// Stable id of the engine contract itself.
pub const ENGINE_CONTRACT_ID: &str = "the-engine/v1";

/// Path of the agent-facing prose lock, relative to the repo root.
pub const ENGINE_CONTRACT_DOC: &str = "docs/THE_ENGINE_CONTRACT.md";

/// What a quality report is allowed to treat as the document vs a proposal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReportSurfaceRole {
    /// Live Apple hypothesis for an open or just-committed span.
    LiveHypothesis,
    /// Layer-1 hole-fill inside a still-unsealed span.
    WhisperHoleFill,
    /// Closed span after Apple + Whisper + lexicon fusion.
    SealedSpan,
    /// Session document after `transcript_sealed`.
    SessionDocument,
    /// Full-file HQ or Cloud pass after session seal — never auto-applied.
    HumanTriggeredProposal,
}

/// One of the three finality bars. Not synonyms.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FinalityBar {
    /// This layer finished its hypothesis for the fragment. The layer is
    /// banned from further overwrite of that span. Preview stays grey;
    /// committed is bright. This is not the document.
    UtteranceFinal,
    /// Apple + Whisper + lexicon finished fusion for a Silero-bounded span.
    /// The record `[sample_start, sample_end)` becomes append-only and may
    /// start inline formatting. Order on the PCM axis is frozen.
    UtteranceSealed,
    /// The whole session — tail and formatter included — was assembled into
    /// the document. Automation puts its hands down. Full HQ / Cloud may
    /// only propose a variant.
    TranscriptSealed,
}

/// A layer in the live relay. Ban is per-layer, per-span: the layer that
/// already passed this span is out; the next one may enrich the same time
/// window.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RelayLayer {
    Apple,
    Whisper,
    Lexicon,
    Formatter,
    Human,
}

/// Machine-readable lock. Quality HTML and corpus JSON must serialize this
/// object, not a free-form paragraph an agent can paraphrase.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EngineContract {
    pub id: &'static str,
    pub primary_key: &'static str,
    pub relay: &'static [RelayLayer],
    pub bars: &'static [FinalityBar],
    pub forbidden: &'static [&'static str],
    pub whisper_window: &'static str,
    pub full_file_pass: &'static str,
    pub product_goal: &'static str,
}

/// The only contract instance quality reports may emit.
pub const ENGINE_CONTRACT: EngineContract = EngineContract {
    id: ENGINE_CONTRACT_ID,
    primary_key: "pcm_time",
    relay: &[
        RelayLayer::Apple,
        RelayLayer::Whisper,
        RelayLayer::Lexicon,
        RelayLayer::Formatter,
        RelayLayer::Human,
    ],
    bars: &[
        FinalityBar::UtteranceFinal,
        FinalityBar::UtteranceSealed,
        FinalityBar::TranscriptSealed,
    ],
    forbidden: &[
        "rewrite_from_zero",
        "reorder_spans",
        "hallucinate_into_silence",
        "full_file_in_automatic_pipeline",
        "auto_replace_after_transcript_sealed",
        "treat_committed_as_document",
        "treat_whole_text_mutable_until_session_seal",
    ],
    whisper_window: "3-5s utterance-bounded partials",
    full_file_pass: "button_only_proposal",
    product_goal: "energy × time → the true sentence, live in the buffer, ~10ms to paste",
};

/// Role of a named quality-report column. WER against a column does not
/// promote that column to document.
pub fn surface_role(column: &str) -> Option<ReportSurfaceRole> {
    match column {
        "raw" | "live" => Some(ReportSurfaceRole::LiveHypothesis),
        "post" | "layer1" => Some(ReportSurfaceRole::WhisperHoleFill),
        "sealed" => Some(ReportSurfaceRole::SealedSpan),
        "delivered" | "session" => Some(ReportSurfaceRole::SessionDocument),
        "ai" | "ai_formatted" | "cloud" | "hq" => Some(ReportSurfaceRole::HumanTriggeredProposal),
        _ => None,
    }
}

/// Self-contained HTML plate. Inlined into Qube / corpus / teacher reports
/// so opening any quality HTML shows the lock before the scores.
pub fn render_engine_contract_html() -> String {
    let bars = [
        (
            "utterance_final / committed",
            "This layer finished its hypothesis for the fragment. That layer is banned from further overwrite of this span. Preview grey, committed bright. Not the document.",
        ),
        (
            "utterance_sealed",
            "Apple + Whisper + lexicon finished fusion for the Silero-bounded span. Record [sample_start, sample_end) is append-only and may start inline formatting. Order on the PCM axis is frozen.",
        ),
        (
            "transcript_sealed",
            "The session — tail and formatter included — was assembled into the document. Automation puts its hands down. Full HQ / Cloud may only propose a variant.",
        ),
    ];
    let mut rows = String::new();
    for (name, meaning) in bars {
        rows.push_str(&format!("<tr><th>{name}</th><td>{meaning}</td></tr>\n"));
    }
    let forbidden = ENGINE_CONTRACT
        .forbidden
        .iter()
        .map(|item| format!("<li><code>{item}</code></li>"))
        .collect::<Vec<_>>()
        .join("");
    format!(
        r#"<section class="engine-contract" data-contract="{id}" data-primary-key="{key}">
<p class="engine-contract-kicker">THE ENGINE · quality-report contract · {id}</p>
<h2>Place on the canvas is given by energy in time — not by tokens.</h2>
<p class="engine-contract-goal">{goal}</p>
<p class="engine-contract-relay">Relay: Apple → Whisper → lexicon → formatter → human. Ban is per layer, per span. Whisper works 3–5 s partials at utterance boundaries and fills holes. It does not hallucinate into silence and does not see full audio unless a human presses the button.</p>
<table class="engine-contract-bars">
<thead><tr><th>Bar</th><th>Means</th></tr></thead>
<tbody>
{rows}
</tbody>
</table>
<p class="engine-contract-not">Before <code>transcript_sealed</code> the whole document is <strong>not</strong> mutable. Closed spans stay on the PCM axis. The tail may still evolve. Whisper may replace weaker evidence inside a still-unsealed span. Stop closes only the tail.</p>
<ul class="engine-contract-forbidden">{forbidden}</ul>
</section>
"#,
        id = ENGINE_CONTRACT.id,
        key = ENGINE_CONTRACT.primary_key,
        goal = ENGINE_CONTRACT.product_goal,
        rows = rows,
        forbidden = forbidden,
    )
}

/// CSS for the plate. Safe on the light Qube page and the dark teacher page.
pub fn engine_contract_css() -> &'static str {
    r#"
.engine-contract { border: 1px solid #1f2937; border-radius: 12px; padding: 16px 18px; margin: 16px 0 20px; background: #111827; color: #e5e7eb; }
.engine-contract-kicker { font-size: 0.72rem; letter-spacing: 0.14em; text-transform: uppercase; color: #93c5fd; margin: 0 0 8px; }
.engine-contract h2 { font-size: 1.05rem; margin: 0 0 8px; color: #fff; }
.engine-contract-goal { font-size: 0.95rem; color: #fde68a; margin: 0 0 10px; }
.engine-contract-relay, .engine-contract-not { font-size: 0.88rem; color: #d1d5db; margin: 0 0 10px; }
.engine-contract-bars { width: 100%; border-collapse: collapse; font-size: 0.85rem; margin: 0 0 10px; }
.engine-contract-bars th, .engine-contract-bars td { border-bottom: 1px solid #374151; padding: 6px 8px; text-align: left; vertical-align: top; }
.engine-contract-bars th { width: 28%; color: #93c5fd; }
.engine-contract-forbidden { margin: 0; padding-left: 1.2rem; font-size: 0.82rem; color: #fca5a5; }
.engine-contract-forbidden code { color: #fecaca; }
"#
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contract_id_and_schema_are_stable() {
        assert_eq!(ENGINE_CONTRACT.id, "the-engine/v1");
        assert_eq!(CORPUS_REPORT_SCHEMA, "codescribe-corpus-parity/v3");
        assert_eq!(ENGINE_CONTRACT.primary_key, "pcm_time");
    }

    #[test]
    fn three_bars_in_order_and_not_synonyms() {
        assert_eq!(ENGINE_CONTRACT.bars.len(), 3);
        assert_eq!(ENGINE_CONTRACT.bars[0], FinalityBar::UtteranceFinal);
        assert_eq!(ENGINE_CONTRACT.bars[1], FinalityBar::UtteranceSealed);
        assert_eq!(ENGINE_CONTRACT.bars[2], FinalityBar::TranscriptSealed);
    }

    #[test]
    fn relay_is_apple_then_whisper_then_lexicon_then_formatter_then_human() {
        assert_eq!(
            ENGINE_CONTRACT.relay,
            &[
                RelayLayer::Apple,
                RelayLayer::Whisper,
                RelayLayer::Lexicon,
                RelayLayer::Formatter,
                RelayLayer::Human
            ]
        );
    }

    #[test]
    fn whole_text_mutable_until_seal_is_explicitly_forbidden() {
        assert!(
            ENGINE_CONTRACT
                .forbidden
                .contains(&"treat_whole_text_mutable_until_session_seal")
        );
        assert!(ENGINE_CONTRACT.forbidden.contains(&"rewrite_from_zero"));
        assert!(ENGINE_CONTRACT.forbidden.contains(&"reorder_spans"));
        assert!(
            ENGINE_CONTRACT
                .forbidden
                .contains(&"hallucinate_into_silence")
        );
        assert!(
            ENGINE_CONTRACT
                .forbidden
                .contains(&"full_file_in_automatic_pipeline")
        );
        assert!(
            ENGINE_CONTRACT
                .forbidden
                .contains(&"auto_replace_after_transcript_sealed")
        );
        assert!(
            ENGINE_CONTRACT
                .forbidden
                .contains(&"treat_committed_as_document")
        );
    }

    #[test]
    fn hq_and_cloud_are_proposals_not_documents() {
        assert_eq!(
            surface_role("cloud"),
            Some(ReportSurfaceRole::HumanTriggeredProposal)
        );
        assert_eq!(
            surface_role("hq"),
            Some(ReportSurfaceRole::HumanTriggeredProposal)
        );
        assert_eq!(
            surface_role("ai_formatted"),
            Some(ReportSurfaceRole::HumanTriggeredProposal)
        );
        assert_eq!(
            surface_role("delivered"),
            Some(ReportSurfaceRole::SessionDocument)
        );
        assert_ne!(
            surface_role("raw"),
            Some(ReportSurfaceRole::SessionDocument)
        );
    }

    #[test]
    fn html_plate_names_every_bar_and_the_pcm_key() {
        let html = render_engine_contract_html();
        assert!(html.contains("data-contract=\"the-engine/v1\""));
        assert!(html.contains("data-primary-key=\"pcm_time\""));
        assert!(html.contains("utterance_final / committed"));
        assert!(html.contains("utterance_sealed"));
        assert!(html.contains("transcript_sealed"));
        assert!(html.contains("treat_whole_text_mutable_until_session_seal"));
        assert!(
            !html.contains("the whole text is mutable"),
            "the rejected one-liner must not re-enter the plate"
        );
    }

    #[test]
    fn canonical_doc_exists_and_matches_the_lock() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
        let path = root.join(ENGINE_CONTRACT_DOC);
        let body = std::fs::read_to_string(&path)
            .unwrap_or_else(|err| panic!("{} must exist: {err}", path.display()));
        for needle in [
            "the-engine/v1",
            "pcm_time",
            "utterance_final",
            "utterance_sealed",
            "transcript_sealed",
            "treat_whole_text_mutable_until_session_seal",
            "rewrite_from_zero",
            "button_only_proposal",
            "Apple → Whisper → lexicon → formatter → human",
        ] {
            assert!(
                body.contains(needle),
                "{ENGINE_CONTRACT_DOC} missing locked token {needle:?}"
            );
        }
        assert!(
            !body.contains("cały tekst jest mutable"),
            "the rejected sentence must not live in the canonical doc"
        );
    }

    #[test]
    fn full_file_pass_is_never_automatic() {
        assert_eq!(ENGINE_CONTRACT.full_file_pass, "button_only_proposal");
        assert!(ENGINE_CONTRACT.whisper_window.contains("3-5s"));
    }
}

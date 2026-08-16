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

/// Path of the HTML-surface contract, relative to the repo root.
pub const QUALITY_HTML_CONTRACT_DOC: &str = "docs/quality-reports/CONTRACT.md";

/// Failures if this string is not a Seal Atlas quality report.
pub fn validate_quality_html(html: &str) -> Vec<String> {
    let lowered = html.to_ascii_lowercase();
    let mut failures = Vec::new();
    if !html.contains(r#"name="engine-contract""#) || !html.contains(ENGINE_CONTRACT_ID) {
        failures.push("missing meta engine-contract=the-engine/v1".into());
    }
    if !html.contains(r#"name="quality-report-surface""#)
        || !lowered.contains("seal-atlas") && !lowered.contains("seal atlas")
    {
        failures.push("missing meta quality-report-surface=seal-atlas".into());
    }
    if !lowered.contains("seal atlas") && !lowered.contains("seal-atlas") {
        failures.push("title/body must name Seal Atlas".into());
    }
    if !html.contains(r#"class="stat""#) {
        failures.push("Voice Lab handshake needs div.stat cards".into());
    }
    if !lowered.contains("word-grain") {
        failures.push("must label word-grain".into());
    }
    if !lowered.contains("utterance-grain") {
        failures.push("must label utterance-grain".into());
    }
    if !lowered.contains("clock-lie") && !html.contains("kłamstwo zegarowe") {
        failures.push("clock-lie must be a first-class finding".into());
    }
    if !html.contains("SealedSpan.words")
        && !lowered.contains("sealedspan.words")
        && !lowered.contains("sealed spans")
    {
        failures.push("must name SealedSpan.words or sealed spans".into());
    }
    if !lowered.contains("whisper") {
        failures.push("must mention whisper_words / Whisper on the same clock".into());
    }
    if lowered.contains("<h1>codescribe quality report</h1>") {
        failures.push("retired Qube H1 is not a Seal Atlas".into());
    }
    let wer_at = lowered.find("avg wer");
    let footnote_at = lowered.find("footnote");
    if let Some(wer) = wer_at {
        if footnote_at.map(|f| f < wer).unwrap_or(false) == false {
            failures.push("Avg WER may only appear after a footnote marker".into());
        }
    }
    failures
}


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
///
/// Serialize-only: the lock lives as a `const` with `&'static` slices.
/// serde cannot `Deserialize` those borrows, and nothing in the tree
/// reads this type back from JSON — the compile-time constant is the
/// source of truth.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
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

/// Required visual of a private quality HTML. WER is a footnote.
pub const QUALITY_REPORT_SURFACE: &str = "seal-atlas";

/// Gold take checked into the repo — the report an agent must not replace
/// with a scores table.
pub const SEAL_ATLAS_GOLD_HTML: &str = "docs/quality-reports/seal-atlas.take01.html";

/// Speech faster than this (characters / second over a span range) is a
/// clock-lie: the range is not the range of that speech. Take 01 span 2
/// is 410 chars/s. Conversational Polish sits well below 20.
pub const CLOCK_LIE_CHARS_PER_SEC: f32 = 30.0;

/// How a sealed span's word payload is allowed to be read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SpanGrain {
    /// SFSpeech returned more than one distinct word pin.
    Word,
    /// One segment covering the Apple commit-to-commit window.
    Utterance,
}

/// Classify grain. Per-word pins are real where they exist and never
/// guaranteed — two or more distinct ranges = word-grain, otherwise utterance.
pub fn span_grain(distinct_word_ranges: usize) -> SpanGrain {
    if distinct_word_ranges >= 2 {
        SpanGrain::Word
    } else {
        SpanGrain::Utterance
    }
}

/// Clock-lie: too many characters for the claimed PCM duration.
pub fn is_clock_lie(chars: usize, duration_secs: f32) -> bool {
    duration_secs > 0.0 && (chars as f32 / duration_secs) > CLOCK_LIE_CHARS_PER_SEC
}

/// Grapheme ticks inside a word range are an even split, never a measurement.
pub const LETTER_TIMING: &str = "interpolation_not_measurement";

/// Directory Voice Lab scans. Corpus atlas HTML must land here (or under
/// `$CODESCRIBE_ARTIFACTS_DIR`) or the operator never sees it.
pub const VOICE_LAB_ARTIFACTS_ROOT: &str = "~/.vibecrafted/artifacts/vetcoders/codescribe";

/// How Voice Lab labels a discovered HTML. Mirrors `discover_quality_reports`
/// in voice-lab `server.py` — change both or the catalog lies.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VoiceLabReportKind {
    SealAtlas,
    QualityContract,
    QualityReport,
}

impl VoiceLabReportKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::SealAtlas => "seal_atlas",
            Self::QualityContract => "quality_contract",
            Self::QualityReport => "quality_report",
        }
    }
}

/// Same classifier Voice Lab uses on `title + relative path`.
pub fn voice_lab_kind(title: &str, relative_path: &str) -> VoiceLabReportKind {
    let lowered = format!("{title} {relative_path}").to_ascii_lowercase();
    if lowered.contains("seal atlas") || lowered.contains("seal-atlas") {
        VoiceLabReportKind::SealAtlas
    } else if lowered.contains("quality") && lowered.contains("contract") {
        VoiceLabReportKind::QualityContract
    } else {
        VoiceLabReportKind::QualityReport
    }
}

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
            "seal-atlas",
            "SealedSpan.words",
            "clock-lie",
            "interpolation",
            "Voice Lab",
            "seal_atlas",
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

    #[test]
    fn take01_span2_is_the_canonical_clock_lie() {
        assert!(is_clock_lie(41, 0.10));
        assert!(!is_clock_lie(6, 0.24)); // "ten" @ 240 ms
        assert_eq!(span_grain(6), SpanGrain::Word);
        assert_eq!(span_grain(1), SpanGrain::Utterance);
        assert_eq!(LETTER_TIMING, "interpolation_not_measurement");
        assert_eq!(QUALITY_REPORT_SURFACE, "seal-atlas");
    }

    #[test]
    fn gold_atlas_passes_html_handshake() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
        let path = root.join(SEAL_ATLAS_GOLD_HTML);
        let body = std::fs::read_to_string(&path)
            .unwrap_or_else(|err| panic!("{} must exist: {err}", path.display()));
        let failures = validate_quality_html(&body);
        assert!(failures.is_empty(), "gold take 01 failed handshake: {failures:?}");
    }

    #[test]
    fn html_contract_doc_exists() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
        let body = std::fs::read_to_string(root.join(QUALITY_HTML_CONTRACT_DOC))
            .unwrap_or_else(|err| panic!("{QUALITY_HTML_CONTRACT_DOC} must exist: {err}"));
        for needle in [
            "quality-report-surface",
            "div class=\"stat\"",
            "word-grain",
            "clock-lie",
            "quality/seal-atlas.",
        ] {
            assert!(body.contains(needle), "CONTRACT.md missing {needle:?}");
        }
    }

    #[test]
    fn retired_qube_title_fails_handshake() {
        let fake = r#"<html><head><title>Codescribe Quality Report</title></head>
<body><h1>Codescribe Quality Report</h1><p>Avg WER 12%</p></body></html>"#;
        let failures = validate_quality_html(fake);
        assert!(failures.len() >= 3, "{failures:?}");
    }

    #[test]
    fn gold_atlas_html_is_a_pcm_instrument_not_a_wer_table() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
        let path = root.join(SEAL_ATLAS_GOLD_HTML);
        let body = std::fs::read_to_string(&path)
            .unwrap_or_else(|err| panic!("{} must exist: {err}", path.display()));
        for needle in [
            "Seal Atlas",
            "SealedSpan.words",
            "kłamstwo zegarowe",
            "word-grain",
            "utterance-grain",
            "równomierna interpolacja",
            "CODESCRIBE_SEAL_ATLAS_DUMP",
            "vad_atlas_probe",
            "whisper_words",
        ] {
            assert!(
                body.contains(needle),
                "{SEAL_ATLAS_GOLD_HTML} missing {needle:?}"
            );
        }
        assert!(
            !body.contains("Avg WER"),
            "gold atlas must not be a scores table"
        );
        assert!(body.contains(r#"class="stat""#));
        assert_eq!(
            voice_lab_kind(
                "Seal Atlas — take 01",
                "quality-reports/seal-atlas.take01.html"
            ),
            VoiceLabReportKind::SealAtlas
        );
        assert_eq!(
            voice_lab_kind("Codescribe Quality Report", "quality/apple-layer0.html"),
            VoiceLabReportKind::QualityReport
        );
        assert_eq!(
            voice_lab_kind("THE ENGINE quality-report contract", "docs/contract.html"),
            VoiceLabReportKind::QualityContract
        );
    }
}


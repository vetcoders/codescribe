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

/// Path of the HTML-surface contract, relative to the repo root.
pub const QUALITY_HTML_CONTRACT_DOC: &str = "docs/quality-reports/CONTRACT.md";

/// True when a `<meta>` tag carries both `name` and `content` on the same tag.
/// Prose elsewhere in the document does not satisfy the handshake.
fn meta_content_is(html: &str, name: &str, content: &str) -> bool {
    let name_attr = format!(r#"name="{name}""#);
    let content_attr = format!(r#"content="{content}""#);
    let mut rest = html;
    while let Some(name_at) = rest.find(&name_attr) {
        let before = &rest[..name_at];
        let tag_start = before.rfind('<').unwrap_or(0);
        let after = &rest[name_at..];
        let tag_end = after.find('>').unwrap_or(after.len());
        let tag = &rest[tag_start..name_at + tag_end];
        if tag.contains(&content_attr) {
            return true;
        }
        rest = &rest[name_at + name_attr.len()..];
    }
    false
}

/// Failures if this string is not a Seal Atlas quality report.
pub fn validate_quality_html(html: &str) -> Vec<String> {
    let lowered = html.to_ascii_lowercase();
    let mut failures = Vec::new();
    if !meta_content_is(html, "engine-contract", ENGINE_CONTRACT_ID) {
        failures.push("missing meta engine-contract=the-engine/v1".into());
    }
    if !meta_content_is(html, "quality-report-surface", QUALITY_REPORT_SURFACE) {
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
    if let Some(wer) = wer_at
        && !footnote_at.map(|footnote| footnote < wer).unwrap_or(false)
    {
        failures.push("Avg WER may only appear after a footnote marker".into());
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
    /// Apple + Whisper + lexicon / Light+ finished L2 shaping for a
    /// time-bounded span. The record `[sample_start, sample_end)` becomes
    /// append-only and may schedule the existing Responses formatter. Order
    /// on the PCM axis is frozen.
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
    LexiconLightPlus,
    ResponsesFormatter,
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
    pub machine_layer_count: usize,
    pub bars: &'static [FinalityBar],
    pub forbidden: &'static [&'static str],
    pub whisper_window: &'static str,
    pub full_file_pass: &'static str,
    pub inline_format_role: &'static str,
    pub silero_role: &'static str,
    pub sideband_role: &'static str,
    pub sideband_labels: &'static [&'static str],
    pub sideband_absence: &'static str,
    pub final_bam_status: &'static str,
    pub session_finalised_role: &'static str,
    pub product_goal: &'static str,
}

/// The only contract instance quality reports may emit.
pub const ENGINE_CONTRACT: EngineContract = EngineContract {
    id: ENGINE_CONTRACT_ID,
    primary_key: "pcm_time",
    relay: &[
        RelayLayer::Apple,
        RelayLayer::Whisper,
        RelayLayer::LexiconLightPlus,
        RelayLayer::ResponsesFormatter,
        RelayLayer::Human,
    ],
    machine_layer_count: 4,
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
        "small_inline_llm",
        "infer_named_sound_from_silero",
        "final_bam_automatic_producer",
        "session_finalised_content_mutation",
    ],
    whisper_window: "approximately_4s_with_approximately_1s_overlap",
    full_file_pass: "button_only_proposal",
    inline_format_role: "schedule_existing_responses_formatter",
    silero_role: "orthogonal_vad_and_pcm_time_evidence",
    sideband_role: "content_free_exact_pcm_evidence_never_text_authority",
    sideband_labels: &["speech_start", "speech_end", "pause_unknown_non_speech"],
    sideband_absence: "fail_open_continuous_apple",
    final_bam_status: "superseded_no_automatic_producer",
    session_finalised_role: "lifecycle_only",
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
            "Apple + Whisper + lexicon / Light+ finished L2 shaping for the time-bounded span. Record [sample_start, sample_end) is append-only and may schedule the existing Responses formatter. Order on the PCM axis is frozen.",
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
<p class="engine-contract-relay">Four machine layers: L0 Apple → L1 Whisper → L2 lexicon / Light+ → L3 existing Responses formatter; then human. “Inline” is scheduling, not a separate model. Silero is orthogonal VAD and PCM-time evidence: exact speech edges plus pause=unknown_non_speech, never named-sound or text authority. Sideband absence fails open to continuous Apple. Final BAM is superseded and SessionFinalised is lifecycle-only.</p>
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
        assert_eq!(ENGINE_CONTRACT.machine_layer_count, 4);
        assert_eq!(ENGINE_CONTRACT.relay.len(), 5);
        assert_eq!(
            ENGINE_CONTRACT.relay,
            &[
                RelayLayer::Apple,
                RelayLayer::Whisper,
                RelayLayer::LexiconLightPlus,
                RelayLayer::ResponsesFormatter,
                RelayLayer::Human
            ]
        );
        assert_eq!(
            &ENGINE_CONTRACT.relay[..ENGINE_CONTRACT.machine_layer_count],
            &[
                RelayLayer::Apple,
                RelayLayer::Whisper,
                RelayLayer::LexiconLightPlus,
                RelayLayer::ResponsesFormatter,
            ]
        );
        assert_eq!(
            ENGINE_CONTRACT.relay[ENGINE_CONTRACT.machine_layer_count],
            RelayLayer::Human
        );
        assert_eq!(
            ENGINE_CONTRACT.inline_format_role,
            "schedule_existing_responses_formatter"
        );
        assert_eq!(
            ENGINE_CONTRACT.silero_role,
            "orthogonal_vad_and_pcm_time_evidence"
        );
        assert_eq!(
            ENGINE_CONTRACT.final_bam_status,
            "superseded_no_automatic_producer"
        );
        assert_eq!(ENGINE_CONTRACT.session_finalised_role, "lifecycle_only");
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
            "Apple → Whisper → Lexicon + Light+ → Responses formatter → human",
            "Exactly four machine layers",
            "L2 — Lexicon + Light+",
            "L3 — Responses formatter",
            "Inline describes scheduling",
            "Silero is orthogonal",
            "Sideband evidence contract",
            "unknown_non_speech",
            "infer_named_sound_from_silero",
            "L3 may consume only measured pause duration",
            "one continuous stream with no sideband events",
            "Final BAM is superseded",
            "SessionFinalised is lifecycle-only",
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
    fn normative_docs_name_four_machine_layers_and_historical_adrs_are_superseded() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
        for relative in [
            "docs/THE_ENGINE_CONTRACT.md",
            "docs/TRANSCRIPT_LANES.md",
            "docs/OVERLAY_STREAMING.md",
            "docs/ARCHITECTURE.md",
            "docs/WHISPER_LIVE.md",
        ] {
            let path = root.join(relative);
            let body = std::fs::read_to_string(&path)
                .unwrap_or_else(|err| panic!("{} must exist: {err}", path.display()));
            let lowered = body
                .to_ascii_lowercase()
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ");
            for needle in [
                "four machine layers",
                "light+",
                "responses",
                "formatter",
                "silero",
                "orthogonal",
                "final bam",
                "superseded",
                "sessionfinalised",
                "lifecycle",
            ] {
                assert!(
                    lowered.contains(needle),
                    "{relative} missing four-layer contract token {needle:?}"
                );
            }
            for rejected in [
                "small inline llm",
                "adopt a **five-layer",
                "final bam, when built",
            ] {
                assert!(
                    !lowered.contains(rejected),
                    "{relative} contains active superseded claim {rejected:?}"
                );
            }
        }

        for relative in [
            "docs/ADR/2026-05-26-LAYERED_INCREMENTAL_TRANSCRIPTION.md",
            "docs/ADR/2026-05-28-Correction-Continuous-Hands-Off.md",
        ] {
            let path = root.join(relative);
            let body = std::fs::read_to_string(&path)
                .unwrap_or_else(|err| panic!("{} must exist: {err}", path.display()));
            assert!(
                body.contains("Status: SUPERSEDED IN FULL"),
                "{relative} must be unmistakably historical"
            );
        }
    }

    #[test]
    fn full_file_pass_is_never_automatic() {
        assert_eq!(ENGINE_CONTRACT.full_file_pass, "button_only_proposal");
        assert!(ENGINE_CONTRACT.whisper_window.contains("4s"));
        assert!(ENGINE_CONTRACT.whisper_window.contains("1s_overlap"));
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
        assert!(
            failures.is_empty(),
            "gold take 01 failed handshake: {failures:?}"
        );
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
    fn handshake_rejects_prose_that_is_not_the_meta_content() {
        let fake = r#"<html><head>
<meta name="engine-contract" content="other/v0" />
<meta name="quality-report-surface" content="wer-table" />
<title>mentions the-engine/v1 and seal-atlas in prose</title>
</head>
<body>
<p>the-engine/v1 seal-atlas</p>
<div class="stat"><b>1</b><span>word-grain</span></div>
<p>utterance-grain clock-lie SealedSpan.words whisper</p>
</body></html>"#;
        let failures = validate_quality_html(fake);
        assert!(
            failures.iter().any(|f| f.contains("engine-contract")),
            "{failures:?}"
        );
        assert!(
            failures
                .iter()
                .any(|f| f.contains("quality-report-surface")),
            "{failures:?}"
        );
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

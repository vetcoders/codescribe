//! Transcript tagging helpers for paste delivery.

/// Default wrapper: an XML-ish element carrying `{mode}`, `{lang}`, `{text}`.
pub const DEFAULT_TRANSCRIPT_TAG_TEMPLATE: &str =
    "<codescribe mode=\"{mode}\" lang=\"{lang}\">\n{text}\n</codescribe>";

// Conservative calibration from observed Whisper logs: healthy sessions cluster
// around avg_logprob ≈ -0.2, suspicious/hallucinated ones around ≈ -1.4.
/// At or above this average logprob the transcript is labelled `high`.
const HIGH_CONFIDENCE_AVG_LOGPROB_MIN: f32 = -0.45;
/// At or below this average logprob the transcript is labelled `low`.
const LOW_CONFIDENCE_AVG_LOGPROB_MAX: f32 = -1.20;

/// Wrap a transcript in `template` without any quality annotation.
pub fn wrap_transcript(text: &str, template: &str, mode: &str, lang: &str) -> String {
    wrap_transcript_with_quality(text, template, mode, lang, None, &[] as &[&str])
}

/// Wrap a transcript, also substituting `{conf}` and `{flags}`.
///
/// Blank input stays blank — an empty transcript is never wrapped in a tag. A
/// template without `{text}` gets the body appended on its own line, so a
/// misconfigured template can drop the markup but never the user's words. On the
/// default template the output is byte-identical to [`wrap_transcript`], which
/// keeps the quality-aware path a drop-in replacement.
pub fn wrap_transcript_with_quality<F: std::fmt::Display>(
    text: &str,
    template: &str,
    mode: &str,
    lang: &str,
    avg_logprob: Option<f32>,
    confidence_flags: &[F],
) -> String {
    if text.trim().is_empty() {
        return String::new();
    }

    let flags = confidence_flags
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(",");
    let mut rendered = template
        .replace("{mode}", mode)
        .replace("{lang}", lang)
        .replace("{conf}", confidence_label(avg_logprob))
        .replace("{flags}", &flags);
    if rendered.contains("{text}") {
        return rendered.replace("{text}", text);
    }

    if !rendered.is_empty() && !rendered.ends_with('\n') {
        rendered.push('\n');
    }
    rendered.push_str(text);
    rendered
}

/// Bucket an average logprob into `high` / `medium` / `low` / `unknown`.
fn confidence_label(avg_logprob: Option<f32>) -> &'static str {
    match avg_logprob {
        Some(value) if value >= HIGH_CONFIDENCE_AVG_LOGPROB_MIN => "high",
        Some(value) if value <= LOW_CONFIDENCE_AVG_LOGPROB_MAX => "low",
        Some(_) => "medium",
        None => "unknown",
    }
}

/// Covers template substitution, the empty-input and missing-`{text}` edges,
/// byte-compatibility between the two entry points, and the confidence buckets.
#[cfg(test)]
mod tests {
    use super::{DEFAULT_TRANSCRIPT_TAG_TEMPLATE, wrap_transcript};

    /// Default XML-ish template substitutes mode/lang and wraps body text.
    #[test]
    fn wrap_transcript_uses_default_template() {
        let wrapped = wrap_transcript(
            "Ala ma kota.",
            DEFAULT_TRANSCRIPT_TAG_TEMPLATE,
            "dictation",
            "pl",
        );

        assert_eq!(
            wrapped,
            "<codescribe mode=\"dictation\" lang=\"pl\">\nAla ma kota.\n</codescribe>"
        );
    }

    /// Custom templates honor `{lang}`, `{mode}`, and `{text}` placeholders.
    #[test]
    fn wrap_transcript_uses_custom_template_and_placeholders() {
        let wrapped = wrap_transcript("hello", "[{lang}:{mode}] {text}", "format", "en");

        assert_eq!(wrapped, "[en:format] hello");
    }

    /// Whitespace-only input stays unwrapped (never emits an empty tag shell).
    #[test]
    fn wrap_transcript_leaves_empty_text_unwrapped() {
        assert_eq!(
            wrap_transcript(
                "   \n\t",
                DEFAULT_TRANSCRIPT_TAG_TEMPLATE,
                "dictation",
                "pl"
            ),
            ""
        );
    }

    /// Missing `{text}` still appends body on a new line (never drop words).
    #[test]
    fn wrap_transcript_appends_when_template_has_no_text_placeholder() {
        let wrapped = wrap_transcript("body", "<tag mode=\"{mode}\">", "dictation", "pl");

        assert_eq!(wrapped, "<tag mode=\"dictation\">\nbody");
    }

    /// Literal special chars and a `{text}` token in the body stay intact.
    #[test]
    fn wrap_transcript_preserves_special_characters() {
        let text = "5 < 7 & \"quotes\" {text}";
        let wrapped = wrap_transcript(text, DEFAULT_TRANSCRIPT_TAG_TEMPLATE, "dictation", "pl");

        assert!(wrapped.contains(text));
    }

    /// Quality path fills `{conf}`/`{flags}` from logprob and flag list.
    #[test]
    fn wrap_transcript_with_quality_renders_confidence_and_flags() {
        let wrapped = super::wrap_transcript_with_quality(
            "body",
            "<tag conf=\"{conf}\" flags=\"{flags}\">{text}</tag>",
            "dictation",
            "pl",
            Some(-1.42),
            &["possible_hallucination_logprob", "unverified_stream"],
        );

        assert_eq!(
            wrapped,
            "<tag conf=\"low\" flags=\"possible_hallucination_logprob,unverified_stream\">body</tag>"
        );
    }

    /// Missing logprob → `unknown`; empty flags render as an empty slot.
    #[test]
    fn wrap_transcript_with_quality_reports_unknown_confidence_and_empty_flags() {
        let wrapped = super::wrap_transcript_with_quality(
            "body",
            "[{conf}|{flags}] {text}",
            "dictation",
            "pl",
            None,
            &[] as &[&str],
        );

        assert_eq!(wrapped, "[unknown|] body");
    }

    /// Default template is byte-identical between plain and quality-aware wrap.
    #[test]
    fn wrap_transcript_old_template_is_byte_compatible_with_quality_helper() {
        let old = wrap_transcript("body", DEFAULT_TRANSCRIPT_TAG_TEMPLATE, "dictation", "pl");
        let new = super::wrap_transcript_with_quality(
            "body",
            DEFAULT_TRANSCRIPT_TAG_TEMPLATE,
            "dictation",
            "pl",
            Some(-0.2),
            &["unverified_stream"],
        );

        assert_eq!(new, old);
    }

    /// Confidence buckets: high ≥ -0.45, low ≤ -1.20, else medium; None unknown.
    #[test]
    fn confidence_thresholds_are_conservative() {
        assert_eq!(super::confidence_label(Some(-0.20)), "high");
        assert_eq!(super::confidence_label(Some(-0.90)), "medium");
        assert_eq!(super::confidence_label(Some(-1.40)), "low");
        assert_eq!(super::confidence_label(None), "unknown");
    }
}

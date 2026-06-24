//! Transcript tagging helpers for paste delivery.

pub const DEFAULT_TRANSCRIPT_TAG_TEMPLATE: &str =
    "<codescribe mode=\"{mode}\" lang=\"{lang}\">\n{text}\n</codescribe>";

pub fn wrap_transcript(text: &str, template: &str, mode: &str, lang: &str) -> String {
    if text.trim().is_empty() {
        return String::new();
    }

    let mut rendered = template.replace("{mode}", mode).replace("{lang}", lang);
    if rendered.contains("{text}") {
        return rendered.replace("{text}", text);
    }

    if !rendered.is_empty() && !rendered.ends_with('\n') {
        rendered.push('\n');
    }
    rendered.push_str(text);
    rendered
}

#[cfg(test)]
mod tests {
    use super::{DEFAULT_TRANSCRIPT_TAG_TEMPLATE, wrap_transcript};

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

    #[test]
    fn wrap_transcript_uses_custom_template_and_placeholders() {
        let wrapped = wrap_transcript("hello", "[{lang}:{mode}] {text}", "format", "en");

        assert_eq!(wrapped, "[en:format] hello");
    }

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

    #[test]
    fn wrap_transcript_appends_when_template_has_no_text_placeholder() {
        let wrapped = wrap_transcript("body", "<tag mode=\"{mode}\">", "dictation", "pl");

        assert_eq!(wrapped, "<tag mode=\"dictation\">\nbody");
    }

    #[test]
    fn wrap_transcript_preserves_special_characters() {
        let text = "5 < 7 & \"quotes\" {text}";
        let wrapped = wrap_transcript(text, DEFAULT_TRANSCRIPT_TAG_TEMPLATE, "dictation", "pl");

        assert!(wrapped.contains(text));
    }
}

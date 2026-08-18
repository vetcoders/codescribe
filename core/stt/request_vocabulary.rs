//! Explicit STT topic token. The client names the domain; audio never does.
//!
//! Codescribe takes send `programming`. Official OpenAI file audio does not
//! accept this field, so that host stays omitted. Missing field means no
//! dictionary bias. The client does not classify audio to pick a token.

use reqwest::Url;

/// Codescribe product domain for hosts that accept a topic token.
pub const CODESCRIBE_STT_VOCABULARY: &str = "programming";

/// Topic token to send on one outbound STT URL, if that host accepts one.
///
/// `None` for official OpenAI (unknown field) and unparseable URLs. Every
/// other Codescribe take is `programming`. Never inferred from audio.
pub fn codescribe_stt_vocabulary(endpoint: &str) -> Option<&'static str> {
    let host = Url::parse(endpoint).ok().and_then(|url| {
        url.host_str()
            .map(|host| host.trim_matches(['[', ']']).to_owned())
    })?;
    if host.eq_ignore_ascii_case("api.openai.com") {
        return None;
    }
    Some(CODESCRIBE_STT_VOCABULARY)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codescribe_sends_programming_except_official_openai() {
        assert_eq!(
            codescribe_stt_vocabulary("http://127.0.0.1:8444/v1/audio/transcriptions"),
            Some("programming")
        );
        assert_eq!(
            codescribe_stt_vocabulary("http://127.0.0.1:8088/v1/audio/transcriptions"),
            Some("programming")
        );
        assert_eq!(
            codescribe_stt_vocabulary("ws://127.0.0.1:8446/v1/audio/transcribe"),
            Some("programming")
        );
        assert_eq!(
            codescribe_stt_vocabulary("https://api.libraxis.cloud/v1/audio/transcriptions"),
            Some("programming")
        );
        assert_eq!(
            codescribe_stt_vocabulary("wss://api.libraxis.cloud/v1/audio/transcribe"),
            Some("programming")
        );
        assert_eq!(
            codescribe_stt_vocabulary("https://stt.example.test/v1/audio/transcriptions"),
            Some("programming")
        );
        assert_eq!(
            codescribe_stt_vocabulary("https://api.openai.com/v1/audio/transcriptions"),
            None
        );
        assert_eq!(codescribe_stt_vocabulary("not a url"), None);
    }

    #[test]
    fn token_is_not_chosen_from_audio() {
        assert_eq!(CODESCRIBE_STT_VOCABULARY, "programming");
        assert_ne!(CODESCRIBE_STT_VOCABULARY, "veterinary");
        assert_ne!(CODESCRIBE_STT_VOCABULARY, "off");
    }
}

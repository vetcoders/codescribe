//! Lightweight word tokenization for Teacher diffs.

/// A display token with original surface form.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Token {
    /// The word exactly as it appeared, punctuation and casing intact — what
    /// a diff shows the reader.
    pub surface: String,
    /// The matching key: lowercased and stripped of edge punctuation, so
    /// "Codescribe." and "codescribe" align.
    pub norm: String,
}

/// Split on whitespace; keep punctuation glued for now (product diffs care about
/// jargon tokens more than commas).
pub fn tokenize(text: &str) -> Vec<Token> {
    text.split_whitespace()
        .filter(|w| !w.is_empty())
        .map(|w| Token {
            surface: w.to_string(),
            norm: normalize_token(w),
        })
        .collect()
}

/// Lowercase + strip common trailing punctuation for matching.
pub fn normalize_token(token: &str) -> String {
    let trimmed = token.trim_matches(|c: char| {
        matches!(
            c,
            ',' | '.'
                | ';'
                | ':'
                | '!'
                | '?'
                | '"'
                | '\''
                | '('
                | ')'
                | '['
                | ']'
                | '…'
                | '—'
                | '-'
        )
    });
    trimmed.to_lowercase()
}

#[cfg(test)]
mod tok_tests {
    use super::*;

    #[test]
    fn normalize_strips_punct_and_case() {
        assert_eq!(normalize_token("Codescribe."), "codescribe");
        assert_eq!(normalize_token("Loctree,"), "loctree");
    }
}

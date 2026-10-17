//! Straight double quotes become typographic ones outside code.
//!
//! Founder, 2026-10-14: dictated text is pasted into terminals (`echo …`) and
//! agents' bus replies are copied the same way; a straight `"` inside such a
//! line breaks the command. The rule: inside backticks and fenced code the
//! quotes stay programmer's quotes; everywhere else they are writer's quotes.
//! Single quotes are left alone — an apostrophe in `take'u` or `it's` is not a
//! quote, and a pass that guesses would damage more than it repairs.

/// Which pair of writer's quotes a language uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuoteStyle {
    /// Polish: „low opening” and high closing.
    Polish,
    /// English and most of the rest: “high opening” and high closing.
    English,
}

impl QuoteStyle {
    /// The style for a BCP-47-ish language tag such as `pl`, `pl-PL`, `en`.
    /// `None` or an empty tag means "not configured": guess from the text.
    pub fn for_language(tag: Option<&str>) -> Option<Self> {
        let tag = tag?.trim();
        if tag.is_empty() || tag.eq_ignore_ascii_case("auto") {
            return None;
        }
        let primary = tag
            .split(['-', '_'])
            .next()
            .unwrap_or(tag)
            .to_ascii_lowercase();
        Some(match primary.as_str() {
            "pl" => Self::Polish,
            _ => Self::English,
        })
    }

    /// Polish when the text carries Polish diacritics, English otherwise.
    pub fn guess(text: &str) -> Self {
        if text.chars().any(|c| "ąćęłńóśźżĄĆĘŁŃÓŚŹŻ".contains(c)) {
            Self::Polish
        } else {
            Self::English
        }
    }

    fn pair(self) -> (char, char) {
        match self {
            Self::Polish => ('„', '”'),
            Self::English => ('“', '”'),
        }
    }
}

/// Replace paired straight double quotes outside code spans with the style's
/// writer's quotes. Code is a backtick span on one line or a fenced block
/// opened and closed by a line starting with three backticks. An unpaired
/// trailing `"` is left as it is: half a pair is not a quotation.
pub fn typographic_quotes(text: &str, style: QuoteStyle) -> String {
    let (open, close) = style.pair();
    let mut out = String::with_capacity(text.len() + 8);
    let mut in_fence = false;
    for (index, line) in text.split('\n').enumerate() {
        if index > 0 {
            out.push('\n');
        }
        if line.trim_start().starts_with("```") {
            in_fence = !in_fence;
            out.push_str(line);
            continue;
        }
        if in_fence {
            out.push_str(line);
            continue;
        }
        out.push_str(&convert_line(line, open, close));
    }
    out
}

fn convert_line(line: &str, open: char, close: char) -> String {
    // Pairs are decided per line so one straight quote never swallows the
    // rest of a paragraph; backtick spans are copied verbatim.
    let total = line
        .split('`')
        .step_by(2)
        .map(|prose| prose.matches('"').count())
        .sum::<usize>();
    let paired = total - (total % 2);
    let mut seen = 0usize;
    let mut out = String::with_capacity(line.len() + 4);
    let mut in_code = false;
    for segment in line.split('`') {
        if in_code {
            out.push('`');
            out.push_str(segment);
            out.push('`');
            in_code = false;
            continue;
        }
        for c in segment.chars() {
            if c == '"' && seen < paired {
                out.push(if seen.is_multiple_of(2) { open } else { close });
                seen += 1;
            } else {
                out.push(c);
            }
        }
        in_code = true;
    }
    // `split` yields a trailing empty prose segment after a closing backtick;
    // the loop above re-emitted every backtick pair, so nothing is lost. An
    // odd backtick count means the final "code" segment was prose: strip the
    // extra backtick that the pairing pushed.
    if line.matches('`').count() % 2 == 1 {
        out.pop();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn polish_pairs_outside_code_and_straight_inside() {
        let text = "Powiedz \"cześć\" i wpisz `echo \"ok\"` potem \"dalej\".";
        assert_eq!(
            typographic_quotes(text, QuoteStyle::Polish),
            "Powiedz „cześć” i wpisz `echo \"ok\"` potem „dalej”."
        );
    }

    #[test]
    fn english_style_and_language_tags() {
        assert_eq!(
            typographic_quotes("say \"hi\"", QuoteStyle::English),
            "say “hi”"
        );
        assert_eq!(
            QuoteStyle::for_language(Some("pl-PL")),
            Some(QuoteStyle::Polish)
        );
        assert_eq!(
            QuoteStyle::for_language(Some("en")),
            Some(QuoteStyle::English)
        );
        assert_eq!(QuoteStyle::for_language(Some("auto")), None);
        assert_eq!(QuoteStyle::for_language(None), None);
        assert_eq!(QuoteStyle::guess("to jest próba"), QuoteStyle::Polish);
        assert_eq!(QuoteStyle::guess("plain ascii"), QuoteStyle::English);
    }

    #[test]
    fn fenced_blocks_odd_quotes_and_apostrophes_are_untouched() {
        let text = "Tytuł \"A\"\n```\nprint(\"x\")\n```\nTo \"B\" i take'u \"C";
        assert_eq!(
            typographic_quotes(text, QuoteStyle::Polish),
            "Tytuł „A”\n```\nprint(\"x\")\n```\nTo „B” i take'u \"C"
        );
    }

    #[test]
    fn unbalanced_backtick_keeps_text_and_converts_prose() {
        assert_eq!(
            typographic_quotes("a \"b\" `c \"d\"", QuoteStyle::English),
            "a “b” `c \"d\""
        );
        assert_eq!(typographic_quotes("", QuoteStyle::Polish), "");
        assert_eq!(
            typographic_quotes("no quotes", QuoteStyle::Polish),
            "no quotes"
        );
    }
}

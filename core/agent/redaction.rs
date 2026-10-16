//! The one redaction authority for operator-visible agent text.
//!
//! Three surfaces show text derived from a tool call: the approval card's
//! command line, the card's argument snapshot, and the Tool Activity summary
//! of a finished call. They used to redact by three different rules with two
//! different placeholders, so a secret hidden on one surface reappeared on the
//! next — a `curl -H 'Authorization: Bearer …'` redacted in `preview.command`
//! came back verbatim in the argument snapshot.
//!
//! This module is the single throne for all three. It owns one placeholder,
//! one key predicate, one command-line token redactor, and the card snapshot
//! built on top of them. Callers redact through it or not at all.
//!
//! Two matchers live here because they answer two different questions:
//!
//! - [`contains_sensitive_key`] judges a *JSON key* — normalized to ASCII
//!   alphanumerics so `API_KEY`, `api-key` and `apiKey` collapse to one needle.
//!   It deliberately does not list bare `key` or `auth`: those would hide a
//!   whole tool-output summary that merely mentions an "author" or a
//!   "keyboard".
//! - [`redact_command_line`] judges *command-line tokens*, where a marker also
//!   governs the token(s) that follow it (`--token abc`), so the secret is not
//!   under a key at all.

use serde_json::Value;

/// Placeholder written in place of anything redacted, on every surface.
pub const REDACTED: &str = "[REDACTED]";

/// Longest single string value rendered in full on an approval card.
pub(crate) const MAX_PREVIEW_STRING_CHARS: usize = 200;

/// Upper bound on the whole JSON snapshot shown on an approval card.
pub(crate) const MAX_PREVIEW_ARGUMENTS_BYTES: usize = 4096;

/// Key needles whose value never reaches operator-visible text. Matched as a
/// substring of the normalized key, so a nested `headers.Authorization` and an
/// `X-Api-Key` are both covered. Over-matching (an innocent `monkeytoken`)
/// costs the operator nothing; under-matching leaks.
const SENSITIVE_KEYS: &[&str] = &[
    "token",
    "password",
    "passwd",
    "secret",
    "apikey",
    "accesskey",
    "privatekey",
    "authorization",
    "cookie",
    "credential",
    "bearer",
    "environment",
];

/// Whether a JSON key names something whose value must not be shown.
///
/// Non-alphanumerics are dropped and the rest lowercased before matching, so
/// `API_KEY`, `api-key` and `apiKey` all collapse to the same needle and
/// separator tricks cannot slip a secret past the filter.
pub fn contains_sensitive_key(key: &str) -> bool {
    let normalized = key
        .chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .collect::<String>()
        .to_ascii_lowercase();
    SENSITIVE_KEYS
        .iter()
        .any(|sensitive| normalized.contains(sensitive))
}

/// Redact secrets from one command line before it is shown to the operator.
///
/// A token can name a secret in four shapes, and only these: an assignment
/// (`TOKEN=…`, `--password=…`), a header with its value attached
/// (`X-Api-Key:…`), a header name whose value follows (`Authorization: Bearer
/// …` takes two tokens, every other header one), and an option name
/// (`--token …`, `--api-key …`), plus the bare `Bearer` scheme. The name is
/// judged by [`contains_sensitive_key`], so `X-Api-Key` and `api_key` match
/// like JSON keys do. An ordinary word that merely contains a marker
/// (`tokenizer.py`) names nothing, and a following option (`--delete-all`) is
/// never taken for a secret value, so the card cannot hide the action itself.
/// Whitespace is preserved exactly. Display only; what is dispatched upstream
/// is never touched.
pub fn redact_command_line(text: &str) -> String {
    CommandRedactor::new().redact(text)
}

/// A command-line redactor that carries its "redact the next N tokens" state
/// between calls.
///
/// An argument vector splits a command line across JSON array elements, so
/// `["--token", "abc"]` only redacts `abc` if the marker seen in one element
/// still governs the next. One redactor walked across the elements is what
/// makes the argv form as safe as the joined form.
#[derive(Debug, Default)]
pub struct CommandRedactor {
    /// How many further values the last marker still governs.
    pending: usize,
    /// The quote that opened a secret value still running across tokens;
    /// every token up to the one that closes it belongs to that value.
    quoted: Option<char>,
}

/// The quote a token opens without closing it, if any. A value such as
/// `'two words'` spans tokens, so the redactor must follow it to its end.
fn opening_quote(token: &str) -> Option<char> {
    let first = token.chars().next()?;
    if !matches!(first, '\'' | '"') {
        return None;
    }
    let closed = token.len() > first.len_utf8() && token.ends_with(first);
    (!closed).then_some(first)
}

impl CommandRedactor {
    /// A redactor with no marker in flight.
    pub fn new() -> Self {
        Self::default()
    }

    /// Forget any marker in flight.
    ///
    /// Used when an argument array stops being a run of command tokens — a
    /// non-string element means the next string is not this flag's value.
    pub fn reset(&mut self) {
        self.pending = 0;
        self.quoted = None;
    }

    /// Redact one chunk of command line, preserving its whitespace verbatim.
    pub fn redact(&mut self, text: &str) -> String {
        let mut out = String::with_capacity(text.len());
        let mut rest = text;
        while !rest.is_empty() {
            let gap = rest
                .find(|character: char| !character.is_whitespace())
                .unwrap_or(rest.len());
            out.push_str(&rest[..gap]);
            rest = &rest[gap..];
            if rest.is_empty() {
                break;
            }
            let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
            out.push_str(&self.redact_token(&rest[..end]));
            rest = &rest[end..];
        }
        out
    }

    /// Redact one argv element. An element is one argument, so a value the
    /// previous element marked as secret is hidden whole even when it holds
    /// spaces (`["--password", "two words"]`); otherwise the element is read
    /// as command text. Shell quoting does not cross argv elements.
    pub fn redact_argument(&mut self, element: &str) -> String {
        let value = element.trim_start();
        if self.pending > 0 && !value.is_empty() && !value.starts_with('-') {
            self.pending -= 1;
            return REDACTED.to_string();
        }
        let out = self.redact(element);
        self.quoted = None;
        out
    }

    /// Redact a single whitespace-free token and update the marker state.
    fn redact_token(&mut self, token: &str) -> String {
        if let Some(quote) = self.quoted {
            if token.ends_with(quote) {
                self.quoted = None;
                return format!("{REDACTED}{quote}");
            }
            return REDACTED.to_string();
        }
        let trimmed = token.trim_matches(|character| matches!(character, '\'' | '"'));
        if self.pending > 0 {
            if !trimmed.starts_with('-') {
                self.pending -= 1;
                if let Some(quote) = opening_quote(token) {
                    // A quoted value is one value, however many words it has.
                    self.pending = 0;
                    self.quoted = Some(quote);
                    return format!("{quote}{REDACTED}");
                }
                return REDACTED.to_string();
            }
            // An option is never a secret value: the card must show the action.
            self.pending = 0;
        }
        if let Some((name, value)) = trimmed.split_once('=')
            && contains_sensitive_key(name)
        {
            if let Some(quote) = opening_quote(value) {
                self.quoted = Some(quote);
            }
            return format!("{name}={REDACTED}");
        }
        if let Some((name, value)) = trimmed.split_once(':') {
            if !contains_sensitive_key(name) {
                return token.to_string();
            }
            // A header quoted as one argument (`'Cookie: a=1; b=2'`) ends at its
            // closing quote, so its whole value is hidden and nothing after it.
            if let Some(quote) = opening_quote(token) {
                self.quoted = Some(quote);
                return if value.is_empty() {
                    token.to_string()
                } else {
                    format!("{quote}{name}:{REDACTED}")
                };
            }
            if value.is_empty() {
                // `Authorization: Bearer <secret>` carries a scheme and a
                // credential; every other header one value.
                let scheme_and_value = name.to_ascii_lowercase().contains("authorization");
                self.pending = if scheme_and_value { 2 } else { 1 };
                return token.to_string();
            }
            // `Authorization:Bearer <secret>` keeps its credential in the
            // next token even when the scheme is glued to the header.
            if value.eq_ignore_ascii_case("bearer") || value.eq_ignore_ascii_case("basic") {
                self.pending = 1;
            }
            return format!("{name}:{REDACTED}");
        }
        let names_a_secret_option = trimmed.starts_with('-') && contains_sensitive_key(trimmed);
        let is_scheme =
            trimmed.eq_ignore_ascii_case("bearer") || trimmed.eq_ignore_ascii_case("basic");
        if names_a_secret_option || is_scheme {
            self.pending = 1;
        }
        token.to_string()
    }
}

/// Bounded, secret-redacted rendering of the exact input a pending call would
/// be dispatched with.
///
/// The one place a tool call's raw arguments become operator-visible text. It
/// reads the same immutable `input` the gate decided on and the session later
/// hands to the handler, so the card cannot describe a different call than the
/// one suspended. `None` when there are no arguments at all, so a no-argument
/// tool does not grow an empty block on its card.
pub fn redacted_arguments(input: &Value) -> Option<String> {
    match input {
        Value::Null => return None,
        Value::Object(fields) if fields.is_empty() => return None,
        _ => {}
    }
    let snapshot = serde_json::to_string_pretty(&redact_argument_value(input, false))
        .unwrap_or_else(|_| REDACTED.to_string());
    Some(bound_snapshot(snapshot))
}

/// Recursive redaction: a value under a sensitive key collapses to the
/// placeholder whole (including an object or array, which would otherwise hide
/// the secret one level down); every surviving string is command-redacted,
/// because a secret embedded in an ordinary string is under no key at all.
///
/// An array shares one [`CommandRedactor`] across its elements, so an argv
/// pair like `["--token", "abc"]` hides `abc`; a non-string element ends that
/// run.
fn redact_argument_value(value: &Value, under_sensitive_key: bool) -> Value {
    if under_sensitive_key {
        return Value::String(REDACTED.to_string());
    }
    match value {
        Value::Object(fields) => {
            let governed = names_a_sensitive_sibling(fields);
            let mut redacted = serde_json::Map::with_capacity(fields.len());
            for (key, value) in fields {
                let sensitive = contains_sensitive_key(key) || (governed && is_value_field(key));
                redacted.insert(key.clone(), redact_argument_value(value, sensitive));
            }
            Value::Object(redacted)
        }
        Value::Array(items) => Value::Array(redact_argument_array(items)),
        Value::String(text) => Value::String(bound_string(&redact_command_line(text))),
        other => other.clone(),
    }
}

/// Fields that name the setting an object carries (`{"key": "api_key", …}`,
/// `{"name": "Authorization", …}`), as opposed to the setting's own value.
const NAME_FIELDS: &[&str] = &[
    "key", "name", "header", "field", "variable", "env", "setting",
];

/// Fields that carry the value of the setting a sibling name field names.
const VALUE_FIELDS: &[&str] = &["value", "val", "values", "data", "content", "secret"];

/// Whether a key/value-shaped object names a secret in one of its name
/// fields, so that its value field must be hidden although the literal
/// property is only called `value`.
fn names_a_sensitive_sibling(fields: &serde_json::Map<String, Value>) -> bool {
    fields.iter().any(|(key, value)| {
        NAME_FIELDS.contains(&key.to_ascii_lowercase().as_str())
            && value.as_str().is_some_and(contains_sensitive_key)
    })
}

/// Whether a property is the value half of a key/value-shaped object.
fn is_value_field(key: &str) -> bool {
    VALUE_FIELDS.contains(&key.to_ascii_lowercase().as_str())
}

/// Redact an argument array, sharing one [`CommandRedactor`] across its string
/// elements.
///
/// An argv arrives split into elements, so a marker seen in one element must
/// still govern the next: `["--token", "abc"]` hides `abc`, and
/// `["-H", "Authorization: Bearer x"]` hides both following tokens. A
/// non-string element is not part of that run and clears the state.
fn redact_argument_array(items: &[Value]) -> Vec<Value> {
    let mut redactor = CommandRedactor::new();
    items
        .iter()
        .map(|item| match item {
            Value::String(text) => Value::String(bound_string(&redactor.redact_argument(text))),
            other => {
                redactor.reset();
                redact_argument_value(other, false)
            }
        })
        .collect()
}

/// Cut one string value to [`MAX_PREVIEW_STRING_CHARS`], saying how much was
/// left out rather than ending silently.
fn bound_string(text: &str) -> String {
    let total = text.chars().count();
    if total <= MAX_PREVIEW_STRING_CHARS {
        return text.to_string();
    }
    let kept: String = text.chars().take(MAX_PREVIEW_STRING_CHARS).collect();
    let dropped = total - MAX_PREVIEW_STRING_CHARS;
    format!("{kept}… [{dropped} more characters]")
}

/// Cut the rendered snapshot to [`MAX_PREVIEW_ARGUMENTS_BYTES`] on a character
/// boundary, with an explicit marker so a truncated card is never mistaken for
/// the whole call.
fn bound_snapshot(snapshot: String) -> String {
    if snapshot.len() <= MAX_PREVIEW_ARGUMENTS_BYTES {
        return snapshot;
    }
    let mut cut = MAX_PREVIEW_ARGUMENTS_BYTES;
    while cut > 0 && !snapshot.is_char_boundary(cut) {
        cut -= 1;
    }
    let dropped = snapshot.len() - cut;
    format!("{}\n… [truncated, {dropped} more bytes]", &snapshot[..cut])
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{contains_sensitive_key, redact_command_line, redacted_arguments};

    /// PR #173 review: a header credential and a flag value are hidden, and the
    /// operator still reads the command's real shape (spacing and newlines).
    #[test]
    fn command_line_redaction_preserves_whitespace_and_hides_header_credentials() {
        assert_eq!(
            redact_command_line("curl  -H 'Authorization: Bearer hunter2'\n  --token abc"),
            "curl  -H 'Authorization: [REDACTED] [REDACTED]'\n  --token [REDACTED]"
        );
        assert_eq!(
            redact_command_line("TOKEN=hunter2 curl --password swordfish example.test"),
            "TOKEN=[REDACTED] curl --password [REDACTED] example.test"
        );
    }

    /// PR #173 review (P1): the exact Desktop Commander leak — a secret inside
    /// an ordinary `command` string never reaches the argument snapshot.
    #[test]
    fn a_secret_inside_an_ordinary_string_is_redacted() {
        let snapshot = redacted_arguments(&json!({
            "command": "curl -H 'Authorization: Bearer live-key' https://example.test",
            "cwd": "/work",
        }))
        .expect("snapshot");
        assert!(!snapshot.contains("live-key"), "{snapshot}");
        assert!(snapshot.contains("/work") && snapshot.contains("https://example.test"));
    }

    /// An argv split across array elements is redacted as one command line.
    #[test]
    fn argv_elements_share_one_redactor() {
        let snapshot = redacted_arguments(&json!({"args": ["--token", "abc", "--verbose"]}))
            .expect("snapshot");
        assert!(!snapshot.contains("abc"), "{snapshot}");
        assert!(snapshot.contains("--verbose"));

        let header = redacted_arguments(&json!({"args": ["-H", "Authorization: Bearer live-key"]}))
            .expect("snapshot");
        assert!(!header.contains("live-key"), "{header}");
    }

    /// A non-string element ends the argv run; an empty string does not spend
    /// the pending redaction.
    #[test]
    fn argv_state_resets_on_non_strings_and_survives_empty_elements() {
        let reset = redacted_arguments(&json!({"args": ["--token", 7, "abc"]})).expect("snapshot");
        assert!(reset.contains("\"abc\""), "{reset}");
        let empty = redacted_arguments(&json!({"args": ["--token", "", "abc"]})).expect("snapshot");
        assert!(!empty.contains("abc"), "{empty}");
    }

    /// PR #173 review round 2: header names match like JSON keys, in both the
    /// spaced and the attached form, and a glued scheme still hides the
    /// credential that follows.
    #[test]
    fn header_names_match_like_json_keys() {
        assert_eq!(
            redact_command_line("curl -H 'X-Api-Key: live-key' https://example.test"),
            "curl -H 'X-Api-Key: [REDACTED]' https://example.test"
        );
        assert_eq!(
            redact_command_line("curl -H X-Api-Key:live-key"),
            "curl -H X-Api-Key:[REDACTED]"
        );
        assert_eq!(
            redact_command_line("curl -H Authorization:Bearer live-key"),
            "curl -H Authorization:[REDACTED] [REDACTED]"
        );
    }

    /// PR #173 review round 2: an ordinary word containing a marker names no
    /// secret, and an option is never taken for a secret value, so the card
    /// always shows the action the call would take.
    #[test]
    fn markers_never_hide_the_action() {
        assert_eq!(
            redact_command_line("python tokenizer.py --delete-all"),
            "python tokenizer.py --delete-all"
        );
        assert_eq!(
            redact_command_line("tool --tokenizer --delete-all --password hunter2"),
            "tool --tokenizer --delete-all --password [REDACTED]"
        );
        let snapshot =
            redacted_arguments(&json!({"args": ["--token", "--force", "push"]})).expect("snapshot");
        assert!(
            snapshot.contains("--force") && snapshot.contains("push"),
            "{snapshot}"
        );
    }

    /// PR #173 review round 3: a secret value is hidden whole, whether it is a
    /// quoted command-line value with spaces or one argv element with spaces,
    /// and nothing after it is taken for part of it.
    #[test]
    fn a_spaced_secret_is_hidden_whole_and_nothing_after_it() {
        assert_eq!(
            redact_command_line("mysql --password 'two words' -h db"),
            "mysql --password '[REDACTED] [REDACTED]' -h db"
        );
        let snapshot =
            redacted_arguments(&json!({"args": ["--password", "two words", "-h", "db"]}))
                .expect("snapshot");
        assert!(
            !snapshot.contains("two") && !snapshot.contains("words"),
            "{snapshot}"
        );
        assert!(
            snapshot.contains("\"-h\"") && snapshot.contains("\"db\""),
            "{snapshot}"
        );
    }

    /// PR #173 review round 3: a Cookie header hides its value and keeps the
    /// destination visible, quoted or as its own argv element.
    #[test]
    fn a_cookie_header_never_hides_the_destination() {
        assert_eq!(
            redact_command_line("curl -H 'Cookie: session=abc' https://example.test"),
            "curl -H 'Cookie: [REDACTED]' https://example.test"
        );
        assert_eq!(
            redact_command_line("curl -H 'Cookie: a=1; b=2' https://example.test"),
            "curl -H 'Cookie: [REDACTED] [REDACTED]' https://example.test"
        );
        let snapshot = redacted_arguments(
            &json!({"args": ["-H", "Cookie: session=abc", "https://example.test"]}),
        )
        .expect("snapshot");
        assert!(!snapshot.contains("session=abc"), "{snapshot}");
        assert!(snapshot.contains("https://example.test"), "{snapshot}");
    }

    /// PR #173 review round 4: a key/value-shaped object hides the value when
    /// a sibling name field names a secret, and keeps an ordinary setting.
    #[test]
    fn a_value_named_by_a_sibling_key_is_redacted() {
        let config =
            redacted_arguments(&json!({"key": "api_key", "value": "live-key"})).expect("snapshot");
        assert!(
            !config.contains("live-key") && config.contains("api_key"),
            "{config}"
        );
        let env = redacted_arguments(&json!({"env": [
            {"name": "GITHUB_TOKEN", "value": "ghp-live"},
            {"name": "Authorization", "value": "Bearer live"},
            {"name": "LANG", "value": "pl_PL.UTF-8"},
        ]}))
        .expect("snapshot");
        assert!(
            !env.contains("ghp-live") && !env.contains("Bearer live"),
            "{env}"
        );
        assert!(env.contains("pl_PL.UTF-8"), "{env}");
        let theme =
            redacted_arguments(&json!({"key": "theme", "value": "dark"})).expect("snapshot");
        assert!(theme.contains("dark"), "{theme}");
    }

    /// One key predicate for every surface: separator spellings collapse, and
    /// innocent words that merely contain "auth" or "key" stay visible.
    #[test]
    fn sensitive_keys_normalize_and_do_not_hide_innocent_words() {
        for key in [
            "X-Api-Key",
            "access_key_id",
            "privateKey",
            "Set-Cookie",
            "client_secret",
        ] {
            assert!(contains_sensitive_key(key), "{key} must be sensitive");
        }
        for key in ["author", "keyboard", "path", "cwd"] {
            assert!(!contains_sensitive_key(key), "{key} must stay visible");
        }
    }
}

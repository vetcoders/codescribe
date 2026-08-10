//! Streaming tag parser for demuxed LLM output.
//!
//! MVP scope:
//! - Tags: <speak> ... </speak>, <tool name="..."> ... </tool>
//! - Flat (no nesting)
//! - Partial chunks (SSE) supported

/// Maximum buffer size before forced flush (防DoS - prevents unbounded memory growth)
const MAX_BUFFER_SIZE: usize = 64 * 1024; // 64KB

/// Which recognized tag a span of the stream belongs to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TagKind {
    /// `<speak>` — spoken output, chunked for TTS.
    Speak,
    /// `<tool name="...">` — a tool invocation with a JSON argument body.
    Tool,
    /// Anything else; treated as plain text rather than a tag.
    Unknown,
}

/// One parsed unit of demuxed LLM output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DemuxEvent {
    /// A speakable chunk, already cut on a sentence or word boundary.
    Speak(String),
    /// A complete tool call. Only emitted once the closing tag arrived.
    Tool {
        /// Value of the `name` attribute, empty when the tag omitted it.
        name: String,
        /// Raw tag body, passed through verbatim (typically JSON).
        args: String,
    },
    /// Text outside any recognized tag, including malformed tags.
    Text(String),
    /// A tag is open but not yet closed — signals the stream is mid-tag.
    Partial(TagKind),
}

/// Incremental tag parser over an SSE-style chunked LLM stream.
///
/// Feed chunks as they arrive; the parser retains partial state across calls and
/// emits events only once they are complete. Speak content is additionally cut
/// into chunks between `speak_min_chars` and `speak_max_chars`.
#[derive(Debug, Clone)]
pub struct StreamingTagParser {
    buffer: String,
    state: ParserState,
    speak_min_chars: usize,
    speak_max_chars: usize,
}

/// Position of the parser inside the tag grammar.
#[derive(Debug, Clone)]
enum ParserState {
    /// Outside any tag; bytes flow through as text until a `<` appears.
    Text,
    /// Saw `<` but not yet the matching `>`.
    TagOpen,
    /// Inside a recognized tag, accumulating its body until the closing tag.
    TagContent {
        /// Which tag family this body belongs to.
        kind: TagKind,
        /// Bare tag name, used to build the closing tag.
        tag: String,
        /// The literal opening tag, replayed verbatim if the tag never closes.
        open_tag: String,
        /// `name` attribute for tool tags.
        tool_name: Option<String>,
        /// Char offset in `content` already emitted as speak chunks.
        last_emit: usize,
        /// Body accumulated so far across chunks.
        content: String,
    },
}

/// Parsed shape of an opening tag literal.
#[derive(Debug)]
struct TagInfo {
    /// Recognized family for the tag name.
    kind: TagKind,
    /// Bare tag name as written.
    tag: String,
    /// `name` attribute value for tool tags.
    tool_name: Option<String>,
}

impl StreamingTagParser {
    /// Parser with the default speak chunk window (40..160 chars).
    pub fn new() -> Self {
        Self {
            buffer: String::new(),
            state: ParserState::Text,
            speak_min_chars: 40,
            speak_max_chars: 160,
        }
    }

    /// Parser with a custom speak chunk window. Both bounds are clamped to at
    /// least 1 so chunking can never stall on a zero-width window.
    pub fn with_speak_chunking(min_chars: usize, max_chars: usize) -> Self {
        Self {
            buffer: String::new(),
            state: ParserState::Text,
            speak_min_chars: min_chars.max(1),
            speak_max_chars: max_chars.max(1),
        }
    }

    /// Feed a new chunk and return parsed events.
    pub fn feed(&mut self, chunk: &str) -> Vec<DemuxEvent> {
        self.buffer.push_str(chunk);
        let mut events = Vec::new();

        // DoS protection: force flush if buffer exceeds limit
        if self.buffer.len() > MAX_BUFFER_SIZE {
            tracing::warn!(
                "Demux buffer exceeded {}KB, forcing flush",
                MAX_BUFFER_SIZE / 1024
            );
            events.extend(self.flush());
            return events;
        }
        let speak_min = self.speak_min_chars;
        let speak_max = self.speak_max_chars;

        loop {
            match &mut self.state {
                ParserState::Text => {
                    if let Some(pos) = self.buffer.find('<') {
                        if pos > 0 {
                            events.push(DemuxEvent::Text(self.buffer[..pos].to_string()));
                        }
                        self.buffer = self.buffer[pos..].to_string();
                        self.state = ParserState::TagOpen;
                        continue;
                    }

                    if !self.buffer.is_empty() {
                        events.push(DemuxEvent::Text(std::mem::take(&mut self.buffer)));
                    }
                    break;
                }
                ParserState::TagOpen => {
                    if let Some(end) = self.buffer.find('>') {
                        let tag_literal = self.buffer[..=end].to_string();
                        self.buffer = self.buffer[end + 1..].to_string();

                        if let Some(info) = parse_open_tag(&tag_literal)
                            && info.kind != TagKind::Unknown
                        {
                            self.state = ParserState::TagContent {
                                kind: info.kind,
                                tag: info.tag,
                                open_tag: tag_literal,
                                tool_name: info.tool_name,
                                last_emit: 0,
                                content: String::new(),
                            };
                            continue;
                        }

                        // Unknown or malformed tag: treat as text.
                        events.push(DemuxEvent::Text(tag_literal));
                        self.state = ParserState::Text;
                        continue;
                    }

                    // Partial tag (no closing '>').
                    let kind = partial_kind(&self.buffer);
                    events.push(DemuxEvent::Partial(kind));
                    break;
                }
                ParserState::TagContent {
                    kind,
                    tag,
                    open_tag,
                    tool_name,
                    last_emit,
                    content,
                } => {
                    let close_tag = format!("</{}>", tag);
                    if let Some(pos) = self.buffer.find(&close_tag) {
                        content.push_str(&self.buffer[..pos]);
                        self.buffer = self.buffer[pos + close_tag.len()..].to_string();

                        match kind {
                            TagKind::Speak => {
                                emit_speak_chunks(
                                    speak_min,
                                    speak_max,
                                    content,
                                    last_emit,
                                    &mut events,
                                );
                                let remainder = content.get(*last_emit..).unwrap_or("");
                                let remainder = remainder.trim_start();
                                if !remainder.is_empty() {
                                    events.push(DemuxEvent::Speak(remainder.to_string()));
                                }
                                content.clear();
                                *last_emit = 0;
                            }
                            TagKind::Tool => {
                                events.push(DemuxEvent::Tool {
                                    name: tool_name.clone().unwrap_or_default(),
                                    args: std::mem::take(content),
                                });
                            }
                            TagKind::Unknown => {
                                events.push(DemuxEvent::Text(format!(
                                    "{}{}{}",
                                    open_tag,
                                    std::mem::take(content),
                                    close_tag
                                )));
                            }
                        }

                        self.state = ParserState::Text;
                        continue;
                    }

                    // Tag not closed yet: buffer content.
                    content.push_str(&self.buffer);
                    self.buffer.clear();
                    if *kind == TagKind::Speak {
                        emit_speak_chunks(speak_min, speak_max, content, last_emit, &mut events);
                    }
                    events.push(DemuxEvent::Partial(kind.clone()));
                    break;
                }
            }
        }

        events
    }

    /// Flush any buffered content (EOF/timeout).
    pub fn flush(&mut self) -> Vec<DemuxEvent> {
        let mut events = Vec::new();
        let speak_min = self.speak_min_chars;
        let speak_max = self.speak_max_chars;

        match &mut self.state {
            ParserState::Text => {
                if !self.buffer.is_empty() {
                    events.push(DemuxEvent::Text(std::mem::take(&mut self.buffer)));
                }
            }
            ParserState::TagOpen => {
                if !self.buffer.is_empty() {
                    events.push(DemuxEvent::Text(std::mem::take(&mut self.buffer)));
                }
                self.state = ParserState::Text;
            }
            ParserState::TagContent {
                kind,
                open_tag,
                content,
                last_emit,
                ..
            } => {
                let trailing = std::mem::take(&mut self.buffer);
                content.push_str(&trailing);

                match kind {
                    TagKind::Speak => {
                        emit_speak_chunks(speak_min, speak_max, content, last_emit, &mut events);
                        let remainder = content.get(*last_emit..).unwrap_or("");
                        let remainder = remainder.trim_start();
                        if !remainder.is_empty() {
                            events.push(DemuxEvent::Speak(remainder.to_string()));
                        }
                        content.clear();
                        *last_emit = 0;
                    }
                    TagKind::Tool => {
                        // Incomplete tool tag: do not execute. Preserve as text.
                        let args = std::mem::take(content);
                        events.push(DemuxEvent::Text(format!("{}{}", open_tag, args)));
                    }
                    TagKind::Unknown => {
                        events.push(DemuxEvent::Text(format!(
                            "{}{}",
                            open_tag,
                            std::mem::take(content)
                        )));
                    }
                }
                self.state = ParserState::Text;
            }
        }

        events
    }
}

impl Default for StreamingTagParser {
    /// Same empty state as [`StreamingTagParser::new`].
    fn default() -> Self {
        Self::new()
    }
}

/// Drain `content` into speak chunks, advancing `last_emit` past everything
/// emitted. Stops as soon as no further boundary fits the window, leaving the
/// remainder buffered for the next chunk.
fn emit_speak_chunks(
    min_chars: usize,
    max_chars: usize,
    content: &str,
    last_emit: &mut usize,
    events: &mut Vec<DemuxEvent>,
) {
    if min_chars == 0 || max_chars == 0 {
        return;
    }
    loop {
        let boundary = find_speak_boundary(content, *last_emit, min_chars, max_chars);
        let Some(boundary) = boundary else {
            break;
        };

        if boundary <= *last_emit {
            break;
        }

        let chunk = content.get(*last_emit..boundary).unwrap_or("").trim_start();
        if !chunk.is_empty() {
            events.push(DemuxEvent::Speak(chunk.to_string()));
        }
        *last_emit = boundary;
    }
}

/// Parse an opening tag literal such as `<tool name="x">`.
///
/// Returns `None` for closing tags and malformed input, so the caller can fall
/// back to treating the literal as plain text.
fn parse_open_tag(tag: &str) -> Option<TagInfo> {
    if !tag.starts_with('<') || !tag.ends_with('>') {
        return None;
    }
    let inner = &tag[1..tag.len() - 1];
    let inner = inner.trim();
    if inner.starts_with('/') {
        return None;
    }

    let mut parts = inner.splitn(2, char::is_whitespace);
    let name = parts.next()?.trim();
    if name.is_empty() {
        return None;
    }
    let attrs = parts.next().unwrap_or("").trim();

    let kind = match name {
        "speak" => TagKind::Speak,
        "tool" => TagKind::Tool,
        _ => TagKind::Unknown,
    };

    let tool_name = if kind == TagKind::Tool {
        find_attr_value(attrs, "name")
    } else {
        None
    };

    Some(TagInfo {
        kind,
        tag: name.to_string(),
        tool_name,
    })
}

/// Guess which tag a still-incomplete `<...` prefix is turning into, so the
/// consumer learns what is coming before the tag closes.
fn partial_kind(buf: &str) -> TagKind {
    let s = buf.trim_start_matches('<').trim_start_matches('/');
    if "speak".starts_with(s) || s.starts_with("speak") {
        return TagKind::Speak;
    }
    if "tool".starts_with(s) || s.starts_with("tool") {
        return TagKind::Tool;
    }
    TagKind::Unknown
}

/// Read a quoted attribute value out of a tag's attribute string.
///
/// Hand-rolled rather than regex-based: it skips malformed pairs instead of
/// failing the whole tag, matching the parser's treat-garbage-as-text stance.
fn find_attr_value(attrs: &str, key: &str) -> Option<String> {
    let bytes = attrs.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        let start = i;
        while i < bytes.len() && !bytes[i].is_ascii_whitespace() && bytes[i] != b'=' {
            i += 1;
        }
        if start == i {
            break;
        }
        let attr_key = &attrs[start..i];
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        if i >= bytes.len() || bytes[i] != b'=' {
            continue;
        }
        i += 1;
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        if i >= bytes.len() {
            break;
        }
        let quote = bytes[i];
        if quote != b'"' && quote != b'\'' {
            continue;
        }
        i += 1;
        let val_start = i;
        while i < bytes.len() && bytes[i] != quote {
            i += 1;
        }
        if i >= bytes.len() {
            break;
        }
        let val = &attrs[val_start..i];
        i += 1;
        if attr_key == key {
            return Some(val.to_string());
        }
    }
    None
}

/// Find where to cut the next speak chunk, starting at `start`.
///
/// Preference order: sentence punctuation once `min_chars` is reached, then the
/// last word boundary before `max_chars`, then a hard cut at `max_chars`.
/// Returns `None` when not enough content has arrived to justify a cut.
fn find_speak_boundary(
    content: &str,
    start: usize,
    min_chars: usize,
    max_chars: usize,
) -> Option<usize> {
    if start >= content.len() {
        return None;
    }
    let mut count = 0usize;
    let mut last_punct: Option<usize> = None;
    let mut last_space: Option<usize> = None;
    let mut idx_at_max: Option<usize> = None;

    for (i, ch) in content[start..].char_indices() {
        count += 1;
        let byte_idx = start + i;
        if matches!(ch, '.' | '!' | '?' | ';' | ':' | '\n') && count >= min_chars {
            last_punct = Some(byte_idx + ch.len_utf8());
        }
        if ch.is_whitespace() {
            last_space = Some(byte_idx + ch.len_utf8());
        }
        if count >= max_chars {
            idx_at_max = Some(byte_idx + ch.len_utf8());
            break;
        }
    }

    if let Some(punct) = last_punct {
        return Some(punct);
    }
    if let Some(max_idx) = idx_at_max {
        if let Some(space) = last_space.filter(|s| *s > start) {
            return Some(space);
        }
        return Some(max_idx);
    }
    None
}

/// Streaming tag parser coverage: speak, tool, partial tags, and flush.
#[cfg(test)]
mod tests {
    use super::*;

    /// A complete `<speak>` chunk emits one Speak event with the body.
    #[test]
    fn parses_speak_single_chunk() {
        let mut p = StreamingTagParser::new();
        let ev = p.feed("<speak>hi</speak>");
        assert_eq!(ev, vec![DemuxEvent::Speak("hi".to_string())]);
    }

    /// `<tool name=…>` captures name + JSON args as a Tool event.
    #[test]
    fn parses_tool_with_name() {
        let mut p = StreamingTagParser::new();
        let ev = p.feed(r#"<tool name="weather">{"q":1}</tool>"#);
        assert_eq!(
            ev,
            vec![DemuxEvent::Tool {
                name: "weather".to_string(),
                args: r#"{"q":1}"#.to_string()
            }]
        );
    }

    /// Plain text and tags interleave without losing surrounding Text spans.
    #[test]
    fn parses_text_and_tags() {
        let mut p = StreamingTagParser::new();
        let ev = p.feed("hello <speak>hi</speak> world");
        assert_eq!(
            ev,
            vec![
                DemuxEvent::Text("hello ".to_string()),
                DemuxEvent::Speak("hi".to_string()),
                DemuxEvent::Text(" world".to_string())
            ]
        );
    }

    /// Tag name split across feeds surfaces Partial then completes as Speak.
    #[test]
    fn handles_partial_tag() {
        let mut p = StreamingTagParser::new();
        let ev1 = p.feed("<spe");
        assert_eq!(ev1, vec![DemuxEvent::Partial(TagKind::Speak)]);
        let ev2 = p.feed("ak>hi</speak>");
        assert_eq!(ev2, vec![DemuxEvent::Speak("hi".to_string())]);
    }

    /// `flush` emits buffered open-speak content when the stream ends mid-tag.
    #[test]
    fn flushes_open_speak() {
        let mut p = StreamingTagParser::new();
        let _ = p.feed("<speak>hello");
        let ev = p.flush();
        assert_eq!(ev, vec![DemuxEvent::Speak("hello".to_string())]);
    }
}

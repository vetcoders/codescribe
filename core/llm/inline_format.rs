//! Inline-format buffer (W13-1, "Backspace Magic").
//!
//! Formats dictated text with the formatting LLM **while dictation is still
//! running**, one sealed utterance chunk at a time, chained through the
//! Responses API `previous_response_id` so consecutive chunks keep style and
//! context without resending the transcript. At stop, the already-formatted
//! prefix is composed with a single final request that formats **only the
//! unformatted tail** and closes the text coherently — instead of paying the
//! measured 8.6–13.8 s full-text format on the stop path.
//!
//! Doctrine constraints carried here:
//! - **Feature-flagged, default OFF** (`CODESCRIBE_INLINE_FORMAT=1` to arm).
//! - **Fail-open per chunk**: an LLM error/timeout keeps the raw chunk text and
//!   logs a receipt; the session is never blocked.
//! - **Anti-invention guard**: a formatted chunk whose word-set materially
//!   exceeds its input is rejected (raw kept + receipt). The formatter may
//!   punctuate and case, never add words — a formatter that invents text was
//!   observed live on 2026-08-12/13.
//! - **Seal = "format now" signal** (wave atlas amendment 2): sealed utterances
//!   are byte-stable, so they are the natural chunk boundary; the chunk store
//!   is keyed by the sealed span id.
//!
//! Receipts are stable INFO log lines (`inline_format_chunk`,
//! `inline_format_compose`, `inline_format_fallback`) following the
//! `stop_path_budget` convention.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use tokio::sync::{mpsc, oneshot};
use tracing::{info, warn};

use super::ai_formatting::{self, AiFormatResult, AiFormatStatus};

/// Master switch. Unset/anything-else = OFF; the operator flips it (⛔).
pub const INLINE_FORMAT_ENV: &str = "CODESCRIBE_INLINE_FORMAT";

/// Per-chunk LLM budget; a chunk that misses it keeps its raw text.
const DEFAULT_CHUNK_TIMEOUT_MS: u64 = 10_000;
/// Stop-path wait for the worker to drain queued chunks before composing.
const DEFAULT_FLUSH_TIMEOUT_MS: u64 = 2_500;
/// Stop-path budget for the single tail-close request.
const DEFAULT_TAIL_TIMEOUT_MS: u64 = 15_000;
/// Chunks shorter than this are recorded raw without an LLM round-trip.
const MIN_CHUNK_CHARS: usize = 8;
/// Hard cap on chunks per session (runaway guard).
const MAX_CHUNKS_PER_SESSION: usize = 240;

/// System prompt for a mid-dictation chunk. New prompt on purpose — the
/// final-pass formatter prompt is out of scope for this lane.
const INLINE_CHUNK_PROMPT: &str = "You format live dictation transcripts. Each user message is the next \
consecutive chunk of one ongoing dictation session. Format ONLY the current \
chunk: fix punctuation, capitalization, spacing, and obvious dictation \
artifacts. Keep every word — never add, remove, translate, reorder, or invent \
words. Never repeat or rewrite earlier chunks. Never answer questions or add \
commentary. Keep the language of the input. Return only the formatted chunk.";

/// System prompt for the stop-path tail: same contract plus the coherent close.
const INLINE_CLOSE_PROMPT: &str = "You format live dictation transcripts. This is the FINAL chunk of the \
dictation session. Format it exactly like the previous chunks: fix \
punctuation, capitalization, spacing, and obvious dictation artifacts. Keep \
every word — never add, remove, translate, reorder, or invent words. Close \
the text coherently: the last sentence must end with proper terminal \
punctuation. Never repeat earlier chunks. Return only the formatted final \
chunk.";

/// Whether the inline-format buffer is armed for this process.
pub fn enabled() -> bool {
    std::env::var(INLINE_FORMAT_ENV)
        .map(|v| matches!(v.trim().to_ascii_lowercase().as_str(), "1" | "true" | "on"))
        .unwrap_or(false)
}

fn env_ms(key: &str, default_ms: u64) -> Duration {
    Duration::from_millis(
        std::env::var(key)
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(default_ms),
    )
}

fn chunk_timeout() -> Duration {
    env_ms(
        "CODESCRIBE_INLINE_FORMAT_CHUNK_TIMEOUT_MS",
        DEFAULT_CHUNK_TIMEOUT_MS,
    )
}

fn flush_timeout() -> Duration {
    env_ms(
        "CODESCRIBE_INLINE_FORMAT_FLUSH_TIMEOUT_MS",
        DEFAULT_FLUSH_TIMEOUT_MS,
    )
}

fn tail_timeout() -> Duration {
    env_ms(
        "CODESCRIBE_INLINE_FORMAT_TAIL_TIMEOUT_MS",
        DEFAULT_TAIL_TIMEOUT_MS,
    )
}

// ── Session store ───────────────────────────────────────────────────────────

/// How one chunk's in-flight format attempt ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ChunkStatus {
    /// Enqueued or mid-request; composes as raw until resolved.
    Pending,
    /// LLM output accepted (guard passed, lexicon re-applied).
    Applied,
    /// LLM error or timeout — raw kept (fail-open).
    Failed,
    /// Guard rejected invented/dropped words — raw kept.
    RejectedInvention,
    /// Below the char floor — never sent.
    Skipped,
}

impl ChunkStatus {
    fn label(self) -> &'static str {
        match self {
            ChunkStatus::Pending => "pending",
            ChunkStatus::Applied => "applied",
            ChunkStatus::Failed => "failed",
            ChunkStatus::RejectedInvention => "rejected_invention",
            ChunkStatus::Skipped => "skipped",
        }
    }
}

/// One sealed-span chunk and its formatting outcome, keyed by the span id.
#[derive(Debug, Clone)]
pub(crate) struct ChunkRecord {
    /// Sealed span / utterance id (identity within the session).
    pub id: u64,
    /// Sealed text exactly as fed (post-lexicon, byte-stable).
    pub raw: String,
    /// Accepted formatted text; `None` composes as raw.
    pub formatted: Option<String>,
    pub status: ChunkStatus,
}

impl ChunkRecord {
    fn display_text(&self) -> &str {
        self.formatted.as_deref().unwrap_or(&self.raw)
    }
}

#[derive(Debug, Default, Clone)]
struct SessionStore {
    generation: u64,
    language: Option<String>,
    chunks: Vec<ChunkRecord>,
    /// Responses chain id of the last accepted chunk; resets per session.
    chain: Option<String>,
}

static STORE: OnceLock<Arc<Mutex<SessionStore>>> = OnceLock::new();
static GENERATION: AtomicU64 = AtomicU64::new(0);
static SENDER: OnceLock<mpsc::UnboundedSender<Cmd>> = OnceLock::new();

fn store() -> &'static Arc<Mutex<SessionStore>> {
    STORE.get_or_init(|| Arc::new(Mutex::new(SessionStore::default())))
}

enum Cmd {
    Begin {
        generation: u64,
        language: Option<String>,
    },
    Chunk {
        generation: u64,
        id: u64,
        text: String,
    },
    Flush {
        ack: oneshot::Sender<()>,
    },
}

// ── Live-session hooks ──────────────────────────────────────────────────────

/// Arm the buffer for a new live session. Must run inside a tokio runtime
/// (spawns the sequential worker on first use); resets chunks and the chain.
/// No-op when the feature flag is off.
pub fn begin_session(language: Option<&str>) {
    if !enabled() {
        return;
    }
    let Ok(handle) = tokio::runtime::Handle::try_current() else {
        warn!("inline_format_fallback reason=no_tokio_runtime (begin_session outside runtime)");
        return;
    };
    let tx = SENDER.get_or_init(|| {
        let (tx, rx) = mpsc::unbounded_channel();
        let shared = Arc::clone(store());
        handle.spawn(worker_loop(rx, shared));
        tx
    });
    let generation = GENERATION.fetch_add(1, Ordering::SeqCst) + 1;
    if tx
        .send(Cmd::Begin {
            generation,
            language: language.map(str::to_string),
        })
        .is_err()
    {
        warn!("inline_format_fallback reason=worker_gone (begin_session send failed)");
    }
}

/// Feed one sealed span. Sync + non-blocking (safe from the blocking seal
/// worker thread). No-op when disabled or when no session was begun.
pub fn on_chunk_sealed(id: u64, text: &str) {
    if !enabled() {
        return;
    }
    let Some(tx) = SENDER.get() else {
        return;
    };
    let generation = GENERATION.load(Ordering::SeqCst);
    if generation == 0 {
        return;
    }
    let _ = tx.send(Cmd::Chunk {
        generation,
        id,
        text: text.to_string(),
    });
}

// ── Worker ──────────────────────────────────────────────────────────────────

async fn worker_loop(mut rx: mpsc::UnboundedReceiver<Cmd>, shared: Arc<Mutex<SessionStore>>) {
    while let Some(cmd) = rx.recv().await {
        match cmd {
            Cmd::Begin {
                generation,
                language,
            } => {
                if let Ok(mut s) = shared.lock() {
                    *s = SessionStore {
                        generation,
                        language,
                        chunks: Vec::new(),
                        chain: None,
                    };
                }
                info!("inline_format_session_begin generation={generation}");
            }
            Cmd::Chunk {
                generation,
                id,
                text,
            } => {
                process_chunk(&shared, generation, id, text).await;
            }
            Cmd::Flush { ack } => {
                let _ = ack.send(());
            }
        }
    }
}

async fn process_chunk(shared: &Arc<Mutex<SessionStore>>, generation: u64, id: u64, text: String) {
    let trimmed = text.trim().to_string();
    let (idx, language, chain) = {
        let Ok(mut s) = shared.lock() else {
            return;
        };
        if s.generation != generation
            || trimmed.is_empty()
            || s.chunks.len() >= MAX_CHUNKS_PER_SESSION
        {
            return;
        }
        let status = if trimmed.chars().count() < MIN_CHUNK_CHARS {
            ChunkStatus::Skipped
        } else {
            ChunkStatus::Pending
        };
        s.chunks.push(ChunkRecord {
            id,
            raw: trimmed.clone(),
            formatted: None,
            status,
        });
        if status == ChunkStatus::Skipped {
            return;
        }
        (s.chunks.len() - 1, s.language.clone(), s.chain.clone())
    };

    let chained = chain.is_some();
    let started = Instant::now();
    let outcome = tokio::time::timeout(
        chunk_timeout(),
        ai_formatting::format_inline_chunk(
            &trimmed,
            language.as_deref(),
            chain,
            INLINE_CHUNK_PROMPT,
        ),
    )
    .await;
    let latency_ms = started.elapsed().as_millis();

    let (status, formatted, response_id) = match outcome {
        Ok(Ok((raw_out, response_id))) => {
            let cleaned = crate::stream_postprocess::apply_lexicon(raw_out.trim());
            if invention_guard_rejects(&trimmed, &cleaned) {
                (ChunkStatus::RejectedInvention, None, None)
            } else {
                (ChunkStatus::Applied, Some(cleaned), response_id)
            }
        }
        Ok(Err(error)) => {
            warn!("inline format chunk request failed: {error:#}");
            (ChunkStatus::Failed, None, None)
        }
        Err(_) => (ChunkStatus::Failed, None, None),
    };

    let chars_in = trimmed.chars().count();
    let chars_out = formatted
        .as_deref()
        .map(|t| t.chars().count())
        .unwrap_or(chars_in);
    if let Ok(mut s) = shared.lock() {
        // The session may have been reset or consumed mid-request; only write
        // back into the record this request was created for.
        if s.generation == generation
            && let Some(record) = s.chunks.get_mut(idx)
            && record.id == id
        {
            record.status = status;
            record.formatted = formatted;
            if status == ChunkStatus::Applied
                && let Some(rid) = response_id.filter(|r| !r.is_empty())
            {
                s.chain = Some(rid);
            }
        }
    }
    info!(
        "inline_format_chunk id={id} status={} latency_ms={latency_ms} chained={chained} chars_in={chars_in} chars_out={chars_out}",
        status.label(),
    );
}

// ── Anti-invention guard ────────────────────────────────────────────────────

fn normalize_token(token: &str) -> String {
    token
        .chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

fn normalized_words(text: &str) -> Vec<String> {
    text.split_whitespace()
        .map(normalize_token)
        .filter(|w| !w.is_empty())
        .collect()
}

/// True when the formatted text materially exceeds (or guts) the input words.
///
/// The formatter's licence is punctuation/casing/spacing — so the normalized
/// word multiset must stay essentially the same. Budget: 2 novel words or 10%
/// of the input, whichever is larger (absorbs digit↔word style flips without
/// admitting invented sentences). Losing more than half the words is equally
/// rejected: a truncated chunk silently drops the user's speech.
pub(crate) fn invention_guard_rejects(raw: &str, formatted: &str) -> bool {
    let raw_words = normalized_words(raw);
    if raw_words.is_empty() {
        return false;
    }
    let formatted_words = normalized_words(formatted);
    if formatted_words.len() * 2 < raw_words.len() {
        return true;
    }
    let mut counts: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
    for w in &raw_words {
        *counts.entry(w.as_str()).or_default() += 1;
    }
    let mut novel = 0usize;
    for w in &formatted_words {
        match counts.get_mut(w.as_str()) {
            Some(c) if *c > 0 => *c -= 1,
            _ => novel += 1,
        }
    }
    let budget = (raw_words.len() / 10).max(2);
    novel > budget
}

// ── Stop-path composition ───────────────────────────────────────────────────

/// Outcome of matching the session's chunks against the delivered transcript.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PrefixMatchOutcome {
    /// Formatted (or raw, for failed chunks) prefix text, chunk-joined.
    pub formatted_prefix: String,
    /// Byte offset in the full text where the unmatched tail begins.
    pub tail_start_byte: usize,
    /// Chunks whose words matched the transcript prefix in order.
    pub chunks_matched: usize,
    /// Matched chunks that carry accepted LLM formatting.
    pub formatted_matched: usize,
}

/// Word starts (normalized token + byte offset) over the full text.
fn word_spans(text: &str) -> Vec<(String, usize)> {
    let mut spans = Vec::new();
    let mut word_start: Option<usize> = None;
    for (i, c) in text.char_indices() {
        if c.is_whitespace() {
            if let Some(start) = word_start.take() {
                let token = normalize_token(&text[start..i]);
                if !token.is_empty() {
                    spans.push((token, start));
                }
            }
        } else if word_start.is_none() {
            word_start = Some(i);
        }
    }
    if let Some(start) = word_start {
        let token = normalize_token(&text[start..]);
        if !token.is_empty() {
            spans.push((token, start));
        }
    }
    spans
}

/// Match chunks (in order) against the head of the delivered transcript.
///
/// Comparison is word-based and punctuation/case-insensitive, so lexicon or
/// Light+ drift at chunk boundaries does not break the match. The first chunk
/// that fails to match stops the walk — everything after it (gap-appends,
/// diverged text) becomes the tail and is formatted fresh at stop. This is the
/// fail-open posture: a mismatch costs latency, never words.
pub(crate) fn match_chunks_against_text(
    chunks: &[ChunkRecord],
    full_text: &str,
) -> PrefixMatchOutcome {
    let spans = word_spans(full_text);
    let mut cursor = 0usize;
    let mut chunks_matched = 0usize;
    let mut formatted_matched = 0usize;
    let mut prefix_parts: Vec<String> = Vec::new();

    for chunk in chunks {
        let chunk_words = normalized_words(&chunk.raw);
        if chunk_words.is_empty() {
            chunks_matched += 1;
            continue;
        }
        let end = cursor + chunk_words.len();
        if end > spans.len() {
            break;
        }
        let matches = spans[cursor..end]
            .iter()
            .zip(chunk_words.iter())
            .all(|((span_word, _), chunk_word)| span_word == chunk_word);
        if !matches {
            break;
        }
        cursor = end;
        chunks_matched += 1;
        if chunk.formatted.is_some() {
            formatted_matched += 1;
        }
        let display = chunk.display_text().trim();
        if !display.is_empty() {
            prefix_parts.push(display.to_string());
        }
    }

    let tail_start_byte = spans
        .get(cursor)
        .map(|(_, b)| *b)
        .unwrap_or(full_text.len());
    PrefixMatchOutcome {
        formatted_prefix: prefix_parts.join(" "),
        tail_start_byte,
        chunks_matched,
        formatted_matched,
    }
}

/// Snapshot a session's chunks and consume them (one stop = one consumption;
/// a later non-live recording can never reuse a stale buffer).
fn snapshot_and_consume(shared: &Arc<Mutex<SessionStore>>) -> SessionStore {
    let Ok(mut s) = shared.lock() else {
        return SessionStore::default();
    };
    let snapshot = s.clone();
    s.chunks.clear();
    s.chain = None;
    snapshot
}

/// Stop-path entry point: compose formatted chunks + freshly formatted tail,
/// falling back to the classic full-text format whenever the buffer cannot
/// prove it covers the transcript. Drop-in replacement for
/// [`ai_formatting::format_text_with_status`] on the formatting lanes.
pub async fn format_text_with_inline_buffer(text: &str, language: Option<&str>) -> AiFormatResult {
    if enabled()
        && let Some(tx) = SENDER.get()
    {
        let started = Instant::now();
        if let Some(result) = compose_and_close_with(tx, store(), text, language).await {
            info!(
                "inline_format_stop total_ms={} composed_chars={}",
                started.elapsed().as_millis(),
                result.text.chars().count()
            );
            return result;
        }
    }
    ai_formatting::format_text_with_status(text, language, false, None).await
}

/// Compose against an explicit worker channel + store. Split from the global
/// entry point so the delivery harness can drive a private worker without
/// touching (or being polluted by) process-global session state.
async fn compose_and_close_with(
    tx: &mpsc::UnboundedSender<Cmd>,
    shared: &Arc<Mutex<SessionStore>>,
    full_text: &str,
    language: Option<&str>,
) -> Option<AiFormatResult> {
    // Drain queued chunks so the freshest seal (often emitted during
    // recorder stop) is formatted before we snapshot. Bounded: a stuck
    // worker degrades to raw-tail composition, never to a blocked stop.
    let flush_started = Instant::now();
    let (ack_tx, ack_rx) = oneshot::channel();
    let flushed = if tx.send(Cmd::Flush { ack: ack_tx }).is_ok() {
        tokio::time::timeout(flush_timeout(), ack_rx).await.is_ok()
    } else {
        false
    };
    let flush_wait_ms = flush_started.elapsed().as_millis();

    let snapshot = snapshot_and_consume(shared);
    if snapshot.chunks.is_empty() {
        info!("inline_format_fallback reason=no_chunks flush_wait_ms={flush_wait_ms}");
        return None;
    }

    let matched = match_chunks_against_text(&snapshot.chunks, full_text);
    if matched.chunks_matched == 0 || matched.formatted_matched == 0 {
        info!(
            "inline_format_fallback reason=prefix_mismatch chunks={} matched={} formatted={} flush_wait_ms={flush_wait_ms}",
            snapshot.chunks.len(),
            matched.chunks_matched,
            matched.formatted_matched
        );
        return None;
    }

    let tail_raw = full_text[matched.tail_start_byte..].trim();
    let tail_chars = tail_raw.chars().count();
    let (tail_text, tail_status) = if tail_raw.is_empty() {
        (String::new(), "empty")
    } else {
        match tokio::time::timeout(
            tail_timeout(),
            ai_formatting::format_inline_chunk(
                tail_raw,
                language.or(snapshot.language.as_deref()),
                snapshot.chain.clone(),
                INLINE_CLOSE_PROMPT,
            ),
        )
        .await
        {
            Ok(Ok((raw_out, _response_id))) => {
                let cleaned = crate::stream_postprocess::apply_lexicon(raw_out.trim());
                if invention_guard_rejects(tail_raw, &cleaned) {
                    (tail_raw.to_string(), "rejected_invention")
                } else {
                    (cleaned, "applied")
                }
            }
            Ok(Err(error)) => {
                warn!("inline format tail request failed: {error:#}");
                (tail_raw.to_string(), "failed")
            }
            Err(_) => (tail_raw.to_string(), "timeout"),
        }
    };

    let mut composed = matched.formatted_prefix.clone();
    if !tail_text.is_empty() {
        if !composed.is_empty() {
            composed.push(' ');
        }
        composed.push_str(&tail_text);
    }
    if composed.trim().is_empty() {
        info!("inline_format_fallback reason=empty_composition flush_wait_ms={flush_wait_ms}");
        return None;
    }

    info!(
        "inline_format_compose chunks={} matched={} formatted={} tail_chars={tail_chars} tail_status={tail_status} flushed={flushed} flush_wait_ms={flush_wait_ms}",
        snapshot.chunks.len(),
        matched.chunks_matched,
        matched.formatted_matched,
    );

    Some(AiFormatResult {
        text: composed,
        reasoning_text: None,
        status: AiFormatStatus::Applied,
    })
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn record(id: u64, raw: &str, formatted: Option<&str>) -> ChunkRecord {
        ChunkRecord {
            id,
            raw: raw.to_string(),
            formatted: formatted.map(str::to_string),
            status: if formatted.is_some() {
                ChunkStatus::Applied
            } else {
                ChunkStatus::Failed
            },
        }
    }

    /// Punctuation and casing may change freely; the guard only counts words.
    #[test]
    fn guard_accepts_punctuation_and_casing_changes() {
        assert!(!invention_guard_rejects(
            "no dobra to jest test dyktowania w codescribe",
            "No dobra, to jest test dyktowania w Codescribe."
        ));
    }

    /// A formatter that answers instead of formatting is rejected.
    #[test]
    fn guard_rejects_invented_content() {
        assert!(invention_guard_rejects(
            "kup mleko i chleb",
            "Oczywiście! Oto sformatowana lista zakupów: kup mleko i chleb, a także masło."
        ));
    }

    /// A formatter that eats most of the chunk is rejected too.
    #[test]
    fn guard_rejects_heavy_truncation() {
        assert!(invention_guard_rejects(
            "pierwsza część zdania oraz druga część zdania oraz trzecia część zdania",
            "pierwsza część."
        ));
    }

    /// Small novel-word drift (within budget) is tolerated.
    #[test]
    fn guard_tolerates_tiny_drift() {
        assert!(!invention_guard_rejects(
            "spotkanie jutro o ósmej rano w klinice",
            "Spotkanie jutro o 8 rano w klinice."
        ));
    }

    /// Matched chunks compose the formatted prefix; the tail byte offset points
    /// at the first unmatched word — including multibyte Polish input.
    #[test]
    fn matcher_matches_prefix_and_finds_tail() {
        let chunks = vec![
            record(
                1,
                "pierwsze zdanie o żółwiu",
                Some("Pierwsze zdanie o żółwiu."),
            ),
            record(2, "drugie zdanie o jeżu", Some("Drugie zdanie o jeżu.")),
        ];
        let full = "pierwsze zdanie o żółwiu drugie zdanie o jeżu i ogon który został";
        let m = match_chunks_against_text(&chunks, full);
        assert_eq!(m.chunks_matched, 2);
        assert_eq!(m.formatted_matched, 2);
        assert_eq!(
            m.formatted_prefix,
            "Pierwsze zdanie o żółwiu. Drugie zdanie o jeżu."
        );
        assert_eq!(&full[m.tail_start_byte..], "i ogon który został");
    }

    /// Canvas drift (gap-append between chunks) stops the walk at the last
    /// provable chunk; the rest becomes tail. Words are never lost.
    #[test]
    fn matcher_partial_match_on_gap_append() {
        let chunks = vec![
            record(1, "pierwsze zdanie", Some("Pierwsze zdanie.")),
            record(2, "trzecie zdanie", Some("Trzecie zdanie.")),
        ];
        let full = "Pierwsze zdanie wstawka z gap append trzecie zdanie";
        let m = match_chunks_against_text(&chunks, full);
        assert_eq!(m.chunks_matched, 1);
        assert_eq!(m.formatted_prefix, "Pierwsze zdanie.");
        assert_eq!(
            &full[m.tail_start_byte..],
            "wstawka z gap append trzecie zdanie"
        );
    }

    /// A failed chunk (no formatted text) still matches and composes raw —
    /// fail-open never drops the chunk from the prefix.
    #[test]
    fn matcher_failed_chunk_composes_raw() {
        let chunks = vec![
            record(1, "pierwsze zdanie", Some("Pierwsze zdanie.")),
            record(2, "drugie zdanie", None),
        ];
        let full = "pierwsze zdanie drugie zdanie ogon";
        let m = match_chunks_against_text(&chunks, full);
        assert_eq!(m.chunks_matched, 2);
        assert_eq!(m.formatted_matched, 1);
        assert_eq!(m.formatted_prefix, "Pierwsze zdanie. drugie zdanie");
        assert_eq!(&full[m.tail_start_byte..], "ogon");
    }

    /// Zero matches → the caller must fall back to full-text formatting.
    #[test]
    fn matcher_no_match_yields_zero() {
        let chunks = vec![record(
            1,
            "zupełnie inny tekst",
            Some("Zupełnie inny tekst."),
        )];
        let full = "to nagranie nie ma nic wspólnego z buforem";
        let m = match_chunks_against_text(&chunks, full);
        assert_eq!(m.chunks_matched, 0);
        assert_eq!(m.tail_start_byte, 0);
    }

    /// Fully covered transcript → empty tail (stop pays zero LLM requests).
    #[test]
    fn matcher_full_coverage_leaves_empty_tail() {
        let chunks = vec![record(1, "całość wypowiedzi", Some("Całość wypowiedzi."))];
        let full = "całość wypowiedzi";
        let m = match_chunks_against_text(&chunks, full);
        assert_eq!(m.chunks_matched, 1);
        assert_eq!(full[m.tail_start_byte..].trim(), "");
    }

    /// Delivery-verifier seam harness: a private worker + mock Responses
    /// provider drive the full chunk→chain→compose path without process-global
    /// state, so parallel tests (or a concurrent live session) cannot pollute
    /// the measurement.
    mod seam {
        use super::super::*;
        use mockito::Matcher;
        use serde_json::json;
        use serial_test::serial;

        /// RAII env pin (mirrors `ai_formatting`'s test guard): captures the
        /// prior value so an operator dotenv leaves the env as it found it.
        struct EnvGuard {
            key: &'static str,
            prev: Option<String>,
        }

        impl EnvGuard {
            fn set(key: &'static str, value: &str) -> Self {
                let prev = std::env::var(key).ok();
                unsafe { std::env::set_var(key, value) };
                Self { key, prev }
            }

            fn remove(key: &'static str) -> Self {
                let prev = std::env::var(key).ok();
                unsafe { std::env::remove_var(key) };
                Self { key, prev }
            }
        }

        impl Drop for EnvGuard {
            fn drop(&mut self) {
                match self.prev.as_deref() {
                    Some(value) => unsafe { std::env::set_var(self.key, value) },
                    None => unsafe { std::env::remove_var(self.key) },
                }
            }
        }

        fn pin_formatting_lane(server_url: &str) -> Vec<EnvGuard> {
            vec![
                EnvGuard::set(
                    "LLM_FORMATTING_ENDPOINT",
                    &format!("{server_url}/v1/responses"),
                ),
                EnvGuard::set("LLM_FORMATTING_MODEL", "mock-nano"),
                EnvGuard::set("LLM_FORMATTING_API_KEY", "mock-key"),
                EnvGuard::remove("LLM_FORMATTING_TEMPERATURE"),
                EnvGuard::remove("LLM_TEMPERATURE"),
                // Pin the operation clocks to their defaults: the operator's
                // dotenv injects into every test process, and the settle
                // waiter's 30s backstop is calibrated against THESE numbers.
                EnvGuard::set("CODESCRIBE_INLINE_FORMAT_CHUNK_TIMEOUT_MS", "10000"),
                EnvGuard::set("CODESCRIBE_INLINE_FORMAT_FLUSH_TIMEOUT_MS", "2500"),
                EnvGuard::set("CODESCRIBE_INLINE_FORMAT_TAIL_TIMEOUT_MS", "15000"),
            ]
        }

        fn responses_body(id: &str, text: &str) -> String {
            json!({
                "id": id,
                "output": [{
                    "type": "message",
                    "content": [{"type": "output_text", "text": text}]
                }]
            })
            .to_string()
        }

        struct Harness {
            tx: mpsc::UnboundedSender<Cmd>,
            shared: Arc<Mutex<SessionStore>>,
        }

        fn spawn_private_worker(language: &str) -> Harness {
            let shared = Arc::new(Mutex::new(SessionStore::default()));
            let (tx, rx) = mpsc::unbounded_channel();
            let worker = tokio::spawn(worker_loop(rx, Arc::clone(&shared)));
            // Surface a silent worker panic instead of an opaque settle timeout.
            tokio::spawn(async move {
                if let Err(join_error) = worker.await {
                    eprintln!("inline-format test worker died: {join_error:?}");
                }
            });
            tx.send(Cmd::Begin {
                generation: 1,
                language: Some(language.to_string()),
            })
            .expect("worker alive");
            Harness { tx, shared }
        }

        // Backstop, not a claim: it must sit OUT OF REACH of the chunk
        // request's own 10s budget, or the two clocks race under machine load
        // (measured flake 2026-08-14: chunk still Pending at the waiter's
        // 10s while its own timeout was about to settle it).
        async fn wait_for_settled_chunks(shared: &Arc<Mutex<SessionStore>>, expected: usize) {
            let deadline = Instant::now() + Duration::from_secs(30);
            loop {
                {
                    let s = shared.lock().expect("store lock");
                    if s.chunks.len() == expected
                        && s.chunks.iter().all(|c| c.status != ChunkStatus::Pending)
                    {
                        return;
                    }
                }
                assert!(
                    Instant::now() < deadline,
                    "chunks did not settle in flight within 30s: {:?}",
                    shared.lock().expect("store lock").chunks
                );
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        }

        /// The stop path pays exactly ONE provider round-trip — the tail close
        /// — because both sealed chunks were formatted in flight and chained
        /// via `previous_response_id`. This is the W13-1 delivery seam: with a
        /// local mock provider the whole stop composition fits far inside the
        /// <3 s budget; real-network cost is the single ~1–2 s nano tail call.
        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        #[serial]
        async fn stop_seam_pays_only_the_tail_request() {
            let mut server = mockito::Server::new_async().await;
            let _env = pin_formatting_lane(&server.url());

            let chunk1 = server
                .mock("POST", "/v1/responses")
                .match_body(Matcher::AllOf(vec![
                    Matcher::Regex("consecutive chunk".into()),
                    Matcher::Regex("pierwsze zdanie o testowaniu bufora".into()),
                ]))
                .with_status(200)
                .with_header("content-type", "application/json")
                .with_body(responses_body(
                    "resp_1",
                    "Pierwsze zdanie o testowaniu bufora.",
                ))
                .expect(1)
                .create_async()
                .await;
            let chunk2 = server
                .mock("POST", "/v1/responses")
                .match_body(Matcher::AllOf(vec![
                    Matcher::Regex("drugie zdanie o zamykaniu wypowiedzi".into()),
                    Matcher::Regex(r#""previous_response_id":"resp_1""#.into()),
                ]))
                .with_status(200)
                .with_header("content-type", "application/json")
                .with_body(responses_body(
                    "resp_2",
                    "Drugie zdanie o zamykaniu wypowiedzi.",
                ))
                .expect(1)
                .create_async()
                .await;
            let tail = server
                .mock("POST", "/v1/responses")
                .match_body(Matcher::AllOf(vec![
                    Matcher::Regex("FINAL chunk".into()),
                    Matcher::Regex(r#""previous_response_id":"resp_2""#.into()),
                    Matcher::Regex("ogon który nie został".into()),
                ]))
                .with_status(200)
                .with_header("content-type", "application/json")
                .with_body(responses_body(
                    "resp_3",
                    "Ogon, który nie został zapieczętowany.",
                ))
                .expect(1)
                .create_async()
                .await;

            let h = spawn_private_worker("pl");
            h.tx.send(Cmd::Chunk {
                generation: 1,
                id: 1,
                text: "pierwsze zdanie o testowaniu bufora".into(),
            })
            .unwrap();
            h.tx.send(Cmd::Chunk {
                generation: 1,
                id: 2,
                text: "drugie zdanie o zamykaniu wypowiedzi".into(),
            })
            .unwrap();

            // Both chunks format DURING dictation (before any stop call).
            wait_for_settled_chunks(&h.shared, 2).await;
            {
                let s = h.shared.lock().unwrap();
                assert!(
                    s.chunks.iter().all(|c| c.status == ChunkStatus::Applied),
                    "both chunks must be accepted in flight: {:?}",
                    s.chunks
                );
                assert_eq!(
                    s.chain.as_deref(),
                    Some("resp_2"),
                    "chain must advance to the last accepted chunk"
                );
            }

            let full_text = "pierwsze zdanie o testowaniu bufora drugie zdanie o \
                             zamykaniu wypowiedzi ogon który nie został zapieczętowany";
            let started = Instant::now();
            let result = compose_and_close_with(&h.tx, &h.shared, full_text, Some("pl"))
                .await
                .expect("compose must succeed when chunks cover the prefix");
            let stop_secs = started.elapsed().as_secs_f64();

            assert_eq!(result.status, AiFormatStatus::Applied);
            assert_eq!(
                result.text,
                "Pierwsze zdanie o testowaniu bufora. Drugie zdanie o zamykaniu \
                 wypowiedzi. Ogon, który nie został zapieczętowany."
            );
            chunk1.assert_async().await;
            chunk2.assert_async().await;
            tail.assert_async().await;
            assert!(
                stop_secs < 3.0,
                "stop seam must fit the <3s budget with a local provider (measured {stop_secs:.3}s)"
            );
            // Emit the measured number so the report can quote it.
            eprintln!(
                "inline_format_stop_seam_secs={stop_secs:.3} chunks_in_flight=2 stop_requests=1"
            );
        }

        /// Fail-open per chunk: a provider 500 keeps the raw chunk text, the
        /// chain stays on the last accepted id, and stop still composes —
        /// the session is never blocked by a failed chunk.
        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        #[serial]
        async fn chunk_failure_is_fail_open_and_keeps_chain() {
            let mut server = mockito::Server::new_async().await;
            let _env = pin_formatting_lane(&server.url());

            let chunk1 = server
                .mock("POST", "/v1/responses")
                .match_body(Matcher::Regex("pierwszy kawalek dyktowania".into()))
                .with_status(200)
                .with_header("content-type", "application/json")
                .with_body(responses_body("resp_1", "Pierwszy kawalek dyktowania."))
                .expect(1)
                .create_async()
                .await;
            // One ordered regex over the CURRENT wire truth (5d62aacb): a
            // chained request re-carries the closing prompt as a leading
            // `developer` input item (instructions do NOT persist server-side
            // across previous_response_id), and `input` serializes before
            // `previous_response_id`. Proves this is the closing request AND
            // that the chain still points at the last ACCEPTED chunk.
            let tail = server
                .mock("POST", "/v1/responses")
                .match_body(Matcher::Regex(
                    r#""role":"developer"[\s\S]*FINAL chunk[\s\S]*"previous_response_id":"resp_1""#
                        .into(),
                ))
                .with_status(200)
                .with_header("content-type", "application/json")
                .with_body(responses_body("resp_9", "Ogon po awarii."))
                .expect(1)
                .create_async()
                .await;

            let h = spawn_private_worker("pl");
            h.tx.send(Cmd::Chunk {
                generation: 1,
                id: 1,
                text: "pierwszy kawalek dyktowania".into(),
            })
            .unwrap();
            wait_for_settled_chunks(&h.shared, 1).await;

            // Real transport failure for chunk 2: the formatting lane briefly
            // points at a closed port (connection refused — the same fail-open
            // arm a dead provider takes in production). The guard's captured
            // previous value restores the mock endpoint before the tail runs.
            {
                let _dead_lane =
                    EnvGuard::set("LLM_FORMATTING_ENDPOINT", "http://127.0.0.1:1/v1/responses");
                h.tx.send(Cmd::Chunk {
                    generation: 1,
                    id: 2,
                    text: "drugi kawalek ktory pada".into(),
                })
                .unwrap();
                wait_for_settled_chunks(&h.shared, 2).await;
            }
            {
                let s = h.shared.lock().unwrap();
                assert_eq!(s.chunks[0].status, ChunkStatus::Applied);
                assert_eq!(s.chunks[1].status, ChunkStatus::Failed);
                assert_eq!(s.chunks[1].formatted, None, "failed chunk keeps raw");
                assert_eq!(s.chain.as_deref(), Some("resp_1"));
            }

            let full_text = "pierwszy kawalek dyktowania drugi kawalek ktory pada ogon po awarii";
            let result = compose_and_close_with(&h.tx, &h.shared, full_text, Some("pl"))
                .await
                .expect("fail-open compose must still succeed");

            assert_eq!(
                result.text,
                "Pierwszy kawalek dyktowania. drugi kawalek ktory pada Ogon po awarii."
            );
            chunk1.assert_async().await;
            tail.assert_async().await;
        }

        /// An anti-invention violation from the provider is rejected: the raw
        /// chunk survives and the chain does not advance onto the poisoned id.
        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        #[serial]
        async fn invented_chunk_output_is_rejected_with_raw_kept() {
            let mut server = mockito::Server::new_async().await;
            let _env = pin_formatting_lane(&server.url());

            let chunk = server
                .mock("POST", "/v1/responses")
                .match_body(Matcher::Regex("kup mleko i chleb dla kliniki".into()))
                .with_status(200)
                .with_header("content-type", "application/json")
                .with_body(responses_body(
                    "resp_bad",
                    "Oczywiście! Oto lista: kup mleko i chleb dla kliniki, a także \
                     opatrunki, strzykawki i wszystko czego potrzebuje przychodnia.",
                ))
                .expect(1)
                .create_async()
                .await;

            let h = spawn_private_worker("pl");
            h.tx.send(Cmd::Chunk {
                generation: 1,
                id: 1,
                text: "kup mleko i chleb dla kliniki".into(),
            })
            .unwrap();
            wait_for_settled_chunks(&h.shared, 1).await;

            {
                let s = h.shared.lock().unwrap();
                assert_eq!(s.chunks[0].status, ChunkStatus::RejectedInvention);
                assert_eq!(s.chunks[0].formatted, None, "invented text must not land");
                assert_eq!(s.chain, None, "chain must not advance onto a rejected id");
            }
            chunk.assert_async().await;
        }
    }
}

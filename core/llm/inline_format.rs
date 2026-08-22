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
//! - **Symmetric content guards**: addition, semantic loss, and spoken-token
//!   reorder are independently rejected (raw kept + typed receipt).
//! - **Seal = "format now" signal** (wave atlas amendment 2): sealed utterances
//!   are byte-stable, so they are the natural chunk boundary; the chunk store
//!   is keyed by session/span/PCM identity.
//! - **Bounded admission**: capture uses `try_send`; overflow is ledgered as a
//!   raw L2 fallback and never backpressures the microphone or reducer.
//!
//! Receipts are stable INFO log lines (`inline_format_chunk`,
//! `inline_format_compose`, `inline_format_fallback`) following the
//! `stop_path_budget` convention.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use tokio::sync::{mpsc, oneshot};
use tracing::{info, warn};

use super::ai_formatting::{self, AiFormatResult, AiFormatStatus};
use crate::pipeline::contracts::{SidebandEvidence, SidebandEvidenceKind};

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
/// Sealed spans waiting for the single ordered Responses worker. Capture uses
/// `try_send`; a full queue records raw fallback and returns immediately.
const SEALED_SPAN_QUEUE_CAPACITY: usize = 32;

/// Span-local scheduling contract appended to the configured Formatting lane
/// prompt. The provider/model/credential/base prompt all remain the existing
/// lane; this suffix only fences one ordered span from rewriting its siblings.
const INLINE_CHUNK_PROMPT: &str = "This request is one consecutive stable span of the current live dictation. \
Format ONLY this span. Keep its spoken content and order: never add, omit, \
translate, move, repeat, answer, or comment. Return only this formatted span.";

/// Tail-close suffix over the same configured Formatting lane prompt.
const INLINE_CLOSE_PROMPT: &str = "This request is the FINAL residual stable span of the current live dictation. \
Format ONLY this tail, keep its spoken content and order, and close its final \
sentence with proper terminal punctuation. Never repeat earlier spans. Return \
only the formatted tail.";

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
    /// Guard rejected content added by the formatter — raw kept.
    RejectedAddition,
    /// Guard rejected semantic/content loss — raw kept.
    RejectedLoss,
    /// Guard rejected a change in spoken token order — raw kept.
    RejectedReorder,
    /// The bounded queue was full — raw kept without blocking capture.
    QueueOverflow,
    /// PCM/session identity was invalid or out of order — raw kept.
    RejectedIdentity,
    /// Below the char floor — never sent.
    Skipped,
}

impl ChunkStatus {
    fn label(self) -> &'static str {
        match self {
            ChunkStatus::Pending => "pending",
            ChunkStatus::Applied => "applied",
            ChunkStatus::Failed => "failed",
            ChunkStatus::RejectedAddition => "rejected_addition",
            ChunkStatus::RejectedLoss => "rejected_loss",
            ChunkStatus::RejectedReorder => "rejected_reorder",
            ChunkStatus::QueueOverflow => "queue_overflow",
            ChunkStatus::RejectedIdentity => "rejected_identity",
            ChunkStatus::Skipped => "skipped",
        }
    }
}

/// Typed identity carried from the stable Apple/L2 seal into L3. Text is
/// payload; session/span/sample identity is authority.
#[derive(Debug, Clone, PartialEq)]
pub struct StableFormatSpan {
    pub session_id: String,
    pub capture_epoch: u64,
    pub span_id: u64,
    pub sample_start: u64,
    pub sample_end: u64,
    pub text: String,
    /// Optional content-free timing context. L3 filters this to measured pause
    /// durations only; speech edges and non-speech semantics are not prompts.
    pub sideband: Vec<SidebandEvidence>,
}

/// One sealed-span chunk and its formatting outcome, keyed by the span id.
#[derive(Debug, Clone)]
pub(crate) struct ChunkRecord {
    /// Stable session/span/PCM identity from the L2 seal.
    pub identity: StableFormatSpan,
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

#[derive(Default, Clone)]
struct SessionStore {
    active: bool,
    generation: u64,
    session_id: String,
    language: Option<String>,
    /// Result ownership is the span id, never array position or text.
    chunks: BTreeMap<u64, ChunkRecord>,
    /// Arrival order is separately retained and must agree with PCM order.
    order: Vec<u64>,
    /// Responses chain id of the last accepted chunk; resets per session.
    chain: Option<String>,
    /// Existing Formatting lane pinned once at recording start.
    lane: Option<ai_formatting::InlineFormattingLane>,
    /// More than the bounded ledger can represent: close must return full L2.
    ledger_overflow: usize,
}

static STORE: OnceLock<Arc<Mutex<SessionStore>>> = OnceLock::new();
static GENERATION: AtomicU64 = AtomicU64::new(0);
static SENDER: OnceLock<mpsc::Sender<Cmd>> = OnceLock::new();

fn store() -> &'static Arc<Mutex<SessionStore>> {
    STORE.get_or_init(|| Arc::new(Mutex::new(SessionStore::default())))
}

enum Cmd {
    Chunk { generation: u64, span_id: u64 },
    Flush { ack: oneshot::Sender<()> },
}

// ── Live-session hooks ──────────────────────────────────────────────────────

/// Arm the buffer for a new live session. Must run inside a tokio runtime
/// (spawns the sequential worker on first use); resets chunks and the chain.
/// No-op when the feature flag is off.
pub fn begin_session(session_id: &str, language: Option<&str>) {
    if !enabled() {
        return;
    }
    let Ok(handle) = tokio::runtime::Handle::try_current() else {
        warn!("inline_format_fallback reason=no_tokio_runtime (begin_session outside runtime)");
        return;
    };
    SENDER.get_or_init(|| {
        let (tx, rx) = mpsc::channel(SEALED_SPAN_QUEUE_CAPACITY);
        let shared = Arc::clone(store());
        handle.spawn(worker_loop(rx, shared));
        tx
    });
    let generation = GENERATION.fetch_add(1, Ordering::SeqCst) + 1;
    let lane = match ai_formatting::resolve_inline_formatting_lane() {
        Ok(lane) => {
            info!(
                endpoint = lane.endpoint(),
                model = lane.model(),
                "inline formatting lane pinned"
            );
            Some(lane)
        }
        Err(error) => {
            warn!("inline_format_fallback reason=lane_unavailable error={error:#}");
            None
        }
    };
    if let Ok(mut s) = store().lock() {
        *s = SessionStore {
            active: true,
            generation,
            session_id: session_id.to_string(),
            language: language.map(str::to_string),
            chunks: BTreeMap::new(),
            order: Vec::new(),
            chain: None,
            lane,
            ledger_overflow: 0,
        };
    }
    info!(
        generation,
        session_id,
        queue_capacity = SEALED_SPAN_QUEUE_CAPACITY,
        "inline_format_session_begin"
    );
}

/// Feed one sealed span. Sync + non-blocking (safe from the blocking seal
/// worker thread). No-op when disabled or when no session was begun.
pub fn on_span_sealed(mut span: StableFormatSpan) {
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
    span.text = span.text.trim().to_string();
    let span_id = span.span_id;
    let should_queue = register_stable_span(store(), generation, span);
    if !should_queue {
        return;
    }
    match tx.try_send(Cmd::Chunk {
        generation,
        span_id,
    }) {
        Ok(()) => {
            info!(
                generation,
                span_id,
                status = "queued",
                "inline_format_chunk"
            );
        }
        Err(mpsc::error::TrySendError::Full(_)) => {
            settle_without_request(generation, span_id, ChunkStatus::QueueOverflow);
        }
        Err(mpsc::error::TrySendError::Closed(_)) => {
            settle_without_request(generation, span_id, ChunkStatus::Failed);
        }
    }
}

fn register_stable_span(
    shared: &Arc<Mutex<SessionStore>>,
    generation: u64,
    span: StableFormatSpan,
) -> bool {
    let span_id = span.span_id;
    let Ok(mut s) = shared.lock() else {
        return false;
    };
    if !s.active
        || s.generation != generation
        || s.session_id != span.session_id
        || span.text.is_empty()
        || s.chunks.contains_key(&span_id)
    {
        return false;
    }
    if s.chunks.len() >= MAX_CHUNKS_PER_SESSION {
        s.ledger_overflow = s.ledger_overflow.saturating_add(1);
        info!(
            generation,
            span_id, "inline_format_chunk status=ledger_overflow"
        );
        return false;
    }
    let identity_valid = span.sample_start < span.sample_end
        && s.order
            .last()
            .and_then(|id| s.chunks.get(id))
            .map(|previous| {
                previous.identity.capture_epoch < span.capture_epoch
                    || (previous.identity.capture_epoch == span.capture_epoch
                        && previous.identity.sample_end <= span.sample_start)
            })
            .unwrap_or(true);
    let status = if !identity_valid {
        ChunkStatus::RejectedIdentity
    } else if span.text.chars().count() < MIN_CHUNK_CHARS {
        ChunkStatus::Skipped
    } else if s.lane.is_none() {
        ChunkStatus::Failed
    } else {
        ChunkStatus::Pending
    };
    s.order.push(span_id);
    s.chunks.insert(
        span_id,
        ChunkRecord {
            raw: span.text.clone(),
            identity: span,
            formatted: None,
            status,
        },
    );
    if status != ChunkStatus::Pending {
        info!(
            generation,
            span_id,
            status = status.label(),
            "inline_format_chunk"
        );
    }
    status == ChunkStatus::Pending
}

fn settle_without_request(generation: u64, span_id: u64, status: ChunkStatus) {
    if let Ok(mut s) = store().lock()
        && s.generation == generation
        && let Some(record) = s.chunks.get_mut(&span_id)
        && record.status == ChunkStatus::Pending
    {
        record.status = status;
        info!(
            generation,
            span_id,
            status = status.label(),
            "inline_format_chunk"
        );
    }
}

// ── Worker ──────────────────────────────────────────────────────────────────

async fn worker_loop(mut rx: mpsc::Receiver<Cmd>, shared: Arc<Mutex<SessionStore>>) {
    while let Some(cmd) = rx.recv().await {
        match cmd {
            Cmd::Chunk {
                generation,
                span_id,
            } => {
                process_chunk(&shared, generation, span_id).await;
            }
            Cmd::Flush { ack } => {
                let _ = ack.send(());
            }
        }
    }
}

async fn process_chunk(shared: &Arc<Mutex<SessionStore>>, generation: u64, span_id: u64) {
    let (raw, language, chain, lane, timing_instruction) = {
        let Ok(s) = shared.lock() else {
            return;
        };
        if s.generation != generation || !s.active {
            return;
        }
        let Some(record) = s.chunks.get(&span_id) else {
            return;
        };
        if record.status != ChunkStatus::Pending {
            return;
        }
        let Some(lane) = s.lane.clone() else {
            return;
        };
        (
            record.raw.clone(),
            s.language.clone(),
            s.chain.clone(),
            lane,
            pause_timing_instruction(&record.identity.sideband),
        )
    };

    let chained = chain.is_some();
    let mut system_prompt = format!("{}\n\n{}", lane.system_prompt(), INLINE_CHUNK_PROMPT);
    if let Some(timing_instruction) = timing_instruction {
        system_prompt.push_str("\n\n");
        system_prompt.push_str(&timing_instruction);
    }
    let started = Instant::now();
    let outcome = tokio::time::timeout(
        chunk_timeout(),
        format_inline_with_chain_recovery(&raw, language.as_deref(), chain, &system_prompt, &lane),
    )
    .await;
    let latency_ms = started.elapsed().as_millis();

    let (status, formatted, response_id, guard, chain_reset) = match outcome {
        Ok(InlineAttempt {
            result: Ok((raw_out, response_id)),
            chain_reset,
        }) => {
            let cleaned = crate::stream_postprocess::apply_lexicon(raw_out.trim());
            let guard = validate_formatted_span(&raw, &cleaned);
            match guard.disposition {
                GuardDisposition::Accepted => (
                    ChunkStatus::Applied,
                    Some(cleaned),
                    response_id,
                    guard,
                    chain_reset,
                ),
                GuardDisposition::RejectedAddition => (
                    ChunkStatus::RejectedAddition,
                    None,
                    None,
                    guard,
                    chain_reset,
                ),
                GuardDisposition::RejectedLoss => {
                    (ChunkStatus::RejectedLoss, None, None, guard, chain_reset)
                }
                GuardDisposition::RejectedReorder => {
                    (ChunkStatus::RejectedReorder, None, None, guard, chain_reset)
                }
            }
        }
        Ok(InlineAttempt {
            result: Err(error),
            chain_reset,
        }) => {
            warn!("inline format chunk request failed: {error:#}");
            (
                ChunkStatus::Failed,
                None,
                None,
                GuardReceipt::default(),
                chain_reset,
            )
        }
        Err(_) => (
            ChunkStatus::Failed,
            None,
            None,
            GuardReceipt::default(),
            false,
        ),
    };

    let chars_in = raw.chars().count();
    let chars_out = formatted
        .as_deref()
        .map(|t| t.chars().count())
        .unwrap_or(chars_in);
    let mut response_advanced = false;
    if let Ok(mut s) = shared.lock() {
        // The session may have been reset or consumed mid-request; only write
        // back into the record this request was created for.
        if s.generation == generation
            && let Some(record) = s.chunks.get_mut(&span_id)
            && record.identity.span_id == span_id
        {
            record.status = status;
            record.formatted = formatted;
            if chain_reset {
                s.chain = None;
            }
            if status == ChunkStatus::Applied
                && let Some(rid) = response_id.filter(|r| !r.is_empty())
            {
                s.chain = Some(rid);
                response_advanced = true;
            }
        }
    }
    info!(
        generation,
        span_id,
        status = status.label(),
        latency_ms,
        chained,
        chars_in,
        chars_out,
        added_tokens = guard.added_tokens,
        omitted_tokens = guard.omitted_tokens,
        reordered = guard.reordered,
        response_advanced,
        "inline_format_chunk",
    );
}

/// Convert only measured pause duration into a tightly fenced L3 hint.
///
/// The hint is a developer instruction, never transcript payload. Speech-edge
/// probability and the unknown non-speech classification are deliberately not
/// promoted into words or named sounds.
fn pause_timing_instruction(sideband: &[SidebandEvidence]) -> Option<String> {
    let durations_ms = sideband
        .iter()
        .filter_map(|evidence| match evidence.evidence {
            SidebandEvidenceKind::Pause {
                duration_samples, ..
            } if evidence.sample_rate_hz > 0 => {
                Some(duration_samples.saturating_mul(1_000) / u64::from(evidence.sample_rate_hz))
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    if durations_ms.is_empty() {
        return None;
    }
    let durations = durations_ms
        .iter()
        .map(|duration| format!("{duration} ms"))
        .collect::<Vec<_>>()
        .join(", ");
    Some(format!(
        "Measured pause timing adjacent to this span: {durations}. Use this timing only to decide punctuation or paragraph boundaries. It is not word or sound-label evidence: never add transcript words or annotations from it."
    ))
}

/// Result of one chained request, including whether a stale provider chain was
/// discarded before one bounded unchained retry.
struct InlineAttempt {
    result: anyhow::Result<(String, Option<String>)>,
    chain_reset: bool,
}

/// Keep stale-chain recovery local to this dictation session. A provider 400
/// naming `previous_response_not_found` means only the session's response id is
/// poisoned; retry exactly once without it while the caller's timeout remains
/// the single overall budget.
async fn format_inline_with_chain_recovery(
    text: &str,
    language: Option<&str>,
    previous_response_id: Option<String>,
    system_prompt: &str,
    lane: &ai_formatting::InlineFormattingLane,
) -> InlineAttempt {
    let first = ai_formatting::format_inline_chunk(
        text,
        language,
        previous_response_id.clone(),
        system_prompt,
        lane,
    )
    .await;
    match first {
        Err(error)
            if previous_response_id.is_some() && ai_formatting::is_stale_chain_error(&error) =>
        {
            warn!("inline formatting chain stale; retrying this session turn unchained");
            InlineAttempt {
                result: ai_formatting::format_inline_chunk(
                    text,
                    language,
                    None,
                    system_prompt,
                    lane,
                )
                .await,
                chain_reset: true,
            }
        }
        result => InlineAttempt {
            result,
            chain_reset: false,
        },
    }
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum GuardDisposition {
    #[default]
    Accepted,
    RejectedAddition,
    RejectedLoss,
    RejectedReorder,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
struct GuardReceipt {
    disposition: GuardDisposition,
    added_tokens: usize,
    omitted_tokens: usize,
    reordered: bool,
}

/// Symmetric completeness guard for one typed span.
///
/// Addition, loss, and reorder are measured independently. A one-token budget
/// (or 10% for long spans) permits obvious spelling/number normalization; it
/// does not permit a new clause or a missing phrase. Reorder has no budget:
/// punctuation and paragraphing never require changing spoken token order.
fn validate_formatted_span(raw: &str, formatted: &str) -> GuardReceipt {
    let raw_words = normalized_words(raw);
    if raw_words.is_empty() {
        return GuardReceipt::default();
    }
    let formatted_words = normalized_words(formatted);
    let mut raw_counts: HashMap<&str, usize> = HashMap::new();
    for w in &raw_words {
        *raw_counts.entry(w.as_str()).or_default() += 1;
    }
    let mut output_counts: HashMap<&str, usize> = HashMap::new();
    for w in &formatted_words {
        *output_counts.entry(w.as_str()).or_default() += 1;
    }
    let added_tokens = output_counts
        .iter()
        .map(|(word, count)| count.saturating_sub(*raw_counts.get(word).unwrap_or(&0)))
        .sum();
    let omitted_tokens = raw_counts
        .iter()
        .map(|(word, count)| count.saturating_sub(*output_counts.get(word).unwrap_or(&0)))
        .sum();

    let mut raw_positions: HashMap<&str, VecDeque<usize>> = HashMap::new();
    for (position, word) in raw_words.iter().enumerate() {
        raw_positions
            .entry(word.as_str())
            .or_default()
            .push_back(position);
    }
    let mut previous_position = None;
    let mut reordered = false;
    for word in &formatted_words {
        let Some(position) = raw_positions
            .get_mut(word.as_str())
            .and_then(VecDeque::pop_front)
        else {
            continue;
        };
        if previous_position.is_some_and(|previous| position < previous) {
            reordered = true;
            break;
        }
        previous_position = Some(position);
    }

    let substitution_budget = (raw_words.len() / 10).max(1);
    let disposition = if added_tokens > 0 && omitted_tokens == 0 {
        GuardDisposition::RejectedAddition
    } else if omitted_tokens > 0 && added_tokens == 0 {
        GuardDisposition::RejectedLoss
    } else if added_tokens > substitution_budget {
        GuardDisposition::RejectedAddition
    } else if omitted_tokens > substitution_budget {
        GuardDisposition::RejectedLoss
    } else if reordered {
        GuardDisposition::RejectedReorder
    } else {
        GuardDisposition::Accepted
    };
    GuardReceipt {
        disposition,
        added_tokens,
        omitted_tokens,
        reordered,
    }
}

// ── Stop-path composition ───────────────────────────────────────────────────

/// Completeness proof for the typed L2 ledger against controller delivery.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LedgerValidation {
    /// Formatted (or raw, for failed chunks) prefix text, chunk-joined.
    pub formatted_prefix: String,
    /// Byte offset in the full text where the unmatched tail begins.
    pub tail_start_byte: usize,
    /// Typed spans proven present in the controller's L2 text, in PCM order.
    pub spans_validated: usize,
    /// Validated spans that carry accepted L3 formatting.
    pub formatted_validated: usize,
    /// True only when every ledger span was found in its typed PCM order.
    pub complete: bool,
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

/// Validate typed ledger order and coverage against the delivered L2 text.
///
/// Text comparison is only a completeness guard after result ownership has
/// already been established by `(session, span_id, PCM range)`. It never
/// chooses which result belongs to which span. A mismatch rejects composition
/// and returns the full L2 controller text unchanged.
pub(crate) fn validate_ledger_against_text(
    chunks: &[ChunkRecord],
    full_text: &str,
) -> LedgerValidation {
    let spans = word_spans(full_text);
    let mut cursor = 0usize;
    let mut spans_validated = 0usize;
    let mut formatted_validated = 0usize;
    let mut prefix_parts: Vec<String> = Vec::new();
    let mut previous_identity = None;
    let expected_session = chunks
        .first()
        .map(|chunk| chunk.identity.session_id.as_str());

    for chunk in chunks {
        let identity_is_ordered = chunk.identity.sample_start < chunk.identity.sample_end
            && chunk.identity.session_id.as_str() == expected_session.unwrap_or_default()
            && previous_identity.is_none_or(|(epoch, end)| {
                epoch < chunk.identity.capture_epoch
                    || (epoch == chunk.identity.capture_epoch && end <= chunk.identity.sample_start)
            });
        if !identity_is_ordered {
            return LedgerValidation {
                formatted_prefix: prefix_parts.join(" "),
                tail_start_byte: 0,
                spans_validated,
                formatted_validated,
                complete: false,
            };
        }
        previous_identity = Some((chunk.identity.capture_epoch, chunk.identity.sample_end));
        let chunk_words = normalized_words(&chunk.raw);
        if chunk_words.is_empty() {
            spans_validated += 1;
            continue;
        }
        let end = cursor + chunk_words.len();
        if end > spans.len() {
            return LedgerValidation {
                formatted_prefix: prefix_parts.join(" "),
                tail_start_byte: 0,
                spans_validated,
                formatted_validated,
                complete: false,
            };
        }
        let matches = spans[cursor..end]
            .iter()
            .zip(chunk_words.iter())
            .all(|((span_word, _), chunk_word)| span_word == chunk_word);
        if !matches {
            return LedgerValidation {
                formatted_prefix: prefix_parts.join(" "),
                tail_start_byte: 0,
                spans_validated,
                formatted_validated,
                complete: false,
            };
        }
        cursor = end;
        spans_validated += 1;
        if chunk.formatted.is_some() {
            formatted_validated += 1;
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
    LedgerValidation {
        formatted_prefix: prefix_parts.join(" "),
        tail_start_byte,
        spans_validated,
        formatted_validated,
        complete: spans_validated == chunks.len(),
    }
}

fn ordered_records(snapshot: &SessionStore) -> Option<Vec<ChunkRecord>> {
    snapshot
        .order
        .iter()
        .map(|span_id| snapshot.chunks.get(span_id).cloned())
        .collect()
}

/// Snapshot a session's chunks and consume them (one stop = one consumption;
/// a later non-live recording can never reuse a stale buffer).
fn snapshot_and_consume(shared: &Arc<Mutex<SessionStore>>) -> SessionStore {
    let Ok(mut s) = shared.lock() else {
        return SessionStore::default();
    };
    let snapshot = s.clone();
    s.active = false;
    s.generation = s.generation.wrapping_add(1);
    s.chunks.clear();
    s.order.clear();
    s.chain = None;
    s.lane = None;
    snapshot
}

fn raw_l2_result(text: &str, reason: &str, generation: u64) -> AiFormatResult {
    info!(generation, reason, "inline_format_fallback");
    AiFormatResult {
        text: text.to_string(),
        reasoning_text: None,
        status: AiFormatStatus::Failed,
    }
}

/// Stop-path entry point: compose accepted span-keyed results plus a freshly
/// formatted tail. An active live session that cannot prove L2 completeness
/// returns the controller's full L2 text unchanged; only calls outside an
/// active inline session use the existing one-shot formatter.
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
    tx: &mpsc::Sender<Cmd>,
    shared: &Arc<Mutex<SessionStore>>,
    full_text: &str,
    language: Option<&str>,
) -> Option<AiFormatResult> {
    // Drain queued chunks so the freshest seal (often emitted during
    // recorder stop) is formatted before we snapshot. Bounded: a stuck
    // worker degrades to raw-tail composition, never to a blocked stop.
    let flush_started = Instant::now();
    let (ack_tx, ack_rx) = oneshot::channel();
    let flush_budget = flush_timeout();
    let sent = tokio::time::timeout(flush_budget, tx.send(Cmd::Flush { ack: ack_tx }))
        .await
        .is_ok_and(|result| result.is_ok());
    let remaining = flush_budget.saturating_sub(flush_started.elapsed());
    let flushed =
        sent && !remaining.is_zero() && tokio::time::timeout(remaining, ack_rx).await.is_ok();
    let flush_wait_ms = flush_started.elapsed().as_millis();

    let snapshot = snapshot_and_consume(shared);
    if !snapshot.active {
        return None;
    }
    if snapshot.ledger_overflow > 0 {
        return Some(raw_l2_result(
            full_text,
            "ledger_overflow",
            snapshot.generation,
        ));
    }
    let Some(ordered) = ordered_records(&snapshot) else {
        return Some(raw_l2_result(
            full_text,
            "missing_span_identity",
            snapshot.generation,
        ));
    };
    if ordered.is_empty() {
        return Some(raw_l2_result(
            full_text,
            "no_stable_spans",
            snapshot.generation,
        ));
    }
    let validated = validate_ledger_against_text(&ordered, full_text);
    if !validated.complete {
        info!(
            generation = snapshot.generation,
            spans = ordered.len(),
            validated = validated.spans_validated,
            formatted = validated.formatted_validated,
            flush_wait_ms,
            "inline_format_fallback reason=ledger_mismatch"
        );
        return Some(raw_l2_result(
            full_text,
            "ledger_mismatch",
            snapshot.generation,
        ));
    }

    let tail_raw = full_text[validated.tail_start_byte..].trim();
    let tail_chars = tail_raw.chars().count();
    let (tail_text, tail_status, tail_degraded) = if tail_raw.is_empty() {
        (String::new(), "empty", false)
    } else if let Some(lane) = snapshot.lane.as_ref() {
        let system_prompt = format!("{}\n\n{}", lane.system_prompt(), INLINE_CLOSE_PROMPT);
        match tokio::time::timeout(
            tail_timeout(),
            format_inline_with_chain_recovery(
                tail_raw,
                language.or(snapshot.language.as_deref()),
                snapshot.chain.clone(),
                &system_prompt,
                lane,
            ),
        )
        .await
        {
            Ok(InlineAttempt {
                result: Ok((raw_out, _response_id)),
                ..
            }) => {
                let cleaned = crate::stream_postprocess::apply_lexicon(raw_out.trim());
                let guard = validate_formatted_span(tail_raw, &cleaned);
                match guard.disposition {
                    GuardDisposition::Accepted => (cleaned, "applied", false),
                    GuardDisposition::RejectedAddition => {
                        (tail_raw.to_string(), "rejected_addition", true)
                    }
                    GuardDisposition::RejectedLoss => (tail_raw.to_string(), "rejected_loss", true),
                    GuardDisposition::RejectedReorder => {
                        (tail_raw.to_string(), "rejected_reorder", true)
                    }
                }
            }
            Ok(InlineAttempt {
                result: Err(error), ..
            }) => {
                warn!("inline format tail request failed: {error:#}");
                (tail_raw.to_string(), "failed", true)
            }
            Err(_) => (tail_raw.to_string(), "timeout", true),
        }
    } else {
        (tail_raw.to_string(), "lane_unavailable", true)
    };

    let mut composed = validated.formatted_prefix.clone();
    if !tail_text.is_empty() {
        if !composed.is_empty() {
            composed.push(' ');
        }
        composed.push_str(&tail_text);
    }
    if composed.trim().is_empty() {
        info!("inline_format_fallback reason=empty_composition flush_wait_ms={flush_wait_ms}");
        return Some(raw_l2_result(
            full_text,
            "empty_composition",
            snapshot.generation,
        ));
    }

    let document_guard = validate_formatted_span(full_text, &composed);
    if document_guard.disposition != GuardDisposition::Accepted {
        info!(
            generation = snapshot.generation,
            disposition = ?document_guard.disposition,
            added_tokens = document_guard.added_tokens,
            omitted_tokens = document_guard.omitted_tokens,
            reordered = document_guard.reordered,
            "inline_format_fallback reason=document_guard"
        );
        return Some(raw_l2_result(
            full_text,
            "document_guard",
            snapshot.generation,
        ));
    }

    let fallback_spans = ordered
        .iter()
        .filter(|record| !matches!(record.status, ChunkStatus::Applied | ChunkStatus::Skipped))
        .count();
    let applied_spans = ordered
        .iter()
        .filter(|record| record.status == ChunkStatus::Applied)
        .count();
    let degraded = !flushed || fallback_spans > 0 || tail_degraded;

    info!(
        generation = snapshot.generation,
        spans = ordered.len(),
        validated = validated.spans_validated,
        applied = applied_spans,
        raw_fallback = fallback_spans,
        tail_chars,
        tail_status,
        flushed,
        flush_wait_ms,
        "inline_format_compose",
    );

    Some(AiFormatResult {
        text: composed,
        reasoning_text: None,
        status: if degraded {
            AiFormatStatus::Failed
        } else if applied_spans == 0 && tail_raw.is_empty() {
            AiFormatStatus::Skipped
        } else {
            AiFormatStatus::Applied
        },
    })
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pipeline::contracts::{NonSpeechEvidence, SidebandProvenance};
    use crate::stt::tail_provider::TailSampleRange;

    fn record(id: u64, raw: &str, formatted: Option<&str>) -> ChunkRecord {
        ChunkRecord {
            identity: StableFormatSpan {
                session_id: "session-test".to_string(),
                capture_epoch: 1,
                span_id: id,
                sample_start: id.saturating_sub(1) * 16_000,
                sample_end: id * 16_000,
                text: raw.to_string(),
                sideband: Vec::new(),
            },
            raw: raw.to_string(),
            formatted: formatted.map(str::to_string),
            status: if formatted.is_some() {
                ChunkStatus::Applied
            } else {
                ChunkStatus::Failed
            },
        }
    }

    #[test]
    fn l3_consumes_only_pause_duration_as_formatting_context() {
        let range = |start, end| TailSampleRange {
            session: "session-test".to_string(),
            capture_epoch: 1,
            sample_start: start,
            sample_end: end,
        };
        let evidence = vec![
            SidebandEvidence {
                sequence: 1,
                range: range(8_000, 8_000),
                sample_rate_hz: 16_000,
                provenance: SidebandProvenance::SileroVad,
                evidence: SidebandEvidenceKind::SpeechStart {
                    speech_probability: 0.91,
                },
            },
            SidebandEvidence {
                sequence: 2,
                range: range(8_000, 24_000),
                sample_rate_hz: 16_000,
                provenance: SidebandProvenance::SileroVad,
                evidence: SidebandEvidenceKind::Pause {
                    duration_samples: 16_000,
                    non_speech: NonSpeechEvidence::UnknownNonSpeech,
                },
            },
        ];

        let instruction = pause_timing_instruction(&evidence).expect("pause hint");
        assert!(instruction.contains("1000 ms"));
        assert!(instruction.contains("punctuation or paragraph boundaries"));
        assert!(
            !instruction.contains("0.91"),
            "speech probability is not L3 input"
        );
        for unsupported in ["laughter", "noise", "cough"] {
            assert!(
                !instruction.contains(unsupported),
                "unmeasured named sound leaked into L3: {unsupported}"
            );
        }

        assert!(
            pause_timing_instruction(&evidence[..1]).is_none(),
            "speech edges alone are not formatter context"
        );
    }

    /// Punctuation and casing may change freely; the guard only counts words.
    #[test]
    fn guard_accepts_punctuation_and_casing_changes() {
        assert_eq!(
            validate_formatted_span(
                "no dobra to jest test dyktowania w codescribe",
                "No dobra, to jest test dyktowania w Codescribe."
            )
            .disposition,
            GuardDisposition::Accepted
        );
    }

    /// Addition is a distinct rejection receipt, not a generic similarity miss.
    #[test]
    fn guard_rejects_addition_independently() {
        let receipt = validate_formatted_span(
            "kup mleko i chleb dla kliniki",
            "Kup mleko i chleb dla kliniki dzisiaj.",
        );
        assert_eq!(receipt.disposition, GuardDisposition::RejectedAddition);
        assert!(receipt.added_tokens > 0);
    }

    /// Semantic/content loss has its own rejection receipt.
    #[test]
    fn guard_rejects_loss_independently() {
        let receipt = validate_formatted_span(
            "pierwsza część zdania oraz druga część zdania",
            "Pierwsza część zdania oraz druga część.",
        );
        assert_eq!(receipt.disposition, GuardDisposition::RejectedLoss);
        assert!(receipt.omitted_tokens > 0);
    }

    /// Preserving the multiset is insufficient: moving a spoken phrase fails.
    #[test]
    fn guard_rejects_reorder_independently() {
        let receipt = validate_formatted_span(
            "pierwszy pacjent potem drugi pacjent na końcu trzeci pacjent",
            "Trzeci pacjent, potem drugi pacjent, na końcu pierwszy pacjent.",
        );
        assert_eq!(receipt.disposition, GuardDisposition::RejectedReorder);
        assert!(receipt.reordered);
    }

    /// Small novel-word drift (within budget) is tolerated.
    #[test]
    fn guard_tolerates_tiny_drift() {
        assert_eq!(
            validate_formatted_span(
                "spotkanie jutro o ósmej rano w klinice",
                "Spotkanie jutro o 8 rano w klinice."
            )
            .disposition,
            GuardDisposition::Accepted
        );
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
        let m = validate_ledger_against_text(&chunks, full);
        assert!(m.complete);
        assert_eq!(m.spans_validated, 2);
        assert_eq!(m.formatted_validated, 2);
        assert_eq!(
            m.formatted_prefix,
            "Pierwsze zdanie o żółwiu. Drugie zdanie o jeżu."
        );
        assert_eq!(&full[m.tail_start_byte..], "i ogon który został");
    }

    /// Canvas drift cannot be used to re-key results by text: composition is
    /// refused and the caller returns the complete controller L2 text.
    #[test]
    fn ledger_gap_mismatch_refuses_partial_composition() {
        let chunks = vec![
            record(1, "pierwsze zdanie", Some("Pierwsze zdanie.")),
            record(2, "trzecie zdanie", Some("Trzecie zdanie.")),
        ];
        let full = "Pierwsze zdanie wstawka z gap append trzecie zdanie";
        let m = validate_ledger_against_text(&chunks, full);
        assert!(!m.complete);
        assert_eq!(m.spans_validated, 1);
        assert_eq!(m.tail_start_byte, 0);
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
        let m = validate_ledger_against_text(&chunks, full);
        assert!(m.complete);
        assert_eq!(m.spans_validated, 2);
        assert_eq!(m.formatted_validated, 1);
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
        let m = validate_ledger_against_text(&chunks, full);
        assert!(!m.complete);
        assert_eq!(m.spans_validated, 0);
        assert_eq!(m.tail_start_byte, 0);
    }

    /// Fully covered transcript → empty tail (stop pays zero LLM requests).
    #[test]
    fn matcher_full_coverage_leaves_empty_tail() {
        let chunks = vec![record(1, "całość wypowiedzi", Some("Całość wypowiedzi."))];
        let full = "całość wypowiedzi";
        let m = validate_ledger_against_text(&chunks, full);
        assert!(m.complete);
        assert_eq!(m.spans_validated, 1);
        assert_eq!(full[m.tail_start_byte..].trim(), "");
    }

    /// PCM order is independently required even when the concatenated text
    /// would happen to match.
    #[test]
    fn ledger_rejects_out_of_order_pcm_identity() {
        let mut first = record(1, "pierwsze zdanie", Some("Pierwsze zdanie."));
        first.identity.sample_start = 16_000;
        first.identity.sample_end = 32_000;
        let mut second = record(2, "drugie zdanie", Some("Drugie zdanie."));
        second.identity.sample_start = 0;
        second.identity.sample_end = 16_000;
        let validation =
            validate_ledger_against_text(&[first, second], "pierwsze zdanie drugie zdanie");
        assert!(!validation.complete);
        assert_eq!(validation.tail_start_byte, 0);
    }

    #[test]
    fn ledger_orders_epoch_before_sample_clock_and_allows_epoch_reset() {
        let mut first = record(1, "pierwsze zdanie", Some("Pierwsze zdanie."));
        first.identity.capture_epoch = 4;
        first.identity.sample_start = 80_000;
        first.identity.sample_end = 96_000;
        let mut second = record(2, "drugie zdanie", Some("Drugie zdanie."));
        second.identity.capture_epoch = 5;
        second.identity.sample_start = 0;
        second.identity.sample_end = 16_000;
        assert!(
            validate_ledger_against_text(
                &[first.clone(), second.clone()],
                "pierwsze zdanie drugie zdanie"
            )
            .complete
        );

        second.identity.capture_epoch = 3;
        assert!(
            !validate_ledger_against_text(&[first, second], "pierwsze zdanie drugie zdanie")
                .complete
        );
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
        }

        impl Drop for EnvGuard {
            fn drop(&mut self) {
                match self.prev.as_deref() {
                    Some(value) => unsafe { std::env::set_var(self.key, value) },
                    None => unsafe { std::env::remove_var(self.key) },
                }
            }
        }

        fn pin_clocks(chunk_ms: u64, flush_ms: u64, tail_ms: u64) -> Vec<EnvGuard> {
            vec![
                EnvGuard::set(
                    "CODESCRIBE_INLINE_FORMAT_CHUNK_TIMEOUT_MS",
                    &chunk_ms.to_string(),
                ),
                EnvGuard::set(
                    "CODESCRIBE_INLINE_FORMAT_FLUSH_TIMEOUT_MS",
                    &flush_ms.to_string(),
                ),
                EnvGuard::set(
                    "CODESCRIBE_INLINE_FORMAT_TAIL_TIMEOUT_MS",
                    &tail_ms.to_string(),
                ),
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
            tx: mpsc::Sender<Cmd>,
            shared: Arc<Mutex<SessionStore>>,
        }

        fn private_store(language: &str, server_url: &str) -> Arc<Mutex<SessionStore>> {
            Arc::new(Mutex::new(SessionStore {
                active: true,
                generation: 1,
                session_id: "session-test".to_string(),
                language: Some(language.to_string()),
                chunks: BTreeMap::new(),
                order: Vec::new(),
                chain: None,
                lane: Some(ai_formatting::InlineFormattingLane::for_test(
                    format!("{server_url}/v1/responses"),
                    "mock-nano",
                    "mock-key",
                )),
                ledger_overflow: 0,
            }))
        }

        fn spawn_private_worker(language: &str, server_url: &str) -> Harness {
            let shared = private_store(language, server_url);
            let (tx, rx) = mpsc::channel(SEALED_SPAN_QUEUE_CAPACITY);
            let worker = tokio::spawn(worker_loop(rx, Arc::clone(&shared)));
            // Surface a silent worker panic instead of an opaque settle timeout.
            tokio::spawn(async move {
                if let Err(join_error) = worker.await {
                    eprintln!("inline-format test worker died: {join_error:?}");
                }
            });
            Harness { tx, shared }
        }

        fn span(id: u64, text: &str) -> StableFormatSpan {
            StableFormatSpan {
                session_id: "session-test".to_string(),
                capture_epoch: 1,
                span_id: id,
                sample_start: id.saturating_sub(1) * 16_000,
                sample_end: id * 16_000,
                text: text.to_string(),
                sideband: Vec::new(),
            }
        }

        async fn queue_span(harness: &Harness, id: u64, text: &str) {
            assert!(register_stable_span(&harness.shared, 1, span(id, text)));
            harness
                .tx
                .send(Cmd::Chunk {
                    generation: 1,
                    span_id: id,
                })
                .await
                .expect("worker alive");
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
                        && s.chunks
                            .values()
                            .all(|chunk| chunk.status != ChunkStatus::Pending)
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
        async fn inline_format_closes_span_ordered_document() {
            let mut server = mockito::Server::new_async().await;
            let _env = pin_clocks(10_000, 2_500, 15_000);

            let chunk1 = server
                .mock("POST", "/v1/responses")
                .match_body(Matcher::AllOf(vec![
                    Matcher::Regex("consecutive stable span".into()),
                    Matcher::Regex(r#""instructions":"[^"]+""#.into()),
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
            let chunk3 = server
                .mock("POST", "/v1/responses")
                .match_body(Matcher::AllOf(vec![
                    Matcher::Regex("trzecie zdanie zachowuje kolejnosc".into()),
                    Matcher::Regex(r#""previous_response_id":"resp_2""#.into()),
                ]))
                .with_status(200)
                .with_header("content-type", "application/json")
                .with_body(responses_body(
                    "resp_3",
                    "Trzecie zdanie zachowuje kolejnosc.",
                ))
                .expect(1)
                .create_async()
                .await;
            let tail = server
                .mock("POST", "/v1/responses")
                .match_body(Matcher::AllOf(vec![
                    Matcher::Regex("FINAL residual stable span".into()),
                    Matcher::Regex(r#""previous_response_id":"resp_3""#.into()),
                    Matcher::Regex("ogon który nie został".into()),
                ]))
                .with_status(200)
                .with_header("content-type", "application/json")
                .with_body(responses_body(
                    "resp_4",
                    "Ogon, który nie został zapieczętowany.",
                ))
                .expect(1)
                .create_async()
                .await;

            let h = spawn_private_worker("pl", &server.url());
            queue_span(&h, 1, "pierwsze zdanie o testowaniu bufora").await;
            queue_span(&h, 2, "drugie zdanie o zamykaniu wypowiedzi").await;
            queue_span(&h, 3, "trzecie zdanie zachowuje kolejnosc").await;

            // All three spans format DURING dictation (before any stop call).
            wait_for_settled_chunks(&h.shared, 3).await;
            {
                let s = h.shared.lock().unwrap();
                assert!(
                    s.chunks
                        .values()
                        .all(|chunk| chunk.status == ChunkStatus::Applied),
                    "all spans must be accepted in flight: {:?}",
                    s.chunks
                );
                assert_eq!(
                    s.chain.as_deref(),
                    Some("resp_3"),
                    "chain must advance to the last accepted chunk"
                );
            }

            let full_text = "pierwsze zdanie o testowaniu bufora drugie zdanie o \
                             zamykaniu wypowiedzi trzecie zdanie zachowuje kolejnosc \
                             ogon który nie został zapieczętowany";
            let started = Instant::now();
            let result = compose_and_close_with(&h.tx, &h.shared, full_text, Some("pl"))
                .await
                .expect("compose must succeed when chunks cover the prefix");
            let stop_secs = started.elapsed().as_secs_f64();

            assert_eq!(result.status, AiFormatStatus::Applied);
            assert_eq!(
                result.text,
                "Pierwsze zdanie o testowaniu bufora. Drugie zdanie o zamykaniu \
                 wypowiedzi. Trzecie zdanie zachowuje kolejnosc. Ogon, który nie \
                 został zapieczętowany."
            );
            chunk1.assert_async().await;
            chunk2.assert_async().await;
            chunk3.assert_async().await;
            tail.assert_async().await;
            assert!(
                stop_secs < 3.0,
                "stop seam must fit the <3s budget with a local provider (measured {stop_secs:.3}s)"
            );
            // Emit the measured number so the report can quote it.
            eprintln!(
                "inline_format_stop_seam_secs={stop_secs:.3} spans_in_flight=3 stop_requests=1"
            );
        }

        /// Fail-open per chunk: a provider 500 keeps the raw chunk text, the
        /// chain stays on the last accepted id, and stop still composes —
        /// the session is never blocked by a failed chunk.
        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        #[serial]
        async fn chunk_failure_is_fail_open_and_keeps_chain() {
            let mut server = mockito::Server::new_async().await;
            let _env = pin_clocks(10_000, 2_500, 15_000);

            let chunk1 = server
                .mock("POST", "/v1/responses")
                .match_body(Matcher::Regex("pierwszy kawalek dyktowania".into()))
                .with_status(200)
                .with_header("content-type", "application/json")
                .with_body(responses_body("resp_1", "Pierwszy kawalek dyktowania."))
                .expect(1)
                .create_async()
                .await;
            let chunk2 = server
                .mock("POST", "/v1/responses")
                .match_body(Matcher::Regex("drugi kawalek ktory pada".into()))
                .with_status(500)
                .with_header("content-type", "text/plain")
                .with_body("provider unavailable")
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
                    r#""role":"developer"[\s\S]*FINAL residual stable span[\s\S]*"previous_response_id":"resp_1""#
                        .into(),
                ))
                .with_status(200)
                .with_header("content-type", "application/json")
                .with_body(responses_body("resp_9", "Ogon po awarii."))
                .expect(1)
                .create_async()
                .await;

            let h = spawn_private_worker("pl", &server.url());
            queue_span(&h, 1, "pierwszy kawalek dyktowania").await;
            wait_for_settled_chunks(&h.shared, 1).await;

            queue_span(&h, 2, "drugi kawalek ktory pada").await;
            wait_for_settled_chunks(&h.shared, 2).await;
            {
                let s = h.shared.lock().unwrap();
                assert_eq!(s.chunks.get(&1).unwrap().status, ChunkStatus::Applied);
                assert_eq!(s.chunks.get(&2).unwrap().status, ChunkStatus::Failed);
                assert_eq!(
                    s.chunks.get(&2).unwrap().formatted,
                    None,
                    "failed chunk keeps raw"
                );
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
            assert_eq!(result.status, AiFormatStatus::Failed);
            chunk1.assert_async().await;
            chunk2.assert_async().await;
            tail.assert_async().await;
        }

        /// An anti-invention violation from the provider is rejected: the raw
        /// chunk survives and the chain does not advance onto the poisoned id.
        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        #[serial]
        async fn invented_chunk_output_is_rejected_with_raw_kept() {
            let mut server = mockito::Server::new_async().await;
            let _env = pin_clocks(10_000, 2_500, 15_000);

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

            let h = spawn_private_worker("pl", &server.url());
            queue_span(&h, 1, "kup mleko i chleb dla kliniki").await;
            wait_for_settled_chunks(&h.shared, 1).await;

            {
                let s = h.shared.lock().unwrap();
                assert_eq!(
                    s.chunks.get(&1).unwrap().status,
                    ChunkStatus::RejectedAddition
                );
                assert_eq!(
                    s.chunks.get(&1).unwrap().formatted,
                    None,
                    "invented text must not land"
                );
                assert_eq!(s.chain, None, "chain must not advance onto a rejected id");
            }
            chunk.assert_async().await;
        }

        /// A real delayed HTTP response crosses the chunk clock. The span is
        /// retained raw and the chain remains clean.
        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        #[serial]
        async fn chunk_timeout_keeps_l2_and_does_not_advance_chain() {
            let mut server = mockito::Server::new_async().await;
            let _env = pin_clocks(500, 2_500, 15_000);
            let delayed_body = responses_body("resp_late", "Powolny fragment odpowiedzi.");
            let delayed = server
                .mock("POST", "/v1/responses")
                .match_body(Matcher::Regex("powolny fragment odpowiedzi".into()))
                .with_status(200)
                .with_header("content-type", "application/json")
                .with_chunked_body(move |writer| {
                    std::thread::sleep(Duration::from_millis(1_500));
                    writer.write_all(delayed_body.as_bytes())
                })
                .expect(1)
                .create_async()
                .await;

            let h = spawn_private_worker("pl", &server.url());
            queue_span(&h, 1, "powolny fragment odpowiedzi").await;
            wait_for_settled_chunks(&h.shared, 1).await;

            {
                let s = h.shared.lock().unwrap();
                assert_eq!(s.chunks.get(&1).unwrap().status, ChunkStatus::Failed);
                assert_eq!(s.chunks.get(&1).unwrap().formatted, None);
                assert_eq!(s.chain, None);
            }
            delayed.assert_async().await;
        }

        /// A response id invalid for the pinned credential is session-local
        /// poison: clear it and retry this span once without a previous id.
        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        #[serial]
        async fn stale_chain_retries_once_unchained_and_advances_clean_id() {
            let mut server = mockito::Server::new_async().await;
            let _env = pin_clocks(10_000, 2_500, 15_000);

            let stale = server
                .mock("POST", "/v1/responses")
                .match_body(Matcher::Regex(
                    r#""previous_response_id":"resp_stale""#.into(),
                ))
                .with_status(400)
                .with_header("content-type", "application/json")
                .with_body(r#"{"error":{"code":"previous_response_not_found"}}"#)
                .expect(1)
                .create_async()
                .await;
            let recovered = server
                .mock("POST", "/v1/responses")
                .match_body(Matcher::AllOf(vec![
                    Matcher::Regex("fragment po rotacji klucza".into()),
                    Matcher::Regex(r#""instructions":"[^"]+""#.into()),
                ]))
                .with_status(200)
                .with_header("content-type", "application/json")
                .with_body(responses_body("resp_fresh", "Fragment po rotacji klucza."))
                .expect(1)
                .create_async()
                .await;

            let h = spawn_private_worker("pl", &server.url());
            h.shared.lock().unwrap().chain = Some("resp_stale".to_string());
            queue_span(&h, 1, "fragment po rotacji klucza").await;
            wait_for_settled_chunks(&h.shared, 1).await;

            {
                let store = h.shared.lock().unwrap();
                assert_eq!(store.chunks.get(&1).unwrap().status, ChunkStatus::Applied);
                assert_eq!(store.chain.as_deref(), Some("resp_fresh"));
            }
            stale.assert_async().await;
            recovered.assert_async().await;
        }

        /// Queue pressure is fail-open and synchronous: the second stable span
        /// is ledgered as raw without waiting for the worker or capture path.
        #[tokio::test]
        #[serial_test::serial]
        async fn bounded_queue_overflow_keeps_span_identity_and_raw_text() {
            let shared = private_store("pl", "http://127.0.0.1:1");
            let (tx, _rx) = mpsc::channel(1);
            assert!(register_stable_span(
                &shared,
                1,
                span(1, "pierwszy stabilny fragment")
            ));
            tx.try_send(Cmd::Chunk {
                generation: 1,
                span_id: 1,
            })
            .expect("first item fills the queue");

            let started = Instant::now();
            assert!(register_stable_span(
                &shared,
                1,
                span(2, "drugi stabilny fragment")
            ));
            match tx.try_send(Cmd::Chunk {
                generation: 1,
                span_id: 2,
            }) {
                Err(mpsc::error::TrySendError::Full(_)) => {
                    if let Ok(mut store) = shared.lock()
                        && let Some(record) = store.chunks.get_mut(&2)
                    {
                        record.status = ChunkStatus::QueueOverflow;
                    }
                }
                other => panic!("expected bounded queue overflow, got {other:?}"),
            }
            assert!(started.elapsed() < Duration::from_millis(50));

            let store = shared.lock().unwrap();
            assert_eq!(store.order, vec![1, 2]);
            let overflow = store.chunks.get(&2).unwrap();
            assert_eq!(overflow.identity.span_id, 2);
            assert_eq!(overflow.status, ChunkStatus::QueueOverflow);
            assert_eq!(overflow.display_text(), "drugi stabilny fragment");
        }

        /// Once a live ledger exists, an identity/coverage mismatch returns
        /// the controller's complete L2 document. It must not escape into the
        /// classic whole-document formatter on the stop path.
        #[tokio::test]
        #[serial_test::serial]
        async fn active_ledger_mismatch_returns_complete_l2_without_full_request() {
            let shared = private_store("pl", "http://127.0.0.1:1");
            assert!(register_stable_span(
                &shared,
                1,
                span(1, "pierwszy stabilny fragment")
            ));
            {
                let mut store = shared.lock().unwrap();
                let record = store.chunks.get_mut(&1).unwrap();
                record.status = ChunkStatus::Applied;
                record.formatted = Some("Pierwszy stabilny fragment.".to_string());
            }
            let (tx, rx) = mpsc::channel(1);
            let worker = tokio::spawn(worker_loop(rx, Arc::clone(&shared)));
            let l2 = "kontroler ma inny kompletny tekst warstwy drugiej";
            let result = compose_and_close_with(&tx, &shared, l2, Some("pl"))
                .await
                .expect("active session always returns a fail-open result");
            drop(tx);
            worker.await.expect("worker exits cleanly");

            assert_eq!(result.text, l2);
            assert_eq!(result.status, AiFormatStatus::Failed);
        }
    }
}

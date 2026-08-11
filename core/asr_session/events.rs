//! Typed Layer 1 session events.
//!
//! Every event a Layer 1 provider emits carries three identity fields:
//!
//! - **session** — which recording this belongs to. A provider that reconnects
//!   and resumes the wrong stream is caught here rather than downstream.
//! - **utterance** — which speech unit inside the session. Sealing is
//!   per-utterance, so a late partial cannot reopen committed text.
//! - **sequence** — the provider's monotonic stream counter, the only ordering
//!   authority. Arrival order is not ordering: a reconnect replays.
//!
//! Partial versus final is a **variant**, not a boolean flag, so a caller
//! cannot forget to check it — the compiler makes the finality decision
//! explicit at every match site.
//!
//! Errors and usage are typed with no free-form string payload. That is a
//! deliberate privacy boundary: a `String` on an error is exactly where a
//! transcript fragment, an audio path, or a bearer token ends up in a log.

use std::fmt;

/// Per-recording session identity minted when a Layer 1 session opens.
///
/// Opaque on purpose: the desktop consumes whatever the gateway/session mint
/// hands it and never parses meaning out of it.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SessionId(String);

impl SessionId {
    /// Build a session id, rejecting blank input.
    ///
    /// An empty id would make every session compare equal, which turns the
    /// foreign-session guard in [`super::ingest`] into a no-op.
    pub fn new(raw: impl Into<String>) -> Option<Self> {
        let raw = raw.into();
        if raw.trim().is_empty() {
            return None;
        }
        Some(Self(raw))
    }

    /// Borrow the raw id.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for SessionId {
    /// Print the raw id (it is an opaque handle, not a secret).
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Session, utterance, and sequence identity carried by every event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventIdentity {
    /// Which session this event belongs to.
    session_id: SessionId,
    /// Which utterance inside the session.
    utterance_id: u64,
    /// Provider-side monotonic stream counter; the ordering authority.
    sequence_id: u64,
}

impl EventIdentity {
    /// Build an identity triple.
    pub fn new(session_id: SessionId, utterance_id: u64, sequence_id: u64) -> Self {
        Self {
            session_id,
            utterance_id,
            sequence_id,
        }
    }

    /// Session this event belongs to.
    pub fn session_id(&self) -> &SessionId {
        &self.session_id
    }

    /// Utterance this event belongs to.
    pub fn utterance_id(&self) -> u64 {
        self.utterance_id
    }

    /// Monotonic stream position of this event.
    pub fn sequence_id(&self) -> u64 {
        self.sequence_id
    }
}

/// A bounded span of session audio an event describes.
///
/// Session time, measured in seconds from the first captured sample — the same
/// clock the Apple progressive path derives `audio_secs` from. It is neither
/// wall clock nor the capture device clock.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AudioRange {
    /// Inclusive start in session seconds.
    start_secs: f32,
    /// Exclusive end in session seconds.
    end_secs: f32,
}

impl AudioRange {
    /// Widest span an event may claim.
    ///
    /// Pinned to the live PCM ring's retention rather than restated, so the two
    /// cannot drift: a range wider than what the session still holds describes
    /// audio nothing can re-read, and a consumer resolving it would be handed a
    /// silently short window.
    pub const MAX_SPAN_SECS: f32 =
        crate::pipeline::streaming::live_audio_buffer::DEFAULT_RETENTION_SECS;

    /// Build a range, refusing anything that is not a usable span.
    ///
    /// Rejects non-finite bounds (`f32 as u64` maps NaN to 0 and saturates
    /// infinities, turning a corrupt timestamp into a plausible window),
    /// negative starts, inverted or empty spans, and spans past
    /// [`Self::MAX_SPAN_SECS`].
    pub fn new(start_secs: f32, end_secs: f32) -> Option<Self> {
        if !start_secs.is_finite() || !end_secs.is_finite() {
            return None;
        }
        if start_secs < 0.0 || end_secs <= start_secs {
            return None;
        }
        if end_secs - start_secs > Self::MAX_SPAN_SECS {
            return None;
        }
        Some(Self {
            start_secs,
            end_secs,
        })
    }

    /// Inclusive start in session seconds.
    pub fn start_secs(&self) -> f32 {
        self.start_secs
    }

    /// Exclusive end in session seconds.
    pub fn end_secs(&self) -> f32 {
        self.end_secs
    }

    /// Span length in seconds.
    pub fn duration_secs(&self) -> f32 {
        self.end_secs - self.start_secs
    }
}

/// Why a Layer 1 session failed — typed, with no free-form payload.
///
/// Every variant means the same thing to the product: the refiner is gone for
/// now and the canvas plus lexicon carry the session. The distinction exists so
/// a caller can decide whether retrying is worth anything, never so an error
/// message can be shown verbatim.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AsrErrorKind {
    /// Connection dropped, timed out, or was never established.
    Transport,
    /// Session credentials were rejected or expired.
    Auth,
    /// Provider asked us to slow down.
    RateLimited,
    /// Our side could not keep up and dropped bounded work.
    Overflow,
    /// Provider does not support what the session asked for (locale, mode).
    Unsupported,
    /// Provider spoke something this contract cannot parse.
    Protocol,
    /// The session was closed by us before it produced a final.
    Cancelled,
}

impl AsrErrorKind {
    /// Whether reopening the session could plausibly succeed.
    ///
    /// `Auth`, `Unsupported`, and `Protocol` are settings- or contract-level
    /// faults: retrying them just burns audio egress for the same failure.
    pub fn is_retryable(&self) -> bool {
        match self {
            Self::Transport | Self::RateLimited | Self::Overflow => true,
            Self::Auth | Self::Unsupported | Self::Protocol | Self::Cancelled => false,
        }
    }

    /// Stable snake_case token for logs and telemetry.
    pub fn as_token(&self) -> &'static str {
        match self {
            Self::Transport => "transport",
            Self::Auth => "auth",
            Self::RateLimited => "rate_limited",
            Self::Overflow => "overflow",
            Self::Unsupported => "unsupported",
            Self::Protocol => "protocol",
            Self::Cancelled => "cancelled",
        }
    }
}

impl fmt::Display for AsrErrorKind {
    /// Print the stable token — never a provider-supplied string.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_token())
    }
}

/// Recognized text for one utterance, partial or final.
#[derive(Debug, Clone, PartialEq)]
pub struct TranscriptEvent {
    /// Session, utterance, and sequence identity.
    pub identity: EventIdentity,
    /// The recognized text. Layer 1 output is a *candidate*; committing it is
    /// the caller's decision and is bounded by the append-only doctrine.
    pub text: String,
    /// Session-time span this text came from, when the provider reports one.
    pub range: Option<AudioRange>,
}

/// A typed session failure.
#[derive(Debug, Clone, PartialEq)]
pub struct ErrorEvent {
    /// Session, utterance, and sequence identity.
    pub identity: EventIdentity,
    /// What went wrong.
    pub kind: AsrErrorKind,
}

/// Consumption accounting for one session — no content, ever.
#[derive(Debug, Clone, PartialEq)]
pub struct UsageEvent {
    /// Session, utterance, and sequence identity.
    pub identity: EventIdentity,
    /// Audio seconds the provider processed.
    pub audio_secs: f32,
    /// Provider-side billable units, when it reports them.
    pub billable_units: Option<u64>,
}

/// Everything a Layer 1 provider can emit.
#[derive(Debug, Clone, PartialEq)]
pub enum AsrSessionEvent {
    /// Volatile hypothesis; may be revised by a later partial or the final.
    Partial(TranscriptEvent),
    /// Sealing result for an utterance. Re-delivery is idempotent.
    Final(TranscriptEvent),
    /// Typed failure.
    Error(ErrorEvent),
    /// Consumption accounting.
    Usage(UsageEvent),
}

impl AsrSessionEvent {
    /// Identity triple carried by this event.
    pub fn identity(&self) -> &EventIdentity {
        match self {
            Self::Partial(event) | Self::Final(event) => &event.identity,
            Self::Error(event) => &event.identity,
            Self::Usage(event) => &event.identity,
        }
    }

    /// Whether this event carries recognized text.
    pub fn is_transcript(&self) -> bool {
        matches!(self, Self::Partial(_) | Self::Final(_))
    }

    /// Whether this event seals its utterance.
    pub fn is_final(&self) -> bool {
        matches!(self, Self::Final(_))
    }

    /// Stable snake_case variant token for logs and telemetry.
    pub fn as_token(&self) -> &'static str {
        match self {
            Self::Partial(_) => "partial",
            Self::Final(_) => "final",
            Self::Error(_) => "error",
            Self::Usage(_) => "usage",
        }
    }
}

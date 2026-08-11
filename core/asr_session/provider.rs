//! The Layer 1 provider seam, and the selection split it depends on.
//!
//! ## Two axes, not one dial
//!
//! Which engine draws the live canvas (Layer 0) and which refiner improves it
//! (Layer 1) are independent choices, and [`LayerSelection`] is what keeps them
//! that way. Collapsing them into a single "engine" setting is how a Layer 1
//! failure ends up silently changing what the user sees being typed, and how a
//! refiner choice ends up loading local weights nobody asked for.
//!
//! The canvas axis already has an owner — the STT router's
//! `CODESCRIBE_STT_ENGINE` policy. [`LayerSelection::for_active_canvas`] reads
//! that decision rather than restating it, so this module can never become a
//! second, disagreeing source of truth about the canvas.
//!
//! ## What the trait does not promise
//!
//! No transport, no retry policy, no bounded-drain policy, no consent gate, no
//! model. Those are follow-on cuts. What is fixed here is the shape: a session
//! opens once, is fed audio, emits typed events, and closes.

use super::events::{AsrErrorKind, AsrSessionEvent, SessionId};

/// Which engine draws the instant live canvas (Layer 0).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CanvasEngine {
    /// Apple Speech — the letter-level instant canvas, the product default.
    AppleSpeech,
    /// Local Whisper, when Apple is unavailable or explicitly overridden.
    LocalWhisper,
}

impl CanvasEngine {
    /// Stable snake_case token for logs and telemetry.
    pub fn as_token(&self) -> &'static str {
        match self {
            Self::AppleSpeech => "apple_speech",
            Self::LocalWhisper => "local_whisper",
        }
    }
}

/// Which Layer 1 refiner is armed.
///
/// [`RefinerMode::Off`] is a complete, shipping product: canvas plus lexicon.
/// Every failure path in Layer 1 lands here, and landing here is never a
/// degraded-mode apology that justifies loading something heavier instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RefinerMode {
    /// No Layer 1. Canvas plus lexicon carries the session.
    #[default]
    Off,
    /// A normalized remote session behind the gateway contract.
    CloudSession,
    /// A killable local helper process holding its own weights.
    LocalHelper,
}

impl RefinerMode {
    /// Whether this mode sends captured audio off the machine.
    ///
    /// The consent gate is a separate cut; this is the classifier it will ask.
    pub fn sends_audio_off_device(&self) -> bool {
        matches!(self, Self::CloudSession)
    }

    /// Stable snake_case token for logs and telemetry.
    pub fn as_token(&self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::CloudSession => "cloud_session",
            Self::LocalHelper => "local_helper",
        }
    }
}

/// The canvas/refiner pair, held together so neither can silently move the
/// other.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LayerSelection {
    /// Layer 0 — who draws.
    canvas: CanvasEngine,
    /// Layer 1 — who refines.
    refiner: RefinerMode,
}

impl LayerSelection {
    /// Pair an explicit canvas with an explicit refiner.
    pub fn new(canvas: CanvasEngine, refiner: RefinerMode) -> Self {
        Self { canvas, refiner }
    }

    /// Pair the router's *live* canvas decision with an independent refiner.
    ///
    /// Reads `stt::active_engine_is_apple`, the same selector the live lane
    /// uses, so the canvas reported here is the canvas that will actually draw.
    /// The refiner argument is untouched by that read — that independence is
    /// the whole point and is pinned by test.
    pub fn for_active_canvas(refiner: RefinerMode) -> Self {
        let canvas = if crate::stt::active_engine_is_apple() {
            CanvasEngine::AppleSpeech
        } else {
            CanvasEngine::LocalWhisper
        };
        Self::new(canvas, refiner)
    }

    /// Layer 0 engine.
    pub fn canvas(&self) -> CanvasEngine {
        self.canvas
    }

    /// Layer 1 mode.
    pub fn refiner(&self) -> RefinerMode {
        self.refiner
    }

    /// The selection this degrades to when Layer 1 is unavailable.
    ///
    /// The canvas is carried through unchanged. A refiner that cannot run is a
    /// missing improvement, never a reason to redraw the canvas with a
    /// different engine or to reach for local weights.
    pub fn degraded(&self) -> Self {
        Self {
            canvas: self.canvas,
            refiner: RefinerMode::Off,
        }
    }
}

/// Parameters a Layer 1 session opens with.
///
/// Deliberately thin: identity, language, and audio format. Credentials,
/// endpoints, and consent are the gateway/settings cuts' business and must not
/// leak into the provider-facing shape.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionInput {
    /// Identity every event from this session must carry.
    pub session_id: SessionId,
    /// BCP-47 language hint, when the product has one to give.
    pub locale: Option<String>,
    /// Sample rate of the audio that will be pushed, in Hz.
    pub sample_rate: u32,
}

/// A Layer 1 refiner session.
///
/// Object-safe on purpose: the recorder cut will hold a
/// `Box<dyn AsrSessionProvider>` chosen at runtime and must not be generic over
/// the transport.
///
/// Lifecycle: [`open`](Self::open) once, then any number of
/// [`push_audio`](Self::push_audio) / [`drain`](Self::drain) calls, then
/// [`close`](Self::close). Calling out of order is a [`AsrErrorKind::Protocol`]
/// fault, not a panic — a live session must degrade, never abort the recording.
pub trait AsrSessionProvider {
    /// Which refiner mode this provider implements.
    fn mode(&self) -> RefinerMode;

    /// Open the session. Called at most once.
    fn open(&mut self, input: &SessionInput) -> Result<(), AsrErrorKind>;

    /// Feed captured audio. Session time is derived from what has been pushed,
    /// so the caller does not have to keep a second clock in sync.
    fn push_audio(&mut self, samples: &[f32]) -> Result<(), AsrErrorKind>;

    /// Take whatever events are ready. Never blocks.
    fn drain(&mut self) -> Vec<AsrSessionEvent>;

    /// Close the session. Trailing events remain available via
    /// [`drain`](Self::drain).
    fn close(&mut self) -> Result<(), AsrErrorKind>;
}

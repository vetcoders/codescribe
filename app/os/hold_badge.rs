//! Caret-anchored "hold badge" recording indicator and deferred-insert capsule.
//!
//! A small colored dot that opens at the mouse cursor and moves once to the
//! text caret (one off-main Accessibility lookup per take). It never chases
//! the pointer and never polls AX afterwards: the old per-tick caret/cursor
//! probe burned CPU and starved capture. Used to signal recording /
//! processing / assistive states during dictation, with the live preview
//! words in a dark capsule beside the dot.
//!
//! When a take's text cannot be pasted and is armed for Deferred Paste, the
//! same panel becomes the draggable capsule ("Blobek"): it outlives the
//! recording, pastes on click into the focused field, pastes where it is
//! dropped after a drag, and dismisses on `×`. The armed slot itself lives in
//! `crate::os::clipboard`; the capsule only projects it.
//!
//! ## K3 contract (W10-E)
//! Size / visibility (`HOLD_BADGE_SIZE`, `HOLD_INDICATOR`) persist immediately
//! when Settings or the tray writes them. A **visible** badge is **not** live-
//! resized — the next `show_hold_badge` reads the config at request time (via
//! the generation gate). Do not claim "restart required"; claim "next show".
//!
//! Resurrected as a self-contained `app/os` module (previously lived in the now
//! excised `app/ui` AppKit layer). All AppKit/objc code is macOS-only; a no-op
//! stub surface keeps non-macOS builds compiling.
//!
//! Vibecrafted with AI Agents by Vetcoders (c)2026 Vetcoders

// ─────────────────────────────────────────────────────────────
// Platform-agnostic surface (pure data, no AppKit)
// ─────────────────────────────────────────────────────────────

#[cfg(any(target_os = "macos", test))]
fn ax_caret_to_appkit(x: f64, y: f64, height: f64, primary_height: f64) -> (f64, f64) {
    (x, primary_height - y - height)
}

/// Bottom-left AppKit screen point → top-left CoreGraphics global point (the
/// space `CGEvent` and `AXUIElementCopyElementAtPosition` use).
#[cfg(any(target_os = "macos", test))]
fn appkit_to_cg_point(x: f64, y: f64, primary_height: f64) -> (f64, f64) {
    (x, primary_height - y)
}

/// Longest label the deferred-insert capsule paints, ellipsis included.
const BLOB_PREVIEW_CHARS: usize = 42;

/// One-line preview of an armed transcript for the draggable capsule: runs of
/// whitespace collapse to one space and the label is capped at
/// [`BLOB_PREVIEW_CHARS`] characters, ending in `…` when cut. The armed slot
/// keeps the full text; this is paint only.
pub fn blob_preview_text(text: &str) -> String {
    let normalized = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if normalized.chars().count() <= BLOB_PREVIEW_CHARS {
        return normalized;
    }
    let mut capped = normalized
        .chars()
        .take(BLOB_PREVIEW_CHARS - 1)
        .collect::<String>()
        .trim_end()
        .to_string();
    capped.push('…');
    capped
}

#[cfg(any(target_os = "macos", test))]
fn fit_origin(origin: f64, extent: f64, screen_origin: f64, screen_extent: f64) -> f64 {
    origin
        .max(screen_origin)
        .min((screen_origin + screen_extent - extent).max(screen_origin))
}

#[cfg(any(target_os = "macos", test))]
fn token_is_current(token: u64, generation: u64) -> bool {
    token == generation
}

/// Bound only the pointer paint; repetition and the source document stay intact.
#[cfg(any(target_os = "macos", test))]
fn preview_tail(text: &str) -> String {
    let mut words = text.split_whitespace().rev().take(3).collect::<Vec<_>>();
    words.reverse();
    let tail = words.join(" ");
    let mut characters = tail.chars().rev().take(120).collect::<Vec<_>>();
    characters.reverse();
    characters.into_iter().collect()
}

/// UI freshness, not an acoustic speech/coverage verdict.
#[cfg(any(target_os = "macos", test))]
fn preview_expired(updated_at: Option<std::time::Instant>, now: std::time::Instant) -> bool {
    updated_at.is_some_and(|updated| now.saturating_duration_since(updated).as_millis() >= 1200)
}

/// Text-input shape of the focused UI element, as the delivery throne's paste
/// gate needs it (`focused_input_field`). An observation, never a destination.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FocusedInputField {
    /// An enabled text input with a writable value or selected-text attribute.
    Text,
    /// A password field, or secure event input is on system-wide (a password
    /// prompt owns the keyboard, e.g. a terminal `sudo` prompt).
    Secure,
    /// Something focused that is not a text-input role (a list, a canvas, …).
    NotText,
    /// Accessibility could not read the focused element.
    Unobserved,
}

/// Classify an AX role/subrole pair. Password fields report `AXTextField` with
/// the `AXSecureTextField` subrole, a few hosts report it as the role itself.
#[cfg(any(target_os = "macos", test))]
fn classify_input_role(role: &str, subrole: Option<&str>) -> FocusedInputField {
    if role == "AXSecureTextField" || subrole == Some("AXSecureTextField") {
        return FocusedInputField::Secure;
    }
    if matches!(
        role,
        "AXTextArea" | "AXTextField" | "AXComboBox" | "AXTextView" | "AXWebArea"
    ) {
        FocusedInputField::Text
    } else {
        FocusedInputField::NotText
    }
}

/// An AX role is only a candidate. A whole web page or a read-only transcript
/// must never become a paste recipient merely by reporting a text role.
#[cfg(any(target_os = "macos", test))]
fn classify_input_capability(
    role: &str,
    subrole: Option<&str>,
    enabled: bool,
    writable: bool,
) -> FocusedInputField {
    match classify_input_role(role, subrole) {
        FocusedInputField::Secure => FocusedInputField::Secure,
        FocusedInputField::Text if enabled && writable && role != "AXWebArea" => {
            FocusedInputField::Text
        }
        _ => FocusedInputField::NotText,
    }
}

pub use crate::presentation::emitter::CursorVoice;

/// One state the pointer dot can show, in the order the Settings legend lists
/// them. [`IndicatorState::color`] is the only palette of the dot: the badge
/// paints from it and the bridge hands it to Swift for the legend and the
/// recording lamp, so no surface writes its own copy of these values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IndicatorState {
    /// Hold and hands-free capture; hands-free pulses, hold stays solid.
    Recording,
    /// Speech outran its words, or the ledger still owes text (dictation).
    WordsOwed,
    /// Transcribing and formatting after the capture ended.
    Processing,
    /// Agent mode while the voice speaks or its words are owed.
    Agent,
    /// Agent mode between phrases: white with an Agent rim.
    AgentQuiet,
    /// A refused chord (a vacant channel digit); nothing is capturing.
    Refused,
}

impl IndicatorState {
    /// Every state, in legend order.
    pub const ALL: [IndicatorState; 6] = [
        IndicatorState::Recording,
        IndicatorState::WordsOwed,
        IndicatorState::Processing,
        IndicatorState::Agent,
        IndicatorState::AgentQuiet,
        IndicatorState::Refused,
    ];

    /// The dot's fill for this state (RGBA). Recording is #FF3B30, the red the
    /// app's recording lamp already drew, so "recording" has one red.
    pub fn color(self) -> (f64, f64, f64, f64) {
        match self {
            IndicatorState::Recording => (1.0, 59.0 / 255.0, 48.0 / 255.0, 0.8),
            IndicatorState::WordsOwed => (1.0, 0.62, 0.0, 1.0),
            IndicatorState::Processing => (1.0, 0.5, 0.0, 0.85),
            IndicatorState::Agent => (0.6, 0.2, 0.9, 0.85),
            IndicatorState::AgentQuiet => (0.97, 0.97, 0.97, 0.95),
            // Slate, so the flash never reads as a recording dot.
            IndicatorState::Refused => (0.45, 0.45, 0.5, 0.92),
        }
    }

    /// The state whose colour rims this dot. A white dot vanishes over a white
    /// page, so the quiet Agent dot keeps an Agent rim.
    pub fn rim(self) -> Option<IndicatorState> {
        match self {
            IndicatorState::AgentQuiet => Some(IndicatorState::Agent),
            _ => None,
        }
    }
}

/// Badge display mode for different recording/processing states
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BadgeMode {
    /// Hold mode (Ctrl): Red, solid - "trzymaj palec!"
    Hold,
    /// Toggle mode (⌥⌥): Red, pulsing - "nagrywam hands-off"
    Toggle,
    /// Processing: Orange - "transkrybuję/formatuję"
    Processing,
    /// AI mode (Chat/Selection): Purple with glow - "AI słucha"
    Assistive,
    /// Refused chord (a vacant channel digit): slate, solid, one flash with a
    /// label beside it. Nothing is capturing while this dot shows.
    Refused,
}

impl BadgeMode {
    /// The palette state this mode shows at rest.
    pub fn indicator_state(&self) -> IndicatorState {
        match self {
            BadgeMode::Hold | BadgeMode::Toggle => IndicatorState::Recording,
            BadgeMode::Processing => IndicatorState::Processing,
            BadgeMode::Assistive => IndicatorState::Agent,
            BadgeMode::Refused => IndicatorState::Refused,
        }
    }

    /// Get the base color for this mode (RGBA)
    pub fn color(&self) -> (f64, f64, f64, f64) {
        self.indicator_state().color()
    }

    /// Whether this mode should pulse (opacity animation)
    pub fn should_pulse(&self) -> bool {
        matches!(self, BadgeMode::Toggle | BadgeMode::Processing)
    }

    /// Whether this mode has glow effect
    pub fn has_glow(&self) -> bool {
        matches!(self, BadgeMode::Assistive)
    }

    /// Get diameter multiplier for this mode
    pub fn diameter_multiplier(&self) -> f64 {
        match self {
            BadgeMode::Assistive => 1.2, // Slightly larger for AI mode
            _ => 1.0,
        }
    }

    /// The dot's fill for the voice under the cursor. Agent mode is purple
    /// whatever the voice does: speaking, and pulsing while words are owed; it
    /// turns white only between phrases (Maciej, 2026-10-14, 2026-10-17: orange
    /// means processing, so owed Agent words must not borrow it). Dictation's
    /// owed words stay amber.
    pub fn voice_color(
        &self,
        base: (f64, f64, f64, f64),
        voice: CursorVoice,
    ) -> (f64, f64, f64, f64) {
        match (self, voice) {
            (BadgeMode::Assistive, CursorVoice::Quiet) => IndicatorState::AgentQuiet.color(),
            (BadgeMode::Assistive, _) => base,
            (_, CursorVoice::Lagging) => IndicatorState::WordsOwed.color(),
            _ => base,
        }
    }

    /// Agent mode pulses while its words are owed; the others keep their own beat.
    pub fn voice_pulses(&self, voice: CursorVoice) -> bool {
        self.should_pulse() || (*self == BadgeMode::Assistive && voice == CursorVoice::Lagging)
    }

    /// A white dot vanishes over a white page; the quiet Agent dot keeps a
    /// purple rim so it still reads as Agent mode.
    pub fn voice_ring(&self, voice: CursorVoice) -> Option<(f64, f64, f64, f64)> {
        if *self != BadgeMode::Assistive || voice != CursorVoice::Quiet {
            return None;
        }
        let rim = IndicatorState::AgentQuiet.rim()?;
        Some(rim.color())
    }
}

/// Configuration for the hold badge
#[derive(Debug, Clone)]
pub struct HoldBadgeConfig {
    /// Diameter of the badge circle in pixels
    pub diameter: f64,
    /// Offset from caret/cursor position (x, y)
    pub offset: (f64, f64),
    /// Update interval in milliseconds
    pub update_interval_ms: u64,
    /// Badge color (R, G, B, A)
    pub color: (f64, f64, f64, f64),
    /// Badge mode for animations
    pub mode: BadgeMode,
}

impl Default for HoldBadgeConfig {
    /// Default hold badge: 12 px recording-red dot, 150 ms updates, Hold mode.
    fn default() -> Self {
        Self {
            diameter: 12.0,
            offset: (10.0, -10.0),
            update_interval_ms: 150,
            color: BadgeMode::Hold.color(),
            mode: BadgeMode::Hold,
        }
    }
}

impl HoldBadgeConfig {
    /// Create config from badge mode with appropriate colors
    pub fn from_mode(mode: BadgeMode) -> Self {
        Self::from_mode_with_base_diameter(mode, Self::default().diameter)
    }

    /// Build the next show request from the currently persisted badge size.
    pub fn from_mode_with_base_diameter(mode: BadgeMode, base_diameter: f64) -> Self {
        let base = Self::default();
        Self {
            diameter: base_diameter * mode.diameter_multiplier(),
            color: mode.color(),
            mode,
            ..base
        }
    }
}

/// Mode visual contracts and size-change-without-mutate visible config.
#[cfg(test)]
mod tests {
    use super::{
        BadgeMode, CursorVoice, FocusedInputField, HoldBadgeConfig, IndicatorState,
        classify_input_role,
    };

    /// S5 (Founder smoke, 2026-10-26): the legend drew colours the dot never
    /// showed, and "recording" had two reds. One palette, one red, every state.
    #[test]
    fn indicator_palette_has_one_recording_red_and_a_colour_for_every_state() {
        let recording = IndicatorState::Recording.color();
        assert_eq!(
            (recording.0, recording.1, recording.2),
            (1.0, 59.0 / 255.0, 48.0 / 255.0)
        );
        assert_eq!(BadgeMode::Hold.color(), recording);
        assert_eq!(BadgeMode::Toggle.color(), recording);
        assert_eq!(HoldBadgeConfig::default().color, recording);

        for (index, state) in IndicatorState::ALL.iter().enumerate() {
            let (r, g, b, a) = state.color();
            for channel in [r, g, b, a] {
                assert!((0.0..=1.0).contains(&channel), "{state:?}");
            }
            assert!(a > 0.0, "{state:?} must be visible");
            for other in &IndicatorState::ALL[index + 1..] {
                assert_ne!(state.color(), other.color(), "{state:?} vs {other:?}");
            }
        }

        for mode in [
            BadgeMode::Hold,
            BadgeMode::Toggle,
            BadgeMode::Processing,
            BadgeMode::Assistive,
            BadgeMode::Refused,
        ] {
            assert!(IndicatorState::ALL.contains(&mode.indicator_state()));
        }
        assert_eq!(
            IndicatorState::AgentQuiet.rim(),
            Some(IndicatorState::Agent)
        );
        assert_eq!(IndicatorState::Recording.rim(), None);
    }

    #[test]
    fn refused_flash_is_a_still_slate_dot_unlike_any_recording_mode() {
        let refused = BadgeMode::Refused;
        for recording in [
            BadgeMode::Hold,
            BadgeMode::Toggle,
            BadgeMode::Processing,
            BadgeMode::Assistive,
        ] {
            assert_ne!(refused.color(), recording.color(), "{recording:?}");
        }
        assert!(!refused.should_pulse());
        assert!(!refused.has_glow());
        assert!(!refused.voice_pulses(CursorVoice::Quiet));
        assert_eq!(refused.voice_ring(CursorVoice::Quiet), None);
        assert_eq!(refused.diameter_multiplier(), 1.0);
        let config = HoldBadgeConfig::from_mode(refused);
        assert_eq!(config.mode, BadgeMode::Refused);
        assert_eq!(config.color, IndicatorState::Refused.color());
    }

    #[test]
    fn live_cue_pointer_keeps_three_latest_words_and_repetitions() {
        assert_eq!(super::preview_tail("Iwo Iwo Iwo Iwo Iwo"), "Iwo Iwo Iwo");
        assert_eq!(
            super::preview_tail("Dobra to jest nagranie testowe teraz mówię dalej"),
            "teraz mówię dalej"
        );
        assert_eq!(
            super::preview_tail("\tzażółć\n gęślą\u{a0}jaźń!  "),
            "zażółć gęślą jaźń!"
        );
        assert_eq!(super::preview_tail("  \n\t"), "");
    }

    #[test]
    fn live_cue_pointer_bounds_unicode_without_breaking_the_last_word() {
        let word = "ź".repeat(160);
        let tail = super::preview_tail(&format!("dawny początek {word}"));
        assert_eq!(tail, "ź".repeat(120));
        assert_eq!(tail.chars().count(), 120);
    }

    #[test]
    fn live_cue_freshness_expires_and_a_new_partial_renews_it() {
        use std::time::{Duration, Instant};
        let start = Instant::now();
        assert!(!super::preview_expired(
            None,
            start + Duration::from_secs(4)
        ));
        assert!(!super::preview_expired(
            Some(start),
            start + Duration::from_millis(1199)
        ));
        assert!(super::preview_expired(
            Some(start),
            start + Duration::from_millis(1200)
        ));
        assert!(!super::preview_expired(
            Some(start + Duration::from_millis(800)),
            start + Duration::from_millis(1200)
        ));
    }

    #[test]
    fn paste_capability_refuses_pages_disabled_and_read_only_text() {
        use super::classify_input_capability as classify;
        assert_eq!(
            classify("AXTextArea", None, true, true),
            FocusedInputField::Text
        );
        for (role, enabled, writable) in [
            ("AXWebArea", true, true),
            ("AXTextArea", false, true),
            ("AXTextArea", true, false),
            ("AXButton", true, true),
        ] {
            assert_eq!(
                classify(role, None, enabled, writable),
                FocusedInputField::NotText
            );
        }
        assert_eq!(
            classify("AXTextField", Some("AXSecureTextField"), true, true),
            FocusedInputField::Secure
        );
    }

    #[test]
    fn password_subrole_is_secure_and_text_roles_are_text() {
        assert_eq!(
            classify_input_role("AXTextField", Some("AXSecureTextField")),
            FocusedInputField::Secure
        );
        assert_eq!(
            classify_input_role("AXSecureTextField", None),
            FocusedInputField::Secure
        );
        for role in [
            "AXTextArea",
            "AXTextField",
            "AXComboBox",
            "AXTextView",
            "AXWebArea",
        ] {
            assert_eq!(
                classify_input_role(role, None),
                FocusedInputField::Text,
                "{role}"
            );
        }
        for role in ["AXList", "AXOutline", "AXButton", "AXGroup"] {
            assert_eq!(
                classify_input_role(role, None),
                FocusedInputField::NotText,
                "{role}"
            );
        }
    }

    #[test]
    fn ax_caret_coordinates_preserve_negative_and_stacked_displays() {
        assert_eq!(
            super::ax_caret_to_appkit(100.0, 200.0, 20.0, 1080.0),
            (100.0, 860.0)
        );
        assert_eq!(
            super::ax_caret_to_appkit(-900.0, -500.0, 20.0, 1080.0),
            (-900.0, 1560.0)
        );
        assert_eq!(
            super::ax_caret_to_appkit(100.0, 1300.0, 20.0, 1080.0),
            (100.0, -240.0)
        );
    }

    #[test]
    fn blob_preview_collapses_whitespace_and_caps_at_42_characters() {
        assert_eq!(
            super::blob_preview_text("  krótka\n\tnotatka  "),
            "krótka notatka"
        );
        assert_eq!(super::blob_preview_text(" \n\t "), "");
        let exact = "ż".repeat(42);
        assert_eq!(super::blob_preview_text(&exact), exact);
        let long = super::blob_preview_text("To jest odłożona transkrypcja do przeciągnięcia");
        assert!(long.starts_with("To jest odłożona transkrypcja"));
        assert!(long.ends_with('…'));
        assert!(long.chars().count() <= 42, "{long}");
        let unicode = super::blob_preview_text(&"ź".repeat(80));
        assert_eq!(unicode, format!("{}…", "ź".repeat(41)));
    }

    #[test]
    fn appkit_and_cg_points_round_trip_with_the_caret_conversion() {
        assert_eq!(
            super::appkit_to_cg_point(100.0, 860.0, 1080.0),
            (100.0, 220.0)
        );
        assert_eq!(
            super::appkit_to_cg_point(-900.0, 1560.0, 1080.0),
            (-900.0, -480.0)
        );
        // A zero-height caret lands on the same top-left point it came from.
        for (x, y) in [(100.0, 200.0), (-900.0, -500.0), (100.0, 1300.0)] {
            let (ax, ay) = super::ax_caret_to_appkit(x, y, 0.0, 1080.0);
            assert_eq!(super::appkit_to_cg_point(ax, ay, 1080.0), (x, y));
        }
    }

    #[test]
    fn cursor_widget_stays_inside_small_or_negative_screen() {
        assert_eq!(super::fit_origin(-10.0, 200.0, -1000.0, 1000.0), -200.0);
        assert_eq!(super::fit_origin(90.0, 200.0, 0.0, 100.0), 0.0);
    }

    #[test]
    fn dismissed_take_cannot_paint_or_track_the_next_generation() {
        let old_take = 7;
        assert!(super::token_is_current(old_take, 7));
        assert!(!super::token_is_current(old_take, 8));
        assert!(super::token_is_current(8, 8));
    }

    /// Processing pulses orange; Assistive glows purple and is slightly larger.
    #[test]
    fn hold_badge_modes_encode_processing_and_assistive_affordances() {
        assert_eq!(BadgeMode::Processing.color(), (1.0, 0.5, 0.0, 0.85));
        assert!(BadgeMode::Processing.should_pulse());
        assert!(!BadgeMode::Processing.has_glow());
        assert_eq!(BadgeMode::Processing.diameter_multiplier(), 1.0);

        assert_eq!(BadgeMode::Assistive.color(), (0.6, 0.2, 0.9, 0.85));
        assert!(!BadgeMode::Assistive.should_pulse());
        assert!(BadgeMode::Assistive.has_glow());
        assert_eq!(BadgeMode::Assistive.diameter_multiplier(), 1.2);
    }

    /// Maciej, 2026-10-14 and 2026-10-17: Agent mode's dot is purple while the
    /// voice speaks, white between phrases and pulsing purple while its words
    /// are owed — orange belongs to processing.
    /// Dictation keeps its red at rest and its amber, unpulsed, for owed words.
    #[test]
    fn agent_dot_tells_speaking_from_quiet_and_owed_words() {
        let purple = BadgeMode::Assistive.color();
        let agent = |voice| BadgeMode::Assistive.voice_color(purple, voice);
        assert_eq!(agent(CursorVoice::Speaking), purple);
        assert_eq!(
            agent(CursorVoice::Unobserved),
            purple,
            "no observer keeps the mode"
        );
        assert_eq!(
            agent(CursorVoice::Quiet),
            IndicatorState::AgentQuiet.color()
        );
        assert_eq!(
            agent(CursorVoice::Lagging),
            purple,
            "owed Agent words stay purple"
        );
        assert!(BadgeMode::Assistive.voice_pulses(CursorVoice::Lagging));
        assert!(!BadgeMode::Assistive.voice_pulses(CursorVoice::Quiet));
        assert!(!BadgeMode::Assistive.voice_pulses(CursorVoice::Speaking));
        assert_eq!(
            BadgeMode::Assistive.voice_ring(CursorVoice::Quiet),
            Some(purple),
            "the white dot keeps a purple rim over a white page"
        );
        assert_eq!(BadgeMode::Assistive.voice_ring(CursorVoice::Speaking), None);

        let red = BadgeMode::Hold.color();
        assert_eq!(BadgeMode::Hold.voice_color(red, CursorVoice::Quiet), red);
        assert_eq!(
            BadgeMode::Hold.voice_color(red, CursorVoice::Lagging),
            IndicatorState::WordsOwed.color()
        );
        assert!(!BadgeMode::Hold.voice_pulses(CursorVoice::Lagging));
        assert!(BadgeMode::Toggle.voice_pulses(CursorVoice::Quiet));
        assert_eq!(BadgeMode::Hold.voice_ring(CursorVoice::Quiet), None);
    }

    /// from_mode keeps layout fields while swapping color/diameter for the mode.
    #[test]
    fn hold_badge_config_from_mode_preserves_layout_and_applies_mode_visuals() {
        let base = HoldBadgeConfig::default();
        let processing = HoldBadgeConfig::from_mode(BadgeMode::Processing);
        assert_eq!(processing.mode, BadgeMode::Processing);
        assert_eq!(processing.color, BadgeMode::Processing.color());
        assert_eq!(processing.diameter, base.diameter);
        assert_eq!(processing.offset, base.offset);
        assert_eq!(processing.update_interval_ms, base.update_interval_ms);

        let assistive = HoldBadgeConfig::from_mode(BadgeMode::Assistive);
        assert_eq!(assistive.mode, BadgeMode::Assistive);
        assert_eq!(assistive.color, BadgeMode::Assistive.color());
        assert_eq!(
            assistive.diameter,
            base.diameter * BadgeMode::Assistive.diameter_multiplier()
        );
        assert_eq!(assistive.offset, base.offset);
        assert_eq!(assistive.update_interval_ms, base.update_interval_ms);
    }

    /// Next show reads new base diameter; already-visible config stays frozen.
    #[test]
    fn badge_size_change_applies_to_next_show_without_mutating_visible_config() {
        let visible = HoldBadgeConfig::from_mode_with_base_diameter(BadgeMode::Hold, 8.0);
        let next_show = HoldBadgeConfig::from_mode_with_base_diameter(BadgeMode::Hold, 4.0);

        assert_eq!(
            visible.diameter, 8.0,
            "visible badge keeps its captured config"
        );
        assert_eq!(
            next_show.diameter, 4.0,
            "next show reads the new persisted size"
        );
    }
}

// ─────────────────────────────────────────────────────────────
// macOS implementation
// ─────────────────────────────────────────────────────────────

/// macOS badge implementation: AppKit panel, Accessibility caret tracking, and
/// the position/pulse updater thread.
///
/// Panel work must happen on the main thread, so the public entry points
/// dispatch there; the generation gate described on `BADGE_GENERATION` is what
/// keeps a queued show from resurrecting a badge the user already dismissed.
#[cfg(target_os = "macos")]
mod imp {
    use super::{BadgeMode, CursorVoice, FocusedInputField, HoldBadgeConfig, IndicatorState};

    use core_foundation::base::{CFType, CFTypeRef, TCFType};
    use core_foundation::boolean::CFBoolean;
    use core_foundation::string::CFString;
    use core_graphics::event::{CGEvent, CGEventTapLocation, CGEventType, CGMouseButton};
    use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};
    use core_graphics::geometry::{CGPoint, CGRect, CGSize};
    use dispatch::Queue;
    use objc::declare::ClassDecl;
    use objc::runtime::{BOOL, Class, Object, Sel, YES};
    use objc::{msg_send, sel, sel_impl};
    use objc2_app_kit::{
        NSBackingStoreType, NSColor, NSEvent, NSWindowCollectionBehavior, NSWindowStyleMask,
    };
    use std::ptr;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::{Arc, Mutex, OnceLock};
    use std::thread;
    use std::time::{Duration, Instant};
    use tracing::{debug, info, warn};

    use crate::os::Id;
    use crate::os::ax_ffi::{AXUIElementCopyAttributeValue, AXUIElementIsAttributeSettable};

    // Accessibility API bindings (use raw pointers compatible with C FFI)
    /// Opaque `AXUIElementRef` / `AXValueRef` handle.
    ///
    /// Kept as a raw pointer rather than a typed wrapper because these values
    /// cross the C FFI boundary; ownership is manual — every `+1` handle
    /// returned by the AX calls below must be `CFRelease`d.
    type AXId = *mut std::ffi::c_void;

    #[link(name = "ApplicationServices", kind = "framework")]
    unsafe extern "C" {
        fn AXUIElementCopyParameterizedAttributeValue(
            element: AXId,
            attribute: AXId,
            parameter: AXId,
            value: *mut AXId,
        ) -> i32;
        /// Create the system-wide AX element (the root for "whatever is focused
        /// right now"). Returns a `+1` handle. Requires Accessibility trust.
        fn AXUIElementCreateSystemWide() -> AXId;
        /// Owning process of an AX element; identifies our own UI, whose AX
        /// answers execute AppKit accessibility code on the calling thread.
        fn AXUIElementGetPid(element: AXId, pid: *mut i32) -> i32;
        /// Unwrap an `AXValueRef` into a caller-provided struct matching
        /// `type_` (one of the `AX_VALUE_*` constants). `false` on mismatch.
        fn AXValueGetValue(value: AXId, type_: i32, value_ptr: *mut std::ffi::c_void) -> bool;
        /// Bound every later AX request on `element` (never on the
        /// system-wide element: that would set the process-global timeout).
        fn AXUIElementSetMessagingTimeout(element: AXId, seconds: f32) -> i32;
        /// Hit-test the accessibility tree at a top-left global point. Returns
        /// a `+1` element in `element` on success.
        fn AXUIElementCopyElementAtPosition(
            application: AXId,
            x: f32,
            y: f32,
            element: *mut AXId,
        ) -> i32;
        /// Release a `+1` CoreFoundation handle.
        fn CFRelease(cf: *const std::ffi::c_void);
    }

    #[link(name = "Carbon", kind = "framework")]
    unsafe extern "C" {
        /// True while any process holds secure event input — a focused
        /// password field, or a terminal password prompt with secure entry.
        fn IsSecureEventInputEnabled() -> u8;
    }

    // CGColor functions
    #[link(name = "CoreGraphics", kind = "framework")]
    unsafe extern "C" {
        fn CGMainDisplayID() -> u32;
        fn CGDisplayBounds(display: u32) -> CGRect;
        /// Create a `+1` `CGColorRef` from `components` in `space`.
        fn CGColorCreate(
            space: *const std::ffi::c_void,
            components: *const f64,
        ) -> *const std::ffi::c_void;
        /// Create a `+1` device RGB color space.
        fn CGColorSpaceCreateDeviceRGB() -> *const std::ffi::c_void;
        /// Release a color space obtained above.
        fn CGColorSpaceRelease(space: *const std::ffi::c_void);
        /// Release a color obtained above.
        fn CGColorRelease(color: *const std::ffi::c_void);
    }

    // AX constants
    /// `kAXErrorSuccess` — the only return value treated as a usable result.
    const AX_ERROR_SUCCESS: i32 = 0;
    /// Attribute naming the element that currently has keyboard focus.
    const AX_FOCUSED_UIELEMENT_ATTRIBUTE: &str = "AXFocusedUIElement";
    /// Attribute naming an element's role, matched against the text-input roles
    /// in `classify_input_role`.
    const AX_ROLE_ATTRIBUTE: &str = "AXRole";
    /// Attribute naming an element's subrole (`AXSecureTextField` for passwords).
    const AX_SUBROLE_ATTRIBUTE: &str = "AXSubrole";
    /// Attribute holding the current selection/caret range.
    const AX_SELECTED_TEXT_RANGE_ATTRIBUTE: &str = "AXSelectedTextRange";

    // Window level constants
    /// `NSStatusWindowLevel`. Sits above the floating level (3), which is what
    /// keeps the badge on top while it stays a non-activating panel.
    const NS_STATUS_WINDOW_LEVEL: i64 = 25;

    // ── inlined AppKit helpers (were `ui::shared::helpers::shell`) ──

    /// Add subview to a view.
    /// # Safety
    /// `parent` and `child` must be valid Objective-C views.
    unsafe fn add_subview(parent: Id, child: Id) {
        unsafe {
            let _: () = msg_send![parent, addSubview: child];
        }
    }

    /// Show panel (order front, even when app is inactive).
    /// # Safety
    /// `panel` must be a valid `NSPanel` / `NSWindow` instance.
    unsafe fn panel_show(panel: Id) {
        unsafe {
            let _: () = msg_send![panel, orderFrontRegardless];
        }
    }

    /// Close panel.
    /// # Safety
    /// `panel` must be a valid `NSPanel` / `NSWindow` instance.
    unsafe fn panel_close(panel: Id) {
        unsafe {
            let _: () = msg_send![panel, close];
            let _: () = msg_send![panel, release];
        }
    }

    /// Hold badge state
    struct HoldBadgeState {
        window: Option<usize>, // Stores the NSPanel pointer as usize to make it Send.
        timer_running: bool,
        config: HoldBadgeConfig,
        voice: CursorVoice,
        preview_updated_at: Option<Instant>,
        /// Only the updater thread spawned with this epoch may keep running;
        /// a take that restarts the updater retires the previous one.
        updater_epoch: u64,
        /// The panel is (or is about to become) the draggable deferred-insert
        /// capsule. Set at request time so `end_recording_badge` keeps it even
        /// when the paint is still queued on the main thread.
        deferred_blob_active: bool,
        /// The capsule paint has run on the main thread. Only a painted
        /// capsule turns into a new take on the next show; a show queued
        /// before the capsule request stays a mode transition.
        deferred_blob_painted: bool,
        /// Bumped per capsule request; TTL expiry and drop results act only
        /// on the capsule they belong to.
        blob_epoch: u64,
        drag_start_mouse: (f64, f64),
        drag_start_origin: (f64, f64),
        is_dragging: bool,
    }

    lazy_static::lazy_static! {
        /// Process-wide badge state shared by the show/hide entry points and the
        /// updater thread.
        ///
        /// Never hold this lock across `panel_close`: closing a panel can
        /// re-enter this module through AppKit notifications and deadlock. The
        /// updater uses `try_lock` so a busy tick is skipped rather than
        /// blocking the UI.
        static ref BADGE_STATE: Arc<Mutex<HoldBadgeState>> = Arc::new(Mutex::new(HoldBadgeState {
            window: None,
            timer_running: false,
            config: HoldBadgeConfig::default(),
            voice: CursorVoice::Unobserved,
            preview_updated_at: None,
            updater_epoch: 0,
            deferred_blob_active: false,
            deferred_blob_painted: false,
            blob_epoch: 0,
            drag_start_mouse: (0.0, 0.0),
            drag_start_origin: (0.0, 0.0),
            is_dragging: false,
        }));
    }

    /// Monotonic show generation. A show captures it at REQUEST time; `hide_hold_badge`
    /// bumps it. When a queued (`exec_async`) show finally runs on the main thread it
    /// aborts if the generation moved — otherwise a hide that raced ahead of the
    /// enqueued show would be undone, leaving a badge panel + updater thread stuck
    /// with nothing to tear them down.
    static BADGE_GENERATION: AtomicU64 = AtomicU64::new(0);

    /// Token captured by the take, never by an arriving late projection.
    pub fn take_token() -> u64 {
        BADGE_GENERATION.load(Ordering::SeqCst)
    }

    /// Paint a supplied projection in-place; never show or resurrect a panel.
    pub fn update_transcript(token: u64, text: &str) {
        let text = super::preview_tail(text);
        let received_at = Instant::now();
        Queue::main().exec_async(move || {
            if !super::token_is_current(token, take_token()) {
                return;
            }
            let (panel, diameter, mode, base_color, voice) = {
                let mut state = BADGE_STATE.lock().unwrap_or_else(|e| e.into_inner());
                let Some(panel) = state.window else {
                    return;
                };
                // A late projection of the ended take never repaints the
                // armed capsule's label.
                if state.deferred_blob_active {
                    return;
                }
                state.preview_updated_at = (!text.is_empty()).then_some(received_at);
                (
                    panel as Id,
                    state.config.diameter,
                    state.config.mode,
                    state.config.color,
                    state.voice,
                )
            };
            unsafe {
                let fresh = if super::preview_expired(Some(received_at), Instant::now()) {
                    ""
                } else {
                    &text
                };
                paint_preview(panel, diameter, mode, base_color, fresh, voice, false);
            }
        });
    }

    /// Voice changes repaint only the dot, without refreshing the Apple label.
    pub fn update_voice_state(token: u64, voice: CursorVoice) {
        Queue::main().exec_async(move || {
            if !super::token_is_current(token, take_token()) {
                return;
            }
            let (panel, mode, base_color) = {
                let mut state = BADGE_STATE.lock().unwrap_or_else(|e| e.into_inner());
                let Some(panel) = state.window else {
                    return;
                };
                if state.voice == voice || state.deferred_blob_active {
                    return;
                }
                state.voice = voice;
                (panel as Id, state.config.mode, state.config.color)
            };
            unsafe { paint_voice_state(panel, mode, base_color, voice) };
        });
    }

    /// A stale label can expire even when the mouse is stationary and the dot does not pulse.
    fn expire_transcript(token: u64, window_ptr: usize) {
        Queue::main().exec_async(move || {
            let paint = {
                let mut state = BADGE_STATE.lock().unwrap_or_else(|e| e.into_inner());
                if !super::token_is_current(token, take_token())
                    || state.window != Some(window_ptr)
                    || state.deferred_blob_active
                    || !super::preview_expired(state.preview_updated_at, Instant::now())
                {
                    return;
                }
                state.preview_updated_at = None;
                (
                    state.config.diameter,
                    state.config.mode,
                    state.config.color,
                    state.voice,
                )
            };
            unsafe {
                paint_preview(
                    window_ptr as Id,
                    paint.0,
                    paint.1,
                    paint.2,
                    "",
                    paint.3,
                    false,
                )
            };
        });
    }

    /// Light label foreground on the dark capsule.
    const LABEL_RGBA: (f64, f64, f64, f64) = (0.96, 0.96, 0.98, 1.0);
    /// Dark glass capsule fill.
    const CAPSULE_FILL: (f64, f64, f64, f64) = (0.09, 0.09, 0.11, 0.94);
    /// Hairline rim during a live preview.
    const CAPSULE_RIM_LIVE: (f64, f64, f64, f64) = (1.0, 1.0, 1.0, 0.14);
    /// Amber rim while an armed transcript waits in the capsule.
    const CAPSULE_RIM_DEFERRED: (f64, f64, f64, f64) = (1.0, 0.62, 0.0, 0.45);
    const CAPSULE_PAD_X: f64 = 10.0;
    const CAPSULE_PAD_Y: f64 = 5.0;
    const CAPSULE_DOT_GAP: f64 = 7.0;
    const CAPSULE_LABEL_MAX_WIDTH: f64 = 360.0;
    const DISMISS_GAP: f64 = 4.0;
    const DISMISS_WIDTH: f64 = 16.0;
    /// Right-edge hit zone of the `×` glyph: its box plus the trailing pad.
    const DISMISS_HIT_WIDTH: f64 = DISMISS_WIDTH + CAPSULE_PAD_X;
    /// Pointer travel (points) before a press on the capsule becomes a drag.
    const DRAG_THRESHOLD: f64 = 3.0;

    /// Set a `CALayer` color property from RGBA without leaking the CGColor.
    ///
    /// # Safety
    /// `layer` must be a live `CALayer`; main thread only.
    unsafe fn set_layer_color(layer: Id, background: bool, rgba: (f64, f64, f64, f64)) {
        unsafe {
            let cg = create_cg_color(rgba.0, rgba.1, rgba.2, rgba.3);
            if background {
                let _: () = msg_send![layer, setBackgroundColor: cg];
            } else {
                let _: () = msg_send![layer, setBorderColor: cg];
            }
            CGColorRelease(cg);
        }
    }

    /// Main-thread painting shared by text updates, mode changes and the
    /// deferred-insert capsule. Subview order is a contract: `[0]` the dot,
    /// `[1]` the preview label (its `stringValue` is the exact preview), `[2]`
    /// the `×` dismiss glyph shown only on the armed capsule.
    unsafe fn paint_preview(
        panel: Id,
        diameter: f64,
        mode: BadgeMode,
        base_color: (f64, f64, f64, f64),
        text: &str,
        voice: CursorVoice,
        deferred: bool,
    ) {
        unsafe {
            let content: Id = msg_send![panel, contentView];
            let children: Id = msg_send![content, subviews];
            let count: usize = msg_send![children, count];
            if count < 2 {
                return;
            }
            let dot: Id = msg_send![children, objectAtIndex: 0usize];
            let label: Id = msg_send![children, objectAtIndex: 1usize];
            let dismiss: Option<Id> =
                (count > 2).then(|| msg_send![children, objectAtIndex: 2usize]);
            let value = CFString::new(text);
            let _: () = msg_send![label, setStringValue: value.as_concrete_TypeRef() as Id];
            let _: () = msg_send![label, setHidden: text.is_empty()];
            if let Some(dismiss) = dismiss {
                let _: () = msg_send![dismiss, setHidden: !deferred];
            }
            let content_layer: Id = msg_send![content, layer];
            let size = if text.is_empty() && !deferred {
                let _: () = msg_send![dot, setFrameOrigin: CGPoint { x: 0.0, y: 0.0 }];
                if !content_layer.is_null() {
                    set_layer_color(content_layer, true, (0.0, 0.0, 0.0, 0.0));
                    let _: () = msg_send![content_layer, setBorderWidth: 0.0f64];
                    let _: () = msg_send![content_layer, setCornerRadius: 0.0f64];
                }
                CGSize {
                    width: diameter,
                    height: diameter,
                }
            } else {
                let _: () = msg_send![label, sizeToFit];
                let label_frame: CGRect = msg_send![label, frame];
                let label_width = if text.is_empty() {
                    0.0
                } else {
                    label_frame.size.width.min(CAPSULE_LABEL_MAX_WIDTH)
                };
                let label_height = label_frame.size.height;
                let pill_height = (label_height + CAPSULE_PAD_Y * 2.0).max(diameter + 10.0);
                let mut x = CAPSULE_PAD_X;
                let dot_origin = CGPoint {
                    x,
                    y: (pill_height - diameter) / 2.0,
                };
                let _: () = msg_send![dot, setFrameOrigin: dot_origin];
                x += diameter;
                if !text.is_empty() {
                    x += CAPSULE_DOT_GAP;
                    let frame = CGRect {
                        origin: CGPoint {
                            x,
                            y: (pill_height - label_height) / 2.0,
                        },
                        size: CGSize {
                            width: label_width,
                            height: label_height,
                        },
                    };
                    let _: () = msg_send![label, setFrame: frame];
                    x += label_width;
                }
                if deferred && let Some(dismiss) = dismiss {
                    x += DISMISS_GAP;
                    let _: () = msg_send![dismiss, sizeToFit];
                    let glyph: CGRect = msg_send![dismiss, frame];
                    let glyph_width = glyph.size.width.min(DISMISS_WIDTH);
                    let frame = CGRect {
                        origin: CGPoint {
                            x: x + (DISMISS_WIDTH - glyph_width) / 2.0,
                            y: (pill_height - glyph.size.height) / 2.0,
                        },
                        size: CGSize {
                            width: glyph_width,
                            height: glyph.size.height,
                        },
                    };
                    let _: () = msg_send![dismiss, setFrame: frame];
                    x += DISMISS_WIDTH;
                }
                x += CAPSULE_PAD_X;
                if !content_layer.is_null() {
                    set_layer_color(content_layer, true, CAPSULE_FILL);
                    let _: () = msg_send![content_layer, setCornerRadius: pill_height / 2.0];
                    let _: () = msg_send![content_layer, setMasksToBounds: true];
                    let _: () = msg_send![content_layer, setBorderWidth: 1.0f64];
                    let rim = if deferred {
                        CAPSULE_RIM_DEFERRED
                    } else {
                        CAPSULE_RIM_LIVE
                    };
                    set_layer_color(content_layer, false, rim);
                }
                CGSize {
                    width: x,
                    height: pill_height,
                }
            };
            let _: () = msg_send![panel, setContentSize: size];
            let panel_frame: CGRect = msg_send![panel, frame];
            let origin = fit_panel_origin(panel, panel_frame.origin.x, panel_frame.origin.y);
            let _: () = msg_send![panel, setFrameOrigin: origin];
            if deferred {
                // The armed capsule wears the owed-words amber, steady.
                let layer: Id = msg_send![dot, layer];
                if !layer.is_null() {
                    set_layer_color(layer, true, IndicatorState::WordsOwed.color());
                    let _: () = msg_send![layer, setBorderWidth: 0.0f64];
                    let _: () = msg_send![layer, setOpacity: 1.0f32];
                }
            } else {
                paint_voice_state(panel, mode, base_color, voice);
            }
        }
    }

    /// A steady state ends the pulse at full opacity; the updater resumes it.
    unsafe fn paint_voice_state(
        panel: Id,
        mode: BadgeMode,
        base_color: (f64, f64, f64, f64),
        voice: CursorVoice,
    ) {
        unsafe {
            let content: Id = msg_send![panel, contentView];
            let children: Id = msg_send![content, subviews];
            let count: usize = msg_send![children, count];
            if count == 0 {
                return;
            }
            let dot: Id = msg_send![children, objectAtIndex: 0usize];
            let color = mode.voice_color(base_color, voice);
            let cg = create_cg_color(color.0, color.1, color.2, color.3);
            let layer: Id = msg_send![dot, layer];
            let _: () = msg_send![layer, setBackgroundColor: cg];
            CGColorRelease(cg);
            if let Some(ring) = mode.voice_ring(voice) {
                let cg = create_cg_color(ring.0, ring.1, ring.2, 1.0);
                let _: () = msg_send![layer, setBorderColor: cg];
                CGColorRelease(cg);
                let _: () = msg_send![layer, setBorderWidth: 1.0f64];
            } else {
                let _: () = msg_send![layer, setBorderWidth: 0.0f64];
            }
            if !mode.voice_pulses(voice) {
                let _: () = msg_send![layer, setOpacity: 1.0f32];
            }
        }
    }

    /// Copy one string attribute off `element`; `None` when it is absent,
    /// unreadable, or not a string.
    ///
    /// # Safety
    /// `element` must be a live AX element handle owned by the caller.
    unsafe fn copy_string_attribute(element: AXId, name: &str) -> Option<String> {
        unsafe {
            let mut value: AXId = ptr::null_mut();
            let attribute = CFString::new(name);
            let result = AXUIElementCopyAttributeValue(
                element,
                attribute.as_concrete_TypeRef() as AXId,
                &mut value,
            );
            if result != AX_ERROR_SUCCESS || value.is_null() {
                return None;
            }
            // Create rule: the wrapper owns the +1 and releases it on drop.
            CFType::wrap_under_create_rule(value as CFTypeRef)
                .downcast::<CFString>()
                .map(|text| text.to_string())
        }
    }

    /// Classify capability on the same retained AX identity used by transport.
    ///
    /// # Safety
    /// `element` must be a live retained AXUIElement owned by the caller.
    pub(crate) unsafe fn input_field_for_element(element: AXId) -> FocusedInputField {
        unsafe {
            if IsSecureEventInputEnabled() != 0 {
                return FocusedInputField::Secure;
            }
            let role = copy_string_attribute(element, AX_ROLE_ATTRIBUTE);
            let subrole = copy_string_attribute(element, AX_SUBROLE_ATTRIBUTE);
            let mut enabled_value: AXId = ptr::null_mut();
            let enabled_attribute = CFString::new("AXEnabled");
            let enabled_result = AXUIElementCopyAttributeValue(
                element,
                enabled_attribute.as_concrete_TypeRef() as AXId,
                &mut enabled_value,
            );
            let enabled = if enabled_result == AX_ERROR_SUCCESS && !enabled_value.is_null() {
                CFType::wrap_under_create_rule(enabled_value as CFTypeRef)
                    .downcast::<CFBoolean>()
                    .is_some_and(|value| value.into())
            } else {
                false
            };
            let writable = ["AXValue", "AXSelectedText"].iter().any(|name| {
                let attribute = CFString::new(name);
                let mut settable = 0;
                AXUIElementIsAttributeSettable(
                    element,
                    attribute.as_concrete_TypeRef() as AXId,
                    &mut settable,
                ) == AX_ERROR_SUCCESS
                    && settable != 0
            });
            match role {
                Some(role) => {
                    super::classify_input_capability(&role, subrole.as_deref(), enabled, writable)
                }
                None => FocusedInputField::Unobserved,
            }
        }
    }

    /// Text-input shape of the element holding keyboard focus right now.
    ///
    /// Secure event input wins first: while any password prompt owns the
    /// keyboard, nothing is classified as an ordinary text field.
    pub fn focused_input_field() -> FocusedInputField {
        // SAFETY: every AX Create/Copy result is released exactly once (the
        // string reads own theirs); output pointers are valid locals.
        unsafe {
            if IsSecureEventInputEnabled() != 0 {
                return FocusedInputField::Secure;
            }
            let system_wide = AXUIElementCreateSystemWide();
            if system_wide.is_null() {
                return FocusedInputField::Unobserved;
            }
            let mut focused: AXId = ptr::null_mut();
            let attribute = CFString::new(AX_FOCUSED_UIELEMENT_ATTRIBUTE);
            let result = AXUIElementCopyAttributeValue(
                system_wide,
                attribute.as_concrete_TypeRef() as AXId,
                &mut focused,
            );
            CFRelease(system_wide);
            if result != AX_ERROR_SUCCESS || focused.is_null() {
                return FocusedInputField::Unobserved;
            }
            // A stalled host must not hold the stop path for the default AX
            // timeout while the role and subrole are read.
            AXUIElementSetMessagingTimeout(focused, 0.1);
            let field = input_field_for_element(focused);
            CFRelease(focused);
            field
        }
    }

    /// Get the current text caret position in screen coordinates
    pub fn get_caret_position() -> Option<(f64, f64)> {
        unsafe {
            let system = AXUIElementCreateSystemWide();
            if system.is_null() {
                return None;
            }
            let mut focused: AXId = ptr::null_mut();
            let attr = CFString::new(AX_FOCUSED_UIELEMENT_ATTRIBUTE);
            let result = AXUIElementCopyAttributeValue(
                system,
                attr.as_concrete_TypeRef() as AXId,
                &mut focused,
            );
            CFRelease(system);
            if result != AX_ERROR_SUCCESS || focused.is_null() {
                return None;
            }
            // An in-process focused element answers AX queries by running
            // AppKit accessibility code on THIS background thread; a
            // MainActor-isolated view getter then traps
            // (dispatch_assert_queue → SIGTRAP) and kills the app. Our own
            // windows fall back to the cursor position instead.
            let mut focused_pid: i32 = 0;
            if AXUIElementGetPid(focused, &mut focused_pid) != AX_ERROR_SUCCESS
                || focused_pid == std::process::id() as i32
            {
                CFRelease(focused);
                return None;
            }
            // One slow host must not stall the anchor thread for the default
            // AX timeout while the selection and its bounds are read.
            AXUIElementSetMessagingTimeout(focused, 0.05);
            let mut range: AXId = ptr::null_mut();
            let attr = CFString::new(AX_SELECTED_TEXT_RANGE_ATTRIBUTE);
            let result = AXUIElementCopyAttributeValue(
                focused,
                attr.as_concrete_TypeRef() as AXId,
                &mut range,
            );
            if result != AX_ERROR_SUCCESS || range.is_null() {
                if !range.is_null() {
                    CFRelease(range);
                }
                CFRelease(focused);
                return None;
            }
            let mut bounds: AXId = ptr::null_mut();
            let attr = CFString::new("AXBoundsForRange");
            let result = AXUIElementCopyParameterizedAttributeValue(
                focused,
                attr.as_concrete_TypeRef() as AXId,
                range,
                &mut bounds,
            );
            CFRelease(range);
            CFRelease(focused);
            if result != AX_ERROR_SUCCESS || bounds.is_null() {
                if !bounds.is_null() {
                    CFRelease(bounds);
                }
                return None;
            }
            let mut rect = CGRect::new(&CGPoint::new(0.0, 0.0), &CGSize::new(0.0, 0.0));
            let valid = AXValueGetValue(bounds, 3, &mut rect as *mut _ as *mut std::ffi::c_void);
            CFRelease(bounds);
            if !valid {
                return None;
            }
            let display = CGDisplayBounds(CGMainDisplayID());
            Some(super::ax_caret_to_appkit(
                rect.origin.x,
                rect.origin.y,
                rect.size.height,
                display.size.height,
            ))
        }
    }

    /// Get the current mouse cursor position in screen coordinates
    pub fn get_cursor_position() -> (f64, f64) {
        let mouse_location = NSEvent::mouseLocation();
        (mouse_location.x, mouse_location.y)
    }

    /// Anchor the panel once at the caret, off the main thread.
    ///
    /// The panel already sits at the cursor; this single AX lookup moves it to
    /// the caret when the focused host exposes one, and never runs again for
    /// the take. Nothing polls the mouse or the AX server afterwards, so the
    /// badge stays where the words are going instead of chasing the pointer.
    fn spawn_caret_anchor(generation: u64, window_ptr: usize, offset: (f64, f64)) {
        let spawned = thread::Builder::new()
            .name("hold-badge-caret".into())
            .spawn(move || {
                let Some((caret_x, caret_y)) = get_caret_position() else {
                    return;
                };
                if !super::token_is_current(generation, take_token()) {
                    return;
                }
                Queue::main().exec_async(move || {
                    {
                        let state = BADGE_STATE.lock().unwrap_or_else(|e| e.into_inner());
                        if !super::token_is_current(generation, take_token())
                            || state.window != Some(window_ptr)
                            || state.is_dragging
                        {
                            return;
                        }
                    }
                    let panel = window_ptr as Id;
                    unsafe {
                        let origin =
                            fit_panel_origin(panel, caret_x + offset.0, caret_y + offset.1);
                        let _: () = msg_send![panel, setFrameOrigin: origin];
                    }
                });
            });
        if let Err(error) = spawned {
            debug!("hold-badge caret anchor unavailable: {error}");
        }
    }

    /// Place `panel` at the cursor plus the configured offset, fitted to the
    /// screen under it. Main thread only.
    unsafe fn place_at_cursor(panel: Id, offset: (f64, f64)) {
        unsafe {
            let (x, y) = get_cursor_position();
            let origin = fit_panel_origin(panel, x + offset.0, y + offset.1);
            let _: () = msg_send![panel, setFrameOrigin: origin];
        }
    }

    /// Called only on the main queue, where AppKit owns screen and panel geometry.
    unsafe fn fit_panel_origin(panel: Id, x: f64, y: f64) -> CGPoint {
        unsafe {
            let screens: Id = msg_send![Class::get("NSScreen").unwrap(), screens];
            let count: usize = msg_send![screens, count];
            let panel_frame: CGRect = msg_send![panel, frame];
            for index in 0..count {
                let screen: Id = msg_send![screens, objectAtIndex: index];
                let frame: CGRect = msg_send![screen, frame];
                if x >= frame.origin.x
                    && x <= frame.origin.x + frame.size.width
                    && y >= frame.origin.y
                    && y <= frame.origin.y + frame.size.height
                {
                    let visible: CGRect = msg_send![screen, visibleFrame];
                    return CGPoint {
                        x: super::fit_origin(
                            x,
                            panel_frame.size.width,
                            visible.origin.x,
                            visible.size.width,
                        ),
                        y: super::fit_origin(
                            y,
                            panel_frame.size.height,
                            visible.origin.y,
                            visible.size.height,
                        ),
                    };
                }
            }
            CGPoint { x, y }
        }
    }

    /// Create a CGColor from RGBA components
    /// # Safety
    /// Returns a `+1` retained `CGColorRef`; caller must `CGColorRelease` it.
    unsafe fn create_cg_color(r: f64, g: f64, b: f64, a: f64) -> *const std::ffi::c_void {
        unsafe {
            let color_space = CGColorSpaceCreateDeviceRGB();
            let components: [f64; 4] = [r, g, b, a];
            let color = CGColorCreate(color_space, components.as_ptr());
            CGColorSpaceRelease(color_space);
            color
        }
    }

    /// Create the circular badge view using CALayer for reliable rendering
    /// # Safety
    /// Must run on the main thread; returns a `+1` retained `NSView`.
    unsafe fn create_badge_view(config: &HoldBadgeConfig) -> Id {
        unsafe {
            // Use a plain NSView with a CALayer for drawing
            let ns_view = Class::get("NSView").unwrap();
            let view: Id = msg_send![ns_view, alloc];
            let frame = CGRect {
                origin: CGPoint { x: 0.0, y: 0.0 },
                size: CGSize {
                    width: config.diameter,
                    height: config.diameter,
                },
            };
            let view: Id = msg_send![view, initWithFrame: frame];

            // Enable layer-backing
            let _: () = msg_send![view, setWantsLayer: true];

            // Get the layer
            let layer: Id = msg_send![view, layer];
            if layer.is_null() {
                warn!("Badge layer is null - badge will not be visible");
                return view;
            }

            // Configure the layer to draw a circle.
            // Set background color from config (default: red with 80% opacity).
            let cg_color = create_cg_color(
                config.color.0,
                config.color.1,
                config.color.2,
                config.color.3,
            );
            let _: () = msg_send![layer, setBackgroundColor: cg_color];
            CGColorRelease(cg_color);

            // Make it circular by setting corner radius to half the diameter
            let corner_radius = config.diameter / 2.0;
            let _: () = msg_send![layer, setCornerRadius: corner_radius];

            // Ensure the layer clips to bounds (for the circle shape)
            let _: () = msg_send![layer, setMasksToBounds: true];

            view
        }
    }

    /// Create the hold badge panel
    /// # Safety
    /// Must run on the main thread; returns a `+1` retained non-activating `NSPanel`.
    unsafe fn create_badge_panel(config: &HoldBadgeConfig) -> Id {
        unsafe {
            let ns_panel = Class::get("NSPanel").unwrap();

            // Get initial position
            // Initial paint must not synchronously query another app's AX server.
            let (x, y) = get_cursor_position();
            let adjusted_x = x + config.offset.0;
            let adjusted_y = y + config.offset.1;
            debug!(
                "Badge position: raw=({:.1}, {:.1}), adjusted=({:.1}, {:.1}), diameter={}",
                x, y, adjusted_x, adjusted_y, config.diameter
            );

            // Create panel frame using CGRect (screen coordinates)
            let panel_frame = CGRect {
                origin: CGPoint {
                    x: adjusted_x,
                    y: adjusted_y,
                },
                size: CGSize {
                    width: config.diameter,
                    height: config.diameter,
                },
            };

            // Create a non-activating panel so the badge never steals app/key focus.
            let panel: Id = msg_send![ns_panel, alloc];
            let style_mask = NSWindowStyleMask::Borderless | NSWindowStyleMask::NonactivatingPanel;
            let backing = NSBackingStoreType::Buffered;
            let panel: Id = msg_send![
                panel,
                initWithContentRect: panel_frame
                styleMask: style_mask
                backing: backing
                defer: false
            ];
            let _: () = msg_send![panel, setReleasedWhenClosed: false];

            // Configure panel for floating transparent overlay.
            let clear_color = NSColor::clearColor();
            let clear_color_ptr = &*clear_color as *const _ as Id;
            let _: () = msg_send![panel, setOpaque: false];
            let _: () = msg_send![panel, setBackgroundColor: clear_color_ptr];
            let _: () = msg_send![panel, setIgnoresMouseEvents: true];
            let _: () = msg_send![panel, setHidesOnDeactivate: false];
            let _: () = msg_send![panel, setBecomesKeyOnlyIfNeeded: true];
            let _: () = msg_send![panel, setFloatingPanel: true];
            // Status-window level (25) is above floating level (3), preserving the
            // previous always-on-top behavior while keeping this surface a panel.
            let _: () = msg_send![panel, setLevel: NS_STATUS_WINDOW_LEVEL];
            // Ensure the helper panel shows over fullscreen Spaces and stays out of cycling.
            let collection_behavior = NSWindowCollectionBehavior::CanJoinAllSpaces
                | NSWindowCollectionBehavior::FullScreenAuxiliary
                | NSWindowCollectionBehavior::IgnoresCycle;
            let _: () = msg_send![panel, setCollectionBehavior: collection_behavior];

            // The draggable capsule's mouse handling lives on the content view;
            // the panel stays non-activating, so a press never steals focus.
            let view_class = badge_view_class();
            let content_view: Id = msg_send![view_class, alloc];
            let content_frame = CGRect {
                origin: CGPoint { x: 0.0, y: 0.0 },
                size: panel_frame.size,
            };
            let content_view: Id = msg_send![content_view, initWithFrame: content_frame];
            let _: () = msg_send![panel, setContentView: content_view];
            let _: () = msg_send![content_view, release];
            // Enable layer-backed views for better transparency/compositing
            let _: () = msg_send![content_view, setWantsLayer: true];

            // Create badge view (circular colored indicator)
            let badge_view = create_badge_view(config);
            add_subview(content_view, badge_view);

            // Force the view to display
            let _: () = msg_send![badge_view, setNeedsDisplay: true];
            let _: () = msg_send![badge_view, release];

            let empty = CFString::new("");
            let label_class = Class::get("NSTextField").unwrap();
            let label: Id =
                msg_send![label_class, labelWithString: empty.as_concrete_TypeRef() as Id];
            style_capsule_text(label, 12.5, LABEL_RGBA);
            // NSLineBreakByTruncatingTail: one line, cut at the capsule edge.
            let _: () = msg_send![label, setLineBreakMode: 4usize];
            let _: () = msg_send![label, setMaximumNumberOfLines: 1isize];
            let _: () = msg_send![label, setHidden: true];
            add_subview(content_view, label);

            let glyph = CFString::new("×");
            let dismiss: Id =
                msg_send![label_class, labelWithString: glyph.as_concrete_TypeRef() as Id];
            style_capsule_text(dismiss, 13.0, (1.0, 1.0, 1.0, 0.65));
            let _: () = msg_send![dismiss, setHidden: true];
            add_subview(content_view, dismiss);

            panel
        }
    }

    /// Medium system font and an sRGB foreground for a capsule label.
    ///
    /// # Safety
    /// `label` must be a live `NSTextField`; main thread only.
    unsafe fn style_capsule_text(label: Id, size: f64, rgba: (f64, f64, f64, f64)) {
        unsafe {
            // NSFontWeightMedium.
            let font: Id = msg_send![
                Class::get("NSFont").unwrap(),
                systemFontOfSize: size
                weight: 0.23f64
            ];
            if !font.is_null() {
                let _: () = msg_send![label, setFont: font];
            }
            let color: Id = msg_send![
                Class::get("NSColor").unwrap(),
                colorWithSRGBRed: rgba.0
                green: rgba.1
                blue: rgba.2
                alpha: rgba.3
            ];
            if !color.is_null() {
                let _: () = msg_send![label, setTextColor: color];
            }
        }
    }

    /// Show the hold badge and start position tracking (default: Hold mode)
    pub fn show_hold_badge() {
        show_hold_badge_with_config(HoldBadgeConfig::default());
    }

    /// Internal implementation that must run on the main thread.
    ///
    /// `generation` is the show generation captured when this show was requested.
    /// If a `hide_hold_badge` bumped it in the meantime, the show is stale and is
    /// aborted so it cannot resurrect a badge the user already dismissed.
    fn show_hold_badge_impl(config: HoldBadgeConfig, generation: u64) {
        // A hide issued after this show was enqueued already tore the badge down;
        // honor it and do not create a new panel/updater.
        if generation != BADGE_GENERATION.load(Ordering::SeqCst) {
            debug!("Skipping stale hold-badge show (superseded by hide)");
            return;
        }
        debug!("Showing hold badge (diameter={})", config.diameter);
        // A mode transition belongs to the same take. Keep its existing label
        // and window; recreating the panel loses the last projection at Stop.
        // A painted deferred-insert capsule is different: the next show is a
        // new take, which reclaims the same panel as a live badge.
        let existing = {
            let mut state = BADGE_STATE.lock().unwrap_or_else(|e| e.into_inner());
            if generation != BADGE_GENERATION.load(Ordering::SeqCst) {
                return;
            }
            let reclaim = state.deferred_blob_painted;
            state.window.map(|panel| {
                state.config = config.clone();
                let updater_epoch = reclaim.then(|| {
                    state.deferred_blob_active = false;
                    state.deferred_blob_painted = false;
                    state.is_dragging = false;
                    state.voice = CursorVoice::Unobserved;
                    state.preview_updated_at = None;
                    state.timer_running = true;
                    state.updater_epoch += 1;
                    state.updater_epoch
                });
                (panel, state.voice, updater_epoch)
            })
        };
        if let Some((panel_ptr, voice, reclaimed)) = existing {
            let panel = panel_ptr as Id;
            unsafe {
                let content: Id = msg_send![panel, contentView];
                let children: Id = msg_send![content, subviews];
                let dot: Id = msg_send![children, objectAtIndex: 0usize];
                let text = if reclaimed.is_some() {
                    // The armed transcript is not this take's preview.
                    String::new()
                } else {
                    let label: Id = msg_send![children, objectAtIndex: 1usize];
                    let value: Id = msg_send![label, stringValue];
                    CFString::wrap_under_get_rule(value.cast()).to_string()
                };
                let size = CGSize {
                    width: config.diameter,
                    height: config.diameter,
                };
                let _: () = msg_send![dot, setFrameSize: size];
                let layer: Id = msg_send![dot, layer];
                let _: () = msg_send![layer, setCornerRadius: config.diameter / 2.0];
                let _: () = msg_send![layer, setOpacity: 1.0f32];
                if reclaimed.is_some() {
                    let _: () = msg_send![panel, setIgnoresMouseEvents: true];
                }
                paint_preview(
                    panel,
                    config.diameter,
                    config.mode,
                    config.color,
                    &text,
                    voice,
                    false,
                );
                if reclaimed.is_some() {
                    place_at_cursor(panel, config.offset);
                    // A drop in flight may have ordered the capsule out.
                    panel_show(panel);
                }
            }
            if let Some(updater_epoch) = reclaimed {
                spawn_caret_anchor(generation, panel_ptr, config.offset);
                spawn_updater(generation, updater_epoch, config.update_interval_ms);
            }
            return;
        }
        unsafe {
            // Create new badge panel (MUST be on main thread)
            let panel = create_badge_panel(&config);
            place_at_cursor(panel, config.offset);

            // Make panel visible without activating the app.
            panel_show(panel);

            // Force content view to redraw
            let content_view: Id = msg_send![panel, contentView];
            let _: () = msg_send![content_view, setNeedsDisplay: true];

            // Update shared state; every new panel gets its own updater epoch.
            let updater_epoch;
            {
                let mut state = BADGE_STATE.lock().unwrap_or_else(|e| e.into_inner());
                // Re-check under the lock: a hide that landed between the top-of-fn
                // check and here bumped the generation. Honor it — drop the lock,
                // close the panel we just created (never hold BADGE_STATE across
                // panel_close, see above), and leave the state torn down.
                if generation != BADGE_GENERATION.load(Ordering::SeqCst) {
                    drop(state);
                    panel_close(panel);
                    debug!("Aborting hold-badge show; hide raced in during panel creation");
                    return;
                }
                state.window = Some(panel as usize);
                state.config = config.clone();
                state.timer_running = true;
                state.voice = CursorVoice::Unobserved;
                state.preview_updated_at = None;
                // A capsule requested after this show was queued keeps its
                // claim: its paint runs next and takes over this panel.
                state.is_dragging = false;
                state.updater_epoch += 1;
                updater_epoch = state.updater_epoch;
            }
            spawn_caret_anchor(generation, panel as usize, config.offset);
            spawn_updater(generation, updater_epoch, config.update_interval_ms);
        }
    }

    /// The take's pulse and preview-expiry clock.
    ///
    /// It never reads the caret or the cursor: the panel was anchored once at
    /// show time. A tick dispatches to the main thread only when the dot
    /// pulses or a preview label is due to expire; a solid Hold dot with no
    /// preview costs no main-thread work at all.
    fn spawn_updater(generation: u64, epoch: u64, update_interval_ms: u64) {
        let spawned = thread::Builder::new()
            .name("hold-badge-updater".into())
            .spawn(move || {
                let mut pulse_phase: f64 = 0.0;
                let pulse_speed = 0.15; // Radians per update cycle

                loop {
                    thread::sleep(Duration::from_millis(update_interval_ms));

                    // Snapshot state without blocking the main thread.
                    let (window_ptr, mode, should_pulse, preview_due) = {
                        use std::sync::TryLockError;
                        let state = match BADGE_STATE.try_lock() {
                            Ok(state) => state,
                            Err(TryLockError::Poisoned(err)) => err.into_inner(),
                            Err(TryLockError::WouldBlock) => {
                                // Skip this tick if the state is busy; avoid blocking UI.
                                continue;
                            }
                        };
                        if !state.timer_running
                            || state.updater_epoch != epoch
                            || !super::token_is_current(generation, take_token())
                        {
                            break;
                        }
                        (
                            state.window,
                            state.config.mode,
                            state.config.mode.voice_pulses(state.voice),
                            super::preview_expired(state.preview_updated_at, Instant::now()),
                        )
                    };

                    let Some(window_ptr) = window_ptr else {
                        continue;
                    };

                    if preview_due {
                        expire_transcript(generation, window_ptr);
                    }
                    if !should_pulse {
                        continue;
                    }

                    // Pulse opacity: sine wave from 0.4 to 1.0.
                    pulse_phase += pulse_speed;
                    let pulse_opacity = 0.7 + 0.3 * pulse_phase.sin();

                    Queue::main().exec_async(move || {
                        // Ensure state is still valid; do not block UI if busy.
                        {
                            use std::sync::TryLockError;
                            let state = match BADGE_STATE.try_lock() {
                                Ok(state) => state,
                                Err(TryLockError::Poisoned(err)) => err.into_inner(),
                                Err(TryLockError::WouldBlock) => return,
                            };
                            // A steady voice painted since the snapshot owns opacity.
                            if !state.timer_running
                                || state.updater_epoch != epoch
                                || state.window != Some(window_ptr)
                                || state.config.mode != mode
                                || state.deferred_blob_active
                                || !super::token_is_current(generation, take_token())
                                || !state.config.mode.voice_pulses(state.voice)
                            {
                                return;
                            }
                        }

                        let window = window_ptr as Id;
                        unsafe {
                            let content_view: Id = msg_send![window, contentView];
                            if content_view.is_null() {
                                return;
                            }
                            let subviews: Id = msg_send![content_view, subviews];
                            let count: usize = msg_send![subviews, count];
                            if count == 0 {
                                return;
                            }
                            let badge_view: Id = msg_send![subviews, objectAtIndex: 0usize];
                            let layer: Id = msg_send![badge_view, layer];
                            if !layer.is_null() {
                                let _: () = msg_send![layer, setOpacity: pulse_opacity as f32];
                            }
                        }
                    });
                }
            });
        if let Err(error) = spawned {
            debug!("hold-badge updater unavailable: {error}");
        }
    }

    /// Show the hold badge with custom configuration.
    /// This dispatches to the main thread for thread safety with AppKit.
    pub fn show_hold_badge_with_config(config: HoldBadgeConfig) {
        // Capture the show generation at REQUEST time. A hide that lands before the
        // (possibly queued) impl runs bumps the generation, so the impl recognises
        // itself as stale and does not resurrect a dismissed badge.
        let generation = BADGE_GENERATION.load(Ordering::SeqCst);

        // Queue ownership, not a Rust thread name, proves AppKit main-thread access.
        Queue::main().exec_async(move || {
            show_hold_badge_impl(config, generation);
        });
    }

    /// Hide the hold badge and stop position tracking.
    /// This dispatches to the main thread for thread safety with AppKit.
    pub fn hide_hold_badge() {
        debug!("Hiding hold badge");

        // Bump the show generation so any show enqueued before this hide (or racing
        // its panel creation) is recognised as stale and does not resurrect the
        // badge after we tear it down.
        BADGE_GENERATION.fetch_add(1, Ordering::SeqCst);

        // Stop the timer first (can be done on any thread)
        let panel_ptr = {
            let mut state = BADGE_STATE.lock().unwrap_or_else(|e| e.into_inner());
            state.timer_running = false;
            state.voice = CursorVoice::Unobserved;
            state.deferred_blob_active = false;
            state.deferred_blob_painted = false;
            state.is_dragging = false;
            state.window.take()
        };

        // Dispatch panel close to main thread. Do NOT hold BADGE_STATE while closing.
        Queue::main().exec_async(move || {
            if let Some(panel_ptr) = panel_ptr {
                unsafe {
                    panel_close(panel_ptr as Id);
                }
            }
        });
    }

    /// End-of-recording teardown (`State::Idle`). An armed deferred-insert
    /// capsule outlives the recording that armed it; any other badge goes.
    pub fn end_recording_badge() {
        if is_deferred_blob_active() {
            debug!("Recording ended; the deferred-insert capsule stays armed");
            return;
        }
        hide_hold_badge();
    }

    /// Dismiss the deferred-insert capsule, and only the capsule: a live take
    /// badge is left alone.
    pub fn hide_deferred_blob() {
        if is_deferred_blob_active() {
            hide_hold_badge();
        }
    }

    /// Whether the panel is (or is about to be painted as) the armed capsule.
    pub fn is_deferred_blob_active() -> bool {
        BADGE_STATE
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .deferred_blob_active
    }

    /// Hide the capsule only if it is still the one requested as `epoch`.
    fn hide_blob_epoch(epoch: u64) {
        let current = {
            let state = BADGE_STATE.lock().unwrap_or_else(|e| e.into_inner());
            state.deferred_blob_active && state.blob_epoch == epoch
        };
        if current {
            hide_hold_badge();
        }
    }

    /// Turn the badge into the draggable deferred-insert capsule ("Blobek").
    ///
    /// The armed transcript stays in the clipboard module's slot; the capsule
    /// is its projection. It survives the end of the recording, accepts mouse
    /// input without activating Codescribe, pastes into the focused field on
    /// a click, pastes where it is dropped after a drag, and dismisses on `×`.
    /// It expires with the slot after [`crate::os::clipboard::DEFERRED_INSERT_TTL`].
    pub fn show_deferred_blob(text: &str) {
        let text = text.trim();
        if text.is_empty() {
            return;
        }
        let preview = super::blob_preview_text(text);
        // Claimed at request time: `end_recording_badge` may run before the
        // main thread paints, and must already see an armed capsule.
        let epoch = {
            let mut state = BADGE_STATE.lock().unwrap_or_else(|e| e.into_inner());
            state.deferred_blob_active = true;
            state.timer_running = false;
            state.preview_updated_at = None;
            state.is_dragging = false;
            state.blob_epoch += 1;
            state.blob_epoch
        };
        Queue::main().exec_async(move || show_deferred_blob_impl(&preview, epoch));
        let ttl = crate::os::clipboard::DEFERRED_INSERT_TTL;
        let spawned = thread::Builder::new()
            .name("hold-badge-blob-ttl".into())
            .spawn(move || {
                thread::sleep(ttl);
                hide_blob_epoch(epoch);
            });
        if let Err(error) = spawned {
            debug!("deferred capsule TTL thread unavailable: {error}");
        }
    }

    /// Main-thread half of [`show_deferred_blob`].
    fn show_deferred_blob_impl(preview: &str, epoch: u64) {
        let (existing, config) = {
            let state = BADGE_STATE.lock().unwrap_or_else(|e| e.into_inner());
            if !state.deferred_blob_active || state.blob_epoch != epoch {
                return;
            }
            (state.window, state.config.clone())
        };
        let panel_ptr = match existing {
            Some(panel) => panel,
            None => unsafe {
                let panel = create_badge_panel(&config);
                place_at_cursor(panel, config.offset);
                {
                    let mut state = BADGE_STATE.lock().unwrap_or_else(|e| e.into_inner());
                    if !state.deferred_blob_active
                        || state.blob_epoch != epoch
                        || state.window.is_some()
                    {
                        drop(state);
                        panel_close(panel);
                        return;
                    }
                    state.window = Some(panel as usize);
                }
                spawn_caret_anchor(take_token(), panel as usize, config.offset);
                panel as usize
            },
        };
        {
            let mut state = BADGE_STATE.lock().unwrap_or_else(|e| e.into_inner());
            state.deferred_blob_painted = true;
            // A show queued before this request may have restarted the clock.
            state.timer_running = false;
            state.preview_updated_at = None;
        }
        let panel = panel_ptr as Id;
        unsafe {
            let _: () = msg_send![panel, setIgnoresMouseEvents: false];
            let _: () = msg_send![panel, setHasShadow: true];
            let content: Id = msg_send![panel, contentView];
            let children: Id = msg_send![content, subviews];
            let dot: Id = msg_send![children, objectAtIndex: 0usize];
            let size = CGSize {
                width: config.diameter,
                height: config.diameter,
            };
            let _: () = msg_send![dot, setFrameSize: size];
            let layer: Id = msg_send![dot, layer];
            if !layer.is_null() {
                let _: () = msg_send![layer, setCornerRadius: config.diameter / 2.0];
            }
            paint_preview(
                panel,
                config.diameter,
                config.mode,
                config.color,
                preview,
                CursorVoice::Unobserved,
                true,
            );
            panel_show(panel);
        }
    }

    /// Lazily register `CodescribeHoldBadgeView`, the badge panel's content
    /// view. It carries the capsule's press / drag / release handling; the
    /// live take badge ignores mouse events at the panel level and never
    /// reaches these methods.
    fn badge_view_class() -> &'static Class {
        static CLASS: OnceLock<usize> = OnceLock::new();
        let class = *CLASS.get_or_init(|| {
            let superclass = Class::get("NSView").expect("NSView class missing");
            let mut decl =
                ClassDecl::new("CodescribeHoldBadgeView", superclass).expect("class decl");
            unsafe {
                decl.add_method(
                    sel!(acceptsFirstMouse:),
                    badge_accepts_first_mouse as extern "C" fn(&Object, Sel, Id) -> BOOL,
                );
                decl.add_method(
                    sel!(mouseDown:),
                    badge_mouse_down as extern "C" fn(&Object, Sel, Id),
                );
                decl.add_method(
                    sel!(mouseDragged:),
                    badge_mouse_dragged as extern "C" fn(&Object, Sel, Id),
                );
                decl.add_method(
                    sel!(mouseUp:),
                    badge_mouse_up as extern "C" fn(&Object, Sel, Id),
                );
            }
            decl.register() as *const Class as usize
        });
        // SAFETY: registered classes live for the whole process.
        unsafe { &*(class as *const Class) }
    }

    /// The first press drags the capsule without activating Codescribe.
    extern "C" fn badge_accepts_first_mouse(_this: &Object, _sel: Sel, _event: Id) -> BOOL {
        YES
    }

    extern "C" fn badge_mouse_down(this: &Object, _sel: Sel, _event: Id) {
        let window: Id = unsafe { msg_send![this, window] };
        if window.is_null() {
            return;
        }
        let frame: CGRect = unsafe { msg_send![window, frame] };
        let mouse = NSEvent::mouseLocation();
        let mut state = BADGE_STATE.lock().unwrap_or_else(|e| e.into_inner());
        if !state.deferred_blob_active {
            return;
        }
        state.drag_start_mouse = (mouse.x, mouse.y);
        state.drag_start_origin = (frame.origin.x, frame.origin.y);
        state.is_dragging = false;
    }

    extern "C" fn badge_mouse_dragged(this: &Object, _sel: Sel, _event: Id) {
        let mouse = NSEvent::mouseLocation();
        let origin = {
            let mut state = BADGE_STATE.lock().unwrap_or_else(|e| e.into_inner());
            if !state.deferred_blob_active {
                return;
            }
            let dx = mouse.x - state.drag_start_mouse.0;
            let dy = mouse.y - state.drag_start_mouse.1;
            if !state.is_dragging && dx.hypot(dy) > DRAG_THRESHOLD {
                state.is_dragging = true;
            }
            if !state.is_dragging {
                return;
            }
            CGPoint {
                x: state.drag_start_origin.0 + dx,
                y: state.drag_start_origin.1 + dy,
            }
        };
        let window: Id = unsafe { msg_send![this, window] };
        if !window.is_null() {
            let _: () = unsafe { msg_send![window, setFrameOrigin: origin] };
        }
    }

    extern "C" fn badge_mouse_up(this: &Object, _sel: Sel, event: Id) {
        let (active, dragged, epoch) = {
            let mut state = BADGE_STATE.lock().unwrap_or_else(|e| e.into_inner());
            let dragged = state.is_dragging;
            state.is_dragging = false;
            (state.deferred_blob_active, dragged, state.blob_epoch)
        };
        if !active {
            return;
        }
        if dragged {
            let drop_mouse = NSEvent::mouseLocation();
            complete_blob_drop(drop_mouse.x, drop_mouse.y);
            return;
        }
        let window: Id = unsafe { msg_send![this, window] };
        if window.is_null() || event.is_null() {
            return;
        }
        let location: CGPoint = unsafe { msg_send![event, locationInWindow] };
        let frame: CGRect = unsafe { msg_send![window, frame] };
        if location.x >= frame.size.width - DISMISS_HIT_WIDTH {
            info!("Deferred-insert capsule dismissed");
            crate::os::clipboard::clear_deferred_insert();
            hide_hold_badge();
            return;
        }
        // A click on the body pastes into the field that still has focus:
        // the non-activating panel never took it.
        let spawned = thread::Builder::new()
            .name("hold-badge-click".into())
            .spawn(
                move || match crate::os::clipboard::deliver_deferred_insert_with_runtime_defaults()
                {
                    Ok(outcome) => {
                        debug!(?outcome, "Deferred-insert capsule click settled");
                        hide_blob_epoch(epoch);
                    }
                    Err(error) => warn!(%error, "Deferred-insert capsule click kept the text"),
                },
            );
        if let Err(error) = spawned {
            warn!("deferred capsule click thread unavailable: {error}");
        }
    }

    /// Frontmost application's pid, read through NSWorkspace.
    fn frontmost_pid() -> Option<i32> {
        unsafe {
            let class = Class::get("NSWorkspace")?;
            let workspace: Id = msg_send![class, sharedWorkspace];
            if workspace.is_null() {
                return None;
            }
            let app: Id = msg_send![workspace, frontmostApplication];
            if app.is_null() {
                return None;
            }
            let pid: i32 = msg_send![app, processIdentifier];
            (pid > 0).then_some(pid)
        }
    }

    /// Owning process of the accessibility element under a top-left global
    /// point, or `None` when nothing answers there.
    fn pid_at_point(cg_x: f64, cg_y: f64) -> Option<i32> {
        unsafe {
            let system_wide = AXUIElementCreateSystemWide();
            if system_wide.is_null() {
                return None;
            }
            let mut element: AXId = ptr::null_mut();
            let result = AXUIElementCopyElementAtPosition(
                system_wide,
                cg_x as f32,
                cg_y as f32,
                &mut element,
            );
            CFRelease(system_wide);
            if result != AX_ERROR_SUCCESS || element.is_null() {
                return None;
            }
            let mut pid: i32 = 0;
            let ok = AXUIElementGetPid(element, &mut pid) == AX_ERROR_SUCCESS;
            CFRelease(element);
            (ok && pid > 0).then_some(pid)
        }
    }

    /// Bring the app under the drop point to the front and wait briefly for
    /// the window server to agree.
    fn activate_app_at(cg_x: f64, cg_y: f64) {
        let Some(target_pid) = pid_at_point(cg_x, cg_y) else {
            return;
        };
        if target_pid == std::process::id() as i32 {
            return;
        }
        unsafe {
            let Some(class) = Class::get("NSRunningApplication") else {
                return;
            };
            let app: Id = msg_send![class, runningApplicationWithProcessIdentifier: target_pid];
            if app.is_null() {
                return;
            }
            // NSApplicationActivateIgnoringOtherApps.
            let _: BOOL = msg_send![app, activateWithOptions: 1usize << 1];
        }
        for _ in 0..15 {
            if frontmost_pid() == Some(target_pid) {
                return;
            }
            thread::sleep(Duration::from_millis(10));
        }
    }

    /// One synthetic left click at a top-left global point: it focuses the
    /// window and text field under the drop and places the caret there.
    fn post_click(cg_x: f64, cg_y: f64) {
        let point = CGPoint::new(cg_x, cg_y);
        for (kind, pause) in [
            (CGEventType::LeftMouseDown, Duration::from_millis(15)),
            (CGEventType::LeftMouseUp, Duration::ZERO),
        ] {
            let Ok(source) = CGEventSource::new(CGEventSourceStateID::HIDSystemState) else {
                warn!("Deferred-insert drop: no CGEventSource for the click");
                return;
            };
            let Ok(event) = CGEvent::new_mouse_event(source, kind, point, CGMouseButton::Left)
            else {
                warn!("Deferred-insert drop: could not create the click");
                return;
            };
            event.post(CGEventTapLocation::HID);
            if !pause.is_zero() {
                thread::sleep(pause);
            }
        }
    }

    /// Paste the armed transcript where the capsule was dropped.
    ///
    /// The capsule leaves the screen first so the probe and the click reach
    /// the window under it. A refused paste (a password field, Codescribe's
    /// own read-only canvas, an unreadable recipient) brings the capsule back
    /// with the transcript still armed for another drop.
    fn complete_blob_drop(ns_x: f64, ns_y: f64) {
        let (panel_ptr, epoch) = {
            let state = BADGE_STATE.lock().unwrap_or_else(|e| e.into_inner());
            (state.window, state.blob_epoch)
        };
        let Some(panel_ptr) = panel_ptr else {
            return;
        };
        unsafe {
            let panel = panel_ptr as Id;
            let _: () = msg_send![panel, orderOut: ptr::null::<Object>()];
            let _: () = msg_send![panel, setIgnoresMouseEvents: true];
        }
        let primary_height = unsafe { CGDisplayBounds(CGMainDisplayID()).size.height };
        let (cg_x, cg_y) = super::appkit_to_cg_point(ns_x, ns_y, primary_height);
        let spawned = thread::Builder::new()
            .name("hold-badge-drop".into())
            .spawn(move || {
                objc::rc::autoreleasepool(|| {
                    activate_app_at(cg_x, cg_y);
                    post_click(cg_x, cg_y);
                    thread::sleep(Duration::from_millis(55));
                    match crate::os::clipboard::deliver_deferred_insert_after_drop() {
                        Ok(outcome) => {
                            info!(?outcome, "Deferred-insert capsule drop settled");
                            hide_blob_epoch(epoch);
                        }
                        Err(error) => {
                            warn!(%error, "Deferred-insert capsule drop refused; capsule kept");
                            reshow_blob(panel_ptr, epoch);
                        }
                    }
                });
            });
        if let Err(error) = spawned {
            warn!("deferred capsule drop thread unavailable: {error}");
            reshow_blob(panel_ptr, epoch);
        }
    }

    /// Bring a still-armed capsule back after a refused drop.
    fn reshow_blob(panel_ptr: usize, epoch: u64) {
        Queue::main().exec_async(move || {
            {
                let state = BADGE_STATE.lock().unwrap_or_else(|e| e.into_inner());
                if !state.deferred_blob_active
                    || state.blob_epoch != epoch
                    || state.window != Some(panel_ptr)
                {
                    return;
                }
            }
            unsafe {
                let panel = panel_ptr as Id;
                panel_show(panel);
                let _: () = msg_send![panel, setIgnoresMouseEvents: false];
            }
        });
    }

    /// One timed flash of the dot with `label` beside it, for a refused
    /// gesture (Maciej, 2026-10-17: a chord on a vacant channel was silent).
    ///
    /// A take's dot outranks a refusal: while any panel exists the flash is
    /// skipped, and the delayed hide tears down only a panel that still shows
    /// this flash. A take that opened meanwhile repainted the same panel in
    /// its own mode and keeps it; a hide that raced the show bumped the
    /// generation, so the show was dropped and there is nothing to hide.
    pub fn flash_hold_badge(config: HoldBadgeConfig, label: &str, duration: Duration) {
        {
            let state = BADGE_STATE.lock().unwrap_or_else(|e| e.into_inner());
            if state.window.is_some() || state.timer_running {
                debug!("Skipping refusal flash: a live badge owns the pointer");
                return;
            }
        }
        let mode = config.mode;
        show_hold_badge_with_config(config);
        update_transcript(take_token(), label);
        let spawned = thread::Builder::new()
            .name("hold-badge-flash".into())
            .spawn(move || {
                thread::sleep(duration);
                let still_flashing = {
                    let state = BADGE_STATE.lock().unwrap_or_else(|e| e.into_inner());
                    state.window.is_some() && state.config.mode == mode
                };
                if still_flashing {
                    hide_hold_badge();
                }
            });
        if let Err(error) = spawned {
            // No thread to end the flash: bump the generation now so the
            // queued show is dropped instead of leaving a dot forever.
            debug!("hold-badge flash thread unavailable: {error}");
            hide_hold_badge();
        }
    }
}

#[cfg(target_os = "macos")]
pub use imp::{
    end_recording_badge, flash_hold_badge, focused_input_field, get_caret_position,
    get_cursor_position, hide_deferred_blob, hide_hold_badge, is_deferred_blob_active,
    show_deferred_blob, show_hold_badge, show_hold_badge_with_config, take_token,
    update_transcript, update_voice_state,
};

// ─────────────────────────────────────────────────────────────
// Non-macOS no-op stubs (keep the public surface compiling)
// ─────────────────────────────────────────────────────────────

/// No-op badge surface for non-macOS targets.
///
/// Mirrors `imp`'s public functions so the rest of the app compiles and links
/// unchanged off macOS; there is no AppKit or Accessibility equivalent to wire
/// up, so every entry point is inert.
#[cfg(not(target_os = "macos"))]
mod stubs {
    use super::{BadgeMode, FocusedInputField, HoldBadgeConfig};

    /// Inert take token outside macOS.
    pub fn take_token() -> u64 {
        0
    }
    /// Inert cursor projection outside macOS.
    pub fn update_transcript(_token: u64, _text: &str) {}

    /// Inert voice paint outside macOS.
    pub fn update_voice_state(_token: u64, _voice: super::CursorVoice) {}

    /// Focused-element text detection is macOS-only.
    pub fn focused_input_field() -> FocusedInputField {
        FocusedInputField::Unobserved
    }

    /// No-op: caret tracking is macOS-only.
    pub fn get_caret_position() -> Option<(f64, f64)> {
        None
    }

    /// No-op: cursor tracking is macOS-only.
    pub fn get_cursor_position() -> (f64, f64) {
        (0.0, 0.0)
    }

    /// No-op on non-macOS platforms.
    pub fn show_hold_badge() {}

    /// No-op on non-macOS platforms.

    /// No-op on non-macOS platforms.
    pub fn show_hold_badge_with_config(_config: HoldBadgeConfig) {}

    /// No-op on non-macOS platforms.
    pub fn hide_hold_badge() {}

    /// No-op on non-macOS platforms.
    pub fn end_recording_badge() {}

    /// No-op: the deferred-insert capsule is macOS-only.
    pub fn show_deferred_blob(_text: &str) {}

    /// No-op on non-macOS platforms.
    pub fn hide_deferred_blob() {}

    /// The capsule never exists off macOS.
    pub fn is_deferred_blob_active() -> bool {
        false
    }

    /// No-op on non-macOS platforms.
    pub fn flash_hold_badge(
        _config: HoldBadgeConfig,
        _label: &str,
        _duration: std::time::Duration,
    ) {
    }
}

#[cfg(not(target_os = "macos"))]
pub use stubs::{
    end_recording_badge, flash_hold_badge, focused_input_field, get_caret_position,
    get_cursor_position, hide_deferred_blob, hide_hold_badge, is_deferred_blob_active,
    show_deferred_blob, show_hold_badge, show_hold_badge_with_config, take_token,
    update_transcript, update_voice_state,
};

#[cfg(target_os = "macos")]
pub(crate) use imp::input_field_for_element;

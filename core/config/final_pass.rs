//! Stop-path final-pass routing mode — the canonical, crate-shared resolution of
//! Settings → Final pass (`FINAL_PASS_MODE`).
//!
//! Lives in core (not in the app controller) because **every** stop lane must
//! consult the same mode: the hotkey/overlay controller lane and the composer
//! voice-note lane in the UniFFI bridge. Duplicating the parsing per lane is how
//! a lane silently keeps running a full-file Whisper re-pass under Smart/Off.

/// Canonical stop-path final-pass routing (Settings → Final pass / `FINAL_PASS_MODE`).
/// Distinct from `pipeline::contracts::FinalPassMode` (lexicon cleanup request).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FinalPassRoutingMode {
    /// Always: full Whisper file re-pass after release (on). The **only** mode
    /// permitted to re-transcribe the whole file.
    Always,
    /// Smart: per-utterance tail gap-fill appended to committed streaming text;
    /// never a whole-file re-pass onto existing text.
    Smart,
    /// Off: no Whisper on any stop path; streaming (+ post-process) is final
    /// (dictionary / lexicon still applies later, ungated).
    Off,
}

impl FinalPassRoutingMode {
    /// Canonical lowercase token (`always` / `smart` / `off`) for logs and telemetry.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Always => "always",
            Self::Smart => "smart",
            Self::Off => "off",
        }
    }

    /// Parse a settings/env token, accepting the documented aliases.
    ///
    /// Case- and whitespace-insensitive. Returns `None` for anything unknown —
    /// an unrecognised token must never silently coerce to a mode that
    /// re-transcribes the whole file.
    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "always" | "on" | "1" | "true" | "yes" => Some(Self::Always),
            "smart" | "auto" => Some(Self::Smart),
            "off" | "0" | "false" | "no" => Some(Self::Off),
            _ => None,
        }
    }
}

/// Resolve final-pass routing from env/settings. Default: Smart.
///
/// Precedence:
/// 1. `FINAL_PASS_MODE` / `CODESCRIBE_FINAL_PASS_MODE` (`always|smart|off`)
/// 2. Legacy `CODESCRIBE_LOCAL_STT_FINAL_PASS` falsey → Off, truthy → Always
/// 3. Smart
pub fn final_pass_routing_mode() -> FinalPassRoutingMode {
    for key in ["FINAL_PASS_MODE", "CODESCRIBE_FINAL_PASS_MODE"] {
        if let Ok(raw) = std::env::var(key)
            && let Some(mode) = FinalPassRoutingMode::parse(&raw)
        {
            return mode;
        }
    }
    if let Ok(raw) = std::env::var("CODESCRIBE_LOCAL_STT_FINAL_PASS") {
        let v = raw.trim().to_ascii_lowercase();
        if matches!(v.as_str(), "" | "0" | "false" | "no" | "off") {
            return FinalPassRoutingMode::Off;
        }
        if matches!(v.as_str(), "1" | "true" | "yes" | "on") {
            return FinalPassRoutingMode::Always;
        }
    }
    FinalPassRoutingMode::Smart
}

/// Final-pass routing mode defaults and env override coverage.
#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;

    /// Default routing is Smart; CODESCRIBE_FINAL_PASS_* env values override when valid.
    #[test]
    #[serial]
    fn test_final_pass_routing_mode_defaults_smart_and_honors_env() {
        unsafe {
            std::env::remove_var("FINAL_PASS_MODE");
            std::env::remove_var("CODESCRIBE_FINAL_PASS_MODE");
            std::env::remove_var("CODESCRIBE_LOCAL_STT_FINAL_PASS");
        }
        assert_eq!(final_pass_routing_mode(), FinalPassRoutingMode::Smart);

        for (raw, expected) in [
            ("always", FinalPassRoutingMode::Always),
            ("SMART", FinalPassRoutingMode::Smart),
            ("off", FinalPassRoutingMode::Off),
        ] {
            unsafe {
                std::env::set_var("FINAL_PASS_MODE", raw);
            }
            assert_eq!(final_pass_routing_mode(), expected, "FINAL_PASS_MODE={raw}");
        }

        unsafe {
            std::env::remove_var("FINAL_PASS_MODE");
            std::env::set_var("CODESCRIBE_LOCAL_STT_FINAL_PASS", "0");
        }
        assert_eq!(
            final_pass_routing_mode(),
            FinalPassRoutingMode::Off,
            "legacy LOCAL_STT_FINAL_PASS=0 maps to Off"
        );

        unsafe {
            std::env::set_var("CODESCRIBE_LOCAL_STT_FINAL_PASS", "1");
        }
        assert_eq!(
            final_pass_routing_mode(),
            FinalPassRoutingMode::Always,
            "legacy LOCAL_STT_FINAL_PASS=1 maps to Always"
        );

        unsafe {
            std::env::remove_var("CODESCRIBE_LOCAL_STT_FINAL_PASS");
            std::env::remove_var("FINAL_PASS_MODE");
            std::env::remove_var("CODESCRIBE_FINAL_PASS_MODE");
        }
    }

    /// Settings / env string parse: `on` (and aliases) map to Always; smart/auto; off aliases.
    #[test]
    fn test_final_pass_routing_mode_parse_on_maps_to_always() {
        for raw in ["on", "ON", "1", "true", "yes", "always", " Always "] {
            assert_eq!(
                FinalPassRoutingMode::parse(raw),
                Some(FinalPassRoutingMode::Always),
                "parse({raw:?}) must be Always"
            );
        }
        for raw in ["smart", "SMART", "auto", "Auto"] {
            assert_eq!(
                FinalPassRoutingMode::parse(raw),
                Some(FinalPassRoutingMode::Smart),
                "parse({raw:?}) must be Smart"
            );
        }
        for raw in ["off", "OFF", "0", "false", "no"] {
            assert_eq!(
                FinalPassRoutingMode::parse(raw),
                Some(FinalPassRoutingMode::Off),
                "parse({raw:?}) must be Off"
            );
        }
        assert_eq!(
            FinalPassRoutingMode::parse("bogus"),
            None,
            "unknown tokens must not silently coerce"
        );
        assert_eq!(FinalPassRoutingMode::Always.as_str(), "always");
        assert_eq!(FinalPassRoutingMode::Smart.as_str(), "smart");
        assert_eq!(FinalPassRoutingMode::Off.as_str(), "off");
    }
}

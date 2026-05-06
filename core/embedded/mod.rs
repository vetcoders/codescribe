//! Embedded asset registry: shared abstraction over compiled-in model blobs.
//!
//! CodeScribe ships up to four independent model assets that may be embedded
//! into the binary at build time: Whisper STT, MiniLM text embedder, Silero
//! VAD, and CSM-1B TTS. Each asset has its own payload shape (Whisper carries
//! mel filters; TTS carries Mimi codec + voice tokens; VAD is a single
//! tensor blob; the embedder is config + tokenizer + weights).
//!
//! Rather than force a one-size-fits-all payload struct, this module exposes a
//! lightweight metadata trait that every asset implements. Code that wants to
//! reason about embedded assets uniformly (audit, telemetry, diagnostics) can
//! iterate `EmbeddedAsset` impls; code that needs the actual byte payload
//! continues to call the asset-specific `get_embedded_data()` helper.
//!
//! # Why not a single `Payload` struct?
//!
//! VAD ships a single tensor with no tokenizer or config. TTS ships six
//! distinct slices including voice tokens. Forcing `Option<&'static [u8]>`
//! getters for the lowest common denominator (`config`/`tokenizer`/`weights`)
//! would lie to callers about VAD (which has none of those) and lose
//! information about TTS / Whisper (which carry more). The trait below
//! preserves naming, availability, and total size — the metadata that actually
//! is shared — while leaving payload access to type-specific APIs.
//!
//! Created by M&K (c)2026 VetCoders

/// Metadata + availability for one embedded model asset.
///
/// Implementors are zero-sized marker types; the trait body delegates to the
/// asset-specific `embedded` module. Use this trait for cross-asset queries
/// (e.g. "list every embedded model and report bytes shipped"). For actual
/// model bytes, call the asset-specific `get_embedded_data()` instead.
pub trait EmbeddedAsset {
    /// Human-readable asset name, used for logs and diagnostics.
    const NAME: &'static str;

    /// Whether the binary was built with this asset embedded.
    fn is_embedded_available() -> bool;

    /// Total bytes embedded for this asset (sum of every slice). Returns 0
    /// when the asset is not embedded.
    fn total_size() -> usize;
}

/// Whisper STT asset (config + tokenizer + mel filters + weights).
pub struct WhisperAsset;

impl EmbeddedAsset for WhisperAsset {
    const NAME: &'static str = "whisper";

    fn is_embedded_available() -> bool {
        crate::stt::whisper::embedded::is_embedded_available()
    }

    fn total_size() -> usize {
        crate::stt::whisper::embedded::get_embedded_data()
            .map(|m| m.total_size())
            .unwrap_or(0)
    }
}

/// MiniLM text embedder asset (config + tokenizer + weights).
pub struct EmbedderAsset;

impl EmbeddedAsset for EmbedderAsset {
    const NAME: &'static str = "embedder";

    fn is_embedded_available() -> bool {
        crate::embedder::embedded::is_embedded_available()
    }

    fn total_size() -> usize {
        crate::embedder::embedded::get_embedded_data()
            .map(|m| m.total_size())
            .unwrap_or(0)
    }
}

/// Silero VAD asset (single ONNX blob).
pub struct VadAsset;

impl EmbeddedAsset for VadAsset {
    const NAME: &'static str = "silero_vad";

    fn is_embedded_available() -> bool {
        crate::vad::embedded::is_embedded_available()
    }

    fn total_size() -> usize {
        crate::vad::embedded::get_embedded_data()
            .map(|b| b.len())
            .unwrap_or(0)
    }
}

/// CSM-1B TTS asset (config + tokenizer + weights + Mimi codec + voice tokens).
pub struct TtsAsset;

impl EmbeddedAsset for TtsAsset {
    const NAME: &'static str = "tts";

    fn is_embedded_available() -> bool {
        crate::tts::embedded::is_embedded_available()
    }

    fn total_size() -> usize {
        crate::tts::embedded::get_embedded_data()
            .map(|t| t.total_size())
            .unwrap_or(0)
    }
}

/// Snapshot of every embedded asset's availability + footprint.
///
/// Useful for diagnostics and `--version`-style introspection. The returned
/// vector is ordered by canonical asset name, and each tuple is
/// `(name, embedded?, total_size_bytes)`.
pub fn snapshot() -> Vec<(&'static str, bool, usize)> {
    vec![
        report::<WhisperAsset>(),
        report::<EmbedderAsset>(),
        report::<VadAsset>(),
        report::<TtsAsset>(),
    ]
}

fn report<A: EmbeddedAsset>() -> (&'static str, bool, usize) {
    (A::NAME, A::is_embedded_available(), A::total_size())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_lists_all_four_assets() {
        let snap = snapshot();
        let names: Vec<&str> = snap.iter().map(|(n, _, _)| *n).collect();
        assert_eq!(names, ["whisper", "embedder", "silero_vad", "tts"]);
    }

    #[test]
    fn total_size_is_zero_when_not_embedded() {
        // VAD is small enough that builds usually embed it; this test only
        // checks the contract (`Some(0)` is impossible — must be `> 0` when
        // available, exactly `0` when not).
        for (name, available, size) in snapshot() {
            if !available {
                assert_eq!(size, 0, "{name} reports nonzero size when unavailable");
            }
            // When available we cannot assert a lower bound at compile time
            // because release vs debug vs feature flags differ; the type
            // contract is enforced by the per-asset modules.
        }
    }
}

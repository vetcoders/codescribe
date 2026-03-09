/// Integration tests for the unified pipeline components.
///
/// These exercise dedup + sinks together, simulating multi-chunk and
/// multi-utterance scenarios without needing a real Whisper engine.
#[cfg(test)]
mod pipeline_integration {
    use crate::audio::load_audio_file;
    use crate::pipeline::contracts::DeltaSink;
    use crate::pipeline::contracts::TranscriptDelta;
    use crate::pipeline::dedup::{dedup_chunk_overlap, strip_suffix_overlap};
    use crate::pipeline::sinks::{CallbackSink, CollectorSink};
    use crate::pipeline::streaming::{transcribe_buffered_samples, transcribe_streaming_samples};
    use std::path::PathBuf;
    use std::sync::Arc;

    // ── Test 1: Pipeline roundtrip, no dedup needed ──────────────

    #[test]
    fn test_pipeline_roundtrip_no_dedup() {
        let collector = Arc::new(CollectorSink::new());
        let sink: Arc<dyn DeltaSink> = collector.clone();

        // Simulate: single chunk → delta → sink
        let mut buffer = String::new();
        let chunk = "Hello world.";
        dedup_chunk_overlap(&mut buffer, chunk);

        let delta = TranscriptDelta::from_raw(&buffer);
        sink.apply(&delta);

        assert_eq!(buffer, "Hello world.");
        assert_eq!(collector.collected(), vec!["Hello world."]);
    }

    // ── Test 2: Dedup overlapping chunks ─────────────────────────

    #[test]
    fn test_pipeline_dedup_overlapping_chunks() {
        let mut buffer = String::new();

        // Chunk 1
        dedup_chunk_overlap(&mut buffer, "Hello world this is");
        // Chunk 2 overlaps: "this is" appears at end of chunk 1 and start of chunk 2
        dedup_chunk_overlap(&mut buffer, "this is a test");

        assert_eq!(buffer, "Hello world this is a test");
    }

    // ── Test 3: Suffix dedup across utterances ───────────────────

    #[test]
    fn test_pipeline_suffix_dedup_utterances() {
        let mut transcript = String::new();
        // Utterance 1
        let utt1 = "Hello world.";
        transcript.push_str(utt1);
        let last_suffix = utt1;

        // Utterance 2 — overlaps with end of utterance 1
        let utt2 = "world. And more.";
        let stripped = strip_suffix_overlap(last_suffix, utt2);
        if !stripped.is_empty() {
            if !transcript.ends_with(' ') && !stripped.starts_with(' ') {
                transcript.push(' ');
            }
            transcript.push_str(&stripped);
        }

        assert_eq!(transcript, "Hello world. And more.");
    }

    // ── Test 4: Delta accumulation equals transcript ─────────────

    #[test]
    fn test_delta_accumulation_equals_transcript() {
        let collector = Arc::new(CollectorSink::new());
        let sink: Arc<dyn DeltaSink> = collector.clone();

        let chunks = ["Cześć, ", "jestem ", "weterynarzem."];
        let mut buffer = String::new();

        for chunk in &chunks {
            let before = buffer.clone();
            dedup_chunk_overlap(&mut buffer, chunk);
            // Build delta as difference
            let delta_str = &buffer[before.len()..];
            if !delta_str.is_empty() {
                sink.apply(&TranscriptDelta::from_raw(delta_str));
            }
        }

        // Reassemble from deltas
        let reassembled: String = collector.collected().join("");
        assert_eq!(buffer, "Cześć, jestem weterynarzem.");
        assert_eq!(reassembled, buffer);
    }

    // ── Test 5: CallbackSink bridges to Fn(&str) ────────────────

    #[test]
    fn test_callback_sink_integration() {
        let received = Arc::new(std::sync::Mutex::new(Vec::new()));
        let r = received.clone();
        let sink: Arc<dyn DeltaSink> = Arc::new(CallbackSink::new(Arc::new(move |s: &str| {
            r.lock().unwrap().push(s.to_string());
        })));

        sink.apply(&TranscriptDelta::from_raw("Hello"));
        sink.apply(&TranscriptDelta::from_raw(" world"));

        let result = received.lock().unwrap();
        assert_eq!(*result, vec!["Hello", " world"]);
    }

    // ── Test 6: Multi-chunk fuzzy dedup end-to-end ───────────────

    #[test]
    fn test_pipeline_fuzzy_dedup_chain() {
        let mut buffer = String::new();

        // 3 chunks with overlapping regions, some fuzzy
        dedup_chunk_overlap(&mut buffer, "The quick brown fox");
        dedup_chunk_overlap(&mut buffer, "brown fox jumps over");
        dedup_chunk_overlap(&mut buffer, "jumps over the lazy dog");

        assert_eq!(buffer, "The quick brown fox jumps over the lazy dog");
    }

    // ── Test 7: Empty and whitespace-only chunks ─────────────────

    #[test]
    fn test_pipeline_empty_chunks_ignored() {
        let mut buffer = String::new();

        dedup_chunk_overlap(&mut buffer, "Hello");
        dedup_chunk_overlap(&mut buffer, "");
        dedup_chunk_overlap(&mut buffer, "   ");
        dedup_chunk_overlap(&mut buffer, "world");

        assert_eq!(buffer, "Hello world");
    }

    // ── Test 8: Overlay vs CLI streaming (real audio, opt-in) ───────────────

    fn e2e_enabled() -> bool {
        std::env::var("CODESCRIBE_E2E_STT")
            .ok()
            .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
            .unwrap_or(false)
    }

    fn data_assets_dir() -> Option<PathBuf> {
        let home = std::env::var("HOME").ok()?;
        let dir = PathBuf::from(home).join(".codescribe/data_assets");
        if dir.exists() { Some(dir) } else { None }
    }

    fn sample_asset() -> Option<PathBuf> {
        let dir = data_assets_dir()?;
        let path = dir.join("01_no-to-dobra.wav");
        if path.exists() { Some(path) } else { None }
    }

    fn normalize_text(input: &str) -> String {
        let mut out = String::with_capacity(input.len());
        for ch in input.to_lowercase().chars() {
            if ch.is_alphanumeric()
                || ch == ' '
                || ch == 'ą'
                || ch == 'ć'
                || ch == 'ę'
                || ch == 'ł'
                || ch == 'ń'
                || ch == 'ó'
                || ch == 'ś'
                || ch == 'ż'
                || ch == 'ź'
            {
                out.push(ch);
            } else {
                out.push(' ');
            }
        }
        out.split_whitespace().collect::<Vec<_>>().join(" ")
    }

    fn word_overlap_ratio(actual: &str, expected: &str) -> f32 {
        let actual_words: std::collections::HashSet<&str> = actual.split_whitespace().collect();
        let expected_words: Vec<&str> = expected.split_whitespace().collect();
        if expected_words.is_empty() {
            return 0.0;
        }
        let matches = expected_words
            .iter()
            .filter(|w| actual_words.contains(**w))
            .count();
        matches as f32 / expected_words.len() as f32
    }

    #[tokio::test]
    async fn test_overlay_vs_cli_streaming_consistency() {
        if !e2e_enabled() {
            eprintln!("Skipping overlay-vs-cli E2E (set CODESCRIBE_E2E_STT=1 to enable)");
            return;
        }

        let audio_path = match sample_asset() {
            Some(p) => p,
            None => {
                eprintln!("No real audio assets found at ~/.codescribe/data_assets");
                return;
            }
        };

        let (samples, sample_rate) = load_audio_file(&audio_path).expect("load audio samples");

        // CLI raw streaming (no overlay postprocess)
        let raw = transcribe_streaming_samples(&samples, sample_rate, None, None)
            .expect("raw streaming transcribe");

        // Event-session pipeline (VAD → utterances → UtteranceFinal collector)
        let overlay = transcribe_buffered_samples(&samples, sample_rate, None)
            .await
            .expect("overlay buffered transcribe");

        assert!(
            !overlay.trim().is_empty(),
            "Overlay pipeline returned empty transcript"
        );

        let norm_raw = normalize_text(&raw);
        let norm_overlay = normalize_text(&overlay);
        let overlap = word_overlap_ratio(&norm_overlay, &norm_raw)
            .max(word_overlap_ratio(&norm_raw, &norm_overlay));

        println!(
            "Overlay vs CLI overlap: {:.2} (raw={} chars, overlay={} chars)",
            overlap,
            raw.len(),
            overlay.len()
        );

        assert!(
            overlap >= 0.55,
            "Overlay vs CLI overlap too low ({:.2})",
            overlap
        );
    }
}

// Additional regression-focused tests live in a separate file to minimize conflicts.
mod regressions;

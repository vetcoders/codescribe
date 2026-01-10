#[cfg(target_os = "macos")]
#[test]
fn test_local_stt_instantiation() {
    use std::path::PathBuf;
    use codescribe::local_stt::LocalWhisperEngine;

    let model_path = std::env::var("CODESCRIBE_TEST_MODEL_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/Users/maciejgad/hosted/VetCoders/CodeScribe/models/whisper-large-v3-mlx-q8"));

    if !model_path.exists() {
        eprintln!("Model dir not found at {:?}, skipping instantiation test", model_path);
        return;
    }

    let engine = LocalWhisperEngine::new(&model_path);
    assert!(engine.is_ok(), "Failed to instantiate engine: {:?}", engine.err());
}

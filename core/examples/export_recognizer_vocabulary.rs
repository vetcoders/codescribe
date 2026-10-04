//! Export a frozen production vocabulary for the opt-in Apple A/B runner.
//! Point CODESCRIBE_DATA_DIR at a copied dictionary, never the live directory:
//! the dictionary reader also cleans up temporary files.

use codescribe_core::stt::recognizer_vocabulary::RecognizerVocabulary;

fn main() -> anyhow::Result<()> {
    let private = directories::BaseDirs::new()
        .ok_or_else(|| anyhow::anyhow!("Cannot resolve home directory"))?
        .home_dir()
        .join(".codescribe");
    let private = private.canonicalize().unwrap_or(private);
    anyhow::ensure!(
        !codescribe_core::config::Config::config_dir().starts_with(&private),
        "Set CODESCRIBE_DATA_DIR to a copied dictionary outside ~/.codescribe"
    );
    let vocabulary = RecognizerVocabulary::load();
    println!(
        "{}",
        serde_json::to_string(&serde_json::json!({
            "terms": vocabulary.apple_contextual_strings().unwrap_or_default(),
            "sources": {
                "active_names": vocabulary.sources.active_names,
                "custom_canonicals": vocabulary.sources.custom_canonicals,
                "protected_terms": vocabulary.sources.protected_terms,
            },
        }))?
    );
    Ok(())
}

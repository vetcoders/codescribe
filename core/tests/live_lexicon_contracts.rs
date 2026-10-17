//! Executes the current registered-phrase contracts against the exact production
//! source and real ledger rule type. This proves lexical rules, not ASR accuracy.
mod pipeline {
    pub use codescribe_core::pipeline::acoustic_ledger;
}

// The source contract includes the same admission gate as the live module.
// Compile it here so its crate-visible predicates keep their production visibility.
#[path = "../quality/lexicon_gate.rs"]
pub(crate) mod lexicon_gate;
mod quality {
    pub(crate) use super::lexicon_gate;
}

#[path = "../pipeline/streaming/live_lexicon.rs"]
mod live_lexicon;

#[test]
fn registered_rules_keep_bundled_inventory_available() {
    assert!(live_lexicon::bundled_count() > 0);
    let protected = lexicon_gate::ProtectedTerms::builtin();
    assert!(!protected.is_empty());
    assert!(protected.terms().any(|term| term == "whisper"));
}

#[test]
fn registered_witness_requires_a_unique_heard_alias_instead_of_fuzzy_proximity() {
    let directory = tempfile::tempdir().expect("isolated lexicon");
    let path = directory.path().join("lexicon.custom.jsonl");
    std::fs::write(
        &path,
        concat!(
            "{\"term\":\"Zorvexa\",\"mispronunciations\":[\"zorveksa\",\"sharedheard\"]}\n",
            "{\"term\":\"DifferentName\",\"mispronunciations\":[\"sharedheard\"]}\n",
        ),
    )
    .expect("write current registered aliases");
    let lexicon = live_lexicon::FuzzyLexicon::load(&path);
    assert_eq!(
        lexicon.registered_candidate("zorveksa"),
        Some("Zorvexa".into())
    );
    assert_eq!(lexicon.candidate("Alfaxone"), Some("Alfaksalon".into()));
    assert_eq!(
        lexicon.registered_candidate("Alfaxone"),
        None,
        "retrieval proximity cannot stand in for a heard registered form"
    );
    assert_eq!(
        lexicon.registered_candidate("sharedheard"),
        None,
        "an ambiguous registered form cannot corroborate either canonical"
    );
    assert_eq!(
        lexicon.registered_candidate("Iwo Iwo Iwo Iwo Iwo"),
        None,
        "unregistered physical repetitions must not become a dictionary witness"
    );
}

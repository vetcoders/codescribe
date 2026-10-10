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

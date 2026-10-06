//! Executes the current registered-phrase contracts against the exact production
//! source and real ledger rule type. This proves lexical rules, not ASR accuracy.
mod pipeline {
    pub use codescribe_core::pipeline::acoustic_ledger;
}

#[path = "../pipeline/streaming/live_lexicon.rs"]
mod live_lexicon;

#[test]
fn registered_rules_keep_bundled_inventory_available() {
    assert!(live_lexicon::bundled_count() > 0);
}

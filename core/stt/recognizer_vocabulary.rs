//! Recognition vocabulary, independent of document correction and PCM identity.
//! The builder is available for validation; no recognizer enables it yet.

use std::collections::{BTreeMap, HashSet};

use unicode_casefold::UnicodeCaseFold;

use crate::config::Config;
use crate::quality::lexicon_gate::ProtectedTerms;
use crate::quality::overlay_quality::custom_lexicon_entries;

/// SFSpeech contextualStrings ceiling, applied after source-priority deduplication.
pub const APPLE_VOCABULARY_LIMIT: usize = 100;

/// Counts of terms retained from each source, after deduplication and budgeting.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct VocabularySources {
    pub active_names: usize,
    pub custom_canonicals: usize,
    pub protected_terms: usize,
}

/// One deterministic vocabulary snapshot for recognition engines.
#[derive(Debug, Default)]
pub struct RecognizerVocabulary {
    terms: Vec<String>,
    pub sources: VocabularySources,
}

impl RecognizerVocabulary {
    /// Read the existing production sources. Tests use `from_sources` instead.
    /// Loading this snapshot does not enable recognition bias.
    pub fn load() -> Self {
        let names = super::active_names::active_names();
        let canonicals = match custom_lexicon_entries() {
            Ok(entries) => entries.into_iter().map(|entry| entry.canonical).collect(),
            Err(error) => {
                tracing::warn!(%error, "recognizer vocabulary: custom dictionary unavailable");
                Vec::new()
            }
        };
        let protected =
            ProtectedTerms::load_from(&ProtectedTerms::default_path(&Config::config_dir()));
        let vocabulary = Self::from_sources(names, canonicals, &protected);
        tracing::info!(
            terms = vocabulary.terms.len(),
            active_names = vocabulary.sources.active_names,
            custom_canonicals = vocabulary.sources.custom_canonicals,
            protected_terms = vocabulary.sources.protected_terms,
            "recognizer vocabulary built"
        );
        tracing::debug!(terms = ?vocabulary.terms, "recognizer vocabulary terms");
        vocabulary
    }

    /// Inject sources without reading the user's configuration or agent leases.
    /// Source priority is names, custom canonical forms, then protected terms.
    /// Within each source, Unicode casefold order and lexical spelling ties
    /// make the result independent of file, lease, and hash-map iteration order.
    pub fn from_sources(
        active_names: impl IntoIterator<Item = String>,
        custom_canonicals: impl IntoIterator<Item = String>,
        protected: &ProtectedTerms,
    ) -> Self {
        let mut vocabulary = Self::default();
        let mut seen = HashSet::new();
        for (source, values) in [
            active_names.into_iter().collect::<Vec<_>>(),
            custom_canonicals.into_iter().collect(),
            protected.terms().map(str::to_owned).collect(),
        ]
        .into_iter()
        .enumerate()
        {
            let mut sorted = BTreeMap::<String, String>::new();
            for raw in values {
                let term = raw.trim();
                if term.is_empty() {
                    continue;
                }
                let key: String = term.case_fold().collect();
                sorted
                    .entry(key)
                    .and_modify(|spelling| {
                        if term < spelling.as_str() {
                            *spelling = term.to_owned();
                        }
                    })
                    .or_insert_with(|| term.to_owned());
            }
            for (key, term) in sorted {
                if vocabulary.terms.len() == APPLE_VOCABULARY_LIMIT {
                    break;
                }
                if seen.insert(key) {
                    vocabulary.terms.push(term);
                    match source {
                        0 => vocabulary.sources.active_names += 1,
                        1 => vocabulary.sources.custom_canonicals += 1,
                        _ => vocabulary.sources.protected_terms += 1,
                    }
                }
            }
        }
        vocabulary
    }

    /// Empty context is omitted from requests, never sent as an empty array.
    pub fn apple_contextual_strings(&self) -> Option<&[String]> {
        (!self.terms.is_empty()).then_some(self.terms.as_slice())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn owned(values: &[&str]) -> Vec<String> {
        values.iter().map(|term| (*term).to_owned()).collect()
    }

    #[test]
    fn source_priority_keeps_agent_and_custom_spellings() {
        let vocabulary = RecognizerVocabulary::from_sources(
            owned(&["Zoe", "Iwo"]),
            owned(&["Vibecrafted", "iwo", "CLI-ów"]),
            &ProtectedTerms::builtin(),
        );
        let terms = vocabulary.apple_contextual_strings().unwrap();
        assert_eq!(&terms[..4], ["Iwo", "Zoe", "CLI-ów", "Vibecrafted"]);
        assert_eq!(terms[4], "anthropic");
        assert!(
            !terms
                .iter()
                .any(|term| term == "iwo" || term == "vibecrafted")
        );
        assert_eq!(vocabulary.sources.active_names, 2);
        assert_eq!(vocabulary.sources.custom_canonicals, 2);
        assert_eq!(
            vocabulary.sources.protected_terms,
            ProtectedTerms::builtin().len() - 1
        );
    }

    #[test]
    fn unicode_casefold_deduplicates_and_trims_without_losing_spelling() {
        let vocabulary = RecognizerVocabulary::from_sources(
            owned(&[" Straße ", "Σ", "ŁÓDŹ"]),
            owned(&["STRASSE", "ς", "łódź", "", "  "]),
            &ProtectedTerms::default(),
        );
        assert_eq!(
            vocabulary.apple_contextual_strings().unwrap(),
            ["Straße", "ŁÓDŹ", "Σ"]
        );
        assert_eq!(vocabulary.sources.custom_canonicals, 0);
    }

    #[test]
    fn ordering_is_independent_of_source_iteration_order() {
        let first = RecognizerVocabulary::from_sources(
            owned(&["zoe", "Iwo", "Zoe"]),
            owned(&["worktrees", "Checkout", "checkout"]),
            &ProtectedTerms::builtin(),
        );
        let second = RecognizerVocabulary::from_sources(
            owned(&["Zoe", "Iwo", "zoe"]),
            owned(&["checkout", "Checkout", "worktrees"]),
            &ProtectedTerms::builtin(),
        );
        assert_eq!(
            first.apple_contextual_strings(),
            second.apple_contextual_strings()
        );
        assert_eq!(first.sources, second.sources);
    }

    #[test]
    fn apple_limit_applies_after_dedup_and_preserves_source_priority() {
        let custom = (0..150).map(|index| format!("Term{index:03}"));
        let vocabulary = RecognizerVocabulary::from_sources(
            owned(&["Iwo", "iwo"]),
            custom,
            &ProtectedTerms::builtin(),
        );
        let terms = vocabulary.apple_contextual_strings().unwrap();
        assert_eq!(terms.len(), 100);
        assert_eq!(terms[0], "Iwo");
        assert_eq!(terms[99], "Term098");
        assert_eq!(
            vocabulary.sources,
            VocabularySources {
                active_names: 1,
                custom_canonicals: 99,
                protected_terms: 0,
            }
        );
    }

    #[test]
    fn empty_sources_omit_context() {
        let vocabulary = RecognizerVocabulary::from_sources(
            owned(&[" "]),
            owned(&[""]),
            &ProtectedTerms::default(),
        );
        assert_eq!(vocabulary.apple_contextual_strings(), None);
        assert_eq!(vocabulary.sources, VocabularySources::default());
    }

    #[test]
    fn protected_file_extends_the_same_gate_vocabulary() {
        let directory = tempfile::tempdir().unwrap();
        let path = ProtectedTerms::default_path(directory.path());
        std::fs::write(&path, "# domain\nCheckout\n\nWORKTREES\ncheckout\n").unwrap();
        let protected = ProtectedTerms::load_from(&path);
        let vocabulary = RecognizerVocabulary::from_sources(Vec::new(), Vec::new(), &protected);
        let terms = vocabulary.apple_contextual_strings().unwrap();
        assert!(terms.iter().any(|term| term == "checkout"));
        assert!(terms.iter().any(|term| term == "worktrees"));
        assert!(terms.windows(2).all(|pair| pair[0] < pair[1]));
        assert_eq!(vocabulary.sources.protected_terms, protected.len());
    }
}

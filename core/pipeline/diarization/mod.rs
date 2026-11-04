//! Experimental speaker diarization and attribution cells.
//!
//! Speaker attribution is an independent metadata axis joined to immutable
//! [`crate::pipeline::acoustic_ledger::WordSlot`]s by sample-range overlap on the
//! capture clock. Nothing in this module modifies `WordSlot`, its `Eq`/`PartialEq`
//! semantics, or `AcousticLedger` occurrence identity.

pub mod nemotron3_bridge_contract;
pub mod nemotron3_coreml;
pub mod sealed_embedding;
pub mod speakrs_batch;

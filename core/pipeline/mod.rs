pub mod contracts;
pub mod dedup;
pub mod sinks;
pub mod stream_postprocess;
pub mod streaming;

// Re-export core event types for ergonomic access
pub use contracts::{DropKind, EngineEvent, EventSink};
pub use sinks::{CollectorEventSink, DeltaSinkAdapter};

#[cfg(test)]
mod tests;

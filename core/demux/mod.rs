//! Streaming tag demultiplexer: splits a model's token stream into tagged
//! channels as it arrives, without waiting for the response to complete.

pub mod parser;

pub use parser::{DemuxEvent, StreamingTagParser, TagKind};

//! Wire types shared across the process boundary between the Rust engine and
//! its hosts. Re-exported flat so callers spell `ipc::Foo`, not `ipc::types::Foo`.

pub mod types;
pub use types::*;

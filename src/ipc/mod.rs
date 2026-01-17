//! Unix Socket IPC for CLI ↔ GUI communication
//!
//! Socket path: /tmp/codescribe.sock

mod server;
mod types;

pub use server::run_server;
pub use types::{AppStatus, IpcCommand, IpcResponse};

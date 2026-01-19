//! Unix Socket IPC for CLI ↔ GUI communication
//!
//! Socket path: <config_dir>/ipc/codescribe.sock (user-only)

mod server;

pub use server::run_server;
// Re-export types from core
pub use codescribe_core::ipc::{AppStatus, IpcCommand, IpcResponse};

use std::path::PathBuf;

use crate::config::Config;

pub fn socket_path() -> PathBuf {
    Config::config_dir().join("ipc").join("codescribe.sock")
}

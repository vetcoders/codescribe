//! Unix Socket IPC for CLI ↔ GUI communication
//!
//! Socket path: <config_dir>/ipc/codescribe.sock (user-only)

mod server;

pub use server::run_server;
// Re-export types and the socket path from core: the server must bind exactly
// the path core's IpcClient connects to, so the computation lives once, in core.
pub use codescribe_core::ipc::{AppStatus, IpcCommand, IpcResponse, socket_path};

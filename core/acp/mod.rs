//! Agent Client Protocol v1: an owned stdio process and a permission-gated client.
//!
//! ACP agents execute their own tools. The bridge mediates the client methods
//! they request; it is not an OS sandbox for the agent executable itself.

pub mod client_bridge;
pub mod mapper;
pub mod protocol;
pub mod transport;

use std::collections::BTreeMap;
use std::fmt;
use std::path::PathBuf;

/// Explicit argv launch specification. No shell parses the command or arguments.
#[derive(Debug, Clone)]
pub struct AcpCommand {
    pub program: String,
    pub args: Vec<String>,
    pub cwd: PathBuf,
    pub env: BTreeMap<String, String>,
}

/// A controlled ACP failure, safe to render after the boundary's redaction.
#[derive(Debug)]
pub enum AcpError {
    Io(std::io::Error),
    MalformedFrame(String),
    Protocol(String),
    ProcessExited(String),
    PermissionDenied(String),
    Cancelled,
}

impl fmt::Display for AcpError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(f, "ACP I/O: {error}"),
            Self::MalformedFrame(detail) => write!(f, "ACP malformed frame: {detail}"),
            Self::Protocol(detail) => write!(f, "ACP protocol: {detail}"),
            Self::ProcessExited(detail) => write!(f, "ACP process exited: {detail}"),
            Self::PermissionDenied(detail) => write!(f, "ACP permission denied: {detail}"),
            Self::Cancelled => f.write_str("ACP turn cancelled"),
        }
    }
}

impl std::error::Error for AcpError {}

impl From<std::io::Error> for AcpError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

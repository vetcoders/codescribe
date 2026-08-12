//! Cross-cutting helpers with no home in a domain module: filesystem path
//! safety, child-pipe signal hygiene, and the process-wide status channel.

/// Per-fd SIGPIPE suppression — required because the core runs inside a Swift host.
pub mod pipes;
pub mod safe_path;
/// Process-wide status signal channel (Thinking/Error) for tray/bridge surfaces.
pub mod status;

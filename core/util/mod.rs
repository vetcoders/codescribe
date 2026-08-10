//! Cross-cutting helpers with no home in a domain module: filesystem path
//! safety and the process-wide status signal channel.

pub mod safe_path;
/// Process-wide status signal channel (Thinking/Error) for tray/bridge surfaces.
pub mod status;

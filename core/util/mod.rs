//! Cross-cutting helpers with no home in a domain module: filesystem path
//! safety and child-pipe signal hygiene.

/// Per-fd SIGPIPE suppression — required because the core runs inside a Swift host.
pub mod pipes;
pub mod safe_path;

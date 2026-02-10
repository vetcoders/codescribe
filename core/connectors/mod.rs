//! External content connectors for CodeScribe attachments.
//!
//! Each connector fetches content from an external source and produces
//! files on disk that become regular `Attachment` objects.

pub mod github;
pub mod web;

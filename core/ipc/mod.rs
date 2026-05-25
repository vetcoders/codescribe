pub mod client;
pub mod types;
pub use client::*;
pub use types::*;

use crate::config::Config;
use std::path::PathBuf;

pub fn socket_path() -> PathBuf {
    Config::config_dir().join("ipc").join("codescribe.sock")
}

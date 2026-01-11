pub mod app;
pub mod lab;
pub mod teacher;
pub mod settings;

#[cfg(target_arch = "wasm32")]
pub mod tauri;

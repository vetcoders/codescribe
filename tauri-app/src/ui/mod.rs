pub mod app;
pub mod lab;
pub mod settings;
pub mod teacher;

#[cfg(target_arch = "wasm32")]
pub mod tauri;

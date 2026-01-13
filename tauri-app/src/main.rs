#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    #[cfg(not(target_arch = "wasm32"))]
    codescribe_app::run_backend();
}

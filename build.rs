use std::env;
use std::fs;
use std::path::PathBuf;

fn main() {
    // Save repo path during build (for cargo install --path .)
    // This helps the app launcher find the Python backend
    if let Ok(manifest_dir) = env::var("CARGO_MANIFEST_DIR") {
        let codescribe_dir = dirs::home_dir()
            .map(|h| h.join(".codescribe"))
            .unwrap_or_else(|| PathBuf::from("/tmp/.codescribe"));

        // Only write if building in release mode (cargo install uses release)
        if env::var("PROFILE").map(|p| p == "release").unwrap_or(false) {
            let _ = fs::create_dir_all(&codescribe_dir);
            let repo_path_file = codescribe_dir.join("repo_path");
            let _ = fs::write(&repo_path_file, &manifest_dir);
            println!("cargo:warning=Saved repo path: {}", manifest_dir);
        }
    }

    // Re-run if Cargo.toml changes
    println!("cargo:rerun-if-changed=Cargo.toml");
}

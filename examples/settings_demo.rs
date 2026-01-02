//! Demonstration of the Settings module.
//!
//! Run with: cargo run --example settings_demo

use codescribe::settings::Settings;

fn main() {
    println!("=== Settings Demo ===\n");

    // Load settings (will use defaults if file doesn't exist)
    println!("Loading settings...");
    let mut settings = Settings::load();

    println!("Current settings:");
    println!("  Language: {}", settings.language);
    println!("  Hotkeys enabled: {}", settings.hotkeys_enabled);
    println!("  Formatting enabled: {}", settings.formatting_enabled);
    println!("  Sound enabled: {}", settings.sound_enabled);
    println!();

    // Modify settings
    println!("Modifying settings...");
    settings.language = "pl".to_string();
    settings.sound_enabled = false;

    // Save to disk
    println!("Saving to ~/.CodeScribe/settings.json...");
    match settings.save() {
        Ok(_) => println!("Settings saved successfully!"),
        Err(e) => eprintln!("Error saving settings: {}", e),
    }
    println!();

    // Load again to verify
    println!("Reloading settings to verify...");
    let reloaded = Settings::load();
    println!("Reloaded settings:");
    println!("  Language: {}", reloaded.language);
    println!("  Hotkeys enabled: {}", reloaded.hotkeys_enabled);
    println!("  Formatting enabled: {}", reloaded.formatting_enabled);
    println!("  Sound enabled: {}", reloaded.sound_enabled);
}

//! Demo of the config module capabilities
//!
//! Run with: cargo run --example config_demo

use codescribe::config::{Config, HoldMods, Language};

fn main() -> anyhow::Result<()> {
    println!("CodeScribe Config Demo\n");

    // Load config (from .env or defaults)
    let config = Config::load();
    println!("Loaded config:");
    println!("  Hold mods: {:?}", config.hold_mods);
    println!("  Language: {:?}", config.whisper_language);
    println!("  Beep on start: {}", config.beep_on_start);
    println!("  Sound name: {}", config.sound_name);
    println!("  Sound volume: {}", config.sound_volume);
    println!();

    // Demonstrate enum parsing
    println!("Enum parsing examples:");
    println!(
        "  \"ctrl_alt\".parse::<HoldMods>() = {:?}",
        "ctrl_alt".parse::<HoldMods>()
    );
    println!(
        "  \"pl\".parse::<Language>() = {:?}",
        "pl".parse::<Language>()
    );
    println!();

    // Demonstrate single-value save
    println!("Updating single value (BEEP_ON_START=false)...");
    config.save_to_env("BEEP_ON_START", "false")?;
    println!("Config saved to: {:?}", Config::env_path());
    println!();

    // Load again to verify
    let reloaded = Config::load();
    println!("Reloaded config:");
    println!(
        "  Beep on start: {} (should be false)",
        reloaded.beep_on_start
    );
    println!();

    println!("Demo complete!");
    Ok(())
}

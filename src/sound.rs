//! System sound playback for macOS
//!
//! Provides simple sound feedback using macOS system sounds.

use tracing::debug;

/// Play a system sound by name
///
/// # Arguments
/// * `name` - Name of the system sound (e.g., "Tink", "Pop", "Glass")
///
/// # Platform Support
/// - macOS: Uses `afplay` with system sounds from `/System/Library/Sounds/`
/// - Other platforms: No-op (silent)
///
/// # Examples
/// ```no_run
/// play_sound("Tink");  // Plays confirmation beep
/// play_sound("Pop");   // Plays pop sound
/// ```
#[cfg(target_os = "macos")]
pub fn play_sound(name: &str) {
    use std::process::Command;

    debug!("Playing system sound: {}", name);

    let path = format!("/System/Library/Sounds/{}.aiff", name);

    // Spawn afplay in background, don't wait for completion
    match Command::new("afplay").arg(&path).spawn() {
        Ok(_) => debug!("Sound playback started: {}", name),
        Err(e) => debug!("Failed to play sound {}: {}", name, e),
    }
}

#[cfg(not(target_os = "macos"))]
pub fn play_sound(_name: &str) {
    // No-op on non-macOS platforms
}

/// Play a system sound by name with specified volume
///
/// # Arguments
/// * `name` - Name of the system sound (e.g., "Tink", "Pop", "Glass")
/// * `volume` - Volume level from 0.0 (mute) to 1.0 (full)
///
/// # Platform Support
/// - macOS: Uses `afplay -v` with system sounds from `/System/Library/Sounds/`
/// - Other platforms: No-op (silent)
#[cfg(target_os = "macos")]
pub fn play_sound_with_volume(name: &str, volume: f32) {
    use std::process::Command;

    let volume = volume.clamp(0.0, 1.0);
    debug!("Playing system sound: {} at volume {:.2}", name, volume);

    if volume == 0.0 {
        debug!("Volume is muted, skipping sound playback");
        return;
    }

    let path = format!("/System/Library/Sounds/{}.aiff", name);

    // Spawn afplay with volume in background, don't wait for completion
    match Command::new("afplay")
        .arg("-v")
        .arg(volume.to_string())
        .arg(&path)
        .spawn()
    {
        Ok(_) => debug!("Sound playback started: {} at volume {:.2}", name, volume),
        Err(e) => debug!("Failed to play sound {}: {}", name, e),
    }
}

#[cfg(not(target_os = "macos"))]
pub fn play_sound_with_volume(_name: &str, _volume: f32) {
    // No-op on non-macOS platforms
}

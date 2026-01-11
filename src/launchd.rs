//! macOS launchd integration for "Start at Login" functionality
//!
//! This module manages a LaunchAgent plist in ~/Library/LaunchAgents/ to enable
//! CodeScribe to start automatically when the user logs in.

use anyhow::{Context, Result};
use std::path::PathBuf;
use tracing::{error, info};

const PLIST_LABEL: &str = "io.loctree.codescribe";
const PLIST_FILENAME: &str = "io.loctree.codescribe.plist";

/// Get the path to the LaunchAgent plist file
pub fn get_plist_path() -> Result<PathBuf> {
    let home = std::env::var("HOME").context("HOME environment variable not set")?;
    let launch_agents_dir = PathBuf::from(home).join("Library/LaunchAgents");

    // Ensure the LaunchAgents directory exists
    std::fs::create_dir_all(&launch_agents_dir)
        .context("Failed to create ~/Library/LaunchAgents directory")?;

    Ok(launch_agents_dir.join(PLIST_FILENAME))
}

/// Check if CodeScribe is configured to start at login
pub fn is_login_item_enabled() -> bool {
    match get_plist_path() {
        Ok(path) => path.exists(),
        Err(e) => {
            error!("Failed to get plist path: {}", e);
            false
        }
    }
}

/// Enable "Start at Login" by creating a LaunchAgent plist
pub fn enable_login_item() -> Result<()> {
    let plist_path = get_plist_path()?;

    // Get the path to the current executable
    let exe_path = std::env::current_exe().context("Failed to get current executable path")?;

    let exe_str = exe_path
        .to_str()
        .context("Executable path is not valid UTF-8")?;

    // Create the plist XML content
    let plist_content = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>{}</string>
    <key>ProgramArguments</key>
    <array>
        <string>{}</string>
    </array>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <false/>
</dict>
</plist>
"#,
        PLIST_LABEL, exe_str
    );

    // Write the plist file
    std::fs::write(&plist_path, plist_content)
        .with_context(|| format!("Failed to write plist to {}", plist_path.display()))?;

    info!("Created LaunchAgent plist at: {}", plist_path.display());

    // Load the LaunchAgent (macOS will automatically load it on next login)
    // Using launchctl load to make it active immediately
    let output = std::process::Command::new("launchctl")
        .arg("load")
        .arg(&plist_path)
        .output()
        .context("Failed to execute launchctl load")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        error!("launchctl load failed: {}", stderr);
        // Don't fail completely - the plist is still created for next login
    } else {
        info!("LaunchAgent loaded successfully");
    }

    Ok(())
}

/// Disable "Start at Login" by removing the LaunchAgent plist
pub fn disable_login_item() -> Result<()> {
    let plist_path = get_plist_path()?;

    if !plist_path.exists() {
        info!("LaunchAgent plist does not exist, nothing to disable");
        return Ok(());
    }

    // Unload the LaunchAgent first (if currently loaded)
    let output = std::process::Command::new("launchctl")
        .arg("unload")
        .arg(&plist_path)
        .output()
        .context("Failed to execute launchctl unload")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        // This might fail if the agent isn't currently loaded - that's OK
        info!("launchctl unload warning: {}", stderr);
    } else {
        info!("LaunchAgent unloaded successfully");
    }

    // Remove the plist file
    std::fs::remove_file(&plist_path)
        .with_context(|| format!("Failed to remove plist at {}", plist_path.display()))?;

    info!("Removed LaunchAgent plist from: {}", plist_path.display());

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_get_plist_path() {
        let path = get_plist_path().expect("Should get plist path");
        assert!(path.to_string_lossy().contains("Library/LaunchAgents"));
        assert!(
            path.to_string_lossy()
                .ends_with("io.loctree.codescribe.plist")
        );
    }

    #[test]
    fn test_is_login_item_enabled() {
        // Should not panic even if plist doesn't exist
        let _ = is_login_item_enabled();
    }
}

//! Native macOS dialog utilities
//!
//! Provides native NSAlert dialogs for user interactions.

use tracing::{debug, info};

/// Result of the quit confirmation dialog
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuitChoice {
    /// Close the tray app AND stop the backend server
    CloseAll,
    /// Close the tray app but leave the backend server running
    LeaveServerRunning,
    /// User cancelled the quit action
    Cancel,
}

/// Show the quit confirmation dialog using osascript (safe from any thread)
///
/// Returns the user's choice:
/// - `CloseAll` - stop backend and exit
/// - `LeaveServerRunning` - exit tray only
/// - `Cancel` - abort quit
#[cfg(target_os = "macos")]
pub fn show_quit_dialog() -> QuitChoice {
    use std::process::Command;

    info!("Showing quit confirmation dialog via osascript");

    // Use osascript to show dialog - works from any thread
    let script = r#"
        set dialogResult to display dialog "Quit CodeScribe?

CodeScribe can keep the transcription server running in the background so you can quickly restart without waiting for model loading.

What would you like to do?" buttons {"Cancel", "Leave Server Running", "Close All"} default button "Close All" with title "CodeScribe" with icon caution
        return button returned of dialogResult
    "#;

    let output = Command::new("osascript").arg("-e").arg(script).output();

    match output {
        Ok(out) if out.status.success() => {
            let response = String::from_utf8_lossy(&out.stdout).trim().to_string();
            info!("Dialog response: {}", response);
            match response.as_str() {
                "Close All" => QuitChoice::CloseAll,
                "Leave Server Running" => QuitChoice::LeaveServerRunning,
                _ => QuitChoice::Cancel,
            }
        }
        Ok(out) => {
            // User clicked Cancel or closed dialog
            debug!("Dialog cancelled or closed: {:?}", out.status);
            QuitChoice::Cancel
        }
        Err(e) => {
            debug!("osascript failed: {}, defaulting to CloseAll", e);
            QuitChoice::CloseAll
        }
    }
}

/// Fallback for non-macOS platforms
#[cfg(not(target_os = "macos"))]
pub fn show_quit_dialog() -> QuitChoice {
    // On non-macOS, just close everything
    QuitChoice::CloseAll
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_quit_choice_enum() {
        assert_ne!(QuitChoice::CloseAll, QuitChoice::LeaveServerRunning);
        assert_ne!(QuitChoice::CloseAll, QuitChoice::Cancel);
        assert_ne!(QuitChoice::LeaveServerRunning, QuitChoice::Cancel);
    }
}

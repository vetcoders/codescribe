//! Python backend server management
//!
//! Spawns and manages the Python whisper_server subprocess.
//! Automatically starts on launch and stops on exit.

use anyhow::{Context, Result};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::Duration;
use tracing::{debug, error, info, warn};

/// Default port for the whisper server
const DEFAULT_PORT: u16 = 8238;

/// Maximum time to wait for server startup
const STARTUP_TIMEOUT: Duration = Duration::from_secs(30);

/// Interval between health check attempts
const HEALTH_CHECK_INTERVAL: Duration = Duration::from_millis(500);

/// Manages the Python backend server process
pub struct BackendServer {
    process: Option<Child>,
    port: u16,
}

impl BackendServer {
    /// Start the Python whisper_server
    pub fn start() -> Result<Self> {
        let port = DEFAULT_PORT;

        // First check if a backend is already running
        if Self::check_existing_backend(port) {
            info!("Found existing backend on port {} - reusing it", port);
            return Ok(Self {
                process: None, // We don't own this process
                port,
            });
        }

        // Find the whisper_server.py script
        let script_path = find_whisper_server()?;
        info!("Starting Python backend from: {}", script_path.display());

        // Spawn the Python process
        let process = Command::new("uv")
            .args(["run", "python", script_path.to_str().unwrap()])
            .env("PORT", port.to_string())
            .env("HOST", "127.0.0.1")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .context("Failed to spawn Python backend - is 'uv' installed?")?;

        info!("Python backend spawned with PID: {}", process.id());

        let server = Self {
            process: Some(process),
            port,
        };

        // Wait for server to be ready
        server.wait_for_ready()?;

        Ok(server)
    }

    /// Wait for the server to respond to health checks
    fn wait_for_ready(&self) -> Result<()> {
        let client = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(2))
            .build()?;

        let health_url = format!("http://127.0.0.1:{}/healthz", self.port);
        let start = std::time::Instant::now();

        info!("Waiting for backend to be ready...");

        while start.elapsed() < STARTUP_TIMEOUT {
            match client.get(&health_url).send() {
                Ok(response) if response.status().is_success() => {
                    info!("Backend is ready on port {}", self.port);
                    return Ok(());
                }
                Ok(response) => {
                    debug!("Backend not ready yet: status {}", response.status());
                }
                Err(e) => {
                    debug!("Backend not ready yet: {}", e);
                }
            }
            std::thread::sleep(HEALTH_CHECK_INTERVAL);
        }

        Err(anyhow::anyhow!(
            "Backend failed to start within {} seconds. Check if 'uv' is installed and whisper_server.py is accessible.",
            STARTUP_TIMEOUT.as_secs()
        ))
    }

    /// Get the server port
    pub fn port(&self) -> u16 {
        self.port
    }

    /// Check if a backend is already running on the given port
    fn check_existing_backend(port: u16) -> bool {
        let client = match reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(2))
            .build()
        {
            Ok(c) => c,
            Err(_) => return false,
        };

        let health_url = format!("http://127.0.0.1:{}/healthz", port);
        debug!("Checking for existing backend at {}", health_url);

        match client.get(&health_url).send() {
            Ok(response) if response.status().is_success() => {
                debug!("Existing backend found and healthy on port {}", port);
                true
            }
            _ => false,
        }
    }

    /// Stop the backend server
    pub fn stop(&mut self) {
        if let Some(mut process) = self.process.take() {
            info!("Stopping Python backend (PID: {})...", process.id());

            // Try graceful shutdown first
            #[cfg(unix)]
            {
                use std::os::unix::process::CommandExt;
                let _ = Command::new("kill")
                    .args(["-TERM", &process.id().to_string()])
                    .exec();
            }

            // Give it a moment to shut down gracefully
            std::thread::sleep(Duration::from_millis(500));

            // Force kill if still running
            match process.try_wait() {
                Ok(Some(_)) => {
                    info!("Backend stopped gracefully");
                }
                _ => {
                    warn!("Backend didn't stop gracefully, forcing kill");
                    let _ = process.kill();
                    let _ = process.wait();
                }
            }
        }
    }
}

impl Drop for BackendServer {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Find the whisper_server.py script
fn find_whisper_server() -> Result<PathBuf> {
    // Try relative to executable first
    let exe_path = std::env::current_exe()?;
    let exe_dir = exe_path.parent().unwrap_or(&exe_path);

    // Possible locations (in order of preference)
    let candidates = [
        // Development: relative to cargo project
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../whisper_server.py"),
        // Installed alongside binary
        exe_dir.join("whisper_server.py"),
        // Current working directory
        std::env::current_dir()?.join("whisper_server.py"),
        // Parent of current directory (monorepo structure)
        std::env::current_dir()?.join("../whisper_server.py"),
        // Absolute fallback for development
        PathBuf::from("/Users/maciejgad/hosted/Loctree-Repos/Codescribe/whisper_server.py"),
    ];

    for path in &candidates {
        let normalized = path.canonicalize().unwrap_or_else(|_| path.clone());
        if normalized.exists() {
            debug!("Found whisper_server.py at: {}", normalized.display());
            return Ok(normalized);
        }
    }

    // List what we tried
    error!("Could not find whisper_server.py. Tried:");
    for path in &candidates {
        error!("  - {}", path.display());
    }

    Err(anyhow::anyhow!(
        "whisper_server.py not found. Ensure you're running from the CodeScribe directory."
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_find_whisper_server() {
        // This will fail in CI but should work locally
        let result = find_whisper_server();
        // Just check it doesn't panic
        let _ = result;
    }
}

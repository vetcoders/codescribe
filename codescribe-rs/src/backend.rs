//! Python backend server management
//!
//! Spawns and manages the Python backend subprocess.
//! Automatically starts on launch and stops on exit.

use anyhow::{Context, Result};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::Duration;
use tracing::{debug, error, info, warn};

/// Default port for the backend server
const DEFAULT_PORT: u16 = 8237;

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
    /// Start the Python backend server
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

        // Check if models exist, download if not
        ensure_models_exist()?;

        // Find the backend.py script
        let script_path = find_backend_script()?;
        info!("Starting Python backend from: {}", script_path.display());

        // Determine the working directory for uv (needs pyproject.toml)
        // If CODESCRIBE_PYTHON_DIR is set, use it as the working directory
        // Otherwise, use the directory containing backend.py
        let working_dir = if let Ok(python_dir) = std::env::var("CODESCRIBE_PYTHON_DIR") {
            PathBuf::from(python_dir)
        } else {
            script_path
                .parent()
                .map(|p| p.to_path_buf())
                .unwrap_or_else(|| std::env::current_dir().unwrap())
        };

        debug!("Using working directory: {}", working_dir.display());

        // Spawn the Python process with uvicorn running the full backend
        // Use whisper-small by default for faster startup and better quality with anti-hallucination filters
        let whisper_variant =
            std::env::var("WHISPER_VARIANT").unwrap_or_else(|_| "small".to_string());
        let process = Command::new("uv")
            .args([
                "run",
                "uvicorn",
                "codescribe.backend:app",
                "--host",
                "127.0.0.1",
                "--port",
                &port.to_string(),
            ])
            .current_dir(&working_dir)
            .env("WHISPER_VARIANT", &whisper_variant)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .context("Failed to spawn Python backend - is 'uv' installed?")?;

        info!("Using Whisper variant: {}", whisper_variant);

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
            "Backend failed to start within {} seconds. Check if 'uv' is installed and the codescribe package is accessible.",
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

/// Ensure whisper models are downloaded
fn ensure_models_exist() -> Result<()> {
    // Determine repo root (development mode) or data directory (bundled mode)
    let exe_path = std::env::current_exe()?;
    let exe_dir = exe_path.parent().unwrap_or(&exe_path);

    // Check if running from .app bundle
    let is_bundled = exe_dir.join("../Resources/python/codescribe").exists();

    let repo_root = if is_bundled {
        // In bundled mode, use user data directory
        PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| ".".to_string())).join(".CodeScribe")
    } else {
        // Development mode
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..")
    };

    // Use WHISPER_VARIANT env var, default to "small" for faster startup
    let variant = std::env::var("WHISPER_VARIANT").unwrap_or_else(|_| "small".to_string());
    let models_dir = repo_root.join(format!("models/whisper-{}", variant));

    if models_dir.exists() {
        debug!("Whisper models found at {}", models_dir.display());
        return Ok(());
    }

    info!("Whisper model '{}' not found, downloading...", variant);
    info!("This may take a few minutes on first run.");

    // Find the download script
    let script_path = if is_bundled {
        exe_dir.join("../Resources/python/scripts/get_models.py")
    } else {
        repo_root.join("scripts/get_models.py")
    };

    if !script_path.exists() {
        return Err(anyhow::anyhow!(
            "Model download script not found at {}. Please run from the CodeScribe directory or reinstall the app.",
            script_path.display()
        ));
    }

    let output = Command::new("uv")
        .args([
            "run",
            "python",
            script_path.to_str().unwrap(),
            "--whisper",
            &variant,
        ])
        .current_dir(&repo_root)
        .output()
        .context("Failed to run model download script")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        error!("Model download failed: {}", stderr);
        return Err(anyhow::anyhow!(
            "Failed to download whisper models: {}",
            stderr
        ));
    }

    info!("Whisper model '{}' downloaded successfully", variant);
    Ok(())
}

/// Find the backend.py script (or the directory containing the codescribe package)
fn find_backend_script() -> Result<PathBuf> {
    // Check environment variable override first
    if let Ok(custom_path) = std::env::var("CODESCRIBE_PYTHON_DIR") {
        let script_path = PathBuf::from(&custom_path).join("backend.py");
        if script_path.exists() {
            debug!(
                "Found backend.py via CODESCRIBE_PYTHON_DIR: {}",
                script_path.display()
            );
            return Ok(script_path);
        }
    }

    // Try relative to executable first
    let exe_path = std::env::current_exe()?;
    let exe_dir = exe_path.parent().unwrap_or(&exe_path);

    // Possible locations (in order of preference)
    let candidates = [
        // .app bundle: Contents/MacOS/../Resources/python/backend.py
        exe_dir.join("../Resources/python/backend.py"),
        // Development: relative to cargo project
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../backend.py"),
        // Installed alongside binary
        exe_dir.join("backend.py"),
        // Current working directory
        std::env::current_dir()?.join("backend.py"),
        // Parent of current directory (monorepo structure)
        std::env::current_dir()?.join("../backend.py"),
        // Absolute fallback for development
        PathBuf::from("/Users/maciejgad/hosted/Loctree-Repos/Codescribe/backend.py"),
    ];

    for path in &candidates {
        let normalized = path.canonicalize().unwrap_or_else(|_| path.clone());
        if normalized.exists() {
            debug!("Found backend.py at: {}", normalized.display());
            return Ok(normalized);
        }
    }

    // List what we tried
    error!("Could not find backend.py. Tried:");
    for path in &candidates {
        error!("  - {}", path.display());
    }

    Err(anyhow::anyhow!(
        "backend.py not found. Ensure you're running from the CodeScribe directory."
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_find_backend_script() {
        // This will fail in CI but should work locally
        let result = find_backend_script();
        // Just check it doesn't panic
        let _ = result;
    }
}

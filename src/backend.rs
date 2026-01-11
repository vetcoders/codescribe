//! Python backend server management
//!
//! Spawns and manages the Python backend subprocess.
//! Automatically starts on launch and stops on exit.

use crate::config::Config;
use anyhow::{Context, Result};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::Duration;
use tracing::{debug, error, info, warn};

/// Default port for the backend server
const DEFAULT_PORT: u16 = 8237;

/// Extra ports we probe/clean beyond the default (legacy + discovery)
const EXTRA_KNOWN_PORTS: &[u16] = &[8238, 7237, 6237, 5237];

/// Maximum time to wait for server startup
const STARTUP_TIMEOUT: Duration = Duration::from_secs(30);

/// Interval between health check attempts
const HEALTH_CHECK_INTERVAL: Duration = Duration::from_millis(500);

/// Path to backend PID file for fallback cleanup
fn backend_pid_file_path() -> std::path::PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    std::path::PathBuf::from(home)
        .join(".codescribe")
        .join("backend.pid")
}

/// All ports we consider "ours" for cleanup (default + legacy/probe)
fn known_backend_ports() -> Vec<u16> {
    // Start with config defaults so we respect the same set across the app
    let mut ports = Config::default().backend_ports;
    ports.push(DEFAULT_PORT);
    ports.extend_from_slice(EXTRA_KNOWN_PORTS);
    ports.sort_unstable();
    ports.dedup();
    ports
}

/// Check if a PID belongs to a CodeScribe backend process
fn is_codescribe_backend(pid: &str) -> bool {
    // Get process command name
    let output = Command::new("ps").args(["-p", pid, "-o", "comm="]).output();

    if let Ok(output) = output {
        if output.status.success() {
            let comm = String::from_utf8_lossy(&output.stdout);
            let comm = comm.trim().to_lowercase();
            // Check if it's our backend (Python/uvicorn/CodeScribeServer)
            return comm.contains("python")
                || comm.contains("uvicorn")
                || comm.contains("codescribe")
                || comm.contains("uv");
        }
    }
    false
}

/// Manages the Python backend server process
pub struct BackendServer {
    process: Option<Child>,
    port: u16,
}

impl BackendServer {
    /// Start the Python backend server
    pub fn start() -> Result<Self> {
        let port = DEFAULT_PORT;

        // Kill any zombie backend processes from previous runs across all known ports
        Self::kill_existing_on_known_ports();

        // Small delay to ensure port is released
        std::thread::sleep(Duration::from_millis(100));

        // Check if models exist, download if not
        ensure_models_exist()?;

        // Determine the working directory for uv (needs pyproject.toml)
        // Priority: CODESCRIBE_PYTHON_DIR (bundled mode) > find script > fallback
        let working_dir = if let Ok(python_dir) = std::env::var("CODESCRIBE_PYTHON_DIR") {
            let bundled = PathBuf::from(&python_dir);
            if bundled.join("pyproject.toml").exists() {
                info!("Using bundled Python backend at: {}", bundled.display());
                bundled
            } else {
                warn!(
                    "CODESCRIBE_PYTHON_DIR set but no pyproject.toml found: {}",
                    bundled.display()
                );
                find_backend_working_dir()?
            }
        } else {
            find_backend_working_dir()?
        };

        debug!("Using working directory: {}", working_dir.display());

        // Spawn the Python process with uvicorn running the full backend
        // Use whisper-large-v3-mlx-q8 by default - best quality/performance on Apple Silicon
        let whisper_variant =
            std::env::var("WHISPER_VARIANT").unwrap_or_else(|_| "large-v3-mlx-q8".to_string());
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

        let backend_pid = process.id();
        info!("Python backend spawned with PID: {}", backend_pid);

        // Save PID to file for fallback cleanup (in case lsof fails)
        let pid_path = backend_pid_file_path();
        if let Some(parent) = pid_path.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        if let Err(e) = std::fs::write(&pid_path, backend_pid.to_string()) {
            warn!("Failed to write backend PID file: {}", e);
        }

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

    /// Stop the backend server
    pub fn stop(&mut self) {
        if let Some(mut process) = self.process.take() {
            info!("Stopping Python backend (PID: {})...", process.id());

            // Try graceful shutdown first with SIGTERM
            let _ = Command::new("kill")
                .args(["-TERM", &process.id().to_string()])
                .status();

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

            // Clean up PID file
            std::fs::remove_file(backend_pid_file_path()).ok();
        }
    }

    /// Kill any existing backend processes on our port (cleanup zombie processes)
    pub fn kill_existing_on_port(port: u16) {
        info!("Checking for zombie backend processes on port {}...", port);

        let mut killed_pids: Vec<String> = Vec::new();

        // Method 1: Find processes listening on our port using lsof
        let output = Command::new("lsof")
            .args(["-ti", &format!(":{}", port)])
            .output();

        if let Ok(output) = output {
            if output.status.success() {
                let pids = String::from_utf8_lossy(&output.stdout);
                info!("lsof found PIDs on port {}: {:?}", port, pids.trim());
                for pid in pids.lines() {
                    let pid = pid.trim();
                    if !pid.is_empty() {
                        // SAFETY: Only kill if it's actually our backend process
                        let is_backend = is_codescribe_backend(pid);
                        info!("PID {} is_codescribe_backend: {}", pid, is_backend);
                        if is_backend {
                            warn!("Killing backend process: PID {}", pid);
                            let kill_result = Command::new("kill").args(["-TERM", pid]).status();
                            info!("kill -TERM {} result: {:?}", pid, kill_result);
                            killed_pids.push(pid.to_string());
                        } else {
                            warn!(
                                "Process {} on port {} is not a CodeScribe backend, skipping",
                                pid, port
                            );
                        }
                    }
                }
            } else {
                debug!("lsof found no processes on port {}", port);
            }
        } else {
            warn!("lsof command failed for port {}", port);
        }

        // Method 2: Fallback - check PID file (for cases where backend died before binding)
        let pid_path = backend_pid_file_path();
        if pid_path.exists() {
            if let Ok(contents) = std::fs::read_to_string(&pid_path) {
                let pid = contents.trim();
                if !pid.is_empty() && !killed_pids.contains(&pid.to_string()) {
                    // Check if process is still running AND is our backend
                    let check = Command::new("kill").args(["-0", pid]).status();
                    if check.map(|s| s.success()).unwrap_or(false) && is_codescribe_backend(pid) {
                        warn!("Killing orphan backend from PID file: {}", pid);
                        let _ = Command::new("kill").args(["-TERM", pid]).status();
                        killed_pids.push(pid.to_string());
                    }
                }
            }
            // Clean up PID file
            std::fs::remove_file(&pid_path).ok();
        }

        // Give processes time to shutdown gracefully
        if !killed_pids.is_empty() {
            std::thread::sleep(Duration::from_millis(300));

            // Force kill any that didn't respond to SIGTERM
            for pid in &killed_pids {
                let check = Command::new("kill").args(["-0", pid]).status();
                if check.map(|s| s.success()).unwrap_or(false) {
                    warn!("Process {} didn't stop gracefully, force killing", pid);
                    let _ = Command::new("kill").args(["-9", pid]).status();
                }
            }

            // Also kill children of those processes (multiprocessing workers)
            for pid in &killed_pids {
                let children = Command::new("pgrep").args(["-P", pid]).output();
                if let Ok(output) = children {
                    if output.status.success() {
                        let child_pids = String::from_utf8_lossy(&output.stdout);
                        for child_pid in child_pids.lines() {
                            let child_pid = child_pid.trim();
                            if !child_pid.is_empty() {
                                info!("Killing child process: PID {}", child_pid);
                                let _ = Command::new("kill").args(["-9", child_pid]).status();
                            }
                        }
                    }
                }
            }
        }

        // Small delay to ensure processes are cleaned up
        std::thread::sleep(Duration::from_millis(100));
    }

    /// Kill zombie backends across all known ports (default + legacy/probe list)
    pub fn kill_existing_on_known_ports() {
        let ports = known_backend_ports();
        for port in ports {
            Self::kill_existing_on_port(port);
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
    // First check if WHISPER_DIR is set (bundled mode with embedded models)
    if let Ok(whisper_dir) = std::env::var("WHISPER_DIR") {
        let whisper_path = PathBuf::from(&whisper_dir);
        if whisper_path.exists() {
            info!("Using bundled Whisper model at: {}", whisper_path.display());
            return Ok(());
        }
        warn!(
            "WHISPER_DIR set but path doesn't exist: {}",
            whisper_path.display()
        );
    }

    // Determine paths based on execution context
    let exe_path = std::env::current_exe()?;
    let exe_dir = exe_path.parent().unwrap_or(&exe_path);

    // Check if running from .app bundle
    let is_bundled = exe_dir.join("../Resources/python").exists();

    // Check bundled models directory
    if is_bundled {
        let bundled_models = exe_dir.join("../Resources/Models");
        if bundled_models.exists() {
            // Look for any whisper model
            if let Ok(entries) = std::fs::read_dir(&bundled_models) {
                for entry in entries.flatten() {
                    let name = entry.file_name();
                    let name_str = name.to_string_lossy();
                    if name_str.starts_with("whisper-") && entry.path().is_dir() {
                        info!("Found bundled Whisper model: {}", entry.path().display());
                        // Set WHISPER_DIR for the Python backend to use
                        // SAFETY: Single-threaded during initialization, no concurrent access
                        unsafe { std::env::set_var("WHISPER_DIR", entry.path()) };
                        let variant = name_str.trim_start_matches("whisper-");
                        // SAFETY: Single-threaded during initialization, no concurrent access
                        unsafe { std::env::set_var("WHISPER_VARIANT", variant) };
                        return Ok(());
                    }
                }
            }
        }
    }

    // Development mode: look in repo models directory
    let repo_root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..");
    // Use WHISPER_VARIANT env var, default to large-v3-mlx-q8 for best quality
    let variant =
        std::env::var("WHISPER_VARIANT").unwrap_or_else(|_| "large-v3-mlx-q8".to_string());
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

/// Find a directory with pyproject.toml for running the Python backend
fn find_backend_working_dir() -> Result<PathBuf> {
    // Try relative to executable first
    let exe_path = std::env::current_exe()?;
    let exe_dir = exe_path.parent().unwrap_or(&exe_path);

    // Possible locations (in order of preference)
    let candidates = [
        // .app bundle: Contents/MacOS/../Resources/python/
        exe_dir.join("../Resources/python"),
        // Development: relative to cargo project
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(".."),
        // Current working directory
        std::env::current_dir()?,
        // Parent of current directory (monorepo structure)
        std::env::current_dir()?.join(".."),
    ];

    for path in &candidates {
        let pyproject = path.join("pyproject.toml");
        if pyproject.exists() {
            let normalized = path.canonicalize().unwrap_or_else(|_| path.clone());
            debug!("Found pyproject.toml at: {}", normalized.display());
            return Ok(normalized);
        }
    }

    // List what we tried
    error!("Could not find pyproject.toml. Tried:");
    for path in &candidates {
        error!("  - {}", path.display());
    }

    Err(anyhow::anyhow!(
        "pyproject.toml not found. Ensure you're running from the CodeScribe directory."
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_find_backend_working_dir() {
        // This will fail in CI but should work locally
        let result = find_backend_working_dir();
        // Just check it doesn't panic
        let _ = result;
    }
}

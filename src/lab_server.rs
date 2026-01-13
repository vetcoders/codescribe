//! Minimal HTTP server for Lab UI
//!
//! Serves static files from assets/lab/ directory.
//! No external dependencies - just tokio TCP.
//!
//! NOTE: Currently unused - will be activated when Tauri frontend is integrated.

#![allow(dead_code)]

use std::path::PathBuf;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;
use tracing::{debug, error, info};

const LAB_PORT: u16 = 8237;

/// Get the lab assets directory
fn lab_assets_dir() -> PathBuf {
    // Try relative to executable first
    if let Ok(exe) = std::env::current_exe() {
        if let Some(parent) = exe.parent() {
            let assets = parent.join("assets").join("lab");
            if assets.exists() {
                return assets;
            }
            // Try one level up (for dev builds)
            let assets = parent.parent().map(|p| p.join("assets").join("lab"));
            if let Some(ref a) = assets {
                if a.exists() {
                    return a.clone();
                }
            }
        }
    }

    // Try repo_path from ~/.codescribe/repo_path (set by build.rs during cargo install)
    if let Ok(home) = std::env::var("HOME") {
        let repo_path_file = PathBuf::from(&home).join(".codescribe").join("repo_path");
        if let Ok(repo_path) = std::fs::read_to_string(&repo_path_file) {
            let assets = PathBuf::from(repo_path.trim()).join("assets").join("lab");
            if assets.exists() {
                return assets;
            }
        }
    }

    // Fallback to cwd
    PathBuf::from("assets/lab")
}

/// MIME type for file extension
fn mime_type(path: &str) -> &'static str {
    match path.rsplit('.').next() {
        Some("html") => "text/html; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("js") => "application/javascript; charset=utf-8",
        Some("json") => "application/json; charset=utf-8",
        Some("png") => "image/png",
        Some("svg") => "image/svg+xml",
        Some("ico") => "image/x-icon",
        _ => "application/octet-stream",
    }
}

/// Start the lab server (non-blocking, spawns tokio task)
pub fn start_lab_server() {
    tokio::spawn(async {
        if let Err(e) = run_server().await {
            error!("Lab server error: {}", e);
        }
    });
}

/// Get lab URL
pub fn lab_url() -> String {
    format!("http://127.0.0.1:{}/", LAB_PORT)
}

async fn run_server() -> anyhow::Result<()> {
    let addr = format!("127.0.0.1:{}", LAB_PORT);
    let listener = TcpListener::bind(&addr).await?;
    let assets_dir = lab_assets_dir();

    info!("Lab server started at http://{}", addr);
    info!("Serving files from: {:?}", assets_dir);

    loop {
        let (socket, peer) = listener.accept().await?;
        let assets = assets_dir.clone();

        tokio::spawn(async move {
            if let Err(e) = handle_connection(socket, assets).await {
                debug!("Connection error from {}: {}", peer, e);
            }
        });
    }
}

async fn handle_connection(
    mut socket: tokio::net::TcpStream,
    assets_dir: PathBuf,
) -> anyhow::Result<()> {
    let (reader, mut writer) = socket.split();
    let mut buf_reader = BufReader::new(reader);
    let mut request_line = String::new();
    buf_reader.read_line(&mut request_line).await?;

    // Parse: GET /path HTTP/1.1
    let parts: Vec<&str> = request_line.split_whitespace().collect();
    if parts.len() < 2 {
        return Ok(());
    }

    let method = parts[0];
    let mut path = parts[1];

    // Only handle GET
    if method != "GET" {
        let response = "HTTP/1.1 405 Method Not Allowed\r\nContent-Length: 0\r\n\r\n";
        writer.write_all(response.as_bytes()).await?;
        return Ok(());
    }

    // Drain headers (we don't need them for static serving)
    loop {
        let mut line = String::new();
        buf_reader.read_line(&mut line).await?;
        if line == "\r\n" || line.is_empty() {
            break;
        }
    }

    // Normalize path
    if path == "/" {
        path = "/index.html";
    }

    // Security: prevent directory traversal
    let clean_path = path.trim_start_matches('/').replace("..", "");
    let file_path = assets_dir.join(&clean_path);

    // Check file exists and is within assets dir
    let canonical = match file_path.canonicalize() {
        Ok(p) => p,
        Err(_) => {
            send_404(&mut writer).await?;
            return Ok(());
        }
    };

    let assets_canonical = assets_dir.canonicalize().unwrap_or(assets_dir);
    if !canonical.starts_with(&assets_canonical) {
        send_404(&mut writer).await?;
        return Ok(());
    }

    // Read and send file
    match tokio::fs::read(&canonical).await {
        Ok(contents) => {
            let mime = mime_type(&clean_path);
            let response = format!(
                "HTTP/1.1 200 OK\r\n\
                 Content-Type: {}\r\n\
                 Content-Length: {}\r\n\
                 Access-Control-Allow-Origin: *\r\n\
                 Cache-Control: no-cache\r\n\
                 \r\n",
                mime,
                contents.len()
            );
            writer.write_all(response.as_bytes()).await?;
            writer.write_all(&contents).await?;
            debug!("Served: {} ({} bytes)", clean_path, contents.len());
        }
        Err(_) => {
            send_404(&mut writer).await?;
        }
    }

    Ok(())
}

async fn send_404(writer: &mut tokio::net::tcp::WriteHalf<'_>) -> anyhow::Result<()> {
    let body = "404 Not Found";
    let response = format!(
        "HTTP/1.1 404 Not Found\r\n\
         Content-Type: text/plain\r\n\
         Content-Length: {}\r\n\
         \r\n\
         {}",
        body.len(),
        body
    );
    writer.write_all(response.as_bytes()).await?;
    Ok(())
}

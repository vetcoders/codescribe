use std::net::SocketAddr;
use std::path::PathBuf;
use tokio::time::Duration;
use tracing::info;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter("info,vista_kernel=debug")
        .init();

    info!("Starting Level 1 STT Zero-Provider Zero-Lane server on port 9898...");
    let bind_addr: SocketAddr = "127.0.0.1:9898".parse().unwrap();

    // Uruchomienie serwera axum w tle (który ma podłączony Lexicon!)
    let _handle = vista_kernel::server::start(bind_addr).await?;
    info!("Server listening. Sending sample test audio...");

    // Czekamy chwilę na boot serwera
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Pobranie pliku testowego
    let home = std::env::var("HOME").unwrap_or_else(|_| "/Users/maciejgad".to_string());
    let audio_path = PathBuf::from(home).join(".codescribe/data_assets/01_no-to-dobra.wav");

    if !audio_path.exists() {
        info!("Audio file not found: {:?}", audio_path);
        return Ok(());
    }

    info!("Sending file: {:?}", audio_path);
    let client = reqwest::Client::new();
    let file = tokio::fs::read(&audio_path).await?;

    let part = reqwest::multipart::Part::bytes(file)
        .file_name("01_no-to-dobra.wav")
        .mime_str("audio/wav")?;

    let form = reqwest::multipart::Form::new().part("audio", part);

    // Wysłanie!
    let start_time = std::time::Instant::now();
    let req = client
        .post("http://127.0.0.1:9898/transcribe")
        .multipart(form)
        .header("x-language", "pl")
        .send()
        .await?;

    let elapsed = start_time.elapsed();
    let status = req.status();
    let body = req.text().await?;

    info!("--- VISTA STT ZERO-PROVIDER RESPONSE ---");
    info!("Status: {} ({}ms)", status, elapsed.as_millis());
    info!("Response: {}", body);
    info!("--- EOF ---");

    Ok(())
}

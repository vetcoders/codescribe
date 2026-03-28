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
    info!("Server listening. Sending canonical test audio files...");

    // Czekamy chwilę na boot serwera
    tokio::time::sleep(Duration::from_millis(500)).await;

    let home = std::env::var("HOME").unwrap_or_else(|_| "/Users/maciejgad".to_string());
    let base_path = PathBuf::from(home).join(".codescribe/data_assets");

    let files = [
        "01_no-to-dobra.wav",
        "02_kubernetes-wymaga-konfiguracji.wav",
        "03_algorytm-ma-zlozonosc.wav",
        "04_runda-3-czyli.wav",
    ];

    let client = reqwest::Client::new();

    for filename in &files {
        let audio_path = base_path.join(filename);

        if !audio_path.exists() {
            info!("Audio file not found: {:?}", audio_path);
            continue;
        }

        info!("Sending file: {:?}", audio_path);
        let file = tokio::fs::read(&audio_path).await?;

        // Ważne: zrobienie od nowa part i form dla każdego strzału
        let part = reqwest::multipart::Part::bytes(file)
            .file_name(*filename)
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

        info!("--- VISTA STT ZERO-PROVIDER RESPONSE FOR {} ---", filename);
        info!("Status: {} ({}ms)", status, elapsed.as_millis());
        info!("Response: {}", body);
        info!("--------------------------------------------------");
    }

    info!("All 4 files processed.");
    Ok(())
}

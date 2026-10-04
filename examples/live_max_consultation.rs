//! Explicit live Max acceptance through production provider and history owners.
//! Run: cargo run --example live_max_consultation -- --run
//! Makes two paid/provider requests using copied settings and normal credentials.
//! No executable tools, clipboard, audio, or OS delivery. Not a GUI acceptance test.

use codescribe_core::agent::{ThreadDeliveryGateway, ToolRegistry};
use codescribe_core::config::{Config, FormattingPolicy, UserSettings};
use std::sync::Arc;
use std::time::Duration;
use tempfile::TempDir;

fn main() {
    if std::env::args().skip(1).collect::<Vec<_>>() != ["--run"] {
        eprintln!("Explicit live acceptance: cargo run --example live_max_consultation -- --run");
        std::process::exit(2);
    }
    let source_path = UserSettings::settings_path();
    let source_bytes = std::fs::read(&source_path).expect("existing configured settings required");
    let data_dir = TempDir::new().expect("isolated live acceptance data");
    std::fs::write(data_dir.path().join("settings.json"), &source_bytes)
        .expect("copy settings into isolated directory");
    // The production loader may repair its input; it only sees the copy.
    // Optional .env and prompt files from the live data directory are not copied.
    // SAFETY: single-threaded entrypoint; no runtime or worker exists yet.
    unsafe {
        std::env::set_var("CODESCRIBE_DATA_DIR", data_dir.path());
        std::env::set_var("CODESCRIBE_ENV_PATH", data_dir.path().join(".env"));
    }

    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("acceptance runtime")
        .block_on(async {
            let settings = Config::load_runtime_snapshot().expect("resolve live provider settings");
            assert_eq!(settings.formatting_policy(), FormattingPolicy::Max);
            let lane = settings.llm_lanes().formatting();
            assert!(
                lane.request_available(),
                "configured formatting lane unavailable"
            );
            eprintln!(
                "live Max provider={} model={}",
                lane.provider().as_str(),
                lane.model()
            );

            let consultation = codescribe::agent::max_consultation::MaxConsultation::start(
                format!("live-max-{}", uuid::Uuid::new_v4()),
                &settings,
                Arc::new(ToolRegistry::new()),
                None,
                ThreadDeliveryGateway::new_in(data_dir.path().join("threads"))
                    .expect("isolated history"),
                Arc::new(|_, _, _| {}),
                data_dir.path().join("agent-turn.lock"),
            )
            .expect("start live Max owner");
            let marker = format!("ROMAN{}", uuid::Uuid::new_v4().simple());
            let first = consultation
                .enqueue(
                    "first".into(),
                    format!("Zapamiętaj kod tej rozmowy: {marker}. Odpowiedz wyłącznie OK."),
                    Vec::new(),
                    &settings,
                )
                .expect("admit first live turn");
            let first = tokio::time::timeout(Duration::from_secs(90), first)
                .await
                .expect("first live turn timeout")
                .expect("first owner response")
                .expect("first provider answer");
            assert_eq!(
                first.text.trim(),
                "OK",
                "Max must obey the requested output form"
            );
            assert_eq!(first.delivery.message_count, 2);

            let second = consultation
                .enqueue(
                    "second".into(),
                    "Podaj wyłącznie kod z poprzedniej wiadomości. Bez żadnych dodatkowych słów."
                        .into(),
                    Vec::new(),
                    &settings,
                )
                .expect("admit second live turn");
            let second = tokio::time::timeout(Duration::from_secs(90), second)
                .await
                .expect("second live turn timeout")
                .expect("second owner response")
                .expect("second provider answer");
            assert_eq!(
                second.text.trim(),
                marker,
                "second request must retain prior context"
            );
            assert_eq!(second.delivery.message_count, 4);
            assert_eq!(first.delivery.backend_id, second.delivery.backend_id);
            assert_eq!(second.delivery.backend_id, consultation.id());
            consultation
                .close_if_idle()
                .await
                .expect("close settled live consultation");
            assert!(
                std::fs::read(&source_path).expect("live settings remain readable") == source_bytes,
                "live settings changed during acceptance; do not overwrite concurrent edits"
            );
            eprintln!(
                "live Max two-turn context and four-message settlement verified; no OS delivery"
            );
        });
}

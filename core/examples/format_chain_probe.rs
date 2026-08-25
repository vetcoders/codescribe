//! One-shot probe: push a raw transcript through the PRODUCTION formatting
//! path twice in one process, so the second call runs as a CHAINED turn
//! (previous_response_id present) — the exact condition of the 2026-08-14
//! promptless-chain leak. Prints both outputs verbatim for a 1:1 exhibit.
//!
//! Usage:
//!   cargo run -p codescribe-core --example format_chain_probe -- <raw.txt>

#[tokio::main]
async fn main() {
    let path = std::env::args()
        .nth(1)
        .expect("usage: format_chain_probe <raw.txt>");
    let raw = std::fs::read_to_string(&path).expect("read raw transcript");
    let runtime_settings = codescribe_core::config::Config::load_runtime_snapshot()
        .expect("seal runtime settings");
    println!("=== RAW ({} chars) from {path}", raw.chars().count());

    for turn in 1..=2 {
        let out = codescribe_core::llm::ai_formatting::format_text(
            &raw,
            Some("pl"),
            false,
            runtime_settings.formatting_policy(),
            runtime_settings.llm_lanes().formatting(),
        )
        .await;
        println!("\n=== TURN {turn} ({} chars) ===", out.chars().count());
        println!("{out}");
    }
}

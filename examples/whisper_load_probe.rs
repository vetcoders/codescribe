//! One cold Whisper load with the engine's phase breakdown.
//!
//! "Preloaded, zero latency" is the product claim; the operator's logs show 56
//! cold loads at a median of 9.2 s because the idle reaper drops the weights
//! every 45 minutes. This binary answers the only question that decides the
//! fix: which phase actually costs the seconds.

/// Initialise the singleton once and report the wall time the user would feel.
///
/// The engine's own phase logging supplies the breakdown; this prints the
/// total it adds up to, so the parts can be checked against the whole.
fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "codescribe_core=info".into()),
        )
        .init();
    let started = std::time::Instant::now();
    codescribe_core::stt::whisper::singleton::init()?;
    println!(
        "cold init felt by the user: {:.2}s",
        started.elapsed().as_secs_f64()
    );
    Ok(())
}

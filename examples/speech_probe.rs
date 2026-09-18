//! Content-free live speech proof. Does not play audio or change provider settings.
use codescribe_core::llm::{
    provider::ProviderKind,
    speech::{self, SpeechOptions},
};
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Same settings/env bootstrap as the application, without printing credentials.
    let _settings = codescribe_core::config::Config::load_runtime_snapshot()?;
    let args: Vec<String> = std::env::args().collect();
    let value = |flag: &str| {
        args.windows(2)
            .find(|a| a[0] == flag)
            .map(|a| a[1].as_str())
    };
    let name = value("--vendor").unwrap_or("openai");
    let vendor = match name {
        "openai" => ProviderKind::OpenAiResponses,
        "xai" => ProviderKind::XaiResponses,
        _ => anyhow::bail!("--vendor openai|xai"),
    };
    let signed_in = speech::vendor_signed_in(vendor);
    let auth = match speech::resolve_vendor_auth(vendor, None).await {
        Ok(auth) => auth,
        Err(error) => {
            println!(
                "vendor={name} auth_source={} status=none pcm_bytes=0 duration_ms=0 error={error}",
                if signed_in { "oauth" } else { "none" }
            );
            return Ok(());
        }
    };
    let options = SpeechOptions::for_vendor(vendor)?;
    match speech::synthesize_with(
        value("--text").unwrap_or("Hello from Codescribe."),
        &options,
        &auth,
    )
    .await
    {
        Ok(audio) => println!(
            "vendor={name} auth_source={} status={} pcm_bytes={} duration_ms={} cached={}",
            auth.source.as_str(),
            audio
                .http_status
                .map_or_else(|| "cache".into(), |s| s.to_string()),
            audio.samples.len() * 2,
            audio.duration_ms(),
            audio.cached
        ),
        Err(error) => println!(
            "vendor={name} auth_source={} status={} pcm_bytes=0 duration_ms=0 error={error}",
            auth.source.as_str(),
            match error {
                speech::SpeechError::Http(s) => s.to_string(),
                _ => "none".into(),
            }
        ),
    }
    Ok(())
}

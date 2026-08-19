use std::net::SocketAddr;
use std::path::PathBuf;

use anyhow::{Context, Result, anyhow, bail};
use codescribe_core::stt::tail_provider::{
    FakeTailProvider, InProcessTailProvider, STT_SIDECAR_TOKEN_ENV, TailProvider,
    TailProviderPayload, serve_sidecar,
};

struct Args {
    bind: SocketAddr,
    parent_pid: Option<u32>,
    #[cfg(debug_assertions)]
    fake_payload: Option<PathBuf>,
}

fn main() -> Result<()> {
    let args = parse_args()?;
    let token = std::env::var(STT_SIDECAR_TOKEN_ENV).context("sidecar process token is missing")?;
    // SAFETY: main is still single-threaded and no library worker has started.
    unsafe { std::env::remove_var(STT_SIDECAR_TOKEN_ENV) };

    #[cfg(debug_assertions)]
    let provider: Box<dyn TailProvider> = if let Some(path) = args.fake_payload {
        let payload: TailProviderPayload = serde_json::from_slice(
            &std::fs::read(&path).with_context(|| format!("read fixture {}", path.display()))?,
        )
        .context("parse sidecar fixture payload")?;
        Box::new(FakeTailProvider::new(payload)?)
    } else {
        Box::new(InProcessTailProvider)
    };
    #[cfg(not(debug_assertions))]
    let provider: Box<dyn TailProvider> = Box::new(InProcessTailProvider);

    serve_sidecar(args.bind, token, provider.as_ref(), args.parent_pid)
}

fn parse_args() -> Result<Args> {
    let mut bind: Option<SocketAddr> = None;
    let mut parent_pid = None;
    #[cfg(debug_assertions)]
    let mut fake_payload = None;
    let mut args = std::env::args_os().skip(1);
    while let Some(arg) = args.next() {
        match arg.to_str() {
            Some("--bind") => {
                let value = args.next().ok_or_else(|| anyhow!("--bind needs a value"))?;
                bind = Some(
                    value
                        .to_string_lossy()
                        .parse()
                        .context("invalid --bind socket address")?,
                );
            }
            Some("--parent-pid") => {
                let value = args
                    .next()
                    .ok_or_else(|| anyhow!("--parent-pid needs a value"))?;
                parent_pid = Some(
                    value
                        .to_string_lossy()
                        .parse()
                        .context("invalid --parent-pid")?,
                );
            }
            #[cfg(debug_assertions)]
            Some("--fake-payload") => {
                fake_payload = Some(PathBuf::from(
                    args.next()
                        .ok_or_else(|| anyhow!("--fake-payload needs a value"))?,
                ));
            }
            _ => bail!("unknown sidecar argument"),
        }
    }
    let bind = bind.ok_or_else(|| anyhow!("--bind is required"))?;
    if !bind.ip().is_loopback() {
        bail!("sidecar bind must be loopback");
    }
    Ok(Args {
        bind,
        parent_pid,
        #[cfg(debug_assertions)]
        fake_payload,
    })
}

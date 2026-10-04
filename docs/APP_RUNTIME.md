# Codescribe application runtime

Codescribe.app owns one process-wide, multi-thread Tokio runtime. The shipped
policy starts four workers named `codescribe-app-worker-1` through
`codescribe-app-worker-4`. `CODESCRIBE_APP_RUNTIME_WORKERS` may change that
count to `1..16`, is registered in `ENV_REGISTRY.toml`, and takes effect only
after restart.

`AppDelegate` starts the runtime after configuration has been loaded and before
constructing async bridge surfaces. Every UniFFI async export immediately moves
its root future to this runtime; the foreign Swift executor only waits for the
join result. The bridge deliberately does not use UniFFI's
`async_runtime = "tokio"` compatibility fallback as application execution
authority.

On termination the host first stops gesture intake, cancels pending account
login, drains an active controller recording, releases capture ownership, and
then shuts down the runtime with a bounded timeout. Shutdown is terminal for
the process: the same owner cannot be restarted.

The Whisper engine remains the process-wide `Mutex<WhisperSlot>` singleton.
The runtime adds scheduling capacity; it does not allow concurrent Whisper
decode or create a second transcript reducer. Recording callbacks still hand
off through channels and never wait for inference.

Runtime evidence is available through `applicationRuntimeSnapshot()`: lifecycle
state, configured worker count, observed worker names, stopped worker names,
and active root bridge tasks. It contains no audio, transcript, model path, or
operator content.

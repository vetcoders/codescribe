# Streaming client (Node) – minimal

Purpose: headless client that captures mic via ffmpeg (avfoundation) and sends audio chunks to the backend `/stream/transcribe` using NDJSON. Prints transcripts to stdout and can optionally copy/paste them.

## Requirements
- macOS with microphone permissions
- `ffmpeg` (brew install ffmpeg)
- Node.js >= 18

## Run
```
scripts/VSStream --lang pl --chunkMs 800 --pasteFinal 1 --pasteLive 0
```

Options:
- `--server` (default `http://127.0.0.1:8237`)
- `--lang` (default `pl`)
- `--sr` sample rate (default `16000`)
- `--chunkMs` batch window (default `800`)
- `--pasteFinal 0|1` copy to clipboard and optionally paste on each final
- `--pasteLive 0|1` simulate Cmd+V after copying (requires Accessibility perms)

Notes:
- The current HTTP NDJSON endpoint accumulates events and returns them at the end of the session; you receive the list (acks + final transcripts) after pressing Ctrl+C (which sends `end`). For true live updates, switch to the WebSocket endpoint `/ws/transcribe` and send `flush` messages – see `src/vistascribe/backend.py`.
- This client is intentionally minimal: it avoids global state, hotkeys, or tray UI.

## Next
- WS mode for immediate `transcript.final` delivery.
- Diff-based live paste under caret.
- Optional encoding (m4a/flac) before upload (currently raw PCM → base64 in NDJSON).


# Creator & Settings

CodeScribe ships one native macOS Creator window with six tabs:

1. **Creator**
2. **Keys**
3. **Audio**
4. **Voice Lab**
5. **Engine**
6. **User**

Configuration still lives in three layers:

1. **GUI settings**: `~/Library/Application Support/CodeScribe/settings.json`
2. **Secrets**: macOS Keychain (`com.vetcoders.codescribe`)
3. **Power-user overrides**: `~/.codescribe/.env`

Most users should stay inside the Creator window. The `.env` file is for overrides and automation-heavy workflows.

## Open Creator

- Menu bar icon → **Creator Studio...**
- Dock icon click
- Chat Overlay → gear button (opens Creator)

## Creator

Open **Creator**.

This is the graphical launchpad:

- permission checklist
- first-run quick start
- one-click launch pads into Keys, Audio, Voice Lab, and Agent overlay

## Keys

Open **Keys**.

This tab owns:

- hotkey presets and per-mode bindings
- hold / toggle timing
- formatting provider endpoint, model, and key
- assistive provider endpoint, model, and key

## Audio

Open **Audio**.

This tab owns:

- `Whisper language`
- `Beep on recording start`
- `Enter to send`
- `Show Dock icon`
- `Sound volume`

## Voice Lab

Open **Voice Lab**.

This tab owns live pipeline tuning:

- `CODESCRIBE_BUFFER_DELAY_MS`
- `CODESCRIBE_TYPING_CPS`
- `CODESCRIBE_EMIT_WORDS_MAX`
- `CODESCRIBE_BUFFERED_INTERIM_SEC`
- cloud multipart model / upload caps

Current runtime truth:

- local Whisper remains the live preview path
- cloud STT is still post-capture, not live cloud preview
- the UI exposes only the knobs that materially improve UX

## Engine

Open **Engine**.

This is the read-only runtime truth panel:

- active STT engine
- Whisper / VAD / TTS / embedder availability
- model embedding state

## User

Open **User**.

This tab owns slower-moving toggles:

- dock icon visibility
- quality daemon
- ultra quality final pass

## Power-user Overrides

If you need direct overrides outside the Creator:

```bash
codescribe --config
```

That opens or creates `~/.codescribe/.env`.

Common overrides:

- `USE_LOCAL_STT`
- `STT_ENDPOINT`
- `AI_FORMATTING_ENABLED`
- `CODESCRIBE_BUFFER_DELAY_MS`
- `CODESCRIBE_TYPING_CPS`
- `CODESCRIBE_EMIT_WORDS_MAX`
- `CODESCRIBE_BUFFERED_INTERIM_SEC`

## Reset / Fresh Start

- **New agent context**: Chat Overlay → **New thread**
- **Reset prompts**: menu bar icon → **Edit prompts…**

_Created by M&K (c)2026 VetCoders_

# Settings & Configuration

CodeScribe now has one native Settings window with five tabs:

1. **Transcription**
2. **Modes & Shortcuts**
3. **AI & Prompts**
4. **Audio & Input**
5. **Diagnostics**

Configuration still lives in three layers:

1. **GUI settings**: `~/Library/Application Support/CodeScribe/settings.json`
2. **Secrets**: macOS Keychain (`com.vetcoders.codescribe`)
3. **Power-user overrides**: `~/.codescribe/.env`

Most users should stay inside the Settings window. The `.env` file is for overrides and automation-heavy workflows.

For the product semantics behind preview, verdict, fallback, and AI categories, see [Truth Contract](../truth-contract.md).

## Open Settings

- Menu bar icon → **Settings**
- Chat Overlay → **Settings** tab

## Transcription

Open **Settings → Transcription**.

This tab owns the transcript pipeline itself:

- **Final Transcript Path**
  - `Local transcript`
  - `Cloud final transcript`
  - optional cloud endpoint + API key
- **Preview Timing**
  - `Buffer delay`
  - `Typing speed`
  - `Words per tick`
  - `Interim cadence`
  - live preview panel showing:
    - when partial targets are published
    - how those targets would become visible on the overlay
- **Final Transcript**
  - `Local file-based final pass`
  - `AI Formatting`
  - `Formatting level`
- **Quality Automation**
  - app-launch quality daemon toggle
  - latest report / availability / pending mismatch state

### Current runtime truth

- When **Transcription overlay** is ON, the app is optimized for low-latency live preview.
- When **Transcription overlay** is OFF, the floating preview is hidden and runtime uses a more buffered cadence to reduce local load.
- `USE_LOCAL_STT=0` changes the **committed transcript path after capture**; it does not move live preview to the cloud.
- In the current build, **cloud STT is still post-capture**, not live cloud preview. The Settings UI states this explicitly.

## Modes & Shortcuts

Open **Settings → Modes & Shortcuts**.

This tab owns the global shortcut model:

- **Dictation**
- **Formatting**
- **Assistive**

Each mode gets one binding. You can customize or disable it.

The same tab also owns:

- `Hold delay`
- `Double-tap interval`
- hotkey conflict detection / details

## AI & Prompts

Open **Settings → AI & Prompts**.

This tab owns the LLM side of the product:

- Formatting provider
- Assistive provider
- model + endpoint fields
- API keys in Keychain
- prompt editor for:
  - `formatting`
  - `assistive`

Prompt files live in `~/.codescribe/prompts/`.

## Audio & Input

Open **Settings → Audio & Input**.

This tab owns capture defaults and app-shell behavior:

- `Whisper language`
- `Beep on recording start`
- `Enter to send`
- `Transcription overlay`
- `Show Dock icon`
- `Sound volume`

This is where you decide whether the floating transcription overlay exists at all.

## Diagnostics

Open **Settings → Diagnostics**.

This tab is for environment truth, not onboarding copy:

- live permission matrix
- hotkey conflict summary
- `Refresh matrix`
- `Open System Settings`
- `Copy diagnostics`

Use this tab when the app lies about permissions, focus, shortcuts, or runtime availability.

## Power-user `.env` Overrides

If you need direct overrides outside the GUI:

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
- **Reset prompts**: Settings → **AI & Prompts** → **Reset**

_Created by M&K (c)2026 VetCoders_

# Settings & Configuration

CodeScribe has three configuration layers:

1. **GUI settings** (JSON): `~/Library/Application Support/CodeScribe/settings.json`
2. **Secrets** (API keys): macOS Keychain (`com.vetcoders.codescribe`)
3. **Power‑user overrides** (optional): `~/.codescribe/.env`

Most users should use the **Settings** window. The `.env` file is only for overrides and advanced workflows.

---

## Open Settings

- Menu bar icon → **Settings**
- Chat Overlay → **Settings** tab (routes to the Settings window / onboarding)

---

## Hotkeys (Modes & Shortcuts)

Open **Settings → Modes & Shortcuts**.

CodeScribe uses a **mode‑first** shortcut model: each mode has one binding you can customize (or disable).

- **Dictation** (auto‑paste ON)
  - Default: Hold `Fn/Globe`
  - Can be set to Hold `Ctrl` variants or Double‑tap `Ctrl`
- **Formatting** (auto‑paste ON, AI required)
  - Default: Double‑tap `Left Option`
- **Assistive (Agent)** (auto‑paste OFF, AI required)
  - Default: Double‑tap `Right Option`

If macOS already uses a shortcut, the conflict detector will flag it and you can change the binding.

---

## AI Providers & Prompts (AI & Prompts)

Open **Settings → AI & Prompts**.

- **Formatting AI** powers Formatting mode (and Dictation when AI Formatting is enabled)
- **Assistive AI** powers the Agent overlay in Assistive mode
- API keys are stored in **Keychain**

### Prompt Files

Prompt files live in `~/.codescribe/prompts/`:

| File                                   | Used For |
| -------------------------------------- | -------- |
| `~/.codescribe/prompts/formatting.txt` | Formatting behavior |
| `~/.codescribe/prompts/assistive.txt`  | Agent behavior (user instruction + selected text rules) |

You can edit prompts in-app (prompt editor) or edit the files directly.

---

## Audio & Input

Open **Settings → Audio & Input**.

Key options:

- **Whisper language** (explicit; no auto‑detect)
- **AI Formatting** toggle (affects Dictation mode)
- **Enter to send** (overlay typing behavior)
- Beep/volume controls

---

## Advanced

Open **Settings → Advanced**.

- Hold start delay (ms)
- Double‑tap interval (ms)
- Voice Lab overrides for pipeline experiments

---

## Power‑User `.env` Overrides (Optional)

If you need overrides outside the GUI, use:

```bash
codescribe --config
```

This opens/creates `~/.codescribe/.env`.

Common overrides:

- `WHISPER_LANGUAGE=pl`
- `AI_FORMATTING_ENABLED=1`
- `LLM_FORMATTING_ENDPOINT=...` / `LLM_FORMATTING_MODEL=...`
- `LLM_ASSISTIVE_ENDPOINT=...` / `LLM_ASSISTIVE_MODEL=...`

---

## Reset / “Start Fresh”

- **New agent context**: Chat Overlay → **New thread**
- **Reset prompts**: Settings → AI & Prompts → **Reset** (per prompt type)

---

_Created by M&K (c)2026 VetCoders_

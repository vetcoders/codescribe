# Speech Recognition TCC (Apple live)

## Why this exists

Apple live dictation uses `SFSpeechRecognizer` inside the bundled
`codescribe-stt-bridge` helper (`Contents/MacOS/codescribe-stt-bridge`).

Speech Recognition is a **per-app TCC identity** (`com.vetcoders.codescribe`).

| Context              | Who gets the grant               |
| -------------------- | -------------------------------- |
| CLI / terminal tests | Terminal app (Ghostty, iTerm, …) |
| Codescribe.app       | `com.vetcoders.codescribe`       |

Granting Speech for the terminal does **not** authorize the app. That is why
CLI can probe `speech_auth: authorized` while the installed app fails live
with `speech_auth_not_determined` / hard fail when Candle fallback is disabled
for live.

## Backend scope (W4-B)

Speech Recognition TCC is required **only for the SFSpeechRecognizer path**.

| Backend                                   | Speech Recognition TCC            | Mic (live capture) |
| ----------------------------------------- | --------------------------------- | ------------------ |
| `sf_speech_recognizer`                    | Required                          | Independent        |
| `speech_transcriber` (ST)                 | **Not** a prerequisite            | Independent        |
| `dictation_transcriber` (DT, opt-in W4-A) | **Not** a prerequisite — measured | Independent        |

The DT row stopped being an inference on 2026-08-08: the lane transcribed the
full 140.85 s pl-PL parity fixture (1072 chars) from a terminal-responsible
process whose `SFSpeechRecognizer.authorizationStatus()` read `notDetermined`.
An engine that ran to completion under a _withheld_ Speech grant cannot be
gated on it.

Bridge entry points (`transcribe` / `stream` / `transcribe_live`) no longer call
`ensureSpeechAuthorizedForSfSpeech` before backend selection. Auth is enforced
inside SF-only helpers. Rust `init` hard-fails on non-authorized speech **only**
when the selected probe backend needs SFSpeech.

**Responsible process (D1):** headless bridge under Terminal uses the terminal's
TCC slot unless respawned with `CODESCRIBE_BRIDGE_DISCLAIM` / app identity.
Product onboarding must grant Speech for **Codescribe.app**, not the shell.

## Product surfaces (grant path)

1. **First-run wizard** — dedicated **Speech Recognition Access** step
   (after Screen Recording, before Full Disk). Primary CTA:
   - `notDetermined` → **Allow Speech Recognition** (in-app dialog)
   - determined / denied → **Open System Settings** deep-link
   - **Refresh status** re-probes live TCC
2. **Settings › Dictation** — permission matrix cell (same request / deep-link rules)
3. **Settings › Creator** — checklist row
4. **App launch** — if still undetermined, request once (briefly activate app so
   accessory / LSUIElement policy does not swallow the dialog)
5. **Recording start** — one-shot request+retry on `speech_auth_not_determined`
6. **Overlay** — raw `speech_auth_*` markers rewritten to actionable copy

## Required setup

Speech Recognition is a **required** setup permission in
`app/os/onboarding.rs` (`REQUIRED_SETUP_PERMISSIONS`). Missing grant can
invalidate `setup_done` so the wizard re-opens.

## Operator truth (do not silent-grant)

- Do **not** `tccutil reset` / silent TCC write as a “fix”.
- User must Allow in the system dialog or toggle
  **System Settings › Privacy & Security › Speech Recognition**.
- About panel commit must match the clean HEAD used for the build (see
  `scripts/build-app.sh` stamp rules — no false `-dirty` from UniFFI or
  untracked DMG checksums).

## Related engine doctrine

- Apple = live-only path (virtual mic / AudioBuffer).
- Whisper = file final-pass / gap fill; never full-replace live (merge fill).
- Gaps in Apple live are fill canvas for Teacher / Whisper, not pure WER failure.

_Vibecrafted. with AI Agents by Vetcoders (c)2024-2026 The LibraxisAI Team_

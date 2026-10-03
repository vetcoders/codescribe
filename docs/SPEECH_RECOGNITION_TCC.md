# Speech Recognition TCC (Apple live)

## Why this exists

Apple live dictation uses `SFSpeechRecognizer` inside the bundled
`codescribe-stt-bridge` helper (`Contents/MacOS/codescribe-stt-bridge`).

Speech Recognition authorization belongs to the **responsible process's TCC
identity**, not to the data directory or the shell command's executable path.

| Context | TCC identity |
| ------- | ------------ |
| Release app bundle via LaunchServices | `com.vetcoders.codescribe` |
| Debug app bundle via LaunchServices | `com.vetcoders.codescribe.dev` |
| CLI / terminal probes | Normally the terminal/host; not proof of an app grant |
| Direct `Codescribe.app/Contents/MacOS/Codescribe` execution | Unsupported native app startup; may retain the terminal/agent host's responsibility |

Granting Speech for the terminal does **not** authorize the app. That is why
CLI can probe `speech_auth: authorized` while the installed app fails live
with `speech_auth_not_determined` / hard fail when Candle fallback is disabled
for live.

## Supported native launch context

Launch the **app bundle through LaunchServices**: Finder, Spotlight, or
`/usr/bin/open` with the intended `.app` path. `make start` also uses `open`,
but resolves the app by name first; use an explicit bundle path when selecting
a particular dev/test build. See [Installation: supported native launch
context](./INSTALLATION.md#supported-native-launch-context) for commands and
per-launch environment variables.

Do **not** execute `Contents/MacOS/Codescribe` directly from a shell, agent
host, debugger wrapper or subprocess for native speech acceptance. This is
unsupported even when the executable remains inside a bundle whose
`Info.plist` contains `NSSpeechRecognitionUsageDescription`. TCC can attribute
the request to the launching host and abort for a missing usage description
there. `CODESCRIBE_DATA_DIR` selects application data; it does not create a
different TCC identity. Do not grant Speech to the host, edit its usage
description, reset TCC or disable authorization to make this launch pass.

[Issue #93](https://github.com/vetcoders/codescribe/issues/93) records this
failure on Release 0.15.2 (1652), commit `862bd0a63`, on 2026-10-02: direct
execution aborted in TCC with the agent host as responsible process; the same
build launched through LaunchServices reached Listening. That observation
predates localization and is not a retest of newer builds.

The current source already supplies the usage description in
`macos/project.yml` and requests Speech from the main app through
`AppDelegate.ensureSpeechRecognitionAtLaunch()` and
`SpeechRecognitionPermission.request()`. Keep this permission path and the
backend-specific authorization rules below. Terminal bridge probes and the
XCTest host (which skips application runtime startup) do not verify native
speech startup. Acceptance on a new build must record the actual bundle
path, identifier, version/build, commit and PID, then confirm the app reaches
Speech authorization and Listening without a TCC abort. An `open` exit code
alone is not acceptance evidence.

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

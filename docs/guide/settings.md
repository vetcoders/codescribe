# Settings & Configuration

Codescribe has one native Settings window. Its sidebar separates **Creator**,
**Hotkeys**, **Audio**, **Providers**, **Agent**, **Dictation**, **Dictionary**,
**License**, and **User**. Each section with several concerns has its own tabs;
the experimental **Lab** section is visible only on the developer surface.

Configuration still lives in three layers:

1. **GUI settings**: `~/Library/Application Support/Codescribe/settings.json`
2. **Secrets**: macOS Keychain (`com.vetcoders.codescribe`)
3. **Power-user overrides**: `~/.codescribe/.env`

Most users should stay inside the Settings window. The `.env` file is for overrides and automation-heavy workflows.

For the product semantics behind preview, verdict, fallback, and AI categories, see [Truth Contract](../truth-contract.md).

## Open Settings

- Menu bar icon → **Settings**
- Agent window → **Settings**

## Dictation

Open **Settings → Dictation** for the speech engine, Whisper model, preview
timing, hands-free capture, cloud privacy and permissions. **Creator** carries
the setup controls, including language and automatic formatting.

This tab owns the transcript pipeline itself:

- **Final Transcript Path**
  - `Local transcript`
  - `Cloud final transcript`
  - optional cloud endpoint + API key
- **Preview Timing**
  - `Buffer delay`
  - `Typing speed`
  - `Words per tick`
  - `Interim interval`
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
- Turning it OFF — from the tray toggle or the Settings preview preset — also closes an overlay that is already on screen; it does not wait for the next take. Two things stay: an open agent channel, whose live microphone stays visible, and a take you are correcting — while the caret is in the transcript or a draft is not committed, the overlay waits, then leaves after the usual five seconds once the draft is committed or discarded.
- A blocked recording or a microphone calibration result still shows its status card with the overlay OFF. That card leaves by itself after the usual five seconds, even when **Keep visible between takes** is pinned: the pin keeps the transcript overlay, and with the overlay OFF there is none.
- `USE_LOCAL_STT=0` changes the **committed transcript path after capture**; it does not move live preview to the cloud.
- In the current build, **cloud STT is still post-capture**, not live cloud preview. The Settings UI states this explicitly.

## Hotkeys

Open **Settings → Hotkeys**.

This tab owns the global shortcut model:

- **Dictation**
- **Formatting**
- **Assistive**

Each mode gets one binding. You can customize or disable it.

The same tab also owns:

- `Hold delay`
- `Double-tap interval`
- hotkey conflict detection / details

## Providers and Agent

Open **Settings → Providers** to manage accounts, API keys and provider
endpoints. Vendor endpoints are factory-defined; custom hosts have editable
endpoints. Secrets are stored separately from account sign-in.

Open **Settings → Agent → LLM lanes** to select a provider and model separately
for **Assistive** (Agent and voice-assistant requests) and **Formatting**
(transcript cleanup). **Agent → Prompts** edits their prompts.

### Credential access while refreshing

Settings and Setup read provider credentials in the background. The initial
read shows **Checking provider access…** rather than claiming an account or
key is missing. An access error remains visible with **Retry provider access**;
a previous successful snapshot is labeled as the last checked state. Settings
also offers **Refresh provider access**. Returning focus refreshes only the
owning Settings or Setup window, and repeated requests share the pending read.
Permission checklist changes refresh permissions and hotkeys separately.

Saving or removing credentials and custom providers shows pending work. A
successful storage operation precedes publication of the new credential state;
an error does not become a false “not configured” result. Setup waits for a
pasted key to save before Continue leaves that step. Its provider picker stays
locked during the save. You can edit the key draft while waiting; if the draft
changes, the earlier save does not clear the new text or advance to the next
step. Save or Continue again to submit the current draft. Basic dictation remains
available while credential access is unresolved.

If one stored account cannot be decoded, its card shows **Account access
unavailable** with **Sign out** to remove that account before signing in again.
Other provider, API-key and STT controls remain available. This account state is
not a confirmed “not connected” result.

### Account access and API keys

Setup, its completion summary and the provider cards distinguish these states:

| Configuration     | Account       | API key        | Enabled access                                                                           |
| ----------------- | ------------- | -------------- | ---------------------------------------------------------------------------------------- |
| Account only      | Connected     | Not configured | Supported Assistive requests; no provider model discovery or cloud Formatting credential |
| API key only      | Not connected | Configured     | Supported API requests, including Formatting and model discovery                         |
| Both              | Connected     | Configured     | Account access for supported Assistive requests plus the provider API-key paths          |
| Key-optional host | Not required  | Optional       | Requests supported by that host, after selecting a model                                 |

A connected ChatGPT account is not an OpenAI API key. It does not authorize the
Formatting lane. Setup can continue with account-only access; use **Manage
provider access…** to open Providers when another credential is needed. A lane
is usable only when its resolved runtime snapshot reports it available.

### Model discovery

Model discovery queries the provider's model API with its provider API key.
Account sign-in alone does not supply that key. With account-only Assistive
access, `/model` shows the currently resolved model and explains the missing
catalog access. Keep that model, or enter a supported **Model ID** in **Agent →
LLM lanes**. Adding an API key is optional for Assistive account requests.

Fresh and cached catalogs offer selectable models. A provider returning no
models or a discovery failure gives the corresponding explanation and next
action. Check **Providers**, then **Refresh** models in **Agent → LLM lanes**.
The palette briefly caches results while filtering and refreshes them after a
provider/model/credential-presence change or after its cache expires.

Palette labels and grant actions follow the macOS interface language through
the app's String Catalog. Model IDs, provider IDs and tool grant keys remain
unchanged.

### Readiness scope

The Settings footer describes setup for speech capture, Assistive and enabled
automatic cloud Formatting. An unavailable required Formatting lane prevents a
green ready claim and links to Agent settings. When formatting is disabled, its
cloud lane is not required. Selecting Apple on-device formatting does not
authorize cloud requests: the user-triggered cloud path still checks this lane,
so enabled Formatting remains part of setup readiness. The footer does not
certify on-device execution.

Agent capabilities readiness in Setup covers Assistive access and native tools.
MCP status is separate and optional. The wizard presents account/key presence
separately from that capability verdict; it does not label an account as a key.

Prompt files live in `~/.codescribe/prompts/`.

## Audio

Open **Settings → Audio**.

This section owns microphone selection, calibration, the seal-lane prerequisite
and recording sound feedback. Its recording row observes the existing shared
recording lifecycle used by the overlay and tray:

- Idle: **Start recording** is enabled only with microphone permission and a
  successful admission verdict.
- Starting: shows preparation and prevents another Start.
- Recording: shows **Recording in progress** and **Stop recording**, including
  takes started outside this panel.
- Finishing: shows processing and prevents another Start or Stop.

Stopping through the tray or a shortcut updates this panel too. Calibration is
disabled while a take is active or finishing. These controls use the existing
RecordingController path; opening Audio does not create another recorder.

## Permissions and diagnostics

Open **Settings → Dictation → Permissions** for the permission matrix, and
**Agent → Status** for agent capability diagnostics.

This tab is for environment truth, not onboarding copy:

- live permission matrix
- hotkey conflict summary
- `Refresh matrix`
- `Open System Settings`
- `Copy diagnostics`

Use these surfaces to investigate permissions and runtime availability.

## Power-user `.env` Overrides

If you need direct overrides outside the GUI:

```bash
make config
```

That opens or creates `~/.codescribe/.env`.

When migrating an installation that only has this file, the first Settings
write preserves its promoted choices and records pending credential imports
before saving your edit. Credential access completes those imports later;
opening Settings alone does not acquire credentials or prepare the import.

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
- **Reset prompts**: Settings → **Agent → Prompts** → **Reset**

_Created by Vetcoders (c)2026_

## License access

Open **Settings → License** to activate, restore or remove a CSK1 license.
License storage runs in the background, and **Checking license…** or an access
error remains visible while Settings stays interactive. **Retry license access**
starts another read when the current operation has returned. Activation and
removal stay pending until storage succeeds; a failed replacement or removal
preserves the previously verified license.

An unreadable store does not grant Agentic access on a cold start. If a signed
license was already verified in this process, an access error preserves that
payload while its original validity and offline-grace bounds continue to be
evaluated against the current clock. Refreshing storage does not extend those
bounds. Basic dictation remains free.

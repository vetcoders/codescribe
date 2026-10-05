# Settings & Configuration

Codescribe now has one native Settings window with five tabs:

1. **Transcription**
2. **Modes & Shortcuts**
3. **AI & Prompts**
4. **Audio & Input**
5. **Diagnostics**

Configuration still lives in three layers:

1. **GUI settings**: `~/Library/Application Support/Codescribe/settings.json`
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
available while credential access is unresolved. If provider access is pending
or has failed and the current provider cannot be used for saving, Continue
advances without submitting or clearing a pasted draft. The draft stays in this
Setup session so you can go Back and save it once access is available. The UI
identifies it as unsaved; this is not a saved-credential claim. With a resolved
provider, Continue still waits for the successful save described above.

During a provider credential mutation, Settings also disables its STT section.
Endpoint edits cannot enter the synchronous settings transaction while that
mutation holds the shared persistence lease.

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
Formatting lane. Setup can continue with account-only access. Use **Add/Change**
in the API-key row to edit a key, or **Connect/Manage** in the Agent-account row
to open Providers. A lane
is usable only when its resolved runtime snapshot reports it available.

Setup reuses the Providers sign-in flow so its callbacks, pending state and
account errors stay with the Settings model. The wizard stays open and refreshes
the account/key snapshot when it regains focus.

### Model discovery

Model discovery queries the provider's model API with its provider API key.
Account sign-in alone does not supply that key. With account-only Assistive
access, `/model` shows the currently resolved model and explains the missing
catalog access. Keep that model, or enter a supported **Model ID** in **Agent →
LLM lanes**. Adding an API key is optional for Assistive account requests.

Fresh and cached catalogs offer selectable models. A provider returning no
models or a discovery failure gives the corresponding explanation and next
action. Check **Providers**, then **Refresh** models in **Agent → LLM lanes**.
The palette reuses its model list while you filter it, and refreshes it after
provider, model or credential changes, including settings edited outside the app.
Its short cache also expires automatically. If freshness cannot be established,
it reads the current context again instead of reusing an unverified list.

Palette labels and grant actions follow the macOS interface language through
the app's String Catalog. Model IDs, provider IDs and tool grant keys remain
unchanged.

Open **Settings → Agent → Diagnostics** for the complete agent connection
report: the core readiness rows, managed skill status and installation paths,
capability matrix, MCP status, and a single Refresh action. Native-tool or
workspace failures remain visible there even when credentials are valid.

Setup keeps the Agent step to one decision: which clients to connect. It shows
only a short ready state or an inline setup action and error. The preceding
provider step presents account and API-key presence from the provider credential
snapshot; diagnostic readiness describes usable provider access and must not be
read as proof that an API key exists.

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

Stopping through the tray or a shortcut updates this panel too. A final formatting
pass still counts as finishing even before the overlay changes its visible phase.
After a failed start, **Start recording** uses the same fresh-capture admission as
the tray, so it can retry without a stale capture fence. It waits for the tray's
previous start to settle and cannot turn that retry into a Stop.

Calibration is disabled while a take is active or finishing. These controls use
the existing RecordingController path; opening Audio does not create another
recorder.

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
make config
```

That opens or creates `~/.codescribe/.env`.

When migrating an installation that only has this file, the first Settings
write preserves its promoted choices and records pending credential imports
before saving your edit. Credential access completes those imports later;
opening Settings alone projects the imported choices without acquiring
credentials or creating `settings.json`. The pending import becomes durable
when the first writer prepares it, not when the preview appears.

Malformed JSON, unsupported schema versions and an unreadable existing settings
file produce a configuration refusal even in a Keychain-free snapshot. The
runtime seal remains disarmed; a read-only check preserves the original file
without creating a backup or silently replacing it. Safe field normalization can
appear in the preview, while persistent repair belongs to the writer.

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

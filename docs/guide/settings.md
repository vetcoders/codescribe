# Settings & Configuration

Codescribe now has one native Settings window with five tabs:

1. **Transcription**
2. **Hotkeys**
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

## Creator

The first desk groups what a new install needs: the two language rows, the
permission checklist with a **System Settings…** link in its header (the
Privacy & Security pane, so a granted scope can be reviewed or revoked; a
missing scope still carries its own request or deep link on the row), voice
and formatting, one **Max consultation** card that also holds the pending
approval requests, the coding-agent skill rows with **Refresh status** on the
section header, and the quick-start cards.

### Languages

**Interface language** switches the app between
Polski and English. The choice is saved at once as Codescribe's per-app macOS
language preference (the same one System Settings › General › Language & Region
› Applications shows); it never touches `settings.json` or the dictation
language. The running app keeps its language until you press **Restart now**
(the restart note and button already appear in the language you just chose):
Codescribe waits for an idle moment (no recording, no agent turn) and relaunches
in the chosen language. If a take or an agent turn is in progress, the row keeps
your choice and asks you to try again. The setup wizard's first screen offers
the same switch.

**Speech recognition language** sits directly below it with the same segmented
control: Multilingual (automatic detection per recording), Polish or English
(fine-tuned models). It writes the dictation language in `settings.json` and
has nothing to do with the interface language above it.

## Transcription

Open **Settings → Transcription**.

This tab owns the transcript pipeline itself:

- **Final Transcript Path**
  - `Local transcript`
  - `Cloud final transcript`
  - optional cloud endpoint + API key
- **Transcript display pace** (Dictation → Preview)
  - presets: `Smooth`, `Snappy`, `Relaxed`, `No preview`, `Custom`
  - `Detailed settings` — collapsed by default, opened by `Custom`:
    - `Update delay` → `CODESCRIBE_BUFFER_DELAY_MS`
    - `Character pace` → `CODESCRIBE_TYPING_CPS`
    - `Max words per update` → `CODESCRIBE_EMIT_WORDS_MAX`
    - `Interim result interval` → `CODESCRIBE_BUFFERED_INTERIM_SEC`
  - `No preview` writes only `TRANSCRIPTION_OVERLAY_ENABLED=0` and keeps the
    four values on disk; the sliders are disabled while it is selected.
    `Custom` turns the preview back on without touching those values.
  - moving any slider makes the configuration `Custom`; the three named presets
    are detected back from the stored values within a small tolerance
- **Final Transcript**
  - `Local file-based final pass`
  - `AI Formatting`
  - `Formatting level`
- **Quality Automation**
  - app-launch quality daemon toggle
  - latest report / availability / pending mismatch state

### Current runtime truth

- When **Transcription overlay** is ON, the app is optimized for low-latency live preview.
- When **Transcription overlay** is OFF, the floating preview is hidden and runtime uses a more buffered cadence to reduce local load. Concretely, a non-assistive take ignores the stored `Interim result interval` and runs at the fixed no-overlay cadence instead (`app/controller/mod.rs`, `apply_runtime_transcription_profile`). An agent (assistive) take keeps the stored interval even with the overlay off.
- The interim cadence is an audio-segmentation knob, not only a display knob: `core/audio/chunker.rs` turns it into `interim_limit` and cuts the utterance there, so the engine sees different slices. Do not promise that the display-pace settings leave the committed transcript untouched.
- Turning it OFF — from the tray toggle or the Settings preview preset — also closes an overlay that is already on screen; it does not wait for the next take. Two things stay: an open agent channel, whose live microphone stays visible, and a take you are correcting — while the caret is in the transcript or a draft is not committed, the overlay waits, then leaves after the usual five seconds once the draft is committed or discarded.
- A blocked recording or a microphone calibration result still shows its status card with the overlay OFF. That card leaves by itself after the usual five seconds, even when **Keep visible between takes** is pinned: the pin keeps the transcript overlay, and with the overlay OFF there is none.
- `USE_LOCAL_STT=0` changes the **committed transcript path after capture**; it does not move live preview to the cloud.
- In the current build, **cloud STT is still post-capture**, not live cloud preview. The Settings UI states this explicitly.

## Dictation tabs

The Dictation pane is one tab per concern:

1. **Engine** — _Recognition mode_ first (Apple only, Local power, Cloud), with a
   one-line description of the selected mode under the picker. Below it, _Last
   transcription_: the engine that served the last take (runtime truth from the
   serving verdict, “No transcription in this app session” before the first take,
   no readiness dot), the local Whisper model row only in Local power (the saved
   selection; Cloud shows no model row), and the spoken language as “Polish (pl)”.
   The language applies to Apple live recognition, local Whisper and the cloud
   tail alike.
2. **Whisper** — _Selected model_ (picker, install state with **Check model**, a
   resident-vs-next-load row that never calls the next load “in use”), _Other
   detected models_ (models on disk the loader refuses, with a plain reason),
   _Data footprint_ (installed directories with state, size and **Remove**; the
   selected model explains why it cannot be removed). Full paths, sources and raw
   validation errors live under the collapsed **Model details**.
3. **Preview** — the transcript display pace; the presets, sliders and what they
   really drive are described under [Transcription](#transcription) above.
4. **Privacy** — see [Cloud & privacy](#cloud--privacy) below.
5. **Permissions** — the live macOS permission matrix.

The raw recognition timings are not a Dictation tab. **Pause recognition after
silence** (`TOGGLE_SILENCE_SEC`), **Whisper context length**
(`WHISPER_CONTEXT_WINDOW_SEC`) and **Sentence pause**
(`LIGHT_PLUS_SENTENCE_PAUSE_SEC`) live in one **Speech recognition parameters**
group on the **Lab** desk, which only appears in builds with the developer
surface baked in (`CSDeveloperSurface`). They are parameters, not product
choices; their ranges, defaults and promoted keys are unchanged by the move.
Sentence pause belongs to Light+ text shaping, not to hands-free dictation.

### Cloud & privacy

**Settings → Dictation → Privacy** shows two short sections, with the rest
on demand:

- **Cloud status** — the selected recognition mode and the stored cloud
  consent as two separate rows. A granted record is not evidence that audio
  is leaving now.
- **What can leave this computer?** — two scannable rows. **Audio**: in Cloud
  mode, and when you start a cloud re-transcription of a recording yourself.
  **Text**: during AI requests to the providers you configured.
- **Privacy details** — collapsed by default, nothing removed. Expanding it
  shows four short subsections: the content-free cloud session diagnostics,
  Keychain storage for the keys you configure (and the Libraxis gateway
  keeping the live lane vendor-key-free), what a missing consent resolves to
  (Apple on-device plus your dictionary, with no local model loaded in its
  place), and the fact that choosing `Local power` does not download
  anything. The **Configure cloud services** action below jumps to
  **Providers → Cloud transcription**.

Selecting **Cloud** on the Engine tab is itself the audio-egress grant: it
writes `CODESCRIBE_CLOUD_CONSENT=granted` together with the mode.

## Shortcuts & control

Open **Settings → Hotkeys**. The page is headed **Shortcuts & control**:
how to start Codescribe's modes and how to control recording.

The tab holds two different save contracts, and the note under the Save
buttons says so: the three mode gestures are a draft and need **Save mode
shortcuts**; every other control on the page writes as soon as you change it.

**Mode gestures.** One gesture per work mode — **Dictation** (turns speech into
text), **Formatting** (dictation with AI formatting) and **Agent** (passes the
recognized text to the Agent). None of the three promises a paste: where the
transcript goes is **Automatic paste** below, and `PASTE_MODE=off` means nowhere.
The gesture pill shows the chord (`2× Left ⌥ (Option)`); VoiceOver reads the
spelled-out form, so the left and right Option gestures stay distinguishable.

**Save mode shortcuts** / **Restore default mode shortcuts** sit directly under
the three rows. The screen reports a blocking conflict that refuses the save,
otherwise unsaved changes; underneath, and independently of either line, what
the last save actually persisted. The confirmation is a re-read from disk, not
an echo of the picker: the bridge can refuse one mode while accepting another
in the same save, so a refused gesture is named and its picker snaps back to
the gesture it still holds. That snap-back can itself land in a conflict (a
refused Agent hold returns to Double Right Option, which Double Ctrl dictation
disables); the conflict line and the receipt then show together. The receipt
says **Saved**, not "in effect": a binding present in `settings.json` is not
proof that the gesture fires — see **Settings picker vs routed combinations**
in `docs/HOTKEYS_CONTRACT.md`. Mode names in the receipt are joined in the
interface language, not the macOS region.

**Conflicts** and notes are separate. A conflict blocks the save and sits in a
coloured card above the Save button; a note does not block and sits under it as
a quiet grey field with a globe symbol and secondary text. The macOS Fn configuration message is a note: it
says Codescribe may intercept the short press while dictation runs, and
explicitly that it does not block saving. The technical identifier that came
across the bridge is not shown; the save receipt and the bridge log keep it.

**Dictation control** sits directly under the gestures because it shapes the
same recording gestures; both switches write on change. **Tap Fn to dictate**:
one tap starts, the next stops, a longer hold records only while held (set the
macOS Fn key action to _Do Nothing_, otherwise macOS can claim a double press
for its own dictation). **Middle mouse acts as Fn**: the middle button follows
the same press, hold and tap rules as Fn; custom mappings in your mouse
software can block the standard middle-click signal the app listens for.

**During dictation** is a quiet neutral card: pressing the chosen key during
an already-started dictation hold attaches the selected text to the take.
**Arm with** chooses Shift (default) or Command, and the card's gesture label
derives from that choice. Further pulses during the same hold attach the next
selections; the take, the overlay and the destination do not change.

**Extra shortcuts** holds the two shortcuts beyond the modes. **Agent channel**
(`Ctrl + digit`, or `Fn + digit`; Command is not offered because it collides
with tab switching) and **Paste transcript**: the chord that pastes a
transcript waiting to be inserted — the app in front may handle the same
chord as well.

**Automatic paste** keeps **Safe**, **Comfort** and **Off**, with the picker on
its own full-width row and only the selected mode explained underneath. The
terminal, command and password-field safeguards are unchanged.

**Recording indicator** names the three dot states in full — Recording, Agent,
Processing — and sets the size of the indicator next to the cursor
(Off / 4px / 8px / 12px; the Agent indicator stays proportionally larger).

`HOLD_START_DELAY_MS` and `DOUBLE_TAP_INTERVAL_MS` govern the same gestures but
have no control on this tab; they are settings keys
(`docs/ENV_REGISTRY.toml`, `docs/HOTKEYS_CONTRACT.md`).

## Providers and Agent

Open **Settings → Providers** to connect accounts and add API keys. Each
provider card shows its key as one line — **API key · Set** with **Change**
(or **Add**) opening the editor — and, for vendors with a sign-in flow, one
account line with a single action: **Sign out** while connected (the line
names the account, e.g. **Connected as name@example.com**), **Sign in with …**
otherwise. Vendor endpoints are factory-defined and sit under each card's
**Advanced** disclosure together with the Keychain account name and the OAuth
client-id override; custom hosts show their endpoint on the card and edit it
through **Edit**. **Cloud transcription** holds the File and Live lanes
(endpoint, key, and for Live the optional gateway session URL); a rejected
address reads as one sentence under the field, e.g. **This address needs
ws:// or wss://.** Secrets are stored separately from account sign-in.

Open **Settings → Agent → AI models** to select a provider and model separately
for **Assistive** (the model behind the Agent and the voice assistant) and
**Formatting** (transcript cleanup). Each card shows the provider, the model
field and one line about the lane's access: **Connected account**, **Stored API
key** or **No key required**. These describe what is stored, not whether it
works: a stored key can still be rejected, and a connected account does not open
model discovery. The model field holds your override; when it is empty, the
placeholder is the provider default that actually resolved, and the caption
says **Provider default model** or **Set manually**. **Reset model** clears only
the override and never touches the provider. The settings keys
(`LLM_ASSISTIVE_PROVIDER`, `LLM_ASSISTIVE_MODEL`, …) and the resolved endpoints
sit under **Active configuration details**, collapsed by default. **Agent →
Prompts** edits their prompts.

**Automatic send to the Agent** holds one switch: in Agent mode the untouched
transcript is sent 5 seconds after the take ends unless you start editing it.

### Agent → Prompts

One segmented picker (**Correction**, **Smart**, **Max**, **Agent**) opens one
base prompt at a time; the headers read **Correction prompt**, **Smart prompt**,
**Max prompt** and **Agent prompt**. Each has a single plain sentence under it.
The Agent prompt is the base of the system prompt for Agent turns that act on a
dictated request; voice chat carries its own persona and does not read it.
Codescribe may append further instructions at runtime, so the editor shows the
base text, not the full prompt a provider receives.

The **Source** line names the prompt in use: **Source: Custom prompt** when your
file is read, **Source: Built-in prompt** when no custom file exists or the file
is empty, and **Source: Built-in prompt (file unreadable)** with a red sentence
when the file could not be read. **File details**, collapsed by default, holds
the path, whether a custom file exists or would be created there on save, and
the raw read error.

**Edit** opens the raw text; **Save** (solid accent) writes it and returns to
the rendered view; **Cancel** drops the unsaved draft. Edit state is kept per
prompt: switching segments mid-edit keeps that prompt in edit mode with an
**Unsaved changes** marker, and the rendered view always shows the saved text,
never a draft. **Restore default…** asks for confirmation that names the prompt
and changes only that one. Confirming copies the custom file into the prompt
backups folder, removes it, and records the removal in the prompt audit log,
so the source afterwards reads **Built-in prompt** and the text follows future
app updates. If the file cannot be removed, a red line under the source says
**Could not complete restoring …** with the error and refreshes the actual
source. An error can occur after the file has changed (for example while
synchronizing the directory or writing its receipt); the backup remains
recoverable. A failed save is reported with the same current-source check.

### Agent → Workspace

**Folders available to the Agent** lists where the Agent's built-in file and
terminal tools may read and write; a path outside the list is refused. The same
list bounds the paths Codescribe hands to MCP tools it knows how to check
(Desktop Commander's file and process tools go through the same validator),
so adding or removing a folder here also changes what those tools may touch
through Codescribe. What an MCP server does on its own, outside a call
Codescribe validates, is not bounded by this list. The same list is where the Agent
looks for projects and Git repositories (subfolders included, hidden folders
and build directories skipped), so entries such as `~/.codescribe` or `/tmp`
sit next to checkouts like `~/Git` — it is one access list, not a list of
projects. A green dot marks an existing directory, amber one that does not
resolve. **Add folder…** opens a folder picker and adds the choice as an
editable row; the minus button (**Remove folder**) drops a row, and **Undo
remove** puts the last removed row back where it was. Nothing is written until
**Save changes**; **Discard changes** returns to the saved list.

### Credential access while refreshing

Settings and Setup read provider credentials in the background. The initial
read shows **Checking provider access…** rather than claiming an account or
key is missing. An access error remains visible with **Try again** in Setup;
a previous successful snapshot is labeled as the last checked state. Settings
also offers **Refresh status**; while the read runs, the spinner and
**Checking provider access…** sit in a fixed slot beside the button, and once
it lands the slot keeps **Checked at HH:MM:SS** so even an instant refresh
leaves a visible receipt. Returning focus refreshes only the owning Settings
or Setup window, and repeated requests share the pending read.
Permission changes refresh permissions and hotkeys separately.

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
catalog access. Keep that model, or enter a supported model ID in **Agent →
AI models**. Adding an API key is optional for Assistive account requests.

Fresh and cached catalogs offer selectable models. A provider returning no
models or a discovery failure gives one plain sentence and the next action:
**Could not fetch xAI models. The API key was rejected. Check it under
Providers.** when the provider refused the key (HTTP 401/403, or a 400 whose
body names the API key), otherwise **Could not fetch … models. Check the
provider under Providers.** The provider's raw response is available under
**Error details**; it never appears in the main line. When both lanes use the
same provider, Formatting points to the Agent's line instead of repeating the
error. **Refresh** retries.
The palette reuses its model list while you filter it, and refreshes it after
provider, model or credential changes, including settings edited outside the app.
Its short cache also expires automatically. If freshness cannot be established,
it reads the current context again instead of reusing an unverified list.

Palette labels and grant actions follow the macOS interface language through
the app's String Catalog. Model IDs, provider IDs and tool grant keys remain
unchanged.

Open **Settings → Agent → Diagnostics** (headline "Agent environment status")
for the agent status screen: the readiness verdict with its prerequisite rows,
the detected skill installations, one summary line each for capabilities and
MCP servers, and a single Refresh action. Native-tool or workspace failures
remain visible there even when credentials are valid. Long diagnostic values
wrap within the pane, keeping labels and controls visible when the sidebar is
open. See "Agent → Diagnostics" below for what each part shows.
Managed skill status is read when Settings opens, when Diagnostics is selected
and after launch synchronization finishes. A direct link refreshes even if
Diagnostics is already selected. These inspections do not install skills or
attach listeners.

### Agent → Tools

Tools is the permissions screen: when the Agent may use a tool without
asking (Allow), when it needs approval (Ask), and when it must refuse (Deny).

- **Defaults** — one row per category: Read data, Changes/processes/network,
  Unclassified tools. These are the stored category defaults and apply to every
  tool without a more specific rule.
- **Resolution order** — a rule set for one tool outranks its server's rule,
  and both outrank the category defaults. External destructive tools are
  always refused, and an Allow never silently covers a path that may hold
  secrets (`.env`, key material): that call asks first.
- **Per-tool permissions · N** — N is the whole tool catalog, not the number
  of individual rules. Opening the tab discovers the catalog by starting every
  configured MCP server and asking it for its tools, so the list appears a few
  seconds after the defaults; a "Discovering tools from the MCP servers…" row
  stands in until then. Tool sources down the left (Native plus every MCP
  server, names verbatim), the selected source's tools on the right. Each row
  shows a readable name above the raw identity, the source and localized risk
  class, and where the level comes from: "Individual rule", "Server rule" or
  "Category default". Codescribe's own tools are named in the interface
  language; an MCP server's tools keep the vendor's spelling. Both lines stay
  on one line, so the row's tooltip carries the full name and the full
  identifier. "Remove rule" drops an individual rule; the row then shows the
  server rule or the category default again.
- The level a row shows is the level the gate applies to the tool's next call:
  Settings and the runtime read the same resolver, so a category default
  changed here takes effect without an explicit rule per tool.

### Agent → Diagnostics

Diagnostics is a status screen and the entry point for troubleshooting, not an
inventory. The core reports every row as a stable facet and state with its
structured parts (counts, provider or server name, error cause); the app
renders the interface-language text from those, so the Polish and English
screens never depend on parsing the English probe text.

- **Agent readiness** — the verdict pill plus one row per prerequisite:
  Overall status, Model provider, Native tools, Folders available to the Agent,
  then the optional operator tooling (VibeCrafted runtime, AICX MCP, Loctree MCP,
  PRView integration). Every row ends with a status mark: a dot and a word
  (Good, Warning, Error, Not checked) that is also the tooltip and the
  VoiceOver label.
- **Detected installations and runtime** — one block per detected client
  (Claude Code, Codex) with its managed skill path, the installer's evidence
  line, and the launch synchronization notice folded under "Technical details".
- **Available tools and integrations** — one line of counts (Native · Enhanced
  · Unavailable). "Show details" expands the capability matrix with localized
  tier badges and a readable headline per operation; the core's raw reason is
  the dot's tooltip. Permissions are managed in the Tools tab.
- **MCP servers** — the configuration source path, one line of counts
  (Configured · Tested · Issues), and a note when every server still waits for
  the agent's first turn. "Show servers" expands one merged table: server name,
  runtime status from the probe, and the cached test result. Servers are added,
  tested and removed in the MCP tab. Without any configured server the section
  shows the single configuration state row instead (no mcp.json, empty config,
  or the concrete read error).

### Agent → MCP

The MCP tab is the editing surface for `~/.codescribe/mcp.json`; Diagnostics
only reports it. The headline says what the tab is for (add servers, manage
the tools the Agent may use) and the list reads as servers, not as a config
dump.

- **Server card** — the name, the configured state as a flag button (Enabled /
  Disabled flips `enabled` in `mcp.json`; it never connects or disconnects
  anything), the last handshake, and the Test / Remove actions. Remove only
  asks: an alert names the server and what goes with it (its entry in
  `mcp.json` and its Keychain token); Cancel, Escape or closing the alert
  leaves the configuration as it was, and only "Remove server" removes. "Details"
  folds the transport, the launch command or server URL, environment keys,
  authentication (token in Keychain or none), the server-wide permission rule
  read from the live policy, the identity the server advertised (name,
  version, protocol) and the raw error of a failed handshake. Identifiers,
  paths and URLs stay verbatim.
- **Last handshake** — Test spawns the server once and lists its tools. The
  card shows "Connection not tested", "Checking the connection…", "Last test:
  passed · N tools" or "Last test: failed" (reason under Details). It is a
  test result, not a live connection indicator: the Agent starts servers per
  turn. Toggling the flag drops the cached result, so a card never reports a
  configuration that was just changed.
- **Add server** — a segmented choice between a local process and an HTTP
  connection, then labelled fields: server name, launch command and command
  arguments, or server URL and an optional access token. The token goes to
  the macOS Keychain, never into `mcp.json`. Each field's caption is also its
  accessibility name, so VoiceOver reads "Server name" or "Access token
  (optional)" rather than the placeholder or the typed text. Add is live as
  soon as anything is typed; the store does the checking. A rejected add
  keeps everything typed and says in plain words what to fix, under the field
  it names and with focus moved there: an unparseable or non-HTTP URL,
  credentials inside the URL, an empty command, a name with surrounding
  spaces or unsupported characters, a name already taken. The store's own
  message stays available as a tooltip on that line; it never appears raw on
  the screen.
- **Technical details** — the on-disk note (hand edits and unknown fields are
  preserved), the file path, and "Move MCP configuration to Trash…", which
  after confirmation moves only `mcp.json` to Trash.

Removing a single server also deletes its Keychain token; the alert says so,
and there is no undo after it.

The Settings window carries the title "Settings" for Mission Control, App
Exposé and the Window menu while the toolbar shows the wordmark instead.

Setup keeps the Agent step to one decision: which clients to connect. It shows
only a short ready state or an inline setup action and error. The preceding
provider step presents account and API-key presence from the provider credential
snapshot; diagnostic readiness describes usable provider access and must not be
read as proof that an API key exists.
Setup shows the API-key row and editor whenever the provider has an API-key
account, including optional keys for custom endpoints. Whether a key is required
does not decide whether it can be edited or saved. Providers without an API-key
account expose no editor or save action.
Readiness also requires the loader's sealed lane to be usable, including a
selected model for a custom provider. A key-optional endpoint alone is not ready.
An unresolved, pending or failed provider read cannot show a ready verdict.
Switching providers preserves separate drafts while collapsing the optional
key editor; Continue saves a draft only while that editor is visible. A restored
hidden draft remains available through Add or Change without a Keychain write.
Provider selection is projected only after its configuration write succeeds.
Selection errors and their retry stay beside the provider picker; key-save
errors and their retry stay beside the key editor. Earlier setup errors do not
become key-save errors.
If a configured provider disappears from the registry, Setup can display an
available provider without persisting that choice. Explicitly selecting the
displayed provider writes it; refresh, Back and Skip do not normalize configuration.

The Agent step distinguishes a selected client's missing or damaged installation
from a global provider or native-readiness problem. Global issues open Diagnostics
without selecting or installing another client. An installation error belongs
to the single client affected by the attempted change; errors spanning multiple
clients appear beneath the selection instead of being assigned to an arbitrary
card.
Managed installation health includes the receipt-owned skill files, so missing
or altered instructions also expose repair. An empty selection can forget a
folder already absent; it never deletes an existing unowned or unreadable path.

Prompt files live in `~/.codescribe/prompts/`.

## Audio & Input

Open **Settings → Audio** — headlined **Microphone and recording**. The pane owns
the microphone, recording readiness, how long recorded audio is kept, and the
start signal. Nothing else: the transcription overlay is set under
[Dictation → Preview](#dictation-tabs), and the Dock icon from the menu bar
menu — neither lives in this pane.

The Settings and Agent windows cannot be minimised, with or without the Dock
icon. With the Dock's default “Minimize windows into application icon”, a
minimised Codescribe window leaves the screen and shows up only as an empty
tile in App Exposé and Mission Control, so the yellow button (and ⌘M) is
disabled on both windows; close and reopen them instead.

**Microphone** — one card: the **Input device** picker, the microphone actually
recording, and **Refresh**. **System default** is the first option of the picker,
so Codescribe records on whichever microphone macOS currently uses; a named device
is remembered, and if it is unplugged recording continues on the system
microphone. **Currently:** names the input the running recorder resolved — the
live device, not the saved choice. **Refresh** re-reads the device list; it does
not re-run the readiness checks below. The card stays at those three lines while
nothing is wrong. A saved device that the running recorder is not actually using,
or a Mac with no input hardware at all, adds one sentence saying so — a restart
applies a saved device, and an explicit `AUDIO_INPUT_DEVICE` launch override keeps
winning until it is removed.

**Recording readiness** — one status line plus two values, instead of four
numbered steps:

- The status line is the recorder's own verdict: **Ready to record** with
  **Start recording**, or the live phase while a take is starting, running or
  finishing. **Start recording** names the configured Dictation gesture when
  Hotkeys binds one. After a failed start it uses the same fresh-capture
  admission as the tray, so it can retry without a stale capture fence; it waits
  for the tray's previous start to settle and cannot turn that retry into a Stop.
- **Microphone calibration** — `Ready` or `Required`, with **Recalibrate**.
  Calibration measures about 10 seconds of normal speech through the real
  recorder path, and is unavailable while a take is active, starting or
  finishing.
- **Committing fragments** — `On`, `Off` or `Unavailable`. Audio states the
  effective value and nothing else; the switch lives on the **Lab** desk.

A neutral status never hides a blocker. When microphone access is missing,
calibration has not been measured, the committing lane is off or its detector is
unavailable, or there is no input device, the status line becomes that problem
and carries its remedy — **Allow** or **System Settings** for the permission,
**Calibrate**, or **Open Lab** for the committing switch. The explanation is the same sentence the readiness
rows used before; it is simply shown only when it applies.

These controls borrow the existing `RecordingController`; opening Audio never
creates a second recorder.

The stored calibration profile identifier, the measured device, the sample rate,
the loader verdict and the calibration file sit under the collapsed **Calibration
details**, as label and value pairs.

**Committing transcript fragments** lives on the **Lab** desk (developer builds
only), as a switch with one line: _Required before a recording can start._
Turning the recorder's own precondition off is a power-user act, so only that
desk can do it. When `CODESCRIBE_SILERO_FUSION` is set the switch is read-only
and says so: remove the override to edit the setting again. A build without the
developer surface shows the `Off` state and its blocker in Audio, but has no
switch — the setting is then changed in `settings.json` or by removing the
override.

**Audio retention** — `Keep completed recordings`. `Forever` (the default) adds
no sentence; the choices that expire something each carry one short line:

- `Forever` (default) — nothing expires automatically. An unknown value stored in
  `settings.json` resolves here, exactly as the config loader resolves it.
- `30 days`, `7 days`, `24h` — a completed recording's audio is deleted that long
  after it finishes, including recordings that are already past the age.
- `Off` — a new recording's audio is discarded as soon as processing finishes.
  Recordings already saved are kept.

Each sentence states that text history stays. A take keeps the choice it started
with: switching to `Off` mid-take does not shorten that take, and a take captured
under `Off` is still discarded if the choice is changed afterwards.

**Recording start sound** — **Play a signal** plays the recorder's live start
confirmation, and **Volume** below it shows the level as a percentage. The slider
is disabled while the signal is off.

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

## Dictionary

**Settings → Dictionary** shows what was corrected, what the app actually
learned, and where the active rules come from.

- **Counters** — three separate values: corrections (every take whose text
  changed, not only the recent ones shown below), unchanged takes (kept for
  their confidence telemetry only) and active rules (every variant → canonical
  pair the engine applies). A vocabulary correction is not a learned rule;
  nothing here implies otherwise.
- **Recent corrections** — one card per correction. **Differences between
  versions** compares the stages that actually changed: _Formatting changed
  (raw STT → delivered)_ when Smart/Max rewrote the raw text, and _Your
  correction (delivered → corrected)_ for the manual edit, so a formatter's
  rewrite is never charged to the engine's hearing. Each span is labelled
  **Added**, **Removed** or **Replaced**; replaced fragments can span several
  words. Minor casing and punctuation changes stay collapsed. **Full
  comparison · X → Y characters** opens the raw STT, the text after
  formatting and the text after your correction. The footer reads _Version N ·
  date_ in the interface language. **Diagnostic details** holds the count of
  records without confidence telemetry; it describes the records, not the
  engine.
- **Play original / Retranscribe** — the archived take is paired by its exact
  raw transcript; the pairing runs in the background when a card opens, and
  Retranscribe stays disabled until it is known. When several archived
  recordings share that transcript the
  pairing is ambiguous and both actions refuse, saying so; the panel also
  explains the other reasons Retranscribe is unavailable (no archived
  recording, no helper engine in Apple-only mode, a pass still running).
- **Learn from corrections…** — reviews every saved correction and the
  suggested rules, then adds the new vocabulary rules it can derive. The
  confirmation states that scope first; the result line reports the real
  growth of the rules list (_Added 2 rules from corrections · 9 active
  rules_, or _No new rules_ when everything eligible was already learned).
  Corrections, their revision history and the extraction safeguards are
  unchanged by learning.
- **My rules** — the active rules with their provenance (_from a correction_
  or _added by hand_); up to five rules read as a list, more are paged.
  Rules cannot be edited or removed from the app yet; see
  [CONFIG.md](../CONFIG.md) for the lexicon files.

## About

**Settings → About** (the last item under _Account_) describes the app and its
data instead of a profile; Codescribe has no account. Everyday facts stay
visible; technical values and the resets open on demand.

- **Version** — one line, _Codescribe 0.16.0_ with _Build 526_ under it.
  **Version details** opens the commit and the build date in the interface
  language.
- **Configuration notice** — shown only when the launch repair receipt (see
  [CONFIG.md](../CONFIG.md)) has something to say, directly under the version.
  For `.env` keys it reads _The configuration needs a review — See which setting
  is out of date_. Opening it names each key with what it does in this build
  and whether anything needs doing: an unknown or retired key in the optional
  `.env` file is not read and has no effect, so no action is required (delete
  or correct the line and restart to clear the notice); a `FORMATTING_LEVEL`
  note means a level set outside the app differs from the one in Settings and
  is in effect. A completed repair or a refusal has its own sentence. The
  original receipt line stays at the bottom for support. Nothing in About edits
  `.env`.
- **Local data** — the app-data folder and the Transcripts folder, shown
  home-relative (`~/.codescribe`), with a copy button for the full path.
- **First dictation confirmation** — appears only in a build that can send it.
  While the build ships without an analytics domain
  (`ActivationPingConfiguration.production`), the opt-in switch is not shown at
  all, because no position of it would send anything. The stored choice is kept
  and the switch returns with the service.
- **Transcript markers** — one switch, _Add markers to text_, which marks the
  text delivered to other apps. **Edit template and preview** is closed by
  default and holds the editor, the field chips (`{mode}`, `{lang}`, `{text}`,
  `{conf}`, `{flags}` — each chip appends its field to the template), the
  warning when `{text}` is missing, **Restore default template** and the
  rendered **Template preview**. Saved templates and the marking mechanics do
  not change. The markers change delivered dictation, so they are expected to
  move to the Dictation settings; until then they live here.
- **Information and documentation** — Privacy Policy, Terms of Use,
  Documentation.
- **Reset data** — one closed row at the foot of the page; **Expand** shows
  the two resets described below.

## Reset / Fresh Start

Both resets live under **Settings → About → Reset data**, closed by default.
Each block names its scope in one sentence; the confirmation sheet shows the
live counts and the full scope before anything moves, and asks for a typed
word. Only the two buttons are red.

- **Reset Agent data** — button **Reset Agent…** (type `RESET AGENT`) — moves
  Agent conversations, MCP configuration and tool state to Trash and deletes
  Agent provider keys and MCP connector secrets from Keychain permanently. The
  deleted vendor accounts (`LLM_OPENAI_API_KEY`, `LLM_ANTHROPIC_API_KEY`,
  `LLM_XAI_API_KEY`, `LLM_LIBRAXIS_API_KEY`) are the same accounts the
  Formatting lane reads on that vendor, so the confirmation says that
  Formatting on such a vendor needs its key again afterwards. Recordings,
  transcripts, dictionary, prompts, hotkeys, dictation settings, license and
  macOS permissions stay.
- **Reset app data** — button **Move app data to Trash…** (type `RESET`) —
  moves recordings, transcripts, conversations, logs, preferences and local
  configuration to Trash and relaunches. Two opt-in checkboxes: _Also remove
  API keys from Keychain_ (not recoverable from Trash) and _Also reset my base
  prompts_ — `assistive.txt`, `formatting.txt`, `formatting-smart.txt` and
  `formatting-max.txt`, all four named on the checkbox and in the confirmation.
- **New agent context**: Chat Overlay → **New thread**
- **Reset prompts**: Settings → **Agent → Prompts** → **Restore default…**

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

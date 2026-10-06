# Localization Ledger

> Inventory of user-facing copy in the macOS app: which classes exist, where
> they live, how each is localized, what is ready and what still needs work or a
> human decision. The rules are in `docs/LOCALIZATION.md`; this file is the map.
>
> Created by Vetcoders (c)2026

Scope: the macOS app (`macos/Codescribe`) and the text Rust hands to it. The
website, the docs, the CLI and model prompts are outside this ledger.

---

## 1. Classes of copy

| Class                               | Where it lives                                                                                                                 | How it is localized                                                                                                 | State                       |
| ----------------------------------- | ------------------------------------------------------------------------------------------------------------------------------ | ------------------------------------------------------------------------------------------------------------------- | --------------------------- |
| SwiftUI literals                    | `Text("…")`, `Button("…")`, `.help("…")`, … across `Screens/`                                                                  | Extracted by the compiler (R1)                                                                                      | Ready                       |
| Display copy held as `String`       | View models, enum display names, component arguments, state messages                                                           | `String(localized:)` at the literal (R2)                                                                            | Ready                       |
| Accessibility labels, hints, values | `.accessibilityLabel/Hint/Value`, AppKit accessibility titles                                                                  | R1 for literals, R2 for composed values                                                                             | Ready                       |
| AppKit copy                         | Window titles, `NSMenuItem`, `NSOpenPanel` prompts, the About panel                                                            | `String(localized:)` (R2)                                                                                           | Ready                       |
| Counted phrases                     | Any string with a count governing a word                                                                                       | One key, plural variations in the catalog (R4)                                                                      | Ready (English forms)       |
| Settings search keywords            | `SettingsSection.searchKeywords`, `SettingsTab.searchKeywords`                                                                 | Technical terms stay fixed; natural-language terms come from the catalog                                            | Ready — §3                  |
| System permission prompts           | `NS…UsageDescription` in `macos/project.yml`                                                                                   | `InfoPlist.xcstrings`, kept identical to the plist by `verify-l10n-catalog`                                         | Ready                       |
| Updater interface                   | Sparkle framework                                                                                                              | Sparkle ships its own translations; they activate with the bundle language                                          | Nothing to do               |
| Dates, times, numbers               | Formatters in view models                                                                                                      | System formatters (R8)                                                                                              | Hand-built patterns: see §5 |
| Text composed in Rust               | `CsError` messages, status rows, toasts, notifications                                                                         | Cannot be localized on the Swift side                                                                               | Not ready — §4              |
| Surfaces drawn by Rust              | System notifications, the OAuth landing page, thread export                                                                    | Never pass through Swift                                                                                            | Not ready — §4              |
| Not copy                            | Logs, diagnostics, receipts, machine contracts, prompts, user and model content, proper names                                  | Never localized (`docs/LOCALIZATION.md` §4)                                                                         | By design                   |
| Developer surfaces                  | `DesignGallery`, `#Preview` fixtures, `#if DEBUG` panels, mock engines, the Lab pane and the rows only a developer build shows | Left in English: marked verbatim, or — where the copy is extracted — `shouldTranslate: false` in the catalog (§5.3) | By design                   |

## 2. Where the app stands

`Localizable.xcstrings` holds **1420 keys** (1398 translatable, source inventory
2026-10-06). Before the initial localization work the compiler extracted 468 —
the literals SwiftUI localizes by itself; the
rest was plain `String` and invisible to any translation. **Polish copy covers
every translatable key** in both catalogs (1398/1398 and 4/4), initially imported from the translator worksheet
(`scripts/l10n-sheet.py`); the catalog is the source of the translation from
here on.

| Measure                                        | Count    |
| ---------------------------------------------- | -------- |
| Keys in `Localizable.xcstrings`                | 1420     |
| Keys with Polish copy                          | 1398     |
| License keys awaiting Polish review            | 19       |
| Tray keys awaiting Polish review               | 6        |
| Keys with a translator comment                 | 471      |
| Keys with English plural forms                 | 27       |
| Keys written as identifiers (`defaultValue:`)  | 53       |
| Permission prompts in `InfoPlist.xcstrings`    | 4        |
| Swift sources in the original census / touched | 128 / 77 |

The license copy cut marks 19 agent-authored Polish entries as `needs_review`.
The Founder-confirmed mode names, license-panel blurb and Remove key label
remain `translated`. Drafts provide coverage,
not evidence of UI review; the review process is in `LOCALIZATION.md` §6.

The tray revision marks six additional Polish entries as `needs_review`: the
stop action, the Agent-mode start/stop actions, the start-in-Agent-mode toggle,
and the copy/save transcript actions. Exact Founder-provided labels are
`translated`. The Settings disclosure contains the seven existing toggles and
ends with Open Settings; its previous standalone row was removed. Action
routing and Notes Mode behavior are unchanged by this menu revision.

The Setup Wizard copy revision removes nine keys the screen no longer shows
(the permission-checklist label, the language-chooser subtitle, the AI
formatting and formatting-level subtitles, the agent-section blurb, the agent
client subtitle, the installation-status button, the Test mic subtitle, and the
placeholder form of the language footnote) and adds five with Founder-provided
Polish: `Recognition language`, `Formatting level`, the footnote
`Domain vocabulary and Dictionary entries improve speech recognition.`,
`Install or update the skill directly from Codescribe.`, and the identifier key
`creator.agentBridge.clientInstalled` (English `Installed`, Polish
`Zainstalowano`) — an explicit key under `LOCALIZATION.md` §R7, because the
shared `Installed` key carries the Whisper-model wording `Zainstalowany`. The
screen reuses the existing `Permissions` and `Refresh status` keys rather than
retranslating them. `Whisper language`, `Auto Format` and `Hotkeys` stay in the
catalog for the runtime rows, the tray and the settings rail. All new Polish
entries are `translated`: the wording is the Founder's own.

A follow-up Founder review of the installed build removed the Creator screen's
collapsed `Details` disclosure — the key itself stays in the catalog, since
`LicensePanel.swift` still uses it — and dropped the two remaining quick-start
card subtitles. `Levels and recognition` and `Start a dictation session` are
removed from the catalog entirely: no Swift source references either any
longer. The same review corrected the Polish value of `Refresh status` from
„Sprawdź ponownie” to „Odśwież stan”.

By area:

| Area                                                                                               | State                                                          | What is left                                                                                           |
| -------------------------------------------------------------------------------------------------- | -------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------ |
| Settings — shell, sections, tabs, view model, services                                             | Swift copy ready                                               | Text from Rust (§4 B3, B8)                                                                             |
| Settings — panels (Providers, Agent, Audio, Shortcuts, Voice Lab, License, User, Prompts, Creator) | Swift copy ready; 13 components take `LocalizedStringKey`      | Status rows, probe and readiness messages come from Rust (§4 B4, B8); number formats (§5.2)            |
| Agent chat — window, composer, palette, message list, thread rail                                  | Swift copy ready                                               | Hand-built unit formats (§5.2); `[error]` markers and tool approval text from Rust (§4 B5)             |
| Dictation overlay                                                                                  | Swift copy ready                                               | Status headline, label and message come from Rust (§4 B8); error causes inside Swift sentences (§4 B3) |
| Tray                                                                                               | Ready, including status phrases (derived in Swift from `kind`) | —                                                                                                      |
| Onboarding                                                                                         | Swift copy ready                                               | Readiness card rows come from Rust (§4 B4)                                                             |
| App shell — menus, About panel, window titles                                                      | Ready                                                          | —                                                                                                      |
| Permission prompts                                                                                 | Ready                                                          | —                                                                                                      |
| Notifications, OAuth landing page, thread export                                                   | Not ready                                                      | Drawn by Rust (§4 B7)                                                                                  |

"Swift copy ready" means: every sentence authored in Swift reaches the catalog
as one key, counted phrases inflect through the catalog, and no displayed value
doubles as identity. It does not mean the screen is fully translatable — where
Rust supplies the text, a translated interface will show English in that slot.

---

## 3. Settings search keywords

Search matches a section or tab by its title and by extra terms. The terms are
two lists (`docs/LOCALIZATION.md` R10):

- **Fixed**, in code, matched in every language: `openai`, `anthropic`, `mcp`,
  `stt`, `asr`, `whisper`, `apple`, `stdio`, `fp16`, `tcc`, `formatting.txt`,
  `assistive.txt`.
- **Localized**, one catalog row per surface — 9 sections
  (`settings.search.section.*`) and 12 tabs (`settings.search.tab.*`) — a
  comma-separated list the translator owns.

The developer-only Lab section keeps a fixed list and has no catalog row.

Two English rows carry a Polish word, as the source did before:
`settings.search.section.audio` has `mikrofon`, `settings.search.section.voiceLab`
has `słownik`. They keep search working for a Polish speaker on the English
interface, and two tests pin them. When Polish ships they go into the Polish
rows too; they also stay in the English row (§5.1).

---

## 4. Rust ↔ Swift boundary

Rust composes user-visible English and hands it across UniFFI as finished
sentences. Swift can show such text only as it arrives. This section lists every
seam found, how large it is, and the contract change recommended for each.

Counts come from a read-only census of the source at `32c79e4a`. "Counted" means
every site was read; "estimated" is marked.

### The rule

**Rust sends a code and arguments; Swift owns the sentence.** Rust has no notion
of an interface language and should not gain one: every seam below is resolved
by moving the wording to Swift, not by teaching Rust to translate.

### B1 — Tray status (done)

Rust used to compose 20 tray strings (`TrayStatus::tooltip()` /
`menu_label()`) and send them in `CsTrayStatusPayload`. The payload already
carried `kind`, `tone`, `indicator_mode` and `assistive`, which determine the
wording completely.

Swift now derives every tray phrase from `(kind, assistive)` in
`TrayStatusStore`. The two string fields and the four Rust methods that filled
them are removed, so the wording has one author. This is the model for the
seams below: the payload says what happened, Swift says it.

### B1a — Shortcuts screen labels (done)

`CsModeBinding.{modeLabel, modeDescription, bindingLabel}`,
`CsBindingOption.label` and `CsHotkeyConflict.gestureLabel` still cross the
bridge, but the Shortcuts and Audio panels no longer show them. Swift derives
the mode name, the mode blurb and every gesture name from the `CsWorkMode` /
`CsShortcutBinding` enums in `Screens/Settings/HotkeysPresentation.swift`
(keys `hotkeys.mode.*`, `hotkeys.binding.*`); a conflict's gesture is mapped
back to its enum through the option list. `CsHotkeyConflict.message` is still
Rust prose (B3 territory). The agent lane is named **Agent** on this screen in
every language (Founder decision 2026-10-04); the Rust `WorkMode::label()`
"Assistive" is wire presentation only.

### B2 — Machine meaning carried inside prose (do this before touching the sentences)

Two messages are parsed by Swift for a code hidden in the English text:

| Marker                                                                           | Producer                            | Swift consumer                                                                                                                  | What depends on it                                            |
| -------------------------------------------------------------------------------- | ----------------------------------- | ------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------- |
| `speech_auth_{not_determined,denied,restricted}:` prefix                         | `core/stt/apple_stt/mod.rs:282-294` | `OverlayState.handleStartFailure`, `OverlayState.speechAuthNotice(from:)`                                                       | Permission retry and which of three notices the overlay shows |
| `CODESCRIBE_RESET_RELAUNCH_REQUIRED`, `CODESCRIBE_AGENT_RESET_RELAUNCH_REQUIRED` | `bridge/src/config.rs:57-81`        | `SettingsViewModel.resetFailureRequiresRelaunch` / `agentResetFailureRequiresRelaunch` (matches on `String(describing: error)`) | Whether the app forces a relaunch after a failed reset        |

The second is control flow, not display. Neither string may be reworded,
reordered or translated while the code travels inside it.

Recommended: give both a typed carrier — a dedicated `CsError` case with the
status as an enum (`SpeechAuthorization { status }`,
`ResetRequiresRelaunch { agent }`) — and have Swift switch on the case. The
display sentence then becomes ordinary Swift copy. Small, but it changes
behaviour-bearing code and needs the integrator's tests.

### B3 — Error messages (`CsError`)

One enum crosses the bridge: `CsError::{Agent, Config, Recording, License, Quality, Runtime} { msg }` (`bridge/src/lib.rs:95`). Swift shows `msg` through
one seam, `Core/CsErrorPresentation.swift` (`userFacingMessage`).

Counted: **122 production construction sites** in `bridge/src` (141 textual
hits minus doc comments, pattern matches and test code).

| File                     | Sites | User-actionable | Internal failure | Pass-through |
| ------------------------ | ----- | --------------- | ---------------- | ------------ |
| `config.rs`              | 39    | 10              | 20               | 9            |
| `hotkeys.rs`             | 38    | 4               | 17               | 17           |
| `recording.rs`           | 17    | 2               | 8                | 7            |
| `agent.rs`               | 9     | 3               | 5                | 1            |
| `quality.rs`             | 9     | 0               | 1                | 8            |
| `lib.rs`                 | 3     | 0               | 0                | 3            |
| `vocabulary_ab.rs`       | 3     | 1               | 2                | 0            |
| `licensing.rs`           | 2     | 0               | 1                | 1            |
| `application_runtime.rs` | 1     | 0               | 1                | 0            |
| `mcp_admin.rs`           | 1     | 0               | 0                | 1            |
| **Total**                | 122   | **20**          | **55**           | **47**       |

- **User-actionable (20)** tell the person what is wrong with their input or
  setup, e.g. "Cloud pass needs a file transcription endpoint (Providers ›
  Speech-to-text)".
- **Internal failure (55)** are lock, IO and invariant failures the person can
  only retry or report, e.g. "account login state lock poisoned" (the same
  literal four times in `config.rs`).
- **Pass-through (47)** are not authored at the bridge at all: they are
  `to_string()` of an `anyhow` chain or of a core type's `Display`
  (`LicenseError`, six sentences in `core/licensing/mod.rs:164-178`;
  `SettingsSnapshotValidationError`; `FormattingPolicy` parsing;
  `AccountAuthError`). Translating "at the bridge" cannot reach them.

Recommended contract:

1. User-actionable errors become cases of a UniFFI enum that carry their
   arguments. Swift maps each case to a sentence in `CsErrorPresentation.swift`,
   with the compiler checking exhaustiveness.
2. Internal failures collapse to one localized sentence per domain ("Couldn't
   save settings."). The English detail stays as diagnostic detail for logs and
   support and is not translated.
3. Pass-through is triaged where the error is born: core error types that
   describe a state the person can act on get typed cases mapped in the bridge;
   `anyhow` chains are internal failures.

This turns 122 sentences into roughly 40 to translate (estimated: the 20
user-actionable ones, 10–15 from core types, and a handful of generic ones).
It is the largest item on this list and can be done one domain at a time;
Recording, Agent and License are the errors people meet in normal use.

Swift-side notes:

- 18 sites read the message through the seam (`userFacingMessage`).
- The overlay builds eleven sentences with `String(describing: error)` as the
  cause (`OverlayState`: start, stop, paste, retranscribe, recover and formatter
  failures). For a `CsError` that prints the Swift enum dump —
  `Agent(msg: "…")` — instead of the message. This predates the localization
  work and was left alone because fixing it changes what those notices say;
  the fix is `error.userFacingMessage` at each site.
- `LicenseService` now owns localized read, verification, activation and removal
  error summaries. The underlying error remains in `lastErrorDetails`, shown
  only in the license panel's expandable Details section. Activation reports
  key-verification failure separately from failure to save the verified key
  on this Mac; checking and saving use separate catches. No bridge error
  prose is parsed to determine the summary.
- `OnboardingViewModel.lastError` receives bridge text at five sites and is read
  by no view. Those messages never reach a person today.

### B4 — Status rows

`CsMcpStatusRow { label, value, tone }` is filled with finished text by
`app/agent/tools/mcp.rs` (about 45 literals and patterns, counted over the whole
file): "no mcp.json (optional — MCP off)", "ready — {count} tool(s) live",
"configured — agent not started yet", the Agentic readiness card
("Provider:", "{} — key missing (set {})", "Workspace roots:", …). Shown in
Settings and Onboarding.

Recommended: the row carries a state enum plus arguments (count, server name,
reason); Swift renders label and value. Product names in labels (Vibecrafted,
AICX, Loctree, PRView) stay verbatim. `"{count} tool(s)"` becomes a proper
plural on the Swift side. One pattern prints a Rust `Debug` dump of two lists
into the row ("mismatch — Settings={:?}, native tools={:?}") and should carry
the lists as data.

### B5 — Agent turn events

| Text                                                                                                             | Producer                          |
| ---------------------------------------------------------------------------------------------------------------- | --------------------------------- |
| "Tool approval is unavailable for voice turn … — allow it in Settings → Permissions to use it hands-free"        | `app/controller/helpers.rs:723`   |
| "Agent runtime unavailable" (context prefix on a pass-through chain)                                             | `app/controller/helpers.rs:994`   |
| "Empty tool output", "{n} image result(s)"                                                                       | `core/agent/session.rs:843-849`   |
| Tool approval summaries: "Edit an existing workspace file", "Start a process inside the configured workspace", … | `app/agent/tools/mcp.rs:913-1010` |

The rest of `on_error` / `on_tool_result` is provider or tool output passed
through and is not copy. Recommended: typed event payloads for the authored
cases, same pattern as B3.

### B6 — Voice Lab acknowledgement toast

`CsQualityCommitResult.acknowledgement` is documented as "ready-to-show overlay
toast text" and built by `acknowledgement_message()`
(`core/quality/overlay_quality.rs:1227-1259`): "Saved as evidence",
"Saved — 1 pair learned", "Saved — {count} pairs learned",
"… — {seen}/{required} manual confirmations", "… — {count} pairs pending (…)".

Recommended: return the numbers (pairs learned, pending, seen, required) and let
Swift compose the sentence with catalog plurals. Rust already hand-rolls the
singular here, which no translation can follow.

### B7 — Surfaces Rust draws itself

These never pass through Swift, so no catalog can reach them.

| Surface              | Where                                                                                                              | Text                                                                                                                      |
| -------------------- | ------------------------------------------------------------------------------------------------------------------ | ------------------------------------------------------------------------------------------------------------------------- |
| System notifications | `app/os/notifications.rs`, called from `bridge/src/hotkeys.rs`, `bridge/src/notes.rs`, `app/controller/helpers.rs` | "Codescribe held the paste", "Nothing to insert", "Couldn't insert the armed transcript", plus passed-through messages    |
| OAuth landing page   | `core/llm/account_auth/server.rs:45-79`                                                                            | One HTML page: "Signed in. Back to Codescribe.", "You can close this window." Other responses are plain-text HTTP errors. |
| Thread export        | `core/agent/thread_export.rs`                                                                                      | Markdown labels: "Untitled thread", "User", "Assistant", "Tool result", " (error)", …                                     |

Recommended: post notifications from Swift in response to a typed event; pass
the three landing-page strings from Swift when a login starts; treat the export
labels as a decision (§5) — an exported file is arguably a document format, not
interface.

### B8 — Other string fields on the bridge

The generated bindings expose 53 `String` fields whose names suggest display
text (label, message, detail, reason, summary, title, …) across about 34
records. Fields that are transcript or thread content are data and are not
copy. The fields that look authored by the program:

`CsAccountLoginResult.message` · `CsAdmissionReadiness.message` ·
`CsAgentAvailability.detail` · `CsAnnotationKind.label` ·
`CsApiKeyProbeResult.message` ·
`CsCapabilityRow.reason` · `CsHotkeyConflict.message` ·
`CsModelDirectory.detail` · `CsModelDiscovery.message` ·
`CsPresentationStatusEvent.{statusLabel, message}` ·
`CsProviderOption.accountStatusMessage` · `CsRuntimeLlmLane.unavailableReason` ·
`CsSttLane.title` · `CsToolApprovalRequest.summary` ·
`CsUnanchoredEvidence.reason`

Rendered today, by screen (established while preparing the Swift side):

| Screen     | Bridge text shown as it arrives                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                     |
| ---------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Settings   | `CsAdmissionReadiness.message` · `CsMcpTestResult.error` · `CsMcpStatusRow.{label, value}` · `CsCapabilityRow.{op, tier, nativeTool, provider}` · `CsSttLane.{title, accepts, placeholder}` · `CsProviderOption.{displayName, accountStatusMessage}` · `CsApiKeyProbeResult.message` · `CsHotkeyConflict.message` · `CsModelDirectory.status` · `CsWhisperModelStatus.sizeHint` · `CsPromptSnapshot.readError` · `CsRuntimeLlmLane.unavailableReason` · `CsModelDiscovery.message` · `CsVoiceLabTeachResult.acknowledgement` · `CsVoiceLabSaveResult.lexiconError` · `CsAccountLoginResult.message` |
| Overlay    | `CsPresentationStatusEvent.{headline, message, statusLabel}` (the status pill, the toast, the error card) · `CsTranscriptProjectionEvent.label` · `CsQualityTeachResult.acknowledgement`                                                                                                                                                                                                                                                                                                                                                                                                            |
| Agent chat | `agent.availability().detail` (shown as the assistant reply) · `speechAvailability()` · `CsAgentListener.onError` and delivery errors (after an `[error]` marker) · `CsToolApprovalRequest.{risk, summary}`                                                                                                                                                                                                                                                                                                                                                                                         |
| Onboarding | `CsMcpStatusRow.{label, value}` · `CsProviderOption.displayName`                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                    |

Vendor and model names in these fields are proper names and need no change.
Two fields are shown with Swift casing applied to Rust text
(`CsCapabilityRow.tier` uppercased in the capability table, a retranscribe pass
name uppercased in Voice Lab), which only reads right while the text is
English.

### Recommended order

1. B2 (typed carriers for the two markers) — unblocks everything that touches
   those sentences.
2. B3 for Recording, Agent and License; then B6 and B4, which are small and
   visible; then the rest of B3.
3. B5, B7, B8 as they are met.

### Not established by the census

- `core/quality/overlay_quality.rs`: about 3000 of 3672 lines were not read;
  the toast inventory in B6 is the verified part, not the whole file.
- The body text of two notification callers (`app/controller/helpers.rs:97`,
  `bridge/src/hotkeys.rs:880`).
- The variants of `AccountAuthError` and their wording.
- Whether thread export Markdown is previewed anywhere in the app or only
  written to disk.

---

## 5. Decisions for a person

Nothing below blocks the foundation. Each item is a choice about wording or
product behaviour that code should not make by itself. The Founder settled the
copy questions on 2026-10-02: a row marked **Decided** records that answer, a
row marked **Open** still waits for one.

### 5.1 Copy

| Item                                              | Where                                                                                                                                                                                                | Decision                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                 |
| ------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Three strings held back from the catalog          | `CreatorPanel` (subtitle of the AI formatting gate row), `VoiceLabPanel` (playback message when the original audio is missing), `SettingsViewModel.formattingDescription` (the "disabled · …" state) | **Decided.** Reworded and wrapped: `Master switch. The Off level below always skips the LLM.`, `Original audio is not available for this correction.`, `disabled · AI formatting off`.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                   |
| Copy authored in capitals                         | Eyebrows and badges in Dictionary, MCP servers, Agent status, Onboarding and the thread rail                                                                                                         | **Decided**, as R8 asks: the keys are sentence case (`Low confidence`, `Unreviewed`, `Changed`, `Corrected original`, `Add server`, `Threads`, `Coding assistants`) and the view applies `.textCase(.uppercase)`. The readiness pills use the existing `Ready` / `Not ready`. The Dictionary card header is `dictionary.correction.header`, because `Correction` also names a formatting level. The three counted headers read `… · %lld characters` and inflect.                                                                                                                                                                                                                                                                        |
| One key, several meanings                         | Keys `l10n-sync` reports as used with different comments                                                                                                                                             | **Decided.** Split where the uses need different words: `Off` → `settings.formatting.level.off`, `settings.holdBadge.size.off`, `settings.paste.policy.off`, `settings.previewTiming.preset.off`, `tray.keycap.off` (the bare `Off` is the VoiceOver toggle state); `On` → `tray.keycap.on`; `Allow` → `audio.microphone.permission.allow` (the bare key is the tool permission level). Kept shared, one word serves every use: `Agent`, `Assistive`, `Cloud` (the engine or mode name), `Correction`, `English`, `Polish`, `Expanded`, `Hotkeys`, `Max`, `Smart`, `New thread`, `Ready`, `Recording`, `copy`, `ended`, `error`, `failed`, `granted`, `unset`. A translation that needs two words for one of them asks for a split (R7). |
| Single words with no room for a comment           | `Teach`, `Local` / `Cloud`, `Anchor`, `Open` / `Collapsed`, `Back`, `Copied`                                                                                                                         | **Decided.** The `Teach` field prompt reads `Correct spelling` (the button keeps `Teach`); the engine buttons under "Transcribe again" are `overlay.retranscribe.local` / `overlay.retranscribe.cloud`; `Anchor` carries a comment; the overlay handle reports `Expanded` / `Collapsed`; `Back` and `Copied` stay shared.                                                                                                                                                                                                                                                                                                                                                                                                                |
| Five long near-identical sentences                | `OverlayCoverageStatus.unavailableSentence`                                                                                                                                                          | **Decided:** kept. Each reason stays a whole sentence (R5).                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                              |
| Lane reset notice                                 | `SettingsViewModel.removeCustomProvider`                                                                                                                                                             | **Decided:** kept as two whole keys, singular and plural.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                |
| Polish words in English search rows               | §3                                                                                                                                                                                                   | **Decided:** `mikrofon` and `słownik` stay in the English row.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                           |
| Integrity diagnostics inside a localized sentence | `AgentBridgeInstallationError.invalidManifest` reasons ("checksum mismatch for …", "symlink refused at …")                                                                                           | **Decided:** English in the first localized release. Shown only for a damaged bundle.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                    |
| Launch synchronization reasons                    | `RealAgentBridgeInstaller.requireSynchronizationOwnership` and the receipt check in `install` (seven reasons, e.g. "the receipt is not an ordinary managed file")                                    | **Decided:** English, as diagnostics — the startup synchronization result is only logged. Wrap them if that result is ever shown.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                        |
| `%@ tabs`                                         | Tab bar picker label, read by VoiceOver only                                                                                                                                                         | Fine as a key; listed because it is assembled from a section title.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                      |

### 5.2 Dates, numbers, units (R8)

Hand-built formats found. The thread rail dates follow the locale since
2026-10-02; the rest is accepted debt for the first localized release and
shows the same in every language until it moves to a format style.

| Where                                                   | What                                                                                                                                                                                                                             |
| ------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `ThreadRail`                                            | Dates follow the locale: month and day through the `MMMd` template, `yesterday` and `today %@` from the catalog. Left: the time of day inside `today %@` is `HH:mm`, and the section header capitalizes the phrase by hand       |
| `AgentChatStore`, `RealThreadsEngine`, `RealTrayEngine` | `HH:mm` timestamps (24-hour everywhere)                                                                                                                                                                                          |
| `ThreadRail`                                            | `tok`, `k tok`, `M tok` glued to hand-built numbers                                                                                                                                                                              |
| `AgentChatStore.durationLabel`                          | `ms`, `s` (pinned by `ChatRenderCostTests`)                                                                                                                                                                                      |
| `OversizedText`                                         | `KB`, `MB` — should be a byte-count style                                                                                                                                                                                        |
| `MessageList`                                           | zoom percentage; `worked · %@s` with a pre-formatted number and a glued unit                                                                                                                                                     |
| `OverlayState.sessionTimerText`                         | clock built with `%d:%02d:%02d`                                                                                                                                                                                                  |
| `OverlayEvidenceList`                                   | a bare count                                                                                                                                                                                                                     |
| `VoiceLabPanel`                                         | `logprob %.2f`, `speech %.0f%%`                                                                                                                                                                                                  |
| `OverlayCoverageStatus`                                 | uncovered-speech chip: duration as `%.1f` under `en_US_POSIX` with a glued `s` (pre-formatted, so the decimal point is fixed); positions and ranges as hand-built `m:ss` joined by `, ` (pinned by `OverlayRecordingLightTests`) |
| `DictationPreviewTimingTab`                             | readout `ms · cps · words · s interim`                                                                                                                                                                                           |

### 5.3 Product

| Item                             | State                                                                                                                                                                                                                 | Decision                                                                                                                                                                                                                                      |
| -------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Reset confirmation words         | `RESET` and `RESET AGENT` are constants compared by code and passed into the sentences as arguments                                                                                                                   | **Decided:** the same token in every language; the sentences around them are localized.                                                                                                                                                       |
| Callout headers                  | `[!NOTE]`, `[!TIP]`, `[!IMPORTANT]`, `[!WARNING]`, `[!CAUTION]` in model output are parsed as written                                                                                                                 | **Decided:** the markers stay; the visible header is copy (`chat.callout.note` … `chat.callout.caution`), uppercased by the view.                                                                                                             |
| Copied technical receipt         | Field names and status words stay English                                                                                                                                                                             | **Decided:** English in the first localized release.                                                                                                                                                                                          |
| Thread export                    | Labels are written by Rust (§4 B7)                                                                                                                                                                                    | **Decided:** English in the first localized release.                                                                                                                                                                                          |
| About panel and build info       | `dev`, `unknown` when build metadata is absent                                                                                                                                                                        | **Decided:** English, as diagnostics.                                                                                                                                                                                                         |
| Voice Lab source line            | Shows a raw producer id for engines other than Whisper and cloud                                                                                                                                                      | **Open.** Map every producer, or show nothing.                                                                                                                                                                                                |
| Protocol names                   | `Responses`, `Messages` in the provider editor are wire names                                                                                                                                                         | **Decided:** verbatim.                                                                                                                                                                                                                        |
| Active STT row                   | `Apple` and `Whisper` are proper names and stay verbatim; `Streaming Whisper`, `Cloud`, `Whisper (fallback)` and `Not yet served` are copy. An engine id the app does not know is shown as received                   | **Open.** Map every id, or show `Unknown` for the rest.                                                                                                                                                                                       |
| Developer-only copy              | The Lab pane, its section title, the `Voice Lab…` tray row and the power-mode corner mark exist only on a developer build; `DesignGallery` is reachable only from its preview. Their copy is extracted like any other | **Decided:** these 22 keys are marked `shouldTranslate: false` (13 used only by `LabPanel`, 3 shown only behind `DeveloperSurface`, 6 gallery samples), so no translator sees them. A key that a shipped screen starts to use loses the mark. |
| Coverage before a language ships | `make verify-l10n-catalog` fails a language that is partly translated in either catalog; `--allow-partial` reports instead while a language is built up on a branch                                                   | **Done** with the Polish import: every language the bundle carries must be complete, and `CodeScribe` is refused in any string. New English copy now needs its Polish before `make check` passes.                                             |

### 5.4 Found on the way (not localization, not changed)

- Seven controls have an empty label (`Toggle("")`, `Picker("")`,
  `TextField("")` in Creator, Audio, User, Agent lanes, Thread rail). They have
  no accessible name; the empty key is in the catalog marked do-not-translate.
- Copy with no reader: `OnboardingViewModel.lastError`,
  `TrayViewModel.statusText`, `OverlayState.engineChip`, and the palette
  failure line in agent chat (appended as a tool message whose text the bubble
  never renders).
- Ten constants resolve their copy once per launch (`CloudPrivacyCopy`, the
  empty-thread headline and detail, the default no-speech notice). That is
  correct while a language change relaunches the app, which is how macOS
  applies a per-app language; it would need revisiting for a live switch.
- Default thread titles (`New thread`, `Voice chat`) are display values. No
  path sends them across the bridge today; a path that persisted a title
  would have to persist the user's or the generated one, never the default.
- `ThreadRailMeta.timeOnly(from:)` splits a display string on `·` to recover
  its first part; `ToolLine.detail` holds either a tool name or an authored
  placeholder. Both are identity and display in one slot.

---

## 6. Polish originals found in the source

English is the source language, so these were replaced with English. The
Polish is recorded here for the Polish Handbook.

| Where                                        | Polish original                          | English now                          |
| -------------------------------------------- | ---------------------------------------- | ------------------------------------ |
| Composer palette, model picker title         | `Wybierz model asystenta`                | `Choose the assistant model`         |
| Composer palette, grants title               | `Narzędzia z „zawsze zezwalaj”`          | `Tools with “always allow”`          |
| Composer palette, grants read failure        | `Nie udało się odczytać uprawnień`       | `Couldn't read tool grants`          |
| Composer palette, grants read failure detail | `sprawdź ~/.codescribe/tool_grants.json` | `Check %@` (the path is an argument) |
| Composer palette, grant row                  | `nadane … · wybierz, aby cofnąć`         | `granted %@ · select to revoke`      |
| Agent chat, palette failure                  | `Nie udało się zastosować „…”: …`        | `Couldn't apply “%@”: %@`            |
| Composer, empty palette                      | `Brak pozycji`                           | `No matches`                         |
| Composer, active entry marker                | `aktywny`                                | `active`                             |
| Settings search                              | `mikrofon`, `słownik`                    | unchanged (§3)                       |

---

## 7. Layout that will meet longer text

No layout was changed. These are the slots where a translation 15–30 % longer
than English clips or truncates first.

| Surface                          | Slot                                                                                                                         |
| -------------------------------- | ---------------------------------------------------------------------------------------------------------------------------- |
| Settings tab bar                 | Segmented control sized to its widest segment, up to six segments at the 880 pt minimum window                               |
| Settings sidebar                 | 196–300 pt; health footer is two lines in 196 pt                                                                             |
| Tool permissions                 | `Allow · Ask · Deny` in a picker pinned to 180 pt — the tightest slot in Settings                                            |
| Agent status                     | Label column 160 pt, capability columns 120 / 96 pt, one-line rows                                                           |
| Providers, MCP servers, key rows | One-line status chips and rows; editor sheet 480 pt                                                                          |
| Shortcuts, Creator, Audio        | Pickers pinned to 230–330 pt; a 92 pt readout                                                                                |
| Tray                             | Panel is 300 pt; status pills are one line and fixed-size; `Status: %@` and banners are one line                             |
| Onboarding                       | Welcome cards `minHeight` 135; readiness label column 150 pt                                                                 |
| Overlay                          | Footer notice is one line (the tightest slot in the app); coverage chip one line; popovers 250–300 pt; minimum window 320 pt |
| Agent chat                       | Thread title is one line beside the status pill; inspect labels in a 64 pt column; collapsed rail section titles             |

---

## 8. What changed besides the wrapping

Most of the diff wraps existing English and changes nothing a person sees.
These are the exceptions.

English text:

- Counted phrases inflect: `1 server`, `1 tool`, `1 turn`, `1 attachment`,
  `1 day left`, `1 occurrence`, `1 variant`, `1 correction on disk`,
  `1 model discovered`, `1 live rule`, `1 char`, `1 unchanged take`,
  `1 store row`, `1 recording` / `1 day` / `1 thread`
  in the reset summary. Some of these read `1 servers` before.
- Counts inside sentences are grouped for the locale: `5,000 recordings`.
- The two multi-count Voice Lab summaries have independent plural substitutions
  for every count. English needs singular nouns there too; the earlier claim
  that those sentences read the same for one and many was incorrect.
- Archive character counts keep their integer for plural selection and honor
  the supplied locale, including the existing four-digit grouping. The
  replacement attribute identifies the plural phrase, including its noun; only
  the locale-formatted number inside it is regrouped. Noun forms and word order
  stay in the catalog. Regression cases cover English
  and Polish grouping across 999/1,000 and 9,999/10,000, and up to one million.
- The helper file-pass comparison uppercases its display name with the locale.
- The eight Polish strings in §6 are English.
- Source copy reworded on 2026-10-02 (§5.1): the AI formatting gate subtitle,
  its disabled state and the missing-audio playback message; the Dictionary
  text headers count `characters` (`1 character`) instead of `CHARS`; the
  correction field prompt reads `Correct spelling`; the provider key row of
  the setup summary reads `set` where it read `granted`.
- Thread rail rows show the month and day in the locale's order and names;
  English reads as before (`Jul 8`).
- The prompt restore tooltip no longer lowercases the prompt title.
- The lane reset notice joins lane names with the locale's list format.
- Four error notices show the bridge message instead of an enum dump: the
  two Max approval failures, the consultation notice, and the channel toggle
  failure now read the error through `userFacingMessage`.

VoiceOver only:

- Eyebrow labels are uppercased by the view (`.textCase(.uppercase)`), so
  they look the same and are read in their authored case.
- A prompt with no path is announced as `Path unavailable` (was lower case).
- Labels that were authored in capitals (§5.1) and the callout headers in
  agent chat are now read in sentence case; they look the same.
- The overlay transcript handle reports `Expanded` where it reported `Open`.

Identity and ownership:

- `HoldBadgeOption.id` is derived from the size, not from the visible name.
  It is used only as a list identity; the stored setting is unchanged.
- Tray wording is authored in Swift; the Rust payload no longer carries it
  (§4 B1).
- The keychain status code is interpolated as text, so it is not grouped like
  a count.

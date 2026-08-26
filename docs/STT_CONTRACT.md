# STT contract — front → backend (no lottery)

> Status: operator truth map · 2026-07-24 · branch `feat/operator-feedback-wave9`
> Goal: every UI/hotkey entry is one line to one handler; settings truth is one place.
> **Operator lock: Apple STT is MUST-HAVE for live.** Whisper is never the primary live engine.
> **Superseded on Whisper's role (2026-07-26, `AGENTS.md` — THE ONE RULE):** the target shape is
> Whisper transcribing **partials on the go** to fill canvas gaps — NOT final-pass-only.
> Lexicon substitution is the FINAL automated layer, after Whisper.
>
> **Status (2026-08-25):** `StreamingRecorder` is the sole allocator of live
> capture epochs: a checked next value is committed only after the device opens,
> and a new operator-session bind resets the counter. The Apple progressive path
> receives that explicit epoch. `AcousticLedger` alone qualifies occurrences,
> admits observations, refuses structural replay, and seals; equal text is never
> occurrence identity. `PresentationEmitter` / `TranscriptReducer` reduce the
> resulting ledger events before Transcript Bus and Swift observe them.
> In-process, sidecar, and remote tail providers share that identity seam. The
> VAD/scheduler identity cone and file-tail text-overlap compatibility cone are
> removed. Offline one-file replay seams use caller-domain epoch `1`. Legacy
> `FINAL_PASS_MODE` no longer owns any normal-stop inference.
> C11 makes `publish_revision` the sole committed Bus writer: raw final,
> correction, replacement, annotation, and preview events cannot write product
> text. A terminal ledger seal closes Bus truth; compiler/runtime behavior is
> `NOT_ASSESSED` in this structural cut.
> Planning report: internal plan `stt-apple-must-have` (operator artifact store, 2026-07-24).

---

## 0. Your machine right now (why it failed) — _historical diagnosis_

| Layer                                          | What you had                | Effect                             |
| ---------------------------------------------- | --------------------------- | ---------------------------------- |
| `settings.json` → `speech.engine`              | **`{}` empty**              | No durable `stt_engine`            |
| `~/.codescribe/.env` → `CODESCRIBE_STT_ENGINE` | **`auto`**                  | Env **won** over empty settings    |
| Runtime `selected_engine()`                    | `auto` → Apple if bridge OK | Live STT = Apple                   |
| Live path                                      | `run_apple_live_only`       | **No Whisper mid-live** on failure |

```text
Apple STT live path failed … (Candle Whisper fallback disabled for live)
Recording stopped before a transcript was available.
```

**Not rocket science:** auto picked Apple; Apple failed mid-take; live refuses Whisper.

### Fixed product contract (2026-07-24 ship cut)

| Layer                 | Rule                                                                                                     |
| --------------------- | -------------------------------------------------------------------------------------------------------- |
| Empty `speech.engine` | Load defaults to **`stt_engine=apple`** and **`final_pass_mode=smart`**; explicit saved values still win |
| Settings UI write     | **Promoted** to `settings.json` + reconciles process env **and** `.env` (single brain)                   |
| Record start          | **`preflight_apple_live_ready()`** when engine is Apple — refuse before REC if Speech/bridge not ready   |
| Live vs final         | Cloud/Apple-only live fails closed without local weights; explicit HQ/local Retranscribe may use Whisper |

---

## 1. What `settings.json` should contain (STT-relevant)

**Path (only this file for UI-promoted settings):**
`~/Library/Application Support/Codescribe/settings.json`

**Schema v3 — speech.engine keys that actually matter:**

| JSON path                       | Internal field          | Wire / env              | Values                                               | Required for “simple works”? |
| ------------------------------- | ----------------------- | ----------------------- | ---------------------------------------------------- | ---------------------------- |
| `speech.language`               | `whisper_language`      | `WHISPER_LANGUAGE`      | `pl`, `en`, …                                        | Yes (you have `pl` ✓)        |
| `speech.engine.stt_engine`      | `stt_engine`            | `CODESCRIBE_STT_ENGINE` | `auto` \| `apple` \| `whisper` \| `candle` \| `onnx` | **Yes — pick explicit**      |
| `speech.engine.final_pass_mode` | `final_pass_mode`       | `FINAL_PASS_MODE`       | legacy migration token                               | No                           |
| `speech.engine.whisper_model`   | `whisper_model`         | `WHISPER_MODEL`         | model id                                             | If engine = whisper          |
| `speech.engine.mode`            | maps to `use_local_stt` | legacy                  | `local_whisper` / `cloud_whisper`                    | Optional legacy              |
| `speech.engine.local_model`     | `local_model`           | path                    | model path                                           | Optional                     |
| `speech.formatting.level`       | `formatting_level`      | —                       | `off`/`correction`/`smart`/`max`                     | AI format (not STT)          |
| `speech.emission.*`             | buffer/typing           | Voice Lab               | numbers                                              | Overlay pacing only          |

STT authentication follows endpoint ownership: `api.openai.com` and
`api.libraxis.cloud` use `Authorization: Bearer`; loopback servers require no
API key; remaining custom endpoints retain the `x-api-key` contract. The key
probe, live socket handshake, and explicit file-pass path use the same
resolver, so Settings cannot disagree with delivery. Settings → Test is the
multipart file probe (`/v1/audio/transcriptions`) for every OpenAI-compatible
host. A stored `wss`/`ws` `…/transcribe` URL is remapped to that file path
first; loopback Voice Lab `:8446` becomes `:8444`. It is not a WebSocket
handshake. The inverse is also explicit: a loopback file URL on `:8444`
(`http(s)://…/v1/audio/transcriptions`) becomes the live socket on `:8446`.
A generic loopback file URL on another port keeps that port.

Transport ownership is equally explicit. Live capture uses a stored Voice Lab
WebSocket (`config` → bounded PCM `chunk` → periodic `flush` → `end`) and
streams its normalized events into `PresentationEmitter`. A public HTTPS
`/v1/audio/transcriptions` URL — OpenAI or Libraxis — is file, not a silent
socket. A complete audio-file multipart request is allowed for Settings → Test
and for an explicit file action (Dictionary or Teacher).

**Domain token (client-owned, 2026-08-18).** Codescribe names the take
`vocabulary=programming` on loopback and Libraxis file/live requests
(multipart field `vocabulary`; JSON alias `request_vocabulary`; live
`session.start` / WS `config`). Official OpenAI file audio omits the field.
Absence means no dictionary bias. The client never classifies audio to pick
`programming` vs another domain. A quality bench that must stay unbiased
sends `off` explicitly — omitting the field is not a silent product default.

**Explicit file passes (2026-08-26).** Dictionary `cloud:` and Teacher uploads
to remapped loopback `:8444` (`/v1/audio/transcriptions`) are product file
takes. They attach `vocabulary=programming` so Polish+tech speech can prefer
`Rust` over `raz`. Dictionary binds the archived row audio; it never invents
`last_session.wav`. Voice Lab and CLI file passes remain diagnostic surfaces.
The daily Overlay has no full-file transcription action: it renders Bus
projections and explicit human edits, never raw `transcribeFile` output.

**Legacy Overlay Format is removed (2026-08-25).** The former raw LLM
replacement / delivery-style path no longer exists. Automatic formatting is
only the occurrence-bound observer described below; HQ compare remains Whisper
file vs raw Apple, never vs formatted text.

**Dictionary helper (everyone, 2026-08-17):** Settings → Dictionary Retranscribe
is an explicit file surface on the row's archived `<stem>_raw.{m4a,wav,flac}`.
Helper engine follows `speech.engine.asr_mode`: `local_power` → `hq:` (same
candle file pass as `codescribe transcribe --raw`); `cloud` →
`cloud:` file upload. `apple_only` has no helper. The daily transcript is not
overwritten until the user saves a correction. Missing archive must refuse —
never fall back to `last_session.wav`. Lab three-judge / `:8444` is not this
button.

**Yours today:**

```json
"speech": {
  "language": "pl",
  "engine": {},          // ← EMPTY = no choice recorded
  "formatting": { "enabled": true, "level": "smart", ... },
  "emission": { ... }
}
```

**Minimal durable engine block (Apple-first daily driver):**

```json
"speech": {
  "language": "pl",
  "engine": {
    "stt_engine": "apple",
    "whisper_model": "whisper-large-v3-turbo",
    "final_pass_mode": "smart"
  },
  "formatting": { "enabled": true, "level": "smart" }
}
```

**And** either:

1. Remove `CODESCRIBE_STT_ENGINE=auto` from `~/.codescribe/.env`, **or**
2. Set `CODESCRIBE_STT_ENGINE=apple` in `.env` (env always wins if present).

If you keep `.env = auto` and only fill `settings.json`, **settings do not win** for engine selection at runtime.

Empty recordings with Apple selected are a **reliability cut** (preflight + typed Whisper _recovery_ with audio), not a reason to abandon Apple as primary.

---

## 2. Precedence (one rule)

```text
1. Process env (set at boot from .env load, OR reconciled on Settings write)
2. Else settings.json seeds process env once (loader.rs apply_user_settings)
3. Else built-in default:
     auto → Apple if bridge resolvable else Candle Whisper
     empty settings.stt_engine → product default **apple**
```

Code: `core/config/loader.rs` · `core/stt/mod.rs::selected_engine()` · `reconcile_stt_runtime_key`.

**Single brain (W2-A):**
`CODESCRIBE_STT_ENGINE` and `FINAL_PASS_MODE` are **promoted** settings. UI write updates `settings.json`, process env, and `.env` together. No silent dual brain.

`CODESCRIBE_LAYERED_TRANSCRIPTION` is promoted single-brain configuration;
`CODESCRIBE_STT_INITIAL_PROMPT_ENABLED` remains env-seedable when unset.

> **Historical power-user hazard (measured 2026-08-08, now closed).** Before promotion,
> `Config::inject_file_env_for_runtime` copied `CODESCRIBE_LAYERED_TRANSCRIPTION` out of
> `~/.codescribe/.env` into the process env on the first `Config::load()` — in _every_ process
> that loads the core, tests and harnesses included. A stale `.env` line therefore arms Layer 1
> silently. This was observed live: the same `make test-engine-parity` binary scored 0.931 with
> the lane off and 0.833 with the operator's dotenv arming `phase1`, and the low score was the
> _more accurate_ transcript. The parity target now pins the lane explicitly (`Makefile`), but
> the general hazard affected any tool loading the core. The key is now
> promoted to settings.json; parity harnesses still pin their requested lane.

**Final pass vs layered (orthogonal):**

| Setting               | Env                                | Default    | Role                                                                                                                                 |
| --------------------- | ---------------------------------- | ---------- | ------------------------------------------------------------------------------------------------------------------------------------ |
| Final pass            | `FINAL_PASS_MODE`                  | legacy     | No effect on normal stop; retained only for settings migration; explicit non-Overlay file surfaces own whole-file inference          |
| Layered compatibility | `CODESCRIBE_LAYERED_TRANSCRIPTION` | mode-owned | Local Power + Apple/Auto: unset or `phase1` arms; explicit off/invalid degrades. No parallel VAD/scheduler live route remains |

Normal capture ignores legacy final-pass routing and never decodes/uploads the
completed WAV. Layered phase tokens (`phase1`…) select live refinement;
whole-file inference stays outside the daily Overlay on explicit file surfaces.

---

## 3. Front entry → backend handler (STT spine)

### 3.1 Hotkeys (start/stop recording)

| Front entry                     | Binding (your settings)             | Bridge / OS                      | Controller                                                 | Backend                                      |
| ------------------------------- | ----------------------------------- | -------------------------------- | ---------------------------------------------------------- | -------------------------------------------- |
| Hold Fn (dictation)             | `mode_bindings.dictation = hold_fn` | `CodescribeHotkeys` + CGEventTap | `RecordingController::handle_hotkey_event` → `handle_hold_event` | recorder + streaming session + `core/stt::*` |
| Double Left Option (formatting) | `formatting = double_left_option`   | same                             | hold/toggle + force AI format path                         | STT same, then `core/llm` formatting         |
| Double Right Option (assistive) | `assistive = double_right_option`   | same                             | assistive session                                          | STT same, then agent lane                    |

**Stop** drains the live recorder/session and delivers its committed transcript
(paste / overlay / agent). It does not upload the completed WAV. Whole-file
file-pass belongs only to explicit non-Overlay file surfaces.

### 3.2 Settings UI → config

| Front control                   | UniFFI                                              | Core                                                                   |
| ------------------------------- | --------------------------------------------------- | ---------------------------------------------------------------------- |
| Load Settings form              | `CodescribeConfig.load_settings()`                  | one `RuntimeSettingsSnapshot` → `CsSettings::from_runtime_snapshot`    |
| Save knobs                      | `update_config` / `update_config_many`              | `UserSettings::set_*` → write `settings.json`; may seed env            |
| ASR mode picker                 | `CODESCRIBE_ASR_MODE` + `CODESCRIBE_CLOUD_CONSENT`  | Cloud never displays without `granted`; stop ignores `FINAL_PASS_MODE` |
| Active STT row                  | `current_serving_verdict()`                         | last live take (`local_apple` → Apple). No Smart-final-pass suffix     |
| Whisper model status / download | `whisper_model_status` / `download_whisper_model`   | `core/config/models.rs`                                                |
| Audio device                    | `audio_input_snapshot` + config keys                | `UserSettings.audio_input_device` + cpal                               |
| Mic permission                  | `mic_permission_granted` / `request_mic_permission` | `app/os/permissions`                                                   |
| Lane (LLM) truth                | runtime snapshot projection                         | `RuntimeSettingsSnapshot::llm_lanes()` → `RuntimeLlmLanes`             |
| AI execution generation         | next selected runtime snapshot                      | sealed prompts + retry/delay + shared Agent/formatter request timing   |

### 3.3 Dictation overlay / tray

| Front                         | UniFFI                                             | Handler                                                        |
| ----------------------------- | -------------------------------------------------- | -------------------------------------------------------------- |
| Committed transcript truth    | `CsTranscriptProjectionEvent`                     | ledger receipt → reducer → Transcript Bus → listener projection |
| Ephemeral/raw observations    | remaining `CsTranscriptionListener` text callbacks | preview paint or diagnostics only; never delivery writers       |
| PCM sideband evidence         | `EngineEventWire::SidebandEvidence`                | Silero ingress → IPC → bridge diagnostic; reducer no-op          |
| Recording service object      | `CodescribeHotkeys`                                | shared controller recording API                                |
| Tray status glyphs            | `CodescribeTrayStatus` + listener                  | controller tray payload                                        |
| Auto-paste / auto-format tray | `set_auto_paste_enabled` / `set_auto_format_level` | `UserSettings` + live toggles                                  |

### 3.4 STT engine dispatch (the nit)

| Call site             | When             | Function / transport                                             | Engine rule                                                           |
| --------------------- | ---------------- | ---------------------------------------------------------------- | --------------------------------------------------------------------- |
| Live Layer 0          | during recording | Apple progressive                                                | first PCM-pinned observation; text is revisable by same-span evidence |
| Live Layer 1 typed    | during recording | in-process / sidecar / remote tail provider on ~5 Apple segments | exact-PCM outcome rewrites pending baseline before final              |
| Live Layer 1 unbound  | after recording  | full-session Voice Lab WSS candidate                             | evidence only; typed refusal if it proposes mutation                  |
| Live capture epoch    | device open/reopen | `StreamingRecorder` checked session-local counter              | issued only after successful open; engines only observe it            |
| Explicit Retranscribe | operator action  | local completed-file decode or cloud multipart                   | may replace the selected artifact, never the live canvas              |

Physical occurrence identity is exactly
`(session, capture_epoch, sample_start, sample_end)`. Observation identity adds
producer, request, and generation. Provider-specific request metadata may be
richer, but it cannot mint a second physical occurrence. The request range must
contain the target span and any provider payload must echo its admitted PCM
identity. `AcousticLedger::admit` records the decision and
`AcousticLedger::seal` closes the occurrence; `EngineEvent::LedgerMutation` and
`EngineEvent::LedgerSeal` carry those receipts into
`PresentationEmitter` / `TranscriptReducer`, which alone commit the document
projection. Replayed observation identity, invalid ranges, missing identities,
and late automatic completions are refused structurally. Identical words in
disjoint PCM ranges remain distinct occurrences and survive.

Automatic formatting is one occurrence-bound observer on the Apple live path,
not a second transcript pass. It is scheduled only after a bounded execution
permit owns the exact existing `(session, capture_epoch, sample_start,
sample_end)` and every earlier scheduled automatic observer for that occurrence
has returned. Its sole product route is `OccurrenceLabelProposal` ->
`EngineEvent::OccurrenceLabelProposal` -> `PresentationEmitter` /
`TranscriptReducer` -> `AcousticLedger::admit(Formatter)`. Applied rewrites
propose a label; healthy no-ops and intentional skips preserve; provider or
structural failures refuse. Every accepted job returns its exact frontier slot
before occurrence and terminal sealing; settings or lane availability alone do
not schedule Formatter.

`UtteranceFinal` is raw observation/telemetry only. Committed phrase identity
travels through `LedgerMutation` / `LedgerSeal` receipts and the occurrence-
keyed reducer into `TranscriptBus::publish_revision`. Phrase timing remains
`phrase`; the system never divides provider segment time evenly into invented
word pins. The ledger projection orders `(capture_epoch, sample_start,
sample_end)` lexicographically and rejects loss, addition, or reorder before
delivery. Preview is overlay-only and is discarded at terminal boundaries.

Active W2-04 Agent leases are read directly as a bounded 120-second snapshot.
Their names are placed first in the existing Whisper context budget and
canonicalized only by exact whole-word matching in Lexicon/Light+. Stale,
malformed, unknown, or colliding leases fail open. There is no phonetic/fuzzy
rewrite: active `Iwo` does not rewrite Polish `piwo`.

Ordinary overlay TextEditor edits carry `edit_provenance=manual_human`
separately from delivery `action`. The latch is consumed by one quality commit;
three distinct correction IDs for the same normalized lexical pair expose
`1/3`, `2/3`, `3/3` and promote exactly once. Formatter, machine file passes, replay,
bulk, speech-gap, and delivery actions without that latch cast no vote.

Dictionary **Teach** is a separate, explicit bulk-promotion command: it mines
eligible correction-store and proposed rows immediately and therefore bypasses
the automatic three-human-correction threshold. The UI must say that plainly;
running Teach is operator authorization, not passive learning.

**Runtime proof:** Settings may show configured readiness, but a take counts as
exercised only when its typed receipt says `armed=true` and `submitted>0`.

```text
selected_engine()
  ├── Onnx   → onnx_adapter
  ├── Apple  → LIVE: run_apple_live_only  |  ADAPTER: run_apple_or_whisper
  └── Candle → whisper singleton (label: local_whisper)
```

`auto` = Apple if `apple_stt::is_runtime_available() && is_bridge_resolvable()` else Candle.

---

## 4. Labels vs truth

| Surface                              | Source of truth                             | Not truth          |
| ------------------------------------ | ------------------------------------------- | ------------------ |
| Settings **preference** `stt_engine` | `CsSettings.stt_engine` (env-merged)        | —                  |
| Settings **Active STT**              | `current_serving_verdict().engine` last run | Preference string  |
| Overlay footer engine chip           | last verdict / controller truth label       | “I wanted Whisper” |
| Error text                           | actual failing path                         | —                  |

Valid engine labels on verdict: `local_apple`, `local_whisper`, `streaming_whisper`, `cloud_stt`.

---

## 5. Operator cheat-sheet — make it boring

### Want Apple live (product default — must-have)

1. Settings + env both pin Apple (no empty `engine: {}`, no silent `auto` fight):
   ```bash
   CODESCRIBE_STT_ENGINE=apple
   ```
   ```json
   "engine": {
     "stt_engine": "apple",
     "whisper_model": "whisper-large-v3-turbo",
     "final_pass_mode": "off"
   }
   ```
2. Full quit + relaunch.
3. Footer / Active STT after a take: **`local_apple`** on happy path.
4. Local Power arms bounded Whisper Layer 1 by mode when the model is ready;
   Apple-only does not. An explicit global `off` is a degraded override.
5. Empty death mid-take = **code cut** (preflight + recovery when audio exists).
   Settings alone cannot repair a broken live session.

### Want Whisper-only (power user / offline — not product default)

Same pattern with `stt_engine: "whisper"`. Allowed; not the Codescribe daily-driver story while Apple is must-have.

### Do **not** leave

```json
"engine": {}
```

plus

```bash
CODESCRIBE_STT_ENGINE=auto
```

unless you accept Apple lottery on every session.

---

## 6. Full front surface map (non-STT, for completeness)

| Domain                        | Front / UniFFI                 | Backend owner                          |
| ----------------------------- | ------------------------------ | -------------------------------------- |
| Config / keys                 | `CodescribeConfig`             | `core/config/*`, Keychain              |
| Hotkeys                       | `CodescribeHotkeys`            | `app/os/hotkeys`, controller           |
| Recording / STT               | `CodescribeHotkeys`, listeners | controller + `core/stt` + `core/audio` |
| Agent chat                    | `CodescribeAgent`              | `core/agent/*`, LLM lane               |
| Agent delivery (voice→thread) | `CsAgentDeliveryListener`      | `ThreadDeliveryGateway`                |
| Threads                       | `CodescribeThreads`            | `core/agent/thread_*`                  |
| MCP                           | `CodescribeMcpAdmin`           | `core/mcp`                             |
| Quality / lexicon             | `quality_*`, lexicon FFI       | `core/quality`                         |
| Notes                         | `CodescribeNotes`              | notes store                            |
| Tray                          | `CodescribeTrayStatus`         | controller tray                        |

---

## 7. Falsification premises

1. **Lie:** “settings.json alone sets the engine.”
   **Truth:** process env / `.env` wins when set.
2. **Lie:** “footer local whisper means live is Whisper.”
   **Truth:** last verdict or preference can diverge from live Apple.
3. **Lie:** “Apple fails so Whisper should catch it live.”
   **Truth:** live Apple path explicitly disables Candle fallback (`22305e26`).
4. **Lie:** empty `speech.engine` is fine.
   **Truth:** empty = no durable preference; `auto` decides every boot.

---

## 8. Cuts landed (this ship)

| Cut                                                          | Status               | Where                                                  |
| ------------------------------------------------------------ | -------------------- | ------------------------------------------------------ |
| P0 Promote STT engine to settings + reconcile `.env`/process | **landed**           | `PROMOTED_SETTINGS_KEYS` + `reconcile_stt_runtime_key` |
| P0 Default empty engine → apple + smart final                | **landed**           | `UserSettings::from_v2`                                |
| P1 Preflight Apple before hold/toggle start                  | **landed**           | `preflight_apple_live_ready` + controller start        |
| P1 Settings truth note (pref vs last Active STT)             | **landed**           | `sttEngineTruthNote` + Engine panel                    |
| P1 Mid-live Apple fail → live Layer 1 recovery               | **open** (next wave) | recover inside session; no stop-path file decode       |
| Operator machine pin                                         | **done**             | `settings.json` + `.env` → `apple`                     |

---

_Vibecrafted. with AI Agents by Vetcoders (c)2024-2026 LibraxisAI_

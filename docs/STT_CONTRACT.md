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
> text. A terminal ledger seal closes Bus truth only after an **authenticated**
> acoustic observation reports measured speech-span coverage complete within the
> 250 ms edge tolerance; measured silence qualifies, missing, foreign, invalid
> or partial measurement does not (§3.z).
> Planning report: internal plan `stt-apple-must-have` (operator artifact store, 2026-07-24).

---

## Agent speech synthesis (2026-09-08)

Assistant turns expose an always-visible Speak action and the same action in
its context menu; Stop cancels playback and pending synthesis. The assistive
lane selects OpenAI or xAI. Other providers report that no speech endpoint
exists; no alternate provider is silently chosen. Account OAuth wins over
API keys, including when refresh fails. Availability is a local configuration
check, not proof of server-side permissions.

`core/llm/speech.rs` is independent of the disabled CSM engine. It requests
24 kHz mono PCM16, decodes signed samples, splits text at vendor character
caps, and caches audio under `~/.codescribe/cache/tts/<sha256>.pcm`. The hash
covers vendor, model, voice, speed, text and PCM rate; files are atomically
published with owner-only permissions. No credentials or text enter filenames.
`SPEECH_TTS_*` options are defined in `ENV_REGISTRY.toml`. xAI accepts speed
0.7–1.5; OpenAI accepts 0.25–4.0. xAI's default response is raw audio; its JSON
base64 envelope is also supported. The UI labels the voice as AI-generated.

Wire references: [OpenAI speech](https://developers.openai.com/api/reference/resources/audio/subresources/speech/methods/create),
[xAI speech](https://docs.x.ai/developers/model-capabilities/audio/text-to-speech).

## Vendor speech transports (2026-09-08)

The file lane accepts OpenAI `https://api.openai.com/v1/audio/transcriptions`
and xAI `https://api.x.ai/v1/stt`. File upload and remote tail requests resolve
signed-in vendor OAuth first; an OAuth refresh failure is an error, never a
reason to switch to an API key. Without a signed-in account, the explicit lane
key or vendor key supplies bearer authentication. Settings loading and lane
resolution remain snapshot-only; credentials are resolved at request time.
OpenAI defaults to `gpt-4o-mini-transcribe` when `WHISPER_MODEL` is unset and
requests JSON. xAI sends file and language without an unsupported model or
response-format field. Both responses supply `text`.

The existing `GatewayWebSocketTransport` has an xAI wire adapter for
`wss://api.x.ai/v1/stt`: query-based PCM configuration, a required
`transcript.created` readiness event, raw PCM frames, and `audio.done` followed
by the required `transcript.done`. Chunk finals remain provisional until
`speech_final`; terminal cumulative text is not republished as another
occurrence. OpenAI live STT is unsupported by this adapter.

**Integration boundary:** `asr_session::layer1_decision` constructs the live
provider after Cloud consent and endpoint/key admission. Configuration and a
handshake probe alone do not prove a real capture round trip. Apple retains
the live canvas; Cloud supplies the selected Layer 1 session.

Wire reference: [xAI speech-to-text](https://docs.x.ai/developers/model-capabilities/audio/speech-to-text).

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

### Current engine contract (2026-09-25 source cut)

| Layer                 | Rule                                                                                                     |
| --------------------- | -------------------------------------------------------------------------------------------------------- |
| Empty `speech.engine` | ASR mode resolver derives the mode from existing local/cloud intent; otherwise Apple only |
| Settings UI write | ASR mode persists in `settings.json`; no engine or layered side writes |
| Record start          | **`preflight_apple_live_ready()`** when engine is Apple — refuse before REC if Speech/bridge not ready   |
| Live vs final         | Cloud/Apple-only live fails closed without local weights; explicit HQ/local Retranscribe may use Whisper |

---

## 1. What `settings.json` should contain (STT-relevant)

**Path (only this file for UI-promoted settings):**
`~/Library/Application Support/Codescribe/settings.json`

**Schema v3 — speech.engine keys that actually matter:**

| JSON path                       | Internal field          | Wire / env              | Values                                     | Required for “simple works”? |
| ------------------------------- | ----------------------- | ----------------------- | ------------------------------------------ | ---------------------------- |
| `speech.language`               | `whisper_language`      | `WHISPER_LANGUAGE`      | `pl`, `en`, …                              | Yes (you have `pl` ✓)        |
| `speech.engine.asr_mode` | `asr_mode` | `CODESCRIBE_ASR_MODE` | `apple_only` / `local_power` / `cloud` | Sole engine control |
| `speech.engine.whisper_model`   | `whisper_model`         | `WHISPER_MODEL`         | model id                                   | For local refinement          |
| `speech.engine.mode`            | maps to `use_local_stt` | legacy                  | `local_whisper` / `cloud_whisper`          | Optional legacy              |
| `speech.engine.local_model`     | `local_model`           | path                    | model path                                 | Optional                     |
| `speech.formatting.level`       | `formatting_level`      | —                       | `off`/`correction`/`smart`/`max`           | AI format (not STT)          |
| `speech.emission.*`             | buffer/typing           | Voice Lab               | numbers                                    | Overlay pacing only          |

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
The daily Overlay has no user-invoked full-file transcription action: it renders
Bus projections and explicit human edits, never raw `transcribeFile` output.
**2026-09-08 amendment:** terminal recovery reads the recorder-owned finalized
archive with validated session, epoch, rate and sample count. It requests local
Whisper evidence from each material uncovered PCM range. Only source-mapped
segments wholly contained in a gap may enter calibrated ledger admission.
Neither a whole-session comparison string nor a request-wide substitute segment
can create coverage. Straddling, coarse or missing timing remains a refusal.
See [the dated one-throne amendment](SEAL_COVERAGE_AMENDMENT_2026-09-08.md).

**Legacy Overlay Format is removed (2026-08-25).** The former raw LLM
replacement / delivery-style path no longer exists. Automatic formatting is
only the occurrence-bound observer described below; HQ compare remains Whisper
file vs raw Apple, never vs formatted text.

**Terminal format command:** the overlay sends only the current session and
reducer revision. Rust reads the current committed document, invokes the one
production formatting policy pipeline, and commits an applied result through
the ledger-backed revision corridor with `formatter` provenance. Provider
failure, disabled/short-text skip, and unchanged output are refusals: they must
remain visible UI errors and must never become transcript history or Copy-last.

**Dictionary helper (everyone, 2026-08-17):** Settings → Dictionary Retranscribe
is an explicit file surface on the row's archived `<stem>_raw.{m4a,wav,flac}`.
Helper engine follows `speech.engine.asr_mode`: `local_power` → `hq:` (same
candle file pass as `codescribe transcribe --raw`); `cloud` →
`cloud:` file upload. `apple_only` has no helper. The daily transcript is not
overwritten until the user saves a correction. Missing archive must refuse —
never fall back to `last_session.wav`. Lab three-judge / `:8444` is not this
button.

**Minimal durable engine block (Apple live with local refinement):**

```json
{
  "speech": {
    "language": "pl",
    "engine": {
      "asr_mode": "local_power",
      "whisper_model": "whisper-large-v3-turbo"
    }
  }
}
```

## 2. One engine control

Settings writes `CODESCRIBE_ASR_MODE` to `settings.json`; Cloud additionally
requires explicit `CODESCRIBE_CLOUD_CONSENT`. Apple only has no Layer 1 refiner.
Local Power arms local refinement by default. Cloud uses the live provider
factory and reports `live_endpoint_missing` or `live_key_missing` when unconfigured.

The router probes Apple runtime and bridge availability directly. The retired
`CODESCRIBE_STT_ENGINE`, `FINAL_PASS_MODE`, and `CODESCRIBE_FINAL_PASS_MODE`
keys have no routing effect and cannot be written through the configuration API.
Repair removes persisted `stt_engine`, `final_pass_mode`, and `layered_transcription`
with named receipts. It does not seed them again or rewrite an unchanged file.

`CODESCRIBE_LAYERED_TRANSCRIPTION` remains an env-only diagnostic override for
Local Power. Unset arms the lane; `off` or invalid input degrades it and Settings
shows “Degraded (env override)” with the key name. Cloud ignores this local knob.
The read-only Live Whisper refinement row shows Ready / Not ready / Degraded
and offers Recheck. `CODESCRIBE_STT_INITIAL_PROMPT_ENABLED` remains env-seedable.

Normal capture ignores retired final-pass routing and never uploads the
completed WAV for seal coverage. Stop uses the finalized owned archive to
recover material uncovered ranges locally. It does not compare or replace the
whole document automatically. A fresh gap request never clips text from a
segment that crosses existing coverage. Local phase and provider selection are
frozen in the recording snapshot; explicit file actions remain separate.

---

## 3. Front entry → backend handler (STT spine)

### 3.1 Hotkeys (start/stop recording)

| Front entry                     | Binding (your settings)             | Bridge / OS                      | Controller                                                       | Backend                                      |
| ------------------------------- | ----------------------------------- | -------------------------------- | ---------------------------------------------------------------- | -------------------------------------------- |
| Hold Fn (dictation)             | `mode_bindings.dictation = hold_fn` | `CodescribeHotkeys` + CGEventTap | `RecordingController::handle_hotkey_event` → `handle_hold_event` | recorder + streaming session + `core/stt::*` |
| Double Left Option (formatting) | `formatting = double_left_option`   | same                             | hold/toggle + force AI format path                               | STT same, then `core/llm` formatting         |
| Double Right Option (assistive) | `assistive = double_right_option`   | same                             | assistive session                                                | STT same, then agent lane                    |

**Stop** settles live observers, admits qualified source-mapped gap occurrences
from the owned archive, drains formatter slots they created, then recomputes
occurrence-union coverage. A gap over 250 ms blocks terminal truth, and so does
coverage the acoustic observers cannot authenticate (§3.z). Only after a
terminal ledger seal may the projection and delivery owners publish the **final**
transcript, and only there may a paid formatter or a user edit rewrite the whole
document. The deterministic Light+ presentation floor is not gated that way — it
runs per occurrence seal during capture (§3.y). The WAV is never uploaded for
this decision.

### 3.z Acoustic evidence availability

Coverage is measured against an **authenticated acoustic observation**, never
against an empty range set. Two observers may supply one, both bound to the
take's session and capture epoch:

- the **capture energy ladder** (`CaptureEnergyOwner`, producer
  `capture_energy`) — written by the capture arm's `CaptureLevelAccumulator`
  and read by the Apple worker thread through the same shared handle;
- the **Silero ingress** (`SileroIngress`, producer `silero_boundaries`) —
  threshold crossings padded by 64 ms, over the extent it actually ingested.

`assess_seal_coverage` yields `complete`, `incomplete`, or `unavailable(reason)`
with reason `not_observed`, `identity_mismatch`, `invalid_measurement` or
`partial_observation`. Only `complete` may certify a terminal seal; the ledger,
the emitter, the recorder and controller recovery all branch on `is_complete()`,
so no non-success outcome falls through a guard that knew one refusal.

Three rules decide what an observer may say:

1. **Measured silence is a measurement.** An observer that ingested a
   contiguous, finite extent and found no speech reports `observed` with an
   empty range set, gets a real `coverage_ratio` of `1.0`, and seals. Nothing
   in the validity rules below costs a quiet take its seal.
2. **Invalid PCM is not silence.** Non-finite samples (NaN and both infinities)
   are substituted with zero before measurement, which makes an unmeasurable
   region indistinguishable from a silent one. An observer that saw any
   non-finite sample therefore reports `invalid_measurement` for the whole take
   rather than certifying the part it could still read — whether the invalid
   region arrives before, after, or between valid regions, and whether it is a
   whole buffer or one sample inside an otherwise valid hop (zero substitution
   has already depressed that hop's RMS). The contiguous extent measured before
   the first invalid sample is carried as a diagnostic, never as a coverage
   extent.
3. **A partial observation cannot certify the rest.** An observer reports how
   much PCM reached _it_, which is not necessarily what the microphone
   produced: a chunk lane that stops forwarding leaves no hole to detect,
   because the extent simply ends early. Coverage selection holds each
   observer's extent against `LiveAudioBuffer::session_sample_end()` — the
   capture owner's own count of samples seen this session, retained or
   evicted — and an extent shorter than the capture becomes `discontinuous`,
   which the ledger adjudicates as `partial_observation`. The ledger separately
   refuses when committed or measured spans reach past the observed extent.

Selection order: the capture writer adjudicates the PCM it wrote, so an
`invalid_measurement` from the capture energy ladder wins outright — no later
observer over the same buffer may certify samples the writer could not read.
Otherwise a Silero observation that measured speech is preferred as the
narrowest honest answer, and the capture energy ladder is the fallback. Padded
fusion ownership windows are never a candidate: they stay open across pauses and
close on the capture cursor, so they measure ownership, not speech.

Calibration keeps its own unbound `CaptureLevelAccumulator`, which produces the
full statistical receipt without writing to any take's ladder.

Every non-complete outcome still releases lifecycle ownership and preserves the
authenticated committed words and the session WAV; refusing a seal is a quality
verdict, not a total failure. See `docs/DELIVERY_ROUTE.md` for the delivery
consequences and `docs/TRANSCRIPT_BUS.md` for the projected wire contract.

### 3.2 Settings UI → config

| Front control                   | UniFFI                                              | Core                                                                   |
| ------------------------------- | --------------------------------------------------- | ---------------------------------------------------------------------- |
| Load Settings form              | `CodescribeConfig.load_settings()`                  | one `RuntimeSettingsSnapshot` → `CsSettings::from_runtime_snapshot`    |
| Save knobs                      | `update_config` / `update_config_many`              | `UserSettings::set_*` → write `settings.json`; may seed env            |
| ASR mode picker                 | `CODESCRIBE_ASR_MODE` + `CODESCRIBE_CLOUD_CONSENT`  | Cloud requires `granted`; local override cannot disarm Cloud |
| Active STT row                  | `current_serving_verdict()`                         | last live take (`local_apple` → Apple). No Smart-final-pass suffix     |
| Whisper model status / download | `whisper_model_status` / `download_whisper_model`   | `core/config/models.rs`                                                |
| Audio device                    | `audio_input_snapshot` + config keys                | `UserSettings.audio_input_device` + cpal                               |
| Mic permission                  | `mic_permission_granted` / `request_mic_permission` | `app/os/permissions`                                                   |
| Lane (LLM) truth                | runtime snapshot projection                         | `RuntimeSettingsSnapshot::llm_lanes()` → `RuntimeLlmLanes`             |
| AI execution generation         | next selected runtime snapshot                      | sealed prompts + retry/delay + shared Agent/formatter request timing   |

### 3.3 Dictation overlay / tray

| Front                         | UniFFI                                             | Handler                                                         |
| ----------------------------- | -------------------------------------------------- | --------------------------------------------------------------- |
| Committed transcript truth    | `CsTranscriptProjectionEvent`                      | ledger receipt → reducer → Transcript Bus → listener projection |
| Ephemeral/raw observations    | `EngineEventWire` IPC diagnostics                  | never cross `CsTranscriptionListener`; never delivery writers   |
| PCM sideband evidence         | `EngineEventWire::SidebandEvidence`                | Silero ingress → IPC → bridge diagnostic; reducer no-op         |
| Recording service object      | `CodescribeHotkeys`                                | shared controller recording API                                 |
| Tray status glyphs            | `CodescribeTrayStatus` + listener                  | controller tray payload                                         |
| Auto-paste / auto-format tray | `set_auto_paste_enabled` / `set_auto_format_level` | `UserSettings` + live toggles                                   |

### 3.4 STT engine dispatch (the nit)

| Call site             | When               | Function / transport                                             | Engine rule                                                           |
| --------------------- | ------------------ | ---------------------------------------------------------------- | --------------------------------------------------------------------- |
| Live Layer 0          | during recording   | Apple progressive                                                | first PCM-pinned observation; text is revisable by same-span evidence |
| Live Layer 1 typed    | during recording   | in-process / sidecar / remote tail provider on ~5 Apple segments | exact-PCM outcome rewrites pending baseline before final              |
| Live Layer 1 unbound  | after recording    | full-session Voice Lab WSS candidate                             | evidence only; typed refusal if it proposes mutation                  |
| Live capture epoch    | device open/reopen | `StreamingRecorder` checked session-local counter                | issued only after successful open; engines only observe it            |
| Explicit Retranscribe | operator action    | local completed-file decode or cloud multipart                   | may replace the selected artifact, never the live canvas              |

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
permit owns the exact existing `(session, capture_epoch, sample_start, sample_end)` and every earlier scheduled automatic observer for that occurrence
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
word pins. The ledger projection orders `(capture_epoch, sample_start, sample_end)` lexicographically and rejects loss, addition, or reorder before
delivery. Preview is overlay-only and is discarded at terminal boundaries.

Active W2-04 Agent leases are read directly as a bounded 120-second snapshot.
Their names are placed first in the existing Whisper context budget and
canonicalized only by exact whole-word matching in Lexicon/Light+. Stale,
malformed, unknown, or colliding leases fail open. There is no phonetic/fuzzy
rewrite: active `Iwo` does not rewrite Polish `piwo`.

Terminal overlay edits remain local drafts until a compare-and-swap intent
names the exact source session and reducer revision. Rust authenticates the
sealed source occurrence set, appends a whole-document receipt with
`provenance=user-edit`, and emits the next terminal projection; Swift never
optimistically replaces the committed render. That `user-edit-*` receipt is
consumed by one quality commit separately from delivery `action`. Three
distinct correction IDs for the same normalized lexical pair expose `1/3`,
`2/3`, `3/3` and promote exactly once. Formatter, machine file passes, replay,
bulk, speech-gap, and delivery actions without that receipt cast no vote.

Dictionary **Teach** is a separate, explicit bulk-promotion command: it mines
eligible correction-store and proposed rows immediately and therefore bypasses
the automatic three-human-correction threshold. The UI must say that plainly;
running Teach is operator authorization, not passive learning.

**Runtime proof:** Settings may show configured readiness, but a take counts as
exercised only when its typed receipt says `armed=true` and `submitted>0`.

```text
selected_engine()
  ├── Apple  → LIVE: run_apple_live_only  |  ADAPTER: run_apple_or_whisper
  └── Candle → whisper singleton (label: local_whisper)
```

`auto` = Apple if `apple_stt::is_runtime_available() && is_bridge_resolvable()` else Candle.

---

### 3.x Acoustic admission (precondition of every take)

Before the controller opens a microphone (`start_toggle_recording`, hold-start
task) it evaluates `controller::admission::evaluate_live_admission` on the
selected settings generation, in this order, and stops at the first blocker:

| Order | Blocker code                                                               | Cause                                                                                                                               | Founder action                                                                                |
| ----- | -------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------- |
| 1     | `admission_microphone_permission_unavailable`                              | macOS microphone permission is denied or not determined                                                                             | System Settings › Privacy & Security › Microphone                                             |
| 2     | `admission_capture_device_unavailable`                                     | cpal cannot resolve the configured/default input                                                                                    | plug/select an input device                                                                   |
| 3     | `admission_calibration_missing` / `_refused` / `_no_profile` / `_unusable` | `energy-calibration.json` absent, tampered, expired/future-dated, internally rollbacked, or not measured on this capture generation | Settings › Audio › **Calibrate microphone** (~10 s of normal speech)                          |
| 4     | `admission_seal_lane_disarmed`                                             | `audio.seal_lane_armed=false`, or the `CODESCRIBE_SILERO_FUSION=0` power-user override → no occurrence can qualify or commit        | enable **Seal lane** in Settings › Audio; if overridden, remove the override or set it to `1` |
| 5     | `admission_seal_vad_unavailable`                                           | Silero ORT session refused to load                                                                                                  | reinstall / check the embedded VAD asset                                                      |

A refusal writes nothing to the Transcript Bus, opens no stream, resets the
session to Idle and reaches the overlay as a typed, passive
`PresentationStatusProjection` with kind `admission_refused`, the blocker code,
explanation and action. The projection shares the existing IPC/listener
transport but cannot impersonate occurrence-authenticated transcript truth.
`CodescribeHotkeys.admissionReadiness()` exposes the same verdict to Settings ›
Audio; `calibrateEnergy(seconds:)` performs the guided measurement through the
real recorder path and stores the profile via
`EnergyCalibrationArtifact::record_profile`. Before a successful call returns,
the controller replaces its immutable runtime-settings generation from that
stored profile; the immediate Settings probe therefore cannot read the prior
generation. Success (with profile version) and failure also publish typed
presentation status. Nothing on this path invents a threshold;
`EnergyCalibration` has no default.

The immutable settings generation owns seal-lane arming. Fresh and legacy
settings without `audio.seal_lane_armed` resolve to the supported production
default `true`. An optional `.env` or process
`CODESCRIBE_SILERO_FUSION` value remains the power-user override and wins in
both directions (`0` and `1`); its presence is reported as `env_override` to
Settings and refusal copy. No pipeline or admission consumer re-reads either
source after the snapshot is sealed.

Calibration validity is part of the one profile authority, not an environment
or Settings override. Schema `codescribe.energy-calibration.v2` stores and
digest-covers policy `monthly-capture-path-v1`: a profile is valid through
exactly 30 days after `measured_at_unix_ms` and expires one millisecond later.
Admission also refuses evidence dated after its injected clock, an artifact
whose `updated_at_unix_ms` predates a contained measurement, and a live capture
path whose SHA-256 generation fingerprint differs. That fingerprint covers the
exact cpal device display name, native sample rate, and native channel count,
so the same display name at a changed rate/channel layout is not the same
calibration generation. Older schemas receive no permissive defaults; the one
remediation for every validity refusal is to re-calibrate.

### 3.y Live Light+ (deterministic presentation during capture)

**W2 publication recovery (2026-09-10): source checkpoint, still isolated.**
Reducer snapshots now bind every public field to a private, in-process publication
digest and validate every current/retained presentation receipt against the live
ledger before Bus or delivery publication. The digest is a reducer capability,
not an acoustic witness, serialized signature or second document authority.
Ledger seals have explicit `Occurrence` / `Terminal` scope with different IDs,
even for one occurrence. Tests remain UNRUN under the compile embargo; generated
bindings, executable validation and installed runtime proof belong to W3/W4.
This source description is not a structural-close or runtime attestation.

Committed utterances become readable while the take is running. Light+ has
separate presentation and acoustic finality gates:

| Gate                               | Scope               | Receipt                                                   | Lifecycle  |
| ---------------------------------- | ------------------- | --------------------------------------------------------- | ---------- |
| Committed label (`LedgerMutation`) | exactly those words | `IncrementalShapingReceipt`; seal reference if one exists | stays open |
| Terminal ledger seal               | the sealed document | explicit terminal seal; optional document Light+ receipt  | finalizing |
| Controller `session_ended`         | capture lifecycle   | no new shaping or acoustic authority                      | ended      |

The live gate is `TranscriptReducer::apply_incremental_shaping`, driven by
`PresentationEmitter::mint_incremental_light_plus`:

- It shapes each committed occurrence with `light_plus::apply_live_span`, using
  the committed text to its left as casing context. An open predecessor does
  not block later words. A gap on the same PCM clock starts a sentence when it
  reaches `LIGHT_PLUS_SENTENCE_PAUSE_SEC`; shorter gaps join the words without
  a period. The final period is added when the document is frozen or finalized.
- The acoustic label stays immutable. The shape lives beside it, keyed by the
  same occurrence, so later speech appends instead of replacing the document —
  a single whole-document override discarded on the next insert is not
  incremental delivery.
- The receipt names the occurrence, its seal when present, the exact source
  label, the sentence-break decision and exact left-context bytes with their
  SHA-256, so a shape can be reproduced and a
  stale one detected. A relabel or earlier insertion invalidates affected shapes,
  including dependent suffix shapes. Earlier receipts remain immutable history;
  a later valid shape receives its own source revision and context provenance.
- A shape that consumed every word (a hesitation-only utterance) is refused: an
  empty presentation may never delete captured speech.
- Duplicates are no-ops. A replayed seal, or a shape that changes nothing, mints
  no second revision and no second receipt.
- The literal contract (Ctrl-hold `force_raw`) skips the live gate exactly as it
  skips the terminal one.
- A live revision publishes through the ordinary committed corridor — Bus
  (`reducer_action: apply_incremental_shaping`, phase `listening`), projection
  callback, delivery buffer — and never through the lifecycle one. It sets no
  terminal flag, no `lifecycle_terminal`, and no delivery disposition.

Stop applies Light+ to the frozen canvas before paste and publishes the exact
paste bytes as a document revision. Finalization closes an unfinished last
sentence even when acoustic terminal coverage is refused. The terminal CAS
source for the paid formatter and a user edit is that same document.
Per-entry Bus and bridge `presentation_receipt` fields carry
both new and retained Light+ provenance; they never use `manual_edit_receipt` or
invent word-to-PCM mapping. A terminal user/formatter document receipt retains
its separate whole-document authority. Duplicate seal delivery produces no
reducer revision, callback or Bus row, and Bus rejects replayed revision IDs.
Tests distinguish one document revision from its N per-entry projection rows.

## 4. Labels vs truth

| Surface                              | Source of truth                             | Not truth          |
| ------------------------------------ | ------------------------------------------- | ------------------ |
| Settings **ASR mode** | resolved mode from the runtime snapshot | Last serving engine |
| Settings **Active STT**              | `current_serving_verdict().engine` last run | Preference string  |
| Overlay footer engine chip           | last verdict / controller truth label       | “I wanted Whisper” |
| Error text                           | actual failing path                         | —                  |

Valid engine labels on verdict: `local_apple`, `local_whisper`, `streaming_whisper`, `cloud_stt`.

---

## 5. Engine selection

Choose Apple only for the Apple canvas without refinement; Local power for
bounded local Whisper refinement; Cloud for consent-gated live audio egress.
The last take supplies Active STT and the overlay engine chip. A selection or
model readiness check is not evidence that a particular engine served a take.

Normal stop never performs a whole-session file pass. Dictionary Retranscribe
and other explicit file actions retain their own routes.

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

## 8. Engine controls source checkpoint (2026-09-25)

One ASR mode control replaces the retired engine selector and fixed Final Pass
row. Local readiness and diagnostic override status are read-only projections.
The W1 source checkpoint does not certify build, installation, or live audio;
those remain integrator W3/W4 gates.

---

_Vibecrafted. with AI Agents by Vetcoders (c)2024-2026 LibraxisAI_

## Local Whisper execution settlement (W2 source checkpoint)

Live tail refinement and terminal uncovered-PCM repair share one
`LocalExecutionOwner` per Apple transcription session. It starts native workers
and retains their actual join handles separately from result receivers. Closing
ledger accounting or dropping a result receiver cannot detach inference.
Finished workers are reaped during capture; after the Apple worker closes,
admission is cancelled and all remaining executions are joined before
`SessionFinalised`. The recorder retains its transcription task across a
cancelled Stop caller and still returns the original typed coverage refusal and
WAV when speech could not be authenticated.

The existing five-second useful-refinement drain starts at the worker's local
closure phase. Live requests and every terminal gap share that same absolute
deadline; neither another gap nor a local fallback renews it. This is not a
five-second bound on all of Stop. Key-up does not cancel all refinement, and
ordinary useful terminal repair remains admitted while budget remains.

`LocalExecutionControl` travels through provider selection (including local
fallback), singleton acquisition, VAD boundaries, decoding windows and token
steps. Each independent public file call gets an unlimited control and uses the
same engine implementation. An expired/cancelled waiter polls only its own
control and exits without acquiring or cancelling a foreign engine holder.
The request scope restores the prior prompt and clears model KV caches on
success, error, cancellation and unwind. Cancellation after a native result
returns discards that result as an error; it is not observer success. Existing
session, epoch, request and exact source-PCM containment checks remain the only
route to ledger admission. No text seal or fabricated timing is introduced.

Model resolution/loading, device initialization, native VAD extraction,
resampling/mel construction, individual Candle/Metal encoder/decoder/tensor
operations, cache cleanup and filesystem/native teardown are not preemptible.
The owner waits for a running call to return. Its normal async join retains
handles across awaits; the final Drop fallback cancels and synchronously joins,
which can block the dropping thread. There is no hard release bound, unsafe
thread termination or claim that a timed-out result means released resources.
Native microphone/VAD/archive settlement, Apple-worker lifetime, cloud transport
and formatter acknowledgements remain separate RC obligations.

This is source-level wiring with authored, UNRUN barrier, contention, decoder,
terminal repair and WAV preservation tests. BUILD/TEST/RUNTIME=NOT_ASSESSED
under the Grade B W2 compile embargo. Admission, returning compiler/test gates
and installed real-audio evidence belong to the integrator.

## Take truth sidecar is an observer (2026-09-16)

Every file take leaves a `<file>.truth.json` beside its input (`codescribe transcribe`, opt-out `--no-truth`), and the app daily archive writes
`<base>.txt.truth.json` next to the paired `*_raw.{m4a,wav}` + `*_raw.txt`
whenever the caller hands the archive a `TakeTruth`. The sidecar is an
OBSERVER projection of the `TranscriptionVerdict`; no delivery path reads it
back. `codescribe transcribe --inspect` prints the same truth to stderr under
one time axis: the segment block, the 32 ms Silero row, and the energy row.

# STT contract — front → backend (no lottery)

> Status: operator truth map · 2026-07-24 · branch `feat/operator-feedback-wave9`
> Goal: every UI/hotkey entry is one line to one handler; settings truth is one place.
> **Operator lock: Apple STT is MUST-HAVE for live.** Whisper is never the primary live engine.
> **Superseded on Whisper's role (2026-07-26, `AGENTS.md` — THE ONE RULE):** the target shape is
> Whisper transcribing **partials on the go** to fill canvas gaps — NOT final-pass-only.
> Lexicon substitution is the FINAL automated layer, after Whisper.
>
> **Status (2026-08-14):** on-the-go gap-fill **exists and is the stock live
> default** as Layer 1 tail-patch on both live paths (`a6b1233d`, default
> flip 2026-08-09). `CODESCRIBE_LAYERED_TRANSCRIPTION` unset → `phase1`;
> explicit `off`/`0`/`false` disarms. Legacy `FINAL_PASS_MODE` no longer owns
> any normal-stop inference. W13 fusion /
> idempotence / highlights stay OFF until an operator flip.
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

| Layer                 | Rule                                                                                                   |
| --------------------- | ------------------------------------------------------------------------------------------------------ |
| Empty `speech.engine` | Load defaults to **`stt_engine=apple`**, **`final_pass_mode=smart`**                                   |
| Settings UI write     | **Promoted** to `settings.json` + reconciles process env **and** `.env` (single brain)                 |
| Record start          | **`preflight_apple_live_ready()`** when engine is Apple — refuse before REC if Speech/bridge not ready |
| Live vs final         | Live = Apple only; file final = Whisper; recovery path separate from mid-stream swap                   |

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
probe, live socket handshake, and explicit retranscribe path use the same
resolver, so Settings cannot disagree with delivery.

Transport ownership is equally explicit. Normal cloud capture uses the proven
Voice Lab WebSocket (`config` → bounded PCM `chunk` → periodic `flush` →
`end`) and streams its normalized events into `PresentationEmitter`. A complete
audio-file multipart request is allowed only for an explicit retranscribe
action (Overlay, Dictionary, or Teacher). An OpenAI multipart URL has no Voice
Lab mapping, so it cannot silently turn a normal stop into a whole-file upload.

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

Still env-seedable when unset (not dual writers): `CODESCRIBE_LAYERED_TRANSCRIPTION`, `CODESCRIBE_STT_INITIAL_PROMPT_ENABLED`.

> **Power-user hazard (measured 2026-08-08).** Because `CODESCRIBE_LAYERED_TRANSCRIPTION` is
> **not** promoted to `settings.json`, `Config::inject_file_env_for_runtime` copies it out of
> `~/.codescribe/.env` into the process env on the first `Config::load()` — in _every_ process
> that loads the core, tests and harnesses included. A stale `.env` line therefore arms Layer 1
> silently. This was observed live: the same `make test-engine-parity` binary scored 0.931 with
> the lane off and 0.833 with the operator's dotenv arming `phase1`, and the low score was the
> _more accurate_ transcript. The parity target now pins the lane explicitly (`Makefile`), but
> the general hazard stands for any tool that loads the core. Promoting the key the way
> `CODESCRIBE_STT_ENGINE` was promoted is an open operator decision.

**Final pass vs layered (orthogonal):**

| Setting    | Env                                | Default  | Role                                                                                                                 |
| ---------- | ---------------------------------- | -------- | -------------------------------------------------------------------------------------------------------------------- |
| Final pass | `FINAL_PASS_MODE`                  | legacy   | No effect on normal stop; retained only for settings migration while explicit Retranscribe owns whole-file inference |
| Layered    | `CODESCRIBE_LAYERED_TRANSCRIPTION` | `phase1` | During-hold Layer 1 tail-patch on **both** live paths — local Whisper or live cloud WSS, selected by product mode    |

Normal capture ignores legacy final-pass routing and never decodes/uploads the
completed WAV. Layered phase tokens (`phase1`…) select live refinement;
whole-file inference is an explicit Retranscribe action.

---

## 3. Front entry → backend handler (STT spine)

### 3.1 Hotkeys (start/stop recording)

| Front entry                     | Binding (your settings)             | Bridge / OS                      | Controller                                                 | Backend                                      |
| ------------------------------- | ----------------------------------- | -------------------------------- | ---------------------------------------------------------- | -------------------------------------------- |
| Hold Fn (dictation)             | `mode_bindings.dictation = hold_fn` | `CodescribeHotkeys` + CGEventTap | `AppController::handle_hotkey_event` → `handle_hold_event` | recorder + streaming session + `core/stt::*` |
| Double Left Option (formatting) | `formatting = double_left_option`   | same                             | hold/toggle + force AI format path                         | STT same, then `core/llm` formatting         |
| Double Right Option (assistive) | `assistive = double_right_option`   | same                             | assistive session                                          | STT same, then agent lane                    |

**Stop** drains the live recorder/session and delivers its committed transcript
(paste / overlay / agent). It does not upload the completed WAV. Whole-file
file-pass belongs only to explicit retranscribe surfaces.

### 3.2 Settings UI → config

| Front control                   | UniFFI                                              | Core                                                                   |
| ------------------------------- | --------------------------------------------------- | ---------------------------------------------------------------------- |
| Load Settings form              | `CodescribeConfig.load_settings()`                  | `UserSettings::load` + `Config::load` + env merge → `CsSettings`       |
| Save knobs                      | `update_config` / `update_config_many`              | `UserSettings::set_*` → write `settings.json`; may seed env            |
| ASR mode picker                 | `CODESCRIBE_ASR_MODE` + `CODESCRIBE_CLOUD_CONSENT`  | Cloud never displays without `granted`; stop ignores `FINAL_PASS_MODE` |
| Active STT row                  | `current_serving_verdict()`                         | last live take (`local_apple` → Apple). No Smart-final-pass suffix     |
| Whisper model status / download | `whisper_model_status` / `download_whisper_model`   | `core/config/models.rs`                                                |
| Audio device                    | `audio_input_snapshot` + config keys                | `UserSettings.audio_input_device` + cpal                               |
| Mic permission                  | `mic_permission_granted` / `request_mic_permission` | `app/os/permissions`                                                   |
| Lane (LLM) truth                | `lane_truth_snapshot(lane)`                         | `core/llm/lane_truth.rs`                                               |

### 3.3 Dictation overlay / tray

| Front                         | UniFFI                                             | Handler                         |
| ----------------------------- | -------------------------------------------------- | ------------------------------- |
| Live partials / final text    | `CsTranscriptionListener` callbacks                | streaming pipeline → listener   |
| Recording service object      | `CodescribeHotkeys`                                | shared controller recording API |
| Tray status glyphs            | `CodescribeTrayStatus` + listener                  | controller tray payload         |
| Auto-paste / auto-format tray | `set_auto_paste_enabled` / `set_auto_format_level` | `UserSettings` + live toggles   |

### 3.4 STT engine dispatch (the nit)

| Call site             | When             | Function / transport                           | Engine rule                                              |
| --------------------- | ---------------- | ---------------------------------------------- | -------------------------------------------------------- |
| Live Layer 0          | during recording | Apple progressive                              | committed canvas floor                                   |
| Live Layer 1 local    | during recording | bounded Whisper windows                        | gap/tail fill only                                       |
| Live Layer 1 cloud    | during recording | Voice Lab WSS                                  | normalized gap/tail fill only                            |
| Explicit Retranscribe | operator action  | local completed-file decode or cloud multipart | may replace the selected artifact, never the live canvas |

**This split is the MacGyver fracture:** UI can show Whisper readiness while live is Apple-only and fails closed.

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
   "engine": { "stt_engine": "apple", "final_pass_mode": "smart" }
   ```
2. Full quit + relaunch.
3. Footer / Active STT after a take: **`local_apple`** on happy path.
4. Empty death mid-take = **code cut** (preflight + Whisper recovery when audio exists) — see planning report Wave 1. Settings alone cannot fix `run_apple_live_only`.

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

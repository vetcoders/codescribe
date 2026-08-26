# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## Release reality

| Version  | Repository milestone | Public distribution status                                                                         |
| -------- | -------------------- | -------------------------------------------------------------------------------------------------- |
| `0.13.3` | 2026-08-13           | **Latest published GitHub Release** (`v0.13.3`), signed, notarized, and stapled.                   |
| `0.14.0` | 2026-08-17           | Source/daily-build milestone only; no Git tag or GitHub Release was published.                     |
| `0.14.1` | 2026-08-18 onward    | Current source version and release candidate; no Git tag or GitHub Release has been published yet. |

The sections below distinguish code milestones from public releases. A version
number in `Cargo.toml` is not evidence that a DMG, tag, appcast, or GitHub
Release exists.

## [Unreleased]

> The `0.14.1` stabilization fight: retire Q8 completely, compose and validate
> one loader-compatible FP16/F32 Whisper bundle, make Apple and Whisper observe
> the same PCM clock, stop text-only deduplication from deleting intentional
> repetitions, and make every admitted correction and stop outcome auditable.
> This work is in source; it is not yet a public `v0.14.1` release.

### Added

- **Acoustic occurrence, observation, and mutation receipts.** The live path
  separates what was spoken on the PCM clock from what Apple/Whisper observed
  and from the mutation that changed the canvas. Replays are keyed by
  structural observation identity; two identical spoken words on disjoint PCM
  spans remain two occurrences.
- **One process-owned four-worker async runtime.** UniFFI exports enter a single
  application-owned Tokio runtime instead of implicitly creating independent
  worker pools per feature. Startup, task ownership, cancellation, worker
  names, and bounded teardown are visible through a content-free snapshot.
- **Exactly four machine layers.** L0 Apple live, L1 Whisper observation, L2
  Lexicon + Light+, and L3 the existing Responses formatter. Silero remains the
  VAD/time-evidence plane; `SessionFinalised` is lifecycle, not a hidden Final
  BAM producer.

- **The signed app can install the live named-agent bridge.** Agentic Readiness
  keeps the 13-step Setup Wizard intact while letting the operator explicitly
  select Codex and/or Claude Code. A checksumed bundle payload installs to the
  stable `~/.codescribe/agent-bridge/` runtime with one receipt and managed
  markers; foreign skill folders are visible conflicts. The demux follows live
  drafts, waits for `transcript_sealed` before state changes, and persists a
  provider-session lease/cursor plus active names across provider recovery.

### Fixed

- **Repeated speech is no longer deleted by string equality.** Light+ stopped
  collapsing every immediately repeated word, and decoder-loop cleanup now
  consults the number of acoustic spans before removing a run. Saying a name
  five times must preserve five occurrences; cleanup may remove only copies
  that outnumber the audio evidence.
- **Layer 1 stop receipts use independent terminal counters.** Applied,
  skipped, timed-out, and abandoned jobs must sum to submitted jobs from their
  real producers; `abandoned` is no longer invented as the arithmetic remainder
  that made reconciliation impossible to falsify.
- **Application runtime startup and shutdown fail honestly.** A failed named
  worker start rolls the runtime back so retry cannot report a false `running`
  state. Quit gives recording finalization a bounded wait, releases microphone
  ownership, and then tears down the runtime.
- **RUSTSEC-2026-0258 is removed from the HTTP/2 stack.** `h2` is updated from
  `0.4.15` to `0.4.16`, which contains the empty-DATA-frame resource-exhaustion
  fix. `cargo audit` remains a release gate, not a one-time claim.

- **`make install-app` accepts keys from Get license.** A keyed local
  install verifies CSK1 with the same public key the site signs. The
  forgeable development verifier is no longer baked into that path.
- **Refused paste does not steal the user's clipboard.** Synthetic Cmd+V
  still snapshots and restores after a confirmed paste into a foreign app.
  `CopyTargetUnavailable` / target mismatch / Accessibility deny no longer
  dump the transcript onto `NSPasteboard`. The text parks in the Paste Here
  slot (⌘⌥V when that chord is bound); the overlay keeps it; the user's
  previous clipboard stays put. Explicit overlay Copy is unchanged.
- **Paste status lives in the overlay footer.** Insert that cannot reach
  the ambulance no longer throws a capsule over the action row. A quiet
  chip sits next to `local apple` (`⌘⌥V` / `copied` / `no ax`).
- **Auto-paste lands only in a latched foreign application.** The Codescribe
  overlay and Agent window are not Cmd+V targets; Agent delivery uses the
  explicit Agent route. A foreign target must be observed as frontmost after
  activation; Codescribe remaining frontmost is a refusal, not a guessed
  success. Closed, expired, or unconfirmed targets fail into Paste Here without
  relaunching another application or replacing the user's clipboard.
- **CS Voice Lab starts with the take.** A keyed `install-app` bake
  spawns `~/.codescribe/voice-lab` when recording prepares, and the
  existing Voice Lab buttons ensure `:8765` before opening the
  console. Production stays inert. The child stops when the take
  ends (and on quit). Loopback STT `:8444` / `:8446` stay up.
  `docs/loopback.html` and `~/.codescribe/voice-lab/loopback.html`
  point at those URLs.
- **Agent chat shows live capture.** Assistive/Agent hides the overlay,
  so the composer now renders the growing transcript above the field.
- **Overlay default stays pinned top-right.** Free motion is only the
  explicit toggle. A drag without it is ephemeral. Edge-resize always
  persists, independent of the pin.
- **Overlay no longer owns a whole-file transcript replacement.** The file
  retranscribe/revert UI that wrote machine output directly into the formatted
  canvas is removed. Daily Overlay text now comes only from Bus projections or
  explicit human edits; Dictionary and Voice Lab file helpers stay separate.
- **Mid-hold Shift attaches `{selection_N}`.** Shift or Command during an
  already-started Fn hold captures the current selection into the context
  bucket and overlay marker. It does not open Agent, hide the overlay, or
  stop the take. Fn+Shift from idle stays dictation, not Assistive.
- **Fn hold-down attaches a live selection as `{selection_1}`.** A
  selection already present when Fn goes down is captured immediately.
  Later Shift pulses still add `{selection_2..n}`. Destination stays
  dictation.
- **Layer 1 `cloud_session` stays up on Voice Lab `:8446`.** The live
  socket opens with `hello` (`stt-ws-v1`); treating that as protocol
  dropped the lane at take start (`disconnect`, zero frames). Handshake
  and VAD control are ignored, and the start frame is Voice Lab `set`.
- **Compound Apple chops take the joined Layer 1 rewrite.** Five short
  fragments share one Whisper window and apply the aligned sentence
  swap. Fusion no longer rewrites only the last piece or skips the
  joined sentence at the 50% change cap.
- **Dictionary file retranscribe names the programming domain.** Its `cloud:`
  pass over archived row audio (remapped loopback `:8444`) sends
  `vocabulary=programming` — test-locked on the multipart body. Official
  OpenAI still omits the field; the daily Overlay owns no file-pass writer.

### Changed

- **Span idempotence is enabled by default.**
  `CODESCRIBE_SPAN_IDEMPOTENCE` changed from `0` to `1`. The gate deduplicates
  structural replays of the same observation identity; it must never dedupe
  intentional repetitions by text.
- **Layer 1 is mode-owned.** With no explicit global phase token, Local Power
  arms the Apple-first local Whisper observer by default; Apple-only does not.
  `CODESCRIBE_LAYERED_TRANSCRIPTION=off` is an explicit degraded override,
  while legacy `phase1` remains a compatibility token. “Unset globally” and
  “armed in Local Power” are therefore not contradictory.

- **Local Whisper is an explicitly validated FP16/F32 bundle.** Runtime,
  Settings download, release scripts, E2E discovery, and the optional fat build
  share the same architecture, tokenizer-vocabulary/language, pinned-mel, and
  required tensor-name/shape contract. The loader rejects incomplete bundles
  before cold model construction; prompt/control token IDs, automatic-language
  candidates, layer/context resource bounds, and mapped tensor-name collisions
  are validated by the same runtime-owned helpers. Tokenizers cannot emit IDs
  without embedding rows; audio context must match the supported 30-second
  window; matching state widths are capped at the official Whisper maximum of
  1280; decoder context is capped at the supported 448 positions; timestamp
  token ranges are validated end to end; mel verification is size-bounded before
  hashing; config/tokenizer JSON and vocabulary size are bounded before parsing
  or allocation; and surplus tensors are refused before allocation. Quantized payloads and the
  legacy Q8 fallback are refused; the old public Q8 identifiers remain
  deprecated source-compatibility constants only. Building from source now
  declares Rust 1.88 as the minimum supported toolchain.
  Warm-cache tokenizer repair now returns immediately when it completes the
  installed bundle instead of falling through to redundant network downloads,
  and it preserves a valid installed config/weights pair without creating a
  weights-sized temporary copy. STT benchmarks now use the production model
  resolver, including validated Hugging Face cache snapshots.

- **Supervisor findings own transcript-quality categories.** Engine catalog
  `codescribe-supervisor-findings/v1` (`core/quality/supervisor.rs`) names
  every issue class the tree already had — contract forbiddens, clock-lie,
  speech gaps, Teacher attention, confidence flags, delivery gates, Whisper
  residue, and the Voice Lab lies (HQ-as-document, omitted
  `vocabulary=programming`, live overlay paired with last_session.wav).
  Voice Lab three-judge emits those findings. WER stays a footnote of
  proposal agreement, not accuracy.

- **Layer 1 applies aligned same-utterance wording.** When live Apple and
  the Whisper window share most words, Layer 1 now substitutes those
  spans instead of discarding the repair at the 50% change cap. Unrelated
  dumps and pause-tail inserts still skip.
- **Layer 1 Whisper windows join about five Apple segments.** Short
  fragments wait for a sentence-sized window (or a pause) before the
  background swap, instead of each breath becoming its own failed
  repair.
- **Cloud STT names the programming domain.** Loopback and Libraxis file
  and live requests send `vocabulary=programming`. Official OpenAI file
  audio omits the field. The client does not classify audio to pick a
  dictionary. Overlay Format is not that compare — HQ is Whisper file vs
  raw.
- **Dev-power corner mark.** A keyed local install paints a small
  “You use dev power mode” caption in the bottom-right of overlay, Agent
  chat, and Settings. Production DMGs stay unmarked.

### Candidate closure and next steps before public `v0.14.1`

Candidate `3deadbdf` has completed the local source and distribution gates:
`make check`, `make verify`, `make test-swift`, `cargo audit`, public-tree and
history privacy review, Developer ID signing, Apple notarization, stapling, and
`verify-dmg`. The accepted slim artifact is
`Codescribe_0.14.1-20260822-3deadbdf8.dmg`; this is still not a GitHub Release.

- Keep the competing PR #82 acoustic ledger out of this release candidate.
  After release, port its occurrence/observation/receipt model only through a
  dedicated cut with the five-`Iwo` conservation fixture and left-, right-,
  and multi-owner overlap falsifiers. Shared contract ancestry is not runtime
  integration.
- Close or explicitly retain the small deferred set: two missing
  `TAIL_PATCH_APPLY_REFUSED` branch tests, a host probe for Swift-to-Rust task
  cancellation, a user-visible hotkey/TCC recovery notice, and an Agent Bridge
  manifest cache only if measured Settings latency justifies it. Silero fusion
  stays diagnostic and OFF until its enclosing-range semantics pass live A/B.
- When the Transcript Bus is idle, install the exact stapled `.app` from that
  DMG and run the installed-app microphone/delivery smoke plus the available
  host corpus/acceptance probes. Packaging proof is not installed-runtime proof.
- Keep the functional OAuth client registration as a reviewed public
  identifier unless it is replaced atomically; never treat it as a leaked
  session token and silently break sign-in.
- Publish the tag, appcast, and GitHub Release only after explicit operator
  approval and the installed-app smoke. Until then `v0.13.3` remains Latest.

## [0.14.1] - 2026-08-18

> Patch: everyday-stable 0.14.x. Same slim public SKU as 0.14.0, plus the two
> Settings/auth probes that were still lying on a daily machine, and one
> command that installs the notarized .app instead of re-signing it. This was a
> source/daily-build milestone, not a published GitHub Release.

### Fixed

- **STT Test is a file probe.** Settings Test no longer POSTs to a live
  socket. Known live sockets map to `/v1/audio/transcriptions`.
- **ChatGPT sign-in no longer requires Responses write.** OAuth persists
  identity after exchange. `api.responses.write` stays a lane Test, so
  Codex public tokens can sign in.
- **STT remapper test names loopback explicitly** so `make check` Semgrep
  does not treat a templated live-socket URL as an open WebSocket.
- **Overlay copy stays evidence-only in the quality-chain test.** Isolated
  `make verify` no longer greened that path by reading a leftover host
  lexicon. Voice Lab finalize is still the teach gesture.

### Changed

- **`make release-stable`** is the everyday cut: slim sign + notarize +
  `verify-dmg`, then install that stapled Developer ID `.app` to
  `/Applications` without re-signing. `make install-app` remains the
  local-release path.
- **`make release-full` is fail-closed.** Whisper embed no longer falls
  back to a slim dylib when the HF snapshot is weights-only. It uses the
  composed `~/.codescribe/models` tree from `make download-model`.
- `SITE_VERSION` stays `0.13.3` until a published GitHub release.

## [0.14.0] - 2026-08-17

> Minor: developer Lab surface, Dictionary helper file-pass, bus word pins,
> and a 30-minute Whisper idle. Production DMG still has no Lab menu. This was
> a source/daily-build milestone; no `v0.14.0` tag or GitHub Release exists.

### Added

- **One Transcript Bus and one delivery throne.** The presentation reducer
  became transcript authority for overlay, paste, history, Agent capture, and
  diagnostic followers. Delivery follows explicit operator intent rather than
  whichever application happens to own OS focus.
- **PCM-clock word pins and energy evidence.** Transcript spans carry their
  capture clock and energy so later observers can prove which audio they are
  talking about instead of matching only strings.

- **Developer Lab on a keyed local install.** A public `git clone && make`
  stays Lab-off. Production DMG refuses the bit.
- **Lab mode overlay-off.** Developer veto hides the daily HUD without
  flipping the tray "Transcription Overlay" toggle. Leftover UserDefaults
  cannot hide overlay on a production bundle.
- **Dictionary helper Retranscribe.** Follows Settings `asr_mode`:
  `local_power` → `hq:` candle file pass, `cloud` → `cloud:` file pass,
  `apple_only` disabled. Missing archive refuses — never `last_session.wav`.
- **Voice Lab on the website** (`/voice/lab`). Teacher + Seal Atlas as a
  Codescribe module, not a sidecar: same `teach()` triangle as
  `codescribe-teacher`, idle until Run; Atlas HTML loads only on demand.
- **Overlay stays the canvas.** Assistive hold/toggle stays on the live
  overlay (composer mic is the only Agent-owned capture). Action row whispers
  at rest. Retranscribe runs Full HQ / Cloud on `last_session.wav`. Forest
  glass drinks the desktop; the panel stays non-key until you click FINAL.

### Changed

- **Release signing preserves the user's Keychain domain.** The release lane
  snapshots/restores the exact search list and default keychain, never borrows
  a temporary build keychain as the user's lasting default, and diagnoses
  stale/deleted keychain paths before signing.
- **Cloud and local live observations are explicit lanes.** Apple remains the
  instant canvas; local or provider Layer 1 can contribute bounded evidence,
  while file Retranscribe remains a separate operator action.

- **Whisper idle is 30 minutes after the last finished decode**, not 60
  seconds from load. The running process only picks this up after
  `install-app` + relaunch.
- **Settings matches the live STT contract.** Dictation owns the ASR mode
  picker (`apple_only` / `local_power` / `cloud`) and writes
  `CODESCRIBE_ASR_MODE` plus explicit Cloud consent. Final pass is no longer
  an engine control. Active STT is the last serving take (`local_apple` →
  Apple). `STT_ENDPOINT` is the live WSS socket on Dictation. Retranscribe
  toasts the real error, including a missing `last_session.wav`.
- **Quality HTML is Seal Atlas.** `codescribe-corpus` writes
  `quality/seal-atlas.{profile}.html` as the report (handshake in
  `docs/quality-reports/CONTRACT.md`). Qube scores move to
  `quality/qube.{profile}.html` and stay a footnote. Gold take 01 remains
  `docs/quality-reports/seal-atlas.take01.html`.

## [0.13.3] - 2026-08-13

> The agent-stability and STT-truth-layer wave: one dictation pipeline with an
> editable transcript as the source of truth, a hardened agent substrate
> (native tools, workspace-roots sandbox, permission gateway), licensing (CSK1),
> Sparkle 2 signed updates, consent-gated analytics, and a fail-closed release
> lane. Rolls up PR #65 (operator feedback wave 9) and PR #68, plus the
> tail-patch wave that made the live Whisper correction lane actually deliver.

### Added

- **Unified dictation pipeline** (#65) — hold-to-dictate and toggle modes share
  one capture-to-editable-transcript path; any user edit to the transcript
  cancels assistive auto-send, and delivery goes through a deferred-insert slot
  with an exact-target paste guard and a visible clipboard fallback instead of
  writing the clipboard immediately.
- **Native agent substrate** — `read_file`/`list_directory`/`search_files`/
  `write_file`/`apply_patch`/`move_path` as native tools bounded by persisted
  workspace roots (canonicalized, symlink-aware, fail-closed on empty roots),
  so the agent works without the IntelliJ connector; oversized tool output
  spills to disk instead of flooding the thread.
- **Agent chat surface** — durable FIFO turn queue with shell-first lazy
  bootstrap, recoverable sidebar with queued-message rows, queued messages are
  editable and recallable, reasoning summaries are exposed, rich-formatted text
  is selectable, and the arbitrary 25-iteration agent loop cap became an
  explicit loop guard.
- **Licensing (CSK1)** — Ed25519 offline-tolerant license validation in core,
  Keychain-backed storage with a Settings UI, a soft gate on the paid Agentic
  lane (Basic stays free), and a fail-closed production key contract at build
  time.
- **Sparkle 2 signed updates** — in-app updates with a signed-appcast pipeline;
  release automation injects the Sparkle key and publishes a single-variant
  appcast.
- **Consented funnel analytics** (default **off**) — a single activation ping
  gated on explicit opt-in; the endpoint ships empty.
- **Settings capability matrix** — one surface listing every agent tool with
  origin, risk class, and the effective allow/ask/deny resolution from the
  permission gateway.
- **Site trust pages** — privacy/security/imprint pages with footer and in-app
  links.

### Changed

- **Whisper residency is bounded and observable** — the normal idle-weight TTL
  is now 60 seconds (one minute), while `CODESCRIBE_WHISPER_IDLE_UNLOAD_SECS=0`
  remains the explicit power-user keep-warm override. INFO lifecycle events now
  expose the effective TTL plus load/unload/reclaim counts and durations without
  logging audio or transcript content. Host `vmmap` reclaim remains a release
  acceptance measurement, not a unit-test claim.
- **Engine warnings are classed** — only `transcription_failed` reaches the UI
  as a user-terminal error; routine quality receipts (overlap normalization,
  under-commit, VAD degradation, backpressure) are log-only. Guarded by
  `warning_is_user_terminal` in the pipeline contracts and a bridge-side test.
- **Assistive capture ownership** — assistive capture and agent-window controls
  unified under one owner; the capture contract is documented in `AGENTS.md`.
- **Hold dictation is always raw** (#65) — the detector-level force-AI chord on
  hold (Ctrl+Option) was removed with the unified pipeline: a plain hold now
  pastes the raw transcript regardless of the Auto-Format toggle. AI formatting
  lives in toggle sessions, the double-Option force chord, and the assistive
  lanes. Previously this change shipped undocumented, so Ctrl+Option hold users
  saw an unexplained drop in delivered-text formatting.
- **Release lane is fail-closed** — DMG payload verification gate in the
  release workflow (a DMG missing its model payload aborts the release),
  single version truth across Cargo/site/appcast, 404-proof Pages deploys, and
  a secrets runbook.

### Fixed

- **Live Whisper tail patches finally land** — the correction lane compared
  tokens character-for-character, so the Apple+lexicon canvas (casing,
  punctuation) never matched Whisper's bare lowercase and every healthy
  sentence read as wholesale divergence (a month of 116 counted, 0 applied
  corrections). Tokens now align on words via a casefolded, edge-punctuation-
  stripped key (diacritics stay significant); matched tokens keep the canvas
  casing and substitutions carry the canvas trailing punctuation. A
  substitution-shaped small-edit floor (≤3 tokens) stops the relative
  change-ratio gate from starving short utterances. Measured after the fix:
  147 applied / 99 skipped across 12 sessions in one night.
- **Tail-patch lane is observable per session** — every finalisation logs a
  `tail_patch_session_receipt applied=X skipped=Y` INFO row, and a session
  that rejected every patch (≥3 skips, 0 applied) raises a
  `tail_patch_lane_starved` WARN instead of dying silently.
- **Quality receipts no longer kill the dictation UI** — a routine engine
  warning during recording used to paint "Dictation stopped", reset the UI
  without stopping the engine, and leave an orphaned live microphone stream
  (hot mic at tray Idle). Receipts stay off the error channel, and the error
  handler now always stops the recorder before reporting failure.
- **Explicit To Agent delivers even after the session context expires** — the
  runtime thread is re-created instead of dropping the user's dictated turn.
- **Overlay Insert no longer pastes back into Codescribe itself** — the overlay
  is a non-activating panel that can hold the caret (editable FINAL) while
  another app stays frontmost, so the synthetic Cmd+V followed OUR key window
  and the transcript landed in the overlay instead of the target (reported
  live against alacritty). The Insert action now runs a caret-truth guard: when
  a Codescribe text view is first responder, it degrades to copying the tagged
  transcript (`<codescribe mode="dictation" ...>`) to the clipboard and says so
  in a toast. A second, controller-side guard degrades the same way when the
  paste target never took focus back (activation failure / Automation TCC
  denial) instead of pasting blind, and the bridge now reports the honest
  delivery outcome (`Pasted` / `CopiedToClipboard`) to the UI.
- **Dictionary corrections read raw STT truth again** — the corrections view
  compares against the raw transcript instead of post-formatting output, so the
  lexicon learns from what the engine actually heard.
- **Unbound-key sentinel no longer reads as Fn** (#66) — an unset hotkey could
  match the Fn detector and trigger recording.
- **Legacy sessions render without window collapse** — old thread payloads
  hydrate into the chat window instead of collapsing it.
- **Oversized bubbles leave the selection overlay** — very large messages no
  longer trap the text-selection layer.
- **CI compiles again on self-hosted runners** — the Rust workflow put the
  actual rustup toolchain bin on `PATH` instead of a proxy-less `CARGO_HOME`,
  so Clippy + Tests prove the workspace for real.

### Security

- **Secret-path approval escalation** — a read-level Allow (default or
  operator rule) no longer silently covers credential-bearing paths: `.env*`,
  `mcp.json` (and backups), `tool_grants.json`, key material (`.pem`, `.p12`,
  `id_rsa*`, keychain DBs) and `~/.ssh`/`~/.aws`-style directories now always
  raise an approval card showing the exact path.
- **`mcp.json` written `0o600`** — the Settings MCP store now creates its
  atomic-write temp file owner-only before any byte lands (parity with the
  secret-migration writer), and a mutation tightens a pre-existing
  world-readable config.
- **Terminal policy fail-closed** — shell control/expansion operators blocked,
  interpreters and command launchers denylisted, relative and `~` argv path
  tokens sanitized and root-checked (review P1-05).

## [0.13.0] - 2026-07-19

> Voice→agent delivery stabilization (assistive history continuity, AI titles
> for voice threads, one delivery gateway), overlay reform with reversible
> formatting levels, the agent-surface wave (summon, stop, cancel), a Settings
> information-architecture reorganization, and release hygiene.

### Fixed — voice→agent delivery (night shift 2026-07-19)

- **Assistive conversations no longer lose history between turns** (`e0f2a3a`) — the agent runtime used to drop its thread identity and in-memory history whenever it recovered from a degraded state, so the next voice turn silently started a brand-new session (`messages=1` on the wire) and the previous exchange was orphaned. Thread identity now lives above the runtime, and recovery rehydrates persisted history back into the session (explicit `rehydrated` / `rehydrate_empty` / `rehydrate_failed` logs) instead of minting a fresh thread.
- **Voice threads now get AI-generated titles** (`75d986c`) — first-turn title generation used to fire only for composer-typed messages; dictated threads fell back to a raw text slug (visibly broken for prompts starting with boilerplate). The same out-of-band stateless title coordinator now serves both sources with identical race/cancellation semantics, and never re-sends the conversation itself.
- **Thread rail bucketing symptom** ("today 23:59" listed under _Older_) resolved by the identity fix above — turns land in the thread the rail is watching, so `updated_at` refreshes correctly. The section calculator itself was verified correct and left untouched.

### Changed — architecture and Settings IA

- **One canonical thread-delivery gateway** (`a59c466`) — voice and composer persistence were two independent implementations (duplicate upsert/title/summary/timestamp logic in the app controller and the FFI bridge). They are now a single `ThreadDeliveryGateway` in core returning a measured delivery receipt; ~400 lines of duplicated logic removed, custom-vs-generated title semantics and cancellation behavior preserved under tests.
- **Settings tabs reorganized around clear ownership** (`49f2bdc`) — **Dictation** (formerly Engine: STT engine, layered transcription, preview timing moved in from Voice Lab, hands-free silence moved in from Audio) · **Audio** (input hardware and sound feedback only) · **Dictionary** (formerly Voice Lab: the text lexicon — recent corrections and learned rules, no audio/timing settings) · **Providers** (LLM lanes, API keys, agent status, MCP, workspace roots). Settings keys on disk are unchanged — relocated controls read and write the same `settings.json` entries as before.

### Added — overlay reform and agent surface (2026-07-18 wave)

- **Formatting levels as runtime truth** (`ef50b28`) — Off → Correction → Smart → Max with per-level prompts editable in Settings → Prompts.
- **Overlay controls** (`241c549`) — durable Auto Paste toggle, one-shot Format menu, and an explicit **To Agent** action in the transcription overlay.
- **Reversible formatting** (`9552c13`) — one-slot exact-bytes **Revert** with a 5-second re-arm window; quality learning is evidence-only on Smart/Max.
- **Tray parity** (`65bd4f4`, `0755522`) — Auto Paste and cycling Auto Format directly in the tray menu, reading and writing persisted settings truth.
- **Agent summon** (`79f48cb`) — a global hotkey summons the idle agent window.
- **Composer stop + safe voice cancel** (`21e6214`, `f743cf3`) — active agent responses can be stopped mid-stream; voice-assistive turns cancel without corrupting the session.
- **First-turn AI thread titles, composer path** (`d7aba0e`, `4118784`) — stateless generation contract plus orchestration after the first exchange.
- **Exactly-once auto-paste** (`b63809f`) — delivery hardened with fence-window duplicate suppression.
- **Livelier waveform meter** (`662ad1f`) — tighter dB window (−55…−25 dBFS) with a perceptual response curve, so ordinary speech visibly moves the bars.
- Astro site transplanted into the working branch; legacy landing retired (`b4bbfe5`).

### Added

- **API key liveness probe in Settings → Keys** (PR #50) — per-key **Test** button runs a background probe and shows a result chip (`Key OK` / `Invalid key` / `No credits (check billing)` / `Network error` / `Not set` / `Unsupported`). LLM keys are probed with a minimal generation request rather than an auth-only endpoint, so exhausted billing (`insufficient_quota`) is distinguished from an invalid key.
- **Guided MCP onboarding + reset app data** (PR #52) — fresh installs with no `mcp.json` now get a short explainer with **Set up MCP servers** (deep-links into Settings → Engine) and **Skip for now**, instead of a dead-end wall; Settings → Engine gains a danger-zone **Reset app data…** action with a two-step destructive confirmation and an opt-in checkbox to also remove API keys from the Keychain.

### Changed

- **Public release hygiene** — release packaging, repository metadata, and public-facing docs aligned; `v0.12.3` shipped as a notarized DMG on GitHub Releases (2026-07-18) with the site install page pointing at `releases/latest`.
- **Dual DMG release variants** — release automation now builds a standard notarized DMG with embedded Silero + embedder and runtime Whisper cache/download, plus a `_full` notarized DMG with Whisper embedded.
- **Memory footprint** — idle RAM cut from ~5 GB (peak ~10 GB) to ~0.8 GB. The Whisper and MiniLM embedder models now unload from GPU/host memory after a period of inactivity and reload transparently on next use (`CODESCRIBE_WHISPER_IDLE_UNLOAD_SECS`, `CODESCRIBE_EMBEDDER_IDLE_UNLOAD_SECS`, default 300s, 0 disables).

### Fixed

- **Legacy fallback threads persisted on AI failure** (PR #51) — when the AI runtime is unavailable, the legacy assistive fallback no longer persists a conversation thread for `Failed`/`Skipped` attempts (previously created repeated "AI Failed" junk threads cluttering the history rail); `Applied`/`AiNoop` outcomes are still persisted as before.
- **MCP setup prompt never appeared** (PR #53) — `probe_mcp_status` now reports a `configured` flag so onboarding can tell "no `mcp.json`" apart from "servers configured," fixing the guided MCP setup prompt from PR #52 that was dead code because the row-count check was always true.
- **MCP setup deep-link did nothing** (PR #54) — the onboarding "Set up MCP servers" action now opens Settings via the SwiftUI `openSettings` environment action instead of a dead AppKit `showSettingsWindow:` selector that has no responder in this accessory (LSUIElement) app.
- **Silero VAD reload leak** — the Silero ONNX session is now compiled once and shared process-wide instead of being rebuilt per recording (which leaked native ORT memory over long sessions).
- **Allocator retention** — freed transient buffers are returned to the OS after each recording (`malloc_zone_pressure_relief` on macOS) instead of inflating the resident footprint across a session.

## [0.12.3] - 2026-07-16

> Audit-close patch line for lane-truth configuration, Settings parity, and the assistive/chat render contract.

### Added

- **U1 canonical lane-truth snapshot** (`860e490`) — `LaneTruthSnapshot` is exposed over UniFFI as the single source of provider/endpoint/model/credential truth; the duplicate Swift-side resolver (hardcoded model ids) was removed.
- **U3 Composer Cmd+V** (`fb9f2ff`, `31a64c3`) — pasting into the agent composer now stages Finder images, screenshots, and text as attachments (pure `pasteDisposition` routing, window/focus-scoped NSEvent monitor).
- **U4 Settings truth surface** (`bbfb72f`) — the rail no longer renders inert fake buttons, the footer computes `healthy/degraded/offline/unknown` from live signals (with a jump to the failing section), and User became a real local-first panel.
- **U5 chat stream cost cut + Latest pill** (`d2b73e1`) — per-delta-tick Markdown re-parse eliminated (measured 4555 µs → 0.1 µs per tick on a 20k stream), scroll signature 137 µs → 2.3 µs, plus a floating "↓ Latest" pill and a per-bubble raw↔rich toggle. Streaming and final bubbles share the same **raw-default** render policy (C2b).
- **U7 Voice Lab on the quality loop** (`2f7f920`) — live recent overlay corrections and custom-lexicon entries (read via new bridge surfaces, never raw file reads from Swift) plus preview-timing presets (Smooth 1038/10.6/5/8.0 as recommended default, Snappy, Relaxed, Off, Custom with tolerant detection).
- **U8 Audio panel** (`1d6c386`) — live input-device enumeration with honest runtime resolution (saved wish vs. live device shown explicitly), silence/feedback controls mapped only to keys the runtime consumes, dedicated unset-based reset; the rail has no `comingSoon` placeholders left.
- **D4 ThreadRail sections and metadata** (`0709f4b`) — Today/Yesterday/This week/Older grouping (search filters first, then grouping) and a nil-safe `relative time · model · tokens` meta line; the thread index gained the needed fields additively via a versioned rebuild.
- **D6 overlay Paste + failure marker** (`c207ac9`) — the formatted overlay gained a [Paste] action that delivers the user-edited text to the previous app through the single controller delivery path (tagging included), and formatting failures set a discrete state marker while copy/paste/send keep clean text.
- **U12 recoverable reset safety** (`5ea8502`) — full app-data reset moved to **User → Danger zone**, requires typing `RESET`, previews the affected recordings/threads/bytes, moves data to **Trash**, and writes an external append-only audit log. MCP recovery is now a separate **Clear MCP configuration…** action that moves only `mcp.json` to Trash.

### Changed

- **U2 optional-override mutation contract** (`43e50ad`) — single and batch settings writes share one `apply_optional_override` helper, so batch saves can no longer persist `Some("")` and silently blank promoted lane keys; reset means unset.
- **U10 lane-truth documentation rebuilt** (`2df8493`) — `docs/lane-truth.md` (lanes, precedence, key-optional locals, reset=unset, endpoint normalization, probe vs. agent-gate diagnostics), `docs/ENV_REGISTRY.toml` lane coverage, secret-safe examples across docs, and this 0.12.3 changelog entry.
- **U16 env templates = registry truth** (`7666d94`) — `.env.example` and `.env.debug.example` now mirror `docs/ENV_REGISTRY.toml` exactly (183 keys, commented, grouped, `<your-key>` placeholders only); template-only ghost keys were removed and live keys missing from the registry were added.
- **U13 Settings rail labels + layout** (`6a00398`, `5415e7e`) — rail labels now read **Hotkeys** and **Providers** (user-facing strings only; internal identifiers unchanged), and the navigation stack is flush-top instead of floating in unused vertical space.
- **Overlay CloseDot** (`5415e7e`) — the orange wordmark dot is now a dedicated close control with a traffic-light hover state, the existing close path, and an accessible label; the shared decorative wordmark remains non-interactive elsewhere.

### Fixed

- **U14 MCP resilience — SIGPIPE root cause** (`a35a64b`) — a dead-at-exec MCP server could kill the whole app silently: writing `shutdown` to the dead child's stdin raised SIGPIPE, which is ignored in Rust binaries but fatal (and unreported by ReportCrash) inside the Swift-hosted dylib. Fixed with per-fd `F_SETNOSIGPIPE`, a `try_wait` guard before farewell writes, a dedicated 5 s `initialize` timeout, and parallel per-server discovery isolation; a falsification test reproduces the death (signal 13) without the fix.
- **U15 tray toggle truth** (`c98201c`) — after writing `transcription_overlay_enabled`, the tray re-reads the bridge/settings source of truth so its On/Off label cannot remain on an optimistic stale value.
- **U15 OpenAI restored-image guard** (`5f49f56`) — byte-less `tool_result` images now warn-skip instead of serializing an empty data URI or image reference.
- **U11 model-discovery cancellation** (`66e123f`) — a new discovery generation now aborts the previous in-flight HTTP fetch per provider (proven by an `expect(0)` mock: the cancelled request never reaches the wire) and a stale generation can no longer overwrite the cache.
- **D8 Anthropic image-asset parity** (`41c14de`) — tool-result images restored with `data_omitted` warn-skip instead of silently serializing (with a text fallback keeping the block valid); ImageAsset bytes load from disk at request time only, matching the OpenAI provider contract.
- **U6 quality-chain proof** (`ad4b06d`, tests only) — a hermetic end-to-end test proves the operator promise at engine level: an overlay edit becomes a `QualityRecord`, a lexicon candidate, a custom-lexicon commit, and a corrected next transcript.
- **D-01 hands-off toggle ADR annotation** — `HOTKEYS_CONTRACT.md` records that commit `37f137e` reverted the 2026-05-28 force-RAW toggle decision and restored Settings-driven default routing when no explicit hotkey override exists.

## [0.12.2] - 2026-06-22

> Public-readiness patch line for the assistive/dictation stack. This release keeps the `0.12.x` product shape but hardens the user-visible paths that made private builds feel finished while public releases lagged behind.

### Added

- **Tray startup readiness** — the tray now surfaces startup readiness instead of silently appearing idle while core runtime checks are still settling.
- **Pending follow-up preservation** — voice follow-ups survive finalization instead of being dropped as the recording state clears.

### Changed

- **Voice chat drawer I/O** — card disk operations moved off the main thread to reduce AppKit stalls in the assistant drawer.
- **Onboarding focus behavior** — onboarding stays visible without relying on always-on-top window behavior, and it refreshes when permission state drifts.

### Fixed

- **Assistive message duplication** — the first assistant message renders once instead of double-sending or double-rendering.
- **Raw recording final-pass truth** — raw stops require the correct final-pass behavior instead of silently mixing paths.
- **Dictation lexicon** — preserves Loctree/Vibecrafted vocabulary during dictation cleanup.
- **Settings shortcut copy** — removed fake shortcut customization affordances that did not map to runtime behavior.

## [0.12.1] - 2026-06-13

> Patch release for the editable overlay and assistive transcript handoff.

### Added

- **Editable dictation overlay output** — overlay results can be edited before downstream actions.
- **Audio archive as m4a blobs** — recordings can be retained in a smaller archive format.
- **Native `transcribe_audio` agent tool** — assistant tooling can transcribe an audio file through the same core STT path.

### Fixed

- **Toggle-to-voice-chat handoff** — finalized utterances append into the voice chat session instead of losing most of the spoken session.
- **Overlay button routing** — each overlay action maps to its own command path.
- **Drawer/settings layout clipping** — drawer rows and settings sections were tightened to avoid clipped content.

## [0.12.0] - 2026-06-12

> Minor release for public-source licensing, MCP bridge work, and the modern assistive UI surface.

### Added

- **Stdio MCP tool bridge** — Codescribe can load configured MCP tools and report MCP status honestly.
- **Thread search agent tool** — assistant tooling can search saved thread history.
- **Creator taxonomy shell and preview timing presets** — settings gained richer controls for creator workflows and live-preview cadence.

### Changed

- **License** — relicensed the public codescribe release surface from Apache-2.0 to FSL-1.1-ALv2 to support public availability while protecting against commercial repackaging; each version converts to Apache-2.0 after 2 years.
- **Voice chat UI** — modernized drawer rows, preserved raw markdown bubbles by default, and reduced streaming render cost.
- **UI module shape** — decomposed large settings, voice chat, onboarding, overlay, pipeline, hotkey, and shared-helper surfaces into responsibility modules.

### Fixed

- **Screenshot/asset safety** — agent screenshots are stored as bounded image assets instead of oversized inline payloads.
- **Overlay editability** — format results remain editable and pasteable through the overlay action contract.

## [0.11.2] - 2026-05-28

> Stabilization line for the hands-off transcript path and assistive runtime.

### Added

- **Thermal STT governor** — local transcription can back off under thermal pressure.
- **Build hash telemetry** — About/version surfaces include a short build hash for support and release diagnosis.

### Fixed

- **Hands-off continuous session** — toggle dictation is one continuous session: append utterances, retain audio, and send one assistant message.
- **Toggle-stop watchdog** — added protection against stuck toggle-stop states.
- **Chat overlay input stability** — restored interactive overlay behavior after floating-window focus regressions.
- **Agent stream and SSE robustness** — improved event parsing, retry behavior, and chain reset diagnostics.

## [0.10.0] - 2026-05-06

> Minor release. Embedded VAD contract hardened (zero-IO production path), legacy path-based VAD API hidden, several deprecated transcription/quality surfaces removed. Includes onboarding TOCTOU fix and AppKit overlay teardown contract completion.

### Breaking changes

- **Removed deprecated transcription helpers** — `transcribe_long`, `transcribe_long_with_language`, and the `transcribe_file(&Path, Option<&str>) -> Result<String>` shape are gone. Callers must migrate to the typed `TranscriptionVerdict` surface.
- **Removed `pub const DEFAULT_MODEL` from `core/stt/whisper/singleton.rs`** — re-exported from `core::config::models` instead. Update imports accordingly.
- **Removed `QualityLoopConfig`, `QualityDaemonState`, and `mark_daemon_unavailable`** from the quality public surface. Replaced by the `qube_lifecycle` subsystem.
- **Renamed quality daemon state type** — `read_daemon_state` and `write_daemon_state` now return `QubeDaemonState` instead of `QualityDaemonState`.
- **Hidden legacy path-based VAD loaders** — `SileroVad::new(&Path, ...)` and `AccumulatingVad::with_config(&Path, ...)` are now `#[doc(hidden)]`. Embedded path is canonical via `AccumulatingVad::new(sample_rate)`. The path-based shape is retained only for dev/test overrides.

### Added

- **Embedded Silero VAD as production default** — `RecorderVad` now goes through `AccumulatingVad::new(sample_rate)` (embedded blob via `commit_from_memory`), eliminating the disk-path fallthrough that disabled auto-silence on fresh machines. Regression-locked by new unit test `embedded_vad_loads_without_disk_file`.
- **`TranscriptionVerdict` typed truth surface** — replaces ad-hoc `Result<String>` shape across the transcription boundary; carries confidence flags and adjudication state explicitly.
- **`qube_lifecycle` subsystem** — supersedes the removed `QualityLoopConfig`/`QualityDaemonState` surface with a coherent state machine for daemon lifecycle (start/stop/health probes).

### Fixed

- **TOCTOU lock in onboarding** — replaced check-then-create file lock with `flock(2)` to prevent racing first-run setups across simultaneously launched codescribe instances.
- **NSGlassEffectView retain balance** — UI overlay now autoreleases the glass effect view to balance its explicit retain on construction; prevents a steady leak of glass overlays under heavy use on macOS 26+.
- **ObjC release contract on overlay teardown** — completed the `release` pairing for all overlay subviews so teardown does not leak under ARC-incompatible call paths.

## [v0.9.2] – 2026-04-18

> Patch release. Big-ticket items (typed transcription flags, toggle final-pass adjudication, short-text formatting truth guard) hardened from `0.9.1`. L2 config loader rewrite landed in `0.9.2`; the follow-up parity work in `0.9.3` certifies the already-green loader tests and corrects the shipped changelog narrative.

### Added

- **Typed transcription flags + toggle adjudication** ([091dd67](https://github.com/vetcoders/codescribe/commit/091dd67)) — `TranscriptionConfidenceFlag` enum extended; `Vec<String>` confidence flags converted to typed `Vec<TranscriptionConfidenceFlag>` across `RecordingTruthVerdict` boundary. Toggle mode now adjudicates session truth via the same final-pass pipeline as hold mode (no more 80% speech loss in long toggle sessions). Closes Marbles_truth_plan **L9** + research **Q10/LIE A/Q7**.
- **`final-pass` env toggle for runtime experimentation** ([42a09e7](https://github.com/vetcoders/codescribe/commit/42a09e7)) — `CODESCRIBE_LOCAL_STT_FINAL_PASS=0|1` (default `1`) lets ops disable the saved-WAV adjudicator without rebuild. `Vec::contains` cleanup on flag iteration.
- **Centralized env handling + embedded-Whisper documentation** ([fb30db2](https://github.com/vetcoders/codescribe/commit/fb30db2)) — env-var loading consolidated in one path; README + `.env.example` updated to declare embedded-first Whisper as canonical and `CODESCRIBE_NO_EMBED=1` as opt-out.

### Changed

- **Config loader rewrite** ([0a9bd99](https://github.com/vetcoders/codescribe/commit/0a9bd99)) — `core/config/{loader,migrate,mod}.rs` substantively refactored to enforce priority `settings.json > promoted env > defaults`. Lays infrastructure for upcoming Settings Creator. **Test parity** (verified `0.9.3`): both `test_load_prefers_settings_json_over_promoted_env_file_values` and `test_runtime_env_does_not_persist_into_settings_during_migration` pass on this commit. The L1 marble that flagged them was already converged by `0a9bd99` (inject_file_env_for_runtime skips promoted keys) and `43d67d1` (migrate_if_needed early-returns when `.env` snapshot is absent or empty); the CHANGELOG-as-shipped lagged the actual fix state. Functional impact: none.
- **Sort + collapsible match hygiene** (clippy) — `sort_by(|a,b| b.x.cmp(&a.x))` → `sort_by_key(|b| std::cmp::Reverse(b.x))` across `core/agent/thread_index.rs`, `core/quality/qube_daemon.rs`, `app/ui/shared/helpers.rs`, `app/ui/voice_chat/api.rs`. Collapsible `match` → guard pattern in `core/agent/thread_index.rs`, `app/controller/helpers.rs`, `app/ui/voice_chat/api.rs`. Zero behavior change, idiomatic Rust 2024.

### Fixed

- **Short-text formatting truth guard** ([ab9a9c6](https://github.com/vetcoders/codescribe/commit/ab9a9c6) — L1 marble) — non-assistive AI formatting now hard-skips only inputs `<10` chars; `AiNoop` detection narrowed to whitespace-only echoes. Punctuation and capitalization changes are preserved as legitimate formatting work. Short `FormattedTranscript` outputs in the 10–23 char band re-entered the controller quality gate (previously bypassed). Closes regression in `e2e_prompts_and_history`.

### Internal

- **Marbles convergence loops** — L1 codex marble closed `0.9.2` short-text quality gate gap. Config loader parity is now certified green; `0.9.3` closes the documentation lag and adds defense-in-depth regression coverage.
- **Build pipeline parity** — `release-codescribe` (embedded models) + `release-qube` (`CODESCRIBE_NO_EMBED=1`, isolated `target-noembed/`) split preserved from `0.9.1`. DMG slim ~1.3 GB (vs `0.9.0` legacy ~3.7 GB).

## [v0.9.1] – 2026-04-16

> Patch release. **Critical Silero VAD fix for fresh-machine deployments** + DMG size optimization via build-pipeline split.

### Fixed

- **Silero VAD embedded path** ([8b0e278](https://github.com/vetcoders/codescribe/commit/8b0e278)) — Silero ONNX model was embedded in the binary via `include_bytes!`, but runtime called `Session::commit_from_file(path)` against `~/.codescribe/models/silero_vad.onnx` which doesn't exist on fresh machines. Result: every recording on freshly-installed `0.9.0` DMG returned `vad_no_speech_detected`, regardless of audio content. Fix: new `SileroVad::new_embedded(config)` and `AccumulatingVad::with_config_embedded` use `Session::builder().commit_from_memory(embedded::MODEL)` (ort 2.0.0-rc.11 API). `core/audio/chunker.rs::init_silero_vad` rewired to embedded path; legacy `SileroVad::new(model_path, ...)` kept as dev/test override only. Verified empirically against a real-device `Sesja 1` recording (53-char Polish transcript with 57% speech detected vs prior 0% speech under `0.9.0`).

### Changed

- **Slim DMG via build-pipeline split** — `Makefile` target `release` split into `release-codescribe` (embedded Whisper + MiniLM + Silero) and `release-qube` (`CODESCRIBE_NO_EMBED=1`, isolated `target-noembed/` directory). `qube-daemon` and `qube-report` binaries shrank from ~1.3 GB each (each had its own `include_bytes!()` baked-in models — Cargo doesn't deduplicate `__DATA` segments across workspace binaries) to **24 MB each**, resolving runtime models from HF cache instead. Bundle dropped from **4.0 GB → 1.4 GB**, signed+notarized DMG from **3.7 GB → 1.2 GB** (~67% reduction). `qube-*` binaries continue to function as vetcoders-internal CLI tools without per-binary model embedding overhead.
- **`.gitignore`** — added `target-noembed/` (build-pipeline-split workspace artifact directory).

### Internal

- **Notarytool credentials profile** documented — `xcrun notarytool store-credentials VSNotary --apple-id ... --team-id MW223P3NPX --password ...` is the required one-time setup for signed DMG release pipeline.

## [v0.9.0] – 2026-04-16 (PR #26 — `feat/the-intents-engine`)

> Version bumped from `0.8.1` → `0.9.0` to truthfully signal the breaking changes below (SemVer pre-1.0 minor bump). Release tag remains on this PR.

### Breaking

- **CLI binaries renamed** – `codescribe-quality` → `qube-report`, `codescribe-loop` → `qube-daemon`. External launchd plists, cron entries, and shell scripts must be updated. Install targets (`make install`, `make bundle`) now ship the renamed binaries.
- **Public API removals in `codescribe-core`** – `stt::whisper::singleton::transcribe_file(path, language) -> Result<String>` was removed outright. `pub const DEFAULT_MODEL` is preserved as a re-export from `config::models`. Callers migrate to `stt::whisper::singleton::transcribe_file_verdict(path, language, FileTranscriptionOptions)` returning `TranscriptionVerdict`.
- **Quality daemon state type** – `QualityDaemonState` renamed to `QubeDaemonState` across the public surface.

### Added

- **Truth-surface adjudication** – New `RecordingTruthVerdict`, `RecordingTranscriptSource`, `RecordingFallbackClass`, `FinalPassVerdict`, `VadVerdict` structs replace silent degradation with explicit verdicts. Controller and overlay now render truth flags (`truth_review_trigger`, `truth_display_status`, `push_truth_flag`).
- **File transcription verdict-first** – `transcribe_file_verdict` exposes provenance (embedded vs. runtime, VAD sparkline preservation, final-pass artifact rejection).
- **Assistive preview mode + context cache** – Double-tap Right Option now engages assistive mode with a preview window and LLM context chaining.
- **Veterinary seed + lexicon variants** – Expanded Polish veterinary corpus assets in `core/assets/`.
- **Qube protocol CLI alignment** – `qube-report` / `qube-daemon` binaries and `QUBE_DAEMON_AUTOSTART` settings flag.

### Changed

- **Runtime model resolution hardened** – `resolve_runtime_whisper_model_path` clarifies precedence (`CODESCRIBE_MODEL_PATH` → bundled Resources → `../../models` → `~/.codescribe/models` → HF cache) and `canonicalize_or_self` now logs a warning on canonicalization failure instead of silently swallowing the error.
- **Embedded-first Whisper remains canonical** – Release builds embed the Whisper payload by default; runtime resolution is the opt-in fallback (`CODESCRIBE_NO_EMBED=1` or missing model). README updated to reflect this truth.
- **Settings JSON migrations** – `qube_daemon_autostart` promoted to the v2 `system` section; legacy settings continue to load via alias.
- **Overlay live-preview stability** – New `CODESCRIBE_OVERLAY_STABLE_PREVIEW` env flag gates stable-word-boundary trimming in live mode (default off).

### Fixed

- **Overlay unit tests isolated** – `test_overlay_visible_text_live_mode_defaults_to_exact_text` / `..._decision_mode_uses_exact_text` now use `#[serial]` + a scoped `OverlayStablePreviewEnvGuard` so sibling tests cannot pollute `CODESCRIBE_OVERLAY_STABLE_PREVIEW`.
- **`rustls-webpki` bumped to 0.103.12** – Addresses RUSTSEC-2026-0098 and RUSTSEC-2026-0099 (name-constraint handling for URI names / wildcard certificates).
- **Env-mutation `unsafe` blocks in `core/config/loader.rs` / `core/config/models.rs`** now carry `// SAFETY:` justifications documenting the single-threaded init invariant per Rust 2024 norms.
- **Quality daemon autostart surface** – The settings toggle label/description now tells users truthfully that the tray app does not spawn the daemon; external `qube-daemon --daemon` is required.

### Internal

- **Tray handler** – Notification text now points users to `qube-daemon --daemon` when no quality report is available.
- **Historical ADRs annotated** – `docs/ADR/2026-01-*` and `docs/future/FEASIBILITY_ANALYSIS.md` now carry historical-snapshot disclaimers explaining path drift after the `ui/` refactor and CLI rename.

## [v0.7.14] – 2026-02-07

### Added

- **Settings window (Bootstrap)** with tiered config (settings.json) + Keychain-backed API keys.
- **Fn-first hotkeys** (Globe/Fn as default hold modifier) with Shift/Cmd modifiers for Chat/Selection.
- **Configurable double‑tap interval** and **toggle silence auto‑send** (hands‑off UX).
- **MiniLM embedder** (paraphrase‑multilingual‑MiniLM‑L12‑v2) embedded by default for lightweight semantic gating.
- **Model caching in `make install-app`** (Whisper + embedder auto‑download if missing).

### Changed

- **Default hotkeys** → Hold `Fn` + double‑tap `Option` (left=normal, right=assistive).
- **Buffered streaming default** for smoother live transcription display.
- **Token limits default to 0** (API decides) to avoid truncation.

### Fixed

- **UTF‑8 slicing panic** in streaming overlap (diacritics/emoji safe).
- **Toggle streaming append** now keeps a single bubble per session (no spam bubbles).
- **Overlay header controls** restored on top of split view.
- **Bootstrap deadlocks** removed by shortening lock scopes during UI build.

## [v0.7.2-dev] – 2026-01-20

### Added

- **Hands-off Chat Overlay** – Full chat interface in overlay with history, user/assistant roles, and input field.
- **Persistence** – Chat history is preserved between sessions; messages do not disappear on close.
- **Auto-send Toggle** – UI checkbox to control automatic sending vs. draft mode.
- **Improved VAD** – 5s timeout for hands-off mode to allow for pauses; short silences (1-2s) are ignored.
- **Tray Actions** – Added "Show Chat Overlay" and "Copy Last to Clipboard" to tray menu.
- **UI Improvements** – Input field at top, reversed message flow (newest first), selectable text for copying.

### Fixed

- **Quality Gates** – Resolved `cargo check` and `make check` warnings; improved code quality.
- **Reliability** – Fixed issue where overlay would reset state unexpectedly.

## [v0.7.0] – 2026-01-17

### Added

- **Strict Embedded Policy** – Whisper model is always embedded into release binary. Zero external model files, zero exceptions.
- **IPC server** – New IPC server and message types for stable runtime integration.
- **Quality loop** – Automated transcription quality assessment loop.
- **Quality report** – Batch quality report generator with WER/CER metrics.
- **Stream postprocess** – Semantic gating and stream cleanup in live pipeline.
- **New CLI tools** – `codescribe-quality`, `codescribe-loop` for quality management.
- **serial_test** – E2E test serialization to reduce race conditions.

### Changed

- **Version unification** – Consistent versioning across the project.
- **Security hardening** – `cap-std` and file operation restrictions to allowed paths only.

### Fixed

- SSE formatting and final text collection fixes.

## [v0.6.3] – 2026-01-16

### Added

- **New hotkey architecture** – Each hotkey now determines the processing mode:
  - **Ctrl Hold** = ALWAYS RAW (fast dictation, no AI processing, ignores AI toggle)
  - **Double Option** = respects AI_FORMATTING_ENABLED toggle setting
  - **Ctrl+Shift Hold** = ALWAYS Assistive (AI assistant mode)
- **Triple-tap Option** – Quick toggle for AI Formatting (shows toast notification)
- **Shift upgrade mid-hold** – Adding Shift during Ctrl hold upgrades to Assistive mode
- **KURIER/ASYSTENT prompt system** – Adaptive system prompts that detect user intent:
  - KURIER: Pass-through mode for dictation (zero commentary)
  - KURIER+REDAGUJ: Dictation with light editing on explicit request
  - ASYSTENT: Full AI assistant mode for questions/help
- **SSE streaming by default** – OpenAI/Libraxis endpoints now use SSE streaming for
  immediate handshake and no timeout issues

### Changed

- **Timeout increased to 90s** – GPT-5.x with longer inputs needs more time
- **Token limits removed** – All token limits set to 0 (API decides). Tokens are cheap,
  lost notes are not.
- **force_raw_mode flag** – New controller state flag for explicit RAW mode override

### Fixed

- **Timeout issues with GPT-5.2** – Streaming mode eliminates 30s timeout failures

## [v0.6.2] – 2026-01-16

### Added

- **Whisper Live (streaming transcription)** – Local transcription now happens _during recording_.
  Audio from the CPAL callback is chunked and processed in the background, so on `stop()` we only
  finalize the last chunk for near-instant time-to-paste.
- **StreamingRecorder** – New streaming capture/transcription pipeline built around a non-blocking
  channel from the audio callback, plus overlap + deduplication between chunks.
- **DMG packaging improvements (embedded-only)** – Release packaging is now aligned with the
  embedded-model strategy (no bundling `Resources/models/*` that would duplicate ~900MB).

### Changed

- **Docs & pitch** – Documentation and README now highlight the core differentiator: embedded Whisper
  - live streaming transcription.

## [v0.6.1] – 2026-01-14

### Added

- **Model embedded in binary** – Release builds now include the Whisper model directly via
  `include_bytes!`, eliminating runtime model loading and disk I/O. Binary size ~888MB with
  model welded in. Debug builds still use external model path.
- **Provider separation** – New `LLM_{FORMATTING,ASSISTIVE}_{ENDPOINT,MODEL,API_KEY}` convention
  allows different LLM providers for formatting (Ctrl hold) vs assistive mode (Ctrl+Shift hold).
- **Keep Audio toggle** – Added "Keep Audio" option to History submenu for enabling/disabling
  paired `.wav` + `.txt` storage on the fly.
- **Slug in filenames** – Transcription and audio files now include first 3 words as slug for
  easier identification: `2026-01-14_12-30-00_hello-world-test.txt`.
- **Whisper singleton API** – `whisper::singleton::init()` and `transcribe()` for shared model
  instance with automatic embedded vs external path resolution.

### Changed

- **Responses API optimization** – Instructions are now sent only on first request; subsequent
  requests rely on `previous_response_id` to preserve context, reducing payload size.
- **Build safety** – Release builds now hard-fail when model is missing. Dev-only: set
  `CODESCRIBE_NO_EMBED=1` to build without embedding (binary will require `CODESCRIBE_MODEL_PATH`
  at runtime).
- **Language enum** – Removed `Auto` variant from `Language` enum; use explicit language codes.
- **Tray menu restructure** – Reorganized submenus for History, Modes, and Settings.
- **Environment schema** – Updated `.env.example` with complete configuration reference including
  provider separation, audio settings, and debug options.

### Fixed

- **Clippy warnings** – Resolved unused imports, dead code, and type complexity warnings.
- **E2E tests** – Fixed `LLM_HOST` → `LLM_ENDPOINT` migration in all test files.
- **Borrow checker** – Fixed move-after-borrow in AI formatting trace logging.

## [v0.6.0] – 2026-01-13

### Added

- **Native desktop UI (Tauri + Leptos)** – Introduced the (now legacy) Tauri frontend with a
  three-tab interface (Voice Lab, Teacher, Settings). ([a275ae8](https://github.com/vetcoders/codescribe/commit/a275ae8),
  [7aa0754](https://github.com/vetcoders/codescribe/commit/7aa0754))
- **Pure Rust local Whisper STT (Metal GPU)** – Added local Whisper inference via
  `candle-transformers` (Metal acceleration), with long-audio chunking + language detection.
  ([268f5d0](https://github.com/vetcoders/codescribe/commit/268f5d0),
  [69ed294](https://github.com/vetcoders/codescribe/commit/69ed294))
- **Whisper decoding controls** – Added `DecodingParams` (mlx_whisper-compatible) including
  n-gram blocking and streaming callback support. ([69574fb](https://github.com/vetcoders/codescribe/commit/69574fb),
  [cc0d8aa](https://github.com/vetcoders/codescribe/commit/cc0d8aa))
- **CLI transcription + E2E pipeline tests** – Added file transcription flows and a comprehensive
  end-to-end pipeline test suite. ([d7bdb4b](https://github.com/vetcoders/codescribe/commit/d7bdb4b),
  [d46c62c](https://github.com/vetcoders/codescribe/commit/d46c62c))
- **Config convenience** – Added `--config` flag to open/create the config file. ([535270c](https://github.com/vetcoders/codescribe/commit/535270c))
- **UX updates** – Added badge modes + Dock icon behavior and tightened environment/API key
  requirements. ([7946c17](https://github.com/vetcoders/codescribe/commit/7946c17))

### Changed

- **License** – Switched the project license to Apache 2.0 and added release scripts/docs.
  ([e0e7ec1](https://github.com/vetcoders/codescribe/commit/e0e7ec1))
- **Backend architecture** – Removed the Python backend and updated the Rust CI pipeline to match.
  ([5c65481](https://github.com/vetcoders/codescribe/commit/5c65481))
- **AI formatting pipeline** – Improved configuration, workflows, and Harmony support; refined
  formatting behavior and defaults. ([e11400c](https://github.com/vetcoders/codescribe/commit/e11400c),
  [8a3157f](https://github.com/vetcoders/codescribe/commit/8a3157f),
  [d46c62c](https://github.com/vetcoders/codescribe/commit/d46c62c))
- **Tray menu + local STT integration** – Refactored tray menu plumbing while integrating the local
  Whisper engine and improving related behavior. ([16021b1](https://github.com/vetcoders/codescribe/commit/16021b1))
- **Local model packaging/loading** – Bundled a default model and updated model loading logic.
  ([13378fe](https://github.com/vetcoders/codescribe/commit/13378fe))
- **Cloud/STT provider work** – Refactored lab assets and migrated cloud provider integration.
  ([8392cb9](https://github.com/vetcoders/codescribe/commit/8392cb9))
- **Configuration consolidation** – Deduplicated configuration to a single source of truth.
  ([217a336](https://github.com/vetcoders/codescribe/commit/217a336))
- **Error handling/refactors** – Refactored Whisper engine imports and adopted `anyhow`.
  ([b9ac5d9](https://github.com/vetcoders/codescribe/commit/b9ac5d9))
- **Repository maintenance** – Restructured the repo and added conversation session tracking.
  ([07fe69f](https://github.com/vetcoders/codescribe/commit/07fe69f))
- **Developer ergonomics** – Applied `cargo fmt`-driven formatting fixes.
  ([f8e04ef](https://github.com/vetcoders/codescribe/commit/f8e04ef))

### Fixed

- **Stability** – Handled poisoned mutexes via `into_inner()` fallback to avoid cascading failures
  after panics. ([b7591ab](https://github.com/vetcoders/codescribe/commit/b7591ab))
- **Backend cleanup** – Ensured backend processes are killed on all known ports.
  ([417b002](https://github.com/vetcoders/codescribe/commit/417b002))

### Removed

- **Cleanup** – Removed unused and deprecated code to keep the build clean.
  ([68469dc](https://github.com/vetcoders/codescribe/commit/68469dc))

### Changed (Internal)

- **Foundations** – Landed the initial Rust-based architecture groundwork.
  ([5a17c3a](https://github.com/vetcoders/codescribe/commit/5a17c3a))

## v0.4.3 – 2025-11-21

- Internal updates.

## v0.4.1 – 2025-11-11

- Internal updates.

## v0.4.0 – 2025-11-11

- **License clarification** – Switched from MIT to BSD 4-Clause.
- **Configurator hardening** – `hardware_detector.py` cross-platform improvements.
- **First-run portability** – Onboarding config improvements.
- **Backend & API hardening** – Robustness improvements.
- **Tooling & packaging** – Packaging script enhancements.
- **CI & types** – Type checking and CI improvements.
- **Menu robustness** – Tray menu stability fixes.

[unreleased]: https://github.com/vetcoders/codescribe/compare/v0.13.0...HEAD
[0.13.0]: https://github.com/vetcoders/codescribe/compare/v0.12.3...v0.13.0
[0.12.3]: https://github.com/vetcoders/codescribe/compare/v0.12.2...v0.12.3
[0.12.2]: https://github.com/vetcoders/codescribe/compare/v0.12.1...v0.12.2
[0.12.1]: https://github.com/vetcoders/codescribe/compare/v0.12.0...v0.12.1
[0.12.0]: https://github.com/vetcoders/codescribe/compare/v0.11.2...v0.12.0
[0.11.2]: https://github.com/vetcoders/codescribe/compare/v0.10.0...v0.11.2
[0.10.0]: https://github.com/vetcoders/codescribe/compare/v0.9.2...v0.10.0
[v0.9.2]: https://github.com/vetcoders/codescribe/compare/v0.9.1...v0.9.2
[v0.9.1]: https://github.com/vetcoders/codescribe/compare/v0.9.0...v0.9.1
[v0.9.0]: https://github.com/vetcoders/codescribe/compare/v0.8.0...v0.9.0
[v0.7.14]: https://github.com/vetcoders/codescribe/compare/v0.7.2-dev...v0.7.14
[v0.7.2-dev]: https://github.com/vetcoders/codescribe/compare/v0.7.0...v0.7.2-dev
[v0.7.0]: https://github.com/vetcoders/codescribe/compare/v0.6.3...v0.7.0
[v0.6.3]: https://github.com/vetcoders/codescribe/compare/v0.6.2...v0.6.3
[v0.6.2]: https://github.com/vetcoders/codescribe/compare/v0.6.1...v0.6.2
[v0.6.1]: https://github.com/vetcoders/codescribe/compare/v0.6.0...v0.6.1
[v0.6.0]: https://github.com/vetcoders/codescribe/compare/19e05ad...v0.6.0

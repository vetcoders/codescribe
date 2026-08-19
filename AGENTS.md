---
name: codescribe
title: Codescribe Canonical AGENTS Directive
description: Canonical agent instructions, STT Overlay Doctrine, Peer Bus protocol, and high-
velocity Swarm Entrypoint for Codescribe.
version: 1.0.0
doctrine: stt-overlay-v1
architecture: living-tree
entrypoint:
  bus: AGENT_BUS.md
  loctree: loct
  build: scripts/build-app.sh
  test: make verify
  swarm: AGENTS.md#swarm-orchestration--fast-boot-entrypoint
roles:
  - operator
  - orchestrator
  - worker
  - audit
---

# AGENTS.md — Codescribe

##  Swarm Orchestration & Fast-Boot Entrypoint ("Na Bucie")

> **FOR ALL INCOMING AGENT SWARMS & MULTI-AGENT DISPATCHES:**
> Boot instantaneously, synchronize context across the Living Tree, and execute
> without friction or human-relay drag.

### ⚡ 30-Second Swarm Initialization (Fast Boot Protocol)

```
┌─────────────────────────────────────────────────────────────────────────────┐
│  1. READ PEER SIGNAL │ head -80 AGENT_BUS.md                                │
│  2. STRUCTURAL SIGHT │ loct / loctree-mcp (AST map over text grep)          │
│  3. OBEY DOCTRINE    │ 100% Append + Gap Fill Only + corrections on the fly!│
│  4. VERIFY LOCAL RUN │ make verify   (parity is a bench, not a gate)        │
└─────────────────────────────────────────────────────────────────────────────┘
```

#### Swarm Execution Matrix

| Phase | Swarm Role | Primary Tool / Command | Verification Gate |
| :--- | :--- | :--- | :--- |
| **0. Recon & Sight** | `loctree-scout` | `loct` / `loctree-mcp` | `loct occurrences <SYMBOL>` /
`slice` |
| **1. Signal Sync** | `bus-coordinator` | `head -80 AGENT_BUS.md` | Read & append signals; check
`OPERATOR_AWAY` |
| **2. Implementation** | `core-worker` | `cargo check` / Rust core | Small, atomic commits with
Authored-By |
| **3. UniFFI Bridge** | `bridge-worker` | `make app-bindings` | Bridge parity check between Rust
& Swift |
| **4. Verification** | `test-falsifier` | gate: `make verify` · bench: `make test-engine-parity` | Layer 0 only: similarity ≥
0.90 & structural bounds green. Needs the private corpus + loopback — see "Which ruler gates which
lane"; the layered lane is judged on structure, never on Apple fidelity |
| **5. App Build** | `release-builder` | `scripts/build-app.sh` | Developer ID signed binary
verification |

### 🛡️ Swarm Autonomy & Operational Laws

1. **Zero Human Relay**: Swarm agents communicate directly via `AGENT_BUS.md`. Never make the
human relay messages between workers.
2. **Living Tree Awareness**: Re-read files before editing. Never revert peer agent work. Work
concurrently in small, coherent commits (`[<agent>/<workflow>]`).
3. **No Blind Surgery**: Structural questions MUST go through Loctree (`loct` / `loctree-mcp`).
Grep is strictly for literal text searching.
4. **Immutable Live Transcript**: Any attempt to rewrite or replace live STT text with Whisper or
post-processing is a hard doctrine violation.
5. **Coalesce AppKit notification observers**: macOS 27 fires AppKit notifications from *inside*
window operations — a popover close reaches `becomeKeyWindow` and storms every `object: nil`
observer in the app (2026-08-07: main thread pinned 93/93 inside `_NSPopoverCloseAndAnimate`,
fixed in `d79781b1`). Any new `NotificationCenter` observer on an AppKit notification must
either coalesce onto the next main-queue tick (pattern: `scheduleExternalThreadsRefresh` in
`AgentChatStore.swift`) or bind to one specific `object:` with an O(1) handler. No disk, no
`DateFormatter`/ICU, no layout inside the callout. The live census is pinned in
`scripts/smoke/appkit-observers.allow` and enforced by `scripts/smoke-macos27.sh` — a new
observer fails the smoke until its discipline is written down.

### 🩺 Host smoke after every OS / Xcode bump

`scripts/smoke-macos27.sh` — the standing answer to "did AppKit/CoreGraphics move under us?".
Runs headless and raises no TCC dialog: CoreGraphics constant table vs the raw values pinned in
`app/os/hotkeys/platform.rs`, NSPanel placement clamp, event-tap re-arm, responsibility-disclaim
symbol, Sparkle wiring, AppKit observer census. Rows that need a human at the keyboard are
reported `SKIP`, never silently passed. `--out FILE` writes the filled checklist.

---

## Peer Bus (Do Not Make the Human Relay)

Read and append: `AGENT_BUS.md`
Cross-agent signals live there (operator away, stalls canceled, peer wake-ups).
At session start: `head -80 AGENT_BUS.md`. If you need another agent, write a `SIGNAL` block — the
operator's orchestration tooling handles peer wake-ups.

## Agent-Agnostic Worktrees and Evidence Planes

All agents and dispatchers use the same Vibecrafted-owned geometry. Never encode a client,
vendor, model, or agent name in infrastructure paths (`.claude`, `.codex`, `.gemini`, and similar
roots are forbidden for new worktree infrastructure).

The three canonical planes are separate:

- linked checkout: `~/.vibecrafted/worktrees/<org>/<repo>/YYYY_MMDD/<cut-id>`
- durable artifacts: `~/.vibecrafted/artifacts/<org>/<repo>/YYYY_MMDD/{plans,reports,...}`
- ephemeral current-run state: `~/.vibecrafted/control_plane/...`

Worktree branches use `cut/<cut-id>`. The dispatcher owns canonical `<org>`, `<repo>`, date, cut,
artifact-root, and run-root resolution; workers consume those resolved values and must not invent
their own vendor-specific paths. A linked checkout is disposable and must contain no sole copy of
a report, plan, handoff, verifier result, or delivery proof. Durable evidence goes to the artifact
plane. Heartbeats, locks, process metadata, transcripts, and other live supervision state go to
the control-plane runtime and may be collected or removed according to its lifecycle.

Every linked checkout owns its own ignored `target/` directory. Rust commands in a worker set
`CARGO_TARGET_DIR=$PWD/target` (or use the equivalent checkout-local default) and must never point
at the main checkout, another cut, or a shared fleet target. Sharing Cargo artifacts across
concurrent worktrees can execute a binary compiled from another cut even when the current source
tree differs. Integrators alone use the main checkout target, and integrator gates run with one
writer. Cold compilation is part of trustworthy parallel isolation, not a reason to share target
state.

Do not create a repository-local `./.vibecrafted` as a competing fourth plane. Existing ignored
repo-local scratch is legacy-only and is not authoritative. Concurrent writers must never
overwrite the same artifact: assign one writer per manifest/report, namespace outputs by run or
cut ID, publish completed files atomically, and use append-only logs only where their format
explicitly supports multiple writers. Git commits remain the authority for source changes.

Canonical per-repo instructions for every agent (Claude, Codex, Gemini, Junie, Grok, …). Read this
before touching anything.

## CODESCRIBE: The engine triangulation.

> _The codescribe app had already pre 0.8.0 era, the app was using the "final pass" approach: the Whisper - the **only** transcription engine was transcribing the whole audio file - no live, no overlay no instant delivery. and replacing the live transcript with the final pass. The issue was that Whisper was not very confident and the final pass was not very accurate. Also, the final pass was not very fast and the app was not very responsive._

**The engine has layers and layers are our weapon, disquise and defense while nobody looks. The goal is to connect disquised and visible magic so the perfect transcript arrives as the "overlay show" with its backspace magic, that put the corrections live while speaking: Apple live speech recognition comes instantly with letter-level precision but is not very confident; Whisper tail-patches and fills gaps thanks to better context; finally lexicon corrects specialistic terms and punctuation on the fly**

This means:
1. Apple Speech Delivers Instantly with letter precision but leaves gaps;
2. Whisper Transcribes on partial utterance-level "final passes" filling the gaps with better context - never replace the whole live transcript;
3. Lexicon corrections and punctuation are paralelly applied to the transcript;
4. Final pass is left as **opt-in** for regular runtime, but still acts as the lexicon hidden candidates donor.

## Canonical Layer Order (Operator Directive, 2026-07-26, Verbatim)

> → Neural instant letter-level transcript via Apple Speech API
> → Whisper transcribing partials on the go and all the time applying the
> patches
> → supervisor stays on duty final lexicon correction by substitution with
> heuristic dictionary!
> → human correction feeding lexicon perfectness."

Apple Speech API — instant, letter-level, 100%-confidence live transcript. This is the canvas. It
transcribes only what it is sure of; its gaps are the voids the next layers fill.
Whisper on partials, on the go — transcribes during the session, filling canvas gaps as they
appear. Whisper is never a stop-time full-text authority. A full-file "final pass" that replaces
the live transcript is a doctrine violation. (On-the-go partial transcription now **exists** —
Layer 1 tail-patch runs on both live paths, including the default Apple progressive one, since
`a6b1233d`. It is **on by default** (`CODESCRIBE_LAYERED_TRANSCRIPTION`
unset → `phase1`; explicit `off`/`0`/`false` disarms). A stock install therefore
already runs live tail-patch on both live paths. The stop-path
`merge_live_whisper` remains the residual floor + gap fill — never a
full-replace. W13 fusion / idempotence / highlight / inline-format flags
stay OFF until an operator flip. The bar that ends the live-lane shame is
layered-ON ≥ lbrx file-mode on U-WER vs human at live latency — see
`docs/THE_ENGINE_ROADMAP.md` §13.)
Lexicon correction — the FINAL automated layer — substitution from dictionary heuristics, applied
after Whisper, at the end.
Human correction — feeds lexicon perfectness. The human loop teaches the dictionary; the
dictionary improves every day.

### Why This Shape

> "The final shape of the transcription pipeline is layered. It is about
> the fusion of Apple SoTA neural speech recognition engine
> (SFSpeechRecognizer),
> Whisper (https://openai.com/index/openai-whisper/) and human-curated
> daily feed of custom lexicon rules that patch the mistakes."

Engi## ne triangulation IS the product:

- **Apple's Neural Shyness**: Instant letter-level transcription, outputting only 100%-confident
letters (the live canvas floor).

- **Whisper's Partial Pass**: Fills voids and partials on the go, but context-imprecise if treated
as a full replacement authority.
- **Lexicon Pass**: Final automated substitution based on dictionary heuristics, fed continuously
by human correction loops.

Replacement destroys the trust map that makes this triangulation valuable. Under the append-plus-
gap-fill contract, these three forces combine into pure transcript purity.

Anti-Patterns (Forbidden, Regardless of Who Proposes Them)

Whisper (or any engine) replacing committed live text at stop time.
Lexicon running before Whisper, or being treated as a mid-stream layer.
Any "cleaner rewrite" of the overlay after the fact.
Windowed re-transcription that reorders or drops committed spans.
Inventing a different layer shape from memory. This file is the shape.
  Past sessions contain abandoned ideas (per-request WAV path, Whisper-as-final-authority,
  dictionary-first gap filling) — they are dead. Do not resurrect them.

  ## Measured Bars Guarding the Doctrine

  tests/e2e_overlay_delivery_parity.rs::e2e_apple_live_parity — **in the layer0 lane** the live
  Apple canvas must reproduce the system dictation engine: similarity ≥ 0.90 plus deterministic
  structural bars: head present, tail sealed, word-count ratio 0.9–1.1 (no duplicated phrases, no
  lost spans). The layered lane is judged differently — see the next section, which is the
  authority on which bar applies where.
  **The bar is 0.90 and is not reproducibly green.** The "0.918–0.931 SFSpeech noise floor" this
  file used to quote was n=4; the wider Layer-0 sample measured 2026-08-08 (lane-leaked runs
  excluded) is 0.778 / 0.898 / 0.909 ×3 / 0.920 / 0.924 ×2 / 0.931 ×2 — 8 of 10 clear 0.90, two
  do not. Treat a single green run as a sample, not as proof, and read the lane line the harness
  now prints before trusting any number.

  ### Which ruler gates which lane

  **One rule, and it is enforced in code (`apple_rulers_gate`, `e2e_apple_live_parity`):**
  a bar gates only the lane whose job matches the bar's reference.

  | lane | job | what GATES it | what is only measured |
  |---|---|---|---|
  | layer0 (`off`) | reproduce Apple's live canvas | similarity ≥ 0.90 vs Apple · ratio 0.9–1.1 · head · tail · lane match | accuracy-vs-human |
  | layer1 (`phase1`) | diverge from Apple toward what was SAID | head · tail · ratio **floor** 0.9 (lost spans) · lane match | similarity vs Apple · ratio ceiling · accuracy-vs-human |

  **Why layer1 is not gated on Apple fidelity.** Gap-filling grows the denominator against an
  Apple ruler, so a *more* accurate layer scores *lower*. This is measured, repeatedly, and the
  sign is stable across every pair ever run: similarity falls, accuracy rises. The anchor is
  deterministic and needs no microphone — `apple_reference_is_a_ruler_not_the_truth` pins the
  Apple reference at **0.805** against the human transcription of the same audio, so 1.000 on
  that bar would mean reproducing Apple's errors. Layer 0 is already slightly more accurate than
  the ruler it is scored against. **Never make a layer less accurate to raise a number.**

  **Why accuracy-vs-human gates nothing either.** Its reference is a private fixture
  (`~/.codescribe/data_assets`, never in the repo — deprivatize fence), so a bar on it would
  evaporate silently on any tree without the operator's corpus. Both arms are printed for both
  lanes; which number gates a merge stays an operator decision
  (`.vibecrafted/plans/w12-layered-live-closure/reports/default-flip-memo-layered.md`).
  Live numbers belong in the retained run logs under `target/e2e-blackhole/`, not in this file:
  prose copies of them go stale within a run or two.

  **The structural cliff is the sharper edge.** The word-count ratio ceiling (1.1, scored against
  Apple's token count) caps the capture at 188 tokens on this fixture while the spoken truth
  carries 195 — so no layer can reach what was actually said without tripping it, and it fires
  hardest exactly when Layer 1 is most accurate (measured live at 190 tokens, ratio 1.11, a hard
  panic). The ceiling therefore does not gate the layered lane — but only when the excuse is
  visible: with no human reference beside the fixture, gap-fill and duplication are
  indistinguishable and the ceiling gates after all. `parity accuracy-headroom` prints the
  remaining budget every run.
  **Which target measures which lane** — the pin is per-target, so the lane is chosen by the
  target you run, never by an env var you prepend: `make test-engine-parity` (Layer 0, pinned
  off), `make test-engine-parity-layered` (phase1, the only incantation that actually arms
  Layer 1), `make test-engine-parity-both` (runs both arms, prints both numbers and the delta).
  Prepending `CODESCRIBE_LAYERED_TRANSCRIPTION=…` to any of them is now **refused** with exit 2:
  a recipe pin beats CLI env, so that form silently measured the other lane and reported the
  number as yours — it is how the W12 layered arm was recorded green while asserting nothing
  (review P1-01).
  **This instrument is operator-host-local, not CI.** Its whole corpus — the WAV, the Apple
  reference and the human transcription — lives outside the repo, and nothing in
  `.github/workflows/` runs it. On a checkout without the corpus these targets refuse rather
  than measure. Treat parity as the bench you walk to, never as a bar a merge already cleared.
  app/controller/mod.rs::adjudicate_recording_truth — "never full-replace live with Whisper";
  length-regression guard keeps the stream as the floor of truth.

  ## Working Rules

  Living Tree: Agents share one directory. Re-read files before editing; never revert other
  agents' changes; commit in small packs with [<agent>/<workflow>] titles and non-empty bulleted b
  odies.
  Loctree First: Structural questions (who imports X, blast radius, where a symbol lives) go to l
  oct / loctree-mcp, not grep. Grep is for literal text only.
  What Green Means: A verification command is authoritative only for what it executes, and no
  surface may cite it as proof of something it does not run. Two gates, and only two: `make check`
  (static — format, lint, semgrep, the env registry and the gate ledger; it executes ZERO tests)
  and `make verify` (hermetic — the workspace tests plus doctests, no operator dotenv, no private
  corpus, no Xcode, no API key). `make verify` is not a recipe that resembles CI, it IS the command
  `.github/workflows/rust.yml` runs, so the two cannot drift. Everything else — the parity bars,
  `make test-swift`, `smoke-macos27`, every real-API `make test*` lane — is a bench instrument:
  real proof, this host only, never a bar a merge has already cleared. The classification lives in
  the GATE LEDGER block of the Makefile, `make -s gate-ledger` prints it, and
  `scripts/validate-gates.sh` (run by `check`, and by `tests/gate_registry.rs` inside `verify`)
  fails when a verification target has no row, when a row names no target, or when a `ci=` claim
  disagrees with `.github/workflows/`. This rule exists because `check` used to print "Quality gate
  passed" having run nothing, and rust.yml called it "the full local gate incl. real-API / heavy
  e2e tests" directly above a job that ran cargo itself.
  Test Deadlines: In a test a clock is either the claim or a backstop — never both, and a backstop
  must sit out of reach of machine load. These budgets wrap process spawn, not just the wait for a
  reply: `spawn(python3) + initialize` for the MCP stdio mocks measures ~25 ms idle (n=12), so the
  sub-second budgets that used to guard them were a bet that a loaded box is never 10x slower at
  starting an interpreter. Losing that bet costs one of two things — a red that blames healthy code
  (`unexpected error: Timed out waiting for MCP response to 'initialize'`, reproduced deliberately
  2026-08-08), or, where the assertion is merely `is_err()`, a green that never exercised the guard
  at all. `core/mcp/client.rs::CONTENT_ASSERTION_BACKSTOP` is the pattern and carries the numbers;
  a tight clock is legitimate only where the timeout is the thing asserted.
  Attribution: Authored-By: <agent> <agents@vetcoders.io> — the agent that actually did the work.
  No vendor default footers.
  GitHub surface is English; chat with the operator is Polish.
  Push/Merge/PR actions are operator buttons — prepare the one-liner, do not press it yourself.
  UniFFI Bindings: After changing the core↔Swift bridge API, run make app-bindings — Xcode does
  not regenerate them automatically.
  Full App Build: scripts/build-app.sh (Developer ID signing keeps TCC grants stable across
  rebuilds).

  𝚅𝚒𝚋𝚎𝚌𝚛𝚊𝚏𝚝𝚎𝚍 with AI Agents by Vetcoders ©2024-2026 LibraxisAI

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
  test: make test-engine-parity
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
│  4. VERIFY LOCAL RUN │ make test-engine-parity                              │
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
| **4. Verification** | `test-falsifier` | `make test-engine-parity` | Similarity ≥ 0.90 &
structural bounds green |
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

---

## Peer Bus (Do Not Make the Human Relay)

Read and append: `AGENT_BUS.md`
Cross-agent signals live there (operator away, stalls canceled, peer wake-ups).
At session start: `head -80 AGENT_BUS.md`. If you need another agent, write a `SIGNAL` block — the
operator's orchestration tooling handles peer wake-ups.

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
the live transcript is a doctrine violation. (The current stop-path merge_live_whisper — live
floor + gap fill, never full-replace — is an accepted interim; the target is on-the-go partial
transcription.)
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

  tests/e2e_overlay_delivery_parity.rs::e2e_apple_live_parity — the live Apple canvas must re
  produce the system dictation engine: similarity ≥ 0.90 (SFSpeech's own word-level noise measured
  at 0.918–0.931 across identical runs) plus deterministic structural bars: head present, tail se
  aled, word-count ratio 0.9–1.1 (no duplicated phrases, no lost spans).
  app/controller/mod.rs::adjudicate_recording_truth — "never full-replace live with Whisper"; le
  ngth-regression guard keeps the stream as the floor of truth.

  ## Working Rules

  Living Tree: Agents share one directory. Re-read files before editing; never revert other
  agents' changes; commit in small packs with [<agent>/<workflow>] titles and non-empty bulleted b
  odies.
  Loctree First: Structural questions (who imports X, blast radius, where a symbol lives) go to l
  oct / loctree-mcp, not grep. Grep is for literal text only.
  Attribution: Authored-By: <agent> <agents@vetcoders.io> — the agent that actually did the work.
  No vendor default footers.
  GitHub surface is English; chat with the operator is Polish.
  Push/Merge/PR actions are operator buttons — prepare the one-liner, do not press it yourself.
  UniFFI Bindings: After changing the core↔Swift bridge API, run make app-bindings — Xcode does
  not regenerate them automatically.
  Full App Build: scripts/build-app.sh (Developer ID signing keeps TCC grants stable across
  rebuilds).

  𝚅𝚒𝚋𝚎𝚌𝚛𝚊𝚏𝚝𝚎𝚍 with AI Agents by Vetcoders ©2024-2026 LibraxisAI

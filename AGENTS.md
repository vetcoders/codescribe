# AGENTS.md — Codescribe

## Peer bus (do not make the human relay)

**Read and append:** [`AGENT_BUS.md`](./AGENT_BUS.md)
Cross-agent signals live there (operator away, stalls cancelled, peer wake-ups).
At session start: `head -80 AGENT_BUS.md`. If you need another agent: write a SIGNAL block — the operator's orchestration tooling handles peer wake-ups.


Canonical per-repo instructions for every agent (Claude, Codex, Gemini, Junie,
Grok, …). Read this before touching anything.

## THE ONE RULE — STT Overlay Doctrine (LAW, non-negotiable)

> **Zero overlay replacement — 100% append + gap filling. Nothing else.**

Text once committed to the overlay is immutable. No engine, no pass, no
"better" hypothesis may ever rewrite it. The only allowed operations are
**append** (new speech arrives) and **gap filling** (a layer fills a void the
canvas left open).

### Canonical layer order (operator directive, 2026-07-26, verbatim)

> "Neural instant letter level transcript via apple speech api -> whisper
> transcribing partials on the go (NIE FINAL!!!) -> final lexicon correction
> by substitution z heurystyk dictionary! -> human correction feding lexicon
> perfectness!"

1. **Apple Speech API** — instant, letter-level, 100%-confidence live
   transcript. This is the canvas. It transcribes only what it is sure of;
   its gaps are the voids the next layers fill.
2. **Whisper on partials, on the go** — transcribes DURING the session,
   filling canvas gaps as they appear. Whisper is **never** a stop-time
   full-text authority. A full-file "final pass" that replaces the live
   transcript is a doctrine violation. (The current stop-path
   `merge_live_whisper` — live floor + gap fill, never full-replace — is an
   accepted interim; the target is on-the-go partial transcription.)
3. **Lexicon correction — the FINAL automated layer** — substitution from
   dictionary heuristics, applied after Whisper, at the end.
4. **Human correction** — feeds lexicon perfectness. The human loop teaches
   the dictionary; the dictionary gets better every day.

### Why this shape

> "Ostateczny kształt to jest w chuj zlepianie i triangulacja bo z tego mamy
> mieć wartość: neural shyness + whisper garbage + lexicon pass = transcript
> purity."

Engine triangulation IS the product. Apple's shyness (only 100%-confident
letters), Whisper's context-greedy garbage (transcribes even what it isn't
sure of), and lexicon substitution combine into transcript purity — but ONLY
under the append-plus-gap-fill contract. Replacement destroys the trust map
that makes the triangulation valuable.

### Anti-patterns (forbidden, regardless of who proposes them)

- Whisper (or any engine) replacing committed live text at stop time.
- Lexicon running before Whisper, or being treated as a mid-stream layer.
- Any "cleaner rewrite" of the overlay after the fact.
- Windowed re-transcription that re-orders or drops committed spans.
- Inventing a different layer shape from memory. This file is the shape.
  Past sessions contain abandoned ideas (per-request WAV path, Whisper-as-
  final-authority, dictionary-first gap filling) — they are DEAD. Do not
  resurrect them.

### Measured bars guarding the doctrine

- `tests/e2e_overlay_delivery_parity.rs::e2e_apple_live_parity` — the live
  Apple canvas must reproduce the system dictation engine: similarity ≥ 0.90
  (SFSpeech's own word-level noise measured at 0.918–0.931 across identical
  runs) plus deterministic structural bars: head present, tail sealed,
  word-count ratio 0.9–1.1 (no duplicated phrases, no lost spans).
- `app/controller/mod.rs::adjudicate_recording_truth` — "never full-replace
  live with Whisper"; length-regression guard keeps the stream as floor of
  truth.

## Working rules

- **Living Tree**: agents share one directory. Re-read files before editing;
  never revert other agents' changes; commit in small packs with
  `[<agent>/<workflow>]` titles and non-empty bulleted bodies.
- **Loctree first**: structural questions (who imports X, blast radius,
  where symbol lives) go to `loct` / loctree-mcp, not grep. Grep is for
  literal text only.
- **Attribution**: `Authored-By: <agent> <agents@vetcoders.io>` — the agent
  that actually did the work. No vendor default footers.
- **GitHub surface is English**; chat with the operator is Polish.
- **Push/merge/PR actions are operator buttons** — prepare the one-liner,
  do not press it yourself.
- **UniFFI bindings**: after changing the core↔Swift bridge API, run
  `make app-bindings` — xcodebuild does not regenerate them.
- **Full app build**: `scripts/build-app.sh` (Developer ID signing keeps TCC
  grants stable across rebuilds).

𝚅𝚒𝚋𝚎𝚌𝚛𝚊𝚏𝚝𝚎𝚍. with AI Agents by Vetcoders (c)2024-2026 LibraxisAI

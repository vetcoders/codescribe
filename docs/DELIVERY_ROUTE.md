# Delivery route — one throne for destination

> Operator 2026-08-15: the stop path was a fight for the throne. This file is
> the destination axis only. Mic lock, transcript truth, and agent-chain
> memory stay other thrones.

## Law

1. **Intent is frozen at session start** (or at an explicit overlay click).
   OS focus at stop time is not an input.
2. **`resolve_delivery_route` is the only function that picks a destination.**
   Auto-paste, overlay Insert, and To Agent consult it. They do not invent a
   second king.
3. **Codescribe is never a legal Cmd+V target.** A latched self-app (Agent
   composer, overlay, settings) stays on the Orient canvas or goes to the
   Agent composer as a first-class message — never as a tagged paste into
   ourselves. That is how `<codescribe mode="raw">` stopped landing in chat.

## Intent → route

| Intent | Typical gesture | Route |
|---|---|---|
| `AgentVoice` | Double Right Option / assistive hold | `AgentComposer` |
| `OverlayToAgent` | overlay **To Agent** | `AgentComposer` |
| `OrientDictation` | Hold Fn / Globe | `ClipboardPaste` if auto-paste + latched foreign app; else `OrientCanvas` |
| `OrientFormat` | Double Left Option | same as dictation |
| `OverlayInsert` | overlay Insert / defer | `ClipboardPaste` if the latched target is a foreign app; `DeferredInsert` if the latched target (or the caret) is Codescribe |
| `NotesOnly` | save-only notes | `ArchiveOnly` |

Vetoes that keep Orient off the paste gun: empty / no-speech, live-stream
session, quality-commit pending, latched target is Codescribe.

Explicit overlay clicks do **not** inherit the live-stream or quality-commit
vetoes. The user asked to insert now. Codescribe as the latched target still
refuses Cmd+V into ourselves and arms Paste Here instead.

`paste_text_from_overlay` and `defer_text_from_overlay` consult
`resolve_delivery_route`. They do not pick a destination on their own.

## Telemetry

One INFO line per stop / To Agent / overlay Insert / defer:

```text
delivery_route: intent=orient_dictation route=clipboard_paste reason=auto_paste_to_latched_target target=Ghostty
```

`reason=refuse_paste_into_self` is the smoking gun for "I was looking at the
Agent and Hold Fn dumped raw into the composer".

## What this cut does not do

- It does not pick the transcript (Apple / Whisper / final-pass). That is
  `adjudicate_recording_truth`.
- It does not lock the microphone. That is still a missing `RecordingSessionOwner`.
- It does not make the agent chain mandatory. `previous_response_id` stays
  best-effort until that throne is cut.

Stacked on `fix/engine-routing`.

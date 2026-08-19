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
3. **The overlay canvas is never a legal Cmd+V target.** Caret in our panel
   → park Paste Here. The Agent window, Alacritty/Zellij (vc-terminal),
   Notes, and every other caret **are** legal ambulances. Assistive still
   delivers as a first-class Agent message — that is a different intent,
   not a ban on pasting into the Agent window.
4. **Clipboard is borrowed, never stolen.** We may overwrite `NSPasteboard`
   for a real Cmd+V. We must restore what the user had. If auto-paste cannot
   land, we lose neither: restore the system clipboard, park the transcript
   in our buffer (⌘⌥V). Explicit overlay **Copy** is the only verb that
   writes the pasteboard on purpose and leaves it.

## Intent → route

| Intent            | Typical gesture                      | Route                                                                                                                        |
| ----------------- | ------------------------------------ | ---------------------------------------------------------------------------------------------------------------------------- |
| `AgentVoice`      | Double Right Option / assistive hold | `AgentComposer`                                                                                                              |
| `OverlayToAgent`  | overlay **To Agent**                 | `AgentComposer`                                                                                                              |
| `OrientDictation` | Hold Fn / Globe                      | `ClipboardPaste` if auto-paste; `OrientCanvas` if overlay caret / no auto-paste                                              |
| `OrientFormat`    | Double Left Option                   | same as dictation                                                                                                            |
| `OverlayInsert`   | overlay Insert / defer               | `ClipboardPaste` into the latched caret (Agent, Alacritty, …); `DeferredInsert` only when the overlay canvas holds the caret |
| `NotesOnly`       | save-only notes                      | `ArchiveOnly`                                                                                                                |

Vetoes that keep Orient off the paste gun: empty / no-speech, live-stream
session, quality-commit pending, overlay canvas holds the caret.

Explicit overlay clicks do **not** inherit the live-stream or quality-commit
vetoes. The user asked to insert now. Overlay caret still refuses Cmd+V into
the canvas and arms Paste Here instead. Agent / Alacritty still get Cmd+V.

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

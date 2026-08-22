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
3. **Codescribe is never a legal Cmd+V target.** Caret in our overlay or Agent
   window → park Paste Here or use the explicit Agent route. Alacritty/Zellij
   (vc-terminal), Notes, and other positively latched foreign carets are legal
   paste targets. Assistive delivers as a first-class Agent message; it does
   not synthesize a paste back into our own process.
4. **Clipboard is borrowed, never stolen.** On release we snapshot the user's
   pasteboard, Cmd+V into the latched caret, then restore. The overlay must
   resign key first. The foreign target must then be observed as frontmost;
   Codescribe remaining frontmost is a veto. If Cmd+V cannot land, park ⌘⌥V
   and leave the user's pasteboard alone. Explicit overlay **Copy** is the only
   verb that writes the pasteboard on purpose and leaves it.

## Intent → route

| Intent            | Typical gesture                      | Route                                                                                                                |
| ----------------- | ------------------------------------ | -------------------------------------------------------------------------------------------------------------------- |
| `AgentVoice`      | Double Right Option / assistive hold | `AgentComposer`                                                                                                      |
| `OverlayToAgent`  | overlay **To Agent**                 | `AgentComposer`                                                                                                      |
| `OrientDictation` | Hold Fn / Globe                      | `ClipboardPaste` if auto-paste; `OrientCanvas` if overlay caret / no auto-paste                                      |
| `OrientFormat`    | Double Left Option                   | same as dictation                                                                                                    |
| `OverlayInsert`   | overlay Insert / defer               | `ClipboardPaste` into a latched foreign caret (Alacritty, Notes, …); `DeferredInsert` when Codescribe owns the caret |
| `NotesOnly`       | save-only notes                      | `ArchiveOnly`                                                                                                        |

Vetoes that keep Orient off the paste gun: empty / no-speech, live-stream
session, quality-commit pending, overlay canvas holds the caret.

Explicit overlay clicks do **not** inherit the live-stream or quality-commit
vetoes. The user asked to insert now. Any Codescribe caret still refuses Cmd+V
and arms Paste Here instead. Alacritty and other confirmed foreign targets get
Cmd+V; Agent requires the explicit Agent route.

`paste_text_from_overlay` and `defer_text_from_overlay` consult
`resolve_delivery_route`. They do not pick a destination on their own.

## Telemetry

One INFO line per stop / To Agent / overlay Insert / defer:

```text
delivery_route: intent=orient_dictation route=clipboard_paste reason=auto_paste_to_latched_target target=Ghostty
```

`reason=refuse_paste_into_self` is the smoking gun for "Codescribe owned focus,
so the transcript was parked instead of being pasted into an unknown internal
caret".

## What this cut does not do

- It does not pick the transcript (Apple / Whisper / final-pass). That is
  `adjudicate_recording_truth`.
- It does not lock the microphone. That is still a missing `RecordingSessionOwner`.
- It does not make the agent chain mandatory. `previous_response_id` stays
  best-effort until that throne is cut.

Stacked on `fix/engine-routing`.

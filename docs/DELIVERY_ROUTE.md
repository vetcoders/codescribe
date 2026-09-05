# Delivery route — one throne for destination

> Founder 2026-08-15: the stop path was a fight for the throne. This file is
> the destination axis only. Mic lock, transcript truth, and agent-chain
> memory stay other thrones.

## Law

1. **Intent is frozen at session start** (or at an explicit overlay click).
   OS focus at stop time is not an input.
2. **`resolve_delivery_route` is the only function that picks a destination.**
   Auto-paste, overlay Insert, and To Agent consult it. They do not invent a
   second king.
3. **The Codescribe overlay canvas is never a legal Cmd+V target.** Its caret
   parks Paste Here. A positively latched Agent composer, Alacritty/Zellij
   (vc-terminal), Notes, or another foreign caret is legal; choosing the Agent
   route remains an explicit action. Assistive delivers as a first-class Agent
   message rather than synthesizing a focus-derived paste.
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

The target is latched before the recording overlay takes focus and survives
the terminal transition back to Idle. Start failure and explicit recovery
clear it. This ordering matters: Insert happens after the terminal transition,
so clearing the target as ordinary recording state makes every finished take
degrade to DeferredInsert even when the foreign caret was known.

## Telemetry

One INFO line per stop / To Agent / overlay Insert / defer:

```text
delivery_route: intent=overlay_insert route=clipboard_paste reason=explicit_insert target=Ghostty
```

`reason=refuse_paste_into_self` is the smoking gun for "Codescribe owned focus,
so the transcript was parked instead of being pasted into an unknown internal
caret".

## Terminal and CLI consumers

An overlay Insert into a positively latched terminal/editor uses one borrowed
clipboard swap and one Cmd+V. The terminal emulator owns bracketed-paste
handling; Codescribe never types the transcript character by character.
Alternate-screen confirmation remains a product walk-around, especially for
vc-frame and zellij key-routing combinations.

The CLI reads the same committed Transcript Bus. `codescribe transcribe live`
owns the canonical Rust wake/projection path; `bus-demux.py` is a named-agent
routing adapter, not another transcript reducer. For an editable shell prompt,
`scripts/codescribe.zsh` inserts `codescribe transcribe last` literally through
ZLE without appending Enter. That explicit line-editor path complements UI
Insert and does not create a second text authority.

## What this cut does not do

- It does not pick the transcript (Apple / Whisper / final-pass). That is
  `adjudicate_recording_truth`.
- It does not lock the microphone. That is still a missing `RecordingSessionOwner`.
- It does not make the agent chain mandatory. `previous_response_id` stays
  best-effort until that throne is cut.

Stacked on `fix/engine-routing`.

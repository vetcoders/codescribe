# Dictation overlay and agent conversations

Codescribe has two distinct conversation surfaces:

- The floating **dictation overlay** shows your transcript and the conversations
  with coding agents attached to Codescribe channels.
- **Show Agent** opens the built-in Agent window, with its own thread sidebar,
  provider, tools and composer. It is not the channel roster in the overlay.

The floating overlay does not have Drawer / Agent / Settings tabs. Its channel
menu switches between **My dictation**, **0 · All**, individual agents and saved
conversations. Open the native Settings window from the menu bar.

## Show and resize the overlay

Use the preview control to cycle through mini, compact and expanded forms, or
choose a form from its context menu. The upper-right corner stays pinned while
expanding; moving the window changes that pin.

Mini is a fixed strip. Compact supports horizontal resizing. Expanded supports
edge and corner resizing. The minimum visible sizes are 200 × 46, 410 × 46 and
320 × 260 points respectively. During dragging the current form keeps its
readable floor. A sufficiently small requested drag can select a compact form
when released; it does not squeeze an expanded transcript into a tiny frame
while the gesture is still in progress.

During a manual resize the grabbed edge follows the pointer and its opposite
edge stays fixed. Pointer events are coalesced to the latest position per
display interval; release applies the final position immediately. Closing the
overlay cancels any pending resize. Presentation-mode expansion still uses its
upper-right anchor.

**Keep visible between takes** and the transcription preview controls are
separate from starting a recording. Selecting a conversation also does not start
capture or change the destination of a recording already in progress.

A small orange underline indicates readiness on the visible ordinary dictation
canvas, including when it is empty or a take has finished. It follows the native
text tail and pulses more quietly when text is present. It is decorative: it
changes no transcript bytes, selection, copy or delivery, and does not mean the
microphone is recording. Actual editing, hiding or folding the canvas removes
it; editable text alone does not. Reduce Motion keeps the marker steady.

The glass is tinted to the light or dark text palette. Increase Contrast and
Reduce Transparency replace it with an opaque background. These presentation
changes do not accelerate or change speech recognition.

## Speak to a channel

Open the channel menu and use the microphone control next to a bound agent, or
next to **0 · All** for a broadcast. Selecting a row only changes what you view;
the microphone control changes whom you speak to. The named agent must be
attached with its own provider and session identity.

Channel replies appear as selectable messages. Scrolling back pauses following
the live edge; use the return-to-current control to see the latest messages
again. Selecting a conversation does not replay old speech.

**Read N/M** is the number of recipients that acknowledged reading a message.
It does not mean they completed the work, accepted the transcript's acoustic
coverage or answered aloud.

## Type a message and attach clipboard content

Click the selected conversation's composer, type and press its Send control.
The message targets that conversation's bound agent identity. If publication
fails, the draft remains; do not assume the recipient received it.

Pasting an image or regular file saves a local copy and inserts an editable
attachment marker in the draft. Plain text up to 4096 UTF-8 bytes stays inline;
longer text is stored as an attachment. The total attachment limit is 50 MiB.
Deleting a marker removes it from that message. Sending hands the recipient a
local path and verified file metadata; the bus does not embed the file bytes.
This does not itself prove that an agent opened the attachment.

## Control spoken replies

The speaker beside an agent controls that agent's automatic reply playback.
**0 · All** is the global speaker switch: it covers all channels, including
future ones, and does not erase individual mute choices. Opening or closing a
microphone channel is a different action.

A message's **Play** control explicitly requests playback; **Stop** stops that
playback request. An individual automatic-reply mute still permits manual Play;
the global mute also silences manual playback. Reading history never plays it.
An unavailable playback state or save error is a failure to confirm the setting,
not a successful mute. Text remains visible when audio playback fails.

## Built-in Agent window

Open **Show Agent** from the menu bar or use `Command+Shift+Space`. Select a thread
in its sidebar, or create one explicitly with **New thread**. Summoning the window
does not start recording or send a request.

The built-in Agent composer microphone records one turn until explicit Stop.
It shares Codescribe's recording controller and transcription engine. Hands-free
Agent gestures use their configured silence boundaries; the composer microphone
and a hands-free gesture therefore have different turn lifecycles. Configure
mode gestures in **Settings → Hotkeys**, and providers/prompts under Agent
settings. See [Recording modes](modes.md) and [Settings](settings.md).

## Current acceptance limits

The resize floors and scroll-ownership change are present in source `04fc280c7`
(local build 801, 2026-10-11). Visible flicker during resizing still requires live
acceptance. Reported playback-setting save failures and Settings retaining
keyboard focus after a composer click are open issues. These are defects to
resolve, not intended interaction rules.

An installed bundle and the running process can be different generations after
an installation. The new UI code takes effect only when the app next launches;
installation does not terminate an active recording or conversation.

_Created by Vetcoders (c)2026_

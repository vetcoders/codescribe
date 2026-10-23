# Dictation overlay and agent conversations

Codescribe has two distinct conversation surfaces:

- The floating **dictation overlay** shows your transcript and the conversations
  with coding agents attached to Codescribe channels.
- **Show Agent** opens the built-in Agent window, with its own conversation
  sidebar, provider, tools and composer. It is not the channel roster in the
  overlay.

The floating overlay does not have Drawer / Agent / Settings tabs. Its channel
menu switches between **My dictation**, **0 · All**, individual agents and saved
conversations. Open the native Settings window from the menu bar.

## Show and resize the overlay

Use the preview control to cycle through mini, compact and expanded forms, or
choose a form from its context menu. The upper-right corner stays pinned while
expanding; moving the window changes that pin.

The channel map is optional. In Settings → Creator, enable **Channel map in the
overlay** to show its counter in mini and its chips in compact. It is off until
you opt in. Switching it updates open overlays immediately without selecting a
conversation or changing the microphone. With it off, mini keeps the `cs` wordmark
and the earlier 200 pt width (164 pt during agent recording); compact returns to
its 410 pt natural width. A width you explicitly resized is still yours.

With the channel map enabled, mini is as wide as its content: the brand dot, the channel counter and its
controls, with no empty space between them, so it narrows or widens a little
when the counter changes. Compact supports horizontal resizing. Expanded
supports edge and corner resizing. The minimum visible sizes are 130 × 46,
410 × 46 and 320 × 260 points respectively. Compact's own width follows its channel map: with
agents on the bus it opens wide enough for every named chip, up to 820 points,
and it grows or shrinks at its upper-right corner when a channel is added or
archived. Past 820 points the chips drop their names instead of widening the
strip. Narrow it by hand and it keeps your width for the rest of the session,
with the chips showing digits only or folded into the counter; clicking the
counter gives the strip its natural width back. Widen it and it keeps your
width, growing only when the map needs more. During dragging the current form
keeps its
readable floor. A sufficiently small requested drag can select a compact form
when released; it does not squeeze an expanded transcript into a tiny frame
while the gesture is still in progress.

During a manual resize the grabbed edge follows the pointer and its opposite
edge stays fixed. Pointer events are coalesced to the latest position per
display interval; release applies the final position immediately. Closing the
overlay cancels any pending resize. Presentation-mode expansion still uses its
upper-right anchor.

The header's **✍︎** button shows your transcription and **✦** opens the
**Agents** sidebar. The pin beside them keeps the overlay visible between takes;
it is filled while pinned. Unpinning an overlay that is resting between takes
does not hide it at the click: it stays for three seconds, so another click can
pin it again, and then fades out. Moving the pointer back onto the overlay pauses
that; leaving gives it another three seconds. A transcript draft you have not
committed keeps the overlay until you resolve it.

Pointing at a header control shows its hint just below it, in the same chip as
the action pill's hints, and rings the control in the color of what it opens:
green for the transcription, violet for agents, orange for the pin and the
recording controls. The hint floats outside the panel, so it also appears on
the 46-point mini and compact strips.

A secondary click (right-click or Control-click) anywhere on the header opens
the same menu in every form: **Agents**, **Transcription**, **Pin** / **Unpin**,
the six screen anchors with **Free motion**, and **Show transcript by default**.

None of these controls starts a recording. Selecting a conversation also does
not start capture or change the destination of a recording already in progress.

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

The conversation's name pill brings that agent's Frame pane or terminal to the
front when Codescribe knows where it runs; to change conversations, use the
channel menu or, in the compact form, the channel chips. Rows you can open in the
sidebar show a faint highlight under the pointer.

An agent row in the **Agents** list has a button that brings that agent's own
terminal to the front: its vc-frame pane when it runs in Frame, otherwise its
iTerm2 or Terminal tab. The button appears only when Codescribe knows where the
agent runs. If macOS asks whether the terminal may be controlled, allow it, or
turn it on later in System Settings › Privacy & Security › Automation.

In the full **Agents** list, **0 · All** stays just below the header while the
other channels scroll underneath. Its recording and playback buttons stay
available when you scroll to the last agent.

In Settings → Creator → **Intercom layout**, pick one of three states:
**Classic** (the overlay without a channel map), the **horizontal strip**, or
the **floating agent pills**. Classic remembers which of the two layouts you
used last. With the pills, the small desktop Intercom remains available when
the main overlay is closed. Point at one agent to see information; click the
name to expand that card into a conversation with a draft, pasted attachments
and Send. Drag its window glyph to detach it into a durable fiszka; its row
stays in the group, greyed with a window glyph, and clicking that row brings
the window forward. Click **Intercom** to fold or unfold the group, and drag
any free spot on that bar to move the whole group. On a short screen with
large text, scroll over a pill or use the header arrows to reach the
remaining agents.

A detached fiszka keeps its place, size and mini state across app restarts.
One agent keeps one fiszka: detaching again shows the one already open. Drag
it by any free spot on its header; right-click the header for the agent's
actions, including **Show in Agents**, the road back to the main overlay.
Minimize shrinks a fiszka to its name, activity, microphone and speaker;
click it to restore, hover it to reveal its close button. Closing a fiszka only hides that window: detach the same conversation again —
or right-click **Intercom** and pick it from the saved list — and it
comes back where you left it. Resize a
fiszka from its left edge or bottom corners; the edge lights up when the
pointer is on it. Dragging a resting pill's left
edge or bottom corners (or the header's left edge) zooms the whole attached
group proportionally. Closing a card or switching layouts keeps its draft.
Every row carries the same microphone and speaker pair as the agents list:
grey at rest, violet with a pulse while recording or speaking; the speaker
mutes that agent's automatic replies, and the Intercom bar's megaphone and
speaker are channel 0 and the master mute. Each row shows one status dot
before its figure, the roster's own, and the bar shows channel 0's. Press Play on a
message to hear it even while muted. Agent pills and the agent list wear the
microphone with a waveform, the glyph of speaking to an agent live; the
composer's dictation control keeps the plain microphone, since dictating a
draft is another act.
Right-click a floating pill for the agent's other actions.

For a Codex agent with neither, the same button opens the agent's thread in the
Codex app (ChatGPT). This works even when the row shows the agent as
disconnected, which Codex agents often are between replies. A Codex Cloud
thread cannot be opened from your Mac: its button stays greyed, and you open
that thread in ChatGPT yourself.

Channel replies appear as selectable messages. Scrolling back pauses following
the live edge; use the return-to-current control to see the latest messages
again. Selecting a conversation does not replay old speech.

**Read N/M** is the number of recipients that acknowledged reading a message.
It does not mean they completed the work, accepted the transcript's acoustic
coverage or answered aloud.

## Channel map in the collapsed overlay

Mini and compact carry a map of the bus, so you can see which agent answers
which channel chord without unfolding the **Agents** canvas.

In compact the map sits between the clock and the header buttons: one chip per
channel bound to an agent, in digit order. A chip shows the digit in a circle
and the agent's name, written with a capital first letter, for example
`2 Miksa · 3 Igor · 5 Kuba`. Only agents appear: channel 0 broadcasts to
everyone and has no chip of its own, except while the microphone is open on it
(see below). Only channels the roster binds appear; delivery history alone does
not add one. A chip whose agent has disconnected is dimmed like its row in the
**Agents** list; the digit stays, because the channel chord still reaches the
channel. A dot marks unread replies.

In mini the brand dot is followed by a counter, `3 ▾`, the number of agents,
then the agents button, the microphone and the form control. Mini no longer
shows the "cs" wordmark.

Pointing at a chip names the channel, the agent and the chord, for example
`5 · Kuba · Ctrl+5`. Pointing at the counter lists the agents, for example
`3 Igor · 4 Hania · 6 Konfig`. The modifier is the one chosen in Settings ›
Shortcuts › Extra shortcuts › **Agent channel** (Ctrl or Fn); the map reads it
when it appears and when you point at it.

Clicking a chip opens that agent's conversation on the **Agents** canvas, as its
numbered pill in the folded **Agents** list does. Clicking the counter in mini
switches the overlay to compact, where the chips are; clicking it in a compact
strip you narrowed gives the strip its natural width back, and the chips
return; the counter never selects a conversation. Neither a chip nor the
counter opens or closes the microphone or changes the destination of a
recording in progress; use the channel chord or a microphone control to speak.

While the microphone feeds a channel, that channel's digit carries a small
violet dot, and its chip takes the same tint, in its usual place. The digit
carries no symbol: the **Agents** button shows a violet microphone while you
speak to one agent and a megaphone only while you broadcast to all. While you
broadcast on channel 0, a `0` chip with the dot leads the row. In mini, and in
a compact strip folded to its counter, the counter shows the open channel's
digit with the dot instead of the count. When the channel closes, the plain
chip and the count return. The dot says where your voice goes; it is not a
receipt.

With the map shown, pressing the channel chord in mini or compact keeps that
form: the chip or the counter shows the open digit, and the overlay does not
unfold onto the **Agents** canvas. Unfold it yourself when you want to read the
conversation. Without the map, the chord unfolds the overlay onto that
agent's conversation.

When compact has no room for the names, the chips show digits only; the chip of
the conversation you have selected keeps its name, and pointing at a chip still
names it. When even the digits do not fit the width you set by hand, the chips
collapse into the counter. If even the counter does not fit, the map steps
aside so the header buttons and the fold control keep their place. By default
the strip is as wide as its chips need, up to 820 points, so the counter
appears only after you narrow it. The strip stays 46 points tall, and larger
text grows the chips only as far as that height allows.

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

Open **Show Agent** from the menu bar or use `Command+Shift+Space`. The
collapsed widget itself answers its own chord from anywhere — `Control+Shift+B`
until you record another one in Settings › Shortcuts › Extra shortcuts: hidden,
it shows; visible and idle, it hides; recording or holding an open channel, it
only comes forward. Select a
conversation in its sidebar, or create one explicitly with **New conversation**
at the top of the sidebar. The sidebar looks and behaves like the one in Settings: the button at the
left of the toolbar, or `Control+Command+S`, hides and shows it. Summoning the
window does not start recording or send a request.

The window has one bar, the same height as the Settings window's. It leads
with the wordmark and the conversation's identity: its title, the live status
while the Agent works, and a line with **Agent** and the conversation's model
when it has one. On the trailing side sit a one-click width button (each click
moves Standard → Wide → Full width, the same preference the menu names),
**Context**, **Always on top** and the conversation menu (rename, favorites,
Markdown export, conversation width, settings, delete). On a developer build
with Voice Lab enabled, the bar also shows a small **Voice Lab power mode**
mark before those controls.

**Context** opens the **Agent context** panel on the right of the
conversation; its close button or **Context** again hides it. The panel is
closed by default and the app remembers your choice. It shows what the
conversation already stores (title, model, number of messages, last update)
and a **Tool settings** row that opens **Settings → Agent → Tools**, where
tool permissions are configured. Opening or closing the panel does not change
the conversation, the draft, the model or a recording. The panel has a fixed
width; when the window is too narrow for the panel and a readable
conversation, the window widens as the panel opens.

Each message shows its author and a short time: the clock for today, the word
for yesterday, the date for older messages. Hover the time for the full date
and time; where the conversation crosses midnight, a separator names the new
day. Actions sit in a quiet row under the message: **copy** on every message;
**Speak** (read the reply aloud) and **raw**/**rich** on Agent replies.
**raw** shows that one reply as the exact text that streamed in, **rich**
renders it as Markdown again; it is a view of the message, not a reply style.
The composer controls (attach, microphone, send) name themselves in their
tooltips.

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

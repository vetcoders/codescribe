# Dictation overlay and agent conversations

Codescribe has two distinct conversation surfaces:

- The floating **dictation overlay** shows your transcript and the conversations
  with coding agents attached to Codescribe channels.
- **Show Agent** opens the built-in Agent window, with its own conversation
  sidebar, provider, tools and composer. It is not the channel roster in the
  overlay.

The behavior and code owners behind this guide are specified in the
[bus window contract](../OVERLAY_PRESENTATION.md).

The floating overlay does not have Drawer / Agent / Settings tabs. Its channel
menu switches between **My dictation**, **0 · All**, individual agents and saved
conversations. Open the native Settings window from the menu bar.

## Show and resize the overlay

The bar has two sizes: **mini** and the **expanded bar**. Click the bar's
empty background to switch between them, in either direction; drag the
background to move the bar. The buttons keep their own actions: the brand dot
closes the overlay, the chevron opens the window (transcription or Agents)
and folds it back to mini, and its context menu picks any form directly. The
upper-right corner stays pinned while the bar or the window grows; moving the
window changes that pin.

Mini shows, from the left, the brand dot, transcription (✍︎), the
microphone, Agents (✦) and the chevron, and is exactly as wide as those
controls. Point at ✦ to see the agents on the bus, `N · Name` in channel
order, each followed by its status dot, its channel-0 antenna and
do-not-disturb moon (click them to switch) and a yellow dot beside unread
replies; move the pointer into the list and click an agent to open its
conversation, or right-click it for the agent's menu, the same as on a circle
in the expanded bar. The bar itself keeps only numbers, names and their marks
(Monika's decision of 2026-10-30). Click ✦ itself to open the
**Agents** window. While you speak to agents, mini stays mini: the microphone
becomes Stop, and the ✦ slot shows the violet channel number of each agent
who hears you (overlapping circles for several, `0` for everyone); point at it
for their names.

The expanded bar shows the brand, the agent map (with the horizontal Intercom
layout), transcription, ✦, the pin, the microphone and the chevron. The
waveform and the recording clock appear only while you record; at rest they
take no room. Drag its left or right edge to make it wider or narrower; that
changes the width only, never the form, and the bar keeps your width for the
session. Expanded supports edge and corner resizing. The minimum visible
sizes are 161 × 46 points for mini, the bar's densest map layout for the
expanded bar, and 320 × 260 points for the window. A sufficiently small
requested resize of the window can fold it into a bar when released; it does
not squeeze an expanded transcript into a tiny frame while the gesture is
still in progress.

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
mini and the expanded bar.

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

In the horizontal window, the conversation header keeps the viewed agent and
its microphone, speaker and **More (…)** menu above the scrolling history.
Use the panel button at the right to switch **Agents** between its full list
and narrow channel bar. The full list sits beside the chat in a wide window
and covers it in a narrow window. Choosing a row from the covering list returns
to the bar. Right-click the same panel button to choose the full list, channel
bar or complete hiding; hiding gives all panel width back to the conversation.
The window keeps its size, draft and text recipient. The conversation has no
separate hide/show button or folded edge. The expanded bar's agent map keeps
its own presentation.

Conversation and agent-list headers share the same compact height, without an
extra toggle row. Message action labels use another row when necessary on a
narrow chat so the message stays within its available width. Rows you can open
show a faint highlight under the pointer.

An agent row's **More** menu includes the detected workspace action: its
vc-frame pane when it runs in Frame, otherwise its iTerm2 or Terminal tab. It
is available only when Codescribe knows where the agent runs. The same action
is available in the row's right-click menu. If macOS asks whether the terminal
may be controlled, allow it, or turn it on later in System Settings › Privacy &
Security › Automation.

The horizontal list uses compact cards: identity and audio controls, the
provider/model/project, then a routed preview of up to two lines. A single
check is a delivered receipt; the compact double check is a read receipt. Its
hint names the outgoing message it describes. The circular number counts
replies you have not viewed and is independent of the agent's receipt. A
disconnected card dims but its history remains readable.

Names and channel numbers have priority at standard sidebar widths. The clock
and unread count sit beside the provider/model/project line; longer names can
wrap to two lines. A subtle separator keeps adjacent agents distinct without
requiring a selection or hover.

Rest the pointer over an agent's name or message preview to read the full last
message in a floating preview. This also works on the folded sidebar's channel
entries and the inbox. Move the pointer into the preview to scroll longer
messages; leaving both the entry and preview dismisses it. Previewing does not
open that conversation, mark replies as viewed, change the text recipient or
open a microphone. The last message can be yours or the agent's, matching the
message shown on the card.

The preview fits short messages closely and uses a smaller bounded window for
longer ones. Its subtle glass surface, thin outline and soft shadow separate it
from the conversation underneath. Message text stays at the normal readable
size, with scrolling available for the full content.

Use **More** to change channel-zero membership or **Do not disturb**, open the
workspace, or remove the agent into saved conversations. A moon remains by the
name while Do not disturb is on; a crossed antenna remains while the agent is
outside channel zero. The waveform microphone, speaker and More button stay
visible without hovering. In the narrow bar, buttons stay centered. A channel's
dot shows its status, a small yellow number at the upper-right corner counts
unviewed replies, and the background marks the viewed conversation. The inbox
can be selected like any conversation. These signals are independent; unread
counts do not turn all dots yellow. Two-digit counts stay readable inside the
bar; above 99 the badge reads 99+ and accessibility keeps the exact count.
The conversation keeps its own audio and More actions.

Right-clicking a card opens the same secondary menu as **More**. It contains
contact settings, the detected workspace and removal into saved conversations;
recording and playback stay on their visible buttons. Opening the workspace
brings the agent's detected client, terminal or Frame pane forward. Removal
preserves the saved conversation; hang up the agent's microphone channel first.
The floating pills retain their existing full menus. The
[approved window mockup](bus-window-mockup.html) illustrates this card layout.

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
the window forward. Each pill carries the agents list's status dot, the
channel-0 antenna and the do-not-disturb moon beside the name, the microphone
and speaker drawn flat on the pill, and the same button that brings the
agent's pane or terminal tab forward. On the floating pills the antenna and
moon remain one-click switches: the antenna joins or leaves channel
0, the lit moon holds every message from every channel in the agent's mailbox
without waking it, and either can be set without the other. Click **Intercom** to fold or unfold the group, and drag
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

## Choose where your text goes

The horizontal window names the text destination beside the composer as
**To:**. Choosing another conversation changes what you view without changing
that text destination or opening a microphone. Use the destination chooser,
**Write to …**, or an agent's **Write / Forward…** command to choose where to
write explicitly. Your stored drafts stay with their original conversations.

The **Voice channels** line names the channels you are speaking to live. It is
separate from the composer's plain microphone, which dictates into a draft.
The destination and voice status share a compact row when there is room and
stack at narrower widths. They and the composer keep their own space below
the scrolling messages, so history never passes behind these controls.

If a pinned text recipient is no longer current, **Recipient unavailable**
keeps that identity visible and the chooser remains available. A new agent on
the same digit never inherits the old recipient's draft. Returning a held
message to its draft also preserves any newer destination choice you made
while withdrawal was in progress.

## Type or paste to several agents

Open the chain-link **Sync draft** icon beside the composer's microphone and
Send controls, then select the other agents that should share its input.
Hover the icon for the explanation and linked agents' names. Typing,
editing and pasted attachment chips update the same draft in every linked
conversation, including its main overlay and detached windows.

**Send** (or Return) delivers that draft to the selected agents. Reply references
stay specific to each conversation. The draft clears after every recipient has
accepted publication; if one fails, it remains available and Send retries only
the missing recipients. Editing while publication is in progress starts a newer
draft, which the older completion cannot clear.

A composer with a different nonempty draft cannot be linked until that draft is
resolved. **Stop syncing** leaves a copy of the current draft in that conversation
and keeps the other linked composers intact. Channel 0, Your Inbox and saved
owners are not added to a sync group. A replacement agent on the same channel
does not inherit its predecessor's group. This feature shares manual drafts;
it does not change a live recording's audience.

## Agent map in the bar

With Settings → Creator → **Intercom layout** set to **Horizontal strip**, the
expanded bar carries a map of the bus, so you can see who answers which
channel chord without opening the **Agents** window. **Classic** is the same
bar without the map; **Floating pills** keeps its stacked counter beside the
brand, which brings the floating Intercom forward.

Every agent on the bus appears, in channel order: the channel number in a
circle with the agent's name under it, written with a capital first letter.
Only agents appear: channel 0 broadcasts to everyone and has no circle of its
own, except while the microphone is open on it. A circle whose agent has
disconnected is dimmed like its row in the **Agents** list. Pointing at a
circle names the channel, the agent and the chord, for example
`4 · Lena2 · Ctrl+4`, which also tells similar names apart. The modifier is
the one chosen in Settings › Shortcuts › Fn and Agent channels › **Agent
channel** (Ctrl or Fn).

The marks keep their meanings apart:

- a **yellow dot** means an unread reply from that agent; it clears as you
  read the conversation, as in the **Agents** list;
- a **violet ring with a small violet dot** marks every channel the
  microphone feeds now, the audience of what you are saying, one agent or
  several;
- a **muted violet fill** marks the conversation you are viewing.

Clicking a circle opens that agent's conversation, as its pill in the folded
**Agents** list does. It never opens or closes the microphone or changes where
a recording goes; use the channel chord or a microphone control to speak. A
secondary click on a circle opens the agent's menu: channel 0 membership, do
not disturb, speak, playback, show in **Agents**, the agent's terminal or
panel, and archive.

When the full names do not fit beside the tools, the bar takes a second row:
the brand, waveform, clock and tools on top, the map below with each number
beside its name. Only that row's height is added. Narrow the bar by hand and
the map gets denser in steps: the gaps shrink, then names are shortened to no
fewer than three letters and `…` (names of four letters or fewer stay whole,
and the full name stays in the hint). The circles never overlap, and the bar
stops narrowing there. Each name keeps its own space, every circle stays
clickable, and unread dots stay on top.

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

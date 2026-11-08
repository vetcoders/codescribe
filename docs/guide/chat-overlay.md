# Chat & Overlay

Codescribe has two conversation surfaces:

- The **dictation overlay** displays your transcript and conversations with
  external agents connected to Codescribe channels.
- **Show Agent** opens the **built-in Agent**, with its own history, model,
  workspace, tools and composer. It is not the external-agent roster.

This page describes the current development interface. Check your installed
build in **Settings → User** if a control differs. Sidebar resizing and the
recipient-menu changes are part of the next-release interface; attachment
Markup is nightly-only.

## Show and resize the overlay

The overlay can be a minimal control, a compact strip or an expanded conversation.
Use its presentation control or context menu to choose the form. Moving the
window sets its position; resizing changes how much content it shows. The
expanded view supports edge and corner resizing. While you drag, one frozen
image of the header and content follows the pointer. Text keeps its pixel size;
shrinking clips the image and growing exposes background until release. The
conversation renderer keeps its original dimensions throughout the gesture.
On release the window takes the new size and lays the conversation out once.
Cancel restores the original size. The compact strip resizes live; detached
grouped cards retain their lightweight header-and-outline feedback.

The Bus window's minimum width follows the panes you chose and the text scale.
With conversation and agents side by side, it reserves each pane's readable
minimum. Shrinking the window does not fold a pane; use the divider to fold it.

The window keeps its upper-right placement pin when changing form. Mini uses
tighter header insets than the expanded window, so a control's position inside
that pin follows its form's spacing. The Agents hover list uses the parent
palette and the native tooltip material, without an additional sheet-colour wash.

The header gives access to transcription, agents and pinning. Pinning controls **Always on Top**. Unpinning removes that window level; it
should leave the overlay visible in place, without hiding or sending it behind
other windows. Presentation visibility and auto-fold preferences are separate
from pinning. An unsent or edited draft can also keep the overlay open. These controls change presentation, not the microphone's destination.
A small text-tail marker indicates the dictation canvas; it is not a recording
indicator.

A secondary click on the header exposes presentation and placement actions.
The optional **Channel map in the overlay** setting is in **Settings → Creator**.
It provides channel access in the small forms without requiring the full list.

The main agent view, compact channel navigation and floating cards show the
same channels. Choose the view suited to your workspace; opening another view
does not create another agent identity or another conversation history.

## Main agent view and sidebar

Open the roster to choose the conversation you want to read. Its full sidebar
shows names and conversation previews. Drag the divider to resize it: a guide
line follows the pointer and both panes keep their widths until you release.
Releasing past the compact threshold leaves a **numbered rail**. Drag it wider
to recover the full list. The numbered rail remains a navigation entry point.

The current interface uses that divider rather than separate left/right
collapse buttons. Available row actions live in the agent's **More (…)** menu.
Use it for secondary actions such as workspace, quiet/broadcast preferences,
Sync and panel placement, as offered by the connected session.

The **0 · All** card is the shared conversation. **Your inbox** has an envelope
entry and collects messages addressed to you. Saved conversations remain
available separately from a live attached session.

Hover previews let you inspect another conversation without switching to it.
A preview is not an ACK from the agent, a new recording or permission to play
its audio. Opening a conversation and marking messages read are distinct from
merely pointing at its row.

## Floating agent cards

A floating card is another view of the channel. Expand it to see messages and,
when enough space is available, its composer. Compact cards retain the agent's
identity and controls without showing the whole conversation.

Detaching an agent does **not** remove its entry from the roster. Its detached
state distinguishes it from a docked agent; selecting that entry brings its
existing window forward. It should not create a second copy of the agent.

Closing or docking a panel is different from disconnecting its session.
Presentation changes preserve the channel's conversation and unsent draft.
Use the intercom's show/hide control for the cards, and its docking action when
you want detached windows returned. Hiding is not deletion.

## Speak to a channel

Channels **1–9** represent named external sessions. A session must connect with
its provider and identity before it can receive requests. A disconnected row
can still show readable history; that does not make the agent available.

Open a conversation to read it. Use its microphone control to choose whom you
speak to. Use the speaker control to manage spoken replies. Their states are
independent: muting playback does not disconnect the agent or stop the
microphone.

**0 · All** sends to its configured shared audience. Membership and quiet
preferences determine which agents receive it. It is not a shortcut for
individually addressing every name. **Your inbox (10)** is the user's inbox,
not a tenth agent and not the shared channel.

Continuous channel sessions follow **Settings → Agent → AI models**: whether to
keep listening, the speech pause, completion behavior and publication hold.
Ending an utterance need not end the whole session. Check the visible microphone
state before assuming capture has stopped.

## Write to agents and use Sync

The composer separates the conversation you are reading from the agents you
intend to send text to. Use the recipient control beside **Send** to inspect
or change that selection, including in an individual conversation.

- One agent uses that conversation's private draft.
- Several agents use **Sync**, the shared draft for the selected group.
- **All** remains the shared channel with its own audience rules; do not combine
  it with the same agents as individual recipients.

The recipient menu supports selecting several agents before closing it.
The menu shows the selected agents with checkmarks. Its hint names the linked
agents, and the Sync state uses the control's accent rather than another large
permanent label. Open the selection to review or remove recipients before
sending, especially from Your inbox or a conversation whose text destination
differs from the viewed agent.

Sync shares typing, pasted text and draft attachments across the selected
composers. It does not mean several independently focused keyboards, and it
does not change the recording destination. Private drafts are retained when
you leave the shared group.

Sending publishes to the selected recipients. Do not treat a visually cleared
composer as proof that every agent read the message. A partial-send failure
keeps the unsent portion available; retry sends to the missing recipients
rather than deliberately duplicating successful deliveries. Text entered while
a send is in flight belongs to the new draft.

A return-to-current-reply control above the composer brings the conversation
back to the current response after you have scrolled away.

## Attachments

Paste an image or file, or use the attachment control. Check that its chip is
visible in the draft and inspect the recipient selection before sending.
Removing a chip removes it from that draft, not the original file from disk.

Large pasted text can become a text attachment instead of filling the input.
A chip is not a delivery receipt. The recipient also needs the attachment's
content or an accessible path; a path local to one Mac is not automatically
readable by an agent on another host.

Attaching to a recording and sending a written draft are separate operations.
If attachment capture is refused, preserve the draft and use the offered retry;
do not assume the recording or message was sent. When reporting a problem,
include whether the attachment was added before, during or after capture.

### Preview and Markup

Open an attachment's preview to inspect it. In a **nightly build**, preview uses
its own compact, movable, resizable window. The whole image initially fits the
available area; use **Fit** to return to that view after zooming. The source path
is under the **Path** disclosure, while Copy path, Reveal in Finder and Open
remain available. Supported draft images offer **Markup**. Save the edited image
to update the draft attachment; the annotated copy does not overwrite the
original source file. Review the resulting attachment before sending it.

Markup availability is a build distinction. The presence of a preview in a
public release does not imply that its annotation action is available there.

## Delivery and read states

The message's status distinguishes publication, queue acceptance, delivery and
recipient read/ACK. A **Read N/M** state counts recipients that acknowledged the
delivery. It does not certify that their requested work is done or correct.

The sidebar shows a single neutral grey check for **Queue accepted**, two
neutral grey checks for **Delivered**, and the same two checks in the palette's
theme-adaptive olive for **Read**. Queue acceptance alone does not confirm
delivery to the recipient's mailbox. Delivered and Read differ only in colour,
not in shape. The individual Read label uses the same olive; the aggregate
Read N/M remains a neutral count. Receipt colour is separate from agent presence
and voice activity.

An external agent's response can arrive as text and, if configured and unmuted,
as speech. Playback state is separate from message delivery. A missing voice
response does not by itself prove the text was lost.

For replies intended for the user, the agent must explicitly address
**Your inbox / channel 10**. Private peer coordination and shared-channel traffic
are not automatically the user's personal messages.

## Built-in Agent window

Choose **Show Agent** to open Codescribe's own assistant. Its provider, models,
workspace and tool permissions are configured in **Settings → Agent** and
**Providers**. Its conversation sidebar is separate from the numbered channel
roster described above.

Type a request or record a voice turn, attach the context you intend to share,
then send. Selected text and screenshots are context when captured, not a reason
to silently change the destination. A new conversation starts a new history.

The Agent can offer tools according to your permissions. A workspace folder
list is not an operating-system sandbox around processes or MCP servers. Review
requests and approvals accordingly; see [Settings](settings.md#agent) and
[Privacy & Security](privacy.md).

## Appearance and accessibility

The overlay follows the selected light/dark appearance. Increase Contrast and
Reduce Transparency can replace glass with an opaque surface; Reduce Motion
limits decorative movement. A muted or disconnected appearance is a state,
not proof that message text or history has been deleted.

If a control is clipped, a composer hides typed characters, or the cursor
flickers between regions, report the version/build, presentation form and window
size. Those are defects to investigate, not normal recognition behavior. See
[Troubleshooting](troubleshooting.md).

# Bus window contract

This is the canonical contract for the bus window: the floating overlay's bar,
its Agents panel and conversation, and the vertical Intercom with its fiszki.
It states what each surface shows, which code owns it and which behavior a
redesign must keep. A change to that behavior updates this document in the
same commit. The user guide is [`guide/chat-overlay.md`](guide/chat-overlay.md);
the bus protocol is [`TRANSCRIPT_BUS.md`](TRANSCRIPT_BUS.md); gestures and
shortcuts are [`HOTKEYS_CONTRACT.md`](HOTKEYS_CONTRACT.md); delivery is
[`DELIVERY_ROUTE.md`](DELIVERY_ROUTE.md).

Where each surface is specified:

- Bar, mini and the expanded bar with its agent map: [Bar](#bar).
- Agents panel, agent rows and their menus:
  [Start with the visual owner](#start-with-the-visual-owner).
- Conversation and text destination:
  [Horizontal conversation and text destination](#horizontal-conversation-and-text-destination).
- Commands and evidence every surface reuses:
  [Reuse the projections and commands](#reuse-the-projections-and-commands) and
  [Requirements that survive a new skin](#requirements-that-survive-a-new-skin).
- Window sizes per canvas: [Independent canvas sizes](#independent-canvas-sizes).
- Vertical Intercom and fiszki:
  [Intercom layouts and independent panes](#intercom-layouts-and-independent-panes)
  and [Durable fiszki and the group zoom](#durable-fiszki-and-the-group-zoom).

Design inputs: the channel-bar, capsule and popover directions of 2026-10-20,
the [window mockup of 2026-10-29](guide/bus-window-mockup.html) and Monika's
bar brief of 2026-10-30. Mockups set layout and behavior; they do not add
features, colors or values that are not stated here.

## Bar

The bar's first rule is to stay out of the way: it takes as little room and
attention as it can while still telling who is on which channel. Every layout
choice below follows from that.

### Two sizes

The bar has two sizes: **mini** and the **expanded bar**
(`OverlayPresentationMode.mini` and `.midi`). `.expanded` is the window with
the transcription or Agents canvas, not a third bar size. Narrowing or widening
the expanded bar rearranges its content; it never creates another named size
and never switches the size.

A still click on the bar's empty background switches mini ↔ expanded bar in
both directions (`OverlayState.advancePresentationFromHeader`).
`FloatingOverlayPanel.isPresentationClickHit` keeps buttons, channels and
drags out of that gesture, so dragging the background still moves the bar and
every control keeps its own action. The brand dot closes the overlay and shows
× on hover. The chevron, the Agents button and transcription open the window
as before.

### Mini

Mini shows, from the left: the brand dot, transcription, the microphone,
Agents (✦) and the chevron. At rest it shows no waveform, clock or channel
map, and it is exactly as wide as its controls
(`DictationOverlayWindow.miniNaturalWidth`), with no space reserved for hidden
items.

Pointing at ✦ opens an interactive list of the agents on the bus, `N · Name`
in channel order with the yellow unread dot, in the shared `OverlayHintPanel`
(interactive mode, so the pointer can move into it). Clicking a row selects
that agent's conversation through the same path as a map chip. Clicking ✦
itself opens the Agents window.

Mini stays mini while Monika speaks to agents (decision 2026-10-30). The
microphone becomes Stop, and the ✦ slot shows the violet circled digit of every
channel the microphone feeds: one digit, overlapping digits for several
channels, `0` for everyone. Its hint names the agents.

### Expanded bar

The expanded bar shows the brand, the agent map and the tools: transcription,
Agents, pin, microphone and chevron. The waveform and the clock appear only
while recording (`OverlayState.channelMapBarShowsMeter`); at rest they take no
room.

The map lists every agent on the bus, channels 1–9 in order, with the channel
number in a circle and the name under it. There is no `+N` and no digits-only
form: the numbers alone do not say who holds a channel. Channel 0 has no
circle of its own unless the microphone broadcasts on it. A disconnected agent
is dimmed as in the Agents list. The hint names the channel, the agent and the
chord (`4 · Lena2 · Ctrl+4`), which also separates similar names.

When the full names do not fit beside the tools, the bar takes a second row:
brand, waveform, clock and tools on top, the map below with each number beside
its name. Only that row's height is added. The choice comes from measured name
and tool widths (`OverlayBarGeometry`), not from the number of agents.

Narrowing the bar by its left or right edge changes only its width and
densifies the map in this order:

1. one row, names under circles, gaps 8 → 3 pt;
2. two rows, names beside circles, gaps 10 → 6 pt;
3. one row with names shortened (`OverlayChannelPill.truncatedName`, the full
   name in the hint);
4. one row with overlapping circles, down to a 10 pt slot.

Circles may overlap; names never run together, because each slot keeps its own
3 pt gap. Each slot is its chip's hit area, so every channel stays selectable
at every density. Unread dots are drawn on a layer above all circles. The
natural width is one row up to a 520 pt budget, otherwise two rows; the floor
is the densest layout. The window owner and the header compute the same
geometry (`DictationOverlayWindow.midiBarGeometry`,
`FloatingOverlayPanel.setMidiBarGeometry`), and the height follows the row
count (`midiHeight(forWidth:)`). Placement and the saved position still
describe the 200 pt `miniPlacementSize` strip, with mini drawn at its trailing
edge.

### Marks on a channel

Each circle can carry three marks, and they never stand in for each other:

| Mark                       | Meaning                                                                | Source                                                       |
| -------------------------- | ---------------------------------------------------------------------- | ------------------------------------------------------------ |
| Yellow dot                 | an unread reply from that agent; it clears as the conversation is read | the overlay's unread/read state, shared with the Agents list |
| Violet ring and violet dot | the channel hears the current utterance; several channels can at once  | `channelHudStates[digit].open`, the co-audience cohort       |
| Muted violet fill          | the conversation being viewed                                          | the selected conversation                                    |

The bar adds no text such as "Speaking to Lena and As": the marks on the map
name the audience. The violet dot pulses only while the pill animates and
Reduce Motion is off.

Clicking a circle selects its conversation (`selectChannelConversation`, the
rail pill's path). It never opens or closes the microphone and never
retargets a running capture. Its secondary click is the roster's
`agentPillMenu`: microphone, playback and panel for that agent. These actions
are not drawn as buttons beside each chip.

### Look

The bar uses existing palette roles only: graphite background, neutral grey
tools, muted violet selection, yellow unread, and the existing brand and
microphone accents. Mockup colors, including the orange highlights of earlier
concepts, are not palette values. A row is 26 pt; a second row adds 20 pt plus
a 4 pt gap. No tall capsule wraps small elements, and text is not shrunk below
its normal size to make content fit.

Mini and the expanded bar wear the hover preview's glass through
`OverlayCanvasSurface(glass: .popover)`; the open window keeps the `.sheet`
veil. `OverlayPopoverGlass` in `OverlayCanvasSurface.swift` is the one recipe
both read: popover material, a sheet-color veil (0.28 for the preview, 0.56 for
the bar so digits and names stay legible), a 1 pt text-color edge and the
shadow.

### Layout setting

Settings → Creator → **Intercom layout** chooses the map: **Classic** (the
default for an absent preference) is the same bar without the map,
**Horizontal strip (experimental)** puts the agent map in the expanded bar, and
**Floating pills** shows the vertical Intercom with its stacked counter beside
the brand. Mini is the same in Classic and Horizontal. `OverlayChannelMapPreference`
owns this process-only chrome choice in the app preference domain, as
`AppearancePreference` does; it is not core configuration. Creator and every
overlay observe that same owner. Changing it asks the existing window owner to
refit, including during a take, and keeps the microphone, selected owner,
transcript and explicit width.

### Code owners

- `OverlayChannelStatusView` owns the Agents (✦) button in every form; mini
  turns on its hover list and recipient digits through `miniAgentsButton`.
  The vertical layout leaves them off, since its stacked counter shows the
  digits.
- `OverlayChannelStatusView.channelMapModel` builds `OverlayChannelMapModel`
  from the roster's rows, `hudStates`, conversations, selection and unread
  counts; lease names are capitalised for display only.
- `OverlayChannelPill` is the single owner of the numbered pill visual: rail
  pills, map chips, the stacked counter and mini's recipient digits. It
  receives immutable display values and one action, never `OverlayState`, a
  conversation or a command.
- `OverlayBarGeometry` and `DictationOverlayWindow` own the bar's measured
  layout and window size; `FloatingOverlayPanel` applies it.
- While the map is enabled, a channel the controller opens in mini or the
  expanded bar keeps that form (`OverlayState.channelOpenExpandsSurface`); the
  Agents canvas unfolds for it only with the map off or when the window is
  already open. The vertical layout's counter calls
  `OverlayState.activateChannelMapCounter`, which brings the Intercom group
  forward.

### Bar acceptance

Before a bar change is called done, check it visually at real scale: four and
nine agents; full and similar names (`Lena`, `Lena2`); densification while
narrowing by hand; legibility of numbers, names and unread dots; selecting one
channel among overlapping circles; speaking to one agent and to several; the
agent list in mini; the bar's proportions; and the existing transcription,
Agents, pin, microphone and window actions.

## Start with the visual owner

| Change                                                                 | Existing owner                                                                          |
| ---------------------------------------------------------------------- | --------------------------------------------------------------------------------------- |
| Agent name, clock, descriptor and message preview                      | `OverlayAgentRosterLabel.swift`                                                         |
| Row footer, action grouping, broadcast row and compact channel pills   | `OverlayChannelStatusView.swift`                                                        |
| Numbered channel pill: rail pills, map chips, counter, recipient marks | `OverlayChannelPill.swift`                                                              |
| Bar agent map, its layout ladder, mini Agents slot, stacked counter    | `OverlayChannelMapModel.swift`, `OverlayChannelMapView.swift`                           |
| Header, waveform placement, embedded list/rail and hover region        | `DictationOverlayView.swift`                                                            |
| List width, docking threshold and rail/pill dimensions                 | `OverlayAgentDrawerLayout.swift`                                                        |
| Panel presentation, selection and recipient projection                 | `OverlayState.swift`                                                                    |
| Read evidence, conversation messages and delivery metadata             | `OverlayChannelDelivery.swift`, `OverlayChannelDeliveryReader.swift`                    |
| Native mini/midi/expanded window size, position and morph              | `DictationOverlayWindow.swift`                                                          |
| Overlay colors                                                         | `OverlayAppearancePalette.swift`, projecting `CSPalette` in `DesignSystem/Tokens.swift` |
| Shared microphone/speaker symbols, glass and hit areas                 | `OverlayAgentAudioControls` in `OverlayChannelStatusView.swift`                         |

Swift source paths above are relative to
`macos/Codescribe/Screens/Overlay/`, except `DesignSystem/Tokens.swift`.
The XcodeGen app target includes `Codescribe` recursively; add a visual view
there rather than maintaining a second source list or editing generated projects.

`OverlayAgentRosterLabel` receives immutable display values: title, channel and
conversation IDs, descriptor, preview, timestamp, unread count, status dot and
accessible explanations, reception policy, palette and text scale. It does not
receive `OverlayState`, a controller, bindings, commands, tasks or a bus reader.
`rosterRowLabel` in `OverlayChannelStatusView` composes those values once per
row. Restyle that view, or compose the same projections into a different visual
surface. Keep the policy symbol/help from `AgentBroadcastPolicy`; its meaning
does not belong to the visual style. `OverlayAgentUnreadBadge` is shared by
current and saved conversations.

The [approved window mockup from 2026-10-29](guide/bus-window-mockup.html)
records the compact-card layout and its matching More/right-click menu.
The runtime action semantics and availability are defined below.

An agent row has three blocks: identity and controls, the provider/model/project
descriptor, and a routed message preview of at most two lines. There is no
separate state-and-actions footer. The waveform microphone, speaker and more
menu remain visible beside the identity without hover. The conversation button
and these controls are siblings; opening a conversation never opens its
microphone. Long names retain their full tooltip and accessibility label.

The card header gives the channel digit, status dot, agent name and receipt
priority over secondary metadata. Digits keep their intrinsic width; long names
can use two lines. The clock and circular unread count share the metadata line
below the header so standard sidebar widths do not squeeze names into an
ellipsis or push channel digits outside the row. A theme-colored separator
marks each row boundary even when it is neither selected nor hovered.

Hovering the row's navigation area or a folded-rail entry for 350 ms opens
the last loaded message in the shared `OverlayHintPanel`. The same message
as the inline preview is used, including outgoing user messages; original
paragraphs, rich formatting and attachments use `OverlayConversationMessageContent`.
The preview is bounded to the display and scrollable. Its pointer target stays
open while the cursor crosses the gap or rests on the preview, then dismisses
after leaving both targets. The window cannot become key. It mounts no
composer, changes no selected conversation or recipient, acknowledges no
message, and fetches no saved history. Audio and menu controls keep their own
action hints. Empty entries retain their existing navigation hint.

Roster previews use a maximum 330-by-240-point viewport at text scale 1.
Short messages fit their natural height; long messages use the bounded native
scroll view. The preview uses native popover glass with a light sheet-color
veil, a thin appearance-aware edge and a window shadow (`OverlayPopoverGlass`,
shared with the bar). These separate it from
conversation bubbles beneath it while retaining compact insets. Message body
text keeps its existing readable size. Ordinary hints and fixed rich previews
on the floating Intercom retain their tooltip material and existing presentation.

The more menu contains channel-zero membership, Do not disturb, the detected
client/terminal destination and removal into saved conversations. The same
commands are available from the row's context menu. Existing pending,
availability, frozen-owner and archive guards remain in force. An open
microphone can still be stopped after its reader disconnects, and playback can
still be muted. A client-focus request keeps its progress and result on the
more control and in the menu; removing the footer must not erase that feedback.

`cardAgentMenu` selects only these secondary actions from the existing command
owner. Card menus do not repeat Write/Forward, microphone, playback or Show in
Agents; the full menu remains available on other pill surfaces. The same
subset is used by the conversation header's More control.

Opening the workspace dispatches the existing `focusAgentPanel` command to
the detected Frame pane, terminal or Codex thread. `showAgentInMonitor` is a
separate navigation action. Removing a card uses `archiveAgent`: the exact
owner is verified, an open microphone channel refuses the operation, a live
reader is stopped before its digit is released, and the lease, backlog and
saved conversation remain retained. No new action or archive semantics are
introduced by the mockup's labels.

Reception controls live in the menu, but restrictions remain visible by the
name: a moon while Do not disturb is on, and a crossed antenna while the agent
is outside channel zero. These are passive projections of the existing
`OverlayBroadcastPolicyLedger`, not another policy writer. Disconnected rows
dim without adding a Disconnected text line and remain available for history.

The compact receipt mark describes the newest completed user delivery to the
exact frozen recipient. A single check means a delivered receipt; the compact
double-check shape means a read receipt or the canonical agent acknowledgment.
Queue admission and provider acceptance are not delivered/read evidence. Held,
skipped and unreceipted deliveries keep distinct marks and explanations. The
mark sits near the identity, separately from the last-preview clock; its help
and accessibility text name the time of its own outgoing message. A later
incoming reply must not lend its time to that receipt. The circular unread
reply count is a separate projection. Compact map and floating Intercom badge
rendering retain their existing form.

## Horizontal conversation and text destination

The horizontal window treats the conversation as its primary content and has
one Agents panel. One persisted presentation owns the expanded list, compact
channel bar and optional complete hiding. The expanded list is a resizable
right column when the window can fit a 320-point conversation and a 240-point
roster at the current text scale. Below that threshold it covers the chat at
the current width; revealing it does not enlarge the window. Selecting a row
returns a covering list to its compact bar and keeps a docked full list open.
The panel button switches list and compact bar; its context menu also offers
complete hiding. The main Agents mode/capture control keeps its recording
contract and uses the same panel owner for ordinary list requests.

The conversation remains mounted beneath a covering panel so the editor,
draft, selection and scroll state survive the placement change. A fully hidden
panel reserves no conversation width. The compact bar uses uniform centered
channel buttons, with symmetric clearance for a small circular unread badge
at each upper trailing corner. The badge overlaps the corner; it does not
receive a separate column. Two-digit values remain inside the bar, and values
above 99 display 99+ while accessibility keeps the full count. The status dot
comes only from channel status, the yellow number only from unread replies,
and the button background only from the viewed conversation, including Inbox.
Unread changes never recolor all status dots.

### Panel state and status signals

`OverlayState.agentPanelPresentation` is the persisted owner of the three
panel forms: `expanded` shows the full list, `compact` shows the rail, and
`hidden` reserves no rail width. `showsAgentMonitor` is a projection of this
state. The primary panel control switches expanded and compact; its context
menu can hide the panel completely. The hidden state retains a header control
for reopening it. Earlier visibility and layout preferences are adopted once;
the new presentation then owns subsequent writes.

The 38-point rail button width scales with text. Symmetric horizontal gutters
leave room both for the resize hit band and the largest unread badge; the
button, not the overhanging badge, is centered. The badge is on the upper
trailing corner, mirrored in right-to-left layouts, and does not add a hit
area. Inbox uses the same selection and unread treatment as agent buttons.

The status dot is a separate projection from `statusDotKind(for:)` in
`OverlayChannelStatusView`; unread does not determine its color. The status
check handles an unavailable status or disconnected follower before checking
whether the channel is listening, stalled or skipped, then its delivery stage.
A listening dot uses `listeningStatus`, a received dot `successStatus`, a
waiting or held dot `processingStatus`, and idle or sleeping states use an
unfilled `mutedText` ring. The shared broadcast marker uses `agentAccent`.
These palette roles adapt to the current appearance; they are not delivery
receipts or evidence that the agent is actively reading. A selected button
uses the agent-accent background independently of both dot and unread badge.

Conversation and panel headers share one compact band with one panel control
at the window's trailing edge. The conversation has no independent fold or
reveal edge. Divider resizing clamps the full column without hiding the chat.
Old split/agents/rail and Boolean visibility preferences are adopted once into
the single presentation; later layout reads that presentation and geometry.

Message actions in `.busWindow` stay inline where they fit and use additional
rows when their intrinsic widths exceed the message cap. Read receipts, Copy,
Reply and playback keep their actions and labels. The content itself must fit
its cap after narrowing; clipping an oversized child is not a width policy.
Standalone and floating Intercom message-action layouts retain their existing
presentation.
The existing All, Inbox and saved-conversation views keep their current data
and routing semantics; the earlier proposal's additional history filters are
not part of this change.

The horizontal `.busWindow` presentation gives its compact navigation and
composer real layout space above and below a clipped message viewport. Only
history scrolls; messages never pass behind the agent identity, controls or text
destination. The destination and voice status share one quiet row when they
fit, and use two compact rows at narrower widths. The standalone `.standard`
and floating `.intercom` presentations keep their existing layout. Text scale,
resize, retained history, reply navigation, attachments, draft dictation and
draft synchronization remain supported.

### Conversation, destination and microphone

In candidate 19 the visible conversation, text destination and open microphone
are independent. This is a description of the current implementation, not a
settlement of the different recipient model under review.
The text destination is named beside the composer and can be selected
explicitly. `OverlayState.composerTarget` is one current selection: ordinary
row, map and rail navigation changes the viewed conversation but does not reset
that selection. For example, after choosing agent A as the text destination,
opening agent B's conversation can still show A in the destination field and
use A's draft and Send route. The explicit Write/Forward and Reply actions
choose their own conversation as the destination. The send path freezes and
validates the selected identity; a UI label or successful selection alone is
not a delivery receipt.

A separate action chooses the viewed conversation as the text destination.
Selection pins conversation ID, channel and frozen owner identity;
reusing a channel digit cannot adopt another owner's draft or pending reply.
All drafts, replies, attachments, editor identity and send callbacks continue
to use the existing per-conversation stores. A missing destination stays named
as unavailable with a usable chooser instead of falling back to the viewed
conversation. The microphone status reports actual open channels, separately
from dictation into the text field.

Native Undo remains scoped to the mounted editor. Switching the frozen text
destination preserves each owner's stored draft, but starts a fresh native
undo stack, as switching conversations did before. Undo never crosses owners.

Replying to a displayed message and the explicit Write/Forward menu command
choose their own frozen conversation as the text destination. Ordinary row,
map and rail navigation only change the viewed conversation.
Returning a held message to the draft first completes its withdrawal, then
returns text to that frozen private owner or to the original channel-zero
conversation. Navigating to a quoted reply opens its own history and retains
the requested message ID while history loads. None of these changes creates a
second document, recorder, bus writer or delivery route.

### Acceptance for the Agents panel and conversation

Before calling a change accepted, inspect the full list, compact rail and
hidden state in both wide and narrow windows. Check a covering list returns to
the rail on selection while a docked list stays open, and that the composer,
draft, selection and scroll position survive the transition. In light and
dark appearances, including enlarged text and right-to-left layout, check
centered buttons, an Inbox selection, independent status and unread signals,
and counts 1, 42, 99 and 99+ without clipping. Check the full count through
accessibility. Exercise actual pointer and keyboard controls, not only a static
render. Separately verify viewed conversation, displayed text destination,
frozen recipient, open microphone and Sync membership before a real send; use
consenting test recipients and verify the resulting receipt. Test the explicit
Write/Forward and Reply actions, a missing owner, channel reuse and a draft
retained during navigation. Native interaction and delivery are distinct from
source review, test fixtures and image rendering.

The floating Intercom windows and the mini/expanded bar are outside this
horizontal-window cut. Shared view parameters default to the existing Intercom
behavior, and shared control renderers retain their existing appearance.

`0 · All` is the exception. Its title button keeps the identity line and the
clock. The outline megaphone and the speaker sit beside a three-line preview
of the last message, with one mark size, one frame and one hit target. Those
controls are not in the Agents header, and they are not nested inside the
title button. Three preview lines belong only to All. An open microphone uses
the same violet as a speaker that is playing a reply (`agentAccent`). The
existing pulse and Reduce Motion stay. Broadcast, read-mute and Channel 0
membership remain different controls.

A bubble names the sender and the audience with
`OverlayChannelStatusView.messageRoute`. The names come from that message's
source channel and frozen recipients, so a later selection does not rewrite
them. Channel 0 reads `All`. A private delivery names the frozen recipients
(`You → lena, astra`). A reply with no destination, and a channel 10 reply,
read `sender → You`. Attachment-only messages use the same route. The receipt
menu lists the frozen recipient set of a broadcast. A speaker who is not the
open conversation stays a button that opens their conversation; the arrow and
the audience are text.

User-message bubbles retain a quiet warm outline using `terracottaDeep` at
`CSAlpha.selectionStroke` strength. Incoming bubbles use the neutral panel edge.

Moving controls must not turn a whole row into one button or nest buttons
inside another button. A list row or bar map chip selects its
conversation, including `0` for All. Selection does not open a microphone
and does not retarget a capture that is already running.

## Reuse the projections and commands

`OverlayState` remains the UI projection and command coordinator. Its
`visibleChannelRows`, `channelHudStates`, conversations, selection, pending
commands and errors feed `channelStatusView` in `DictationOverlayView`.
`OverlayChannelDelivery` and `OverlayChannelHudProjection` carry evidence;
`OverlayAgentGlyph` and the existing status helpers render that evidence.
No visual component should poll another roster, cache another conversation,
create receipt state or open a recorder.

| Visual interaction             | Existing path to preserve                                         |
| ------------------------------ | ----------------------------------------------------------------- |
| Reveal/fold the agent list     | `unfoldAgentList`, `foldAgentList` and existing sidebar callbacks |
| Select a conversation          | `onSelectConversation` / `chooseConversation`                     |
| Toggle microphone for a digit  | `onToggleChannel`, then `OverlayState` and the controller         |
| Toggle reply playback          | `onTogglePlayback` / conversation playback command                |
| Open the agent's terminal pane | `canFocusAgentPanel` and `focusAgentPanel`, with the exact owner  |
| Archive an agent               | `onArchiveAgent`, with the exact conversation owner               |
| Change broadcast reception     | `OverlayBroadcastPolicyMenu` and its existing command             |

These callbacks already carry pending/error and availability guards. A new
popover or horizontal map should use those paths, retaining selection as
navigation and microphone controls as explicit capture commands. A channel
digit or display name alone is not a session identity.

Below the UI, `RecordingController` owns the microphone,
`app/controller/delivery_route.rs` owns delivery routing,
`TranscriptReducer` owns the transcript and
`app/presentation/transcript_bus.rs` observes committed events. A design change
does not need changes to these owners.

## Requirements that survive a new skin

- On the Agents surface, the compact agent rail and its reveal button are
  available from the first reveal, before any conversation has been selected.
  A visible full list replaces the rail. Hover over its disclosure control
  reveals that same list; hovering numbered pills keeps their click targets
  in place. Do not mount either only after opening an agent conversation.
  The transcription surface does not mount that rail, its pills, or the agent
  list. Its trailing column is the transcript actions sidebar
  (`overlay-intent-dock` inside `overlay-transcript-actions`). That column
  does not read the agents drawer width. Both trailing columns reserve the
  native window's unscaled resize band outside their controls. A nil
  conversation on Agents stays on Agents, including after inventory retires
  the selected owner.
- The conversation's agent pill focuses an available, detected Frame pane.
  When that target is unavailable, the label stays passive; a new shell must
  not display an unsupported navigation option.
- Capture-open, follower-alive, queued, acknowledged, unread and speaking are
  different facts. ACK confirms receipt, not task completion. Color, elapsed
  time, a hover animation or an unread badge must not manufacture a receipt.
- Delivery states come from `runtime/receipts/<message_id>/` only (the
  contract is the Receipts section of `docs/TRANSCRIPT_BUS.md`): the reader
  lists the directory of every unsettled message when its stamp changed (a
  copy neither read nor skipped, a channel-0 text without a known list, one
  of the newest replies not yet heard) and hands all its files to the
  composer, `OverlayReceiptState.compose`. The projection keeps the composed
  state (`OverlayConversationMessage.receipt`) and derives from it the
  recipient rows (`delivered`, `read`, `skipped`), the admitted list
  (`sent.json`, else the files' stamp) and a reply's playback
  (`playback.<ticket>.json`). The ACK and skip markers under
  `acknowledgments/` and `broadcast-skipped/` are not read by the overlay:
  one owner per state, no fallback to the previous files. A write in
  progress (`.*.tmp`) is never read; a file whose name and content disagree
  marks nothing. A delivery without a `message_id` (a plain dictation seal)
  has no directory and stays "Addressed". Provider acceptance (`wakeups/`),
  a lease's `pending` and the app's own `codescribe.agent-ack.v1` bus row
  still mark a copy's row as accepted, queued or read; they are hints ahead
  of the files, not receipts: they never count and never end the listing,
  which stops only when the composed files show every copy and every listed
  agent as `read`, `skipped` or withdrawn. A reply additionally leaves the
  poll once its playback is known from either source, the bus's playback
  rows or the receipt file. The poll costs at most 128 directory
  `fstatat` calls plus 128 wakeup `fstatat` calls per tick and bus. A reply's
  "Spoken" label is decided by one ticket arbiter (`applyPlayback`) for the
  bus's playback rows and the receipt files alike; the composed `spoken` is
  an input to that arbiter, not a second label.
- "Read X/Y" counts the agents the message was sent to, not its copies.
  Y is the contract's denominator (`OverlayConversationMessage.readReceipt`):
  the admitted list (`target.recipients` on the copy, outranked by
  `sent.json`) plus every agent delivered to or read outside it, minus every
  agent whose copy was skipped. A skipped agent is shown as "Skipped: off
  channel 0" and is not an unread one: a skip is final, so it never counts.
  An admitted agent with no copy and no file is "No receipt" and counts: its
  reader was not running or predates receipts. Without a list the copies are
  the list, today's denominator; a channel take's admitted owners are its
  `recipients`. **One contract, two implementations:** the overlay cannot
  call the helper, so it computes the same formula a second time, from the
  same files, in one place: the composer the reader feeds. There is no
  second state machine next to it; the count is read off the composed state.
  Both implementations are pinned to the model directories under
  `scripts/tests/fixtures/receipts/<case>/` with their `expected.json`, every
  new state goes to those directories first and then to both, and the
  overlay computes only the formula: no roster, no policies, no reach of its
  own. `delivered` with `wake: null` is history the helper backfilled.
- A fresh message moves the selected conversation only when it is addressed
  to the Founder: an agent's reply on its own channel, or a message to her
  inbox (channel 10). A message between agents (`sourceChannel` 1–9) and any
  reply that sits in channel 0 never move the selection or show the panel;
  they stay unread in their conversations (Founder decision 2026-10-26).
  `OverlayState.followNewlyReceivedReply` is the one owner of that choice. It
  writes one `overlay-presentation` line per choice:
  `conversation.auto_selected` (reply ids, owner, source, the channel before
  and after) or `conversation.auto_skipped` (reply id, owner, source, reason,
  the channel that stays), with ids and channels only, never the text. A
  message to the Founder whose owner is not unique is skipped and traced
  the same way (`ambiguous_owner` or `no_owner`).
- The panel's chrome and resize edges own the cursor: they show the arrow or
  the resize cursor before native dispatch, so a text view underneath cannot
  set an I-beam there. Native text views, editable fields and links keep their
  own cursor (`FloatingOverlayPanel.pointerDecision` in
  `DictationOverlayWindow.swift`).
- Bus conversations, the Agent window and the prompt preview render messages
  through one `MarkdownText` renderer; a new skin restyles it instead of adding
  a second Markdown path.
- Settings configures agents; the live roster belongs to the bus and the
  overlay projections. Settings keeps no second roster of connected agents.
- Scale text through the existing text-scale projection, preserve Reduce
  Motion/Reduce Transparency, tooltips, keyboard focus and accessibility IDs.
  Change a symbol's font separately from the button's hit area.
- Use the existing palette roles. A one-line bar and a two-row bar are layout
  choices, not reasons for a second status palette or persisted channel model.
  If a design needs a different native window size, update the existing window
  owner and its geometry checks together; SwiftUI layout alone cannot prove it.

## Independent canvas sizes

The transcription and agents canvases use the same native panel, with a last
expanded size for each `OverlayContentSurface`. `OverlayState.windowContentSurface`
retains the intended canvas while mini/midi hide its body. Selection and capture
routing update that projection; they do not create another transcript or recorder.
`FloatingOverlayPanel` owns both size caches, their transition and persistence.
The controller retries the latest projection after an active drag or morph ends.
Mini/midi dimensions never overwrite an expanded profile.

Preferences live at `DictationOverlayPanel.contentSize.v7.transcription` and
`.agents`, each with `.w`/`.h`. The previous shared v6 frame migrates to agents
only: it may be a full-screen bus. Transcription starts at 470 × 280 until sized
independently, then restores its own last size. Manual resizing of agents does
not disable transcript growth. A new ordinary capture selects transcription
before it unfolds; an agent take retains its channel surface. A composer take
keeps both the surface and the form (`applyTakeStartPresentation`). An ordinary
take opens the window when expand-by-default is on and otherwise the expanded
bar (`OverlayState.applyPreferredExpansion(forTake:)`).

The fold button has the same top/trailing inset and header row height in every
form. Native morphs, canvas switches and transcript growth preserve that screen
point, subject to keeping the window on screen. Initial placement and an explicit
placement command still use the selected anchor. Direct edge dragging keeps its
normal opposite-edge behavior. Use the existing header-frame preference for
geometry evidence; do not add a second anchor store.

## Integration and visual acceptance

This document is not proof of a running design. On 2026-10-20 the Founder
assigned Astra the UI/UX lane and explicitly authorized both Astra and Lena to
compile their separate scopes, superseding the earlier source-only assignment.
Keep unfinished generations in their worktrees; the main checkout must remain
stable and installable. Check the admitted generation at first launch with no
selected conversation; folded/expanded lists; wide/narrow windows; channel 0
and individual agents; pending, unavailable and disconnected states; and the
supplied large-text screenshots. Geometry checks should cover row widths
240/300/400 at text scales 1.0/1.5/2.0, including trailing clock alignment,
button bounds and the five-line budget. Preserve action and accessibility
coverage when moving a view to another file; source-shape assertions may need
to follow the new owner.

### Intercom layouts and independent panes

Creator's **Intercom layout** picks the layout. Horizontal puts the agent map
in the expanded bar; each agent's microphone, playback and panel actions live
in its chip's secondary-click menu. Vertical is an independent desktop
Intercom, available with the main overlay closed or folded. Its header and
agent pills share a 30 pt base height, 11 pt semibold names and a 30 pt
microphone beside a 30 pt speaker. Text scale grows these metrics. The common
resting width is measured from the header and the longest name, with numbers
and names aligned left and the audio pair right. Any free spot on the header
moves the whole group (Maciej, 2026-10-23); the wordmark folds it and carries
the saved-fiszki menu. Native capsule windows leave the gaps to the desktop. When
large text exceeds a short display, the header's previous/next buttons and
scrolling over a passive capsule move a bounded range of full-size pills.
Expanded message scrolling stays local to its conversation.

Hover expands only that pill to the left with agent information and a message
preview. It never acquires keyboard focus or changes capture. Clicking the
name expands that same card down into `OverlayConversationView`: messages,
editable draft, attachments and manual Send, without opening the main overlay.
The expanded card is bound to the exact conversation/provider/session/lease,
so another hover or a reused channel digit cannot replace its draft or target.
Close collapses the card and retains the draft. A real drag of its
window-with-plus grip detaches the same window; a motionless click never
detaches or moves anything. The group stays unfolded with the detached row
greyed; clicking the header folds and unfolds it and refits the visible rows
to the screen. The left edge and bottom corners of a detached window resize
that conversation through 8 pt and 18 pt bands that light their edge under the
pointer. The card becomes key only after an explicit click.

### Durable fiszki and the group zoom

A detached conversation is a durable **fiszka** (Maciej, 2026-10-22): a view
instance with its own persistent identity, separate from the conversation it
presents. One agent keeps one open fiszka (Maciej, 2026-10-23): while it
stands on the desktop, its row stays in the group greyed out with a window
glyph, hover slides nothing out, and clicking the row — or dragging or
resizing an expanded card of the same conversation — shows that window
(restored from mini if needed) instead of minting a copy. Each fiszka records
its own position, size and mini/closed window state, and all of them observe
the one shared conversation, draft and command state. Restore
on the next launch is a UI operation: windows come back exactly as left, a
closed instance stays closed, mini returns as mini, and nothing opens a
microphone, re-sends a message or changes recipients. An instance follows the
exact conversation owner; a reused digit or an identical display name never
rebinds it. When the exact owner is gone the saved words stay readable and the
live actions withdraw.

Every instance window has minimize and close. Both are window operations: they
never disconnect, archive or re-address the agent. Any free spot on the
window's header moves it — not one grip on one side (Maciej, 2026-10-23) —
and the header, on the control glass of the composer and the name pill,
carries the same right-click menu as the row in the group.
Mini rests with the name, the activity dots and the audio pair, measured
to its own compact width at the stored window's top-right corner; clicking the
fiszka restores it, and hover reveals the close control in its fixed place, so
the audio pair never moves. Closing records the fiszka as closed and the group
row returns to its live state; detaching the same conversation again reopens
that saved window instead of minting a duplicate. A fiszka's audio pair is
`OverlayAgentAudioControls`, the roster's: monochrome at rest, the accent and
a pulse when lit, one control height (channel 0 keeps the megaphone without
the roster's larger tarcza) — painted flat on the fiszka with `chrome: false`,
since the fiszka is the surface, not in glass circles of its own (Maciej,
2026-10-23: "robisz to prosto na powierzchni fiszki"). Its status is one
`OverlayAgentStatusDot` from `statusDotKind`, before the figure; after the
name sit the agent's two reception marks (`OverlayReceptionMarks`): the
channel-0 antenna (`broadcastPolicy.statusSymbol`, lit for a member) and the
do-not-disturb moon (`quietSymbol`, filled while lit). Both are one-click
switches that issue the helper's `--broadcast`/`--quiet` command for that
lease, the same view the agents list draws after its title; the axes are
independent (Maciej, 2026-10-23). After the pair comes the list's focus button,
drawn from the pill menu's `focus` action (`agentPillMenuActions`); the archive
X stays in the full view only. The bar wears the All row's antenna before
**Intercom** — a state icon, no status verb — and the wordmark keeps one line
(the controller measures the antenna, both marks and the focus slot into the
resting width). The Intercom header's
context menu lists the saved closed fiszki, the explicit road back when the
channel's owner is no longer in the group. Resizing an expanded group card is
an explicit independent placement and turns it into a fiszka with its own
geometry; the group stays unfolded.

The attached group resizes like one window. The left edge of the header and
the left edge plus bottom corners of any resting pill scale every attached
fiszka proportionally; the drag ratio becomes a persisted group zoom between
1 and 2.5 over the text scale. Detached fiszki keep their own geometry and the
plain text scale. The persisted desktop (instances plus the group zoom) lives
with the other chrome preferences in the app domain, never in the engine's
runtime configuration.

The microphone slot uses the existing microphone command; the speaker beside
it is that conversation's automatic-playback mute (Maciej, 2026-10-23: mute
the agent while it speaks), lit while `conversation.replySpeaking` is true.
The header's pair is channel zero: the broadcast megaphone and the master
mute. An explicit Play on a message lifts both mutes for that one reply; a
mute switched on while it speaks still stops it. Actual speech also brightens
that agent's name, including in the collapsed pill. An open nonzero channel
has a violet dot on its digit and a violet microphone. The pair is independent
of the conversation click. Agent pills, the roster, the chip strip and the
Intercom slot wear `waveform.badge.mic`, the live-speech glyph; the composer's
dictation control keeps the plain `mic`, the dictation verb (Maciej,
2026-10-23).
Channel activity updates an already visible main overlay, but the vertical
Intercom does not summon a closed main overlay; Show in Agents remains explicit.
The floating pills retain their rich informational hover. The classic Agents
list keeps its inline actions and also offers the last-message hover described
above; provider, model, status and session metadata are not repeated in a
second tooltip.
The same `OverlayChannelMapPreference` stores the layout; switching it preserves
the selected conversation, drafts and active route. No second inventory or
recorder is created. The default remains horizontal, and an absent opt-in stays off.

The horizontal Agents panel follows the responsive placement and single
visibility contract in “Horizontal conversation and text destination” above.
Floating Intercom groups, detached cards and the bar's agent map retain
their separate presentation rules.

Open nonzero capture uses a violet microphone; only an open channel zero uses
the outlined broadcast megaphone. These glyphs project routing and never choose it.

### Pinned All row

The full Agents column renders `0 · All` immediately below its header, outside
the scroll viewport. Only channels 1–9 and saved conversations scroll below it.
All retains its three-line preview, recipient counts, microphone/megaphone and
playback controls. There is one All row and one projection of its state.
Height and text-scale changes reduce the agents' viewport, leaving All in place;
the final agent remains reachable by scrolling. Keyboard focus can land on All,
but scroll-to-selected only targets IDs inside the scrolling content.

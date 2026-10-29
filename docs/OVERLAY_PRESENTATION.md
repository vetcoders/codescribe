# Overlay presentation handoff

The Founder supplied channel-bar, capsule and channel-popover designs on
2026-10-20 and asked for a visual layer Monika can reshape without rebuilding
capture or bus handling. Those images are visual directions, with several
alternative arrangements; this document does not select one as the final UI.

## Start with the visual owner

| Change                                                                  | Existing owner                                                                          |
| ----------------------------------------------------------------------- | --------------------------------------------------------------------------------------- |
| Agent name, clock, descriptor and message preview                       | `OverlayAgentRosterLabel.swift`                                                         |
| Row footer, action grouping, broadcast row and compact channel pills    | `OverlayChannelStatusView.swift`                                                        |
| Numbered channel pill: rail pills, map chips, counter, open-channel dot | `OverlayChannelPill.swift`                                                              |
| Channel map in mini and compact: tiers, counter, its click              | `OverlayChannelMapModel.swift`, `OverlayChannelMapView.swift`                           |
| Header, waveform placement, embedded list/rail and hover region         | `DictationOverlayView.swift`                                                            |
| List width, docking threshold and rail/pill dimensions                  | `OverlayAgentDrawerLayout.swift`                                                        |
| Native mini/midi/expanded window size, position and morph               | `DictationOverlayWindow.swift`                                                          |
| Overlay colors                                                          | `OverlayAppearancePalette.swift`, projecting `CSPalette` in `DesignSystem/Tokens.swift` |
| Shared microphone/speaker symbols, glass and hit areas                  | `OverlayAgentAudioControls` in `OverlayChannelStatusView.swift`                         |

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
scroll view. The preview uses the shared tooltip material, compact insets and
no window shadow. Message body text keeps its existing readable size. Fixed
rich previews on the floating Intercom retain their existing presentation.

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
reply count is a separate projection, also shown on the folded rail; compact
map and floating Intercom badge rendering retain their existing form.

## Horizontal conversation and text destination

The horizontal window keeps the conversation header and its current-agent
actions visible with either the full sidebar or its folded rail. The sidebar
choice does not replace the current conversation, draft or microphone target.
Folded-rail controls sit on the full column's centerline, with equal horizontal
gutters that keep the numbered buttons clear of the native resize band.
Unread badges occupy a separate trailing gutter outside the capsule, with a
visible gap from the channel digit. That gutter remains inside the scroll
viewport so the count is neither painted over the digit nor clipped. Rail
badges cap their visual label at `99+`; accessibility retains the exact count.
The existing All, Inbox and saved-conversation views keep their current data
and routing semantics; the earlier proposal's additional history filters are
not part of this change.

The standard conversation reserves its composer space below the message
viewport. The shared `.intercom` presentation keeps its existing floating
composer arrangement. Text scale, resize, retained history, reply navigation,
attachments, draft dictation and draft synchronization remain supported.

The visible conversation, text destination and open microphone are independent.
The text destination is always named beside the composer and can be selected
explicitly. A separate action chooses the viewed conversation as the text
destination. Selection pins conversation ID, channel and frozen owner identity;
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

The floating Intercom windows and the mini/compact top bar are outside this
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

The channel map in mini and compact is another surface of the same
projections. `OverlayChannelStatusView.channelMapModel` builds
`OverlayChannelMapModel` from the roster's rows, `hudStates`, current
conversations, the selection and unread counts. It lists the bound agents
1–9, with lease names capitalised for display only; channel 0 joins it only
while the microphone broadcasts on it. A chip calls
`selectChannelConversation`, the rail pill's selection path. The counter calls
`OverlayState.activateChannelMapCounter`: in mini it selects compact, in
compact it asks the window owner, through `onRestoreMidiNaturalWidth`, to drop
a width the user set (`FloatingOverlayPanel.restoreMidiNaturalWidth`). There is
no map popover; the counter's hint lists the agents. While the map is
enabled, a channel the controller opens in mini or compact keeps that form
(`OverlayState.channelOpenExpandsSurface`): the selection follows the open
channel and the strip shows its digit, and the Agents canvas unfolds for it
only with the map off or when the overlay is already expanded.

`OverlayChannelPill` is the single owner of the numbered pill visual: the rail
pill, the map's chips and its counter render the digit through it, from
immutable display values and one action, and an open channel's digit carries
a small dot in the agent accent there (spec §14: a dot, no symbol, never
`MIC →` text; the microphone and megaphone signs belong to the Agents header
button). The dot overlaps the digit's circle, so it adds no width, and pulses
only while the pill animates and Reduce Motion is off. It receives
no `OverlayState`, conversation or command. Compact's strip picks the first
tier that fits its width: named chips, digit-only chips (the selected
conversation's chip keeps its name), the counter, nothing. Every threshold is
measured from the same text metrics the pill draws with, and the window owner
sizes compact from them too: `FloatingOverlayPanel.setMidiNaturalWidth`
receives `DictationOverlayWindow.midiNaturalWidth`, bounded by the 820 pt
`midiChannelMapBudget`, whenever `OverlayState.onChannelMapLayoutChanged`
reports new agents or names. The same hook, which also fires when the open
digits change, feeds `FloatingOverlayPanel.setChannelMapPresentation` with
`DictationOverlayWindow.miniNaturalWidth`: mini is as wide as its content, its
130 pt floor of controls plus the counter as drawn, at the same header scale
as compact. Placement and the saved position still describe the 200 pt
`miniPlacementSize` strip, with mini drawn at its trailing edge.

Moving controls must not turn a whole row into one button or nest buttons
inside another button. A folded rail pill already selects that conversation,
including `0` for All, and leaves the list folded. It does not open a
microphone and it does not retarget a capture that is already running.

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
keeps both the surface and the form (`applyTakeStartPresentation`).

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

### Channel map opt-in

Settings → Creator → **Channel map in the overlay** controls the map's mini
counter and compact chips together. It is off for an absent preference.
`OverlayChannelMapPreference` owns this process-only chrome choice in the app
preference domain, as `AppearancePreference` does; it is not core configuration.
Creator and every overlay observe that same owner. The view's change handler
asks its existing window owner to refit, including while a take is open.
When disabled, mini draws `cs` at its earlier 200 pt width (164 while an agent
records), and compact's natural width is 410. The microphone, selected owner,
transcript and explicit resize choice are retained.

### Intercom layouts and independent panes

Creator's channel-map opt-in also offers **Intercom layout**. Horizontal keeps
Monika's strip and gives names and separate microphone/playback/focus controls
priority over the waveform at wide widths. Vertical is an independent desktop
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

The roster toggle belongs to its right column; the conversation toggle belongs
to its left canvas. A reveal always requests a side column, independent of width.
A narrow host grows to the split floor where the display permits it, with both
panes retained during the morph. An ordinary saved width is clamped to leave
conversation space. Only explicit conversation folding or an explicit divider
drag chooses the roster-only layout; its left reveal edge remains reachable.
Hiding the roster chooses the rail, and hovering that rail cannot undo the fold.

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

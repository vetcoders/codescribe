# Overlay presentation handoff

The Founder supplied channel-bar, capsule and channel-popover designs on
2026-10-20 and asked for a visual layer the Founder can reshape without rebuilding
capture or bus handling. Those images are visual directions, with several
alternative arrangements; this document does not select one as the final UI.

## Start with the visual owner

| Change                                                                  | Existing owner                                                                          |
| ----------------------------------------------------------------------- | --------------------------------------------------------------------------------------- |
| Agent name, clock, descriptor and message preview                       | `OverlayAgentRosterLabel.swift`                                                         |
| Row footer, action grouping, broadcast row and compact channel pills    | `OverlayChannelStatusView.swift`                                                        |
| Numbered channel pill: rail pills, map chips, counter, open-channel dot | `OverlayChannelPill.swift`                                                              |
| Channel map in the bar: rows, name widths, counter, its click           | `OverlayChannelMapModel.swift`, `OverlayChannelMapView.swift`                           |
| Hover card of an agent's chip in the bar                                | `OverlayChannelChipHoverCard.swift`                                                     |
| Header, bar rows (`OverlayBarLayout`), embedded list/rail and hover     | `DictationOverlayView.swift`                                                            |
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

An agent row gives its full first line to identity, receipt and clock. The
waveform microphone, speaker and More menu remain visible over the top trailing
corner of the provider/model/project descriptor and routed two-line preview
below it. The preview keeps the full text layout width underneath the controls,
like the conversation's message-header actions. The delivery/reader state line
remains below the preview; panel and Sync controls keep their separate targets
on the identity row.
The conversation button and these controls remain siblings; opening a
conversation never opens its microphone. Long names retain their full tooltip
and accessibility label.

Hovering the row's navigation area or a folded-rail entry for 350 ms opens
a compact bounded carousel (up to five recent loaded messages, starting at the
latest) in the shared `OverlayHintPanel`. Original
paragraphs, rich formatting and attachments use `OverlayConversationMessageContent`.
The card is narrow (`260 × textScale`), fits short messages to their content
height, and scrolls only when a message exceeds its `180 × textScale` ceiling;
pagination controls appear only when more than one message is loaded. The
card's header names an agent as its rows do, the digit, the presence dot
(`presenceDotKind`, `OverlayAgentStatusDot`) and the name (Tola IV.10a,
2026-11-07; `OverlayRosterMessagePreview.headerDot`); channel 0, the inbox
and a saved conversation without a live roster row keep the plain
`N · name` title. The open conversation's own header is unchanged. An already
selected conversation has no duplicate hover preview in the roster (the
bar's `(0)` chip is the exception, see the channel map below). Clicking an unselected
preview's header or body uses that row's existing conversation-navigation
command; hovering or stepping the carousel never selects it. Its pointer target
stays open while the cursor crosses the gap or rests on the preview, then
dismisses after leaving both targets. The window cannot become key. It mounts no
composer, changes no recipient, acknowledges no
message, and fetches no saved history. Audio and menu controls keep their own
action hints. Empty entries retain their existing navigation hint without
opening an empty preview card.

The roster scrolls without a scroll clip, so rows passing above its viewport
stay mounted under the window header, and AppKit's `visibleRect` still counts
them. A cursor region takes the pointer only inside the frame of its enclosing
scroll view (`OverlayMenuCursorRegionView`), so a row outside the viewport
takes no hover and opens no card; before, a hidden row under ✦ answered the
pointer there and its card fought the agents list over the shared panel. A card closed because
another owner took the panel does not order itself again: its content revision
re-measures an open card and never opens one.

The more menu contains channel-zero membership, Do not disturb, the detected
client/terminal destination and removal into saved conversations. The same
commands are available from the row's context menu. Existing pending,
availability, frozen-owner and archive guards remain in force. An open
microphone can still be stopped after its reader disconnects, and playback can
still be muted. A client-focus request keeps its progress and result on the
more control and in the menu; removing the footer must not erase that feedback.

`cardAgentMenu` selects only these secondary actions for the compact More
control. The roster label's right-click uses the full `agentPillMenu`, restoring
Write/Forward, microphone, playback and Show in Agents with the existing command
guards (Founder correction, 2026-10-30). The conversation header label uses
the same full menu only after matching its current frozen owner and when the
presentation allows live actions. Historical owners retain reading and copying
without live commands. The header's More control keeps the compact subset.

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
One modifier owns that dim for the bar's pills, this list and the agents list
under ✦ (`OverlayDisconnectedRowDim`): 65 % opacity and half the colour (L5
variant 3, 2026-11-06; until then 45 % and fully grey, which beside live
agents read as the resting state and made the live ones look bold).

The compact receipt mark describes the newest completed user delivery to the
exact frozen recipient. Delivered and read use the same compact double-check
shape in the label ink, read like text (L8, 2026-11-06; the muted ink of
"delivered" measured below 4:1 on the light sheets): the thin stroke means
delivered, the heavier stroke means read or the canonical agent
acknowledgment.

Readability rule for the bar and the roster (L8, 2026-11-06): text to read
wears the label ink (`primaryText`), secondary text included — timestamps,
descriptors, previews, row states, hints, the bar's timer and the take's
detail lines; hierarchy comes from size and weight. An unread
conversation's preview is semibold until the conversation is opened (round 5
decision (b), 2026-11-07; `OverlayAgentRosterLabel.previewWeight`), from the
same unread count as its badge, so read and unread previews differ though
they share the ink. `ChannelRosterStyle`
(`palette.roster`) owns the roster's text and receipt inks. Icons at rest
(the audio controls, search, sort, chevrons, the crossed antenna, the waiting
spinner) keep the system's secondary label, which measures 3.6–3.9:1 on the
light sheets and meets the 3:1 non-text floor. The hover card's muted channel
0 line and Do not disturb stay muted: there the tone is the state.
Error and warning messages follow the same rule (round 5 decision (a),
2026-11-07): their words wear the label ink and only the icon before them
carries the meaning's colour (`OverlayStatusNotice`): an error's
`exclamationmark.circle.fill` on the error red, a warning's
`exclamationmark.triangle.fill` on the orange, each with a dark "!"
(`markInk`) as macOS draws its warnings, since the red and the orange alone
stay under the 3:1 icon floor on the light sheets (red 2.4:1 on
travertine); the dark mark on its fill clears it in both looks. The coverage
pill's microphone (`OverlayCoverageStatus`, a quiet input) keeps the warning's
orange hue with only its brightness lowered (`microphoneWarning`) until it
reads at 3:1 on every light sheet shade; the dark sheets take the orange
as it is. A row's pending state ("connecting…") reads in the ink beside its
spinner. Queue admission and provider acceptance are not delivered/read evidence.
Skipped and unreceipted deliveries keep distinct marks and explanations; a
message still held for withdrawal says "Held" under the preview and wears no
mark beside the name, since the hold is the sender's and no receipt exists yet
(Founder, 2026-11-04: the pause glyph that appeared after sending and turned
into checkmarks read as an intruder). The
mark sits near the identity, separately from the last-preview clock; its help
and accessibility text name the time of its own outgoing message. A later
incoming reply must not lend its time to that receipt. The circular unread
reply count is a separate projection (`OverlayState.unreadReplies`), also
shown on the folded rail; a reply heard to the end, its playback `spoken`
(the bubble's "Spoken"), counts as read and leaves the count (L14b,
2026-11-07), while an interrupted, failed or refused one stays unread; compact
map and floating Intercom badge rendering retain their existing form. On the
folded rail the count is its own small capsule outside the pill, at the
pill's top-right shoulder, so neither the pill nor its channel digit is
covered: white with a hairline and dark figures in light, grey with light
figures in dark, wider with each digit (Founder's mock-up, 2026-11-06).
A count above 99 reads `99+` in the two digits' width; the rail does not
grow for it.
The roster's positive unread count keeps its fixed badge instead of disappearing
when space is tight. It has layout priority over the name and clock. The read
mark consists of two identical translated checks; it is not two different states.

## Horizontal conversation and text destination

The horizontal window keeps the conversation header and its current-agent
actions visible with either the full sidebar or its folded rail. The sidebar
choice does not replace the current conversation, draft or microphone target.
Folded-rail controls sit on one centerline, with equal horizontal gutters that
keep the numbered buttons clear of the native resize band; beside them the
column keeps room for a two-digit unread capsule, so the centerline is that of
the pills and their capsules together (`OverlayAgentDrawerLayout.railWidth`).
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
the same indigo as a speaker that is playing a reply (`agentAccent`). The
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

User-message bubbles keep a quiet outline in `CSColor.secondaryInk` at
`CSAlpha.selectionStroke` strength. Incoming bubbles use the palette's
`border`, the system separator (`CSPalette.panelEdge`).

The channel map in the expanded bar (`midi`) is another surface
of the same projections. `OverlayChannelStatusView.channelMapModel` builds
`OverlayChannelMapModel` from the roster's rows, `hudStates`, current
conversations, the selection and unread counts. It lists the bound agents
1–9, preserving each name's entered spelling from the roster without changing
capitalisation (Founder, 2026-11-05), led by channel 0's chip, first in
the single row and first in the second row of a two-row bar, at rest as well
as while the microphone broadcasts on it (Founder, 2026-11-04), when the
violet open-channel dot lights it. Channel 0 carries its display name in the
agents' own layout: under the 0 in the single row, beside it in the second
row, cut by the same name rules and never widening the row past them
(`OverlayChannelMapNameWidths`). The name is the shared channel's
(`SHARED_CHANNEL_NAME` in settings.json → `Config.shared_channel_name` →
`CsSettings.sharedChannelName` → `OverlayState.sharedChannelName`, re-read at
launch and after every Settings write); with none stored it is the
interface's default, "Shared" ("Wspólny"), whose short form ("Wsp.") a cut
width shows instead of an ellipsis (`OverlaySharedChannelName`). Settings ›
Hotkeys › Fn and Agent channels edits it, one line of at most 24 characters
(grapheme clusters; `SettingsViewModel.sharedChannelNameLimit`, and
`shared_channel_name_line` in the loader cuts a hand-edited settings.json
the same way), with **Restore default**; the bar does not. The same name heads channel 0's conversation (`0 · <name>`),
its row in the Bus window (`0 · <name> (n of
m)`), the chip's hint and its message card; action wording such as the
bubble route `You → All` stays. Only the name is personal: the digit, the
chord and the routing of channel 0 do not change. It is not an agent: the counter, `OverlayChannelMapModel.count`
and the 1–5 / 6–9 row split count agents only. With no agent bound and no
broadcast the map is empty. A chip calls `selectChannelConversation`, the
rail pill's selection path; `(0)` selects the All conversation the same way
and never opens a microphone.

The floating Intercom also preserves the supplied spelling of agent names.
Changing a display name's case never changes its conversation/provider/session/
lease identity. Provider labels retain their separate human-readable formatting.

Hovering `(0)` opens channel 0's message carousel: the same
`OverlayRosterMessageHint` (`OverlayRosterPreviewSession`, the last five
messages, withdrawn ones hidden) over the same All conversation, with the
same open action, as the Bus window's All row
(`OverlayChannelStatusView.channelZeroMessageHint`, handed to the map as
`channelZeroHint`). The map keeps no messages of its own. Unlike a roster
row, the chip's card shows while All is the selected conversation: the bar
shows no conversation to duplicate, and a click on `(0)` selects All, so the
roster's rule hid the card after one click (Monika, 2026-11-07, Z2).

Hovering an agent's chip opens its card (`OverlayChannelChipHoverCard`), not
the old `5 · Kuba · Ctrl+5`, which only repeated the button (Founder,
2026-11-04). The card is a tight, non-interactive panel in the header's
hint window (`OverlayHeaderControlChrome` with `hintContent`, sized from its
measured lines, 200–240 pt) and states the agent's facts kept apart, each from
the projection the roster row reads (`channelChipStatus`, carried as
`OverlayChannelMapChip.status`):

1. **Presence** from the roster snapshot's `followerAlive`: `true` reads
   "Online" for the current sample, `false` only "Not active" (a capped
   discovery can report false for a live agent, so never a sure "Offline"),
   `nil` "No data". A stalled follower (`hasStalledFollower`) adds "Not
   reading". The dot before the name (`presenceDotKind`) shows this same
   presence; neither it nor the channel 0 policy carries a delivery stage.
   The card's line wears the same dot as the agents list and the Bus
   window's roster (`OverlayAgentStatusDot`, one owner of its colour; L2,
   2026-11-06): olive online, violet and blinking while the agent's reply
   plays aloud (still violet under Reduce Motion), a muted ring for a gone
   follower and a muted "?" without data. The speaking fact reaches the
   card's status (`OverlayChannelChipStatus.speaking`) from the roster's
   `speakingDigits`, the same set that gives the chip its speaking dot. A
   stall is the same dot's `stalled` kind, olive with its bold "!", beside
   "Not reading" (L6, 2026-11-06): a stall is not sleep, and "zzz" is the
   moon's alone. **Rule (Founder, 2026-11-06):** "zzz" means only an agent
   asleep under the moon (Do not disturb). A stall is something else, a live
   agent without the moon that has not acknowledged reading for 10 minutes,
   and everywhere it looks like the presence dot: green with "!". The
   card's "Not reading" text stays. `OverlayChannelStatusView.statusSymbol(for:)` names no
   stall glyph either; the Intercom's status mark draws the presence dot for
   a stalled reader and an explicit glyph only for a gone one. The "!"
   comes at the horizon itself, not with the next change on the bus:
   `isStalled(now:)` reads the clock when a view draws and an unchanged
   roster poll draws nothing, so `OverlayState` plans one wake for the
   oldest backlog of a live follower plus the horizon
   (`nextFollowerStall`, `followerStallDeadline`). The wake announces the
   same `channelHudStates` again, so the bar, the list under ✦, the roster
   and the Bus window draw the "!", and plans the next follower's wake; no
   poll, and an unchanged roster keeps the wake it has (Tola IV.6,
   2026-11-07).
2. **Channel 0** from the policy as `OverlayBroadcastPolicyLedger` paints it,
   two states like the roster's antenna and the context menu's checkmark,
   with no unconfirmed third (Founder, 2026-11-04). The line is channel 0's
   display name (`OverlaySharedChannelName`, carried as
   `OverlayChannelChipStatus.channelZeroName`) in the nominative, never
   declined and without "in channel" wording (L3, 2026-11-06): behind the
   antenna in the ink (`primary`, not the agent violet) while the agent
   hears it, behind the crossed antenna in the muted tone while it does not.
   The moon takes the symbol's place and adds "Do not disturb", muted; off
   channel 0 under the moon the crossed antenna stays and the moon stands
   beside it, so the line shows what VoiceOver reads (Founder, 2026-11-06).
   VoiceOver, which hears no antenna, reads "Hears: Shared (channel 0)" or
   "Does not hear: Shared (channel 0)" (PL "Słyszy: Wspólny (kanał 0)"),
   the name after a colon so it is never declined.
3. **The newest message** (L16, 2026-11-06) from the conversation's
   visible messages: an arrow for its direction (`arrow.up.right` yours,
   `arrow.down.left` the agent's), its time and its fate in the roster's
   own words: for yours the receipt line's reading
   (`deliveryReceiptDisplay`: Read, Delivered, Held, …), for a reply
   "Spoken" once its playback finished. No line without a message.
4. **Audio** (L16): "Microphone open" while the HUD feeds the channel,
   "Voice muted" while the roster's speaker mutes the agent's replies
   (`mutedChannels`); one line when either is so, else none.
5. **Unread** from `unreadCounts`: "N unread", or no line.

**Since when** (L16) rides the presence line: a stalled reader says
"Not reading for N min" (`stalledMinutes`, from the roster's
`unread_since_unix_ms`), a gone agent "last heard 20:36", the time of its
newest reply. The roster snapshot carries no time of a presence change, so
"Not active for 3 h" is not drawn; it would need a field such as a
follower-state-since timestamp on `CsChannelRosterState`.

The first line names the digit, the whole name, the provider and the project
the agent works in ("Agy · Codescribe", L16), the project from the same
verified lease as the roster's descriptor (`AgentPlaybackBinding.repositoryName`,
`OverlayState.channelAgentProjects`), either alone when the other is unknown;
the chord
(`Ctrl+5`) is the last, small line: at most seven short lines. Nothing on
the card is ever cut (Monika, 2026-11-07): the card is as wide as its widest
line drawn whole, between 200 and 320 pt, measured in the fonts it draws
them in (`OverlayChannelChipHoverCard.size(lines:)`), and a line still wider
than 320 pt wraps to the next line instead of ending in an ellipsis; the
height counts every wrapped line, since the hint panel draws the card at
exactly that size. The provider and project stay whole beside the name when
the title fits within 320 pt, else they stand under the name. The panel
stays centred under the chip and inside the screen, so a wider card grows
to both sides. The card's fonts do not follow the bar's text scale, so the
ceiling does not either. The card is one accessibility element whose label
reads every line, and the chip's accessibility value carries the same
facts. The card stays non-interactive. Mini
carries no map and no counter (Founder, 2026-11-04): the interactive list of
agents under the Agents button (✦), described below, replaces the counter
that stood there since 2026-10-22. The vertical layout's stacked counter
stays in the bar and the window (`OverlayChannelMapView.Placement.counter`);
its click calls `OverlayState.activateChannelMapCounter`, which brings the
Intercom group forward. While the map is enabled, a
channel the controller opens in mini or the bar keeps that form
(`OverlayState.channelOpenExpandsSurface`): the selection follows the open
channel, and the Agents canvas unfolds for it only with the map off or when
the overlay is already expanded.

In mini, which has no chips, the Agents button itself says an agent is
speaking (L15, 2026-11-06): while any agent's reply plays aloud
(`OverlayChannelStatusView.speakingDigits`, the source of the chips' and the
list's speaking dot) the star wears the speaking violet (`agentAccent`) and
blinks gently on the speaking dot's beat, full and 60 %
(`speakingStarOpacity`); Reduce Motion and a hidden panel hold it steady, in
the colour alone. An open agent capture keeps its own glyph. The expanded
bar's button does not change: its chips carry the dot
(`signalsSpeaking`, set only by mini).

Hovering the Agents button (✦), in mini and in the expanded bar, opens a
small list of the mailboxes and the bound agents after a short pause
(`OverlayAgentsHoverListChrome`, `OverlayChannelMapHoverView.swift`). It
replaces the button's "Agents" hint; while agent capture is open, or with no
row to show (no current mailbox conversation and no agent bound), the button
keeps its one-line hint, because a click then stops the capture or opens the
Agents canvas. A click on ✦ keeps its action
(`OverlayState.toggleAgentSidebar`). The list rides the shared interactive
hint panel (`OverlayHeaderControlChrome`, as the roster's message preview):
it is a child window of the overlay, stays while the pointer crosses from ✦
onto it, and closes 220 ms after the pointer has left both; Esc closes it.
While the pointer rests on ✦ the list is the panel's only owner: roster rows
scrolled under the header take no hover (see the row preview above).
The panel takes its parent's window level (`OverlayHintPanel.synchronizeLevel`)
and follows it when `OverlayPresence` moves the overlay between rest, normal
and capture levels; it is ordered above its parent and raised to the front
of that level only when it is presented, not on a data refresh. A list left
at the normal level hung under the foreground application below a pinned
bar: the pointer never entered it, so it closed after the grace, and clicks
reached the application beneath.
While the pointer is over the list it holds mini's auto-hide and the
auto-collapse through `OverlayState.setPointerHovering`. The content reads a
live model, so a roster or policy refresh repaints rows without re-showing,
raising or re-timing the panel. No hint opens from inside the list: the
shared hint panel is the list itself.

One quick-access row leads the list (Founder, 2026-11-04): your inbox (10)
as the envelope with its name, a chip that opens that conversation through
`onSelectConversation` without touching the microphone, with the unread dot
in the text tone and nothing else (no presence dot, no antenna or moon, and
no width kept for them: its unread dot stands at the right edge one gap
after the chip, in the same column as the agents' unread dots). Channel 0
has no row in any mode (Founder, 2026-11-07: channel 0 lives on the bar; the
list keeps only the inbox); mini has no shortcut to it.
An inbox without a current conversation has no row, as it has no rail pill
(`hoverListRows`). Under it one low row per agent 1–9 in ascending order
(`agentsHoverRows`), no header or cards, sized to its rows so the inbox and
nine agents fit without scrolling. The width is the widest row's own need
(`OverlayAgentsHoverList.rowWidth`; Monika, 2026-11-07): an agent row is its
chip, the antenna, the moon and the unread dot, the inbox row its chip and
the unread dot, never less than 140 pt. A long inbox name above short agent
names no longer widens the list by the marks it does not draw. Columns: the
numbered `OverlayChannelPill` chip carrying the presence dot
(`presenceDotKind`) between the digit and the agent's name (`nr · dot · name`, as the Bus window's roster rows; Founder, 2026-11-04), the antenna
(channel-0 membership), the moon (do not disturb) and the unread dot in the
text tone. The presence dot is the one status dot before an agent's name
everywhere (this list, the Bus window's roster, the Intercom, a fiszka) and
shows the agent alone (Founder, 2026-11-04): filled system green while its
follower is alive (the one green of the app since 2026-11-05), the same
green with a bold "!" in the panel's background tone
while a live reader has not read past the stall horizon (no fourth colour,
no zzz, no greyed row; the card says "Not reading"), the agent violet with
the microphone open to it (your voice to agents is the system indigo, as
the waveform and the chip's circle; terracotta stays with dictation outside
the bus), the same violet blinking on the listening caret's 0.6 s beat while
the agent's reply plays aloud (`OverlayAgentDotKind.speaking`, a reply whose
playback is `playing`; still with Reduce Motion; L1, 2026-11-06) and back to
its presence when the playback ends,
"zzz" while a live agent sleeps under the moon as the bus paints it (the
mailbox keeps every message for when it wakes; the moon comes before the
stall, since a sleeper acknowledges nothing), an empty ring for a gone
follower (a new direct message to it is refused, not queued), and a "?" in a
circle without a liveness claim. The dot never
reads the stage of the last sealed utterance: the former dot kept a channel
silent for hours green on an old receipt and turned a replying agent amber
on a newer one. The fate of a message stays with the message in the window
and on the chip's hover card; the row's VoiceOver status reads the card's
presence facts. New replies, wherever a dot or badge marks them (this list,
the chips, the roster badge, the card), wear the text tone: not the amber
that means warning and waiting, not the violet that means your voice. The
dot on the Agents button itself (`attentionUnread`) says a message waits on
channel 0 or in your inbox (10), the mailboxes mini has no other sign for;
an agent's own unread replies stay on its chip and row. On a bar chip (and
on a stacked pill's circle) the unread dot is a 5 pt badge set on the
outline, its centre on the rounded edge at 45°, with a 1.5 pt knockout in
the bar's sheet colour between it and the outline (`OverlayChannelUnreadMark`,
L14, 2026-11-06); the folded rail keeps its count capsule. The chip opens the channel's conversation
through `onSelectConversation`, the path `selectChannelConversation` uses.
The antenna and the moon are sibling buttons with their own hit targets;
they change reception through `OverlayBroadcastPolicyLedger.change`, never
open the conversation and never close the list, are dimmed while a change is
pending and repaint from the read-back receipt. Each mark has exactly two
looks, lit or not, the same state as the context menu's "Channel 0" and "Do
not disturb" checkmarks (Founder, 2026-11-04). The lit antenna wears the
regular ink, the crossed one the muted tone, as on the chip's hover card;
violet stays with a voice on the bus, never with reception (L7, 2026-11-06;
one owner, `OverlayReceptionMarks.antennaInk`, for this list, the Bus
window's roster marks and the fiszki). A policy the helper has not
receipted draws as painted, never as a third, greyed mark, and the receipt
only confirms this app's own click. The dots
are decoration for VoiceOver: each row carries one label with every state
(`5 · Kuba · Listening · in channel 0 · Do not disturb · 2 unread replies`;
a mailbox row `0 · All · 2 unread replies`), the marks their own;
identifiers are `overlay-agents-hover-row-<digit>`, `-open-`, `-antenna-`
and `-moon-<digit>`, with `0` and `10` for the mailboxes.

`OverlayChannelPill` is the single owner of the numbered pill visual: the rail
pill, the map's chips and its counter render the digit through it, from
immutable display values and one action. An open channel's circle and pill
wear the recording violet and nothing else (spec §14; never `MIC →` text; the
microphone and megaphone signs belong to the Agents header button; mini's
take adds no sign for channel 0). The pulsing open-channel dot on the
circle's rim went on 2026-11-04 (Founder, decision 3: the violet circle and
the violet waveform say it). The selected conversation's fill is neutral, in
the text tone, on the pill and on the roster row's leading bar alike (decision
4): in the bus violet means your voice and terracotta the microphone, so the
bus is the one surface where "choice = terracotta" does not apply. The bar's
chips carry no presence dot; the agents list's chip does, between the digit
and the name, except while the agent's reply plays aloud: then the chip, in
the single row and the second row alike, shows the blinking violet speaking
dot just before its circled digit, inside the pill's own margin, so the bar's
measured width never changes (`OverlayChannelMapChip.speaking`). A gone
agent keeps its name on the bar and is only dimmed (`OverlayDisconnectedRowDim`),
with no ring and no other dot (review of 2026-11-06, point 7): the dim reads as
inactive and the hover card gives the details; the agents list and the Bus
window keep the empty `.sleeping` ring, where every row carries a dot. It
receives no `OverlayState`, conversation or command.

The bar with the horizontal map is as wide as its content (bus brief
2026-11-04 §4, §5, §8, §11C). `OverlayBarLayout` sets the brand, a waveform of
one fixed width (110 pt, the same at rest and while recording, so a take never
changes the bar's length), the clock, the map and the tools side by side;
nothing stretches between them. In the two-row bar the first row ends after
the clock, so its slack before the tools goes to the waveform, up to 170 pt
(`waveformMaximumWidth`), never to a hole. With 1–5 agents the widths between
every name whole in two rows and one row of the shortest names are a hole
after the chips: `OverlayMidiBarGeometry.clampedWidth` snaps a dragged edge
across that band to the nearer form, and when the roster drops from 6–9 to
1–5 agents the width the user gave the two-row bar is forgotten, so the
single row settles at its natural width. Its two arrangements:

- **One row, preferred for 1–5 agents.** The map follows the clock as
  `.stacked` pills: the circled digit with the name under it, one button with
  its caption included. The row is the taller of the 26 pt header row and a
  stacked pill: the circle, the open-channel dot's reach below it and the
  caption's line height, all at the header text scale.
- **Two rows, preferred for 6–9 agents.** The first row holds the brand, the
  waveform and clock, and the tools at its trailing edge; the second row holds
  every agent in digit order as `.chip` pills, the name beside the digit. No
  divider and no spacer: the rows are 26 pt and 22 pt with a 2 pt gap, and the
  bar is as wide as the wider row.

Measured width decides inside that preference. Names are measured in the
pill's own font (`OverlayChannelMapNameWidths`): whole up to six letters in
the default bar, otherwise five letters and `…`; whole when the user widens
the bar; four letters and `…` at the floor, so names of up to five letters
never lose a letter and `Lena` and `Lena…` still differ. When a row has less
room than its names, one common cap cuts the longest names first; the font
never shrinks, captions never overlap and circles never overlap. A single row
the user narrows below its shortest names becomes two rows. No tier drops a
name, shows only digits (an empty name is the exception), collapses the map
into a counter or `+N`, or hides a channel or a tool.

The window owns the geometry. `DictationOverlayWindow.midiBarGeometry`
measures the bar from the same values: 32 pt of side padding, the brand
(close dot, 5 pt, word), the waveform, the clock (`00:00` in the 11 pt mono
face), the map row and the 141 pt of tools, with 10 pt between items and 8 pt
between the clock and the map. It yields `OverlayMidiBarGeometry`: the natural
width (preferred names), the resize minimum (shortest names, in two rows when
that is narrower), the maximum (every name whole, so widening never opens a
hole), and the height for a width. `FloatingOverlayPanel.setMidiBarGeometry`
receives it whenever `OverlayState.onChannelMapLayoutChanged` reports new
agents, names or open digits, and when the text scale changes. The height is
rounded once there; the header reads its row count from the height the window
gives it, so the layout and the frame never disagree. `applyPlacement` and
the anchors use `FloatingOverlayPanel.midiSize`, and the bar keeps its
upper-right corner when its size settles, so a second row grows downward.

Resizing the bar moves only its left or right edge, through 8 pt bands (the
close dot's hit circle starts 8.5 pt from the left edge). The edge stops at
the measured minimum and maximum, and the height follows the rows the width
holds. Only a width the edge really moved is remembered
(`noteUserMidiWidth`): a press that does not move the frame, a push against a
limit included, keeps the remembered width, and an agent leaving for a moment
narrows the bar without overwriting it. The panel never carries AppKit's
`.resizable` style: on a borderless panel its native resize corner swallowed
the close dot's first points, so `FloatingOverlayPanel.allowsUserResize`
gates every edge band instead. Without the horizontal map, or with no agent
and no broadcast, the bar keeps its classic 410 × 46 floor with a flexible
waveform.

While a take records (bus brief §6, §11D) the map stays as it is: every
bound agent keeps its digit and name, and the addressees, the channels the
microphone feeds (`OverlayChannelMapChip.microphoneOpen`), wear the recording
violet on their circle with the pulsing open-channel dot; the selected
conversation keeps its fill and the unread dot stays independent of
both. The waveform and the clock are drawn at rest too, at their fixed
widths, so the bar has no hole at rest and a take alone never changes its
length. Channel 0's circle is on the map at rest too, so a broadcast only
lights it and never changes the bar's width. The
header microphone is the stop. No recording state reduces the map
to one digit without a name or leaves an empty gap (anti-pattern 09).

The take wears its mode's colour (decision D6, 2026-11-06), the same rule in
both looks and in mini and the bar: the brand terracotta for a dictation, the
agent indigo (`CSColor.modeAgent`) for an agent take, whether a channel's
microphone or the assistive take feeds it (`OverlayState.liveTakeMode`). It is
on the brand dot (`OverlayCloseButton.takeTint`), the waveform, the active
recording control and the open channels' pills; at rest the dot and the
microphone are muted ink. Red is for errors only and orange for transcribing
(the spinning arc); the silence yellow stays the overlay's own state. The
recording control is the microphone before a take and the Stop square during
any take (`OverlayRecordingControls.recordingSymbol`): for a dictation it
relays `.finish`; while only agent channels record it is their Stop and closes
every open channel through `OverlayState.stopOpenChannels`, the chord's own
toggle, never starting a dictation. The Agents button keeps its live-capture
sign and its stop meaning in mini.

### Mini

No form of the bar (mini, folded, expanded, two-row) draws a processing
caption: the orange "Finishing…" under the header or at the folded bar's
bottom is gone (L13, 2026-11-07), with no replacement. The take's phase
still reaches assistive technology through the intent dock's accessibility
value (`OverlayState.statusText`), which never was the caption's source.

Mini at rest (Founder, 2026-11-04, bus brief §3) reads, from the left: the
brand dot (the close button), transcription (`transcriptionPreviewButton`),
the microphone, the Agents button (✦) and the preview chevron, in the bar's
glyphs, palette and 22 pt controls. No counter, waveform, timer, map or pin:
the pin is the expanded bar's and the window's. The transcription control is
the system `text.quote` symbol in every form (`OverlayTranscriptionGlyph`,
`CSIcon.transcription`, decision D7 and L10, 2026-11-06: `note.text` read as a
calendar at 11 pt); while the window shows the transcription it is
chosen like the pin, primary ink on the neutral wash
(`OverlayHeaderAccent.state`), never terracotta. With
the channel map disabled the `codescribe` word follows the dot, as before;
with it enabled the dot stands alone. Mini is as wide and as tall as its
content, nothing reserved for hidden elements: 7 pt from a control to the
capsule's edge on every side (`DictationOverlayWindow.miniInset`), 4 pt
between controls (`miniControlSpacing`) and two 2 pt row gaps around the
closed centre (`miniRowSpacing`), so 14 + 9 + 4 + 100 = 127 pt wide
(`miniFloorWidth`) and 22 + 14 = 36 pt tall (`miniHeight`); the word adds
6 pt and its measured width (`miniBrandWidth`). Mini does not scale with the
text size. The same hook feeds `FloatingOverlayPanel.setChannelMapPresentation`
with the map switch only. Placement and the saved position still describe the
200 × 46 pt `miniPlacementSize` strip, with mini drawn at its upper-right
corner (`applyPlacement`, `originForPersistence`).

While a take is live (`OverlayState.miniTakeLive`: a dictation from its start
to the controller's stop, or any channel, channel 0 included, with its
microphone open), mini grows slightly and stays mini (Founder, 2026-11-04,
bus brief §6, §11D, decision 12.2). Between the brand and the transcription
button it shows its own 82 pt waveform (`miniWaveformWidth`), the clock (`00:00`, the same
counter as the bar) and the addressees: one circled digit per open channel,
in digit order, drawn by `OverlayChannelPill` in the counter form's
open-channel look (15 pt circle, recording violet, the open-channel dot),
with no names and no count or `+N`. A broadcast on channel 0 adds no
addressee: the Agents button already wears its megaphone, and a second one
beside the clock repeated the sign. A dictation alone shows no addressees.
A circle selects that channel's conversation, as a bar chip does. The row's
gaps open from 2 pt to the
4 pt its controls keep, so mini grows by the waveform, the clock, the
addressees and those gaps (`DictationOverlayWindow.miniTakeWidth`), never by
more. The upper-right corner stays: mini grows leftward past the strip, and
placement draws the measured width instead of clipping to 200 pt. Nothing
remembers the wider width, so when the take ends mini returns to its resting
127 pt (or brand-word) width. The change rides
`OverlayState.onChannelMapLayoutChanged`, which also fires when the
`recording` flag or the open digits change, into
`FloatingOverlayPanel.setMiniTake(width:)`. On a visible panel it glides at
the morph speed with one layout held at the upper-right corner (growing, the
take's row; folding, the departing row), so the tools never move and are
never clipped; with Reduce Motion it is one step. The Agents button keeps
its stop meaning during an agent take, and the background click still
toggles mini and the bar.

A still click on mini's free background opens the bar; a still click on the
bar's free background (its `overlay-header*` drag regions) folds it back to
mini (`OverlayState.advancePresentationFromHeader`, decision 12.1). From the
window a header click returns to the bar. Controls, an agent's circle, the
8 pt resize bands and a drag past the 3 pt threshold never switch the form;
the window opens from the chevron, ✦ and the menus. The chevron's context
menu picks the form directly: **Mini / Bar / Window**.

Folding the bar to mini follows the native bounds: a bar-to-bar morph holds
no layout (`barMorph` in `FloatingOverlayPanel.setPresentationMode`), because
pinning the bar's size would clip the leading-aligned mini row from the left.
Mini lands at the right edge of the bar it left (`miniFrame.maxX`).

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
- Roster follower liveness is derived from the bound or open owner's specific
  validated lease. A fresh active heartbeat means alive; a verified inactive,
  expired or absent lease means not alive. Unreadable, invalid or mismatched
  evidence makes no liveness claim. An owner omitted from a bounded lease
  discovery is not thereby disconnected. Session identity keeps its original
  bytes, including whitespace. Capture-open remains independent.
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

### Covered conversation

With the list covering the window (`agentsLayout == .agents`) the
conversation cell keeps its split width under opacity 0 and a clip, so the
slide back is a reveal, not a reflow. The glass navigation header does not
honour that opacity or clip: it stayed on screen over the list's first rows
at the split width (Founder, 2026-11-05). `OverlayConversationView` therefore
takes the header and the standard composer off the tree while `isCovered`,
keeping their measured heights as the content margin and the bottom inset,
and mounts them again when the conversation returns.

## Independent canvas sizes

The Founder's 2026-10-30 recording makes the full Agents canvas and floating
Intercom mutually exclusive. `OverlayController.syncIntercomPresentation`
decides native visibility from the existing preference, canvas and presentation
mode. Opening the full Agents canvas hides grouped and detached windows without
closing their durable instances. Entering Intercom hides the default Agents
widget; a separate transcription canvas retains its capture presentation.
Drafts, recipients and microphone state do not change with this visibility.

Automatic collapse set to Never also excludes folding when another window of
this application becomes key. An explicit Bar/Mini choice remains available.
Window presence and layering keep their existing owner; Never is not a new pin policy.

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

The header's secondary-click menu (`OverlayHeaderMenu`, every header form)
offers placement as native menu toggles, so the menu draws the checkmark (L9,
2026-11-06; a checkmark drawn as a `Label` image is not the item's state and
macOS 26 menus showed none). Under **Anchor** the mode comes first and the
place under it (L17, 2026-11-06): the first level holds **Free motion** and
**Fixed position**, exactly one of them checked (`OverlayState.freeMotion`),
then the **Place** submenu with the six places, the remembered one checked
also while Free motion is on (`OverlayState.placementRemembered`). Choosing
a place, the remembered one included, switches to Fixed position and moves
the panel there (`selectPlacementAnchor`). Choosing **Fixed position**
returns the panel to the remembered place, and so does turning Free motion
off (`OverlayState.setFreeMotion(false)` →
`selectPlacementAnchor(placementAnchor)`, which fires `onPlacementChanged`);
turning it on keeps the panel where it is dragged (`selectFreeMotion`). A
native menu item that opens a submenu carries neither a checkmark nor an
action, so Fixed position and Place are two items, not one.
Dragging and the saved origin (`OverlayPlacement`) are unchanged. The auto-fold
submenu uses the same native toggles.

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

Settings → Creator → **Channel map in the overlay** controls the bar's pills
and whether mini shows the brand word. It is off for an absent preference.
`OverlayChannelMapPreference` owns this process-only chrome choice in the app
preference domain, as `AppearancePreference` does; it is not core configuration.
Creator and every overlay observe that same owner. The view's change handler
asks its existing window owner to refit, including while a take is open.
When disabled, mini draws the `codescribe` word beside its dot, and the bar
keeps its classic 410 × 46 size. The microphone, selected owner,
transcript and explicit resize choice are retained.

### Intercom layouts and independent panes

Creator's channel-map opt-in also offers **Intercom layout**. Horizontal keeps
Founder's strip and gives names and separate microphone/playback/focus controls
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
`OverlayAgentStatusDot` from `presenceDotKind`, before the figure; after the
name sit the agent's two reception marks (`OverlayReceptionMarks`): the
channel-0 antenna (`broadcastPolicy.statusSymbol`, lit for a member in the
regular ink, `OverlayReceptionMarks.antennaInk`) and the
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

An open conversation's title bar is one row (Maciej, 2026-11-03: "jest za
gruby … żeby on był jednorzędowy"), on a detached fiszka and on an expanded
group card alike. The agent descriptor (provider, model, declared repository)
stays visible: it joins the name line after the name and truncates in the
middle, so the provider leads and the repository ends the line. No second
metadata strip sits on the plate, and no metadata sits on an opaque fill. Only a
resting group card's context tier keeps its full-width metadata line. Moving the
metadata under the composer is a later redesign, not this one.

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

### All and inbox rows

The full Agents column lists `0 · All` and your inbox as its first two rows,
in the rail's order, then channels 1–9 and the saved conversations: one list,
scrolled together (Founder, 2026-11-04: one sidebar, no separate card on top).
Until then the two sat on a pinned card over the list, on the composer's
surface (Maciej, 2026-10-30); the card and its surface are gone, and the
monitor's pinned slot stays empty. Each mailbox row wears the agents' glass,
padding, selection bar, hover and gesture focus ring. All keeps its
three-line preview, recipient counts, microphone/megaphone and playback
controls, drawn as the agents' glass circles (`rosterDiameter`), not the
flat glyphs the card had; there is one All row and one projection of its
state. The inbox
row shows the envelope where an agent's digit and presence dot stand (it
has neither), its name, the last message's preview, the unread badge and the
clock (`inboxRow`, identifier `overlay-inbox-row`). Keyboard focus and
scroll-to-selected reach both rows like any other.

### One fold button

The list has no header row: no "Agents" title (Maciej, 2026-11-01) and no
row for the fold. One `sidebar.right` (`overlay-agent-sidebar-fold`, "Hide
agents list") stands in the pane's top-right corner, in the empty band
between the window header and the first row (Monika, 2026-11-07, from her
screenshot of the installed build). The band is the column's 8 pt gap under
the header (`listColumnTopGap`), the roster's 8 pt inset and the scroll
content's 8 pt margin: `listTopBand`, 24 pt to the first row's frame, 27 pt
to its card, at every text scale, since none of the three grows with the
text. The button is a fixed 22 pt square there (`listFoldSide`; 22 pt times
1.5 or 2 would be taller than the band), centred in it (frame 1 to 23 pt
below the header) and flush with the rows' trailing content, where the
clocks and the "…" end (`listFoldTrailingInset`: the roster's 4 pt and a
row's 5 pt). It is an overlay of the list's body, not part of the scrolled
content: it does not scroll with channel 0's row and takes no width from
its title line. A row of its own pushed every row down by 30 pt and was
rejected (Monika, 2026-11-07: "to spychanie nie jest akceptowalne, na pewno
nie"); the rows stand where they stand without the button
(`testFoldButtonsLeaveTheRowsAndPillsWhereTheyStand`, at text scales 1, 1.5
and 2).

The button calls `foldAgentList`, which keeps the selection and drafts and
leaves the rail of pills. The rail shows the same button in the same top band
(`overlay-agent-rail-unfold`, "Show agents list"), calling `unfoldAgentList`
(Monika, 2026-11-07, on the render of the folded rail): the same 22 pt
`sidebar.right` at the same height (frame 1 to 23 pt below the header),
centred on the pills' column, which leaves the room beside it for the unread
capsule. The rail's own band above its first pill (the column's 8 pt gap, the
rail's 8 pt top and the capsule's 2 pt rise, scaled: 18 pt at text scale 1)
is shorter than the button, so the pills sit lower by `railFoldDrop`, 6 pt at
every text scale: the first pill's frame begins 1 pt under the button's at
scale 1 and the glyph's ink ends 6 pt above it. Their own spacing is
unchanged, and without the button (no `onUnfoldList`) they stand where they
stood. The button used to stand at the rail's foot, because the pills were
not to move; they may, and one button stands in one place. There is no second
button on the left and none on the conversation's edge (Monika, 2026-11-05:
"jeden po prawej stronie to był dokładnie taki układ, jaki chciałam").
The divider still folds and unfolds
the list; the button is the visible way, the divider the dragged one.

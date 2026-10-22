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

An agent row's primary action and its microphone, speaker, Frame and archive
actions are siblings. Identity, the clock, the descriptor and the preview take
the full row width; the footer owns only its compact action group. That budget
is five text lines: identity, descriptor, up to two preview lines, and footer
state. Preserve it for agent rows.

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
before it unfolds; an agent take retains its channel surface.

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
priority over the waveform at wide widths. Vertical shows independently movable,
nonactivating desktop capsules. Native capsule windows leave the gaps to the
desktop; only one hovered agent opens a transient message preview. Clicking a
capsule pins that small preview; clicking it again, or Close, dismisses it.
Hovering another agent does not replace a pinned preview. The preview's explicit
Open conversation action selects and reveals the existing conversation, while
the capsule's independent microphone changes capture.
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

# Glass contract: native materials, chrome and reading surfaces

**Status:** accepted product requirements; optical acceptance remains open where
listed below. This is the canonical glass contract for Codescribe. Its original
2026-11-08 path is retained so existing links keep resolving.

**Scope:** Agent and Settings windows, their shared sidebar and header,
onboarding's glass surfaces, and the glass/reading-surface boundary in mini,
MIDI and the expanded overlay/Bus. These surfaces belong to one coherent glass
family, with geometry appropriate to their function. Other glass consumers
inherit the ownership and evidence rules; this does not require every card,
onboarding screen or reading surface to become glass.

The Founder's explicit visual brief of 2026-11-07, the frozen-resize choice of
2026-11-08, and the 2026-11-09 prohibition on opaque seam/tone compensation
define the result. The construction described later is the current means of
reaching that result, not proof that it has been achieved. Changing the means
must preserve the contract and update the affected documentation.

## Decision — product requirements

### G1 — Keep live optical glass

Glass must visibly respond to its scene: colours, shapes and moving content
remain optically present through the native material. Visible letters behind
glass are not a defect by themselves. Judge the legibility and operation of
the foreground branding, title and controls, not whether the background has
been erased into a uniform milky patch.

Use the appropriate public system material and its real content relationship.
A stack of translucent rectangles, a static blur or a glass API call alone
does not establish the required optical result. Desktop sampling and the
treatment of content scrolling inside the window are separate checks; one
cannot stand in for the other.

Prefer standard AppKit/SwiftUI navigation, controls, materials and scroll-edge
behaviour when they satisfy the product requirements. Before adding a custom
toolbar/navigation background or composition, document the unmet requirement,
why the standard component is insufficient, and a controlled comparison of the
result. The continuous full-width header is a Codescribe product requirement;
the custom accessory below is the present construction, not an Apple mandate
or a reason to reject a standard component that meets the same goal. This
follows Apple's guidance to minimise custom backgrounds that can interfere
with system adaptation and scroll-edge effects:
[Adopting Liquid Glass](https://developer.apple.com/documentation/technologyoverviews/adopting-liquid-glass),
[Toolbars](https://developer.apple.com/design/human-interface-guidelines/toolbars).

### G2 — Compose one coherent window

Agent and Settings have one header band above the detail, with continuous glass
from the sidebar/detail boundary to the right window edge. Branding, view
identity and actions belong to that band. Do not split it into oval glass
islands, leave uncovered gaps between groups, add another title row, or repeat
the same identity to compensate for a layout defect.

Keep AppKit's window controls and native sidebar toggle, their hit targets,
keyboard behaviour and titlebar geometry. When the sidebar collapses, the
native-control region still needs a coherent visual join to the remaining
header. Excluding controls from a custom material's hit/geometry region is not
visual acceptance of a hole around them.

Sidebar, header and reading canvas must share a coherent temperature, depth
and character in Light and Dark, active and inactive states. They do not need
identical RGB values. Preserve natural system separation and optical rims;
neither a faint native edge nor the small coloured rim observed in other macOS
windows is automatically a bug to remove. Accidental seams, notches, uncovered
strips and hard panel boundaries need evidence about their actual owner.

### G3 — One owner per surface and role

| Surface / responsibility                  | Owner and boundary                                                                                                                                                                                                                                                    |
| ----------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Agent/Settings navigation material        | `NavigationSplitView`; `CSSidebarColumn` owns sidebar content/style, not a second material behind it.                                                                                                                                                                 |
| Shared Agent/Settings header              | `CSWindowTopHeader`; one passive native-glass host in the app background and one trailing AppKit accessory for the real foreground content. Material coverage and control exclusion are distinct bounds.                                                              |
| Window buttons and native sidebar toggle  | AppKit; never replace them with lookalikes or modify private system views.                                                                                                                                                                                            |
| Agent conversation / Settings detail      | Their reading canvas below the header, respecting leading and top safe areas.                                                                                                                                                                                         |
| Agent Context                             | A compact, neutral opaque card over the conversation; no full-height trailing material or decorative glass owner.                                                                                                                                                     |
| Onboarding window, navigation and actions | `OnboardingWindowController` owns the native host; `OnboardingView` owns header/progress and footer navigation, composing its root through the existing `OverlayCanvasBackdrop` owner with `.chrome` and the onboarding tint scope.                                   |
| Onboarding reading content                | `OnboardingView` owns `stepHeading` and the scrollable reading column; `OnboardingSteps.swift` owns the step bodies and their controls. Their reading role does not become chrome merely because the current root supplies glass behind them; see the boundary below. |
| Overlay canvas material and reading veil  | `OverlayCanvasBackdropView` in `OverlayCanvasSurface.swift`; `.chrome` and `.readingCanvas` are distinct roles.                                                                                                                                                       |
| Expanded overlay header material          | `OverlayControlGlass` composed by `DictationOverlayView`; mini/MIDI already use their root chrome material and do not add another header sheet.                                                                                                                       |
| Overlay resize feedback                   | `OverlayResizePreview`; temporary frozen foreground plus a live backdrop using the canvas owner's policy.                                                                                                                                                             |

Paths in the table are under `macos/Codescribe/DesignSystem/` or
`macos/Codescribe/Screens/Overlay/`, except the Agent/Settings screen owners
and onboarding under `macos/Codescribe/Screens/Onboarding/`.
The table records responsibilities, not permission to stack all these materials
over the same pixels. Check the complete backing chain, including ancestors.
If ownership is wrong, repair the existing owner or replace it coherently;
do not add a parallel material, alternate header or fallback renderer.

**Onboarding boundary.** Header/progress and Back/Skip/Continue/Finish are the
orientation, navigation and action layer. Step titles, explanations, permission
rows, forms and summaries belong to the reading/content layer. Do not add a new
decorative glass surface around every step, form or card. Apple's
[Materials](https://developer.apple.com/design/human-interface-guidelines/materials)
guidance distinguishes Liquid Glass navigation/controls from the content layer,
where standard materials provide differentiation.

The current `OnboardingView` places a single `.chrome` backdrop behind the whole
wizard, including reading content; in standard mode that backdrop is not a
separate opaque reading canvas. Its intent is the requested coherent glass family, not a rule
that large reading areas should use Liquid Glass. Treat this full-window use as
a product-specific composition requiring an explicit rationale and the same
foreground, accessibility and motion evidence as other glass. The scope entry
alone does not supply that acceptance. Any future material-boundary change must
preserve the approved live-glass character and name the affected owner; this
documentation update neither redesigns the wizard nor authorises an opaque mask
or removal of its glass. Onboarding readability acceptance remains open.

### G4 — Repair the cause; never paint over chrome defects

Do not hide a seam, gap, tone jump or unreadable foreground by:

- adding an opaque colour wash above **or below** chrome glass;
- extending a reading canvas into the header/sidebar sampling area, including
  a small negative-padding, offset or safe-area bleed used as a seam patch;
- raising veil/tint opacity, adding a masking stripe, duplicate material,
  synthetic border or shadow merely to conceal the defect;
- removing glass, suppressing all visible background letters, or disabling
  the system's adaptation to make screenshots match;
- selecting, hiding or changing private AppKit/SwiftUI system views.

A two-point cover is still a cover. Inspect ownership, backing, coordinate
conversion, clipping, safe areas and sampling before selecting a repair.
Material bleed that moves an unwanted optical edge outside a legitimate clip
is different from painted backing, but still needs evidence that it preserves
coverage, native controls and the intended optical result.

This prohibition does not remove quiet reading canvases, the neutral Context
card, or required accessibility surfaces. They have their own roles and bounds.
An existing reading veil is not a precedent for putting one behind chrome.

This seam-mask prohibition is a deliberate **Codescribe product rule**. It is
not a claim that Apple forbids every backing or dimming treatment: Apple's
[Materials](https://developer.apple.com/design/human-interface-guidelines/materials)
guidance includes dimming behind clear glass over bright media. That separate
use case is not approval to conceal a seam or tone defect in our regular chrome.

### G5 — Preserve header character through presentation changes

Opening content or changing mini → expanded → MIDI must not turn the same
header into a conspicuously different grey/cream plate at unchanged settings.
Check the scene behind the window, activation, appearance and effective tint
when comparing. Native adaptation may change pixels; pixel-identical colour
across different backdrops is not the contract.

Do not equalise modes by painting them all opaque or by weakening a reading
surface's comfort. Investigate role and backing changes across the transition,
including a reading canvas reaching behind an otherwise un-veiled header.
Preserve the intended distinction between live chrome and quiet reading content.

The current developer-only Glass tint implements the requested experimentation
tool through native tint for onboarding, mini/MIDI and the Bus header.
Production ignores retained developer values, and Agent/Settings do not read
them. It is not a blur/refraction control or a permitted default workaround for
a seam or mode mismatch. No final palette or numeric tint recipe was selected;
an experimental setting or a screenshot's current value does not establish one.

### G6 — Keep accessibility and interaction correct

Preserve Reduce Transparency, Increase Contrast and Reduce Motion behaviour.
An opaque accessibility canvas is intentional and is not an opaque-wash workaround in
normal mode. Do not force transparency to satisfy a glass screenshot, or
silently turn on an accessibility path to hide a material defect. Record the
effective policy in evidence and verify both directions of a policy change.

With Reduce Motion enabled, reduce or replace automatic/repetitive animation
and large spatial morphs without losing state, meaning or actions. Check step
changes, mini/MIDI/expanded transitions and any affected glass animation, both
when the preference is set before opening and when it changes while open.
Turning it off must restore the intended presentation without stale layers or
duplicate animations. This does not require removing intentional static glass,
changing direct pointer tracking, or restoring live Markdown reflow. Follow
[Apple's accessibility guidance](https://developer.apple.com/design/human-interface-guidelines/accessibility).

Passive material must not steal clicks, keyboard focus, text selection,
scrolling or drag/resize gestures. Native controls and foreground actions stay
operable at minimum and normal widths, with the sidebar open and closed.

For changed foreground elements, retain objective evidence alongside optical
review:

- Use semantic system foreground colours where appropriate and evaluate
  text/icon contrast against the **rendered effective material**, in the
  relevant themes, activation and accessibility states and contrasting scenes.
  Record the foreground size/weight and the applicable contrast criterion from
  [Apple's accessibility guidance](https://developer.apple.com/design/human-interface-guidelines/accessibility).
  One ratio between palette constants does not certify a moving glass surface.
- Inspect labels, roles, values, enabled/selected state and hit regions with
  [Accessibility Inspector](https://developer.apple.com/documentation/accessibility/accessibility-inspector).
  Retain supported audit results and explain unchecked cases; an audit alone
  does not establish a fully accessible interaction.
- Exercise keyboard-only navigation, focus visibility/order, activation and
  dismissal for the affected controls, and navigate them with VoiceOver to
  verify meaningful announcements and the intended reading order. Presence in
  the accessibility tree is not proof that these actions work.

If a platform/tool cannot audit a property, state the limitation and retain the
corresponding direct check as open until performed. These checks do not permit
erasing the optical scene to obtain a contrast pass.

### G7 — Preserve resize and hidden-window performance protections

Expanded resize uses the Founder's selected frozen image of the full content,
without stretching text or reflowing Markdown at pointer frequency. The live
backdrop follows the preview bounds, including newly exposed space; the frozen
foreground must not carry an old-sized opaque canvas, rounded corners or
accumulated stale pixels. Finish lays out the real content once; cancellation
restores the original frame and input/visibility state.

Keep observation scoping, projection coalescing, covered-conversation rendering
gates and hidden/occluded animation gates. Do not restore expensive live reflow
to hide preview defects. Detached groups retain their separate header-only
feedback contract. The detailed mechanics and capture-reuse invariant live in
[Overlay presentation](../OVERLAY_PRESENTATION.md#resize-feedback-and-performance-boundary-2026-11-08).

Still frames cannot establish smoothness. A passing geometry or capture-count
test cannot overrule a reported lag. Compare actual resize gestures and collect
a profile when needed; unequal manual gestures are not a matched performance
benchmark. Retain unresolved runtime and suite failures as unresolved.

## Change admission and STOP rule

Read this contract before changing glass, chrome, reading-background bounds or
resize feedback, including layout changes that move their boundaries. Keep a
restyle of Context, the composer or conversation from silently changing the
shared glass system.

Stop admission of the affected change when it violates G1–G7, relies on a cover
instead of a causal repair, or claims optical/performance success without the
relevant evidence. An unexplained image is a reason to obtain the missing
evidence, not to invent its cause or automatically blame the newest PR.
Unrelated changes in a mixed patch can be assessed separately. Do not waive the
contract or redefine the product goal merely to make a check green; a product
change requires an explicit Founder decision.

A casual reaction, screenshot or assistant proposal is not a new product
decision. Recover its context and later corrections before treating it as one.
The Founder assesses the visual result; engineering, interaction, accessibility
and performance verification remain the implementer's responsibility.

For a glass/layout repair, retain a reviewable receipt containing:

1. Exact base/head and build identity, host/macOS, effective appearance,
   accessibility policy (including Reduce Motion) and developer tint, and the
   trigger being compared.
2. The observed defect, the proposed causal mechanism, its owner, and a
   controlled before/after check that could disprove the explanation.
3. The relevant native/structural regression checks and their actual results.
4. Compositor images and motion evidence for the affected cases below, with
   unavailable cases and remaining failures stated explicitly.

| Check                   | Required evidence for the affected surface                                                                                                                                                                                     |
| ----------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| Material and foreground | Light/Dark and active/inactive on neutral/dark and contrasting colour/text backdrops; rendered foreground contrast, supported Accessibility Inspector checks, keyboard-only and VoiceOver operation for changed elements (G6). |
| Scroll interaction      | Real content moving beneath the header; inspect the whole window and both horizontal and vertical joins.                                                                                                                       |
| Geometry                | Minimum/normal widths, sidebar expanded/collapsed, Context open/closed where affected; native controls remain reachable.                                                                                                       |
| Mode continuity         | Mini → expanded → MIDI at unchanged settings, recording the scene and activation; inspect transition and settled result.                                                                                                       |
| Resize                  | Actual drag, growth/shrink, release/cancel and consecutive gestures; inspect new space, clipping, restoration and latency.                                                                                                     |
| Accessibility / OS      | Standard, Reduce Transparency, Increase Contrast and Reduce Motion; check affected transitions with policies set before opening and changed while open, restoration, supported API branches and untested versions.             |

Run the relevant changed cases, not an unrelated screenshot campaign. Existing
checks include `AgentWindowChromeTests`, `SettingsWindowChromeTests`,
`CSSidebarColumnTests` and `OverlayResizePreviewTests`. These prove their stated
structural/behavioural contracts, not optical acceptance or frame pacing.

Use a real desktop/compositor capture and direct observation for scene sampling.
A single-window PNG may omit backdrop contribution or contain transparent
pixels that a viewer displays black. It cannot by itself prove either absent
glass or a black-painted runtime region. Label native window captures, fixture
renders and real desktop captures distinctly. A hierarchy/property assertion
or a synthetic screenshot is not a substitute for the running user path.

## Current construction and geometry assumptions

This section describes the 2026-11-09 routing repair after the `0a3aaaa90`
preview. Native and optical acceptance of that repair are separate below.
Dimensions and hierarchy assumptions may change through an evidenced repair;
G1–G7 remain the acceptance criteria.

Continue the AppKit header implementation on `codex/window-chrome-e2e` as the
single development direction (Founder, 2026-11-08). The parallel SwiftUI header
experiment remains a reference, not a second runtime path.

`NavigationSplitView` owns sidebar glass. `CSWindowTopHeader` owns one passive
rectangular `NSGlassEffectView` in its app-owned background marker, and one
trailing (`.right`) titlebar accessory for the actual foreground content. The
standard unified allocation is currently 52 pt high; AppKit owns its height.
With the sidebar expanded, material starts at the sidebar boundary. Collapsed,
it covers the full window width behind the native control region. The material
host always returns nil from hit-testing. Foreground content remains in the
native accessory, outside AppKit's leading event region; no click forwarding,
replacement control or private system-view mutation is used.

The earlier `.top` accessory container intercepted the sidebar toggle. Moving
the whole glass to `.right` repaired that event route but left 212–216 pt of
native controls without material. The subsequent construction separates these
responsibilities: one background material can cover the control region without
reserving it as an interactive accessory. Its glass contentView is only an
empty passive backing, not a second copy of the header. Native foreground and
material retain one coordinator and independent coordinate conversion. This
composition still needs optical/foreground and fullscreen acceptance.

Agent and Settings set `titlebarSeparatorStyle = .none` to avoid an additional
system separator; this does not remove the glass optical edge.
The Agent window has no trailing column: since 2026-11-09 its context is an
opaque card floating over the conversation, which owns no glass. The
conversation alone owns the opaque reading canvas.

Reading backgrounds respect both leading and top safe areas. The November 8
striped/text desktop captures showed that allowing `.top` behind titlebar
glass preserved a neutral sheet above the conversation while the trailing
header sampled desktop colours and letters. Remove that opaque top extension
in Agent and Settings; keep the reading canvas below it and preserve bottom
coverage. Desktop comparison remains the optical acceptance gate.

The passive marker has an explicit clipping viewport matching the native band
(52 pt in the current unified composition). Only this material background opts
into the top safe area; the opaque reading canvases remain below it. The native material
extends 8 pt below and to its leading side, placing the joined optical rims
outside the visible band;
the real controls retain the original 52 pt frame. No opaque masking stripe is
added. This targets the activation-sensitive rim still seen after the separate
AppKit separator was disabled. Its optical result remains subject to paired
active/inactive Light/Dark captures; geometry assertions alone are insufficient.

The leading extension also places the material's top-left bevel outside the
sidebar/detail join. Text-backdrop captures exposed that small diagonal notch
once the header could sample the scene. The expanded viewport keeps the sidebar boundary. On collapse only the passive
material extends behind native controls; their event region remains AppKit's.

The trailing context column and its material, extended 8 pt above the panel,
were removed on 2026-11-09 with the Context card. The obsolete trailing-column
join no longer exists. This does not establish acceptance of the remaining
header, collapsed-sidebar region or any other glass boundary.

### Overlay mode backing boundary

`DictationOverlayView` supplies its measured `headerHeight` to
`OverlayCanvasSurface`. In expanded mode, `OverlayCanvasBackdropView` restricts
both the native reading material and its veil to the region below that header.
The expanded header retains its existing `OverlayControlGlass` owner, without
another material or the reading veil behind it. Mini and MIDI keep their
full-window `.chrome` backdrop. This repairs the overlapping backing chain;
it does not select a new palette, increase veil opacity or tune away a mismatch.

The shared backdrop configuration retains the measured top inset during frozen
resize. Its existing window perimeter and outer clip stay full-size; the inset
reading layers have a square top edge so they introduce neither an extra join
border nor rounded holes. Other backdrop consumers retain the zero-inset default.

The expanded header's own control-glass shape rounds only its outer top
corners. Its straight lower edge joins the square-topped reading canvas across
the full width. A four-corner rounded header left uncovered wedges at both
ends once the reading backdrop correctly stopped below it. This is repaired
in the existing `OverlayControlGlass` shape, without extending reading backing
behind chrome. Compact modes retain the root canvas as their sole material;
floating controls keep all four of their own rounded corners. Native material,
tint, veil opacity and frozen-resize mechanics are unchanged. Shape coverage
tests do not replace real compositor and transition acceptance of this join.

Native tests exercise repeated mini → expanded → MIDI transitions in both
appearances and verify reading/header separation. Resize coverage checks that
the preview retains the source's inset and accessibility policy. These are
geometry and ownership checks; compositor tone continuity, the join and actual
resize motion still require the G5–G7 optical/performance evidence.

## Why

One owner per surface avoids overlapping sidebar materials and runtime alpha
workarounds. Visible letters and colours through glass may be the intended
effect; the foreground controls must remain readable. Do not solve an optical
integration defect by removing glass or adding an opaque wash.

The header locates the native `NSSplitViewController` through the public
`NSSplitView.delegate` and its two items, with the first marked `.sidebar`.
It converts that sidebar's boundary into the accessory's coordinate system.
When collapsed, native window buttons and toolbar item views establish the
leading exclusion. Frames are converted through screen coordinates so a
fullscreen reveal moving system toolbar views to another window cannot mix
unrelated local coordinate systems. The accessory width follows that exclusion;
its content origin is converted independently from its actual hosting window. These APIs are public, but SwiftUI's hierarchy and accessory
allocation are implementation assumptions, not an Apple layout guarantee.

An early attachment without `.fullSizeContentView` is deferred, with a debug
diagnostic; Settings establishes its window on the same run-loop turn and the
marker retries. An occupied top or trailing accessory header region is logged as an error and
is never replaced. Resize observation is scoped to that split view and removed
on detach. Window update notifications reapply the separator policy after
SwiftUI toolbar adoption. Detach uses AppKit's indexed accessory removal,
not mutation of the returned accessory array. No assertion is raised for an
expected scene-adoption transition.

## Validation and limits

On macOS 26.1 and later the titlebar accessory explicitly requests the system
soft scroll-edge effect through `preferredScrollEdgeEffectStyle`. Glass
sampling of the scene and treatment of scrolling document content are separate
mechanisms; the regular material is unchanged. macOS 26.0 retains its system
default because this AppKit preference is unavailable there. This does not
request a fixed blur radius or remove letters behind glass. Foreground
legibility and the moving-content result require optical review.

The November 8 join-layer diagnostic records public view/layer geometry in the
real window. Both native sidebar and header glass report zero corner radius;
the custom header's physical leading rim is outside its clipped viewport.
AppKit also creates a titlebar boundary region at the join. Its presence is
evidence of another compositing boundary, not proof of the photographed notch's
cause. Do not select, hide or modify private system views to suppress it.

A disposable-window A/B on November 8 isolated the native tracking toolbar
separator with public `NSToolbar.removeItem(at:)`. The header viewport remained
at the same boundary and size, and the 5.5 pt titlebar boundary region remained.
The sidebar toggle's glass backing disappeared from the captured join hierarchy.
This does not establish the pixel source of the photographed notch, but rules
out separator removal as a geometry-level remedy that preserves native chrome.
The earlier `isHidden` trial left the tracking item reported as visible. Neither
experiment is applied in product code. Retain the result-bundle attachments as
diagnostic evidence; do not keep toolbar mutation in the production or normal
regression path. The next optical comparison must distinguish the native
sidebar/titlebar junction from the separately clipped custom glass edge.

Agent/Settings window tests cover the accessory count, native control exclusion,
the unified allocation, sidebar boundary, minimum/normal widths and
collapsed/expanded states. Native toolbar route checks include an accessory
removal control; a hit-test receipt is not a completed physical click cycle. Attachment lifecycle tests cover deferral, a competing owner, retry,
idempotence and detach. These tests do not prove visual integration, refraction,
foreground contrast or frame pacing. Review active/inactive windows, Light/Dark,
contrasting desktop backgrounds and scrolling content separately.

The 2026-11-08 screenshots exposed an activation-sensitive horizontal seam and
different adaptation of the context column. Disabling the additional separator
addresses a known extra drawing owner; visual comparison must still establish
whether any remaining line belongs to glass itself. Do not claim this issue
resolved from a property assertion alone.

## Open acceptance at the documentation checkpoint

These are observed issues or evidence gaps, not additional approved designs or
settled root causes:

- On November 9 the Founder confirmed that the sidebar button fails in Agent
  and Settings while Control-Command-S still opens it. The native A/B identifies
  top-accessory container obstruction; the trailing-slot repair needs the same
  native tests and real pointer acceptance. Fullscreen moving/overlapping
  branding, intermittent traffic-light reachability and banding also require
  a live reveal/exit comparison; coordinate conversion alone does not certify it.
- The 2026-11-09 capture investigation reported a rectangular transparent region
  around native controls and a lower header band with the sidebar closed,
  including transparency predating the Context card. Its live compositor
  result and the coverage/backing cause still need verification; do not call it
  black paint or attribute it to the card from the PNG alone.
- The Founder reports a large header tone change between compact and expanded
  forms at unchanged settings, including mini → expanded → MIDI. Sampling and
  backing across these modes still need investigation; no causal explanation
  or visual workaround is accepted by this checkpoint.
- Frozen resize has structural regression coverage and a single-capture repair.
  The reported lag and visual transition quality still require live acceptance;
  targeted tests do not establish a full-suite or release pass.
- The onboarding root currently extends chrome glass behind reading content.
  Its rationale, rendered foreground contrast, assistive interaction and Reduce
  Motion behaviour need the explicit acceptance described in G3/G6. No such
  runtime audit was performed by this documentation follow-up.

The small multicoloured native rim was explicitly left alone on 2026-11-08.
That decision does not accept the separately reported notch, seam or uncovered
region. Preserve this distinction during later repairs.

## Decision provenance

The source conversation is session `01a1b633-05ce-7ed3-8032-a7de0455c17a`.
Relevant Founder decisions in its cleaned export (timestamps are UTC):

- **2026-11-07 18:19:19:** explicit Agent/Settings visual brief: continuous
  native header, adaptive sidebar, quiet detail, visible moving content and
  foreground legibility; the running result outranks the name of an API.
- **2026-11-08 16:21:14 / 16:24:09:** preserve Markdown performance protections
  and use the frozen full-content image during resize.
- **2026-11-08 20:01:00:** leave the small coloured rim alone; do not turn it
  into a requirement to eliminate all native edges.
- **2026-11-09:** Context becomes a neutral floating card; the large compact/
  expanded tone jump is rejected; opaque colour compensation is explicitly
  forbidden; the Founder authorises STOP for changes that damage glass.

Later corrections take precedence over earlier assistant proposals. In
particular, the earlier judgment that visible background letters alone make
glass unacceptable was withdrawn; it is not a standing design rule.
The cleaned export records 36 omissions and a provenance warning. It supports
the explicit decisions quoted here, not a claim to have audited every source
event, implementation or test run. The construction section was checked
separately against the stated local integration.

## Revisit if

- A macOS update changes split hierarchy, sidebar behaviour or accessory sizing.
- A window gains another top accessory or its header cannot attach.
- Sidebar collapse, resizing or fullscreen changes move controls under glass.
- Optical review still shows seams or unreadable controls after removing the
  separate separator; inspect actual sampling/backing and material boundaries.

Public API references:
[NSGlassEffectView.contentView](https://developer.apple.com/documentation/appkit/nsglasseffectview/contentview),
[NSTitlebarSeparatorStyle](https://developer.apple.com/documentation/appkit/nstitlebarseparatorstyle).

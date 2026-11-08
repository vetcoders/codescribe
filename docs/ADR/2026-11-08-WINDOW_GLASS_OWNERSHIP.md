# Window glass ownership and titlebar geometry

## Decision

Continue the AppKit header implementation on `codex/window-chrome-e2e` as the
single development direction (Founder, 2026-11-08). The parallel SwiftUI header
experiment remains a reference, not a second runtime path.

`NavigationSplitView` owns sidebar glass. `CSWindowTopHeader` owns one 52 pt
top titlebar accessory containing one rectangular `NSGlassEffectView`; the
actual header is hosted inside its `contentView`. AppKit owns window controls and the native
sidebar toggle. Agent and Settings set `titlebarSeparatorStyle = .none` to avoid
an additional system separator; this does not remove the glass optical edge.
The Agent context column has its own real-content glass below the titlebar.
The conversation alone owns the opaque reading canvas.

Reading backgrounds respect both leading and top safe areas. The November 8
striped/text desktop captures showed that allowing `.top` behind titlebar
glass preserved a neutral sheet above the conversation while the trailing
header sampled desktop colours and letters. Remove that opaque top extension
in Agent and Settings; keep the reading canvas below it and preserve bottom
coverage. Desktop comparison remains the optical acceptance gate.

The accessory has an explicit 52 pt clipping viewport. The native material
extends 8 pt below and to its leading side, placing the joined optical rims
outside the visible band;
the real controls retain the original 52 pt frame. No opaque masking stripe is
added. This targets the activation-sensitive rim still seen after the separate
AppKit separator was disabled. Its optical result remains subject to paired
active/inactive Light/Dark captures; geometry assertions alone are insufficient.

The leading extension also places the material's top-left bevel outside the
sidebar/detail join. Text-backdrop captures exposed that small diagonal notch
once the header could sample the scene. The viewport and controls keep the
original sidebar boundary; no material or hit area is added over native controls.

The trailing context material similarly extends above its clipped panel by
8 pt, with compensating padding preserving its content geometry. The November
8 19:14–19:15 captures showed the central header edge softened while the active
context panel's top rim remained visible, especially in Light mode. This is a
separate edge from the intentional divider below the context title. Its new
clipping still requires optical acceptance.

## Why

One owner per surface avoids overlapping sidebar materials and runtime alpha
workarounds. Visible letters and colours through glass may be the intended
effect; the foreground controls must remain readable. Do not solve an optical
integration defect by removing glass or adding an opaque wash.

The header locates the native `NSSplitViewController` through the public
`NSSplitView.delegate` and its two items, with the first marked `.sidebar`.
It converts that sidebar's boundary into the accessory's coordinate system.
When collapsed, native window buttons and toolbar item views establish the
leading exclusion. These APIs are public, but SwiftUI's hierarchy and accessory
allocation are implementation assumptions, not an Apple layout guarantee.

An early attachment without `.fullSizeContentView` is deferred, with a debug
diagnostic; Settings establishes its window on the same run-loop turn and the
marker retries. An occupied top accessory slot is logged as an error and is
never replaced. Resize observation is scoped to that split view and removed
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

Agent/Settings window tests cover the accessory count, native control exclusion,
52 pt allocation, sidebar boundary, minimum/normal widths and collapsed/expanded
states. Attachment lifecycle tests cover deferral, a competing owner, retry,
idempotence and detach. These tests do not prove visual integration, refraction,
foreground contrast or frame pacing. Review active/inactive windows, Light/Dark,
contrasting desktop backgrounds and scrolling content separately.

The 2026-11-08 screenshots exposed an activation-sensitive horizontal seam and
different adaptation of the context column. Disabling the additional separator
addresses a known extra drawing owner; visual comparison must still establish
whether any remaining line belongs to glass itself. Do not claim this issue
resolved from a property assertion alone.

Developer-only **Glass tint** remains an explicitly requested team tool for
onboarding, mini/MIDI and the bus header. It controls native tint, not blur or
refraction, and does not tune Agent/Settings. Production ignores its values.

## Revisit if

- A macOS update changes split hierarchy, sidebar behaviour or accessory sizing.
- A window gains another top accessory or its header cannot attach.
- Sidebar collapse, resizing or fullscreen changes move controls under glass.
- Optical review still shows seams or unreadable controls after removing the
  separate separator; inspect actual sampling/backing and material boundaries.

Public API references:
[NSGlassEffectView.contentView](https://developer.apple.com/documentation/appkit/nsglasseffectview/contentview),
[NSTitlebarSeparatorStyle](https://developer.apple.com/documentation/appkit/nstitlebarseparatorstyle).

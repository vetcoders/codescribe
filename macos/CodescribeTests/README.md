# CodescribeTests — what runs, and what does not

These are the Swift unit tests for the SwiftUI front-end. They are **not** part
of any automated gate today. Read the limitation below before you treat a green
`make check` as covering anything in this directory.

## Running them

```bash
CODE_SIGN_IDENTITY="-" xcodebuild test -scheme Codescribe
```

The ad-hoc signing identity is required — without it the test host fails to
sign and never launches. `macos/project.yml` documents
`xcodebuild test -scheme Codescribe` as the entry point.

## The limitation: the test host boots the Rust core

`xcodebuild build-for-testing` succeeds. It is the **run** phase that hangs
(`hung before establishing connection`; ~345 s to give up, measured in the W12
W2-B cut report, 2026-08-08).

The cause is construction order, not the tests:

- The XCTest host reuses the Codescribe app bundle, so `AppDelegate` is
  instantiated in the host process.
- `AppDelegate`'s stored properties are initialised at that instantiation —
  `private let model = AppModel.shared` (`macos/Codescribe/App.swift:115`) and
  `private let onboarding = OnboardingWindowController(...)` (`App.swift:149`).
  `AppModel.shared` boots the Rust core.
- That happens **before** `applicationWillFinishLaunching`, so the
  `Self.isRunningTests` guards (`App.swift:163` and `App.swift:175`) never get
  the chance to stop it. Those guards correctly keep the host from starting
  hotkeys, the Sparkle updater, and the engine prewarm — they do not, and
  cannot, prevent eager property construction.
- With the real `Codescribe.app` running, the host's second core instance
  fights the live one for the CGEventTap and the microphone, and the run hangs.

`scripts/smoke-macos27.sh` works around the same problem by compiling
production sources into standalone probes instead of using an XCTest host.

## What this costs us honestly

The W12 Swift-side behaviour changes are verified **by compilation only**:

- `OverlayStateTests.swift` — the overlay marker rebase
  (`renderedOffset`, `rebaseContextMarkers`, `liveTextOffset` in
  `Screens/Overlay/OverlayState.swift`)
- `ComposerMicTests.swift` — the composer `onReplaceRange` path
  (`ComposerDictation.swift`)

No CI executes them: the self-hosted runners run cargo only. What does cover
adjacent ground:

- Rust-side patch/marker coexistence tests in `core/pipeline/live_assembly.rs`
  (including char-vs-byte offsets on Polish diacritics and out-of-range drops)
- the operator smoke rows in `scripts/smoke-macos27.sh`

The overlay-side rebase arithmetic itself executes only in production. A
`{selection_N}` fence drifting mid-word is an agent-lane corruption that daily
use will hit before any gate does.

## Named follow-up (not done here)

Make `AppDelegate`'s core-touching stored properties lazy, or construct them
behind the existing `isRunningTests` check, so the XCTest host stops booting a
second core. Then wire `xcodebuild test` into a verifier and let this directory
start earning its keep. Until that lands, no plan may cite these tests as
executed evidence.

_𝚅𝚒𝚋𝚎𝚌𝚛𝚊𝚏𝚝𝚎𝚍. with AI Agents by Vetcoders (c)2024-2026 LibraxisAI_

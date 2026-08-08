# CodescribeTests — what runs, and how

Swift unit tests for the SwiftUI front-end. **317 tests, executed by
`make test-swift`.**

```bash
make test-swift                                    # whole suite
make test-swift SWIFT_TEST_ARGS='-only-testing:CodescribeTests/OverlayStateTests'
```

The target lives in the root `Makefile`; read its comment block before invoking
`xcodebuild` by hand, because two of the traps below cost this plan a stage.

## Invocation traps

1. **`CODE_SIGN_IDENTITY="-" xcodebuild …` does nothing.** xcodebuild reads
   build settings from *arguments*, never from the environment. The env-prefix
   form leaves `project.yml`'s `Codescribe Dev` identity in place and the build
   dies with `No certificate matching 'Codescribe Dev' found` before a single
   test runs. It must be a positional `KEY=value`. This README documented the
   broken form until 2026-08-08.
2. **Do not pass a real identity either.** An `Apple Development: …` identity
   propagates into the SPM package targets, which then fail with
   `Signing for "HighlightSwift_HighlightSwift" requires a development team`.
   Tests need a host that launches, not a distributable one — ad-hoc `-` is
   correct here, and is what `make test-swift` uses.
3. **`-scheme Codescribe` is mandatory.** xcodegen emits no scheme for
   `bundle.unit-test` targets, so no `-target CodescribeTests` invocation can
   resolve the SPM dependencies.
4. **A `-only-testing` filter that matches nothing exits 0** with
   `** TEST SUCCEEDED **` and `Executed 0 tests` — a silent pass, the same trap
   `cargo test <filter>` carries. `make test-swift` fails with rc 3 when a run
   executes zero tests.

## Why the suite was believed unrunnable

Until 2026-08-08 this file said the suite could not run: that the XCTest host
boots the Rust core through `AppDelegate`'s eager stored properties and hangs
(`hung before establishing connection`, ~345 s), so the tests were "verified by
compilation only" and no plan could cite them as evidence.

Half of that was right.

**Right — the eager properties really do boot a second core in the test host.**
The XCTest bundle uses the app as its host, so `AppDelegate` is instantiated in
the test process, and stored properties initialise at instantiation — *before*
`applicationWillFinishLaunching`, which is where the `isRunningTests` guards
live. Those guards are structurally unable to prevent it. `AppModel.shared`
builds the chat/overlay/tray engines and `TrayStatusStore.init` registers a live
listener on the core.

That listener answers `ConfigChangeBus.holdBadgeDidChange`, so a settings unit
test running entirely on a mock engine drove real core work. Measured, whole
suite, same binary except for `let` vs `lazy var` on those properties:

| build | suite | slowest test |
|---|---:|---|
| eager stored properties | 86.7 s | `SettingsTruthTests.testHoldBadgeControlRoundTrips…` 82.3 s |
| `lazy var` (current) | 4.1–4.5 s (n=4) | 0.009 s |

The fan-out is the mechanism: XCTest retains every `XCTestCase` instance for the
whole run, so each view model an earlier test built is still a live observer
when a later test posts on the bus. That test costs **0.009 s alone** and
**82.3 s** in suite position — the cost was never in the test.

**Wrong — the hang does not reproduce.** Checked on 2026-08-08 at HEAD
`6784b160`, i.e. *before* the `lazy` change, on macOS 27 / Xcode 26.6:

| condition | result |
|---|---|
| pre-fix, `Codescribe.app` **not** running | full suite green, 86.7 s |
| pre-fix, `Codescribe.app` **running** (pid 51794) | `ComposerMicTests` green, ~4 s |
| post-fix, app running (pid 13987) | 71 tests green, 0.108 s |

No hang in any arm. The first cold run after a rebuild takes ~20–30 s (dyld
warming the 624 MB `libcodescribe_ffi.dylib`, plus re-signing); the ~345 s
figure is most consistent with a cold run against a give-up timeout rather than
a deadlock, but nobody has reproduced it, so treat it as unexplained rather than
fixed. If you do see a hang, capture a sample of the host process — that is the
missing evidence.

The `lazy` change stands on its own measurement (20× faster suite, no core in
the test process), independent of the hang question.

## Coverage this actually buys

317 tests across 28 files, including the two surfaces the W12 plan could
previously only verify by compilation:

- `OverlayStateTests.swift` — the overlay marker rebase (`renderedOffset`,
  `rebaseContextMarkers`, `liveTextOffset`). 60 tests.
- `ComposerMicTests.swift` — the composer `onReplaceRange` path, including the
  `firstIndex` → `lastIndex` alignment. 11 tests.

`make test-swift` is **not** part of `make check`: it needs Xcode and a built
ffi dylib, and the self-hosted CI runners are cargo-only. Wiring it into CI
needs Xcode on a runner — that is a real open item, not an oversight.

## Known residue

The suite is ~4.3 s, of which one test still carries the bus fan-out described
above. It is bounded now, but it grows with every future test that builds a
`SettingsViewModel` or `TrayViewModel`, because those register on
`NotificationCenter.default` and live until the run ends. A scoped notification
centre for `ConfigChangeBus` would remove the class; that is a production design
change and was left for the operator to decide.

_𝚅𝚒𝚋𝚎𝚌𝚛𝚊𝚏𝚝𝚎𝚍. with AI Agents by Vetcoders (c)2024-2026 LibraxisAI_

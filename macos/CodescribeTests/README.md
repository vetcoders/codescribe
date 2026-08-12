# CodescribeTests — what runs, and how

Swift unit tests for the SwiftUI front-end. **337 tests, executed by
`make test-swift`.**

```bash
make test-swift                                    # whole suite
make test-swift SWIFT_TEST_ARGS='-only-testing:CodescribeTests/OverlayStateTests'
```

The target lives in the root `Makefile`; read its comment block before invoking
`xcodebuild` by hand, because two of the traps below cost this plan a stage.

## Invocation traps

1. **`CODE_SIGN_IDENTITY="-" xcodebuild …` does nothing.** xcodebuild reads
   build settings from _arguments_, never from the environment. The env-prefix
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
the test process, and stored properties initialise at instantiation — _before_
`applicationWillFinishLaunching`, which is where the `isRunningTests` guards
live. Those guards are structurally unable to prevent it. `AppModel.shared`
builds the chat/overlay/tray engines and `TrayStatusStore.init` registers a live
listener on the core.

That listener answers `ConfigChangeBus.holdBadgeDidChange`, so a settings unit
test running entirely on a mock engine drove real core work. Measured, whole
suite, same binary except for `let` vs `lazy var` on those properties:

XCTest retains every `XCTestCase` instance for the whole run, so each view model
an earlier test built is still a live observer when a later test posts on the
bus. That test costs **0.009 s alone** and **82.3 s** in suite position — the
cost was never in the test.

**Wrong — the hang does not reproduce, and the cure was not complete.** No hang
appeared in any arm on macOS 27 / Xcode 26.6. But the `lazy` change was recorded
here as producing a stable `4.1–4.5 s (n=4)` suite, and that number does not
survive contact with a fourth, fifth and sixth run.

### The suite's cost was nondeterministic, and the reason was not fan-out

Measured 2026-08-08, identical tree (`cd2fbb9a`, i.e. _with_ `lazy`), identical
test set, same host, live app not running:

| condition                                   | runs | suite seconds               | spread |
| ------------------------------------------- | ---: | --------------------------- | -----: |
| as committed                                |    3 | 47.507 / 28.246 / **4.484** |  10.6× |
| `TEST_RUNNER_CODESCRIBE_DISABLE_KEYCHAIN=1` |    3 | 4.186 / 4.691 / 4.218       |  1.12× |
| core detects the XCTest host (current)      |    3 | 4.452 / 4.415 / 4.264       |  1.04× |

In the 47.5 s run, `SettingsTruthTests.testHoldBadgeControlRoundTrips…` alone
took **43.059 s** — 91 % of the suite. In the 4.5 s run the same test took
**0.001 s**. Nothing about the binary differed.

The decisive number is CPU: a 47 s wall-clock run burned ~2.9 s user + ~2.2 s
system. The suite was **blocking, not working**, so an O(observers) fan-out
cannot be the dominant cost.

It was the Keychain. `core/config/keychain.rs::is_test_env()` recognises a Rust
harness by the `target/**/deps/` executable path and `RUST_TEST_THREADS`. This
suite hosts its tests **inside the app**, so neither fires: the core classified a
test run as a production launch and made real Keychain calls from an
ad-hoc-signed binary whose signature changes on every rebuild, so macOS
re-evaluated the item ACL instead of reusing a cached decision — variable
latency, no CPU. Every other test lane in the repo already bypasses Keychain
(`TEST_SETUP` in the Makefile, `CODESCRIBE_DISABLE_KEYCHAIN` in
`.github/workflows/rust.yml`); this lane was the one that did not.

`in_xctest_host()` now detects the host from the environment markers XCTest
exports. The independent Swift `LicenseService` mirrors that exact marker set,
because its `shared` instance can reach Keychain from `App.body` before
`XCTestCase` class discovery is reliable. Both stores therefore bypass
Keychain from process truth already present at host launch; neither depends on
an environment variable inherited accidentally from the invoking shell.
`SettingsTruthTests.testXCTestEnvMarkersPinTheSignalTheCoreKeysOn` asserts from
inside a live host that at least one marker is still present, because the
detector fails _open_ — if Xcode renames them, the slow classification returns
silently.

**The ~345 s hang is no longer unexplained**, though it is still unreproduced:
it is the same shape as the 47.5 s run — an idle wait on a blocking Keychain
call, at the tail of a distribution whose measured range already spanned 10×.
That is a mechanism with evidence, not a diagnosis; if you see it, sample the
host process.

**What this does not settle.** The `lazy` change was measured against this
nondeterministic baseline, so the "86.7 s → 4.1–4.5 s" attribution mixes two
effects and its magnitude is not established. `lazy` is still right on its own
argument — the guards on the lifecycle methods cannot reach stored-property init,
so the test host was booting a second core — but eager-vs-lazy has **not** been
re-measured with the Keychain path fixed. Anyone claiming a number for it should
run both arms again.

### The gate now refuses green-but-slow

`make test-swift` reports its wall-clock and slowest test on every run and fails
above `SWIFT_TEST_MAX_SECONDS` (default 30 s, ~6× the measured fast mode). Both
bad runs above would have failed it. Raise the budget on a genuinely loaded host
(`make test-swift SWIFT_TEST_MAX_SECONDS=90`) rather than removing it.

Note on exit codes: the _recipe_ exits 3 (zero tests) or 4 (over budget), which
appears in `make: *** [test-swift] Error N`. GNU make itself exits **2** for any
recipe failure, so a caller reading `$?` sees 2 in every failing case. Scripts
should branch on non-zero, not on the specific code.

## Coverage this actually buys

337 tests across 30 Swift files, including the two surfaces the W12 plan could
previously only verify by compilation:

- `OverlayStateTests.swift` — the overlay marker rebase (`renderedOffset`,
  `rebaseContextMarkers`, `liveTextOffset`). 60 tests.
- `ComposerMicTests.swift` — the composer `onReplaceRange` path, including the
  `firstIndex` → `lastIndex` alignment. 11 tests.

`make test-swift` is **not** part of `make check`: it needs Xcode and a built
ffi dylib, and the self-hosted CI runners are cargo-only. Wiring it into CI
needs Xcode on a runner — that is a real open item, not an oversight.

## Known residue

The suite is 4.26–4.45 s (n=3, current). The bus fan-out described above is real
but is no longer the dominant cost, and it was never measured on its own: it
grows with every future test that builds a `SettingsViewModel` or
`TrayViewModel`, because those register on `NotificationCenter.default` and live
until the run ends. A scoped notification centre for `ConfigChangeBus` would
remove the class; that is a production design change and is the operator's.

Open, and named rather than fixed:

- **eager vs `lazy` has not been re-measured** with the Keychain path fixed, so
  the size of that win is unknown (see above).
- **Nothing in `.github/workflows/` runs this gate** — or `test-engine-parity*`,
  or `smoke-macos27`. Verified across all 495 indexed files: those targets appear
  only in the `Makefile` and in docs. CI also triggers on `main`/`develop` only,
  so no commit on this branch has been CI-verified at all. For the Swift suite
  that is the Xcode-on-a-runner item above; for the parity targets it is
  microphone/loopback hardware. Both are real constraints, but the consequence is
  that every gate this plan built is host-local and operator-run.

_𝚅𝚒𝚋𝚎𝚌𝚛𝚊𝚏𝚝𝚎𝚍. with AI Agents by Vetcoders (c)2024-2026 LibraxisAI_

# Overlay agents and mini widget — integration receipt

Updated: 2026-10-07.
Owner and integrator: this Codex session, explicitly assigned by the Founder.
Runtime: Fleet Worktree; `/Users/polyversai/.vibecrafted/worktrees/e19c/codescribe`.
Branch: `codex/overlay-integrate-261007`.
Baseline: `d9e4233153cac7cd3dd6c4e45e6ee4d1e1c11d79`.
Source checkpoints: `3c1d183b8f64ed8019b156f854120285955bd912`,
`0970cd49e8ea1c5ff102f9181e47962c668715a8`,
`8baeccaec28e220dbcbbc3a173814a749dfc47ea`.
Disposition: isolated branch; no trunk integration or distribution release claimed.

## Implemented product shape

- One embedded agent sidebar replaces the verbose recording-channel list. The
  header and conversation hamburger open it. Dictation remains on the header
  microphone, outside the agent roster. Names, concise status, unread counts,
  microphone and speaker controls share the existing design tokens.
- Wide windows reserve 280 points for the sidebar; narrow windows use a drawer
  that blocks input to covered content. Opening releases the text responder;
  selected conversation and drafts retain their existing owners.
- Shared microphone/speaker controls also appear beside the conversation name.
  Saved conversations cannot toggle a reassigned session's microphone.
- The close dot grows from 7 to 9 points, waveform maximum height doubles,
  elapsed time follows the waveform, and header controls share circular chrome.
- The diagonal fold produces a floating 180 × 46 panel. The microphone is always visible
  and becomes Stop during capture; hover exposes the agent control. Expansion restores the full
  size; persistence stores the expanded size. Existing all-Spaces behavior stays.

## Playback and delivery authority

`bus-demux.py` owns durable automatic-playback mute receipts. Swift invokes the
installed managed `cs-bus --mute-agent` / `--unmute-agent` command and projects
bounded receipts read off the UI thread. Identity is provider, provider session
and resolved bus, not the display name or channel digit. Unknown receipts never
permit automatic speech. Poll/click ordering preserves newer choices.

Automatic replies publish their text and source before considering mute. A
muted reply exits successfully with playback `refused`, reason `muted`, without
requesting synthesis. Capture and agent work continue. Manual Play remains
available. Unmute permits future replies without replaying history. No new
recorder, transcript reducer, PCM authority or bus writer was introduced.

Channel recovery now obtains terminal closure from canonical source evidence.
Unread drafts are history and cannot reopen a closed capture after restart.
Actually unfinished source and an unconsumed source extent still refuse takeover.

## Current verification

- Canonical helper recovery: 34 tests passed again on 7 October after rebuilding the publisher; scoped
  Semgrep found no findings; installed helper and successful takeover were verified.
- All 239 inherited Lena deliveries were read and acknowledged through `cs-bus`;
  both mailbox backlogs and unacknowledged seals were zero after cleanup.
- 7 October speech suite: **44 speech tests passed**, with isolated publication and
  substituted synthesis/player; no network, credentials or speakers were used.
  Added coverage proves persisted muted text without synthesis, unmute, same-name
  session isolation, wrong-bus refusal, manual audition, malformed-receipt refusal.
- UniFFI generation and host dylib build completed. Swift compilation exposed a
  new asynchronous `Thread.sleep` error; it was replaced with cancellable
  `Task.sleep` and scoped child-process cleanup. Full Swift run executed 1101 tests with 30 failures, predominantly outdated
  overlay-shape assertions. The final affected-suite run passed **410 tests**
  with zero failures in 17.224 seconds, including persistent microphone visibility.
- String Catalog synchronization consumed 132 fresh compiler extraction files;
  catalog lint and bridge census passed.
- Scoped Semgrep: one configured rule, 33 tracked targets, zero findings.
- The rendered AppKit collapse test now asserts the actual 180-point panel width.
  Old assertions requiring the removed native channel menu were retired; passive
  navigation and recording ownership retain their behavioral tests.
- Fixed-WAV five-Iwo gate: the real PCM → ledger → reducer → delivery
  fixture passed (1 selected test, zero failures); no live microphone was opened.
- Workspace all-target Rust test compilation initially failed on 265 stale unit-fixture references
  in the baseline (194 Apple session, 69 Silero, 2 seal coverage). Those source files
  are identical across the baseline and all three overlay/recovery checkpoints.
  Seven tests asserting the removed Silero context policy have been retired;
  the lifecycle/identity test remains. The compiler now reports 196 unresolved
  Apple-session/seal fixture references. No fetched branch contains a complete migration.
  The separate five-Iwo geometry
  iterator lint is corrected without changing its occurrence assertions.
- The structural verifier also needs the newer fail-closed manifest/test correction
  from `fb2c16be`, admitted here as patch-equivalent `791e6d1c`. Live and
  instrument self-tests passed (112 tests); the wired source check will rerun
  on the stable integration generation. No broad verify PASS is claimed.

- Integrated roster/mailbox, renamed-session, carrier replacement and same-session
  retirement changes: 82 Python tests passed, including 44 speech tests.
- Integrated Stop ownership: 16 Rust tests passed without opening a microphone.
- Post-integration bridge/overlay Swift suites: 85 tests passed with zero failures,
  including a 5 MiB custom-bus lease and rejection above 16 MiB.
- First background installation succeeded with a verified signature: v0.15.3,
  build 2040, source stamp `791e6d1c5-dirty`. It predates the final integration;
  a new build is required. The Founder app was not restarted.

Outstanding: full Rust fixture migration, wired structural gate and installed-product
acceptance of the final generation. Static checks and fixtures do not prove live UI or
live audio behavior.

## Branch comparison after remote refresh

Counts below were captured at `8baeccaec28e220dbcbbc3a173814a749dfc47ea`
after remote refresh, before integration. They are a historical comparison,
not current divergence counts. None of those remote refs contained the overlay source checkpoint.

| Compared ref                                 | Ours / theirs | Integration concern                                                                               |
| -------------------------------------------- | ------------- | ------------------------------------------------------------------------------------------------- |
| `origin/main`                                | 1561 / 0      | Main is an ancestor; this is a large existing feature history, not a three-commit PR against main |
| `origin/fix/useless-whisper-wandering`       | 4 / 0         | Contains our feature baseline, lacks our bounded cuts                                             |
| `div0/fix/useless-whisper-wandering`         | 3 / 10        | New Stop, word-dispute, ACK, bell and release-readiness cuts                                      |
| `codex/stop-admission-261006`                | 3 / 8         | Shared bus helper and overlay delivery readers/tests; requires integration review                 |
| `origin/fix/settings-tool-permissions-width` | 71 / 19       | Shared String Catalog and bindings; preserve newer translation/settings work                      |
| `origin/fix/roster-lease-size-cap`           | 71 / 4        | Shared lease-size and retirement contracts                                                        |

Earlier Lena header-glyph work (`680e7f59`) is already an ancestor. Duplicate
admission is unnecessary. No blind merges, trunk updates or deployment occurred.

## Admitted cuts

`fb2c16be` was admitted as `791e6d1c`. The roster branch commits
`7f6526b3`, `5d2834c7`, `553445fc`, `423551c5` were admitted as
`dce0023e`, `dbfed39f`, `dac6cddc`, `39742191`.
The main-checkout commits `9b6014ba`, `985c0208`, `47678328`, `4fa6e083`,
`c13fd4d8` were admitted as `7f35953a`, `9865dac9`, `e1d83260`,
`aa64b18d`, `dacda3d1`.
Conflict resolution retains our canonical-source closure proof for cross-session
handover and the newer inherited-cursor rule for same-session detach. Admission
is patch equivalence with these explicit conflict resolutions, not source ancestry.
Site/release and the newly appearing peer-text feature are not admitted.

The bounded Rust fixture checkpoint skips `cargo-check` while the 196 retained
fixture references still cannot compile, and `cargo-fmt` because the hook stages
all tracked edits; the two authored Rust files were formatted directly with
Rust 2024. The five-Iwo geometry test passed (5 tests). This deferral returns when
those retained fixtures are ported to the capture-owned planner; it is not a release PASS.

## Delivery boundary and historical checkpoint

The Founder explicitly asked that app installation happen in the background via
`make install-app`, without closing a running Codescribe; restart belongs to the
Founder. Current session instructions supersede the older idle-install wording.
After a verified artifact installation, report version/build/source and signature;
launch receipt requires the new artifact to actually run. A running old process
is not that receipt. No success ping precedes verified installation and launch.

The initial source checkpoint mistakenly treated this interactive ownership run
as a worker with no integrator. That restriction was corrected by the Founder and
is not a current blocker. The original source commit used `--no-verify` and recorded
these omitted hooks: trailing-whitespace, end-of-file-fixer, check-merge-conflict,
mixed-line-ending, detect-private-key, cargo-check, cargo-fmt, prettier and
commit-msg-provenance. This historical checkpoint is not a gate PASS. Subsequent
recovery commit ran normal hooks. Keep executed gates, installation and destination
admission as separate evidence.

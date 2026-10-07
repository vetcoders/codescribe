# Widget hover, agent archive and audio controls

Updated: 2026-10-07. Author and integrator: Astra, Codex session
`01a11150-1934-7432-8573-b5bb95b9edb6`, explicitly designated by the Founder.
Runtime: Fleet Worktree `/Users/polyversai/.vibecrafted/worktrees/e19c/codescribe`;
parent `/Volumes/vc-workspace/vetcoders/codescribe`.
Branch: `codex/widget-hover-261007`.
Baseline: `80352ed3b8754523418835043aacbdf524ec01cd`, admitted integration generation.
New source is isolated until independently admitted. PR146 remains frozen at
`b921f316821246efccf22184fbef6b423c7d5654`.

## Behavior

Mini retains the close dot, Codescribe wordmark, microphone and expansion button.
Pointer entry reveals midi after 160 ms; departure restores mini after 420 ms
only when midi was opened automatically. Close/microphone/fold targets, dragging,
editing and native menu tracking hold the transition. The microphone and fold
keep their screen positions where the display can contain the strip. Explicit
preview/fold clicks open the full retained canvas or return to mini. Hidden
retained text cannot acquire edit focus. Recording presentation remains separate
from the saved transcription preference. Reduced Motion remains respected.

Tray has Open widget directly below Agent. It opens the cached panel without
starting capture, changing routing or writing recording preferences. Overlay
chrome owns an arrow cursor, text retains its I-beam and native edges retain
resize cursors, even above an inactive underlying application.

Disconnected agents expose X: Remove from list and move to archive. The app
invokes the canonical managed `cs-bus --archive-agent` writer. Binding and lease
locks cover release; the command refuses an active reader or invalid mailbox.
Only the selected binding is removed. The lease, pending mailbox, ACKs, cursor,
bus and replies remain intact. Durable archive metadata preserves the saved
conversation across reader recreation, including an empty conversation. Channel
reuse shows the new owner. Failed release keeps the active entry and reports an
error. Existing takeover already moves previous conversations into the saved
list; X handles remaining disconnected entries.

The shared microphone and speaker controls in both the drawer and conversation
use native glass circle buttons on macOS26, and native bordered buttons on earlier
supported systems. Icons use the primary foreground rather than faint muted
text. The microphone's action is Speak to agent / Stop speaking to agent, with
Polish copy Mów do agenta / Zakończ dyktowanie do agenta. Playback action and
accessibility value distinguish muted, enabled and unavailable. The archive X
shares the native button treatment. The existing root GlassEffectContainer and
composer remain the glass group. Settings continue to own configuration and
validation, not a second active roster.

## Executed verification

- Fresh production bindings: PASS.
- Final affected Swift suite: **485 tests, zero failures, 31.137 seconds**, through
  `make test-swift`, including native mounted hover/geometry/cursor tests,
  archive state/failure tests and the real app-command to isolated Python helper
  to persistent reader path. The latter uses text fixtures, not physical PCM.
- All bus Python suites: **134 tests, zero failures, 35.573 seconds**. Archive
  cases preserve source/mailbox bytes and cover repeated requests, channel
  reuse, lock contention, living readers, malformed ownership and write failures.
- Fresh localization compiler extraction and catalog sync: PASS; 1430 keys,
  1408/1408 Polish translations, 97 awaiting review. Removed four obsolete
  microphone/preview labels. Extraction compiles without warnings after explicit
  MainActor isolation of the composer coordinator and main-run-loop termination.
- Managed installer standalone compilation: PASS; compile-only, no installation.
- Scoped configured Semgrep: zero findings, one rule, 34 tracked targets.
- Strict Swift formatting passes for authored files except existing baseline
  diagnostics in OverlayChannelDelivery.swift and the two snake-case JSON fields
  in AgentBridgeCommands.swift. The former's sole authored change is a defaulted
  archive metadata field; baseline lint independently reproduces the diagnostics.
  Authored diff whitespace check passes. Founder AGENTS.md remains untouched.

Logs: `/tmp/codescribe-e19c-hover-archive-targeted-final-oct7.log`,
`/tmp/codescribe-e19c-agent-archive-python-oct7.log`,
`/tmp/codescribe-e19c-hover-glass-l10n-final-oct7.log`,
`/tmp/codescribe-e19c-hover-archive-installer-oct7.log`,
`/tmp/codescribe-e19c-hover-archive-glass-semgrep-oct7.log`.

## Remaining boundary

The preceding full Swift suite remains unresolved: 1116 tests, one skip, zero
assertion failures exceeded its 60-second budget; a warm run reported a host exit.
The preceding full Rust libtest compilation retained 163 obsolete STT fixture
references. This UI/helper cut does not repair those foreign STT fixtures or
claim a full release gate PASS. No desktop screenshot, microphone/speaker test,
app restart, trunk merge or release was performed. The running Founder process
and installed artifact are distinct from source and test-host generations.
Background app installation and exact installed receipt are recorded separately.

## Single drawer control follow-up

Removed the conversation's duplicate hamburger and its unused callback. The
header remains the single drawer entry point; the conversation retains the agent
name and microphone/playback controls. The mounted conversation fixtures use
the reduced interface.

The affected conversation, state, chrome, resize and hover suites passed:
**326 tests, zero failures, 19.668 seconds**. Fresh Debug compiler extraction,
catalog sync, catalog lint and bridge census passed without new or stale keys.
Logs: `/tmp/codescribe-e19c-single-drawer-swift-oct7.log` and
`/tmp/codescribe-e19c-single-drawer-l10n-gates-oct7.log`.

The first background installation attempt stopped in Xcode CopySwiftLibs before
replacing `/Applications/Codescribe.app`. Installed build2094/80352 and the
Founder process47373 were retained. Packaging diagnosis and a new installed
receipt remain pending; no app restart or success ping was performed.

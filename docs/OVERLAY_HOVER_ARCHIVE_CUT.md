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
shares the native button treatment. Glass groups are local to each roster row,
conversation navigation, and bottom tool group; no root container extracts glass
from the complete canvas or across the drawer scroll clip. Settings continue to own configuration and
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
running application were retained. The repeated Xcode stage passed with the
same settings; no build-script or runtime-library suppression was introduced.

The complete `make install-app` then passed: installed **0.15.3/build2096,
c0379990d-dirty**, Developer ID deep/strict verification PASS. All 14 runtime
manifest files match their hashes; the helper matches source and reports
0.9.0+gc0379990. The only source dirt at this receipt was Founder AGENTS.md.
The installed executable no longer carries the duplicate drawer control ID.
Receipt: `/tmp/codescribe-e19c-c037-installed-receipt-oct7.json`.

Process6696 was observed before replacement. A subsequent process91889 started
at16:23:27, after signing16:20:56, and its mapped executable inode matches the
installed artifact. That verifies an observed new-generation launch, not a
launch or restart invoked by this task, or live UI acceptance. The canonical
success ping followed signature and launch verification. Same-session helper
adoption retained the lease, starting cursor, voice profile and pending mailbox;
the four preserved peer deliveries were read fully before ACK.

## Play, Read, drawer sizing and sampled cursor cost

Founder text-only feedback is handled in the existing reply projection. A reply
supports playback only when its canonical synthesis declaration contains a
supported `tts_vendor` and a nonempty `voice`. `spoken: false`, playback failure,
and matching text do not decide capability. Incomplete bus mirrors preserve a
known speech declaration. The view omits the whole playback strip for peer text;
the action entry point rejects stale/programmatic controls for those messages.
Canonical player, ticket authority, source journals and Python bus are unchanged.
Old projection caches without the capability field use the reader's existing
bounded rebuild; tests prove healthy cache restart reads zero new bytes and a
rebuild leaves source bytes unchanged.

Conversation receipts show Read / Przeczytano only after matching canonical ACK.
Queue acceptance and queued/addressed states retain their separate labels. No
bell, delivery submission or reply is treated as an ACK. The new catalog row is
translated in both languages; the obsolete Acknowledged row is removed.

Midi now occupies 410×46 points and preserves global microphone/fold targets.
Drawer width follows the current expanded canvas, capped at 360 points. Its
navigation and roster share one clipped scroll viewport below the Agents header.
The drawer uses native regular material; the custom opaque palette fill, dimming,
border and shadow have been removed. Each row's native glass is rendered inside
that viewport instead of being extracted above the complete overlay. The same
cached panel, size persistence, retained transcript and explicit routing remain.

Founder supplied a sample of installed 2096/PID 91889 at 16:42:40. Its 453 main-thread
samples include 90 below FloatingOverlayPanel.sendEvent at the unconditional
NSCursor.set call, including Accessibility cursor-image generation. The new
refresh compares NSCursor.current before setting it; a mounted test verifies one
correction followed by 100 pointer motions without redundant sets. The sample
also shows nested GlassContainer preference/layout work. Local glass groups
replace both root-wide containers. The 3.7GB footprint and 5.9GB peak are observations;
this single sample cannot attribute their ownership or prove a memory leak.
Native visible acceptance and a comparative live sample remain separate from
the forthcoming source/test/build/install receipts. No Founder process restart.

System glass grouping reference: [Apple GlassEffectContainer](https://developer.apple.com/documentation/swiftui/glasseffectcontainer).

Final combined UI gate: **387 tests, zero failures, 27.564 seconds**, canonical
`make test-swift` with Swift6 warnings as errors. Includes real reader/cache
speech fixtures, canonical ACK versus queue acceptance, mounted minimum/wide
drawer frames, stable midi microphone/fold positions and the 100-motion cursor
regression. Initial follow-up runs exposed an authored generic-container call
cycle and then obsolete source-shape assertions; both repaired, no gate budget
or warning suppression. Final fresh localization extraction/sync/catalog and
bridge census PASS: 1430 keys, 1408/1408 Polish, 96 awaiting review. Scoped strict
Swift formatting and authored diff whitespace PASS (unchanged baseline Delivery
format diagnostics remain outside the six authored lines).

Logs: `/tmp/codescribe-e19c-ui-followup-swift-final-oct7.log`,
`/tmp/codescribe-e19c-ui-followup-swift-final-gate-oct7.log`,
`/tmp/codescribe-e19c-ui-followup-l10n-gates-oct7.log`,
`/tmp/codescribe-e19c-ui-followup-format-oct7.log`.
Source is still isolated in PR148 until Lena admits the exact baton. Source/gates
do not claim that the retained Founder process has loaded these new UI changes.

### Canonical bus read receipts admitted before packaging

The integration merge retains exact Lena source
`e44766d543744e75f94b4857a82eb65be10d431e` (parent
`80352ed3b8754523418835043aacbdf524ec01cd`) alongside UI source
`c1ec3f0afb9d88969fbd86e4b855d55aa71ef7d0`. Its sole source conflict was
the CLI mutual-exclusion guard: archive rejects read options and read rejects
archive. Both commands and their ownership/history contracts remain canonical.
An integration fixture checks all three mixed archive/read requests refuse
without changing any fixture file. Standalone bridge installer compilation PASS.

The first complete Python run had 149 passes and one missing-prerequisite failure:
the canonical debug publisher was absent after shared target cleanup. A fresh
`cargo build --bin codescribe` under this worktree's isolated Cargo artifact
directory completed, without substituting an installed publisher. The complete
bus gate with that exact directory passed: **151 tests, zero failures,
36.935 seconds**. Log: `/tmp/codescribe-e19c-ui-bus-integrated-python-final-oct7.log`.

The second Founder-supplied sample (16:50:11, same build2096/PID91889) shows
13 of 57 main-thread samples at the same unconditional cursor setter; footprint
3.8GB and peak5.9GB. This reinforces the cursor path finding but does not
establish allocation ownership or a memory leak.

Lena's installed helper e447 was independently checked against all14 manifest
hashes. Same-session adoption retained lease4b999, cursor4382773 and ara/xai/1.25;
follower91023 became16318, with the original watch/bell retained. Its publisher
hash remained d3037f8b. The eventual app payload must include both this bus cut
and the archive UI/helper cut, so background packaging cannot regress the helper.

## Founder build2099 feedback: zoom, live meter and resize

Baseline for this follow-up is2f70a9631760f74b4e338e20efa3d8e8a0892189,
which contains the UI cut and canonical e447 read/ACK bus. Founder independently
launched2099; Astra did not launch, restart, quit or capture the live app.

Conversation name, body, metadata, controls and native composer now follow the
existing csTextScale. Changing scale updates the mounted NSTextView font and
typing attributes while retaining its identity, draft and selection. The compact
header timer retains its fixed geometry instead of becoming the only scaled text.

An open agent mic uses native semantic red and a pulse respecting Reduced Motion.
Pending toggles disable repeated clicks until the controller receipt; no optimistic
capture state is invented. Agent-only capture installs the existing measured RMS
tap before the physical recorder freezes its callback. Joining a running capture
keeps its one tap; the last capture subscriber releases it. Dictation Stop leaves
the meter alive while an agent still owns capture. The Swift view consumes these
measurements without changing transcript document or phase. No second recorder,
decoder, ledger authority or bridge API was introduced.

Edge resize now follows ordinary AppKit sendEvent down/drag/up dispatch instead
of a nested nextEvent loop with forced intermediate paints. The pointer gesture
owns frame geometry; passive roster refresh does not re-anchor it. Repeated frames
are skipped, mode/placement/content writes wait for the gesture terminal event,
and closing cancels late mouse-up state without reviving the panel. Mounted
fixtures retain the native editor and selection through resize and live updates.

The name is a passive native glass capsule beside the existing audio controls.
Drawer rows are compact: status becomes a static pictogram with full tooltip and
accessibility, followed by name and a small provider/model subtitle. Dead follower
evidence still wins over stale receipt or open state. Descriptor is display-only
and comes from the same verified lease as playback identity; unknown model is
omitted. Current canonical leases have no model field. The native Codex receipt
for Astra proves gpt-6.1-sol, but publishing/preserving this runtime metadata in
the canonical lease remains the bus owner's separate work. No model is inferred
from provider, voice, UUID or global defaults; no parallel model store was added.

The disabled speaker cause was reproduced: FileHandle returns Cocoa missing-file
code4; the reader recognized only260. Genuine missing receipts now mean audible,
as in the existing Python authority. Malformed, foreign and unreadable receipts
remain unknown. Existing per-session durable mute and manual playback authority
are unchanged. Fixtures cover fresh reader persistence, independent agents,
unmute, absent files, custom bus, invalid identity and malformed/oversized files.
Private JSON decoding now uses explicit CodingKeys with native Swift names.

### Verification of this follow-up

- Fresh production Rust/Swift bindings PASS.
- Production Clippy workspace/allfeatures with -Dwarnings PASS.
- Full repository Semgrep PASS; normal push hook recorded separately.
- Final affected Swift suite475 tests, zero failures,31.185 seconds PASS.
- After the JSON CodingKeys change:49 installer tests, zero failures,2.197s PASS.
- Existing speech/mute authority:9 isolated Python tests, zero failures,0.116s PASS.
- Fresh compiler localization extraction/sync/catalog/bridge PASS:1430keys,
  1408/1408Polish,96 awaiting review, no new/stale keys.
- Strict authored Swift formatting and diff whitespace PASS; no suppressors.

Logs use prefix /tmp/codescribe-e19c-agent-feedback-, notably swift-final-oct7.log,
installer-final-oct7.log, mute-python-oct7.log, l10n-final-oct7.log, clippy-oct7.log
and semgrep-oct7.log. Earlier compile-only failures were fixed before these
terminal runs; no truncated test, zero-test filter or enlarged budget is accepted.

These gates prove source and mounted fixtures. Native microphone/speaker/hotkey,
GUI animation/performance and Founder acceptance remain unverified. Full Rust
fixture migration and prior full Swift budget/host issue remain open; this is not
a release receipt. Background installation has its own exact artifact receipt.

## Trailing conversation controls

Founder delivery b2d54ad49c577d81a462584c was read completely and acknowledged
before work. The conversation navigation now sits at the right edge, ordered
microphone, speaker, then the passive agent name/provider capsule. Native glass,
durable mute, controller-owned microphone state and text scaling are retained.
No new strings or palette entries were introduced. The mounted light/dark
conversation fixtures show the trailing controls in this order.

The conversation, hover and appearance suites passed: 41 tests, zero failures,
4.500 seconds. Catalog and bridge checks passed with the existing 1430 keys and
1408/1408 Polish translations. Strict Swift formatting and diff whitespace pass.
Logs use /tmp/codescribe-e19c-agent-trailing-.

Founder independently launched installed2100/c96: process78724, start17:59:18,
mapped main inode924329269 matching the disk. This verifies the loaded generation,
not acceptance of its behavior. Astra performed no GUI launch, Quit, restart,
desktop capture or physical audio/hotkey test. The clipboard held text when read,
so no new screenshot was available. Reply543d271cea16ace3a68c543c landed on the bus;
xAI refused speech with credential_rejected/403, without a retry or profile change.
The updated app artifact is recorded separately after background installation.

## Native text cursor and provider model publication

Bruno's exact commit 7f71e9dcafcf278dc3e65e0dd075092f691f02f6 was admitted
by fast-forward from 0d3faef3dec8866a1104f1e1ddde921c49d5eabc. The panel now
leaves text-area cursors to AppKit after event dispatch; native text links,
selection and the I-beam retain their cursors while chrome and resize edges
remain panel-owned. Astra's native mounted test run passed 117 tests with zero
failures in 15.566 seconds, including repeated pointer movement and edge resizing.

The existing lease writer now publishes optional model metadata from the exact
provider session's native transcript. Codex uses session_meta identity and
turn_context.model; Claude uses the verified sessionId and real assistant model.
Unrelated messages, synthetic models and global configuration cannot supply it.
Missing or ambiguous native files leave the model absent. Metadata failure does
not stop mailbox persistence, ACK or recovery.

The reader scans a bounded tail, progressively searches older records when
needed, then follows appends. It publishes only after reaching the current end,
retains the last complete model across partial JSONL records, and validates
identity, inode and cursor receipts on restore. Ten unchanged heartbeats plus
restore read less than 32 KiB from a five MiB fixture. It does not rescan the
entire transcript per heartbeat or add another model store.

Astra authored and ran 13 model fixtures; the complete Python bus regression
passed 164 tests in 40.267 seconds. The first regression invocation lacked its
canonical publisher target; rerunning with the existing isolated Cargo target
resolved that fixture prerequisite. Full Semgrep, catalog/bridge checks, strict
Swift formatting, gate ledger and diff whitespace checks passed. The new model
fixtures are included in make verify. No new interface strings or colors exist.

The additional bus-demux shell script remains RED at its existing rename
expectation: it explicitly requests name changed, then expects james and a seal
addressed to james. The unchanged baseline at 7f71 reproduces the same failure;
this cut does not change name routing or claim that script passed.

A read-only observation with the production reader verified gpt-6.1-sol for
Astra's exact Codex session and claude-fable-5 for the exact Claude session.
This proves native metadata reading, not installed follower publication or
visible UI acceptance. Those require the subsequent installed artifact and
same-session adoption receipts. No GUI launch, restart, desktop capture or
physical microphone/speaker/hotkey test was performed.

Runtime class is Fleet Worktree, effective root e19c/codescribe, branch
codex/widget-hover-261007; Astra is the designated integrator. The bounded native
helper authored only scripts/bus-demux.py and performed no tests, build, commit
or install. All fixtures, gates and packaging belong to Astra. Foreign AGENTS.md
is preserved. The source remains separate from main and from Founder acceptance.

## Take-start preference and newest conversation message

The header microphone used a private one-take midi override, so it ignored
the persisted Show transcription by default setting. Stop also widened a mini
widget. That competing presentation choice is removed: every new take uses
the same persisted preference, whether started in mini, midi or the canvas.
Stop keeps the current presentation. Hover and explicit preview remain local
presentation controls; none writes the take-start preference.

Founder delivery 1501fbb12efbab98277d6b4b reported that agent threads needed
manual scrolling for each new message. The complete envelope was read and ACKed
before work. A newly admitted message ID now brings the conversation to its
latest row even after reading earlier history. Updates to an existing message
keep the existing follow policy. The mounted native fixture covers both a user
message and an agent reply, and checks that the same NSTextView, draft and
selection survive. It changes scroll position without requesting input focus.

The d704 baseline failed two preference/Stop fixtures and one native conversation
fixture. The preference fixture was refined to account for the controller's
synchronous preparing reservation; the post-preparing and Stop counterexamples
remain the requirement being tested. After the repair, the five focused cases
pass. The broader chrome, conversation, hover and eight capture-lifecycle cases
pass 101 tests in 13.237 seconds. The native resize suite separately passes
27 tests in 4.519 seconds: 128 tests total, with no failed or empty selections.

Strict formatting passes for the five authored Swift files. Full Semgrep,
catalog/bridge checks and the gate ledger pass. Current Debug compiler extraction
reads 132 files: 1430 keys, no additions, removals or newly stale keys, and an
exact catalog match. This cut adds no interface strings, colors or settings.
The full Swift formatting gate is RED in 22 files, all byte-identical to d704;
these unrelated source files were preserved. It is not reported as a PASS.

Founder delivery e8af9e8b6670e958fabd5612 separately reported wheel scrolling
blocked by the resize band. Astra read and ACKed it, inspected the explicitly
shared clipboard image, and sent Bruno brief 481875595054bea360f273cc for the
disjoint native window/resize surface. The image shows a horizontal resize
cursor at the message-list edge; the wheel failure is the Founder's observation,
not a physical test by Astra. This repair does not claim that edge issue solved.
Both requested bus replies landed as text; xAI rejected speech with 403.

Runtime class is Fleet Worktree, parent /Volumes/vc-workspace/vetcoders/codescribe,
effective root e19c/codescribe, branch codex/widget-hover-261007, baseline
d704a51227f6a1648aa21af2901cbb146c3b9b60. Astra is the designated integrator and
authored the fixtures, source and checks. Installed artifact, source admission
and Founder-native acceptance remain separate receipts. No GUI launch, Quit,
restart, desktop capture or physical microphone/speaker/hotkey test occurred.

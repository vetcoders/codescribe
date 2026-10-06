# Overlay agents and mini widget — source checkpoint

Date: 2026-10-06
Role: worker, source author in the interactive Founder session.
Runtime: Fleet Worktree, current checkout; no relocation or delegation.
Root: `/Users/polyversai/.vibecrafted/worktrees/e19c/codescribe`.
Baseline: detached HEAD, `d9e4233153cac7cd3dd6c4e45e6ee4d1e1c11d79`, clean.
Integrator: **not designated in this session**. The worker does not assume that role.
Phase: W1 source checkpoint, operational worker embargo (grade B).
The repository hook guard exists, but no phase marker is active. Its wrappers
do not control pre-commit's hook-environment bootstrap. No execution sandbox is
claimed.
Integration disposition: still isolated; no destination admission is claimed.

## Founder direction and implemented shape

The 6 October request replaces the verbose recording-channel list with an
embedded agent sidebar, gives each agent independent microphone and speaker
controls in the sidebar and conversation, unifies header controls, moves elapsed
time next to the waveform, increases waveform height, enlarges the close dot,
and folds the panel toward the upper-right into a small floating widget.

- The header and conversation hamburger open the same sidebar. Agent names,
  concise receipt/listening state, unread counts and two circular controls are
  visible. Saved conversations stay behind a disclosure. Dictation is available
  from the header microphone and its context menu, outside the agent roster.
- At widths of at least 640 points the sidebar reserves 280 points. Smaller
  windows use a drawer over the canvas and suspend interaction with covered
  content. Opening the sidebar releases the current text responder. Selection,
  drafts and bus messages remain in their existing owners.
- The close dot grows from 7 to 9 points, including hover. The compact waveform
  track and bars double their maximum height. Position, agents, microphone and
  fold controls share circular chrome. The timer immediately follows waveform.
- Mini is 180 × 46 points. It keeps the waveform, close dot and expand control;
  agents and microphone appear on hover. The microphone/Stop remains visible
  during recording. Expansion restores the saved full size and clamps it to the
  screen; persistence uses the expanded size. The panel keeps its existing
  nonactivating, floating, all-Spaces behavior.

## Playback ownership

`bus-demux.py` remains the speech owner and the sole writer of playback mute
receipts. `AgentBridgeCommands.swift` invokes that installed managed command
and resolves the actual bus from each roster session's lease, including custom
bus paths, before reading its bounded, immutable mute receipts off the UI thread. `OverlayState`
projects them into both control surfaces. No new recorder, document reducer,
or bus writer is introduced; PCM identity and text projection are untouched.

The new commands are `cs-bus --mute-agent` and `--unmute-agent`, with explicit
`--provider`, `--session`, `--bus` and optional `--bridge-home`. The command
validates the existing provider/session lease and its bus before writing.

The control receipt is `codescribe.agent-playback-mute.v1` at
`<bridge-home>/runtime/playback-mutes/<key>.json`, privately and atomically
written by the existing `atomic_json` owner. Its fields are schema, provider,
provider_session_id, lease_id, bus, muted and emitted_at. The filename is the
first 12 SHA-256 bytes of the UTF-8 NUL-joined provider, session and resolved
absolute bus path. Provider identifiers come from the existing lowercase vendor
registry. This is session playback control state, independent of microphone
`loud` and the immutable settings snapshot.

Mute lasts until explicitly cleared, including an app restart. Another session
of an agent with the same display name has its own state. An unreadable receipt
prevents automatic speech and shows unavailable status. A delayed UI poll cannot
overwrite a click's newer receipt.

Automatic `cs-say` publishes durable reply text and source coordinates first.
A muted reply then finishes with playback state `refused`, reason `muted`,
spoken false, and successful command exit. It makes no TTS request when already
muted. Muting while queued or playing is observed by the existing playback poll;
only that automatic player stops. Capture and agent work continue. Unmuting
affects future replies and does not replay the backlog. Explicit Play on a
retained reply is a deliberate audition and remains available while automatic
playback is muted.

Polish copy for new controls/status is in the existing String Catalog as
`needs_review`. Compiler extraction and catalog synchronization are integrator
gates; the worker did not run them.

## Source review and execution boundary

Loctree project atlas, scoped overlay context, slices and window impact were
read. The String Catalog is outside Loctree's indexed language coverage; its
keys were inspected directly and feedback was appended to the central log.
AICX supplied historical intent only; the current request determines the cut.

The worker's admitted checks are source inspection, Loctree, literal/reference
censuses, bounded static security scan and `git diff --check`. Source review
covered exact playback ownership, publication before mute, session/bus isolation,
manual replay, unknown status, poll/click ordering, saved microphone ownership,
responder release, covered-canvas interaction and full/mini sizing paths.

The final local Semgrep scan ran one generic secrets rule over the Python speech
owner, overlay directory, AppModel and Swift command surface, with 34 tracked
targets, full reported parsing and zero findings. This does not
certify Python behavior or Swift concurrency/type correctness.

Tests/fixtures, compilation, formatter execution, benchmarks, models, product
execution, audio/desktop probes and installation are **NOT_ASSESSED** under
`docs/COMPILE_EMBARGO.md` §0. Five physical Iwo occurrences through PCM, ledger,
reducer and delivery are also **NOT_ASSESSED**. Nothing in this source checkpoint
claims that capture falsifier passed.

This local W1 checkpoint uses `--no-verify` under §5.1: the command guard cannot
defer pre-commit's environment bootstrap, including formatter dependency
installation. That execution is outside the worker's authority. Every bypassed
hook is recorded here: trailing-whitespace, end-of-file-fixer, check-merge-conflict,
mixed-line-ending, detect-private-key, cargo-check, cargo-fmt, prettier,
commit-msg-provenance. Pre-push gates cargo-clippy and semgrep are not invoked;
no push is planned from detached HEAD. The checkpoint certifies preservation,
not admission or execution success.

## Integrator acceptance contracts

The designated integrator authors fixtures/tests and runs gates on the admitted
generation. Required contracts include:

1. Two agents sharing a name, differing sessions, differing bus paths and
   renamed agents: only the exact selected identity mutes. Wrong/missing lease,
   malformed receipt, invalid boolean, oversized receipt, contradictory CLI
   commands and unreadable state never authorize playback.
2. Muted `cs-say`: text and reply source persist; no synthesis/player starts;
   playback reason is muted; exit is successful. Queued/active automatic speech
   stops on mute. Explicit replay, unmute and another session keep working.
3. Sidebar and conversation speakers paint one receipt. Repeat clicks are
   serialized; delayed polls cannot revert the current choice. Saved or
   reassigned conversations cannot toggle another agent's microphone.
4. Sidebar toggling preserves selected conversation/drafts and capture. Narrow
   drawers block covered canvas input; opening releases the text responder;
   unread replies are not marked viewed behind an open sidebar.
5. Full/mini transitions at narrow, wide, bottom, right, custom and multi-screen
   positions preserve 180 × 46 mini size, full size persistence, visible
   expansion and floating behavior. Hover, Stop during capture, drag, keyboard
   navigation, VoiceOver, reduced motion/transparency and Polish copy must work.
6. The existing five-Iwo PCM → ledger → reducer → delivery falsifier and capture
   lifecycle gates remain required, as does installed-product acceptance.

Run `make check`, `make verify`, `make test-swift` and the applicable Python
speech-owner suite. Rebuild and run `make l10n-sync`/catalog gates with fresh
extraction. Inspect receipt labels for intentionally muted speech. Then perform
the repository's `make install-if-idle`, without ending a take or agent turn;
verify installed version/build/commit, signature and launch. Obtain explicit
authorization before new screenshot or audio capture/probes. The success ping
belongs only after verified installation and launch.

## Definition of Undone for this cut

| Surface | State |
| --- | --- |
| Source and Polish draft copy | Authored; static review only |
| Test and compiler verdict | NOT_ASSESSED; integrator required |
| Catalog extraction synchronization | NOT_ASSESSED; integrator required |
| Installed app, UI, microphone and sound acceptance | NOT_ASSESSED |
| Destination integration | Still isolated |
| Distribution/release/public claims | No release performed |

No sales/readiness claim follows from this source checkpoint. The next required
handoff is designation of the integrator and admission of this exact cut.

# Release drift journal

Status: append-only.

This journal records each release-readiness mutation before it is made. Entries
state what will change, where, why the current state drifts, and what the change
is meant to achieve. The journal is evidence of the cut; runtime contracts and
executable checks remain authoritative.

## 2026-08-22 — release hygiene baseline

- **What:** Rebuild the public release narrative for `0.13.3 -> 0.14.0 -> 0.14.1`, separate published-release truth from daily-build truth, and retain
  the unresolved next steps.
- **Where:** `CHANGELOG.md`, `README.md`, release-readiness documentation, and
  the core STT/transcript contracts that currently contradict the running
  branch.
- **Why:** GitHub still publishes `v0.13.3` as Latest while the repository is at
  `0.14.1`; several documents describe earlier Apple/Whisper authority,
  stop-pass, settings, and release states as if they were current.
- **Purpose:** Give operators, reviewers, and future agents one honest account
  of what landed, what changed during the repair, and what still blocks a
  public `0.14.1` release.

## 2026-08-22 — public/private boundary cut

- **What:** Remove the private source-extraction journal after transferring its
  durable engineering decisions; replace personal names, local absolute paths,
  and private-machine prose where the text is non-functional; classify every
  functional or ambiguous identifier instead of rewriting it blindly.
- **Where:** `docs/KORA_CODESCRIBE_JOURNAL.md`, public documentation, examples,
  tests, scripts, and the release-decision ledger.
- **Why:** The repository currently contains a private download path, personal
  operator labels, and host-specific wording. Some similarly named findings
  are load-bearing runtime identifiers and therefore cannot be safely scrubbed
  by search-and-replace.
- **Purpose:** Make the public tree reproducible and non-personal without
  breaking endpoints, signing, CI, model resolution, or deployment paths.

## 2026-08-22 — PR #82 review part 1 disposition

- **What:** Re-test every part-1 finding against the current branch and current
  PR tip, fix only findings owned by this release cut, and record unresolved or
  branch-divergent items with explicit release severity.
- **Where:** The affected runtime owners, tests, documentation, and a durable
  release-readiness report that can accept review part 2.
- **Why:** The supplied review predates the current PR tip and this branch has
  diverged into a separate acoustic-identity implementation. Historical green
  checks and historical findings are neither completion nor current failure.
- **Purpose:** Preserve review continuity without projecting stale evidence or
  hiding release blockers behind documentation work.

## 2026-08-22 — neutral public prose, batch 1

- **What:** Remove personal operator labels from policy prose, field-evidence
  comments, and synthetic text fixtures; delete the stale agent-coordination
  note.
- **Where:** `AGENTS.md`, `app/controller/tests.rs`,
  `app/presentation/emitter.rs`, `core/examples/lexicon_gate_calibration.rs`,
  `core/stt/punctuation_transplant.rs`, `docs/THE_ENGINE_CONTRACT.md`,
  `docs/THE_ENGINE_ROADMAP.md`, `docs/TRANSCRIPT_LANES.md`, and
  `AGENT_BUS.md`.
- **Why:** These occurrences identify private people or a one-off operating
  situation but do not participate in runtime behavior. Loctree reports zero
  consumers for `AGENT_BUS.md`; keeping it would publish obsolete private
  coordination as repository law.
- **Purpose:** Preserve the tests and engineering evidence while making the
  public tree person-neutral and removing a dead process artifact.

## 2026-08-22 — PR #82 part 1 runtime blockers

- **What:** Upgrade `h2` to a patched release; roll back a Tokio runtime whose
  named workers fail to start; bound shutdown recording finalization; make the
  Layer 1 stop receipt consume independently measured abandoned work; and make
  the cancellation test wait for the task guard rather than racing it.
- **Where:** `Cargo.lock`, `bridge/src/application_runtime.rs`,
  `bridge/src/hotkeys.rs`, and
  `core/pipeline/streaming/apple_live_session.rs` with colocated tests.
- **Why:** Current evidence confirms RUSTSEC-2026-0258 at `h2 0.4.15`, a false
  running state after worker-start failure, an unbounded shutdown future, a
  receipt whose conservation assertion is true by construction, and PR #82 CI
  failing because cancellation observes the payload drop before the active-task
  guard has decremented its counter.
- **Purpose:** Remove the confirmed security/lifecycle release blockers and
  turn the review claims into falsifiable runtime checks rather than prose.

## 2026-08-22 — canonical release and operator docs

- **What:** Rebuild the `0.13.3 -> 0.14.0 -> 0.14.1` narrative, correct the
  public/source version split, describe the current four-layer span-bound
  pipeline, fix the DMG command contract, and mark the credential-topology ADR
  as accepted but not implemented.
- **Where:** `CHANGELOG.md`, `README.md`, `AGENTS.md`,
  `docs/INSTALLATION.md`, `docs/STT_CONTRACT.md`, and
  `docs/ADR/2026-08-14-PROVIDER_CREDENTIAL_TOPOLOGY.md`.
- **Why:** The current docs variously claim `v0.13.0` is Latest, Whisper owns
  the live preview, Apple text is an immutable floor, a manual three-command
  chain is the standard release, and the provider redesign has no verified
  implementation status. GitHub and runtime code contradict those claims.
- **Purpose:** Make the README/manual/ADR/changelog agree on what users can
  install, what the current source actually does, and which release work is
  still open.

## 2026-08-22 — PR #82 part 2 release blockers

- **What:** Repair the independently reproducible part-2 failures in human-edit
  provenance, bus-demux delivery, failed coverage cleanup, formatter session
  isolation, capture ownership, staging-path safety, and voice-thread routing;
  add fail-closed schema/cursor guards where they share those owners.
- **Where:** `macos/Codescribe/Screens/Overlay/OverlayState.swift`,
  `scripts/bus-demux.py`, `app/presentation/transcript_bus.rs`,
  `core/llm/inline_format.rs`, `bridge/src/hotkeys.rs`,
  `scripts/build-app.sh`, the Agent/Assistive capture owner, and their closest
  tests.
- **Why:** Review part 2 supplies current-code evidence for one red CI race and
  six P1 runtime failures. They can respectively mislabel machine output as a
  human edit, lose a command after a crash, leak uncovered words into Delivery,
  block a later session behind stale global state, latch the microphone after
  broadcast lag, replace the repository during staging, or send a recording to
  a thread selected after capture began.
- **Purpose:** Turn the combined review into release gates on the selected
  `dbxms-runtime-claude` candidate. The competing PR #82 acoustic runtime is not
  merged mechanically: it diverges after `a95e1272` and changes seven of the
  same owners, so its independent claims remain comparison evidence rather
  than a second authority in one binary.

## 2026-08-22 — release hardening from PR #82 P2 evidence

- **What:** Make the installed-agent attach sequence single-follower, align the
  Swift installer with the demux storage override and manifest permissions,
  reject reserved Layer 1 phases at runtime, correct Apple-only Settings copy,
  and bound/drain VAD boundary evidence.
- **Where:** `skills/codescribe/SKILL.md`,
  `macos/Codescribe/Services/AgentBridgeInstaller.swift`,
  `core/asr_session/bootstrap.rs`, `core/asr_session/recorder.rs`,
  `macos/Codescribe/Screens/Settings/EnginePanel.swift`,
  `macos/Codescribe/Screens/Settings/SettingsViewModel.swift`,
  `core/audio/chunker.rs`, `core/pipeline/streaming/session.rs`, and tests.
- **Why:** The documented second follower is refused by the lease lock; wizard
  and helper can otherwise install/read different roots; `phase2`–`phase4`
  currently run while Settings calls them degraded; Apple-only claims a live
  Whisper lane that is disarmed; and VAD sessions accumulate an unconsumed
  `VecDeque` for the length of a take.
- **Purpose:** Make setup, runtime disposition, operator copy, file modes, and
  long-session memory agree before the candidate is packaged.

## 2026-08-22 — fail-closed delivery and layer ordering

- **What:** Expire cached paste targets and stop relaunching them, refuse
  cross-utterance Layer 1 rewrites, serialize recording hotkey dispatch, and
  admit inline L3 only on a Responses-family wire. Correct the Agent-paste and
  Dictionary Teach prose to match the safe runtime boundary.
- **Where:** `app/os/selection.rs`,
  `core/pipeline/streaming/layer1_window.rs`, `bridge/src/hotkeys.rs`,
  `core/llm/ai_formatting.rs`, `CHANGELOG.md`, `docs/DELIVERY_ROUTE.md`,
  `docs/STT_CONTRACT.md`, and the closest tests/UI copy.
- **Why:** PR #82 part 2 proves four contradictions: a stale target can wake a
  closed application; one rewrite crossing two acoustic owners can be poured
  into the first; Down/Up tasks may overtake; and an Ollama/Anthropic lane can
  receive Responses JSON. Public prose also promises Agent-window Cmd+V even
  though the app-name latch deliberately filters Codescribe, while bulk Teach
  silently bypasses the automatic three-correction threshold.
- **Purpose:** Prefer an explicit fallback over invented ownership: no stale
  paste, no cross-owner mutation, ordered gestures, no wrong-wire provider
  calls, and no release claim stronger than the executable behavior.

## 2026-08-22 — PR #82 observability and contract cleanup

- **What:** Test the two unasserted Layer 1 warning codes, stop logging an
  absolute private bus path, and align bridge quality prose with the executable
  provenance/threshold law.
- **Where:** `core/pipeline/streaming/apple_live_session.rs`,
  `app/presentation/transcript_bus.rs`, and `bridge/src/quality.rs`.
- **Why:** Review part 2 found typed warnings with no event-level assertion, a
  privacy-sensitive absolute path in a warning log, and API documentation that
  reduced the actual `manual_human`/Teach rule to a formatting-level claim.
- **Purpose:** Make diagnostics stable enough to review without leaking a home
  path, and make the bridge describe the same learning law the core executes.

## 2026-08-22 — Silero sideband retention

- **What:** Bound the retained Silero sideband evidence to a fixed recent
  window while preserving monotonic sequence numbers and the events returned
  to live consumers.
- **Where:** `core/pipeline/streaming/silero_fusion.rs` and its unit tests.
- **Why:** The VAD boundary queue is now bounded, but the fusion ingress copied
  every boundary into a second session-long vector and scanned it for every
  sealed range. Long hands-free takes therefore still had unbounded memory and
  quadratic scan exposure even with fusion default OFF.
- **Purpose:** Make one-session VAD evidence physically bounded on every owner,
  not merely on the first queue in the pipeline.

## 2026-08-22 — active-name canonicalization hot path

- **What:** Canonicalize the full tail of active names to lowercase and cache
  their compiled whole-word regexes until the bounded name snapshot changes.
- **Where:** `core/stt/active_names.rs` and
  `core/pipeline/stream_postprocess.rs` with existing name/`piwo` tests.
- **Why:** `IWO` currently survives as `IWO` despite the fixture requiring
  `Iwo`, while up to sixteen regexes are rebuilt on every lexicon pass through
  the live transcript hot path.
- **Purpose:** Preserve one stable spoken-name spelling without rewriting
  substrings such as Polish `piwo`, and remove avoidable per-partial work.

## 2026-08-22 — exact paste-focus confirmation

- **What:** Remove the exception that treated Codescribe remaining frontmost
  as proof a foreign paste target had activated; update tests and release prose
  to require an exact target observation.
- **Where:** `app/controller/overlay_paste.rs`, `app/controller/tests.rs`,
  `CHANGELOG.md`, and `docs/DELIVERY_ROUTE.md`.
- **Why:** Native activation returning success proves only that the request was
  accepted. If the target never takes focus, treating our own overlay as a
  successful handoff can send Cmd+V into the wrong Codescribe window.
- **Purpose:** A false refusal parks recoverable text; a false positive can
  paste into an unintended caret. The release chooses the recoverable failure.

## 2026-08-22 — source-scoped sealed rewrite fence

- **What:** Apply the acoustic-final replay veto only to TailPatch mutations;
  keep other typed `ReplaceRange` producers visible in the live assembly.
- **Where:** `core/pipeline/streaming/live_assembly.rs` and its source-specific
  replay test.
- **Why:** The current wildcard match discards every post-final replacement on
  an acoustic slot, even though the safety rationale and reviewed ownership
  apply specifically to the Layer 1 tail patcher.
- **Purpose:** Keep one safety fence from silently becoming a global ban on
  future lexicon/human/provenance-owned mutations.

## 2026-08-22 — public identity and private-source cleanup

- **What:** Replace person-specific placeholder paths, emails, hook examples,
  transport labels, and installer prose with role-based public examples. Remove
  the private source-extraction journal only after its durable engine decisions
  and unresolved review work are restated in canonical contracts and the
  release-readiness record.
- **Where:** `app/os/onboarding.rs`, `core/examples/license_signer.rs`,
  `core/llm/account_auth/mod.rs`, `docs/ENV_REGISTRY.toml`,
  `scripts/commit-msg-provenance.sh`, `scripts/install-voice-lab.sh`,
  `scripts/tests/install-voice-lab-test.sh`, and
  `docs/KORA_CODESCRIBE_JOURNAL.md`.
- **Why:** The pre-public scanner found a real home-path blocker and several
  personal identifiers in runnable examples and operator-facing diagnostics.
  The Kora journal is useful evidence but is not a public contract: it embeds a
  private download path, person names, and stale point-in-time runtime claims.
- **Purpose:** Publish role- and product-level truth without exposing founder or
  workstation identity, while preserving the technical decisions and open
  risks future maintainers actually need.

## 2026-08-22 — installer verification diagnostics

- **What:** Emit a privacy-safe unified-log event when the bundled Agent bridge
  manifest or payload fails verification, and close the matching review item
  in the release-readiness matrix.
- **Where:** `macos/Codescribe/Services/AgentBridgeInstaller.swift` and
  `docs/releases/2026-08-22-v0.14.1-release-readiness.md`.
- **Why:** Returning only `.unavailable` makes a missing payload and a rejected
  checksum/symlink indistinguishable to support, while logging raw error text
  could expose local paths.
- **Purpose:** Preserve the fail-closed UI state and add a durable diagnostic
  category without publishing operator filesystem details.

## 2026-08-22 — release next-step truth

- **What:** Replace stale `v0.14.1` next steps that still list completed review
  work with the remaining branch, coverage, host-verification, UX, and
  publication gates.
- **Where:** `CHANGELOG.md` under `Next steps before public v0.14.1`.
- **Why:** Bus path parity, active-name normalization/cache, installer tamper
  diagnostics, and Silero retention were fixed after the section was written.
  Leaving them open would make the changelog a chronology of agent memory
  rather than release truth.
- **Purpose:** Let a reviewer distinguish completed stabilization from the few
  explicit deferrals and the still-unrun final release gates.

## 2026-08-22 — stale inline-ledger falsifier

- **What:** Update the old private-store seam test to expect a zero-overlap
  active ledger to yield to one-shot formatting instead of manufacturing a raw
  L2 failure result.
- **Where:** `core/llm/inline_format.rs` test module only.
- **Why:** Full `make verify` passed 1,445 core tests and exposed this single
  assertion as the pre-P1-09 behavior. The production function and the newer
  process-global-store test already agree that zero overlap proves a foreign
  capture.
- **Purpose:** Make the falsifier defend session isolation and the formatter
  fallback contract rather than lock in the reviewed data-loss bug.

## 2026-08-22 — assistive-session test isolation

- **What:** Serialize the start-failure reset test with the other tests that
  mutate the process-global assistive-session badge.
- **Where:** `app/controller/tests.rs` test attribute only.
- **Why:** A second full `make verify` failed one hold-down assertion that had
  passed in the first run and then passed 10/10 in isolation. Loctree showed
  the concurrent reset test was the only direct global `true` writer without a
  serial guard.
- **Purpose:** Remove a real inter-test race without weakening the assertion
  that raw Fn hold may never publish Assistive mode.

## 2026-08-22 — live reducer provenance reset

- **What:** Clear the one-shot human-edit quality latch whenever a new machine
  transcript event marks live transcript activity.
- **Where:** `macos/Codescribe/Screens/Overlay/OverlayState.swift`, centralized
  in `markTranscriptActivity()` for Preview, Correction, UtteranceFinal, and
  SessionFinalised producers.
- **Why:** The full Swift gate proved that `applyPreview` changes the overlay
  back to listening before formatted-text refresh runs. A prior manual edit
  could therefore be attributed to the later machine transcript even though
  the operator did not author those bytes.
- **Purpose:** Keep Voice Lab quality receipts truthful: `manual_human` belongs
  only to an unconsumed human edit, never to a later reducer-owned refresh.

## 2026-08-22 — final local gate evidence

- **What:** Replace pending review-gate labels with the exact local evidence
  produced by the selected candidate tree, while retaining HOLD for commit and
  distribution proof that has not happened yet.
- **Where:** `docs/releases/2026-08-22-v0.14.1-release-readiness.md`.
- **Why:** `make check`, `make verify`, `make test-swift`, `cargo audit`, and the
  deprivatization verification have now completed. Keeping them marked pending
  would be stale; calling the release ready before the exact commit is signed,
  notarized, stapled, and verified would be equally false.
- **Purpose:** Give the release operator one evidence ledger that distinguishes
  finished source gates from the remaining artifact/publication gates.

## 2026-08-22 — dead contract reference cleanup

- **What:** Remove the root tracking exception for deleted `AGENT_BUS.md` and
  redirect the macOS host-smoke safety comment to the active local contract.
- **Where:** `.gitignore` and `scripts/smoke-macos27.sh`.
- **Why:** A Loctree literal audit after staging the deletion found two stale
  references: one would keep inviting recreation of the dead document, and the
  other sent release operators to a file that no longer exists.
- **Purpose:** Complete the deletion as a repository-wide contract migration,
  while preserving the operator-owned private fixture ignore entries in the
  working tree and outside this commit.

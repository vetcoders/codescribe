# Relay acceptance tests

These tests are the executable acceptance criteria for the PCM arbitration
implementation, following the v5/v5.1 audit at `d48e2980`. The test-only change
starts from `63245014`; it does not repair production behavior.

Run from the repository:

```sh
python3 scripts/verify-relay-acceptance.py
```

The command builds both current library test executables through Cargo, then
runs the contract witnesses and their positive controls. It fails on a failed
assertion, a missing selector, an ignored selected test, an incomplete run, or
a build failure. All groups execute even if an earlier group fails. It does
not accept a known-failure count as success. No binary hashes are hardcoded.

The four W-0 falsifiers now participate in ordinary `cargo test` as well.
New tests also have no `ignore` or `should_panic`. Red results are intentional
until implementation satisfies the contract; they must not be hidden to make
the branch green.

## Recorded starting result

On `632450143206f40aaaa8233302abeede4f556593` plus this test-only change:
**108 tests executed, 99 passed, 9 failed, 0 ignored**. The runner exits 1.
The nine failures are four group-omission cases, early formatter dispatch,
terminal Smart replacing Raw, Max replacing Raw, and two operation replay cases.
Corrections/Whisper positive controls, ManualHuman protection, late-version
selection, disabled-layer settlement and five-Iwo delivery pass.

Workspace Clippy with `-D warnings`, Rust formatting, scoped Semgrep, environment
registry and gate-registry validation pass. This is a recorded red acceptance
baseline, not a claim that production behavior is fixed or that the full
workspace test suite passes.

## What must pass

| Contract                                                 | Executable witness                                                                                                                                                 | Positive control / what cannot count as a fix                                                                                                                                                                |
| -------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------ | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| N1: more words do not prove conservation                 | Four `relay_acceptance_*group_omission` tests in `slot_ops.rs`; each checks Whisper/CloudLive and coarse/pinned groups through whole-label or word-batch admission | Keep the existing source and an alternative on ambiguous omission; existing 1→1 correction and genuine insertion tests must also pass. Freezing Apple is not a fix.                                          |
| N2: expected Whisper is not an empty queue               | `relay_acceptance_hands_free_formatter_waits_for_expected_whisper` plus `relay_acceptance_formatter_receives_corrected_whisper_source`, for Correction and Smart   | After a real synthetic completion is admitted, exactly one formatter request must contain the corrected/enriched text and its source observation. Disabling the formatter cannot pass.                       |
| Disabled Whisper owes no phantom work                    | `relay_acceptance_disabled_whisper_does_not_leave_phantom_debt`                                                                                                    | Disabled and pending are different states; a blanket waiting rule cannot pass. Existing timeout/generation tests remain required.                                                                            |
| Live Smart is separate from Raw                          | `w0_falsifier_live_formatter_cannot_drop_a_word` now supplies an authenticated source                                                                              | The derived response must be accepted, with the expected text. Refusing a malformed request cannot make this test pass. Raw bytes, revision and slots remain unchanged.                                      |
| Composer/overlay terminal formatter is separate from Raw | `w0_falsifier_smart_cannot_replace_raw_projection`, through their common production entry                                                                          | Smart must remain selectable for delivery, refer to the original Raw revision, and retain its output. Merely rejecting or discarding Smart is insufficient.                                                  |
| Max is a derived answer                                  | `relay_acceptance_max_cannot_replace_raw_or_later_speech`                                                                                                          | The answer exists and preserves subsequent speech; Raw revision/text and original slots remain unchanged. Disabling Max cannot pass.                                                                         |
| Delivery choice stays fixed after late Smart             | Extended `smart_deadline_delivers_one_whole_corrections_version_and_keeps_raw`                                                                                     | A late successful version is really added, but selection stays whole-take Corrections and the Bus retains exactly one delivery receipt.                                                                      |
| Durable operations retain lineage                        | Two `relay_acceptance_trail_replays_*` tests                                                                                                                       | Real accepted acoustic correction / Dictionary merge → disk JSONL → production replay. Both slots and operation/source receipts must match; truncated trail is refused. Copying only final text cannot pass. |
| Acoustic deletion needs positive proof                   | Existing word-no-speech and slot-operation tests                                                                                                                   | Quiet speech, overlapping speech, wrong epoch, missing PCM, protected human edits and reinsertion are negative controls.                                                                                     |
| Physical repetitions survive                             | Slot tests plus production Bus `p0_b_five_iwo` witnesses                                                                                                           | Separate PCM occurrences stay separate through ledger, reducer and delivery. Equal labels cannot be deduplicated.                                                                                            |
| Raw keeps hesitations and admitted markers               | Light+ tests, including enabled W-0 falsifier                                                                                                                      | `yyy` and accepted event markers survive; this does not claim new event recognition.                                                                                                                         |
| Corrections does not lose ordinary words                 | Ordered-word guard, repeated-word and safe-floor tests                                                                                                             | Case/punctuation changes remain allowed; a guard which refuses everything cannot pass the positive controls.                                                                                                 |

The group-omission fixtures explicitly use one PCM group without child word
boundaries. For example, `czy plan weryfikowałeś` becoming
`czy weryfikowałeś dokładnie` is ambiguous: equal whitespace-token counts do not
account for the source word `plan`. The required outcome is preservation of
the current group and retention of the candidate. This is not a ban on
acoustic correction at a resolved word target. The suite includes the latter
as mandatory positive controls.

## Evidence boundaries

These tests exercise production ledger/reducer/scheduler/Bus code with owned
synthetic inputs and temporary directories. They do not open a microphone,
contact an LLM, install the app, or modify the Founder's settings. The group
fixtures are not evidence about the first cause of a historical recording.

The deadline-named test verifies the **choice and immutability of a handoff**.
It does not measure or simulate the actual deadline owner. It must not be
reported as proof of a two-second time bound.

Still required before complete performance/deadline acceptance:

- A production scheduling clock/deadline seam covering queue entry, execution
  and accepted Light+ commit. Deterministic tests must place queue/compute/commit
  on both sides of the limit, assert no partial publication, and assert lifecycle
  release. A `sleep`, a fast unit run or checking that a timeout constant exists
  would not prove this.
- A real Smart deadline receipt and a test advancing the scheduler clock across
  it, including late completion and user-visible status. The current tests
  establish post-selection behavior only.
- Job-bound scheduling/return/handoff timestamps and complete replay coverage
  for all mutation corridors, including deletion, split, seals and revisions.
  The two new operation tests are concrete minimum witnesses, not exhaustive
  trace certification.
- Identified installed artifacts and W-0b measurements on Dragon and MacBook:
  sustained dictation, Stop, repeated takes and idle; actual CPU/RSS/thermal
  limits and responsiveness. Unit tests cannot certify absence of heating.
- An authorized physical microphone-to-agent/paste acceptance run. A Bus
  receipt in a hermetic fixture is not proof of delivery in a running app.

Implementation may change private APIs used by these tests. Preserve their
behavioral assertions and positive controls when updating the fixture calls;
do not preserve the current wrong projection semantics just to keep older
tests green. In particular, older Max tests which expect a shared Raw rewrite
must be reconciled with the derived-projection contract during implementation.

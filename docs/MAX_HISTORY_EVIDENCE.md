# Max and live speech — bounded history evidence

Reviewed 2026-09-14 by Roman in Fleet Worktree `cut/roman-max-consultation`.
Source baseline for the code observations below: `36310d3fe`.
This is a partial history review, not completion of the all-conversations goal.

## Retrieval receipt

- Census: `/Users/maciejgad/.aicx/codescribe-session-census-20260914-roman.json`,
  1633 records. The census has no review-status field; record count is not a
  count of conversations read. It is an older snapshot, not a fresh host census.
- Canonical source reviewed:
  `/Users/maciejgad/.codex/sessions/2026/09/13/rollout-2026-09-13T21-34-33-01a09c43-a474-7643-ac9a-5a4436118197.jsonl`.
- Extract produced through installed AICX, one direct source parsed:
  `/Users/maciejgad/.aicx/codescribe-history-20260914/roman-review-01a09c43-a474-7643-ac9a-5a4436118197-user.md`.
  Read all 121 lines of the user-only conversation extraction. This does not
  constitute a review of assistant/tool evidence or of the entire raw session.
- AICX bare `codescribe` search refused an ambiguous project. Explicit searches
  across vetcoders/libraxis/loctree-repos/repos buckets returned zero results for
  the selected user-message queries, with freshness unverified. Direct source
  extraction recovered relevant instructions despite those empty search results.
  Do not infer absent intent from this search response.

## Founder requirements actually present in this source

1. Silero speech with stalled Apple text must be visible during the take.
2. At the speech boundary, outstanding text debt requests the whole acoustic
   occurrence from Whisper, not merely its tail.
3. Recovered speech replaces provisional presentation; failed recovery leaves
   an explicit gap, never a false claim of full coverage.
4. The overlay should collapse to a bar, retain the user's expanded preference,
   and expose the tool controls on hover rather than permanently.
5. The pointer widget should expose a short live transcript and avoid excessive
   resource use. The source mentions forthcoming performance reports; it does
   not itself contain those reports or prove a resource-use repair.
6. The named external Roman listener should wait for the logical end of speech
   and re-transcribe questionable input. This is addressed to Roman on the bus,
   not an explicit specification of a semantic classifier for in-app Max.

The latest direct conversation separately requires Max tools, clipboard access,
and consultation continuity. Applying whole-instruction grouping to Max is an
implementation choice consistent with those requirements; do not misattribute
the classifier design to this older source.

## Current-code checks and remaining obligations

- `AppleSealState::emit_speech_integrity` reads ledger recovery debt and emits
  Recovering/Unresolved or measured progress. This establishes a source path,
  not evidence that the installed pointer renders it correctly.
- `tick_refinements` flushes and retries work on live worker ticks. This alone
  does not prove that every missing occurrence gets the correct complete PCM.
- `SealedConsultationInput::from_ledger` now requires authenticated coverage and
  closed members. It refuses incomplete input; it does not schedule recovery.
  The Max grouping owner must wait for existing recovery and retry admission,
  not discard the instruction simply because one read returns None.
- The Max occurrence-formatter sender is now refused before credential lookup.
  This removes a pre-seal dependency; grouped execution is still unassembled.
- Preserve a distinct unresolved state when recovery fails. Do not turn the
  wait into either silent instruction loss or an infinite hidden queue.

Next structural work: connect candidate groups to recovery completion and
ordered admission, define the explicit unresolved/retry path, then scope answer
presentation to the admitted group. Verification remains owed under W1 embargo,
including real-audio coverage, pointer behavior, two-turn Max consultation,
permissions, cancellation and installed-artifact receipts.

## Additional source review at 0e912191e

On 2026-09-14 Roman extracted and read three more user-only conversations with
installed AICX. Each extraction reports one direct source parsed and no catalog
files opened. Extracts are under
`/Users/maciejgad/.aicx/codescribe-history-20260914/`, named
`roman-review-<full-session-id>-user.md`.

| Source session | Read extent | Disposition |
| --- | --- | --- |
| `01a09c61-1e33-7ec1-ac13-baa03ca7f0ff` | All 41 lines; 4 extracted messages | Repeats the integration question and positive pointer feedback present in the current Roman conversation. No additional Max behavior requirement. |
| `01a09c5f-2ae0-78e3-8de9-ad2a92ff3907` | All 29 lines; one image reference | Text extraction has no new instruction. Image pixels were not reviewed in this pass. |
| `ca094059-bb7c-4329-84c8-07d87f701447` | All 603 lines; header reports 22 messages, CLI reports 32 entries | Mixed host/runtime incident, not established Codescribe malfunction. See attribution below. |

Exact sources for the first two are respectively:
`/Users/maciejgad/.codex/sessions/2026/09/13/rollout-2026-09-13T22-06-45-01a09c61-1e33-7ec1-ac13-baa03ca7f0ff.jsonl`
and
`/Users/maciejgad/.codex/sessions/2026/09/13/rollout-2026-09-13T22-04-37-01a09c5f-2ae0-78e3-8de9-ad2a92ff3907.jsonl`.
The third source is
`/Users/maciejgad/.claude/projects/-Users-maciejgad-vc-workspace-VetCoders-CodeScribe/ca094059-bb7c-4329-84c8-07d87f701447.jsonl`.

### Mixed host incident: do not turn association into product causality

The third source contains the Founder's request to investigate recent heat,
inspect AICX catalog work, collect samples of their processes, and inspect
vc-frame logs. The supplied process snapshot lists WindowServer, vc-frame,
mds_stores and other processes; it does not establish a Codescribe root cause.
Later user messages contain vc-frame session listings and the output of the
Founder's deletion of Live runs and Finalized runs. These historical commands
are evidence, not present authorization to delete or terminate anything.

Much of the extraction is a provider-generated context/tool/skill inventory
stored under user role. For example, the huggingface skill table in the initial
continuity payload is inventory output, not a Founder architectural decision.
Role alone is insufficient to distinguish human instructions from pasted
diagnostics or provider commands. Source association by cwd alone is likewise
insufficient to assign every incident to Codescribe.

This pass does not inspect assistant/tool results, establish whether the host
incident was fixed, or prove worktree integration. Those remain separate evidence
questions. It also does not refresh the 1633-record census or imply all other
records are unreviewed: only the explicitly listed read extents are claimed.
No new product behavior or Max semantic-admission rule was derived from these
three extracts. The remaining Max assembly obligations above are unchanged.

## Additional user-source review at d00338b28

On 2026-09-14 Roman extracted two explicit Claude source files with
`aicx extract claude --file <source> --conversation --user-only -o <extract>`.
Both live beneath
`/Users/maciejgad/.claude/projects/-Users-maciejgad-vc-workspace-VetCoders-CodeScribe/`.
Outputs are under `/Users/maciejgad/.aicx/codescribe-history-20260914/`, named
`roman-review-<full-session-id>-user.md`. Each extraction parsed one source and
opened zero catalog files. No transcript was edited.

| Session | Read extent | Attribution |
| --- | --- | --- |
| `2bc39d81-fcc5-4e70-84b8-6688f315f835` | All 115 lines; header 20 messages, CLI 49 entries | Fork shares earlier Codescribe context, then explicitly moves to Screenscribe integration. That later assignment is not an instruction to modify Codescribe. |
| `2f29bfff-1430-4f61-8d21-ac144b207da1` | All 675 lines in two bounded reads; header 77 messages, CLI 155 entries | Codescribe parent plus subsequent unrelated host/privacy investigation and provider-generated compaction summary. |

Direct Founder messages add or reaffirm these Codescribe concerns:

- 2026-09-10 10:56/10:57 UTC: Finder Quick Action installation should be in the
  app, not require manually running a repository script. This independently
  supports the current app-bundled agent-skill installation direction, but is a
  separate product surface and must not be declared fixed by the skill buttons.
- 10:54 and 11:26: build artifact accumulation and proliferating settings backups
  were reported. The later summary's claimed 22 GB cleanup is an agent claim,
  not fresh evidence or authorization to delete today's files.
- 11:02: the short copied debug report was challenged. Its pasted config path
  does not prove the settings JSON location or effective runtime configuration.
- 11:14 and 18:27: text stopped while speech continued; the Founder endorsed
  Silero as an acoustic guard, whole-occurrence recovery and an explicit gap if
  recovery fails. A pasted long transcript is sample data, not a list of commands
  to execute or independently confirmed expected words.
- 11:53: the provider error was explicitly corrected from xAI to OpenAI. The
  compaction summary's proposed cause and claimed fix need separate source and
  executable verification; this pass did not certify them.

Current bounded source checks at this SHA:

- `macos/Codescribe/App.swift::onCopyDebugInfo` still builds seven lines from
  `loadSettings()`, classifies STT using `useLocalStt`, and reports `configDir()`.
  It omits source commit, effective lane provenance and actual settings JSON
  path. It is not sufficient for explaining the observed Dragon/local drift.
- `scripts/install-finder-quick-action.sh` exists and generates an Automator
  workflow which requires the installed CLI in `.local/bin` or `.cargo/bin`.
  Bounded Swift source search found no installer entry for that action; this is
  not a proof that every possible distribution surface was inventoried.
- Settings source has backup operations in migration, portable import and repair.
  Merely finding those writers does not identify which produced the historical
  thousands of timestamped files or whether accumulation continues now.

Images referenced in these extracts were not inspected. Assistant/tool outcomes
and the pasted provider compaction summary were not promoted to verified facts.
Historical fork/dispatch/stop, cleanup and external-program commands are not
replayed. No external privacy audit or Screenscribe implementation was started.
Next bounded work should improve the app's debug receipt from existing runtime
truth while retaining the separate Finder distribution and backup-origin debts.
This review expands explicit coverage of the existing census; it does not mean
all 1633 associated records have been read or the full goal is complete.

## Additional user-source review at 1bbcdda39

Read the user-only AICX extraction of session
`d3c24e5a-714a-46ac-a00e-a93ed7d2972c`, from the same uppercase Claude project
directory listed above. Output:
`/Users/maciejgad/.aicx/codescribe-history-20260914/roman-review-d3c24e5a-714a-46ac-a00e-a93ed7d2972c-user.md`.
All 1080 lines were read in bounded ranges; the truncated portion around lines
505–545 was explicitly reread. CLI reports 159 entries, extracted header 88
messages, while the older census reports 756 user messages. These different
counts are not reconciled: this is coverage of the extraction, not proof that
every raw source message or image was reviewed. No assistant/tool execution
receipts were independently inspected in this pass.

Direct Founder messages establish:

- September 5: design and implement rather than patch for green gates; compile
  embargo; meaningful hover affordances and clipped lettering complaints;
  multi-file CLI and Finder Quick Actions; explicit checkpoint permission with
  outstanding obligations recorded. Historical cleanup/release commands are
  not replayed and do not override the current W1 contract.
- September 6 09:40 UTC: file `501f74b9-4a1a-412f-8346-58f3ccf467ae.wav`
  failed at window 38–63 seconds, with 1328 decoded characters but no timestamped
  segments. At 09:43 the Founder explicitly rejected manufacturing one segment
  spanning the entire window merely to pass the assembly contract.
- At 10:11/10:12: diagnose the architectural cause; gates confirm the result,
  not define correctness. A process sample was supplied for a hung app; the
  extraction alone does not establish the sample's current availability or cause.
- At 10:19/10:20: CLI must use the singleton Whisper and the proper engine.
  The later proposal for an app-hosted socket and one model per machine is
  explicitly an agent design, not a direct Founder protocol specification.

The extraction contains extensive provider compaction summaries and duplicated
peer messages. Their claimed fixes, CPU/RSS measurements, install receipts,
dispatch status and historical hang hypotheses are leads, not current proof.
In particular, peer messages first suggest and then retract a concurrent-build
explanation of LINKEDIT corruption; neither claim is adopted without the actual
measurements. Later Vibecrafted Runtime Pack work is a separate repo assignment,
not completion of the Codescribe architectural request. Image pixels remain
unreviewed.

Current source trace at this baseline, after Loctree literal mapping:
`bin/codescribe.rs` calls `stt::transcribe_file_verdict_observed`, both router arms
call `whisper::transcribe_file_verdict_observed`, and that enters
`with_engine_initial_prompt`. `core/stt/whisper/singleton.rs` owns a static
`OnceLock<Mutex<WhisperSlot>>`. Thus this CLI path uses a process-local engine;
it does not call the app's resident engine through IPC. This source trace is
not a fresh RSS measurement, and no claim is made about every other engine path.
Shared-process ownership, historical long-file refusal recovery and the hang
still need their own current-source and executable evidence. Max consultation
assembly remains separately unfinished; this review does not close W1 or the
all-conversations goal.

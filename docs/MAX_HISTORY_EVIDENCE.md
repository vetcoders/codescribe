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

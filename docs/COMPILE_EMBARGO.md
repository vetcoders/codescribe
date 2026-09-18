# COMPILE_EMBARGO_CODING

Status: canonical execution protocol
Revision: 2026-09-09
Scope: architecture-sensitive implementation by one agent or a Fleet Worktree
formation

## 1. Definition

`COMPILE_EMBARGO_CODING` is a phase-separated implementation technique in
which source structure and its executable contracts are assembled before the
compiler, linter, test runner or live runtime is allowed to influence the
shape.

The technique does **not** claim that compilation is unimportant. It preserves
compilation as an independent falsifier. A green build is valuable after the
architecture has a declared shape; while the shape is still being discovered,
green can become a local optimization target that rewards adapters, duplicate
truth, compatibility scar tissue and accidental ownership.

The short law is:

> Shape first. Close it structurally. Then let compilation falsify it.

Embargo is therefore a sequencing protocol, not a relaxed quality standard.
No phase may borrow the verdict of a later instrument, and no later green gate
may retroactively disguise an earlier structural failure.

## 2. Why a gate is deferred, and when it returns

An embargo follows from the stage of the work and its dependencies, not from
who approved it. A gate is deferred only when its result is predictably
useless because it checks exactly the fragment being dismantled. Every other
check keeps running.

Each deferral carries three things:

1. the exact command or hook id that is deferred;
2. the reason its verdict would be noise at this stage ("refactor in
   progress" is not a reason; name what is missing for the check to be
   meaningful);
3. the condition under which it returns.

A gate returns the moment its reason disappears, not at a ceremonial phase
close. An embargo is never extended because a returning gate exposed a defect;
the defect is repaired. Before a release there are no deferrals: every required
gate and the real product path run on the generation being released.

## 3. What is embargoed

From W1 opening until an explicit W2 structural-close attestation, workers do
not run any command whose result can steer implementation through
executability:

- compiler, build or package build;
- typechecker;
- project import or dependency-resolution smoke;
- linter or formatter when it parses or rewrites project semantics;
- unit, integration, snapshot, UI or end-to-end tests;
- application, service, model or live-runtime probe;
- dependency installation performed to make one of the above possible.

The plan must name exact forbidden commands for the repository. Examples alone
are not a contract. A worker does not choose its own gate subset: partial
gates pull the worker into repairing local greenness instead of finishing the
cut, which is the tunnel the embargo exists to prevent.

The embargo does not forbid evidence. It permits preregistered structural
checks that do not execute or import the product, such as:

- Loctree structure, slice, impact, references and occurrence evidence;
- literal contract and absence checks with bounded text search;
- definition/reference, owner/consumer and residue censuses;
- review of source, schemas, routes, manifests and generated contracts;
- `git diff --check` and scoped diff inspection;
- secret detection and merge-conflict/line-ending hygiene;
- scoped static security scans that do not build or import the product;
- a neutral syntax parser only when the plan explicitly admits it as a
  structural instrument.

Tests are authored during W1/W2 because they state the intended contract. They
remain unrun until the embargo closes.

## 4. Roles and truth ownership

For a multi-agent execution there is one Agent-Operator acting as integrator.
Workers own isolated structural atoms; they do not own admission into the
destination tree.

- Founder: chooses the mission, accepts product/architecture decisions and owns
  trunk, publication, deploy and other explicitly reserved buttons.
- Agent-Operator/integrator: owns the plan, phase state, recovery channel,
  dispatch ledger, admission, structural-close attestation, the first compile
  and the moment every deferred gate returns.
- Worker: owns one closed file/symbol domain, its unrun contract tests, one
  durable checkpoint commit and one honest report of unknowns.
- Compiler/test/runtime: independent falsifiers used only in their declared
  phases. They are not architecture authors.

Shared truth must have one writer: one plan, one tracker/journal, one embargo
state and one integrator. Per-provider copies must not redefine phase or
completion semantics.

### 4.1 Three levels of truth

The protocol keeps three verdicts apart. Conflating any two of them is the
failure the embargo is designed against.

| Level                  | Who                     | What it proves                                                                                                  |
| ---------------------- | ----------------------- | --------------------------------------------------------------------------------------------------------------- |
| Checkpoint             | worker                  | the work and its exact SHA are preserved; nothing about quality                                                 |
| Structural integration | integrator              | scope, ownership, contracts between cuts, security scan and review are reconciled; deferred gates still pending |
| Verified delivery      | integrator, after close | every required gate and the real product path passed on the named generation                                    |

## 5. Enforcement grades

Every execution declares one of these grades before W1:

### A. Repository-enforced

The repository contains and consumes a tracked phase guard. A local marker may
open W1/W2 and defer only explicitly allowlisted gates. Non-deferred security,
provenance and hygiene checks continue to run. Malformed state fails closed.

The reference mechanism in this repository is `scripts/git-hooks/embargo-guard.sh`
wrapping the deferrable pre-commit hooks, driven by `.vibecrafted/embargo.toml`
(template: `.vibecrafted/embargo.toml.example`):

- required fields: `plan_id` (lowercase slug), `phase` (`W1` or `W2`),
  `deferred_gates`, `attestation` (`open` or `W2_STRUCTURALLY_CLOSED`),
  `recovery_ref` (must equal `embargo/<plan_id>`);
- deferrable set: `cargo-check`, `cargo-fmt`, `cargo-clippy`, `prettier`, and
  nothing else; the guard rejects any other id;
- always running, marker or not: whitespace/EOF/line-ending hygiene,
  merge-conflict check, private-key detection, commit-message provenance and
  the pre-push security scan;
- at `attestation = "W2_STRUCTURALLY_CLOSED"` every deferred gate is active
  again immediately; the attestation is only valid in `phase = "W2"`;
- a missing marker is the ordinary repository path (all gates run);
  `scripts/git-hooks/embargo-selftest.sh` proves the guard on a throwaway
  repository.

### B. Dispatcher-enforced operational embargo

Used when the repository has no consuming guard. Every rendered worker prompt
contains the same explicit forbidden-command list; the Agent-Operator checks
transcripts and reports before admission. Reports must call the absence of a
technical guard out directly.

### C. Informal

A conversational request without a marker, dispatcher contract or auditable
phase receipts is not sufficient for a fleet execution. Upgrade it to A or B
before writing code.

Never create a decorative marker in a repository that has no guard consuming
it. The marker is an enforcement input, not theatre.

### 5.1 Checkpoints and `--no-verify`

A worker can always preserve its work. The preferred path is the guard above:
hooks skip only the deferred gates and run everything else. When the
repository has no guard, or the guard cannot express the declared deferral, a
worker may use `git commit --no-verify` for a local checkpoint inside the
declared phase.

`--no-verify` bypasses the whole hook entry point, including checks nobody
intended to defer. The checkpoint report therefore names the exact SHA, the
scope and every hook that did not run, security hooks included. The commit
preserves work; it certifies nothing. The integrator runs the skipped security
and hygiene checks before structural admission, not after delivery.

Outside a declared phase checkpoint, `--no-verify` remains forbidden. Workers
never push with `--no-verify`; that push belongs to the integration and
publication authority, never to a worker.

## 6. Phase protocol

### W0 — admission and preregistration

No product implementation begins until the Agent-Operator records:

1. repository root, runtime class, baseline branch and full baseline SHA;
2. clean/dirty state and ownership of any pre-existing changes;
3. target architecture, invariants and one source of truth per behavior;
4. closed worker domains and their overlap matrix;
5. acceptance contracts and tests each worker must author;
6. allowed structural instruments and exact forbidden commands;
7. each deferred gate with its reason and return condition (section 2);
8. enforcement grade, phase state and recovery ref;
9. commit/report paths, provenance requirements and integration authority;
10. later compile, test and runtime gates;
11. failure/re-entry budget.

For Fleet Worktrees, the supervisor creates each worktree from the exact pinned
SHA. A branch mismatch, dirty starting tree or wrong merge-base is
`SUBSTRATE_FAILURE`, not a reason to improvise.

If Grade A enforcement does not already exist, its guard is a separate W0 cut
that must be proven before product W1 begins. If Grade B is deliberately chosen,
the journal says so and does not claim hook enforcement.

### W1 — structural atoms

All independent workers may run concurrently. Each worker:

1. verifies branch, merge-base and clean start;
2. maps the domain before editing and reads each affected file immediately
   before changing it;
3. implements the complete atom, not a compile-shaped fragment;
4. authors non-trivial tests for success, failure and ownership boundaries but
   does not execute them;
5. stays inside the closed domain and reports required cross-domain seams as
   `DANGLING` or `BOUNDARY`;
6. runs only W1-admitted structural checks;
7. stages only owned files/hunks and creates one durable checkpoint commit;
8. writes a report with baseline, terminal SHA, changed files, exact checks,
   unrun tests, skipped hooks, unknowns and the next integrator instruction.

The canonical W1 receipt is:

```text
GATE=green PHASE=W1 BUILD/LINT/TEST=NOT_ASSESSED (embargo W1)
```

Here `green` describes only the declared W1 structural gate. It must never be
read as compiler, test or runtime success.

Workers do not merge, push, deploy, restart services, repair sibling cuts,
lift the embargo or decide which gates return. They commit coherent local
batons; partial uncommitted work is not a baton.

### W2 — integration and structural closure

The Agent-Operator integrates on the declared recovery/integration ref, not
silently on trunk. For every cut, independently verify:

- exact baseline and ancestry;
- worker branch and terminal commit;
- report identity and finalized claim;
- changed-file fence and absence of unrelated work;
- actual diff, not just the commit subject;
- W1 structural checks, explicit embargo receipt and the list of skipped
  hooks;
- security and hygiene checks the checkpoint skipped, re-run now;
- all declared dangling seams.

Admission alone is not structural closure. The integrator then wires the atoms
and proves the complete source graph without compiling:

- every intended definition has the intended consumers;
- every production consumer resolves to one declared owner;
- no bypass path or duplicate truth remains;
- schemas, types, routes, storage and lifecycle edges agree;
- tests cover cross-cut seams and negative paths;
- all executable residue has an explicit disposition;
- absence claims come from a complete enough instrument, with truncation and
  coverage recorded;
- no `DANGLING`, `BOUNDARY` or unresolved conflict is hidden.

`STRUCTURALLY_WIRED` is not enough. The embargo closes only on an explicit:

```text
W2_STRUCTURALLY_CLOSED
```

The attestation means "the system is assembled and fit to be checked". It does
not mean "the system works". It records the integrated SHA, structural
instruments, unresolved unknowns (normally none), who signed it and the
timestamp. In Grade A, changing the marker to this attestation restores all
deferred commands before any first compile. In Grade B, the Agent-Operator
journals the phase transition and unlocks only the preregistered commands.

Structurally unfinished fragments may be integrated when later cuts depend on
them, provided their unverified state stays visible in the tracker until the
gates that cover them return.

### W3 — first compile and bounded recovery

The first compilation is a measurement. Preserve its exact command, output,
exit code and integrated SHA before changing source.

Classify every failure before repair:

- `MECHANICAL_RECOVERY`: spelling, import/export, signature or generated glue
  that preserves the closed ownership model;
- `CONTRACT_MISMATCH`: authored contracts disagree across cuts and require an
  explicit integration decision;
- `INSTRUMENT_FALSE_NEGATIVE`: the W2 structural instrument claimed closure
  while a discoverable dangling edge remained;
- `ARCHITECTURE_COMPILER_CONFLICT`: satisfying the compiler would require a new
  owner, fallback, duplicate truth, compatibility bypass or changed invariant;
- `SUBSTRATE_FAILURE`: toolchain, dependency, environment or hook state is not
  the declared substrate.

Only `MECHANICAL_RECOVERY` may enter a bounded compiler-guided repair loop
without reopening architecture. `CONTRACT_MISMATCH` and
`ARCHITECTURE_COMPILER_CONFLICT` return to an explicit structural re-entry; the
original W2 result remains recorded and is not rewritten. An
`INSTRUMENT_FALSE_NEGATIVE` also produces an instrument defect receipt and a
new regression assertion.

After compilation passes, the integrator runs the repository's full
lint/type/build/test and security gates and distributes concrete repairs from
their results. Requirements are not lowered to make a gate pass. Green now
means only that those named gates passed on the named SHA.

### W4 — runtime falsification

Exercise the real product path on the artifact produced from the verified SHA.
Check behavior, failure paths, persistence, cleanup, security boundaries and
user-visible truth. Source tests, HTTP 200, process liveness or an install
receipt alone are not runtime proof.

Record runtime identity and provenance. Any runtime-driven repair becomes a
new bounded cut with its own structural and verification receipts; do not
quietly fold it into the old W2 attestation.

## 7. Fleet topology

A valid maximum-concurrency W1 has this shape:

```text
                          pinned baseline SHA
                         /        |        \
                    cut A      cut B      cut C ...
                  checkpoint checkpoint checkpoint
                         \        |        /
                      integrator recovery ref
                               |
                    W2 structural closure
                               |
                      first compile (W3)
                               |
                    full gates and runtime (W4)
```

Concurrency is earned by disjoint ownership, not by optimistic scheduling.
Two workers touching the same owner, schema, route, generated file or shared
test oracle are either one cut or explicitly serialized. Because no cut is
compiled before W2, the cost of overlapping domains lands entirely on the
integrator's first compile; shape the cuts so that it does not.

If a provider fails before writing, verify PID, worktree, diff, commit and
report. Re-dispatch only that atom with a recovery baton stating exactly what
exists. Never restart healthy siblings. If work exists, the baton identifies
the exact commit/diff and inherited unknowns; if nothing exists, it says so.

## 8. Honest-state vocabulary

Use these states literally:

- `NOT_ASSESSED`: the phase intentionally did not run the instrument;
- `UNKNOWN`: evidence is absent or insufficient;
- `STRUCTURALLY_WIRED`: components are connected, closure not yet proven;
- `W2_STRUCTURALLY_CLOSED`: the preregistered structural closure gate passed;
  the system is fit to be checked;
- `MECHANICALLY_GREEN`: named compile/lint/test gates passed on an exact SHA;
- `RUNTIME_VERIFIED`: named real paths passed against an identified artifact;
- `BLOCKED`, `BOUNDARY`, `DANGLING`: unfinished truth, stated before delivered
  work in reports.

Do not use `ready`, `done`, `green` or `integrated` without the instrument,
scope and exact SHA that make the word true.

## 9. Stop conditions

Stop the current phase when:

- the baseline or worktree identity is wrong;
- a worker must cross its closed domain;
- the enforcing guard behaves differently from the declared phase;
- the structural instrument is truncated, stale or unable to prove a required
  absence;
- a compile/test/runtime command was accidentally run during W1/W2;
- compiler recovery would change ownership or architecture;
- integration evidence cannot distinguish authored work from unrelated dirt.

An accidental embargo break is disclosed immediately. The affected scientific
claim may be invalid even when the product work remains salvageable.

## 10. Anti-patterns

- Compiling every worker cut and calling the final integration an embargo.
- Treating unrun tests as passing tests.
- Adding adapters or fallbacks until the build turns green.
- Using formatting or autofix as an undeclared source rewrite.
- Declaring closure from definitions while ignoring references and consumers.
- Letting a marker skip secrets, provenance, merge-conflict or other unrelated
  safety gates.
- Creating a marker no tool consumes.
- Reconstructing missing evidence after seeing later test/runtime results.
- A worker choosing its own subset of gates to run "just to be safe".
- Using `--no-verify` outside a declared phase checkpoint, omitting the
  skipped-hook receipt, or using it to claim security-clean or verified
  delivery.
- A worker pushing with `--no-verify`.
- Extending an embargo because a returning gate found a defect.
- Merging worker branches because their reports say `completed`.

## 11. Minimal execution checklist

Before W1:

- [ ] pinned clean baseline and recovery ref
- [ ] single integrator and explicit reserved buttons
- [ ] disjoint closed domains and overlap matrix
- [ ] architecture invariants and one owner per truth
- [ ] authored-test requirements
- [ ] enforcement grade and exact forbidden commands
- [ ] each deferred gate with reason and return condition
- [ ] structural gates and phase receipts

Before closing W2:

- [ ] every worker baton independently verified
- [ ] skipped security and hygiene checks re-run by the integrator
- [ ] all atoms admitted on the recovery/integration ref
- [ ] consumers, definitions, owners and residue reconciled
- [ ] cross-cut tests authored but still unrun
- [ ] zero hidden dangling/boundary items
- [ ] `W2_STRUCTURALLY_CLOSED` tied to exact SHA

After closing W2:

- [ ] first compile captured as immutable evidence
- [ ] failures classified before repair
- [ ] architecture conflicts cause re-entry, not green-driven patching
- [ ] full named quality and security gates pass on exact SHA
- [ ] real artifact/runtime path is falsified separately
- [ ] final report distinguishes structural, mechanical and runtime verdicts

## 12. Provenance

This protocol distills several Fleet Worktree executions run under a compile
embargo across Vetcoders repositories, including the phase guard and selftest
that live in this repository. The repository-owned guard, its selftest and the
dispatch reports of those executions are the evidence; this document does not
replace them.

This revision derives the embargo from the stage of the work rather than from
an approval, moves the return of every deferred gate to the integrator, and
permits `--no-verify` only for a declared phase checkpoint with a full
skipped-hook receipt. It does not authorize worker pushes or convert skipped
verification into delivery.

## 13. Neutral delivery/Stop AST exception (2026-09-10)

The `rc-w3-guard-ast` plan admits **only** `codescribe-structural-ast` under
`tools/structural-ast`, using locked `syn=2.0.118`. It has no app/core/bridge
dependency and reads complete Loctree body JSON from stdin; it never opens
product source, executes snippets, loads models or opens devices. Python remains
the gate owner and still requires production definitions and real callsites.
The only additional verifier subprocess is exactly:

```sh
cargo run --offline --locked --package codescribe-structural-ast --bin codescribe-structural-ast --quiet
```

Cargo checks/builds the current neutral sources before execution. Python records
the command, root, input and source/lock digests, parser identity and macro import
receipts, and rejects source drift, failed builds, malformed evidence, alternative
packages/arguments and injected package targets/build scripts. Compiler wrappers,
Rust flags and arbitrary Cargo override environment variables are not inherited.
The default target is `<selected-repository>/target`. An explicit
`CARGO_TARGET_DIR` must name an existing directory; relative values resolve
against the selected repository, never the caller's working directory. Validation
rejects empty values, whitespace at component boundaries, control characters,
shell expansion markers (`~`, `$`), backslashes and colons; no expansion is
performed. Root, home,
repository roots and ancestors of the repository or home are refused, as are
non-directory components and symlinks anywhere in the target path (including
before `..` normalization). The selected repository itself is canonicalized first.
Validation creates, deletes and cleans nothing; Cargo may create a missing default
`target`. Only the resolved validated target and the validated build lease enter
the sanitized child environment, and the receipt records that exact target. The
schema requires a canonical absolute target spelling; filesystem and ownership
checks belong to Python's pre-execution validation, not JSON Schema.

The build lease is `CARGO_BUILD_JOBS` and `CARGO_INCREMENTAL`. Absent values keep
the shipped defaults: `jobs=4` and Cargo's own incremental policy; fleet leases
stay in run artifacts and are never shipped defaults. When supplied explicitly,
`CARGO_BUILD_JOBS` must be a positive decimal integer (no sign, no leading zero,
no separators, at most 1024) and `CARGO_INCREMENTAL` exactly `0` or `1`. Both are
validated before the target is resolved and therefore before any child process
could exist; a malformed value is refused and never reaches Cargo. No other
compiler setting is negotiable: wrappers, Rust flags, encoded flags, target
triples and dynamic-library preloads are still dropped, and no invocation uses a
shell. The receipt's `invocation` reports the effective `jobs` and an explicit
`incremental` policy, where `null` means "not supplied, Cargo's own default"
rather than a value the caller never stated.

Adding a required `incremental` key and replacing the previous forced `jobs`
constant is a breaking change to the invocation contract, so the receipt
generation is `codescribe.acoustic-structure-receipt.v3`; `v2` is recorded in
`superseded_versions` and its already-written receipts remain valid under their
own declared generation rather than being retroactively invalidated. Consumers
select on the receipt's declared generation, as below.

Active gates for this tool are package-selected offline tests and Clippy,
package formatting, Python instrument tests, the complete wired verifier,
scoped static security and `git diff --check`. These are neutral instrument
gates, not product W1/W2 execution or RC acceptance. Only `SKIP=cargo-check` is
admitted for its checkpoint: that hook compiles app/core/bridge while the new
tool/receipt contract is being assembled. It returns immediately when the
integrator admits the tool/receipt wiring; full workspace Cargo check is then
mandatory. This is not an extension of the resolved Whisper embargo. Security,
hygiene, provenance and formatting remain active. Broad formatting is checked
read-only before commit; foreign changes are never repaired by this worker.

The evidence language is intentionally closed. Typed `syn` statement productions
recognize target-binding provenance, the guarded paste/deferred branches, recorder
Stop delegation, optional archive publication, task join, sink drain/drop, archive
failure classification and typed incomplete-coverage refusal. Structural AST
comparison checks every nested node, binding, argument and attribute, not source
substring order. A separate AST visitor inventories all paste call expressions,
tracks then/else guard scope and refuses guards borrowed by deferred closures.
Extra statements/effects, altered branches, unsupported attributes, callbacks,
loops, macros or signatures produce `BOUNDARY`; even a safe equivalent rewrite
can require a reviewed grammar extension. This is deliberately not a general
Rust control-flow engine. Formatting/comments do not affect evidence; punctuation
inside macro token trees and exact callback shapes remain part of the grammar.

Assumptions are explicit and bounded to the named bodies:

- Calls retain their resolved ordinary Rust/library meaning and return normally
  unless their explicit `Result` is handled. Panics, aborts, cancellation, dynamic
  dispatch, overloaded trait behavior and hidden effects inside callees are not
  proved. The fixed argument/receiver shapes prevent arbitrary callees from
  borrowing a known operation's label. The drain loop proves lexical shutdown
  ordering, not real-time progress or scheduler fairness.
- `Option` callbacks, the known drain loop and match arms are recognized in full;
  no arbitrary closure return or `?` can stand in for a function exit. The join
  captures errors without `?`; both archive error and success flow through the
  shutdown sequence. The incomplete branch returns the original receipt,
  audio path and committed text before the only final success.
- `debug!`, `info!`, `warn!` are the imported `tracing` macros, not shadowed macros.
  Current Loctree import receipts authenticate both source modules. Their locked
  `tracing 0.1.44` definitions expand to event reporting; only the exact reviewed
  argument trees are admitted (including the route-formatting call). No blanket
  logging-macro exemption exists. Shadowing in an unprovided ancestor module or
  changes in dependency macro semantics require renewed review.
- Standard `matches!` and `format!` retain their standard definitions. The former
  lowers its exact target/frontmost predicate to a Boolean match; the latter
  constructs diagnostic text. Only the enumerated invocations are recognized;
  an added macro or control-bearing argument is refused. Definition inspection
  used the installed Rust 1.95 source and cached tracing source, without builds
  of the product or dependency downloads.

Synthetic bypasses test the instrument; they are not discovered product exploits.
The eight other unresolved corridor obligations remain independent and red.

The neutral command changes the receipt format to
`codescribe.acoustic-structure-receipt.v3`. Its complete JSON Schema is embedded
at `tests/fixtures/acoustic_throne_stages.json#/tool_contract/receipt_schema`,
inside this cut's admitted tool-contract domain. The frozen external v1 schema
remains a legacy artifact; v2 and v3 receipts no longer claim to conform to it.
The schema includes existing ordering observations as well as neutral evidence;
ordering verdicts and all unrelated corridor obligations retain their existing
Python checks. Schema consumers must select the receipt's declared generation.

## 14. Instrument staleness is not product truth (2026-09-10)

A structural instrument names product symbols. When the product renames or
re-owns one, the instrument's claim about it stops being a measurement, and the
verifier's verdict about that hop carries no information about the product. Two
shapes of this were observed and must be read apart from real defects.

**Fail-closed yields no failure list.** `loct body <retired-symbol>` resolves to
zero bodies and exits non-zero, so the verifier aborts before writing a receipt:
exit 2, empty stdout, one stderr line. That is _not_ "zero failures", and it is
not a passing run either. A run with no receipt may never be reported as a count.

**A delegation refactor silently moves every body obligation.** When a function
is split into a thin delegator plus a generic `_with` implementation, body-level
`required_code` still resolves the delegator — now a few lines long — and every
obligation reads as missing, while the callsite edges move to the new owner.
Observed twice in the same file: `repair_terminal_seal_coverage` →
`repair_terminal_seal_coverage_with`, and `seal_sliced_by_silero` →
`reconcile_silero_ledger`. The corridor must follow the owner that actually
executes the code; re-pointing it is an instrument repair, never a product fix.

Two evidence-provider limits bound what a corridor may assert, and neither may be
worked around by flipping metadata, raising a line cap or reading raw product
source as an unauthenticated fallback:

- a generic function whose `where` clause spans lines can return a truncated
  `window` extent instead of `brace`, identically at every line cap. No body
  obligation can be placed on such an owner; its contract has to be carried by
  callsite edges, and the gap recorded as an explicit `BOUNDARY`.
- `enclosing_symbol` can resolve to a function-local `const` rather than the
  enclosing function, so a caller-attribution proof observes zero callsites for
  calls that genuinely exist. That is an `INSTRUMENT_FALSE_NEGATIVE` and must be
  classified as one before anyone repairs product code to satisfy it.

## 15. Guarded ordering is a separate obligation (2026-09-10)

Re-pointing a corridor onto the owner that actually executes the code proves
presence. It does not prove sequence, and in an admission chain sequence is the
property that matters: qualification after admission is not qualification. A
corridor therefore carries two independent kinds of evidence, and a repair that
supplies only the first is a renamed callsite wearing a proof's clothes.

**In-order body evidence.** A hop's `required_code` must appear in the declared
order inside one complete brace-extent body, judged after comments and string
literals are removed. This is what pins ownership against recognition context:
the capture chain mints its occurrence from the physical Silero range and
qualifies it before `bound_context_range` produces any padded window, so the
padded window cannot become the thing that is owned. Reordering those two lines
is refused as `executable code out of required order`, not as missing code.

**Cross-callsite ordering evidence.** A corridor's `ordering` rows compare the
observed lines of two declared invocation receipts inside one caller, and may
require a single-line `barrier` fragment strictly between them. The barrier
exists because two calls in the correct order can still be wrong when the guard
that used to sit between them is gone; naming the guard makes its removal a
failure rather than a silent widening.

Ordering has a falsification property worth stating, because getting it wrong
produces a green that cannot go red. Observed lines come from occurrence
receipts, not from body text, so **mutating a body can never falsify an ordering
row**. A suite that drops `ordering` before mutating bodies proves the hops and
nothing else. Ordering falsifiers must mutate the occurrence receipts — move a
callsite, drop an edge — and a barrier falsifier must remove a fragment that is
deliberately _not_ also a `required_code` snippet, or it merely re-tests the hop.

**One manifest key is read; a near-twin is not.** The corridor engine reads
`minimum_count` and defaults to 1; `min_calls` is never consulted. Every
surviving `min_calls` entry in this repository declares 1, so no obligation is
currently lost — but a future `min_calls: 2` would be silently downgraded to 1
and would read as an enforced multiplicity that is not enforced. The capture
corridor is normalized to `minimum_count` and pinned by a test; the remaining
33 entries in two other corridors are a separate, deliberately unrepaired cut,
because rewriting them is only safe with each corridor's counts re-measured.

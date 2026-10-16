# Merge queue — operator runbook

Status: **native merge queue enabled on protected `main`.** On 2026-10-16,
[PR #172](https://github.com/vetcoders/codescribe/pull/172) landed the hosted
workflow bootstrap as squash commit `8e45c489aa670f06d4e1dfef249307e7099f12f0`.
The queue configuration was verified through GitHub's API. At activation, the
queue was empty and the first real `merge_group` entry had not been exercised.
Configuration and each candidate's actual execution have separate receipts.
The old self-hosted workflow is history (this repository is public; hosted
runners only). This application line includes standalone `l10n.yml` together
with its consumers; they are admitted as one application candidate.

## What the source provides

- `rust.yml`, `l10n.yml`, `semgrep.yml` each trigger on
  `merge_group: types: [checks_requested]` — GitHub requires this in every
  Actions workflow that feeds a required check
  ([docs](https://docs.github.com/en/repositories/configuring-branches-and-merges-in-your-repository/configuring-pull-request-merges/managing-a-merge-queue)).
- Concurrency group = workflow + event + ref. `cancel-in-progress` is false for
  `merge_group`: a cancelled queue check removes the entry. Ordinary PR runs
  still cancel older runs.
- Both `l10n.yml` and Rust Quality measure the prose ratchet for a queue entry against
  `github.event.merge_group.base_sha`, fetched by SHA. A missing, all-zero or
  malformed value fails before any `git fetch`; there is no fallback to `main`.
  Main push keeps `github.event.before`; PRs keep their real destination
  branch (stacked PRs included). Ref data reaches the shell through `env:`,
  never through expression interpolation.
- The required status stays `Clippy + Tests` (job `quality`, unconditional,
  `make verify`). Other jobs: `Rust 1.94 MSRV`, `Format Check`, `Localization`,
  `Security Scan`. No status was renamed.

Tests: `scripts/tests/test_l10n_workflow_base.py` runs the extracted workflow
shell against a fake `git`; `scripts/tests/test_ci_workflow_contract.py` parses
the YAML (triggers, concurrency, required job shape, and that the tests
themselves are wired in). Both are **required CI**: the unconditional step
`Run workflow merge-queue contract tests` in the `Clippy + Tests` job runs
them before any compiler stage, in a private venv under `RUNNER_TEMP` with
test-only `PyYAML==6.0.3` (no global install, no product dependency).

```
python3 -m unittest scripts/tests/test_l10n_workflow_base.py scripts/tests/test_ci_workflow_contract.py
actionlint .github/workflows/rust.yml .github/workflows/l10n.yml .github/workflows/semgrep.yml
```

Locally the first command needs PyYAML importable; CI provisions it itself.
CI invokes the three test files directly with Python's isolated `-I` option;
module-name loading would lose the checkout from its import path. The required
quality job fetches its comparison base before invoking the bridge census,
including on a shallow checkout.

## Not exercised by the tests

A real queue entry on GitHub (`gh-readonly-queue/main/...` branch, hosted
`xcode-27` runner, the ruleset's check timeout) remains unexercised: at the
2026-10-16 06:52 UTC activation receipt, the queue had zero entries and no
`merge_group` run had been observed. Admission depends on the actual required
checks for the entry. Those checks do not replace native application acceptance.

## Activation receipt

- PR #172 landed only the minimal hosted `rust.yml` and `semgrep.yml`
  bootstrap, both with `merge_group`, plus their contract checks. All 13 PR
  checks passed, including `Clippy + Tests` with full `make verify`. The squash
  tree equals the tested PR tree; post-merge runs are separate evidence.
- GitHub API verification on 2026-10-16 confirmed an active `merge_queue` rule
  in `main` ruleset `24525207`: merge method `SQUASH`, grouping `HEADGREEN`,
  max entries to build 1, min/max entries to merge 1, min wait 0 minutes,
  check response timeout 180 minutes. Existing rules, `Clippy + Tests` and
  bypass actors were preserved. Repository `allow_auto_merge=true`;
  automatic branch deletion remains false.
- At bootstrap, `main` had no `L10N_BASE` consumer or prose ratchet. Admit standalone
  `l10n.yml` only together with the consumers it checks, taken from the
  candidate. The candidate keeps all three workflows as one contract.

## Enqueue (integrator only)

Admission is by explicit enqueue guarded by the head SHA, for example the
GraphQL `enqueuePullRequest(input: {pullRequestId, expectedHeadOid})`.
Activation did not enqueue another PR or retarget PR #170. Do not auto-enqueue
every PR, do not use `pull_request_target`, no admin bypass, no force.

## Inspecting the queue

- Entries and head: GraphQL
  `repository(owner: "vetcoders", name: "codescribe") { mergeQueue(branch: "main") { entries(first: 20) { nodes { position state pullRequest { number headRefOid } } pageInfo { hasNextPage endCursor } } } }`
  (read-only), or the repository Merge queue page.
- Checks for the head entry: runs for event `merge_group` on branch
  `gh-readonly-queue/main/pr-<N>-<sha>`
  (`gh run list --event merge_group --branch ...`).
- Removal: the queue page and the PR timeline show why an entry left (failed
  check, timeout, conflict). Re-enqueue only after fixing the cause.

## Reception after a squash merge

A queue squash produces a new commit that is not a descendant of the PR head.
Verify reception by tree/patch equivalence (`git diff <pr-head> <main-commit>`
empty for the PR's paths, or equal `git patch-id`), not by
`git merge-base --is-ancestor`, which reports a false negative.

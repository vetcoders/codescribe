# Merge queue — operator runbook

Status of this document: **candidate source, not activated.** The workflows in
this tree can serve a native GitHub merge queue. Protected `main` does not run
this tree: it still carries the old self-hosted workflow and has no `l10n.yml`.
Nothing here is live until the workflow change reaches `main`, and the old
self-hosted workflow must not be resurrected to bridge that gap (the repository
is public; hosted runners only).

## What the source provides

- `rust.yml`, `l10n.yml`, `semgrep.yml` each trigger on
  `merge_group: types: [checks_requested]` — GitHub requires this in every
  Actions workflow that feeds a required check
  ([docs](https://docs.github.com/en/repositories/configuring-branches-and-merges-in-your-repository/configuring-pull-request-merges/managing-a-merge-queue)).
- Concurrency group = workflow + event + ref. `cancel-in-progress` is false for
  `merge_group`: a cancelled queue check removes the entry. Ordinary PR runs
  still cancel older runs.
- `l10n.yml` measures the prose ratchet for a queue entry against
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
the YAML (triggers, concurrency, required job shape).

```
python3 -m unittest scripts/tests/test_l10n_workflow_base.py scripts/tests/test_ci_workflow_contract.py
actionlint .github/workflows/rust.yml .github/workflows/l10n.yml .github/workflows/semgrep.yml
```

## Not exercised by the tests

A real queue entry on GitHub (`gh-readonly-queue/main/...` branch, hosted
`xcode-27` runner, the ruleset's check timeout) can only be observed after
activation. Candidate-wide failures (known library test failures and Swift AX
cases on the integration tip) are not hidden by the queue: admission depends on
the real required checks.

## Staged activation (integrator only)

1. Land the workflow change on `main` through the normal protected path. Do not
   deploy the candidate wholesale.
2. Confirm on `main` that all three workflows contain `merge_group` and that a
   `Clippy + Tests` run exists for that tip.
3. Only then add the `merge_queue` rule to the `main` ruleset (repo ruleset
   `24525207`), keeping the existing `Clippy + Tests` requirement. Reference
   parameters used by Vibecrafted: merge method `SQUASH`, grouping `HEADGREEN`,
   max entries to build 1, min/max entries to merge 1, min wait 0 minutes,
   check response timeout 180 minutes. Existing bypass actors stay as they are;
   do not copy Vibecrafted status names.
4. Admission is by explicit enqueue guarded by the head SHA, for example the
   GraphQL `enqueuePullRequest(input: {pullRequestId, expectedHeadOid})`. It
   works with `allow_auto_merge=false`; `gh pr merge` may try
   `enablePullRequestAutoMerge` and be refused. Alternatively the integrator
   explicitly enables the repository auto-merge setting. Do not auto-enqueue
   every PR, do not use `pull_request_target`, no admin bypass, no force.

## Inspecting the queue

- Entries and head: GraphQL
  `repository.mergeQueue(branch: "main") { entries { position state pullRequest { number headRefOid } } }`,
  or the repository Merge queue page.
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

# Version and source notes

`make bump-patch`, `make bump-minor` and `make bump-major` prepare the Cargo
versions, owning Cargo.lock package versions, README version references,
CHANGELOG entry and commit inventory in one
local transaction. They do not build, tag, upload or publish a release.

The script promotes human-authored `Unreleased` notes and adds a short draft
summary from commit subjects. The full source history lives in
`<version>-commits.json`; squash headings are historical descriptions, including
intermediate changes that later commits may have revised. They are not runtime
acceptance evidence. Review the short draft before publishing it.

An untagged version with generated notes is still in preparation: repeating a
bump refreshes that entry instead of incrementing again. A reachable `v<version>`
tag closes it. `make bump TYPE=patch TO=0.16.1` also names an explicit desired
version and is safe to repeat. Downgrades are refused. The prior reachable release
tag or recorded source boundary determines the commit range; a missing boundary
requires an explicit `--from` rather than silently importing all repository history.

To backfill or refresh an existing human summary without changing version files:

```sh
python3 scripts/release-version.py notes --version 0.16.0 \
  --from v0.15.2 --until v0.16.0 --date 2026-10-08 --inventory-only
python3 scripts/release-version.py notes --version 0.16.1 --inventory-only
```

Existing human release summaries remain intact. Generated draft summaries are
bounded to two items per section and refreshed within their marked block. Move
approved text outside that block to curate it permanently. The inventory records
only committed work; any pending source repairs must be described explicitly in
the human note and admitted by the integrator after validation.

The Git-directory lock serializes bump invocations. Every changed file is staged
through an atomic replacement with a durable transaction journal containing its
original bytes. Write failures roll back the set; the next invocation recovers an
interrupted transaction. If any affected file has a later foreign edit, recovery
refuses to overwrite it and retains the journal for inspection. A journal cannot
make disk failure recoverable without readable original bytes; retain its receipt
until the transaction completes. No release marker certifies installation or DMG
distribution.

The integrator runs the disposable-history tests:

```sh
python3 -m unittest scripts/tests/test_release_version.py
```

This verifier is included in `make verify`; it never bumps this checkout.

# Compile embargo

Codescribe can temporarily defer four named compile/format gates while a
structural W1 or W2 cut is in progress. The policy lives in the repository: the
marker declares the phase and `scripts/git-hooks/embargo-guard.sh` enforces it.

The embargo is narrow. Only these pre-commit IDs may appear in
`deferred_gates`:

- `cargo-check`
- `cargo-fmt`
- `cargo-clippy`
- `prettier`

Secret detection, merge-conflict and line-ending hygiene, commit-message
provenance, and the pre-push Semgrep scan are never deferred. A malformed
marker fails closed instead of running a deferred command or silently widening
the exception.

## Open W1 or W2

Copy `.vibecrafted/embargo.toml.example` to
`.vibecrafted/embargo.toml`, set `phase` to `W1` or `W2`, and keep
`attestation = "open"`. The recovery ref must be exactly
`embargo/<plan_id>`. The active marker is local/plan state and is ignored by
Git unless an integrator deliberately records it.

For `overlay-canvas-v1`, the complete open marker is:

```toml
plan_id = "overlay-canvas-v1"
phase = "W1"
deferred_gates = ["cargo-check", "cargo-fmt", "cargo-clippy", "prettier"]
attestation = "open"
recovery_ref = "embargo/overlay-canvas-v1"
```

Each wrapped gate derives a process-local `SKIP` value from that list and logs
the phase, exact list, and gate decision to stderr. Hooks are separate
processes, so wrapping each deferrable gate is intentional; a first hook cannot
export environment into later pre-commit hooks.

## Close and remove

At the W2 structural close, change the marker to `phase = "W2"` and
`attestation = "W2_STRUCTURALLY_CLOSED"`. That immediately restores all four
commands while retaining the attestation for recovery. Run the full repository
gates, then remove `.vibecrafted/embargo.toml` when the plan closes. Marker
absence is the ordinary path and executes the original commands unchanged.

Using `--no-verify` is forbidden: a hook/policy conflict is a defect to report,
not an alternate commit path.

## Verify the policy

Run:

```sh
bash scripts/git-hooks/embargo-selftest.sh
pre-commit run --all-files
```

The self-test proves three effects in a temporary repository: active W1 skips
only the four declared gates; private-key and bad-message fixtures still fail
while a Semgrep sentinel executes; and deleting the marker restores every
wrapped command.

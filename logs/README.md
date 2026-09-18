# Workspace layout for runtime evidence

This branch (`dbxms-runtime-claude`) is the working tip for the Acoustic Ledger
/ one-throne transplant. Runtime evidence lands here, not in source.

## logs/

- `logs/transcript-events/` — JSONL event streams from sessions
  (`admit` / `seal` / `refuse` / `replay` receipts).
- `logs/canary-runs/` — output of `scripts/canaries.sh` and
  `scripts/verify-acoustic-throne-structure.py` receipts.
- `logs/receipts/` — structural receipts (`codescribe.acoustic-structure-receipt.v1`).

## Rules

1. No raw PCM, WAV, or M4A in git. Keep audio under
   `~/.vibecrafted/artifacts/` (see `docs/LOCTREE_RESEARCH_TEAM_HANDOFF.md` §15).
2. No secrets, tokens, or bearer material. `Debug` impls are content-free by
   contract.
3. Every committed artifact must be reproducible from a named HEAD + command.
4. `loct` (npm: `loctree` / `@loctree/loct`, binary `loct`) is the only allowed
   executable for structural verification — see
   `scripts/verify-acoustic-throne-structure.py`.

## Install (operator machine, not this sandbox)

```bash
npm i -g loctree          # provides `loct`
# log3 is a Python logger (pip), unrelated to the TS `log3` on npm —
# do not confuse the two.
```

The Grok sandbox here has no outbound pip/npm registry and no `loct`
binary; it reads the repo and receipts, it does not execute the verifier.

# Glass integration checkpoint — 2026-11-09

This is a dated continuation handoff, not a replacement product contract.
Read [Window glass ownership](ADR/2026-11-08-WINDOW_GLASS_OWNERSHIP.md)
before changing materials, reading bounds, titlebar geometry or resize feedback.
Refresh Git, PR and running-build identities before continuing: this snapshot
does not imply that parallel branches have stopped moving.

## Recoverable source and running preview

- Published checkpoint: `codex/glass-integration-checkpoint-20261109`.
- Original integration base: `codex/glass-integration-20261108` at
  `e84a8c890952dde4d1e1e58aef064973949a9dd7`. It stays unchanged because
  other PRs target it; the local integration checkout has continued beyond it.
- Last product change: `ec605c02fa4ddf19b6485d05d12e5116d1f69d61`.
- Physical Agent resize acceptance recorded in `1fbcdf1e6`.
- Silver preview: **Codescribe Readable Floor**, build **3803**, product source
  **`ec605c02f`**. Subsequent documentation commits do not change this identity.
- The preview was built with canonical `make app PROFILE=debug`, signed with
  the stable Developer ID, verified against the previous preview's designated
  requirement, and launched through the native idle-restart protocol. Main,
  debug and FFI loaded-file identities matched the new bundle. The production
  application and previous preview bundle were retained.

## Lineage: pinned inputs, not a claim to include every latest PR

The checkpoint retains the actual merge/cherry-pick history and original
authorship. These input commits are ancestors of the checkpoint:

| Input | Included revision |
| --- | --- |
| Historical global-tip integration | `029fa0142`, followed by `890c04d3d` |
| Bar series, PR #225 | `8b29ff161` |
| Untranslated-key allowlist, PR #226 | `495b156a4` |
| Typed readiness pilot, PR #227 | `1123730d6` |
| Build harness, PR #228 | `657a99afd` |
| Placement menu, PR #229 | `e3b1f5520` |
| Original glass/chrome work | `5feb130a4` |
| Frozen resize single-capture repair | `bac14f5be` |
| Context card, PR #231 | `3b9eef52b` |
| Glass contract follow-up | `7d60cd99c` |
| Agent readable-height repair | `ec605c02f` |

At this checkpoint, `develop` was `7fa1d42c9` and was **not** an ancestor.
Its later merge commits for #226/#227/#228 were not silently integrated. The
local graph has multiple merge bases with `develop`; do not infer a clean
integration or semantic equality from a single three-dot diff.

Fresh PR metadata and Git ancestry checks also showed:

- #225 head `2442f13c3` was not included. Its earlier pinned cut above was.
- #231 head `3b9eef52b` was included but its PR remained open. Inclusion in a
  private candidate is not approval or a GitHub merge of that PR.
- #232 head `529404350` and #233 head `b4b254208` were not included.
- The later historical global-tip revision `441907f3a` was not an ancestor;
  the actual merged parent was `890c04d3d`.
- The separate x9eval permissions/card candidate `f2180a9ae` and screen-atlas
  branch `24dfd602f` have not been admitted into this checkpoint. Their authors'
  reports are handoff inputs, not this integration's verification results.

Do not update the original integration base, close/retarget dependent PRs,
rewrite history, or import later parallel work merely to make this snapshot
appear current. Reconcile each input against the fresh base and review first.

## Accepted Agent bottom-clipping repair

On the previous `4965533bf` preview, read-only native AX geometry on Silver
showed a 640 × 440 pt window containing a 640 × 466 pt split view. The sidebar
extended 26 pt beyond the window bottom and a composer action crossed the
window edge. The Founder confirmed that clipping persisted after release and
after toggling the sidebar twice.

`AgentWindowChrome` had constrained the whole full-size host to 440 pt instead
of reserving the readable floor below native chrome. Programmatic `setFrame`
enlarged the test fixture, initially hiding the incorrect fitting minimum.
The corrected test failed four assertions at `017fbe9db`: 440 versus 492 pt.

`ec605c02f` constrains the app-owned hosting view's **safe-area height** to the
440 pt readable floor. AppKit supplies the titlebar allowance; no 52 pt constant
is added to product sizing. With the current native band, the resulting
fitting minimum is 640 × 492 pt. Removing/restoring only the app-owned minimum
constraints changes fitting size from zero back to the corrected minimum.
Automatic hosting sizing remains disabled; no glass or Markdown path changed.

The Founder then repeated physical shrinking with the sidebar expanded and
collapsed in build 3803 and accepted the result. This issue is closed.

## Verification scope and limits

At product source `ec605c02f`:

- 44 selected Swift tests passed with zero failures: `AgentWindowChromeTests`,
  `CSSidebarColumnTests`, `SettingsWindowChromeTests`, `CSSidebarMaterialTests`
  and `DockPresenceTests`, through canonical `make app-bindings test-swift`.
- `bin/control-codescribe doctor --json`: 7/7 passed.
- `bin/control-codescribe map --check --json`: 6/6 passed.
- `make verify-l10n-catalog verify-l10n-bridge`: passed.
- Changed Swift files passed strict `swift-format` lint; `git diff --check`
  passed. Documentation-only continuation does not re-run compilation.

This is not a full-suite, CI, merge or release pass. The earlier full Swift
suite at `c7004ba7e` retained a failing queued-resize timing case; its isolated
repeat passed, while a later full repeat was terminated without a complete
verdict. Do not convert those receipts into a current full-suite pass.
The broad inherited Rust/build/bus integration requires fresh verification
before merge to current `develop`.

## Remaining work and continuation order

1. Reconcile the draft integration PR and its pinned inputs with current
   `develop` and dependent PRs; preserve this accepted preview as the control.
2. Investigate the large mini → expanded → MIDI tone jump at unchanged settings
   through actual material/backing ownership. Never compensate with opaque
   colour, increased veil opacity, duplicate glass or reading-canvas bleed.
3. Reproduce the reported dense right-header vertical trails on the current
   build before assigning a cause. The selected recording showed the older
   `38698111b` preview. The frame-mutation probe only ruled out frame churn in
   its repeated-update/move scenario, not actual pointer/compositor artifacts.
4. Complete physical fullscreen/native-control/collapsed-header and paired
   Light/Dark, active/inactive optical checks. Agent bottom acceptance does not
   establish acceptance of the entire chrome composition.
5. Finish live overlay/Bus resize smoothness and transition acceptance. Preserve
   frozen foreground capture, live backdrop bounds, single capture, hidden-
   window gates and Markdown performance protections.
6. Review later permissions/card and screen-atlas candidates independently
   before any import. Keep actual application captures distinct from test-host
   rendering and retain the source SHA for every observation.

Native receipts, selected recording frames, galleries, preview manifests and
the complete host/path inventory are retained in the operator artifact handoff.
They are deliberately kept outside product Git; this document carries the
portable source, acceptance and continuation facts needed to resume safely.

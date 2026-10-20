# VC-Frame tab identity

When this conversation runs inside a Vibecrafted vc-frame workspace, the Founder
sees it as a tab in the session list and can jump to it from the Codescribe
agent panel. The tab must name the agent the way the bus does, so the two
surfaces agree: **name, then repository, then harness**, separated by `·`.

```
Ada · codescribe · claude-code
Lena · codescribe · cursor
Roman · vibecrafted · omp
```

Name is the attached bus name, repository is the workspace root's basename,
harness is the provider token used on the attach (`claude-code`, `codex`, `omp`,
`cursor`, `agy`, `junie`, `kimi-code`).

Rename once, right after the attach receipt, as part of connecting. A renamed
tab proves nothing about listening; `listening_verified` still requires a
fresh named utterance.

## Safe recipe

Rename only the tab that owns this conversation. Never `rename-tab` (it targets
the Founder's current focus), never move focus, never touch another agent's tab.

1. Identify your own session and pane from the native context, not from the
   Codescribe channel number. The vc-frame runtime exports them:
   `VC_FRAME_SESSION_NAME` (session) and `VC_FRAME_PANE_ID` (pane). Without
   them you are not inside vc-frame; stop here and say so.
2. Prove the pane → tab mapping:

   ```bash
   vc-frame --session "$VC_FRAME_SESSION_NAME" action list-panes --json --all
   ```

   Pick the record with `is_plugin=false` and `id` equal to your pane. Check
   `pane_command` and `pane_cwd` describe this very process and repository.
   Take `tab_id` from that record. `tab_id` is the stable identifier; it is not
   the tab position and not the pane id.

3. Rename by stable id:

   ```bash
   vc-frame --session "$VC_FRAME_SESSION_NAME" action rename-tab-by-id TAB_ID "NAME · REPO · HARNESS"
   ```

4. Confirm with `vc-frame --session "$VC_FRAME_SESSION_NAME" action list-tabs`.

`vc-frame` lives at `$VIBECRAFTED_VC_FRAME_BIN` when it is not on PATH. If the
pane → tab mapping cannot be proven, report the concrete gap instead of guessing
a tab id.

## Not covered here

Attaching to the bus, the follower and wakeup are in [Attach](attach.md) and
[Monitor](monitor.md). Renaming the tab authorizes no bus operation and no
installation.

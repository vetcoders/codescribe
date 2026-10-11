# Native Junie extension — acceptance evidence, 2026-10-11

## Native mailbox evidence

- Installed Junie CLI: `26.10.5 (3579.5)`.
- Native extension: Claude-compatible marketplace, Junie SessionStart identity,
  skill and MCP tools served by the existing `cs-bus --mcp`. No ACP runner is
  part of this connection.
- Isolated root: `dist/native-junie-proof/bridge`, channel `6`, name
  `junie-native-proof`, real session `session-261011-210940-1plz`.
- Lease: `40691b712a70218aef6b14c0d106446e`.
- Two distinct fresh typed messages: `6dafd0ab2e83a8b61c6d2e86` at
  `2026-10-11T19:11:06Z` and `9f1792bacb606a5b44cfb9b3` at
  `2026-10-11T19:12:46Z`. Native read and explicit ACK completed for both.
- Canonical status records **2 acknowledged markers, backlog 0**. Forwarding
  itself does not create an ACK. The second message arrived 100 seconds after
  the first, through the same session and lease.
- The continuous task keeps a native MCP wait pending during silence; it does
  not depend on terminal stdout wakeup or async hook injection into idle Junie.
- Canonical channel-6 voice handoff is a separate live acceptance: its exact
  delivery, reply and `spoken`/`reason` receipts remain in the Codescribe bus.
  These isolated typed messages are not microphone or speaker evidence.

## Executed gates

- Native MCP/ownership/read/ACK/wait/cancellation/reply/installer tests: **40/40**.
- Canonical read/ACK tests: **26/26**; helper installer tests: **5/5**.
- Signed payload staging/install test passed, including standalone installed
  MCP entry and the non-executable support module.
- App packaging target-root harness: **35/35** scenarios passed.
- Final Semgrep default security pack: **1259 rules, 0 findings** across the
  changed Python, shell and Swift surfaces. Project rules also passed.
- IDE compilation passed without problems; `git diff --check` passed.

## Runtime boundaries

- The extension runs in the Junie conversation that invokes `/codescribe`.
  It does not adopt an unrelated active TUI or create an ACP-controlled chat.
- Explicit stop cancels the native wait and detaches the owning session.
  Ordinary voice answers rearm the wait instead of ending the listening task.
- Marketplace registration/caching belongs to Junie's native manager;
  Codescribe does not recreate its private identity hash.
- Helpers were installed without rebuilding or restarting Codescribe.app.
  A subsequent older app payload must not overwrite the new native helper;
  its integrator must include these source changes or restore the verified
  helper payload after app installation.

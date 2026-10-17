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
- A native MCP wait stays pending during its allowed window. Junie 26.10.5
  imposes a hard **three-minute tool-call deadline**, not configurable through
  `McpServerBean`; progress notifications cannot extend the outer deadline.
  Indefinite zero-model-polling listening is **not established by this transport**.
- Canonical channel-6 voice acceptance used session
  `session-261011-220514-1xgd`. Exact owned delivery:
  `d76ceba2bce984ed45121edd`; reply: `d46c85786a66e78e44a033f6`;
  playback ticket: `0b9c700839ff159f8d0c7a4b`.
- Its canonical playback receipt at `2026-10-11T20:19:41Z` records
  **spoken:true**, voice `xai/aurora`, following a native agent-issued ACK.
  This is actual speaker evidence, not the immutable initial spoken:false row.

## Executed gates

- Native MCP/ownership/read/ACK/wait/cancellation/reply/installer tests: **41/41**.
- Canonical read/ACK tests: **26/26**; helper installer tests: **5/5**.
- Signed payload staging/install test passed, including standalone installed
  MCP entry and the non-executable support module.
- App packaging target-root harness: **35/35** scenarios passed.
- Final Semgrep default security pack: **1259 rules, 0 findings** across the
  changed Python, shell and Swift surfaces. Project rules also passed.
- IDE compilation passed without problems; `git diff --check` passed.
- Live non-terminal startup exposed a missing helper PATH. A new sparse-PATH
  reproducer failed with `FileNotFoundError: cs-bus` before the launcher fix,
  then passed with real helper bytes and without overriding HOME. Native
  session `session-261011-220514-1xgd` resumed with the repaired environment
  and bound canonical channel `6`, lease `8828d41b5dca77cfcf5269b25df48887`.

## Runtime boundaries

- The extension runs in the Junie conversation that invokes `/codescribe`.
  It does not adopt an unrelated active TUI or create an ACP-controlled chat.
- Explicit stop cancels the native wait and detaches the owning session.
- The intended listening loop rearms the wait after answering. The observed
  native task eventually exited with code 0; attachment/follower persistence
  alone is not active delivery. Native wakeup remains a separate requirement.
- Marketplace registration/caching belongs to Junie's native manager;
  Codescribe does not recreate its private identity hash.
- Helpers were installed without rebuilding or restarting Codescribe.app.
  A subsequent older app payload must not overwrite the new native helper;
  its integrator must include these source changes or restore the verified
  helper payload after app installation.

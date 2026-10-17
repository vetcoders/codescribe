# Junie ACP plugin — delivery proof, 2026-10-11

## Measured

- Junie CLI: `26.10.5 (3579.5)`, local installed binary, real ACP process.
- Isolated Codescribe bridge: `dist/junie-plugin-proof`; production user channels
  and microphone were not changed by this proof.
- Session returned by Junie: `session-261011-201915-1kel`.
- Lease: `2d5185306b31c0f70d7b11497c76b319`, channel `6`, name `junie-proof`.
- Three fresh typed bus deliveries: `bf74552f587627cb10f52769`,
  `7e6dc2057b505ef67668d40a`, `6ab1c4529712f7a0164385dc`.
- Each delivery received an explicit Junie-issued ACK and ended with ACP
  `end_turn`: **3/3 read receipts, 3/3 answers**. The first two messages had
  identical text but distinct IDs, and were handled separately.
- Between the second and third deliveries, the plugin was stopped and started
  again. Junie loaded the same session; Codescribe resumed the same lease at
  cursor `1906`. Neither completed task was submitted again.
- The third answer contained `JUNIE_PLUGIN_RESUME_OK`. No prompt was typed into
  Junie to nudge any of these three turns.
- `make install-bus` succeeded, and installed `cs-junie --help` succeeded outside
  any repository-relative import contract. The app was not rebuilt or restarted.

## Automated gates

- Plugin transport/delivery unit tests, canonical read/ACK tests, and helper-only
  installer tests passed.
- Signed-payload staging/install test passed, including the directly installed
  `cs-junie` entrypoint and checksum verification.
- Installer generation test: **7 checks passed**.
- App packaging target-root harness: **35 scenarios passed, 0 failed**.
- Semgrep: **1261 rules run, 0 findings** on the implementation and installation
  surfaces; `git diff --check` passed.

## Not measured

- Fresh microphone utterance and TTS playback. These runs establish automatic
  typed-bus delivery to a real Junie session, not voice acceptance.
- Remote models/providers other than the locally configured Junie installation.
- Injection into an already running interactive Junie conversation. The plugin
  owns a separate ACP-controlled session.

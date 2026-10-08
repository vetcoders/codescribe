# codescribe

Connect a named chat agent to the Codescribe Transcript Bus and prove delivery
into the conversation. The authoritative contract and version are in
[SKILL.md](SKILL.md).

Invoke `/codescribe` in the current chat. This foundation skill has no worker
launcher. A successful attachment includes a fresh voice-triggered reply;
a running tail process is insufficient.

The source package lives in the Codescribe checkout at `skills/codescribe/`.
The app packages it under `Contents/Resources/agent-bridge/skills/codescribe/`;
the product setup installs provider copies. Author the source and synchronize
the intended installed copy. Do not claim a signed app update from editing a
local skill. Vibecrafted is the authoring-standard reference, not a presumed
second owner of this package.

See [FLOW.md](FLOW.md) and [examples](examples/example-prompt.md).

First app launch and `make install-app` install the bundled runtime and expose
`cs-bus` and `cs-say` in `~/.local/bin`. Existing managed client skills update
with the app; choose new clients in Settings → Agent. An unowned skill is kept.
If this shell does not include `~/.local/bin` on PATH, use those two stable full
paths. No private checkout or hook script is required.

To install only helpers and update already selected skills from source, run
`make install-bus`. It uses the same Swift installer without building,
restarting or replacing the app. Reattach the current session afterward; an
existing follower retains its loaded code until reattachment.

`cs-bus --version` and `cs-say --version` report the helper version plus the
installed source commit slug, for example `0.16.0+g1ae953e1`. `.dirty` marks a
payload staged from uncommitted source. The signed manifest retains the full
commit; it is not inferred from the running app's version.

```bash
cs-bus --attach --channel 2 --name lena --provider codex --session <thread-id> --voice eve
cs-say "Jestem na szynie." --provider codex --session <thread-id>
cs-bus --status --provider codex --session <thread-id>
```

Every provider requires an output-notifying `cs-bus --watch` monitor; its default
is a short bell. Keep it active and renew notification windows during tasks.
Codex also uses native queue wakeup after a final answer. Read the current
`--read-pending` batch, immediately ACK only its complete returned IDs, drain to
zero and check once more. Do not act on an obsolete
queue copy. Use `--read-delivery` only for original acoustic details.
Use `--watch --full` only for text diagnostics. A queue receipt is distinct from
an agent ACK.

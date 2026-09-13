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

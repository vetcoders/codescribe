# Native ACP client

The Agent lane can explicitly select an ACP v1 stdio executable instead of
its saved HTTP provider. The ordinary Agent and Max consultation use the same
provider factory and AgentSession. ACP is not a wire alias for Gemini or an
implicit `acp:` model prefix.

Configure the local `settings.json` owner with this shape, replacing the
example paths with real absolute paths:

```json
{
  "schema_version": 3,
  "speech": {
    "assistive": {
      "acp": {
        "program": "/absolute/path/to/agent",
        "args": ["--experimental-acp"],
        "cwd": "/absolute/path/to/workspace"
      }
    }
  },
  "system": {
    "agent_workspace_roots": ["/absolute/path/to/workspace"]
  }
}
```

This is a fragment of the settings schema, not a replacement for the existing
document. Removing `speech.assistive.acp` restores the saved HTTP selection.
The settings loader seals argv, cwd and workspace roots once. ACP requires
explicit absolute program/cwd/root paths without traversal, NUL or implicit
home expansion. It never selects a command from model text or process cwd.
Arguments are literal argv; do not put credentials in them or in settings.
Provision the external agent's credentials through its own supported secure
mechanism. Codescribe does not run an authentication shell or install agents.

The current surface is an explicit local configuration path; there is no new
ACP picker UI in this cut. Portable exports omit executable selection and
imports reject it. Agent reset clears the selection.

Immediately before a real turn, AgentSession binds its existing ToolRegistry,
approval callback, UI channel and execution/thread identity to the provider.
ACP file read/write and terminal creation use the same `read_file`,
`write_file` and `run_process` gate and validators. Unknown permission tools
or missing required approval UI are denied. The bridge also enforces its
sealed canonical roots and checks opened targets again after approval.
Only offered once-only permission options can be selected. Permission
requests without a recognized tool name are refused rather than inferred
from a presentation title or coarse tool kind.

The bridge contains client filesystem RPCs and terminal cwd; it is **not an
OS sandbox for the selected executable or its own tools**. Selecting a local
agent is explicit permission to start that trusted program. Agents are sent
an empty client MCP-server list. This cut does not re-export the host's MCP
credentials or create a second MCP/tool executor.

Every turn owns its child and session terminals. Clean completion is published
after terminal/process teardown and chain publication. EOF, malformed output,
cancellation, refusal and budget exhaustion do not advance a clean chain.
Cancellation attempts pending permission replies and `session/cancel`, then
kills/reaps the dedicated child group. A cancelled remote ACP session cannot
rewind its state: local history is retained by AgentSession and the next real
turn replays it into a fresh session. Session/load replay updates are not
presented as newly generated answer text.

Tool-free disposable Max context preparation is skipped for external agents;
the local history is replayed on the real turn. Direct tool-free classification
calls cannot launch the native ACP provider without an owning AgentSession.
Images require the agent's negotiated prompt image capability.

Protocol references: [ACP v1 tool calls](https://agentclientprotocol.com/protocol/v1/tool-calls),
[session setup](https://agentclientprotocol.com/protocol/v1/session-setup),
and [cancellation](https://agentclientprotocol.com/protocol/v1/cancellation).
This source checkpoint does not assert build, runtime or installed acceptance.

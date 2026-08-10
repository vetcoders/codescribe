# Remote MCP connectors

Codescribe can connect the agent to MCP servers over Streamable HTTP. Remote
and local (`stdio`) servers share the same `~/.codescribe/mcp.json` registry,
tool namespace, and Allow / Ask / Deny policy gateway.

## Add a connector

Open **Settings → Agent → Manage MCP servers**, choose **Remote HTTP**, and
provide:

- a stable server name (for example `slack`);
- the HTTPS MCP endpoint;
- an optional bearer token.

The token field writes directly to macOS Keychain. `mcp.json` stores only an
`auth_ref` account name; it never contains the token. Removing the connector
also removes its Keychain entry.

Equivalent non-secret config shape:

```json
{
  "mcpServers": {
    "slack": {
      "transport": "streamable_http",
      "url": "https://connector.example/mcp",
      "auth_ref": "MCP_CONNECTOR_SLACK_TOKEN",
      "enabled": true
    }
  }
}
```

Do not put `token`, `Authorization`, or bearer values in `mcp.json`.

## Status and permissions

Use **Test** to run the real `initialize` and `tools/list` exchange. The row
shows the advertised server identity, tool count, or a concrete HTTP,
authentication, timeout, or protocol error.

Remote tools appear as `mcp__<server>__<tool>` and keep
`server=<server>` provenance. New remote servers default to **Ask** in the same
Tool Permissions panel used by local MCP tools. A Deny decision is enforced
before the connector handler makes a network call.

## Resilience

Each remote operation is isolated from the surrounding agent session.
Transient failures retry three times with bounded exponential backoff
(250 ms, then 500 ms). If all attempts fail, only that connector degrades and
the tool returns an error; the chat session stays alive. A later call creates a
fresh MCP session and reconnects automatically.

Streamable HTTP JSON responses and `text/event-stream` responses are both
accepted. Codescribe preserves the server-provided `Mcp-Session-Id` across the
initialize, initialized-notification, and tool exchange.

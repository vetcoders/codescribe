# MCP Diagnostics Contract

> What Agent › Diagnostics and Agent › MCP may say about MCP servers, and the
> evidence behind each sentence.

---

## 1. One evidence owner, two ledgers

`app/agent/tools/mcp.rs` owns all MCP connection evidence (`McpEvidence`).
It keeps two separate ledgers, keyed by the `mcp.json` server name:

| Ledger            | Written by                                                  | Proves                                    |
| ----------------- | ----------------------------------------------------------- | ----------------------------------------- |
| Runtime discovery | `register` (agent runtime init, Tools capability listing)   | The agent registered these tools.         |
| Connection test   | `CodescribeMcpAdmin.test_server` → `test_configured_server` | The server answered a one-shot handshake. |

A passed connection test never means the running agent has the tools. Each
entry is pinned to the exact `mcp.json` entry it was gathered against. When
that entry changes (endpoint, command, args, env, timeout, `enabled`, token
reference) or the server is renamed, the old evidence no longer applies and
the server reads as configured until new evidence arrives.

Both evidence kinds are process-local, like the agent registry they describe.

## 2. Server state

Every recorded discovery pass and connection test takes the next value of one
evidence sequence, under the same lock that stores it. "Later" below means a
higher sequence number: the order in which results were recorded, not a clock
and not an assumption that discovery is newer.

For one configured server:

1. `enabled: false` → **Disabled**.
2. Runtime discovery for this entry exists → its outcome is always reported:
   - **Live** (registered tool count). When a connection test recorded later
     failed → **Live, last test failed** (registered tool count + test reason).
     The registered tools are never hidden by a later failed test.
   - **Failed** (reason). When a connection test recorded later passed →
     **Registration failed, last test passed** (registration reason + tested
     tool count). A passing test never turns into a registration.
   - A test recorded before the discovery pass is superseded by it.
3. No runtime discovery → the last connection test decides: **Reachable**
   (tool count, "not registered by the agent yet") or **Unreachable** (reason).
4. Otherwise → **Configured** ("agent not started yet").

Both facts are painted in the row text of Agent › Diagnostics and in the status
column of the MCP server list. PRView keeps reporting registration alone.

## 3. Operator-tool identity (Vibecrafted, AICX, Loctree)

A configured server is taken to be an operator tool when one of these holds:

| Evidence               | Example                                                                                                            |
| ---------------------- | ------------------------------------------------------------------------------------------------------------------ |
| Canonical key          | `"loctree-mcp": {…}`                                                                                               |
| Canonical stdio binary | `"lt": {"command": "/opt/homebrew/bin/loctree-mcp"}`                                                               |
| Advertised identity    | handshake `serverInfo.name` is `loctree` or `loctree-mcp`                                                          |
| Naming convention      | `loctree`, `loctree-http`, `aicx_mcp` — product + one of `mcp`, `http`, `https`, `sse`, `stdio`, `remote`, `local` |

The advertised identity comes from runtime discovery or, failing that, the
last successful connection test of the same entry. The naming convention
applies only while the server has advertised nothing; a server that
advertises another identity is not claimed by its name. Substrings never
count: `aicx-dragon` and `my-loctree` are not identified by name.

When several servers match one tool, the row selects the strongest state
(Live, Reachable, Configured, Failed, Unreachable, Disabled), then explicit
evidence over the naming convention, then the name. The row's subject is the
selected server; its English value names the evidence and lists the other
matches.

When no server matches, the row is **Not configured (optional)** only if every
enabled configured server has a known identity. Otherwise it is **Unverified**
and lists the servers whose identity is unknown; a connection test settles them.

## 4. Readiness is not affected

Operator-tool rows and PRView are informational. Agent readiness is decided by
the core capability gate alone: provider access, native tools, and workspace
root parity. No MCP evidence, absence, or failure changes that verdict.

## 5. Manual check on an installed build

1. Agent › MCP: configure a remote Loctree server under a non-canonical name
   (for example `loctree-http`) and an AICX server whose endpoint is down.
2. Press Test on both. Expect Loctree OK with its tool count and AICX failed
   with the connection reason.
3. Agent › Diagnostics › Refresh. Expect Loctree MCP to name `loctree-http`
   (Live after the agent registered it, otherwise "Connection test passed … not
   registered by the agent yet") and AICX MCP to show the failure, not
   "Not configured (optional)". The readiness pill does not change.
4. Edit the Loctree endpoint and refresh again: the old test no longer applies.

𝚅𝚒𝚋𝚎𝚌𝚛𝚊𝚏𝚝𝚎𝚍. with AI Agents by Vetcoders (c)2024-2026 LibraxisAI

#!/usr/bin/env python3
import json
import sys


MODE = sys.argv[1] if len(sys.argv) > 1 else ""


def emit(payload):
    print(json.dumps(payload, separators=(",", ":")), flush=True)


def tool_definition():
    return {
        "name": "echo",
        "description": "Echo a message through the mock MCP server.",
        "inputSchema": {
            "type": "object",
            "properties": {"message": {"type": "string"}},
            "required": ["message"],
        },
    }


if MODE == "malformed":
    print("{not-json", flush=True)
    sys.exit(0)

for raw_line in sys.stdin:
    if MODE == "silent":
        continue

    message = json.loads(raw_line)
    method = message.get("method")
    request_id = message.get("id")

    if method == "initialize":
        emit(
            {
                "jsonrpc": "2.0",
                "id": request_id,
                "result": {
                    "protocolVersion": "2025-06-18",
                    "capabilities": {"tools": {}},
                    "serverInfo": {"name": "mock-mcp", "version": "0.1.0"},
                },
            }
        )
    elif method == "notifications/initialized":
        continue
    elif method == "tools/list":
        emit({"jsonrpc": "2.0", "id": request_id, "result": {"tools": [tool_definition()]}})
    elif method == "tools/call":
        if MODE == "crash-on-call":
            sys.exit(3)
        params = message.get("params") or {}
        arguments = params.get("arguments") or {}
        text = arguments.get("message", "")
        emit(
            {
                "jsonrpc": "2.0",
                "id": request_id,
                "result": {
                    "content": [{"type": "text", "text": f"echo: {text}"}],
                    "isError": False,
                },
            }
        )
    elif method == "shutdown":
        emit({"jsonrpc": "2.0", "id": request_id, "result": {}})
    elif method == "notifications/exit":
        sys.exit(0)
    else:
        emit(
            {
                "jsonrpc": "2.0",
                "id": request_id,
                "error": {"code": -32601, "message": f"unknown method: {method}"},
            }
        )

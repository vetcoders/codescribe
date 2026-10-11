"""Deterministic ACP wire peer. Not a model or a live delivery proof."""
import json
import sys


def send(value):
    print(json.dumps({"jsonrpc": "2.0", **value}, ensure_ascii=False), flush=True)


for line in sys.stdin:
    request = json.loads(line)
    method = request.get("method")
    identity = request.get("id")
    if method == "initialize":
        send({"id": identity, "result": {"protocolVersion": 1, "agentCapabilities": {"loadSession": True}}})
    elif method in ("session/new", "session/load"):
        send({"id": identity, "result": {"sessionId": "peer-owned-session"}})
    elif method == "session/prompt":
        text = request["params"]["prompt"][0]["text"]
        if text == "permission":
            send({"id": "permission-1", "method": "session/request_permission", "params": {
                "sessionId": "peer-owned-session", "toolCall": {"title": "Run a command"},
                "options": [{"optionId": "allow", "kind": "allow_once", "name": "Allow"}]}})
            response = json.loads(sys.stdin.readline())
            assert response["id"] == "permission-1"
            prefix = "permission=" + response["result"]["outcome"]["outcome"] + "; "
        else:
            prefix = ""
        if text == "foreign":
            send({"method": "session/update", "params": {"sessionId": "another-session", "update": {
                "sessionUpdate": "agent_message_chunk", "content": {"type": "text", "text": "FOREIGN"}}}})
        for chunk in (prefix, "pełna ", "odpowiedź"):
            send({"method": "session/update", "params": {"sessionId": "peer-owned-session", "update": {
                "sessionUpdate": "agent_message_chunk", "content": {"type": "text", "text": chunk}}}})
        send({"id": identity, "result": {"stopReason": "end_turn"}})
    elif method == "exit":
        break
    elif identity is not None:
        send({"id": identity, "error": {"code": -32601, "message": "Unsupported method"}})

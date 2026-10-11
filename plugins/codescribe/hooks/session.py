"""Expose Junie's real identity; never attach, read or ACK in a forwarding hook."""
import json
import sys

payload = json.load(sys.stdin)
session = payload.get("session_id")
if not isinstance(session, str) or not session.strip() or any(ord(c) < 32 for c in session):
    print("Codescribe: native hook did not supply a real session_id; do not invent one.", file=sys.stderr)
    raise SystemExit(1)
context = ("Codescribe native identity for THIS Junie conversation: session_id=" + session + ". "
           "When /codescribe is requested, use exactly this ID with codescribe_connect. "
           "Use codescribe_wait for continuous listening in THIS conversation, not another CLI process. "
           "A received message must be completely read and explicitly ACKed before work.")
print(json.dumps({"hookSpecificOutput": {"hookEventName": payload["hook_event_name"], "additionalContext": context}}))

"""Expose the host conversation's real identity; never attach, read or ACK in a forwarding hook."""
import json
import os
import sys

payload = json.load(sys.stdin)
session = payload.get("session_id")
if not isinstance(session, str) or not session.strip() or any(ord(c) < 32 for c in session):
    print("Codescribe: native hook did not supply a real session_id; do not invent one.", file=sys.stderr)
    raise SystemExit(1)
# Claude Code and Junie load this plugin. Claude Code also exports the current
# session id to every hook; only that agreement identifies it. A Junie started
# from a Claude Code shell inherits a different id and stays Junie.
if os.environ.get("CLAUDE_CODE_SESSION_ID") == session:
    if payload.get("hook_event_name") != "SessionStart":
        # SessionStart re-fires on resume, clear and compact; no per-prompt noise.
        raise SystemExit(0)
    context = ("Codescribe identity for THIS Claude Code conversation: provider=claude-code session_id=" + session + ". "
               "For /codescribe NAME CHANNEL call codescribe_connect with exactly this provider and session_id. "
               "After connecting and after each drained message, end the turn: the Codescribe Stop hook wakes this "
               "conversation when the next message arrives. Read every message completely and ACK it before work.")
else:
    context = ("Codescribe identity for THIS Junie conversation: provider=junie session_id=" + session + ". "
               "When /codescribe is requested, pass exactly this provider and session_id to codescribe_connect. "
               "Use codescribe_wait for reception in THIS conversation during the active task, not another CLI "
               "process; it is not an indefinite idle wakeup. "
               "A received message must be completely read and explicitly ACKed before work.")
print(json.dumps({"hookSpecificOutput": {"hookEventName": payload["hook_event_name"], "additionalContext": context}}))

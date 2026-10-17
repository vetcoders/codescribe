"""Expose the host conversation's real identity; never attach, read or ACK in a forwarding hook."""
import json
import os
import sys

payload = json.load(sys.stdin)
if payload.get("hook_event_name") != "SessionStart":
    # Junie also sends shell commands here without their leading !. Never
    # return prompt context, including when an older hook registration calls us.
    raise SystemExit(0)
session = payload.get("session_id")
if not isinstance(session, str) or not session.strip() or any(ord(c) < 32 for c in session):
    print("Codescribe: native hook did not supply a real session_id; do not invent one.", file=sys.stderr)
    raise SystemExit(1)
# Claude Code and Junie load this plugin. Claude Code also exports the current
# session id to every hook; only that agreement identifies it. A Junie started
# from a Claude Code shell inherits a different id and stays Junie.
if os.environ.get("CLAUDE_CODE_SESSION_ID") == session:
    context = ("Codescribe identity for THIS Claude Code conversation: provider=claude-code session_id=" + session + ". "
               "For /codescribe NAME CHANNEL call codescribe_connect with exactly this provider and session_id. "
               "After connecting and after each drained message, end the turn: the Codescribe Stop hook wakes this "
               "conversation when the next message arrives. Read every message completely and ACK it before work.")
    print(json.dumps({"hookSpecificOutput": {"hookEventName": "SessionStart", "additionalContext": context}}))
else:
    print(json.dumps({"systemMessage": "Codescribe: provider=junie session_id=" + session +
                      ". Share this conversation's /link when connecting; prompt context injection is disabled."}))

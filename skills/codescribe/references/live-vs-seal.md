# Live vs seal

Hold Fn is the event. Release is the seal. Same key as dictation paste.

| Bus status | Agent may |
| --- | --- |
| `session_started` | note that a take began |
| `utterance_draft` / `utterance_revised` | reply in the ~5 s silence gap if the text addresses the bound name |
| `transcript_sealed` | treat as end of event; only now install, kill, commit, delete |

Hearing live ≠ acting live. "James wykasuj tę aplikację" in the middle of a
sentence is not a command.

When no agent is on the demux, Fn is ordinary paste. The bus still writes.

# Live speech, coverage and normal task permissions

Spoken and typed requests follow the same conversation permissions. Do not add
an approval prompt because the acoustic ledger refused coverage, or because a
clear request arrived through the bus. Full Access and no approval retain their
normal meaning. Ask when recognition makes the intended task uncertain.

| Event                   | Meaning                                                                 |
| ----------------------- | ----------------------------------------------------------------------- |
| Draft/revision          | One evolving statement; do not execute every revision again             |
| `transcript_sealed`     | Producer's terminal transcript                                          |
| `coverage: "refused"`   | Words retained without certified acoustic completeness                  |
| `state_change_allowed`  | Preserve the original producer diagnostic exactly                       |
| Native queue acceptance | Provider accepted submission; conversation has not acknowledged reading |
| Revision after a seal   | Update to the same take; not a second command by itself                 |

Fn release, silence, paste success and `session_ended` do not prove a successful
ledger seal. They also do not redefine conversation permissions. Transport
adds no authority beyond the Founder's actual request.

When wording is uncertain, inspect the exact take's audio using the documented
path in [CLI](cli.md). Keep its session identity and disclose ambiguity. A
retranscription is a separate observation, never a reason to repeat completed
work. Replies stay in this conversation and its attached voice.

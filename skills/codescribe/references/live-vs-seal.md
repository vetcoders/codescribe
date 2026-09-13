# Live speech and permission to act

| Observed event                                        | Meaning for this agent                                               |
| ----------------------------------------------------- | -------------------------------------------------------------------- |
| `session_started`                                     | Recording lifecycle began                                            |
| Draft/revision with `state_change_allowed: false`     | May respond conversationally or investigate read-only when addressed |
| `transcript_sealed` with `state_change_allowed: true` | May perform the authorized voice-requested task                      |
| Terminal refusal or `session_ended` without a seal    | No new voice-command execution permission                            |
| Revision after a seal                                 | Not a new sealed command by itself                                   |

Releasing Fn requests closure; it does not guarantee a successful terminal
seal. Follow the emitted envelope, not elapsed silence, UI state, clipboard
delivery, or a successful process exit.

The helper interprets both transcript schemas. Evidence rows can repeat the
whole rendered document across entries; do not treat each row as a new command.
A CLI file verdict is a separate producer/session. Do not describe that result
as proof of successful live recording finalization or silently execute the
same requested operation twice.

Keep replies in this chat. Do not change focus or delivery routing to make the
listener appear functional. The actual hotkey and paste policy belongs to the
current app contracts; attaching an observer does not redefine it.

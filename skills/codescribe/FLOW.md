# Codescribe attach flow

Foundation skill; execute in this conversation.

```mermaid
flowchart TD
    A[Attach requested] --> B[Resolve app, bus, helper, provider session]
    B --> C{Output-notifying bell monitor available?}
    C -->|yes| D[Bind name and one follower lease]
    C -->|no| E[Report limitation; active polling while turn stays open]
    D --> M[Start mandatory watch bell; Codex also arms native queue]
    M --> F[Fresh named take]
    F --> G{Agent receives notification and replies without typed nudge?}
    G -->|yes| H[listening_verified]
    G -->|no| I[Report failing hop; attached_unverified]
    H --> J{Actual request clear and authorized?}
    J -->|unclear| K[Clarify the request]
    J -->|yes| L[Execute once using normal conversation permissions]
```

Recovery preserves the provider session, lease and cursor and rechecks monitor
delivery. Explicit stop closes owned handles. Neither recovery nor an observer
creates a second microphone.

Procedures: [attach](references/attach.md), [monitor](references/monitor.md),
[live vs seal](references/live-vs-seal.md), [CLI](references/cli.md),
[voice reply](references/voice-reply.md).

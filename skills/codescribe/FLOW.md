# Codescribe attach flow

Foundation skill; execute in this conversation.

```mermaid
flowchart TD
    A[Attach requested] --> B[Resolve app, bus, helper, provider session]
    B --> C{Wake-capable monitor available?}
    C -->|yes| D[Bind name and one follower lease]
    C -->|no| E[Report limitation; active polling while turn stays open]
    D --> F[Fresh named take]
    F --> G{Agent receives notification and replies without typed nudge?}
    G -->|yes| H[listening_verified]
    G -->|no| I[Report failing hop; attached_unverified]
    H --> J{Envelope permits state change?}
    J -->|draft or refusal| K[Conversation and read-only work]
    J -->|genuine seal| L[Execute authorized task]
```

Recovery preserves the provider session, lease and cursor and rechecks monitor
delivery. Explicit stop closes owned handles. Neither recovery nor an observer
creates a second microphone.

Procedures: [attach](references/attach.md), [monitor](references/monitor.md),
[live vs seal](references/live-vs-seal.md), [CLI](references/cli.md).

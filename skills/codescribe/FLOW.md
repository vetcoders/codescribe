# `codescribe` attach flow

> Foundation skill. No `vibecrafted codescribe <agent>` worker.

## Flow

```mermaid
flowchart TD
    A[Agent session starts] --> B{Codescribe.app + bus file?}
    B -->|no| C[Ask human: odpal apkę i licencję]
    C --> D{Retry ok?}
    D -->|no| E[Fail loud]
    D -->|yes| F["installed bus-demux --provider --session --become --drafts --follow"]
    B -->|yes| F
    F --> G[Ask human for a name in this chat]
    G --> H[Greet once]
    H --> I["--name stem --follow"]
    I --> J[Fn down: drafts live; state_change_allowed false]
    J --> K{Addresses my name?}
    K -->|yes| L[May reply in ~5s gap]
    K -->|no| J
    J --> M[Fn up: transcript_sealed; state_change_allowed true]
    M --> N[Only now: side effects]
```

## Routes

| Entry         | Args     | Produces                         | Exit          |
| ------------- | -------- | -------------------------------- | ------------- |
| `/codescribe` | none     | agent attached, named, listening | in-session    |
| Worker CLI    | **none** | —                                | do not invent |

### Escalation edges

- Repo surgery after attach → `vc-justdo` / `vc-implement` (not this skill)
- Session orientation of the checkout → `vc-init`
- In-app Agent window → Codescribe Assistive / `⌘⇧Space`, not this skill

### Session artifacts

- Bus: `~/.codescribe/transcript-events.jsonl` (`CODESCRIBE_TRANSCRIPT_BUS_PATH` wins)
- Follower stdout: one JSON object per matching event (kielbasa)
- Lease/cursor: `~/.codescribe/agent-bridge/leases/<lease-id>.json`
- Recovery: preserve/poll the follower handle; reattach with the same provider session

### Anti-patterns

- Fake `vibecrafted codescribe <agent>`
- Second microphone / Voice Lab
- Acting on a half utterance

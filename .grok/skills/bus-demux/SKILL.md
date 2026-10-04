---
name: bus-demux
description: >
  Run scripts/bus-demux.py against codescribe.transcript.v1. Flags: --become,
  --name, --follow, --once. Use when the operator says bus-demux flags, kielbasa
  follower, or needs the CLI. Session attach and naming live in the codescribe
  skill — do not reimplement them here.
---

# Bus demux — CLI

Session attach lives in `codescribe`. This file is flags only.

```bash
python3 scripts/bus-demux.py --become --follow
python3 scripts/bus-demux.py --name james --follow
python3 scripts/bus-demux.py --name james --once
```

Unnamed agents do not pass (exit 2). No microphone. No Lab.

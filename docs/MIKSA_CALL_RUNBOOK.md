# Miksa: internal fixed-call check

The Founder wants full receiving/sending/voice BUS participation. This document
is only a technical check inside the single [membership activation and E2E](MIKSA_BUS_MEMBERSHIP.md),
not an additional product stage or another approval round.

Default `speech_enabled:false` prepares **Moniko, tu Miksa.** without publication,
TTS or playback. `mailbox_enabled:false` exposes only own status and fixed call;
that restricted mode is available for internal discovery checks. It is not
full acceptance. No channel claim or follower is required for an unsolicited
call; an existing mismatched lease refuses it.

When full activation is approved, normal `codescribe_say` is the acceptance path:
use the exact fresh own read/ACK ID, wait for capture end and require matching
owned `spoken:true` plus audible confirmation. Preserve existing profile/vendor,
credentials and capture/playback guards. Inspect an uncertain receipt rather
than submitting a fresh ID to force another speech attempt.

The private Libraxis tunnel/connector now exists. Its live phase is status and
host-only callback preflight; joined receive/send/voice/re-arm acceptance is still
pending. See the current membership runbook for resource and scope facts.

For the implemented reference-only Events storage, failure/reconciliation and
remaining native-backend gate, see [the MCP contract](MIKSA_MCP.md). Active signing
keys must never enter metadata SQLite, config, argv, BUS, diagnostics or chat.

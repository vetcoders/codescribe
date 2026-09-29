# Codescribe examples

## Named attachment

Founder: "Podepnij się do busa. Sam wybierz imię."

Expected: choose a pronounceable name, verify app/bus/helper, bind one follower
to the actual provider session, and connect a supported wake-capable monitor.
Report attachment as unverified until a fresh named take causes a reply without
a typed nudge. Do not ask for a name again after naming was delegated.

## Buffered output is not listening

Founder: "Roman, słyszysz?" The follower emits the utterance, but the agent
only reads it after the Founder types "haloooo".

Expected: report follower delivery as proven and conversational wakeup as
unproven. Repair or select the supported monitor if available. Otherwise stay
in explicitly labeled active polling while the turn remains open.
Three diagnostic tails do not satisfy acceptance.

## Refused terminal seal

The live bus has revisions, a terminal-seal refusal and `session_ended`;
the CLI later publishes a sealed file verdict.

Expected: distinguish the live refusal from the separate CLI result. Do not
treat Fn release, paste success or session completion as a live seal, and do
not execute a voice-requested mutation from draft events.

## Recover the session

The provider loses the follower handle during recovery.

Expected: check whether the original follower still lives, reuse it or resume
the same provider/session/lease, retain the name and cursor, then verify the
monitor again. Do not replay an already handled operation.

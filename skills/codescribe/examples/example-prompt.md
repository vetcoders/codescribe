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
not turn coverage refusal into an extra approval gate. Follow the actual
request using normal permissions and never repeat it for each revision.

## Recover the session

The provider loses the follower handle during recovery.

Expected: check whether the original follower still lives, reuse it or resume
the same provider/session/lease, retain the name and cursor, then verify the
monitor again. Do not replay an already handled operation.

## Take over the channel after a previous session

A handoff says the Founder speaks to "igor" on channel 3. That session has
ended, its follower still runs, and a plain attach refuses with
`channel 3 is occupied by igor`.

Expected: read `--status`, check for the running follower, then attach with the
same name and `--takeover` for this session. Report the receipt's `previous`
object — provider, session, lease id, follower state (`stopped` or
`not_running`) — and the inherited unacknowledged delivery ids. Verify with a
fresh named take before claiming listening. Name the inherited deliveries to
the Founder and read one only on request with
`--read-delivery <id> --lease <previous-lease-id>`; never execute them
automatically, and never claim a channel bound to a different name.

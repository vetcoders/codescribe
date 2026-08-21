# Codescribe voice bridge for Codex

The bridge completes a local voice loop without opening a second microphone:

```text
Fn -> Codescribe.app -> codescribe.transcript.v1 -> named Bus Demux
   -> Codex App Server over stdio -> final agent message -> macOS say
```

Codescribe remains the only microphone owner. The bridge does not scrape the UI,
re-transcribe audio, start Voice Lab, or expose a network listener.

## Current contract

- Live drafts addressed to the bound name may stop playback and call
  `turn/interrupt` on the bridge-owned Codex turn.
- Only `transcript_sealed` becomes a Codex user message.
- One bus `session_id` becomes at most one user message.
- A new command received during a turn interrupts that turn, waits for
  `turn/completed`, then starts a new turn.
- TTS speaks only the completed user-facing agent message. Code fences, raw URLs,
  and Markdown chrome are omitted from speech; the complete answer remains text.
- The bridge always uses `approvalPolicy=never`. Voice cannot approve sandbox
  escapes, network access, credentials, destructive work, push, merge, or release.
- `danger-full-access` is not an accepted CLI option.
- A new bridge starts and owns a dedicated persistent Codex thread. `--thread-id`
  can resume an **idle** thread; an active thread fails closed to avoid two writers.

Interrupt is not rollback. Work completed before the interrupt remains, and a
background process may outlive the interrupted turn. The bridge reports the turn
status but never claims that all effects were undone.

## Run

Prerequisites:

1. Codescribe.app is running and licensed.
2. `~/.codescribe/transcript-events.jsonl` exists.
3. `codex` is authenticated and available on `PATH`.

From the checkout:

```bash
python3 scripts/codex-voice-bridge.py \
  --name james \
  --cwd /path/to/the/repository
```

Then hold Fn and say:

```text
James, sprawdź aktualny branch i powiedz mi, co jest do zrobienia.
```

The bridge prints the created Codex thread id on stderr. The new task is named
`Codescribe voice — james`. Stop the bridge with Ctrl-C.

Useful options:

```bash
# text-only pilot
python3 scripts/codex-voice-bridge.py --name james --cwd . --no-tts

# use a selected local macOS voice
python3 scripts/codex-voice-bridge.py --name james --cwd . --voice Zosia

# resume an idle task explicitly
python3 scripts/codex-voice-bridge.py --name james --cwd . --thread-id <thread-id>

# read-only Codex tools
python3 scripts/codex-voice-bridge.py --name james --cwd . --sandbox read-only
```

## Barge-in

The demux follows live draft/revised events. When an addressed draft arrives
while Codex is working, the bridge:

1. stops local speech playback;
2. sends `turn/interrupt` for the active turn;
3. waits for the terminal turn event;
4. sends the new sealed transcript as the next `turn/start`.

Addressing remains deliberate. Say the bound name when redirecting the coding
agent. Unnamed ordinary Fn dictation continues through the normal Codescribe
route and is not consumed by the bridge.

## Privacy and failure behavior

- Transcript transport is a private mode-0600 local file and stdio JSONL.
- No raw audio is read or stored by the bridge.
- No public or loopback port is opened.
- macOS `say` keeps TTS local. No ElevenLabs key is required for this v1.
- App Server failure, demux exit, or invalid thread ownership stops the bridge
  loudly. It does not silently paste into the focused app.
- The bridge starts at the end of the bus; old transcripts are not replayed.

## Verification

Hermetic protocol and interrupt test:

```bash
bash scripts/tests/bus-demux-test.sh
bash scripts/tests/codex-voice-bridge-test.sh
```

Manual acceptance:

1. Start in `--sandbox read-only --no-tts` mode on a disposable workspace.
2. Send one named Fn take and confirm exactly one user message in the printed
   task id.
3. Start a longer safe request, then make a second named Fn take. Confirm the
   first turn reports `interrupted` before the second turn starts.
4. Enable TTS and verify an addressed draft silences playback immediately.
5. Repeat once with AirPods and once with the MacBook speakers; reject the
   speaker route if TTS is self-transcribed.

## Deliberate limits of v1

- This is a command-line bridge, not a menu-bar control surface.
- It speaks complete final answers, not low-latency sentence streaming.
- It does not approve privileged actions by voice.
- It does not attach to an already active Desktop task.
- It uses local macOS TTS. ElevenLabs TTS is a future backend, not a hidden
  dependency.

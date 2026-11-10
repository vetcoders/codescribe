# Troubleshooting

Preserve the current take, transcript and unsent draft before restarting or
resetting anything. A failed send, an unread message and an unavailable
recognizer are different problems.

## What to include in a report

- App version and build from **Settings → User**; whether it is a nightly.
- Time of the problem, selected recognition mode and microphone.
- The destination: another app, built-in Agent, individual channel, All or inbox.
- What you did, what you expected and what appeared instead.
- A screenshot, and the original audio when a recognition comparison is needed
  and you consent to sharing it.

Review diagnostics before sharing them. They can contain transcript text,
paths, endpoints and other private context. Never send an API key, licence key
or a raw `.env` dump.

## The app does not open

Launch **X9Bus.app** from Applications through Finder or Spotlight. A
standalone `codescribe` terminal command is a CLI utility, not a replacement
for launching the native app bundle.

Confirm which copy and version you are opening, especially after a source
installation. A newer bundle on disk does not prove that an older process has
restarted. Finish active work before quitting normally and reopening it.

For signing, Gatekeeper or source-build problems, use the
[developer installation guide](../INSTALLATION.md). Do not disable system
security or delete local history as a first troubleshooting step.

## Recording will not start

1. Check **Settings → Creator** for Microphone and, for Apple's path, Speech
   Recognition permission.
2. Check the selected input and its effective availability in **Audio**.
3. Check **Dictation → Engine**. Local power also needs a usable model in
   **Dictation → Whisper**; Apple only does not.
4. Check whether an existing recording still owns the microphone. Do not start
   a second recorder or force-kill the app to clear a live take.

A downloaded model, a saved microphone name and a completed setup wizard are
not interchangeable readiness checks. If readiness remains blocked, record the
specific reason shown rather than repeatedly toggling unrelated settings.

## Hotkeys do not work

Check the binding in **Settings → Hotkeys**, including whether it is disabled
or conflicts with another action. Apply the Hotkeys page's Save action after
editing a binding. Check Accessibility and Input Monitoring in macOS settings.

For Fn/Globe, also inspect macOS's own keyboard shortcut assignment. Option and
other double-tap gestures are opt-in; their absence is not a failed install.
After granting a permission, follow any reopen prompt when idle.

## Text is visible but was not pasted

Inspect the recording destination and **Hotkeys → Paste policy**. Safe,
Comfort and Off intentionally differ. Check Accessibility, the receiving app's
insertion point and whether the take was directed to an agent rather than a
text application.

Keep the transcript and copy it manually if needed. Do not change the recognition
engine to repair a delivery problem. Reopening another conversation does not
retroactively redirect a recording that was already started.

## Recognition is inaccurate or text disappears

Compare the final result with the original audio, not only an earlier live
preview. Live hypotheses can change; a successful processing status does not
prove that every word was preserved.

Record the language, recognition mode, app build and exact part of the audio
that differs. Use an available paired take/retranscription action for a
controlled comparison. AI formatting can also change text; compare with
formatting Off when isolating recognition. Do not use formatting to conceal
missing words or assume all repetitions should be deleted.

Keep the original take and diagnostic evidence. A transcript alone cannot
establish whether the microphone, recognizer, formatting or delivery caused
the discrepancy.

## Formatting or the built-in Agent fails

Check **Providers** for the chosen account or key, then **Agent → AI models**
for the function's provider/model. Saving a key is not a successful connection
test. Test the provider using the app's controls and note its actual result.

A model list, an account subscription and API-key access can have different
availability. Do not replace a configured provider with an arbitrary endpoint
or paste credentials into a terminal command for a support report.

Inspect the prompt in **Agent → Prompts** if the service responds but performs
the wrong transformation. Recognition, formatting and tool execution have
separate responsibilities.

## A connected agent is not answering

Check that its named session is attached and that the roster does not show it
as disconnected. Installing a plugin is not the same as attaching a conversation.
An old conversation can remain readable after the session disconnects.

Inspect the recipient selection and delivery state. Published or queued does
not mean read/ACK, and read/ACK does not mean completed work. With a partial
send, preserve the draft and retry the missing recipients.

For voice, inspect microphone destination and speaker mute separately. All
uses its audience rules; it is not the same as individually selecting every
agent. User-directed responses belong in **Your inbox (10)**.

If an agent is attached on another host, verify attachment accessibility there.
A local file path is not automatically a transferred attachment.

## A pasted attachment is missing

Check for the attachment chip in the correct draft. Note whether you pasted
before capture, during capture or after the recording ended. A capture refusal
can keep the draft without starting speech or publishing a message.

Use the offered retry and inspect the result. Opening a preview does not send
an attachment, and seeing a chip does not prove the recipient opened it.
Markup is available only in nightly builds where that action is offered.

## The composer, cursor or scrolling misbehaves

Report the affected surface: main agent view, built-in Agent window or floating
card. Include its size, sidebar/rail state, appearance and app build. Describe
whether typed characters disappear, the pointer flickers, scrolling moves away
from the current reply, or a hover covers an actionable control.

Preserve the draft. These are UI failures, not a reason to discard recordings,
reset provider credentials or treat high CPU as normal.

## High CPU, heat or an unresponsive app

Note whether the app is recording, recognizing, playing speech or simply idle.
Use Activity Monitor to capture the app's resource usage and a process sample;
include the time and build. If you are already recording an Instruments trace,
preserve it and ensure the disk has free space for the output.

A hot Mac or a log that stops after an engine event does not identify the cause
by itself. Keep the relevant logs and take; avoid repeated force-quits or model
changes that destroy the comparison. Quit normally after active work is saved
when the app remains responsive.

## Logs, storage and resets

The local support data is under `~/.codescribe/`. Core tracing is normally in
`data.noindex/logs/codescribe.log`; Swift diagnostics use `diagnostic.log` there.
Review a narrow time range before sharing. See [Privacy & Security](privacy.md)
for recording and transcript storage.

Configure audio retention in **Settings → Audio**. Off affects new takes and
does not erase old recordings; text history is separate. Use the scoped actions
in **Settings → User** for resets and read their confirmation. Do not delete the
whole data directory or empty Trash as a generic repair.

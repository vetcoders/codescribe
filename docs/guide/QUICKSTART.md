# Quick Start

## 1. Install and open the app

Download x9bus from the [website](https://codescribe.vetcoders.io/) or
[GitHub Releases](https://github.com/vetcoders/codescribe/releases). Open the DMG,
copy the app into **Applications**, then launch it from Finder or Spotlight.
You do not need Rust or Xcode to use a packaged app.

Follow the permission checklist in **Settings → Creator**. Microphone access is
needed to record; Apple's recognition path also needs Speech Recognition.
Accessibility and Input Monitoring support delivery and shortcuts. Screen
Recording is used for screenshot context. See the
[installation guide](installation.md#permissions) for the distinction between
setup completion and recording readiness.

## 2. Choose where speech is recognized

Open **Settings → Dictation → Engine**:

- **Apple only** is the fresh-install default. No Whisper download is required.
- **Local power** uses Whisper on your Mac. Open **Dictation → Whisper** to
  download and validate a model first.
- **Cloud** uses an online recognizer and requires audio-egress consent.

A downloaded model is not necessarily loaded, and a selected mode is not proof
that its recognizer is ready. Read the availability shown in Settings.

## 3. Record a short sentence

1. Open a text document and put its insertion point where you want the text.
2. Hold your Dictation shortcut, **Fn/Globe** by default.
3. Say a short sentence, then release the shortcut.
4. Review the result in the overlay and in your document.

Delivery follows the destination and paste policy you selected. If text remains
in the overlay, check **Settings → Hotkeys → Paste policy**; do not assume every
recording must paste automatically. Safe, Comfort and Off have different
behavior. [Recording Modes](modes.md) explains the distinction.

You can also start a recording from the menu bar or overlay and stop it with
its visible Stop control. Confirm the microphone state rather than relying on
an open window or a decorative text marker.

## 4. Decide whether to format the text

**Settings → Creator → Formatting level** offers **Off**, **Correction** and
**Smart** for automatic formatting. **Max** is a per-take request from the
overlay Format menu and uses the Agent model and prompt. Other formatting uses
the provider/model configured for that function. Add credentials in **Providers**, then choose the model in
**Agent → AI models**.

Use Off when you want to inspect recognition without an AI rewrite. Formatting
can change wording; it does not establish whether the recognizer heard every
spoken word correctly.

## 5. Try an agent conversation

Choose the surface you need:

- **Show Agent** opens x9bus's built-in Agent. Configure its provider,
  model and tool permissions before sending a request.
- To talk to an existing coding-agent session, install the x9bus integration
  for that client and ask the session to connect under a name and channel.
  Check that the roster shows it as connected.

In the channel roster, opening an agent changes the conversation you view.
Its microphone control selects a voice destination. Written recipients are
chosen in the composer; Sync can share a draft with several agents. These
choices are explained in [Chat & Overlay](chat-overlay.md).

## If something fails

Keep the transcript or unsent draft. Do not force-quit a recording to clear a
status indicator. Start with [Troubleshooting](troubleshooting.md), and include
the app version/build, time of the problem and the selected mode when reporting
it. Never include API keys or an unreviewed configuration dump.

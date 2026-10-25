# Installation

## System Requirements

| Requirement | Minimum                           | Recommended              |
| ----------- | --------------------------------- | ------------------------ |
| macOS       | 13.0 (Ventura)                    | 14.0+ (Sonoma)           |
| Chip        | Apple Silicon or Intel with Metal | Apple Silicon (M1/M2/M3) |
| RAM         | 8 GB                              | 16 GB                    |
| Disk        | 1 GB                              | 2 GB                     |

---

## Installation Methods

### Option 1: Direct Download (Preferred for end users once a release is published)

1. Go to [Releases](https://github.com/vetcoders/codescribe/releases)
2. Download `Codescribe-x.x.x.dmg`
3. Open DMG, drag Codescribe to Applications
4. Eject DMG

> If Releases is empty, use the source path below. The repository now ships a release workflow, but not every branch has a published tag yet.

### Option 2: Build from Source

```bash
git clone https://github.com/vetcoders/codescribe.git
cd codescribe
make app PROFILE=local-release   # Build an optimized local Codescribe.app
make install-app      # Build + copy app to /Applications
```

---

## First Launch

1. **Open Codescribe** from Applications or Spotlight
2. **Grant Microphone access** when prompted
3. **Grant Accessibility access** in System Settings → Privacy & Security → Accessibility
4. **Wait for initialization** (first launch may take a few seconds to resolve and load the local Whisper model)

You'll see the Codescribe icon appear in your menu bar. It starts black (idle).

The first-run setup wizard has nine steps: interface language, how you want to
work, permissions, dictation language, local Whisper model, AI provider,
shortcuts, agents (only when you choose **Dictation + AI agent**; the
dictation-only path counts eight) and a summary. Every step has the same
layout in one narrow column: the step's icon with its one question or
instruction, a small category label under it (for example **INTERFACE
LANGUAGE** or **OPERATING LANE**), the cards or rows, and a short note only
where it adds something. The step counter and progress bar stay at the top and
**Back** / **Skip** / **Continue** / **Finish** at the bottom. Permissions say
"You can do this later in Settings." — the same checklist, with the same
request and System Settings actions, is in **Settings › Creator ›
Permissions**. The provider step's sentence names only the access the selected
provider has: "account or API key" only where a signed-in account can make
requests (OpenAI and xAI with account sign-in available), otherwise the API key,
or that a key-optional server needs none. On the agents step each client card reads **Install the Codescribe
skill**, or **Codescribe skill installed** once the skill is in that client's
skills folder: Continue installs the skill, it does not join a running agent
session. The summary's heading is **Codescribe is ready.** only when dictation
and, on the agent path, the agent step are both ready; with dictation ready and
the agent not, it reads **Dictation is ready.**

The wizard never downloads the local Whisper model on its own: its model step
shows one card with the model, its install state, its source, its size and
where it is or would be stored; the summary shows the model name and state with
the source and path under **Details**. **Download model** asks to confirm first, naming the size and the
directory the files go to, and the transfer starts only once you confirm.
While it runs, the step shows a progress bar, the file being fetched with the
megabytes done, the rate and the time left, and a **Cancel download** button
that stops the transfer and removes the partial file — the model goes back to
not installed and the download button returns. Continuing without the model is
fine: the cloud and Apple paths do not need it, and a running download finishes
in the background.

---

## Required Permissions

Codescribe needs these permissions to function:

| Permission           | Why                        | How to Grant                                            |
| -------------------- | -------------------------- | ------------------------------------------------------- |
| **Microphone**       | Record your speech         | System Settings → Privacy & Security → Microphone       |
| **Accessibility**    | Global hotkeys, paste text | System Settings → Privacy & Security → Accessibility    |
| **Input Monitoring** | Detect modifier keys       | System Settings → Privacy & Security → Input Monitoring |

> **Tip**: If hotkeys don't work, check that Codescribe is enabled in all three permission categories.

---

## Verify Installation

Open Terminal and run:

```bash
make version
```

Expected output:

```
v0.13.x
```

Test transcription:

1. Launch Codescribe.
2. Start a short Dictation capture with your configured shortcut.
3. Check `make logs` if the menu-bar state reports an error.

---

## Configuration Location

Codescribe stores configuration in:

```
~/.codescribe/
├── .env              # Configuration file
├── prompts/
│   ├── formatting.txt   # AI formatting prompt
│   └── assistive.txt    # Assistive mode prompt
├── transcriptions/   # Saved transcripts
└── data.noindex/     # Constantly rewritten data, kept out of Spotlight
    └── logs/         # Debug logs (codescribe.log, diagnostic.log)
```

Create default config:

```bash
make config
```

---

## Updating

### Manual / Releases

Download new version from Releases and replace the old app.

---

## Uninstalling

### Manual

1. Delete `/Applications/Codescribe.app`
2. Optionally remove config: `rm -rf ~/.codescribe`

---

_Created by Vetcoders (c)2026_

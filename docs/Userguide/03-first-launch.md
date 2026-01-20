# 03 - First Launch

This guide walks you through what to expect when launching CodeScribe for the
first time, including permission prompts, model initialization, and how to
verify everything is working correctly.

---

## Starting CodeScribe

There are two ways to launch CodeScribe:

### From Applications Folder

Double-click **CodeScribe.app** in your Applications folder. The app runs as a
menu bar application - you will not see a Dock icon or main window.

### From Terminal (CLI)

For advanced users or debugging, run directly from terminal:

```bash
./CodeScribe.app/Contents/MacOS/codescribe
```

Or if installed via the CLI binary:

```bash
codescribe
```

You will see startup messages in terminal:

```
CodeScribe daemon starting...
Whisper engine initialized from embedded model (zero I/O)
Initializing system tray...
System tray initialized
Global hotkeys enabled
Starting tray event loop...
```

---

## What Happens on First Launch

### 1. Model Initialization

CodeScribe uses an embedded Whisper model for speech recognition. On first
launch, the model is loaded directly into GPU memory:

- **Release builds**: Model bytes are embedded in the binary - zero disk I/O
- **Debug builds**: Looks for model at `CODESCRIBE_MODEL_PATH` or bundled path

[Screenshot: Terminal showing "Whisper engine initialized from embedded model"]

If you see an error about missing model, run the download script:

```bash
./scripts/download-model.sh
```

### 2. Tray Icon Appears

After initialization, the CodeScribe icon appears in your menu bar (top-right
of screen, near the clock). The icon displays a status indicator:

| Status Dot | Meaning |
|------------|---------|
| Green dot | Ready - waiting for activation |
| Red dot | Recording - listening to your voice |
| Orange dot | Processing - transcribing audio |
| Bright green | Success - transcription complete |
| Red X | Error - backend unavailable |

[Screenshot: Menu bar showing CodeScribe tray icon with green status dot]

### 3. Hotkey Registration

CodeScribe registers global hotkeys using macOS CGEventTap. The default
configuration:

- **Hold Ctrl**: Hold to record, release to transcribe
- **Double-tap Left Option**: Toggle recording on/off (with AI formatting)
- **Double-tap Right Option**: Toggle assistive mode

---

## Permission Dialogs

macOS requires explicit permission for CodeScribe to function. You will see
permission prompts on first launch.

### Accessibility Permission

**Required for**: Global hotkeys (detecting Ctrl/Option key presses)

When prompted:
1. Click "Open System Settings"
2. Navigate to Privacy & Security > Accessibility
3. Enable the toggle for CodeScribe

[Screenshot: macOS Accessibility permission dialog]

If the CGEventTap fails to create, you will see:

```
Failed to create CGEventTap - check Accessibility permission
```

### Microphone Permission

**Required for**: Recording audio for transcription

When prompted:
1. Click "Allow" in the system dialog
2. Or manually enable in Privacy & Security > Microphone

[Screenshot: macOS Microphone permission dialog]

### Input Monitoring (Optional)

Some macOS versions require Input Monitoring for global hotkeys:
1. System Settings > Privacy & Security > Input Monitoring
2. Enable CodeScribe

---

## If You Missed the Permission Prompts

If you accidentally dismissed a permission dialog:

1. Open **System Settings** (or System Preferences on older macOS)
2. Navigate to **Privacy & Security**
3. Enable CodeScribe in these sections:
   - **Accessibility** - for global hotkeys
   - **Microphone** - for audio recording
   - **Input Monitoring** - if hotkeys do not work

After enabling permissions, restart CodeScribe for changes to take effect.

---

## Expected Initial State

When CodeScribe is running correctly:

1. **Tray icon visible** - CodeScribe logo in menu bar with green status dot
2. **Tooltip shows "Ready"** - Hover over icon to see "CodeScribe - Ready"
3. **Menu opens** - Click icon to see the menu with status and options
4. **Hotkeys respond** - Hold Ctrl briefly to test (you should see red dot)

[Screenshot: Tray menu expanded showing "Status: Idle" and options]

---

## How to Know It is Working

### Quick Test

1. Hold the **Ctrl** key for about 1 second
2. Watch the status dot change from green to red (recording)
3. Speak a few words
4. Release Ctrl
5. Status changes to orange (processing), then green (done)
6. Text is automatically pasted at your cursor position

### Check Terminal Output

If running from terminal, you will see:

```
Hold combo activated (Ctrl, assistive=false) - sending Hold Down event
CGEventTap: Recording started
Transcribing...
Transcription time: 1.23s
Hold combo released after 2.5s
```

### Verify Model is Loaded

Click the tray icon and check the status line. It should show:
- "Status: Idle" when waiting
- "Status: Recording..." when Ctrl is held
- "Status: Processing..." during transcription
- "Status: Done!" after successful transcription

---

## Troubleshooting First Launch

| Problem | Solution |
|---------|----------|
| No tray icon appears | Check Console.app for errors, ensure app is signed |
| Hotkeys do not work | Enable Accessibility permission, restart app |
| No audio recorded | Enable Microphone permission, check input device |
| Model not found | Set `CODESCRIBE_MODEL_PATH` or run download script |
| CGEventTap error | Accessibility permission required |

For detailed troubleshooting, see [06-troubleshooting.md](06-troubleshooting.md).

---

## Next Steps

Once CodeScribe is running:

- Learn the [hotkey combinations](04-hotkeys.md)
- Configure [settings and preferences](05-settings.md)
- Explore [AI formatting options](07-ai-formatting.md)

---

*Created by M&K (c)2026 VetCoders*

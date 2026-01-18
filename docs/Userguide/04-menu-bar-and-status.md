# 04 - Menu Bar and Status

CodeScribe lives in your macOS menu bar, giving you quick access to all features
without opening any windows. This guide covers the menu bar icon, status
indicators, and all available menu items.

## Menu Bar Icon Location

The CodeScribe icon appears in the right side of your macOS menu bar, alongside
other system icons like Wi-Fi, battery, and Spotlight. The icon uses the
CodeScribe logo with a colored status indicator in the bottom-right corner.

## Status Indicator Colors

A small colored dot (glyph) appears on the icon to show the current state:

| Color        | Status       | Meaning                              |
|--------------|--------------|--------------------------------------|
| Green        | Idle         | Ready and waiting for input          |
| Red          | Recording    | Actively capturing your voice        |
| Orange       | Processing   | Transcribing or applying AI format   |
| Bright Green | Success      | Transcription complete               |
| Red X        | Error        | Backend unavailable or failed        |

The tooltip also updates when you hover over the icon:

- **CodeScribe - Ready** (Idle)
- **CodeScribe - Recording...** (Listening)
- **CodeScribe - Processing...** (Thinking)
- **CodeScribe - Done!** (Success)
- **CodeScribe - Backend unavailable!** (Error)

## Opening the Menu

Click the CodeScribe icon in the menu bar to reveal the dropdown menu. The menu
provides access to all features without needing to open the main window.

## Menu Structure

The menu is organized into logical sections separated by dividers:

```
Status: Idle
Open GUI...
─────────────
Copy Last to Clipboard
─────────────
Hold Hotkeys          ▸
History               ▸
─────────────
Settings              ▸
─────────────
Help
About
─────────────
Quit
```

## Menu Items Reference

### Status Line

The top line shows the current application state. This is a read-only display
that mirrors the icon's status indicator. It updates in real-time as you
record and process transcriptions.

### Open GUI

Opens the main CodeScribe window. You can also click the dock icon to open the
GUI. The window provides a larger view of your transcription history and
additional settings.

### Copy Last to Clipboard

Instantly copies your most recent transcription to the system clipboard. This
is the quickest way to paste your last dictation into any application.

### Hold Hotkeys Submenu

Configure which modifier keys trigger recording when held down:

| Option                | Keys to Hold        |
|-----------------------|---------------------|
| Ctrl only             | Control             |
| Ctrl+Option           | Control + Option    |
| Ctrl+Shift            | Control + Shift     |
| Ctrl+Command          | Control + Command   |

Additional options in this submenu:

- **Exclusive mode** - When enabled, ignores extra modifier keys. Only the
  exact combination triggers recording.
- **Toggle triggers** - Configure double-tap shortcuts:
  - Left Option (normal) + right Option (assistive)
  - Right Option only (assistive mode)
  - Disable toggles entirely

The "Current:" label at the top shows your active hold key combination.

### History Submenu

Manage your transcription history:

| Item                      | Description                              |
|---------------------------|------------------------------------------|
| Format Last Transcript    | Apply AI formatting to last entry        |
| Format Last 5 Transcripts | Batch format recent entries              |
| Save to History           | Toggle automatic saving (checkbox)       |
| Keep Audio                | Toggle audio file retention (checkbox)   |
| Copy Latest               | Copy most recent transcript              |
| Open Folder               | Open history folder in Finder            |

### Settings Submenu

Access configuration options:

| Item                 | Description                                    |
|----------------------|------------------------------------------------|
| AI Formatting        | Toggle AI post-processing (checkbox)           |
| Edit Config File     | Open config.toml in your default editor        |
| Edit AI Prompt       | Customize the AI formatting instructions       |
| Open Prompts Folder  | Browse available prompt templates              |
| Reset AI Context     | Clear conversation history for fresh context   |

The AI Formatting toggle persists across sessions. When enabled, transcriptions
are automatically enhanced by the AI before being copied to clipboard.

### Help

Opens the CodeScribe documentation in your default web browser.

### About

Displays version information and credits for CodeScribe.

### Quit

Exits CodeScribe completely. The menu bar icon will disappear and all hotkeys
will be unregistered. To restart, launch CodeScribe from Applications or Dock.

## Working Without the Window

You can use CodeScribe entirely from the menu bar:

1. Hold your configured hotkey combination to start recording
2. Release to stop and transcribe
3. Click "Copy Last to Clipboard" or use the automatic clipboard feature
4. Paste into your target application

The status indicator keeps you informed without needing to watch a window.

## Dock Icon Behavior

Clicking the CodeScribe dock icon opens the GUI window. This is equivalent to
selecting "Open GUI..." from the menu bar dropdown.

---

*Created by M&K (c)2026 VetCoders*

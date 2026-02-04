# Chat Overlay (Voice Chat UI)

The Chat Overlay is CodeScribe's command center for voice-driven AI interactions. It provides a split-panel interface separating your voice input from AI responses.

---

## Opening the Chat Overlay

**From menu bar:**

- Click CodeScribe icon → **Show Chat Overlay**

**Automatically:**

- Opens when using Assistive mode (`Ctrl+Shift`)
- Opens when toggle mode sends to AI

**Keyboard shortcut:**

- Configure in Settings (not set by default)

---

## Interface Layout

```
┌─────────────────────────────────────────────────────────────────┐
│ Status: Ready                                        [Collapse] │
├─────────────────────────────────────┬───────────────────────────┤
│                                     │                           │
│  CHAT HISTORY (Left Panel - 60%)    │  RIGHT PANEL (40%)        │
│                                     │                           │
│  ┌─────────────────────────────┐    │  [Transcriptions][Settings]│
│  │ Your message appears here   │◄───│                           │
│  │ with a blue background      │    │  📄 2026-01-22_14-30.txt  │
│  └─────────────────────────────┘    │  📄 2026-01-22_14-25.txt  │
│                                     │  📄 2026-01-22_14-20.txt  │
│         ┌─────────────────────────┐ │                           │
│         │ AI response appears     │ │  [Format] [Copy] [Augment]│
│         │ with a gray background  │ │                           │
│         └─────────────────────────┘ │                           │
│                                     │                           │
│  [Auto] [📎] [Type here...] [Send]  │                           │
├─────────────────────────────────────┴───────────────────────────┤
└─────────────────────────────────────────────────────────────────┘
```

---

## Left Panel: Chat History

### Message Bubbles

Messages appear as chat bubbles:

| Bubble        | Alignment | Color | Content                 |
| ------------- | --------- | ----- | ----------------------- |
| **User**      | Right     | Blue  | Your transcribed speech |
| **Assistant** | Left      | Gray  | AI response             |
| **Error**     | Left      | Red   | Error messages          |

### Streaming Responses

When AI is responding:

1. Pulsing dots appear (`...`)
2. Text streams in character by character
3. "Thinking..." status shown in header

### Input Area

| Element        | Function                                  |
| -------------- | ----------------------------------------- |
| **[Auto]**     | Toggle auto-send (send on recording stop) |
| **[📎]**       | Attach files (not yet implemented)        |
| **Text field** | Type messages manually                    |
| **[Send]**     | Send message to AI                        |

---

## Right Panel: Transcriptions & Settings

### Transcriptions Tab

Lists your recent transcriptions:

- **File list**: Click to select
- **[Format]**: Run AI formatting on selected transcript
- **[Copy]**: Copy transcript to clipboard
- **[Augment]**: Send to AI for expansion

Files are stored in `~/.codescribe/transcriptions/`.

### Settings Tab

Quick access to common settings:

| Setting           | Description                       |
| ----------------- | --------------------------------- |
| **AI Formatting** | Enable/disable AI post-processing |
| **Edit Config**   | Open `.env` file in editor        |
| **Edit Prompt**   | Customize AI prompts              |
| **Reset Context** | Clear AI conversation history     |

---

## Auto-Send Toggle

The **[Auto]** checkbox controls behavior after recording stops:

| State            | Behavior                                                 |
| ---------------- | -------------------------------------------------------- |
| **ON** (default) | Transcript sent to AI automatically                      |
| **OFF**          | Transcript placed in draft, you review and send manually |

**When to disable Auto-Send:**

- You want to review/edit before sending
- You're dictating raw content, not commands
- You want to batch multiple recordings

---

## Workflow Examples

### 1. Quick AI Query (Auto-Send ON)

1. Hold `Ctrl+Shift`, speak: "Explain what a mutex is"
2. Release keys
3. AI response streams into chat overlay
4. Response auto-copied to clipboard (if configured)

### 2. Reviewed Dictation (Auto-Send OFF)

1. Disable Auto-Send checkbox
2. Double-tap `Option`, dictate your email
3. Tap `Option` twice to stop
4. Review transcript in chat overlay
5. Edit if needed, click **[Send]**
6. AI formats your text

### 3. Format Existing Transcript

1. Open Chat Overlay
2. Click **Transcriptions** tab
3. Select a transcript file
4. Click **[Format]** to clean it up
5. Result appears in chat

---

## Collapsing the Right Panel

Click **[Collapse]** button to hide the right panel:

- Chat area expands to full width
- Click again to restore

Useful when:

- Focusing on conversation
- Working on smaller screens
- You don't need transcription history

---

## Keyboard Shortcuts (in overlay)

| Key           | Action                |
| ------------- | --------------------- |
| `Enter`       | Send message          |
| `Shift+Enter` | New line in message   |
| `Escape`      | Close overlay         |
| `Cmd+C`       | Copy selected message |

---

## Status Messages

The header shows current status:

| Status          | Meaning                       |
| --------------- | ----------------------------- |
| Ready           | Idle, waiting for input       |
| Recording...    | Microphone active             |
| Transcribing... | Whisper processing audio      |
| Thinking...     | Waiting for AI response       |
| Streaming...    | AI response in progress       |
| AI Response:    | Response complete             |
| Draft ready     | Transcript waiting for review |
| Error           | Something went wrong          |

---

## Chat History

Messages persist during your session. History is cleared when:

- You close CodeScribe
- You click **Reset Context** in Settings
- You manually clear the conversation

> **Note**: Chat history is session-only. It's not saved to disk between app launches.

---

_Created by M&K (c)2026 VetCoders_

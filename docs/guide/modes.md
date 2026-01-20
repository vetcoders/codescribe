# Recording Modes

CodeScribe offers three recording modes, each optimized for different use cases.

---

## Mode Comparison

| Mode | Hotkey | Speed | AI | Best For |
|------|--------|-------|-----|----------|
| **Raw** | `Ctrl` hold | Fastest | None | Quick notes, code comments |
| **Assistive** | `Ctrl+Shift` hold | Slower | Always | Complex tasks, AI help |
| **Toggle** | `Double Option` | Medium | Optional | Hands-free, long dictation |

---

## 1. Raw Mode (Ctrl Hold)

**The fastest way to dictate.**

### How to Use
1. Hold `Ctrl` key
2. Wait for red badge to appear (~800ms delay)
3. Speak clearly
4. Release `Ctrl`
5. Text appears at cursor

### Characteristics
- **No AI processing** - raw Whisper output
- **Fastest turnaround** - minimal latency
- **Live preview** - see text as you speak in overlay
- **Ignores AI settings** - always raw, even if AI formatting is enabled

### Visual Feedback
| Badge | Meaning |
|-------|---------|
| 🔴 Solid red | Recording active |
| 🟠 Pulsing orange | Processing |
| 🟢 Green flash | Success |

### Why 800ms Delay?
The delay prevents accidental recordings from quick Ctrl taps (common when using keyboard shortcuts). You can customize this in settings.

---

## 2. Assistive Mode (Ctrl+Shift Hold)

**AI-enhanced transcription for complex tasks.**

### How to Use
1. Hold `Ctrl+Shift` together
2. Wait for purple badge to appear
3. Speak your request or content
4. Release keys
5. AI processes and responds

### Characteristics
- **Always uses AI** - regardless of AI formatting toggle
- **Expands content** - AI can elaborate, structure, improve
- **Opens Chat Overlay** - see AI response with formatting
- **Streaming response** - see AI typing in real-time

### Visual Feedback
| Badge | Meaning |
|-------|---------|
| 🟣 Solid purple | Recording (assistive) |
| 🟠 Pulsing orange | AI processing |

### Example Use Cases
- "Write a commit message for adding user authentication"
- "Explain this error and suggest fixes"
- "Create a README for this project"

---

## 3. Toggle Mode (Double Option)

**Hands-free dictation with automatic stop.**

### How to Use
1. Double-tap `Option` key (left or right)
2. Badge starts pulsing (recording active)
3. Speak freely - no keys to hold
4. Either:
   - Double-tap `Option` again to stop, OR
   - Wait 5 seconds of silence (auto-stop)

### Characteristics
- **Hands-free** - no keys to hold while speaking
- **VAD (Voice Activity Detection)** - auto-stops on silence
- **Respects AI settings** - uses formatting if enabled
- **Great for long content** - paragraphs, emails, documentation

### Visual Feedback
| Badge | Meaning |
|-------|---------|
| 🔴 Pulsing red | Recording (toggle mode) |
| 🟣 Pulsing purple | Recording (assistive toggle) |
| 🟠 Solid orange | Processing |

### Variants
- `Double Option` → Normal toggle (respects AI setting)
- `Double Left Option` → Force AI formatting on
- `Double Option + Shift` → Assistive toggle mode

---

## Mode Decision Tree

```
What do you need?
    │
    ├─► Quick raw text, no AI
    │   └─► Hold Ctrl (Raw Mode)
    │
    ├─► AI to help/expand/structure
    │   └─► Hold Ctrl+Shift (Assistive Mode)
    │
    └─► Hands-free, long dictation
        └─► Double-tap Option (Toggle Mode)
            │
            ├─► AI formatting enabled? → Formatted output
            └─► AI formatting disabled? → Raw output
```

---

## Customizing Hotkeys

### Hold Mode Modifiers

Change the hold key combination in menu bar → **Hold Hotkeys**:

| Option | Hotkey |
|--------|--------|
| Ctrl (default) | `Ctrl` |
| Ctrl+Option | `Ctrl+Option` |
| Ctrl+Shift | `Ctrl+Shift` |
| Ctrl+Cmd | `Ctrl+Cmd` |

### Toggle Trigger

Change the toggle key in menu bar → **Toggle Trigger**:

| Option | Hotkey |
|--------|--------|
| Double Option (default) | Tap `Option` twice quickly |
| Double Right Option | Tap right `Option` twice |
| Disabled | Toggle mode off |

### Hold Delay

Adjust the delay before recording starts (default 800ms):

```bash
# In ~/.codescribe/.env
HOLD_START_DELAY_MS=800
```

Lower = faster start but more accidental triggers.
Higher = slower start but fewer accidents.

---

## Transcription Overlay

When recording, a floating overlay appears showing:

1. **Live transcription** - text appears as you speak
2. **Status indicator** - current mode and state
3. **Auto-hide** - disappears after successful paste

The overlay appears near your cursor or in a fixed position (configurable).

---

## Tips for Best Results

1. **Speak clearly** - Whisper handles accents well but clarity helps
2. **Pause between sentences** - helps with punctuation
3. **Use Assistive mode for complex requests** - AI understands context
4. **Check the overlay** - catch errors before pasting
5. **Quiet environment** - reduces transcription errors

---

*Created by M&K (c)2026 VetCoders*

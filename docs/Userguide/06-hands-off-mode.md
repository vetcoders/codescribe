# 06 - Hands-Off Mode

Hands-off mode allows you to dictate without holding any keys. This is ideal for
longer dictation sessions, accessibility needs, or when you prefer a toggle-based
workflow over hold-to-talk.

## How It Works

Instead of holding a key while speaking, you double-tap the Option key to start
recording, speak freely, then double-tap again to stop. The transcript is
automatically processed and pasted into your active application.

**Recording Cycle:**

1. Double-tap Option key (recording starts, pulsing red badge appears)
2. Speak naturally - take your time, pause as needed
3. Double-tap the same Option key (recording stops, processing begins)
4. Text is pasted into your active application

## Default Hotkeys

CodeScribe provides two hands-off modes with different Option keys:

| Hotkey | Mode | Behavior |
|--------|------|----------|
| Double-tap LEFT ⌥ | Normal | Recording with AI formatting (if enabled) |
| Double-tap RIGHT ⌥ | Assistive | Recording with AI augmentation (always on) |

The double-tap window is 450ms - tap twice within this interval to trigger.

### Normal Mode (Left Option)

Double-tapping the left Option key starts a normal hands-off recording session.
This mode respects your AI Formatting toggle in the menu bar:

- **AI Formatting ON**: Your speech is transcribed and polished by AI
- **AI Formatting OFF**: Raw transcript is pasted directly

Use this when you want consistent behavior matching your global preference.

### Assistive Mode (Right Option)

Double-tapping the right Option key activates assistive hands-off mode. This
always applies AI augmentation regardless of your AI Formatting setting.

Assistive mode is designed for:

- Drafting emails or messages that need expansion
- Creating structured content from brief notes
- Getting help with grammar and clarity
- Expanding bullet points into full paragraphs

## Comparing Hold vs Hands-Off

| Feature | Hold Mode (⌃) | Hands-Off Mode (⌥⌥) |
|---------|---------------|---------------------|
| Key action | Hold while speaking | Double-tap to toggle |
| Best for | Quick notes, commands | Longer dictation |
| Hands free | No (key held) | Yes (after activation) |
| Visual feedback | Solid red badge | Pulsing red badge |
| Cancel method | Release quickly | Double-tap again |

## Visual Indicators

During hands-off recording, a **pulsing red badge** appears near the cursor to
indicate active recording. This differs from hold mode which shows a solid badge.

When processing completes, the badge briefly turns orange, then disappears once
text is pasted.

## Customizing Toggle Triggers

You can customize which Option keys trigger hands-off mode in Settings:

**Toggle Trigger Options:**

| Setting | Left ⌥ | Right ⌥ |
|---------|--------|---------|
| `left+right option` (default) | Normal toggle | Assistive toggle |
| `right option only` | Disabled | Assistive toggle |
| `disabled` | Disabled | Disabled |

To change this setting, click the CodeScribe menu bar icon and navigate to
**Toggle Hotkey** submenu.

## Tips for Efficient Dictation

**Preparation:**

- Position your cursor where you want text inserted before activating
- Close unnecessary applications to reduce background noise
- Ensure your microphone is properly positioned

**During Recording:**

- Speak at a natural pace - no need to rush
- Pause between sentences if needed; the AI handles natural breaks
- Say punctuation explicitly if you want precise control ("period", "comma")
- For code dictation, be explicit: "open parenthesis", "close brace"

**Workflow Suggestions:**

- Use Left Option for routine dictation matching your AI preference
- Use Right Option when you want AI to help expand or improve your speech
- Combine with Hold mode: quick commands with Ctrl, longer notes with Option

## Troubleshooting

**Double-tap not detected:**

- Ensure taps are within 450ms of each other
- Check that Accessibility permission is granted to CodeScribe
- Verify toggle trigger is not set to "disabled" in settings

**Wrong mode activated:**

- Left Option = normal, Right Option = assistive
- If only one works, check Toggle Hotkey setting in menu

**Recording doesn't start:**

- Look for the pulsing red badge - if absent, the tap wasn't registered
- Ensure no other modifier keys (Ctrl, Cmd) are held during the double-tap
- Option+Arrow or Option+Letter combos cancel the tap sequence

## Configuration Reference

These settings control hands-off behavior in `~/.codescribe/.env`:

```bash
# Toggle trigger mode: double_option, double_ralt, none
TOGGLE_TRIGGER=double_option

# AI formatting for normal toggle (respects this when enabled)
AI_FORMATTING_ENABLED=true
```

---

*Created by M&K (c)2026 VetCoders*

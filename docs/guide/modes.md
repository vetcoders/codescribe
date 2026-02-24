# Recording Modes

CodeScribe exposes **three work modes**. Each mode has **one shortcut binding** you can customize (or disable) in **Settings → Modes & Shortcuts**.

---

## Mode Comparison

| Mode                 | Default Shortcut            | Auto‑Paste | AI         | Best For                                  |
| -------------------- | --------------------------- | ---------- | ---------- | ----------------------------------------- |
| **Dictation**        | Hold `Fn/Globe`             | ON         | Optional   | Fast dictation into any app               |
| **Formatting**       | Double‑tap `Left Option`    | ON         | Always     | Cleanup/polish of dictated text           |
| **Assistive (Agent)**| Double‑tap `Right Option`   | OFF        | Always     | Questions, transformations, selected text |

Notes:
- **Dictation** runs with or without AI depending on **Settings → Audio & Input → AI Formatting**.
- **Formatting** and **Assistive** always require AI provider config (see **Settings → AI & Prompts**).

---

## 1) Dictation (Hold Binding)

**Fast transcript with auto‑paste.**

### How to Use

1. Hold your Dictation binding (default: hold `Fn/Globe`)
2. Speak
3. Release the key(s)
4. Text is inserted at the cursor in the frontmost app

### Behavior

- Auto‑paste: **ON**
- AI: **optional** (controlled by the AI Formatting toggle)
- Preview: transcription overlay shows live text while you speak

---

## 2) Formatting (Double‑Tap Left Option)

**Hands‑free dictation with an AI formatting pass.**

### How to Use

1. Double‑tap `Left Option` to start
2. Speak normally
3. Pause to auto‑send an utterance (silence boundary)
4. Double‑tap `Left Option` again to stop the session

### Behavior

- Auto‑paste: **ON**
- AI: **always on** (formatting pass)
- UI: transcription overlay during recording; formatted result is pasted to the active app

---

## 3) Assistive (Agent) (Double‑Tap Right Option)

**Voice chat overlay with an AI assistant.**

### How to Use

1. (Optional) Select text in the frontmost app
2. Double‑tap `Right Option` to start
3. Speak your request
4. Pause to auto‑send an utterance (silence boundary)
5. Double‑tap `Right Option` again to stop the session

### Behavior

- Auto‑paste: **OFF** (agent answers in the overlay)
- AI: **always on** (assistive model)
- Selection: best‑effort capture; if selection is present, the agent is instructed to operate **only** on the selected text
- Threads: use **New thread** in the overlay to reset context

---

## Customizing Shortcuts

Open **Settings → Modes & Shortcuts**:

- **Dictation** supports:
  - Hold `Fn/Globe`
  - Hold `Ctrl`
  - Hold `Ctrl+Option`
  - Hold `Ctrl+Shift`
  - Hold `Ctrl+Command`
  - Double‑tap `Ctrl`
  - Disabled
- **Formatting** supports: Double‑tap `Left Option` or Disabled
- **Assistive** supports: Double‑tap `Right Option` or Disabled

Use the built‑in conflict detector if macOS already uses the same shortcut.

---

## Advanced Tuning

- **Hold delay** and **double‑tap interval** are in **Settings → Advanced**.

---

_Created by M&K (c)2026 VetCoders_

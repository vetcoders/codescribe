# Recording Modes

Codescribe exposes **three work modes**. Dictation has **one shortcut binding** you can customize (or disable) in **Settings → Hotkeys**. Formatting and Assistive have no shortcut and start from the menu bar or the overlay: Option types characters, and the right Option double-tap used to start the Agent on its own.

---

## Mode Comparison

| Mode                  | Default Shortcut                                | Auto‑Paste | AI       | Best For                                  |
| --------------------- | ----------------------------------------------- | ---------- | -------- | ----------------------------------------- |
| **Dictation**         | Hold `Fn/Globe`                                 | ON         | Optional | Fast dictation into any app               |
| **Formatting**        | Menu bar **Start transcription** or the overlay | ON         | Always   | Cleanup/polish of dictated text           |
| **Assistive (Agent)** | Menu bar with **Start in Agent mode** on        | OFF        | Always   | Questions, transformations, selected text |

Notes:

- **Dictation** runs with or without AI depending on **Settings → Creator → Formatting level** (Off keeps the raw transcript).
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
- AI: **optional** (controlled by the Formatting level; Off means no AI)
- Preview: transcription overlay shows live text while you speak

---

## 2) Formatting (menu bar or overlay)

**Hands‑free dictation with an AI formatting pass.**

### How to Use

1. Choose **Start transcription** in the menu bar (or start from the overlay)
2. Speak normally
3. Choose **Stop transcription** (or **Stop** in the overlay) to stop and deliver

### Behavior

- Auto‑paste: **ON**
- AI: **always on** (formatting pass)
- UI: transcription overlay during recording; formatted result is pasted to the active app

---

## 3) Assistive (Agent) (menu bar)

**Voice chat with an AI assistant.**

### How to Use

1. (Optional) Select text in the frontmost app
2. In the menu bar, turn on **Start in Agent mode** and choose **Start transcription in Agent mode**
3. Speak your request
4. Choose **Stop transcription in Agent mode** to stop and send it to the Agent

### Behavior

- Auto‑paste: **OFF** (agent answers in the overlay)
- AI: **always on** (assistive model)
- Selection: best‑effort capture; if selection is present, the agent is instructed to operate **only** on the selected text
- Conversations: use **New conversation** in the overlay to reset context

---

## Customizing Shortcuts

Open **Settings → Hotkeys**:

- **Dictation** supports:
  - Hold `Fn/Globe`
  - Hold `Ctrl`
  - Hold `Ctrl+Option`
  - Hold `Ctrl+Shift`
  - Hold `Ctrl+Command`
  - Double‑tap `Ctrl`
  - Disabled
- **Formatting** and **Assistive** have no gesture: their rows say where they start

Use the built‑in conflict detector if macOS already uses the same shortcut.

---

## Advanced Tuning

- **Hold delay** and **double‑tap interval** are in **Settings → Hotkeys**.

---

_Created by Vetcoders (c)2026_

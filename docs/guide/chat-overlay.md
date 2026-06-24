# Chat Overlay (Voice Chat UI)

The Chat Overlay is the live UI for **agent responses**, **streaming**, and **history**.

It has three tabs:

- **Drawer**: searchable history (threads + transcriptions), favorites filter
- **Agent**: chat bubbles + streaming responses
- **Settings**: routes you to the main Settings window / onboarding

---

## Opening the Overlay

- Menu bar icon → **Show Agent**
- Settings → **Audio & Input** to keep the transcription overlay on
- Settings → **Transcription** to tune partial cadence / final-pass behavior
- Automatically opens when you start **Assistive (Agent)** mode

---

## Core Workflow: Assistive (Agent)

Assistive mode uses the **Assistive AI** provider (Settings → AI & Prompts) and shows the response inside the overlay (auto‑paste is OFF).

1. (Optional) Select text in the frontmost app
2. Trigger **Assistive** (default: double‑tap `Right Option`)
3. Speak your request
4. Pause to auto‑send an utterance (silence boundary)
5. Double‑tap `Right Option` again to stop the session

### Selected Text Behavior

If CodeScribe can capture your selection, Assistive is instructed to operate **only on that selected text**. If no selection is captured, it behaves like normal chat.

---

## New Thread (Reset Context)

Use **New thread** in the overlay to start a fresh conversation. This resets UI state and forces a backend runtime boundary so context does not bleed between tasks.

---

## Keyboard Behavior

- Sending typed messages depends on **Settings → Audio & Input → Enter to send**
  - When enabled: `Enter` sends, `Shift+Enter` inserts a newline
  - When disabled: `Enter` inserts newline, `Cmd+Enter` sends

---

_Created by M&K (c)2026 VetCoders_

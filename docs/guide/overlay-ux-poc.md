# Overlay UX (POC)

This branch adds a more “power-user” chat overlay with two tabs: **Agent** and **Drawer**.

## Agent (chat)

- **Voice → chat**: your spoken input appears as a user bubble; the assistant streams its reply into an assistant bubble.
- **Selection context**: if you trigger the “selection” mode while text is selected in another app, the selected text is provided as context to the assistant.
- **Type input**: the bottom text field supports typing. It starts compact and grows only when needed.
- **Attachments (📎)**: attach files as extra context for the assistant.
  - The attachment set is sent **once** per thread (unless you change/clear attachments).
  - **Text files**: only **UTF-8 text** is inlined, with size limits to avoid huge prompts (large/binary files are skipped).
  - **Images** (`png/jpg/webp/gif/...`): sent as **vision input** when the configured LLM endpoint supports `input_image`.
- **Export (↓ icon)**: exports the current Agent thread as Markdown:
  - **All** → _Copy as Markdown_ / _Save as Markdown (to history)_
  - **Assistant only** → _Copy as Markdown_ / _Save as Markdown (to history)_
  - Saved exports go to `~/.codescribe/transcriptions/YYYY-MM-DD/` as `HHMMSS_chat.md` or `HHMMSS_chat-assistant.md`.
- **More menu (…)**: utility actions like starting a new thread and copying/pasting the last response.

## Drawer (history)

- Shows recent transcripts and AI outputs from `~/.codescribe/transcriptions/`.
- Each card has actions: **Copy**, **Edit**, **Delete**, and **Favorite** (♥).
- **Edit** opens the clicked file in **TextEdit**. If TextEdit is already open and nothing seems to happen, fully quit TextEdit and click **Edit** again.
- Search/filter is available at the bottom; favorites-only view can be toggled (♥ in header).

## Tray menu (simplified)

- Everyday actions live at top-level.
- Advanced options (especially hotkeys) are intentionally tucked under **Tools → Advanced…** to reduce cognitive load.

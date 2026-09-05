# Delivery paste walk-through

This is a product witness for overlay Insert. Automated tests prove target
lifecycle, route selection, multiline payload integrity, and clipboard restore;
they do not prove that a real terminal accepted Cmd+V under the Founder's
window-manager and accessibility state.

## TextEdit

1. Put the caret in a disposable TextEdit document and copy a recognizable
   sentinel to the clipboard.
2. Start a dictation that opens the Codescribe overlay and speak two lines,
   including punctuation or shell-looking text such as `$()`.
3. Stop the take, wait for the committed text on the canvas, then choose
   **Insert**.
4. Confirm that the complete text lands at the original caret exactly once and
   that the clipboard sentinel is restored.
5. Inspect the app log for one effect line:

   ```text
   delivery_route: intent=overlay_insert route=clipboard_paste reason=explicit_insert target=TextEdit
   ```

## vc-frame normal and alternate screen

1. Put the cursor in vc-frame's normal terminal input and repeat the take and
   Insert action. Confirm that the whole payload remains editable and no Enter
   is appended.
2. Open vc-frame in its alternate-screen view inside the terminal and place its
   editor caret where a multiline insertion is harmless.
3. Repeat the take and Insert action.
4. Confirm that the terminal emulator handles the payload as one bracketed
   paste: newlines remain editor content and no line is executed as a series of
   synthetic key presses.
5. Repeat once inside zellij. If zellij captures the key/focus transition,
   record the exact binding and terminal; do not add an application-specific
   keystroke workaround to the delivery throne.

## CLI and demux line

- `codescribe transcribe live` follows committed Bus projections through the
  Rust wake/reader path; it does not open a microphone.
- `bus-demux.py` may route those projections to named agents, but does not
  reduce or author transcript text.
- In zsh, source `scripts/codescribe.zsh`, finish a take, then press
  `Ctrl-X Ctrl-V`. The widget reads `codescribe transcribe last`, appends no
  Enter, and leaves the line editable. `codescribe-send-to <tmux-pane>` uses
  literal `tmux send-keys -l` for the same reason.

## Receipt boundary

Keep both product checks below as `[?]` until a Founder performs them on the
installed app:

- text under the cursor in vc-frame alternate screen;
- unchanged clipboard after real Cmd+V delivery.

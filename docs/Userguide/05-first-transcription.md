# 05 - Your first transcription

This chapter walks you through your first voice-to-text experience with CodeScribe.
By the end, you will have spoken a sentence and watched it appear in a text field.

---

## Before you begin

Make sure:
- CodeScribe is running (you see the icon in the menu bar)
- The menu bar icon shows "Idle" status (not "Error")
- You have a text field ready (Notes, Mail, browser search bar, etc.)

If the icon shows an error, check that microphone permissions are granted.
See chapter 03 for setup details.

---

## The hold-to-talk method (Ctrl key)

This is the simplest and most reliable way to dictate.

### Step 1: Position your cursor

Click inside any text field where you want your words to appear.
The cursor should be blinking in that field.

### Step 2: Hold the Control key

Press and hold the **Ctrl** key on your keyboard.
Do not release it yet.

After a brief moment (about 0.8 seconds), you will see:
- A **red dot** appears near your cursor - this is the recording badge
- The menu bar icon changes to show "Recording" status
- If you have sound enabled, you hear a soft "Tink" sound

The delay prevents accidental recordings when you tap Ctrl by mistake.

### Step 3: Speak clearly

While still holding Ctrl, speak your sentence.
For example, say: "Hello, this is my first transcription."

Speak at a normal pace and avoid background noise.

### Step 4: Release the Control key

When you finish speaking, release the Ctrl key.

What happens next:
1. The red dot changes to **orange** (processing indicator)
2. CodeScribe transcribes your audio
3. The text is copied to the clipboard
4. A Cmd+V paste is simulated automatically
5. Your text appears in the text field
6. The orange dot disappears
7. Menu bar returns to "Idle" status

### Step 5: See your text

Your spoken words now appear where your cursor was.
If you said "Hello, this is my first transcription", you should see exactly that.

---

## Example transcription

Here is what a successful first transcription looks like:

**You say:** "The quick brown fox jumps over the lazy dog."

**Result in text field:** The quick brown fox jumps over the lazy dog.

The transcription is in raw mode by default (Ctrl hold).
This means no AI formatting is applied - what you say is what you get.

---

## Visual feedback summary

| Badge Color | Meaning                                    |
|-------------|--------------------------------------------|
| Red (solid) | Recording in hold mode - keep Ctrl pressed |
| Red (pulsing) | Recording in hands-off mode              |
| Orange      | Processing your audio                      |
| Purple      | Assistive mode (AI augmentation active)    |
| No badge    | Idle - ready for next recording            |

The badge follows your cursor position, so you always know where the text will appear.

---

## Where does the text go?

CodeScribe uses a clipboard-paste workflow:

1. Your transcribed text is copied to the clipboard
2. A Cmd+V keystroke is simulated
3. The text pastes into the active text field
4. Your original clipboard content is restored after 200ms

This means you do not lose what was on your clipboard before dictating.

---

## What if it does not work?

### Nothing happens when I hold Ctrl

- Check that CodeScribe is running (menu bar icon visible)
- Make sure you hold Ctrl for at least 0.8 seconds
- Verify the menu bar shows "Idle", not "Error"

### I see the red dot, but no text appears

- Did you release Ctrl after speaking?
- Check that your cursor is in an editable text field
- Look at the menu bar - does it show "Busy" then return to "Idle"?

### The text appears in the wrong place

- Make sure you clicked inside the target text field before holding Ctrl
- The text pastes where your cursor was when you released Ctrl

### The transcription is wrong

- Try speaking more slowly and clearly
- Reduce background noise
- Check your language setting in Settings menu

### I hear no sound feedback

- Sound feedback can be enabled in Settings
- The "Tink" sound is optional and off by default on some setups

---

## Next steps

Now that you have completed your first transcription:
- Try longer sentences and paragraphs
- Experiment with the hands-off mode (chapter 06)
- Explore AI formatting for polished text (chapter 07)

Master the Ctrl hold method first, then explore other modes.

---
*Created by M&K (c)2026 VetCoders*

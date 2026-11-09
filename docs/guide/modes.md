# Recording Modes

Three choices affect a recording: **where speech is recognized**, **how its
text is formatted**, and **where the result is delivered**. Choosing one does
not automatically choose the others.

## Recognition mode

Choose in **Settings → Dictation → Engine**. Changes apply to the next recording.

| Mode        | Recognition            | Preparation                                                        |
| ----------- | ---------------------- | ------------------------------------------------------------------ |
| Apple only  | Apple's on-device path | Speech Recognition and microphone permissions; no Whisper download |
| Local power | A local Whisper model  | Download and validate the model in Dictation → Whisper             |
| Cloud       | An online STT service  | Service configuration and explicit audio-egress consent            |

Apple only is the default for a fresh installation. The selected mode and the
last recognizer that produced a result are different information. Check actual
availability before starting a take.

Developer options can add Apple live observations or acoustic-ledger admission
to other paths. They are not a mandatory sequence through every recognizer.
Full-file retranscription is a separate operation.

## Dictation

Hold the Dictation binding, **Fn/Globe** by default, speak, then release it.
You can change or disable the binding in **Settings → Hotkeys**. A recording
started from a toggle or menu control ends with its corresponding Stop action.

The overlay displays the developing transcript. Live text can be revised while
recognition finishes. Review the delivered text, especially names, numbers and
content inferred from quiet audio.

Delivery follows the explicit target and **Safe / Comfort / Off** paste policy.
An active application alone does not determine every destination, and opening
another conversation does not redirect a recording already in progress.

## Formatting

Choose the automatic level in **Settings → Creator → Formatting level**:

- **Off:** no AI formatting pass.
- **Correction:** restrained correction.
- **Smart:** more structured cleanup.

**Max** is a separate, explicit per-take action in the overlay Format menu.
It uses the Agent model and prompt for the dictated request; it is not an
automatic formatting level.

Choose the formatting provider/model in **Agent → AI models**; account and key
configuration lives in **Providers**. Review custom prompts under **Agent →
Prompts**. Formatting sends text to the configured service when used. It is not
a substitute for checking recognition against the original audio.

Formatting and Assistive shortcuts are opt-in. Double-tap bindings, including
left or right Option where offered, are configured in **Hotkeys**. Check for
conflicts before using the same gesture in another application.

## Built-in Agent / Assistive mode

**Show Agent** opens x9bus's own assistant conversation. Configure its
provider/model, workspace and tools in **Settings → Agent**. You can type,
attach context or speak a request, then explicitly send it.

The menu bar's Agent-mode start action records an assistive request rather than
ordinary text for pasting. Stop ends that take and submits it to the built-in
Agent. A silent assistive take can time out without sending a request; recover
its text from history rather than assuming the agent received it.

Selected text and screenshots are context when explicitly captured. The Agent's
answer stays in its conversation; it is not ordinary dictation pasted into the
frontmost app. Use a new conversation when you want a new context.

## Connected agents and continuous voice

A channel connects an existing external agent session to x9bus. Start its
microphone control to speak to that channel, or use **0 · All** for the shared
audience. Viewing a channel, choosing written recipients and changing the voice
destination are separate actions.

Channel sessions can keep listening between utterances or end after one,
according to **Settings → Agent → AI models**. Speech-pause and completion
settings control when a message is submitted. Do not confuse a quiet interval
with the end of the whole microphone session.

Written **Sync** shares a composer draft among selected agents. It does not open
several microphones. See [Chat & Overlay](chat-overlay.md) for recipients,
attachments, delivery receipts and the personal inbox.

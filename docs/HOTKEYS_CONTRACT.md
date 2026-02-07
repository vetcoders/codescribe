# Hotkeys Contract

> Technical specification for CodeScribe hotkey system.
>
> Created by M&K (c)2026 VetCoders

---

## Overview

CodeScribe uses a low-level CGEventTap to detect modifier-only keypresses on macOS.
This approach avoids TSMGetInputSourceProperty crashes on macOS 26.2+ (Sequoia).

```mermaid
flowchart TB
    subgraph Input["🎹 Input Layer"]
        CGEventTap["CGEventTap<br/>(kCGEventFlagsChanged)"]
    end

    subgraph Detection["🔍 Event Detection"]
        HoldGesture["check_hold_gesture()"]
        ToggleGesture["check_toggle_gesture()"]
    end

    subgraph Events["📨 HotkeyInput"]
        HoldEvent["Hold { Down/Up, hold_mode }"]
        ToggleEvent["ToggleNormal / ToggleAssistive"]
    end

    subgraph Controller["🎛️ RecordingController"]
        Handler["handle_hotkey_event()"]
        StateMachine["State Machine"]
    end

    CGEventTap --> HoldGesture
    CGEventTap --> ToggleGesture
    CGEventTap --> ConvGesture

    HoldGesture --> HoldEvent
    ToggleGesture --> ToggleEvent
    HoldEvent --> Handler
    ToggleEvent --> Handler

    Handler --> StateMachine
```

---

## Modes

### 1. Hold Mode (Push-to-Talk)

**Trigger:** Press and hold configured modifier combo
**Behavior:** Recording starts on key down, stops on key up
**VAD:** DISABLED - user has 100% control via key release

| Config                 | Keys          | Use Case                          |
| ---------------------- | ------------- | --------------------------------- |
| `HOLD_MODS=fn`         | Fn            | **Default** (best for terminals)  |
| `HOLD_MODS=ctrl`       | Ctrl          | Legacy / terminal-heavy users     |
| `HOLD_MODS=ctrl_alt`   | Ctrl+Option   | Legacy power-combo preset         |
| `HOLD_MODS=ctrl_shift` | Ctrl+Shift    | Assistive always (legacy)         |
| `HOLD_MODS=ctrl_cmd`   | Ctrl+Command  | macOS power users (legacy)        |

**Events:**

```rust
HotkeyInput { key_type: Hold, action: Down, hold_mode: Raw }         // Fn only
HotkeyInput { key_type: Hold, action: Down, hold_mode: Chat }        // Fn+Shift
HotkeyInput { key_type: Hold, action: Down, hold_mode: Selection }   // Fn+Cmd
HotkeyInput { key_type: Hold, action: Up,   hold_mode: <current> }   // Release
```

**Mode modifiers (default Fn):** Shift → Chat, Cmd → Selection (while holding Fn).

---

### 2. Toggle Mode (Hands-Free)

**Trigger:** Double-tap Option key within `DOUBLE_TAP_INTERVAL_MS` (default **200ms**, range 100–450ms)
**Behavior:** First tap starts recording, second tap toggles send/stop
**VAD:** ENABLED – auto‑sends on `TOGGLE_SILENCE_SEC` of silence (default 5s) without stopping recording

| Config                         | Keys                                           | Mode            |
| ------------------------------ | ---------------------------------------------- | --------------- |
| `TOGGLE_TRIGGER=double_option` | Left Option = normal, Right Option = assistive | Default         |
| `TOGGLE_TRIGGER=double_lalt`   | Left Option only                               | Minimal         |
| `TOGGLE_TRIGGER=double_ralt`   | Right Option only (assistive)                  | Minimal         |
| `TOGGLE_TRIGGER=none`          | Toggle disabled                                | Hold-only users |

**Events:**

```rust
HotkeyInput { key_type: Toggle, action: Press, assistive: false } // Left Option
HotkeyInput { key_type: Toggle, action: Press, assistive: true }  // Right Option
```

---

### 3. Conversation Mode (Moshi Full‑Duplex) — experimental

Conversation mode exists in the controller, but **has no default hotkey binding** in the current release.
If you wire a custom trigger, it runs full‑duplex audio (mic → Moshi → speaker) and uses Moshi’s internal
turn‑taking. Requires Moshi models at `~/.codescribe/models/moshiko-q8/`.

---

## State Machine

```mermaid
stateDiagram-v2
    [*] --> IDLE

    IDLE --> REC_HOLD : Hold Down<br/>(Fn pressed)
    IDLE --> REC_TOGGLE : Toggle<br/>(Double-tap Option)
    IDLE --> CONVERSATION : Conversation Down<br/>(custom binding)

    REC_HOLD --> BUSY : Hold Up<br/>(Fn released)
    REC_HOLD --> REC_HOLD : Shift pressed<br/>(upgrade to assistive)

    REC_TOGGLE --> BUSY : Toggle again

    CONVERSATION --> IDLE : Conversation Up

    BUSY --> IDLE : Processing complete<br/>(paste to app)

    note right of REC_HOLD
        VAD: DISABLED
        User controls via key release
    end note

    note right of REC_TOGGLE
        VAD: ENABLED
        Utterance boundary on silence (no stop)
    end note

    note right of CONVERSATION
        VAD: Internal (Moshi)
        Full-duplex audio
    end note
```

**States:**

- `IDLE` - Waiting for hotkey
- `REC_HOLD` - Recording (hold mode, no VAD)
- `REC_TOGGLE` - Recording (toggle mode, VAD active)
- `BUSY` - Processing transcription/AI formatting
- `CONVERSATION` - Moshi full-duplex active

---

## VAD Behavior Contract

```mermaid
flowchart LR
    subgraph HoldMode["🎯 HOLD Mode"]
        H_VAD["VAD: ❌ OFF"]
        H_Control["User controls via<br/>key release"]
    end

    subgraph ToggleMode["👐 TOGGLE Mode"]
        T_VAD["VAD: ✅ ON"]
        T_Silero["Silero Neural VAD"]
        T_Config["CODESCRIBE_VAD_SILENCE_SEC"]
    end

    subgraph ConvMode["💬 CONVERSATION Mode"]
        C_VAD["VAD: 🔄 Internal"]
        C_Moshi["Moshi turn-taking"]
    end

    H_VAD --> H_Control
    T_VAD --> T_Silero
    T_Silero --> T_Config
    C_VAD --> C_Moshi

    style H_VAD fill:#ffcccc
    style T_VAD fill:#ccffcc
    style C_VAD fill:#cce5ff
```

| Mode             | VAD Segmentation | Reason                                                             |
| ---------------- | ---------------- | ------------------------------------------------------------------ |
| **Hold**         | ✅ YES           | VAD segments utterances; user controls start/stop via key release. |
| **Toggle**       | ✅ YES           | Hands-free mode uses utterance boundaries (no stop).               |
| **Conversation** | Internal         | Moshi handles turn-taking internally.                              |

---

## Environment Variables

### Hotkey Configuration

| Variable              | Default         | Options                                        | Reload  |
| --------------------- | --------------- | ---------------------------------------------- | ------- |
| `HOLD_MODS`           | `fn`            | `fn`, `ctrl`, `ctrl_alt`, `ctrl_shift`, `ctrl_cmd` | RESTART |
| `HOLD_EXCLUSIVE`      | `true`          | `true`, `false`                                | RESTART |
| `TOGGLE_TRIGGER`      | `double_option` | `double_option`, `double_lalt`, `double_ralt`, `none` | RESTART |
| `HOLD_START_DELAY_MS` | `800`           | 0-1000                                         | RESTART |
| `DOUBLE_TAP_INTERVAL_MS` | `200`        | 100-450                                        | RESTART |
| `TOGGLE_SILENCE_SEC`  | `5.0`           | 0.5-10.0                                       | RESTART |

### VAD Configuration

| Variable                     | Default | Range    | Description                       |
| ---------------------------- | ------- | -------- | --------------------------------- |
| `CODESCRIBE_VAD_THRESHOLD`   | `0.5`   | 0.1-0.95 | Speech probability threshold      |
| `CODESCRIBE_VAD_SILENCE_SEC` | `1.2`   | 0.1-10.0 | Silence before utterance boundary |

---

## Event Flow

### Hold Mode (Push-to-Talk)

```mermaid
sequenceDiagram
    autonumber
    participant User
    participant CGEventTap
    participant HotkeyDetector
    participant Controller as RecordingController
    participant Recorder as StreamingRecorder
    participant Whisper
    participant App as Active App

    User->>CGEventTap: Press Fn
    CGEventTap->>HotkeyDetector: kCGEventFlagsChanged
    HotkeyDetector->>HotkeyDetector: check_hold_gesture()
    HotkeyDetector->>Controller: HotkeyInput { Hold Down, Raw }

    rect rgb(200, 255, 200)
        Note over Controller: State: IDLE → REC_HOLD
        Controller->>Recorder: start()
        Recorder->>Whisper: Audio chunks (streaming)
        Whisper-->>Controller: Live transcription deltas
    end

    User->>CGEventTap: Release Fn
    CGEventTap->>HotkeyDetector: kCGEventFlagsChanged
    HotkeyDetector->>Controller: HotkeyInput { Hold Up, <current> }

    rect rgb(255, 230, 200)
        Note over Controller: State: REC_HOLD → BUSY
        Controller->>Whisper: Finalize transcription
        Whisper-->>Controller: Final text
        Controller->>Controller: AI formatting (optional)
        Controller->>App: Paste via CGEvent
        Note over Controller: State: BUSY → IDLE
    end
```

### Toggle Mode (Hands-Free)

```mermaid
sequenceDiagram
    autonumber
    participant User
    participant CGEventTap
    participant HotkeyDetector
    participant Controller as RecordingController
    participant VAD as Silero VAD
    participant Whisper

    User->>CGEventTap: Double-tap Left Option
    CGEventTap->>HotkeyDetector: kCGEventFlagsChanged (x4)
    HotkeyDetector->>HotkeyDetector: check_toggle_gesture()<br/>Press→Release→Press→Release < DOUBLE_TAP_INTERVAL_MS
    HotkeyDetector->>Controller: HotkeyInput { Toggle, assistive=false }

    rect rgb(200, 255, 200)
        Note over Controller: State: IDLE → REC_TOGGLE
        loop Recording with VAD
            VAD->>VAD: Monitor speech probability
            VAD-->>Recorder: Utterance boundary on silence
        end
    end

    alt User double-taps again (stop)
        User->>HotkeyDetector: Double-tap Option
        HotkeyDetector->>Controller: ToggleNormal
        Note over Controller: State: REC_TOGGLE → BUSY
        Controller->>Whisper: Finalize + format
        Note over Controller: State: BUSY → IDLE
    else Silence > TOGGLE_SILENCE_SEC
        VAD->>Recorder: Utterance boundary (auto-send, recording continues)
        Note over Controller: State stays REC_TOGGLE
    end
```

### Conversation Mode (Moshi Full‑Duplex)

Conversation mode is available in the controller but **not bound to a default hotkey** in the current release.
Wire it manually if you need full‑duplex audio (mic → Moshi → speaker).

---

## Implementation Notes

### CGEventTap (macOS)

```rust
// We ONLY read CGEventFlags - no keyboard layout queries
let flags = CGEventGetFlags(event);
let ctrl = (flags & kCGEventFlagMaskControl) != 0;
let alt = (flags & kCGEventFlagMaskAlternate) != 0;
// etc.
```

**Why:** TSMGetInputSourceProperty (used by rdev/global-hotkey) crashes on macOS 26.2+ when called from event tap callback thread.

### Double-Tap Detection

```mermaid
sequenceDiagram
    participant User
    participant Detector as HotkeyDetector
    participant State as TapState

    Note over User,State: DOUBLE_TAP_INTERVAL_MS = 200

    User->>Detector: Option DOWN (t=0ms)
    Detector->>State: first_tap_time = now()

    User->>Detector: Option UP (t=50ms)
    Detector->>State: waiting_second_tap = true

    User->>Detector: Option DOWN (t=180ms)
    Detector->>State: Check: 180ms < 200ms ✓

    User->>Detector: Option UP (t=250ms)
    Detector->>Detector: TRIGGER! ToggleNormal

    Note over Detector: Only SECOND release<br/>triggers the event
```

```rust
const DOUBLE_TAP_INTERVAL_MS: u64 = 200;

// Sequence: Press → Release → Press → Release (within interval)
// Only the SECOND release triggers ToggleNormal/ToggleAssistive
```

### Exclusive Mode

When `HOLD_EXCLUSIVE=true` (default):

- Option taps are ignored if Option is part of an unrelated hold combo
- Prevents accidental toggle while trying to hold legacy Ctrl-based combos

---

## Troubleshooting

| Symptom                      | Cause                           | Fix                                                           |
| ---------------------------- | ------------------------------- | ------------------------------------------------------------- |
| Hotkeys don't work           | Accessibility permission denied | System Settings → Privacy → Accessibility → Enable CodeScribe |
| Double-tap too sensitive     | Interval too short              | Increase `DOUBLE_TAP_INTERVAL_MS` (100–450ms)                 |
| Recording won't stop (hold)  | Key stuck in system             | Release all modifiers, try again                              |
| VAD cuts utterance too early | Threshold too high              | Lower `CODESCRIBE_VAD_THRESHOLD`                              |

---

## File Locations

| File                               | Purpose                              |
| ---------------------------------- | ------------------------------------ |
| `app/os/hotkeys.rs`                | CGEventTap listener, event detection |
| `app/controller/mod.rs`            | State machine, event handling        |
| `app/controller/types.rs`          | State enum                           |
| `core/vad/config.rs`               | VAD configuration                    |
| `core/audio/streaming_recorder.rs` | Silero VAD segmentation              |

---

_Copyright © 2024–2026 VetCoders_

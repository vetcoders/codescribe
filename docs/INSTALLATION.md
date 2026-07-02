# Codescribe Installation and Launch Guide

This document describes the installation methods, configuration paths, and how the application locates its resources.

## Installation Methods

### Method 1: App Bundle From Source (Recommended for Development)

```bash
# Build the SwiftUI app bundle
make app PROFILE=release

# Build and copy to /Applications/Codescribe.app
make install-app
```

**Result**: App bundle installed at `/Applications/Codescribe.app`, with model/cache checks handled by `scripts/build-app.sh`.

**How it runs**: Launch from Finder, Spotlight, or `make start`.

### Method 2: Qube CLI Tools (Batch Quality Work)

```bash
make release-qube
make install
```

**Result**: `qube-report` and `qube-daemon` installed from `bin/qube_report.rs` and `bin/qube_daemon.rs`.

**How it runs**: Terminal-only quality/reporting utilities, not the user-facing app.

`make install-app` now prefers a stable local signing identity automatically:

- `Apple Development: ...` if present
- otherwise `Developer ID Application: ...`
- only falls back to `adhoc` when no usable signing identity exists

This matters because macOS TCC permissions are far more stable with a persistent code-signing identity than with ad-hoc signatures.

### Method 3: DMG Distribution (For End Users)

```bash
make dmg-signed       # Build signed DMG
make notarize         # Notarize with Apple (requires Developer ID)
# or one-shot:
# make release-dmgs    # Build + sign + notarize standard and full DMGs
```

**Result**: `Codescribe_X.Y.Z.dmg` and `Codescribe_X.Y.Z_full.dmg` ready for distribution. The standard DMG embeds Silero + embedder and resolves Whisper from cache/download. The full DMG embeds Silero + embedder + Whisper.

## Configuration

### Config Directory

Configuration is **tiered**:

```
~/Library/Application Support/Codescribe/
├── settings.json     # GUI-managed settings (regular-user tier)
└── ...               # app data

~/.codescribe/
├── .env              # Power-user overrides (optional)
├── prompts/          # Custom AI prompts
│   ├── formatting.txt
│   └── assistive.txt
├── history/          # Transcription history
├── reports/          # Quality reports
└── repo_path         # Path to source repo (set during install)
```

**Secrets** (API keys) are stored in **macOS Keychain** under service `com.vetcoders.codescribe`.

### Environment Variables (.env)

The application loads configuration with these priorities:

1. **Environment variables** (highest priority)
2. **~/.codescribe/.env** (power-user overrides)
3. **settings.json** (GUI-managed defaults)
4. **Built-in defaults** (fallback)

```mermaid
flowchart TD
    A[Application Start] --> B{Check ENV vars}
    B -->|Set| C[Use ENV value]
    B -->|Not set| D{Check ~/.codescribe/.env}
    D -->|Exists| E[Load with dotenvy]
    D -->|Missing| F[Skip .env]
    E --> KC[Load Keychain secrets]
    F --> KC
    KC --> S[Load settings.json]
    S --> K[Apply defaults for missing keys]
    C --> L[Config Ready]
    K --> L
```

### Key Configuration Variables

```env
# Speech-to-Text
WHISPER_LANGUAGE=auto            # auto | pl | en
USE_LOCAL_STT=1                  # 1 = keep local transcript as committed result

# Hotkeys timing / behavior
# Per-mode bindings live in Settings -> Modes & Shortcuts (settings.json)
HOLD_EXCLUSIVE=1
DOUBLE_TAP_INTERVAL_MS=200       # 100–450
TOGGLE_SILENCE_SEC=5.0

# AI Formatting
AI_FORMATTING_ENABLED=1
LLM_ENDPOINT=https://api.openai.com/v1/responses
LLM_MODEL=gpt-4.1
LLM_API_KEY=sk-xxx

# Optional: Mode-specific OpenAI overrides
LLM_FORMATTING_{ENDPOINT,MODEL,API_KEY}=...
LLM_ASSISTIVE_{ENDPOINT,MODEL,API_KEY}=...
```

## Bundle Structure

```
Codescribe.app/
└── Contents/
    ├── Info.plist           # Bundle metadata (icon, identifier, version)
    ├── MacOS/
    │   └── Codescribe       # App executable
    └── Resources/
        └── AppIcon.icns     # Application icon
```

### Info.plist Keys

| Key                          | Value                 | Purpose                      |
| ---------------------------- | --------------------- | ---------------------------- |
| CFBundleIdentifier           | com.vetcoders.codescribe | Unique app identifier     |
| CFBundleIconFile             | AppIcon               | Points to AppIcon.icns       |
| CFBundleExecutable           | Codescribe            | Main binary name             |
| LSMinimumSystemVersion       | 14.0                  | Requires macOS Sonoma+       |
| NSMicrophoneUsageDescription | ...                   | Microphone permission prompt |

## Icons

### Tray Icon

- **Source**: `assets/icon.png` (embedded via `include_bytes!`)
- **Location in code**: `src/tray/icons.rs`
- **Size**: 44x44 pixels (Retina), 22x22 logical

### Dock Icon

- **For CLI**: Programmatically set via `set_dock_icon()` in `src/ui.rs`
- **For Bundle**: Uses `CFBundleIconFile` from Info.plist pointing to `AppIcon.icns`
- **Source**: `assets/AppIcon.icns`

### Icon Loading Flow

```mermaid
flowchart LR
    subgraph CLI["CLI Mode (codescribe)"]
        A1[Start] --> A2[set_dock_icon]
        A2 --> A3[NSImage from include_bytes]
        A3 --> A4[setApplicationIconImage]
    end

    subgraph Bundle["Bundle Mode (.app)"]
        B1[Start] --> B2[macOS reads Info.plist]
        B2 --> B3[CFBundleIconFile = AppIcon]
        B3 --> B4[Load AppIcon.icns from Resources]
    end

    subgraph Tray["Tray Icon (both modes)"]
        C1[Tray init] --> C2[load_custom_icon]
        C2 --> C3[include_bytes icon.png]
        C3 --> C4[tray_icon::Icon]
    end
```

## Permissions Required

Grant in **System Settings > Privacy & Security**:

| Permission       | Purpose                | When Prompted           |
| ---------------- | ---------------------- | ----------------------- |
| Microphone       | Audio recording        | First recording attempt |
| Accessibility    | Global hotkeys, paste  | First hotkey press      |
| Input Monitoring | Keyboard event capture | First hotkey press      |

## Troubleshooting

### Empty Dock Icon

- **CLI mode**: `set_dock_icon()` should set it programmatically
- **Bundle mode**: Check that `Info.plist` exists and has `CFBundleIconFile`
- **Verify**: `plutil -lint /Applications/Codescribe.app/Contents/Info.plist`

### Empty Tray Icon

- Check that `assets/icon.png` exists and is valid PNG
- Rebuild with `make app PROFILE=release`

### Config Not Loading

- Check `~/.codescribe/.env` exists
- Verify syntax: `cat ~/.codescribe/.env`
- Check logs: `make logs`

### Hotkeys Not Working

- Grant Accessibility permission
- Grant Input Monitoring permission
- Restart the application after granting

---

_Created by vetcoders (c)2026_

# CodeScribe Installation and Launch Guide

This document describes the installation methods, configuration paths, and how the application locates its resources.

## Installation Methods

### Method 1: CLI Install (Recommended for Development)

```bash
make download-model   # Download Whisper model (required for embedding)
make install          # Build and install to ~/.cargo/bin/
```

**Result**: Binary `codescribe` installed to `~/.cargo/bin/` (~888MB with embedded model).

**How it runs**: Direct execution from terminal or as background daemon.

### Method 2: App Bundle (For Distribution)

```bash
make bundle           # Creates bundle/CodeScribe.app
make install-app      # Copies to /Applications/CodeScribe.app
```

**Result**: Standard macOS .app bundle in `/Applications/`.

**How it runs**: Double-click or launch from Spotlight.

### Method 3: DMG Distribution (For End Users)

```bash
make dmg-signed       # Build signed DMG
make notarize         # Notarize with Apple (requires Developer ID)
# or one-shot:
# make release-full    # Build + sign + notarize
```

**Result**: `CodeScribe_X.Y.Z.dmg` ready for distribution.

## Configuration

### Config Directory

All configuration is stored in:

```
~/.codescribe/
├── .env              # Main configuration file
├── prompts/          # Custom AI prompts
│   ├── formatting.txt
│   └── assistive.txt
├── history/          # Transcription history
├── reports/          # Quality reports
└── repo_path         # Path to source repo (set during install)
```

### Environment Variables (.env)

The application loads configuration from `~/.codescribe/.env` using these priorities:

1. **Environment variables** (highest priority)
2. **~/.codescribe/.env** (main config file)
3. **Default values** (fallback)

```mermaid
flowchart TD
    A[Application Start] --> B{Check ENV vars}
    B -->|Set| C[Use ENV value]
    B -->|Not set| D{Check ~/.codescribe/.env}
    D -->|Exists| E[Load with dotenvy]
    D -->|Missing| F[Create from template]
    F --> G{Find template}
    G -->|Bundle| H[../Resources/.env.example]
    G -->|Repo| I[.env.example in repo root]
    G -->|None| J[Generate minimal .env]
    E --> K[Apply defaults for missing keys]
    H --> K
    I --> K
    J --> K
    C --> L[Config Ready]
    K --> L
```

### Key Configuration Variables

```env
# Speech-to-Text
WHISPER_LANGUAGE=pl              # pl | en | de | fr
USE_LOCAL_STT=1                  # 1 = embedded Whisper

# Hotkeys
HOLD_MODS=ctrl                   # ctrl | ctrl_alt | ctrl_shift
TOGGLE_TRIGGER=double_option     # double_option | none

# AI Formatting
AI_FORMATTING_ENABLED=1
LLM_ENDPOINT=https://api.openai.com/v1/responses
LLM_MODEL=gpt-4.1-mini
LLM_API_KEY=sk-xxx

# Optional: Separate providers for modes
LLM_FORMATTING_{ENDPOINT,MODEL,API_KEY}=...
LLM_ASSISTIVE_{ENDPOINT,MODEL,API_KEY}=...
```

## Bundle Structure

```
CodeScribe.app/
└── Contents/
    ├── Info.plist           # Bundle metadata (icon, identifier, version)
    ├── MacOS/
    │   └── codescribe       # Main executable (~888MB with embedded model)
    └── Resources/
        └── AppIcon.icns     # Application icon
```

### Info.plist Keys

| Key                          | Value                 | Purpose                      |
| ---------------------------- | --------------------- | ---------------------------- |
| CFBundleIdentifier           | io.loctree.codescribe | Unique app identifier        |
| CFBundleIconFile             | AppIcon               | Points to AppIcon.icns       |
| CFBundleExecutable           | codescribe            | Main binary name             |
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
- **Verify**: `plutil -lint /Applications/CodeScribe.app/Contents/Info.plist`

### Empty Tray Icon

- Check that `assets/icon.png` exists and is valid PNG
- Rebuild with `cargo build --release`

### Config Not Loading

- Check `~/.codescribe/.env` exists
- Verify syntax: `cat ~/.codescribe/.env`
- Check logs: `codescribe -v` for verbose output

### Hotkeys Not Working

- Grant Accessibility permission
- Grant Input Monitoring permission
- Restart the application after granting

---

_Copyright © 2024–2026 VetCoders_

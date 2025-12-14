# CodeScribe Release Scripts - Summary

This document summarizes the DMG build and notarization workflow scripts created/updated for CodeScribe.

## Files Created/Updated

### 1. Master Release Script (NEW)
**File**: `packaging/release_master.sh`

Complete orchestration script that handles the entire release process:
- Builds Rust binary in release mode
- Creates .app bundle
- Signs and notarizes .app
- Creates DMG
- Signs and notarizes DMG

**Usage**:
```bash
# Build only
./packaging/release_master.sh

# Build + sign + notarize
./packaging/release_master.sh --sign

# Build + sign (skip notarization)
./packaging/release_master.sh --sign --no-notary

# Re-sign existing build
SKIP_BUILD=1 SKIP_APP=1 ./packaging/release_master.sh --sign
```

### 2. DMG Build Script (UPDATED)
**File**: `packaging/dmg/build_dmg.sh`

**Changes**:
- Output DMG to `packaging/dist/` instead of `packaging/dmg/`
- Use version from `codescribe-rs/Cargo.toml` instead of `pyproject.toml`
- DMG name: `CodeScribe-{version}.dmg` (removed timestamp)
- Auto-cleanup staging directory
- Consistent with new workflow

### 3. Sign & Notarize Script (UPDATED)
**File**: `packaging/scripts/sign_and_notarize.sh`

**Changes**:
- Default certificate: "Developer ID Application: Maciej Gad (MW223P3NPX)"
- Default profile: "Vista Notary"
- Improved error handling and validation
- Signs inner launcher script separately
- Creates temporary ZIP for app notarization
- Staples tickets to both .app and .dmg
- Verifies Gatekeeper assessment

### 4. Entitlements (UPDATED)
**File**: `packaging/entitlements.plist`

**Changes**:
- Added audio input device access (`com.apple.security.device.audio-input`)
- Added unsigned executable memory (`com.apple.security.cs.allow-unsigned-executable-memory`)
- Added DYLD environment variables (`com.apple.security.cs.allow-dyld-environment-variables`)
- Disabled library validation (`com.apple.security.cs.disable-library-validation`)
- Documented each entitlement with comments

### 5. Documentation (NEW)
**File**: `packaging/RELEASE_WORKFLOW.md`

Comprehensive guide covering:
- Quick start commands
- Step-by-step process explanation
- Individual script usage
- Environment variable configuration
- Signing and notarization setup
- Troubleshooting guide
- Version management
- CI/CD integration examples

### 6. Validation Script (NEW)
**File**: `packaging/validate_setup.sh`

Pre-flight check script that validates:
- Required tools (cargo, codesign, hdiutil, xcrun, etc.)
- Optional tools (uv, git-lfs)
- Rust project configuration
- Packaging files existence
- Code signing certificate
- Notarization profile
- Whisper models availability
- Output directory status

**Usage**:
```bash
./packaging/validate_setup.sh
```

## Workflow Overview

```
┌─────────────────────────────────────────────────────────────┐
│                   release_master.sh                         │
│                  (Orchestrates everything)                  │
└──────────────────────┬──────────────────────────────────────┘
                       │
       ┌───────────────┼───────────────┬──────────────┐
       │               │               │              │
       ▼               ▼               ▼              ▼
┌──────────┐  ┌────────────────┐  ┌────────┐  ┌────────────┐
│  cargo   │  │ build_wrapper  │  │  sign  │  │ build_dmg  │
│  build   │  │   _app.sh      │  │  _and  │  │    .sh     │
│ --release│  │                │  │notarize│  │            │
└──────────┘  └────────────────┘  │  .sh   │  └────────────┘
                                  └────────┘
                                       │
                                       ▼
                              ┌─────────────────┐
                              │  xcrun notarytool│
                              │  xcrun stapler   │
                              └─────────────────┘
                                       │
                                       ▼
                              ┌─────────────────┐
                              │ packaging/dist/ │
                              │  • .app (signed)│
                              │  • .dmg (signed)│
                              └─────────────────┘
```

## Configuration

### Default Settings

| Setting | Value |
|---------|-------|
| **Certificate** | Developer ID Application: Maciej Gad (MW223P3NPX) |
| **Profile** | Vista Notary |
| **Entitlements** | packaging/entitlements.plist |
| **Output Dir** | packaging/dist/ |
| **Version Source** | codescribe-rs/Cargo.toml |

### Environment Variables

Override defaults with environment variables:

```bash
# Custom certificate
CERT="Mac Developer" packaging/release_master.sh --sign

# Custom notarization profile
PROFILE="MyProfile" packaging/release_master.sh --sign

# Skip build steps
SKIP_BUILD=1 packaging/release_master.sh
SKIP_APP=1 packaging/release_master.sh
SKIP_DMG=1 packaging/release_master.sh
```

## DMG Contents

The final DMG includes:

```
CodeScribe.dmg/
├── CodeScribe.app          # Signed and notarized application
├── Applications            # Symlink to /Applications
└── README-INSTALL.txt      # Installation instructions
```

## Signing Details

### Code Signing Sequence

1. Sign inner launcher script (`Contents/MacOS/codescribe`)
2. Deep sign .app bundle with hardened runtime + entitlements
3. Verify signature
4. Create ZIP for notarization
5. Submit to Apple notarization service
6. Wait for approval
7. Staple ticket to .app
8. Sign DMG
9. Staple ticket to DMG
10. Verify Gatekeeper acceptance

### Required Permissions (Info.plist)

- **Microphone**: NSMicrophoneUsageDescription
- **Accessibility**: NSAccessibilityUsageDescription
- **Input Monitoring**: NSInputMonitoringUsageDescription

### Hardened Runtime Entitlements

- Audio input device access (microphone)
- Unsigned executable memory (ML frameworks)
- DYLD environment variables (Python runtime)
- Disabled library validation (unsigned frameworks)

## Testing

### Validate Setup
```bash
./packaging/validate_setup.sh
```

### Test Build (No Signing)
```bash
./packaging/release_master.sh
```

### Test Build (With Signing)
```bash
./packaging/release_master.sh --sign --no-notary
```

### Test Full Release
```bash
./packaging/release_master.sh --sign
```

### Verify Signed Build
```bash
# Check codesigning
codesign --verify --deep --strict --verbose=2 packaging/dist/CodeScribe.app

# Check notarization
xcrun stapler validate packaging/dist/CodeScribe.app
xcrun stapler validate packaging/dist/CodeScribe-0.5.0.dmg

# Check Gatekeeper
spctl --assess --type execute -vvvv packaging/dist/CodeScribe.app
```

## Comparison with Old Workflow

### Old Workflow (release.sh)
- Called `build_wrapper_app.sh`
- Called `build_dmg.sh`
- Called `notary_quick.sh` (complex interactive script)
- Output DMG in `packaging/dmg/` with timestamp
- Required external configuration

### New Workflow (release_master.sh)
- All-in-one orchestration
- Builds from Rust source
- Consistent output location (`packaging/dist/`)
- Version-based naming (no timestamps)
- Sensible defaults
- Skip flags for selective execution
- Better error handling and validation
- Clearer progress output

## Next Steps

1. **Test the workflow**:
   ```bash
   ./packaging/validate_setup.sh
   ./packaging/release_master.sh
   ```

2. **Test signing** (if certificate is available):
   ```bash
   ./packaging/release_master.sh --sign --no-notary
   ```

3. **Test full notarization** (requires "Vista Notary" profile):
   ```bash
   ./packaging/release_master.sh --sign
   ```

4. **Create a release**:
   - Update version in `codescribe-rs/Cargo.toml`
   - Run full release: `./packaging/release_master.sh --sign`
   - Test DMG on clean macOS system
   - Upload to GitHub releases or distribution channel

## Maintenance

### Update Certificate
Edit default in `packaging/scripts/sign_and_notarize.sh`:
```zsh
CERT="Developer ID Application: Your Name (TEAMID)"
```

### Update Notarization Profile
Edit default in `packaging/scripts/sign_and_notarize.sh`:
```zsh
PROFILE="Your Profile Name"
```

### Update Entitlements
Edit `packaging/entitlements.plist` as needed for new permissions.

### Update DMG Layout
Edit `packaging/dmg/build_dmg.sh` for custom layouts or backgrounds.

## Support Files

| File | Purpose |
|------|---------|
| `packaging/release_master.sh` | Master orchestration script |
| `packaging/dmg/build_dmg.sh` | DMG creation |
| `packaging/appwrap/build_wrapper_app.sh` | .app bundle creation |
| `packaging/scripts/sign_and_notarize.sh` | Signing and notarization |
| `packaging/entitlements.plist` | Code signing entitlements |
| `packaging/validate_setup.sh` | Pre-flight validation |
| `packaging/RELEASE_WORKFLOW.md` | Comprehensive documentation |

## Migration from Old Scripts

The old `packaging/release.sh` still exists but is superseded by `packaging/release_master.sh`.

Key differences:
- Use `release_master.sh` for new releases
- Old script called `notary_quick.sh` (still available but not needed)
- New workflow has better defaults and simpler usage
- Old DMG timestamps removed for cleaner versioning

Both workflows are compatible and can coexist.

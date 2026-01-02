# CodeScribe Release Workflow

This document describes the complete process for building, signing, and notarizing CodeScribe for distribution.

## Quick Start

### Basic Build (No Signing)
```bash
packaging/release_master.sh
```

### Full Release (Build + Sign + Notarize)
```bash
packaging/release_master.sh --sign
```

### Build + Sign (Skip Notarization)
```bash
packaging/release_master.sh --sign --no-notary
```

## Output Location

All output files are created in: `packaging/dist/`

- `CodeScribe.app` - macOS application bundle
- `CodeScribe-{version}.dmg` - Disk image for distribution

## Release Process Steps

The master script `packaging/release_master.sh` orchestrates these steps:

1. **Build Rust Binary** (release mode with optimizations)
   - Compiles `codescribe-rs/` to `target/release/codescribe`
   - Uses aggressive optimization: LTO, minimal codegen units, stripped

2. **Create .app Bundle**
   - Bundles Rust binary with Python backend
   - Embeds Whisper model (if available)
   - Creates launcher script and Info.plist
   - Sets up proper directory structure

3. **Sign .app Bundle** (if `--sign` flag provided)
   - Certificate: "Developer ID Application: Maciej Gad (MW223P3NPX)"
   - Entitlements: `packaging/entitlements.plist`
   - Hardened runtime enabled
   - Deep signing of all components

4. **Create DMG**
   - Volume name: "CodeScribe"
   - Contains: CodeScribe.app, Applications symlink, README
   - Format: UDZO (compressed)
   - Output: `packaging/dist/CodeScribe-{version}.dmg`

5. **Sign and Notarize DMG** (if `--sign` flag provided)
   - Signs DMG with same certificate
   - Submits to Apple notarization service
   - Profile: "Vista Notary"
   - Staples notarization ticket to both .app and .dmg

## Individual Scripts

If you need to run individual steps:

### Build .app Bundle Only
```bash
packaging/appwrap/build_wrapper_app.sh
```

### Build DMG Only (requires existing .app)
```bash
packaging/dmg/build_dmg.sh
```

### Sign and Notarize Only
```bash
# Sign .app only
packaging/scripts/sign_and_notarize.sh \
  --app packaging/dist/CodeScribe.app

# Sign both .app and .dmg
packaging/scripts/sign_and_notarize.sh \
  --app packaging/dist/CodeScribe.app \
  --dmg packaging/dist/CodeScribe-0.5.0.dmg
```

## Environment Variables

### Certificate Override
```bash
CERT="Mac Developer" packaging/release_master.sh --sign
```

### Notarization Profile Override
```bash
PROFILE="MyOtherProfile" packaging/release_master.sh --sign
```

### Skip Steps
```bash
# Skip Rust build (use existing binary)
SKIP_BUILD=1 packaging/release_master.sh

# Skip .app bundle creation (use existing bundle)
SKIP_APP=1 packaging/release_master.sh

# Skip DMG creation
SKIP_DMG=1 packaging/release_master.sh
```

### Combination Examples
```bash
# Re-sign existing build without rebuilding
SKIP_BUILD=1 SKIP_APP=1 packaging/release_master.sh --sign

# Build new binary but reuse existing .app structure
SKIP_APP=1 packaging/release_master.sh

# Create DMG without notarization (for testing)
packaging/release_master.sh --sign --no-notary
```

## Signing and Notarization

### Certificate Requirements

CodeScribe uses "Developer ID Application" certificate for distribution outside the Mac App Store.

Default certificate: `Developer ID Application: Maciej Gad (MW223P3NPX)`

### Entitlements

The app requires these entitlements (`packaging/entitlements.plist`):

- `com.apple.security.device.audio-input` - Microphone access for speech recording
- `com.apple.security.cs.allow-unsigned-executable-memory` - JIT compilation in ML frameworks
- `com.apple.security.cs.allow-dyld-environment-variables` - Python runtime support
- `com.apple.security.cs.disable-library-validation` - Loading unsigned frameworks/dylibs

### Runtime Permissions

CodeScribe requests these permissions at runtime (via Info.plist):

- **NSMicrophoneUsageDescription** - "Needed to transcribe speech."
- **NSAccessibilityUsageDescription** - "Needed to monitor hotkeys and paste results."
- **NSInputMonitoringUsageDescription** - "Needed to detect keyboard shortcuts for recording."

### Notarization Profile Setup

If you need to create/update the notarization profile:

```bash
xcrun notarytool store-credentials "Vista Notary" \
  --apple-id "your@email.com" \
  --team-id "MW223P3NPX" \
  --password "app-specific-password"
```

### Verify Notarization

```bash
# Check if app is notarized
spctl --assess --type execute -vvvv packaging/dist/CodeScribe.app

# Check DMG
spctl --assess --type open --context context:primary-signature -vvvv packaging/dist/CodeScribe-0.5.0.dmg

# Verify stapled ticket
xcrun stapler validate packaging/dist/CodeScribe.app
xcrun stapler validate packaging/dist/CodeScribe-0.5.0.dmg
```

## DMG Customization

The DMG is created with a simple layout:

```
CodeScribe.dmg/
├── CodeScribe.app          # The application
├── Applications → /Applications   # Symlink for drag-and-drop install
└── README-INSTALL.txt      # Installation instructions
```

To customize the DMG appearance or layout, edit:
- `packaging/dmg/build_dmg.sh` - Build script
- `packaging/dmg/stage/README-INSTALL.txt` - Installation instructions (auto-generated)

## Troubleshooting

### Codesigning Fails

**Issue**: `errSecInternalComponent` or certificate not found

**Solution**: Ensure certificate is imported to keychain
```bash
security find-identity -v -p codesigning
```

### Notarization Fails

**Issue**: Notarization rejected

**Solution**: Check notarization log
```bash
# Get submission ID from previous output, then:
xcrun notarytool log <submission-id> --keychain-profile "Vista Notary"
```

Common issues:
- Missing entitlements (check `packaging/entitlements.plist`)
- Unsigned nested binaries (ensure deep signing)
- Hardened runtime issues (check entitlements)

### Gatekeeper Assessment Fails

**Issue**: `spctl --assess` shows rejection

**Solution**: Check if notarization ticket is stapled
```bash
xcrun stapler validate packaging/dist/CodeScribe.app
```

If not stapled, run:
```bash
xcrun stapler staple packaging/dist/CodeScribe.app
```

### DMG Not Opening

**Issue**: DMG is quarantined or signature invalid

**Solution**: For local testing only:
```bash
DMG_UNQUARANTINE=1 packaging/dmg/build_dmg.sh
```

**⚠️ Never distribute unquarantined DMGs!**

## Version Management

Version is read from `codescribe-rs/Cargo.toml`:

```toml
[package]
version = "0.5.0"
```

This version is used for:
- DMG filename: `CodeScribe-{version}.dmg`
- .app bundle `CFBundleVersion` and `CFBundleShortVersionString`

To release a new version:
1. Update version in `codescribe-rs/Cargo.toml`
2. Run `packaging/release_master.sh --sign`

## Distribution

After successful build, sign, and notarization:

1. Test the DMG on a clean macOS system
2. Verify all permissions are requested properly
3. Upload to distribution channel (website, GitHub releases, etc.)

### GitHub Release Example

```bash
# Create release tag
git tag -a v0.5.0 -m "Release v0.5.0"
git push origin v0.5.0

# Upload DMG to GitHub release
gh release create v0.5.0 \
  packaging/dist/CodeScribe-0.5.0.dmg \
  --title "CodeScribe v0.5.0" \
  --notes "Release notes here"
```

## CI/CD Integration

For automated builds (GitHub Actions, etc.):

```yaml
- name: Build and Sign CodeScribe
  env:
    CERT: ${{ secrets.APPLE_CERT_ID }}
    PROFILE: ${{ secrets.NOTARY_PROFILE }}
  run: |
    # Import certificate to keychain (if using secrets)
    # ...

    # Build and sign
    packaging/release_master.sh --sign

- name: Upload DMG
  uses: actions/upload-artifact@v3
  with:
    name: CodeScribe-dmg
    path: packaging/dist/CodeScribe-*.dmg
```

## Notes

- The build process is idempotent - you can re-run it safely
- Signing requires valid Developer ID certificate and keychain access
- Notarization requires internet connection and can take 5-15 minutes
- DMG is compressed (UDZO format) to reduce download size
- The .app bundle includes a copy of the full repository (excluding .git, models, etc.)
- Bundled Whisper models are auto-detected from `models/` directory

## Support

For issues with the release process, check:
- `packaging/appwrap/build_wrapper_app.sh` - App bundle creation
- `packaging/dmg/build_dmg.sh` - DMG creation
- `packaging/scripts/sign_and_notarize.sh` - Signing/notarization
- `packaging/entitlements.plist` - Code signing entitlements

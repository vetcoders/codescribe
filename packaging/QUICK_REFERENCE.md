# CodeScribe Release - Quick Reference

## One-Line Commands

### Validate Setup
```bash
packaging/validate_setup.sh
```

### Build Only (No Signing)
```bash
packaging/release_master.sh
```

### Full Release (Build + Sign + Notarize)
```bash
packaging/release_master.sh --sign
```

### Sign Only (Skip Notarization)
```bash
packaging/release_master.sh --sign --no-notary
```

### Re-sign Existing Build
```bash
SKIP_BUILD=1 SKIP_APP=1 packaging/release_master.sh --sign
```

## Output Location

```
packaging/dist/
├── CodeScribe.app           # macOS application bundle
└── CodeScribe-0.5.0.dmg    # Disk image for distribution
```

## Individual Components

### Build Rust Binary
```bash
cd codescribe-rs && cargo build --release
```

### Build .app Bundle
```bash
packaging/appwrap/build_wrapper_app.sh
```

### Build DMG
```bash
packaging/dmg/build_dmg.sh
```

### Sign and Notarize
```bash
packaging/scripts/sign_and_notarize.sh \
  --app packaging/dist/CodeScribe.app \
  --dmg packaging/dist/CodeScribe-0.5.0.dmg
```

## Common Scenarios

### Development Build
```bash
packaging/release_master.sh
```

### Testing Signing (Local Only)
```bash
packaging/release_master.sh --sign --no-notary
```

### Production Release
```bash
# 1. Update version in codescribe-rs/Cargo.toml
# 2. Run full release
packaging/release_master.sh --sign
```

### Re-package Without Rebuilding
```bash
SKIP_BUILD=1 packaging/release_master.sh
```

### Just Create DMG (App Already Built)
```bash
SKIP_BUILD=1 SKIP_APP=1 packaging/release_master.sh
```

## Environment Variables

| Variable | Default | Purpose |
|----------|---------|---------|
| `CERT` | Developer ID Application: Maciej Gad (MW223P3NPX) | Code signing certificate |
| `PROFILE` | Vista Notary | Notarization profile |
| `SKIP_BUILD` | 0 | Skip Rust build |
| `SKIP_APP` | 0 | Skip .app bundle creation |
| `SKIP_DMG` | 0 | Skip DMG creation |

## Verification Commands

### Check Signature
```bash
codesign --verify --deep --strict --verbose=2 packaging/dist/CodeScribe.app
```

### Check Notarization
```bash
xcrun stapler validate packaging/dist/CodeScribe.app
xcrun stapler validate packaging/dist/CodeScribe-0.5.0.dmg
```

### Test Gatekeeper
```bash
spctl --assess --type execute -vvvv packaging/dist/CodeScribe.app
```

## Troubleshooting

### Certificate Not Found
```bash
security find-identity -v -p codesigning
```

### Check Notarization Profile
```bash
xcrun notarytool history --keychain-profile "Vista Notary"
```

### View Notarization Log
```bash
xcrun notarytool log <submission-id> --keychain-profile "Vista Notary"
```

## Configuration Files

| File | Purpose |
|------|---------|
| `codescribe-rs/Cargo.toml` | Version number (line 3) |
| `packaging/entitlements.plist` | Code signing entitlements |
| `packaging/scripts/sign_and_notarize.sh` | Default cert/profile (lines 13-14) |

## Documentation

- **Full Guide**: `packaging/RELEASE_WORKFLOW.md`
- **Summary**: `packaging/RELEASE_SCRIPTS_SUMMARY.md`
- **This File**: `packaging/QUICK_REFERENCE.md`

## Version Update Checklist

- [ ] Update version in `codescribe-rs/Cargo.toml`
- [ ] Run `packaging/validate_setup.sh`
- [ ] Run `packaging/release_master.sh --sign`
- [ ] Test DMG on clean macOS system
- [ ] Create git tag: `git tag -a v0.5.0 -m "Release v0.5.0"`
- [ ] Upload DMG to distribution channel

## Help

For detailed information, see:
```bash
cat packaging/RELEASE_WORKFLOW.md
```

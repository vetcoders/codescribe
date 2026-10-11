# Release signing secrets runbook

The `Release DMG` GitHub Actions workflow fails closed until the signing,
notarization, licensing, and Sparkle key inputs below are configured. Only a repository administrator with
access to the Vetcoders Apple Developer account should perform this procedure.

Never paste secret values into issues, pull requests, Actions logs, shell
history, or this document. The commands below read values from local variables
or standard input and send them directly to GitHub.

## Local standard DMG credentials and execution context

The local `make release-standard` lane reuses the approved Apple credentials
under `~/.keys/.notary.env` through the named `VSNotary` profile. It does not read
that file itself. Provisioning the profile is separate from generating or
rotating credentials; do not create new credentials merely because the profile
is missing.

Without `--keychain`, `notarytool` stores and reads credentials in the **data
protection keychain** (iCloud Keychain or Local Items), not a file-based login or
build keychain. See [Apple TN3147](https://developer.apple.com/documentation/technotes/tn3147-migrating-to-the-latest-notarization-tool).
For the normal repository lane, store and validate `VSNotary` without
`--keychain` or `--sync` in the same native GUI user session that runs the build.
On Sztudio, use the native Terminal or iTerm2 session of `silver`; an SSH shell
is a different credential-access context and is not evidence of GUI access.

First try `xcrun notarytool history --keychain-profile VSNotary` in that context.
Reuse a usable profile without overwriting it. Only a confirmed missing profile
permits saving the existing approved account, team and app-specific password
with `notarytool store-credentials VSNotary` and its default validation. Enter
the password through the secure interactive prompt or standard input; never put
it in command arguments, shell history or logs. Other access or validation
errors require diagnosis, not an overwrite or Keychain reset. Prove profile
access with history and a known recorded submission before starting the build.

Developer ID signing is a separate requirement. Seeing an identity or reading
Apple history does not prove access to its private key. On Sztudio the existing
signer may be in `pensieve-build`; resolve any native unlock prompt in the
approved GUI context and verify an isolated copied executable before the heavy
build. Do not import a certificate, change ACLs/search lists, reset Keychain or
substitute another host to hide an access failure.

Sztudio's existing session refresh is `unlock_build_keychain` from
`~/Git/pensieve/scripts/lib/build-keychain.sh`. Resolve and record its selected
keychain and password-file paths before calling it; require the existing
owner-only credential file and keep its value out of logs. This function only
unlocks the selected file keychain in the caller's security session. It does not
import keys or alter ACLs, search lists or auto-lock settings. Run it in the
actual native build process context when that existing signer needs a refresh,
then prove signing with the isolated copy. Opening a GUI app from SSH does not
erase inherited SSH environment markers; verify the real terminal ancestry and
environment rather than clearing markers to make the context guard pass.

After any documentation checkpoint, freeze the actual full source SHA and a
clean tracked tree. The normal command is:

```sh
make release-standard CODESCRIBE_DIST_CODESIGN_IDENTITY='Developer ID Application: Maciej Gad (MW223P3NPX)'
```

This builds the distribution `release` profile with `CSDeveloperSurface=0`.
Its slim payload embeds Silero, uses runtime/downloaded Whisper, and omits
MiniLM. It creates a DMG and checksum in the checkout; it does not install the
app or publish a tag, GitHub Release or appcast. Preserve an existing file at
the exact output path before invoking the producer.

Keep the complete command log and a durable execution record containing the
host, context, source SHA, times, exit status and exact artifact path. The
six-field invocation receipt used internally by `release-standard` is temporary
and removed on exit. Durable proof comprises the final DMG/checksum, Apple
submission UUID/info/log, signatures, staple/Gatekeeper and payload-gate results.
Apple's archive hash describes the submitted pre-staple bytes; the refreshed
sidecar describes final post-staple bytes. Preserve both without requiring them
to be identical. Record the mounted app's version, build, source, Lab flag and
public verification-key parity separately.

A failed command does not justify rerunning the whole release target. If a
submission UUID exists, inspect that UUID rather than uploading again. Passing
this local artifact lane does not certify publication, cold installation or
microphone/voice acceptance; those remain the public checklist's later steps.

## Prerequisites

- `gh auth status` shows an account allowed to administer
  `vetcoders/codescribe` Actions secrets.
- The `Developer ID Application` certificate and its private key are present
  together in Keychain Access on the operator Mac.
- The operator can sign in to the Apple Account used for notarization.
- Work in a private shell session with history disabled or use the interactive
  `gh secret set NAME` prompts described below.

## 1. `CODESIGN_CERTIFICATE_BASE64`

Source: export the `Developer ID Application` certificate **with its private
key** from Keychain Access as a password-protected `.p12` file. Do not export a
`.cer`; it does not contain the private key needed by CI.

Convert the export to one base64 line, then submit it without putting the value
on the command line:

```bash
read -r "P12_PATH?Private path to the exported .p12: "
base64 < "$P12_PATH" | gh secret set CODESIGN_CERTIFICATE_BASE64 --repo vetcoders/codescribe
```

Delete the temporary `.p12` securely after the repository secret has been
validated. Keep the authoritative certificate/private key in Keychain and the
approved backup location.

## 2. `CODESIGN_CERTIFICATE_PASSWORD`

Source: the export password chosen while creating the `.p12` above. It is not
the Mac login password unless the operator deliberately chose the same value.

```bash
gh secret set CODESIGN_CERTIFICATE_PASSWORD --repo vetcoders/codescribe
```

`gh` prompts for the value on standard input. Paste it there; do not add
`--body` with a literal password.

## 3. `CODESCRIBE_CODESIGN_IDENTITY`

Source: the Common Name of the imported `Developer ID Application`
certificate. Read it from Keychain Access or list local code-signing identities:

```bash
security find-identity -v -p codesigning
gh secret set CODESCRIBE_CODESIGN_IDENTITY --repo vetcoders/codescribe
```

Enter the complete identity shown for the distribution certificate, including
the organization and Team ID suffix. Do not use an `Apple Development`
identity.

## 4. `APPLE_ID`

Source: the email address of the Apple Account authorized to notarize apps for
the Vetcoders developer team.

```bash
gh secret set APPLE_ID --repo vetcoders/codescribe
```

This account must belong to the same team as the signing certificate and must
have sufficient App Store Connect permissions for notarization.

## 5. `APPLE_TEAM_ID`

Source: the ten-character Team Identifier attached to the `Developer ID Application` certificate and shown in the Apple Developer membership details.
For this repository, the expected public team identifier is `MW223P3NPX`.
Confirm that identifier against the certificate before submitting it; never
infer it from an unrelated certificate.

```bash
gh secret set APPLE_TEAM_ID --repo vetcoders/codescribe
```

The value must match the Team ID suffix in
`CODESCRIBE_CODESIGN_IDENTITY`. A mismatch lets certificate import succeed but
causes notarization authentication to fail.

## 6. `APPLE_APP_SPECIFIC_PASSWORD`

Source: create an app-specific password for the notarization Apple Account at
<https://appleid.apple.com/> under **Sign-In and Security → App-Specific
Passwords**. Give it a release-specific label so its owner and purpose are
obvious during rotation.

```bash
gh secret set APPLE_APP_SPECIFIC_PASSWORD --repo vetcoders/codescribe
```

Apple displays this password only once. Store it in the operator-approved
password manager before closing the creation dialog.

## 7. `CODESCRIBE_LICENSE_PUBLIC_KEY_HEX` (repository variable)

Source: the 32-byte Ed25519 public key paired with the operator-owned production
license signer, encoded as exactly 64 hexadecimal characters. This value is
public and belongs in an Actions repository variable, not a secret. Never put
the production private key or seed in this repository or the app build.

```bash
gh variable set CODESCRIBE_LICENSE_PUBLIC_KEY_HEX --repo vetcoders/codescribe
```

The Rust release build fails when this variable is missing, malformed, or has
the checked-in development-key fingerprint. Generating and provisioning the
production keypair is an operator-owned release action.

Local source installs use the optimized `local-release` profile.
`make install-app` reads the configured `license-public.hex` from the private
settings pack and explicitly passes it as this build variable. When invoking
`make app PROFILE=local-release` directly for use with existing licenses, pass
the public key paired with their signer as well: that profile can otherwise
embed the checked-in development verifier. Signing and notarization do not
select or validate the license verifier.

Swift license fixtures deliberately require the development verifier; follow
[the Swift test instructions](../macos/CodescribeTests/README.md) when switching
between app and fixture builds. A UUID or other random identifier is not a
substitute for the Ed25519 public key.

## 8. `SPARKLE_ED_PUBLIC_KEY` (repository variable)

Source: the public key printed by Sparkle's `generate_keys`. The app embeds this
value as `SUPublicEDKey`, so it is intentionally public.

```bash
gh variable set SPARKLE_ED_PUBLIC_KEY --repo vetcoders/codescribe
```

The DMG verifier refuses an empty bundle key and compares it with this release
input. The appcast generator also derives the public key from the private seed
and refuses a mismatched pair.

## 9. `SPARKLE_ED_PRIVATE_KEY` (secret)

Source: the base64 Ed25519 private seed exported by Sparkle's
`generate_keys -x`. Store it only as an Actions secret and in the approved
operator credential store.

```bash
gh secret set SPARKLE_ED_PRIVATE_KEY --repo vetcoders/codescribe
```

The workflow passes this value only to `generate_appcast` over standard input.
It never enters the application bundle or a repository file.

## Validate configuration

GitHub never returns secret values. Confirm that all secret names exist and note
their update timestamps:

```bash
gh secret list --repo vetcoders/codescribe
```

The list must include exactly these release inputs:

- `CODESIGN_CERTIFICATE_BASE64`
- `CODESIGN_CERTIFICATE_PASSWORD`
- `CODESCRIBE_CODESIGN_IDENTITY`
- `APPLE_ID`
- `APPLE_TEAM_ID`
- `APPLE_APP_SPECIFIC_PASSWORD`
- `SPARKLE_ED_PRIVATE_KEY`

Confirm the two public repository variables separately:

```bash
gh variable list --repo vetcoders/codescribe
```

- `CODESCRIBE_LICENSE_PUBLIC_KEY_HEX`
- `SPARKLE_ED_PUBLIC_KEY`

Do not trigger a release merely to test secret presence. The next
operator-authorized tag or manual release run performs its own fail-closed
validation before importing the certificate.

## Rotation and revocation

Rotate the `.p12` payload, its password, and the identity together whenever the
Developer ID certificate is renewed, replaced, exposed, or revoked. Update all
three GitHub secrets in one maintenance window, then securely remove temporary
exports.

Rotate the Apple app-specific password independently when personnel changes,
the credential may have been exposed, or Apple revokes it. Revoke the old
password at <https://appleid.apple.com/> after the replacement has been stored
in GitHub. Reconfirm `APPLE_ID` and `APPLE_TEAM_ID` whenever account ownership
or team membership changes.

Rotate the Codescribe licensing keypair and the Sparkle keypair as separate
atomic pairs. Update each public repository variable in the same maintenance
window as its operator-owned private signer. A public/private mismatch is a
hard release failure.

After any rotation, use `gh secret list --repo vetcoders/codescribe` to confirm
fresh update timestamps. A real release run, tag, or publication remains an
operator-owned action.

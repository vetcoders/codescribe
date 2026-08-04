# Release signing secrets runbook

The `Release DMG` GitHub Actions workflow fails closed until all six signing
and notarization secrets are configured. Only a repository administrator with
access to the VetCoders Apple Developer account should perform this procedure.

Never paste secret values into issues, pull requests, Actions logs, shell
history, or this document. The commands below read values from local variables
or standard input and send them directly to GitHub.

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
the VetCoders developer team.

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

## Validate configuration

GitHub never returns secret values. Confirm that all six names exist and note
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

After any rotation, use `gh secret list --repo vetcoders/codescribe` to confirm
fresh update timestamps. A real release run, tag, or publication remains an
operator-owned action.

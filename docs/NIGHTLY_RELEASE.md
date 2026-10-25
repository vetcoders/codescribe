# Nightly distribution

Founder decision, 2026-10-25: Nightly is a signed and notarized DMG published
on the Codescribe website. GitHub Releases and the stable Sparkle appcast
remain the stable distribution lane. Nightly publication creates no release
tag or GitHub Release.

## Produce and verify

Freeze a clean source commit. Follow `RELEASE_SECRETS_RUNBOOK.md` for the
existing Developer ID signer, production public verification keys and named
notary profile in the native GUI user context. Never print credentials.

```sh
make release-nightly
```

This delegates to the standard slim producer with the explicit `nightly`
channel. It uses the distribution Rust profile, keeps `CSDeveloperSurface=0`,
and adds `_nightly` to the provenance-bearing DMG filename. It retains all
signature, notarization, staple, payload and receipt checks. The default
standard/full commands still expect `stable`; a Nightly DMG cannot pass a
stable payload acceptance.

Notarization has two ordered submissions: first the signed app archive, then
the DMG containing that already-stapled app. The mounted app must carry its own
ticket as well as the DMG. Stapling only the external build app after creating
the image cannot satisfy this check. Retain both Apple submission UUIDs; resume
the relevant UUID after a timeout instead of submitting again.

Manual verification of the exact completed artifact:

```sh
make verify-dmg DMG=/absolute/path/to/artifact_nightly.dmg \
  VARIANT=slim VERSION=0.18.0 EXPECTED_CHANNEL=nightly
```

Never replace or restamp a bundle after signing. A failed submission with an
Apple UUID is resumed by inspecting that UUID; do not rerun submission merely
to recover a log.

## Install the exact notarized app locally

`make install-app-release` verifies the source signature and staple, copies
without re-signing, checks the installed artifact and arms the existing
idle-safe restart waiter. It never sends a raw Quit or termination signal.
An active recording or unsent draft keeps the old loaded process alive.
Installed and loaded receipts must remain separate until activation succeeds.

## Publish

Use the existing canonical static site and separately preserved `/rc/`
candidate-download storage described in `site/README.md` and
`services/license-issuer/README.md`. Put Nightly bytes in a distinct Nightly
subdirectory, retain a checksum beside the DMG, and provide a clearly labeled
Nightly download page/link with version, build, full source SHA and SHA-256.
Keep the stable homepage download snapshot and `/appcast.xml` unchanged.
Website redeploys must continue preserving `/rc/` and `/appcast.xml`.

Publication needs a public HTTPS download check, a fresh download matching
the producer checksum, signature/staple/Gatekeeper and channel verification
of those downloaded bytes, and an install/run smoke receipt. Record exact
artifact source, source commit, website publication commit and rollback path.
A local source test or fake signing fixture is not artifact acceptance.

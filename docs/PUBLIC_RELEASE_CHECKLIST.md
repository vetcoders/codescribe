# Public release checklist

This is the repeatable release procedure. Checkboxes describe required evidence,
not a claim that the current candidate has passed. Record actual commands, source
SHA, artifact hashes and results in the release report.

For the current version, run `make version`; for the last published stable
version, inspect GitHub Releases. The candidate and the public release can differ.

## Freeze and validate the candidate

- [ ] Record the full source SHA, branch and clean tracked tree. Admit worker
      commits through the designated integrator before freezing the candidate.
- [ ] The integrator runs `make check`, `make verify`, and `make test-swift`.
      Regenerate changed bridge APIs with `make app-bindings`. Record nonzero
      selected test counts and outstanding runtime acceptance separately.
- [ ] Update the version, changelog and release notes to describe delivered
      behavior. A worker report, commit or green suite is not installed acceptance.
- [ ] Confirm the candidate `CFBundleVersion` is greater than the published
      stable build. `scripts/build-app.sh` derives it from Git history; a squash
      merge can shorten that history. Preserve proven release ancestry and verify
      the resulting tree instead of silently publishing a lower build number.
- [ ] Confirm `LICENSE` and public product copy agree on `FSL-1.1-ALv2`.

## Sign, notarize and verify the slim DMG

The daily public artifact is the slim DMG. It embeds Silero and uses the runtime
Whisper model cache. The optional full DMG is a separate, explicitly requested
artifact; neither variant requires an unused embedder.

- [ ] Confirm no recording or agent turn is active using the canonical idle
      guard. Never stop a live take to make a release pass.
- [ ] Check local Developer ID signing, production license public key and Sparkle
      public key inputs with `make dist-preflight-signed`. Use the approved
      credentials under `~/.keys` and the existing Keychain notary profile; do not
      print private values. See [the signing runbook](RELEASE_SECRETS_RUNBOOK.md).
- [ ] Run `make release-standard` through the interactive release shell with the
      selected `NOTARY_PROFILE`. At most one production release run per day;
      preserve its log and exact artifact receipt. An ad-hoc source install is
      not a distribution artifact.
- [ ] Verify Developer ID signature, Apple notarization acceptance, stapling,
      Gatekeeper and the fail-closed payload gate:

      ```sh
      make verify-dmg DMG=<exact-artifact> VARIANT=slim VERSION=<version>
      ```

- [ ] Record post-staple SHA256, byte size, embedded version, build and source
      commit. Sign the appcast against these exact DMG bytes.

GitHub Actions publication is an alternative only when its required signing and
notary inputs are provisioned. A failed tag-triggered workflow does not replace
or invalidate an independently verified local artifact; disclose that failure.

## Publish and check the cold path

- [ ] Tag the exact accepted source and attach the verified DMG, checksum,
      provenance and signed appcast to its GitHub Release.
- [ ] Download the public DMG and checksum into a fresh directory. Verify hash,
      payload, signature, notarization ticket and embedded provenance again.
- [ ] Install that downloaded, verified app through the guarded prebuilt path:

      ```sh
      make install-if-idle INSTALL_APP_SOURCE=<mounted-verified-app>
      ```

      Do not use `install-app-release`: that target can terminate the running
      application. Never replace an active capture. An OS permission denial is
      an incomplete installation, not permission to kill or bypass the guard.

- [ ] Verify `/Applications/Codescribe.app` version, build, source, signature and
      a fresh successful launch. Only then play the installation success ping.
- [ ] Check first-run onboarding, installed helpers and selected client skills.
      Record actual UI, voice and transcription acceptance separately; tests and
      app launch alone do not certify those behaviors.

## Update feed and public website

- [ ] The shipped bundle has no broad ATS exception, has a nonempty
      `SUPublicEDKey`, and uses the controlled HTTPS `SUFeedURL`.
- [ ] The live appcast returns 200 and advertises the exact signed stable DMG,
      correct version, monotonic build, byte size and valid EdDSA signature.
- [ ] Deploy the canonical site at `https://codescribe.vetcoders.io/` using the
      topology documented in [site/README.md](../site/README.md). GitHub Pages is
      a secondary copy; its green deployment is not canonical production proof.
- [ ] Preserve the verified appcast when deploying static pages. Check the home,
      Fleet and Install download links against the newly published GitHub asset.
- [ ] Verify TLS, canonical URLs, meaningful HTML, robots, sitemap, social cards
      and current screenshots. Do not advertise unfinished controls as released.
- [ ] Complete the release report's security gate, exposed-surface inventory,
      deployment/rollback decision and cold install smoke with actual evidence.

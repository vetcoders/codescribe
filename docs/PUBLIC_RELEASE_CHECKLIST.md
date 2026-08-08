# Public Release Checklist

Codescribe is close enough to go public only when the public surfaces tell the same truth as the code.

Current source version: `0.12.2`

## Must Be True Before Public Announcement

- [x] GitHub repository visibility is intentionally changed from private to public.
- [x] GitHub repository description matches the product:
      `Native macOS tray dictation and assistive voice overlay with local Whisper live preview.`
- [x] GitHub repository homepage points to `https://vetcoders.github.io/codescribe/`.
- [x] GitHub topics include launch metadata for macOS dictation, speech-to-text, Whisper, Rust, and voice-agent positioning.
- [ ] GitHub license display is checked after public visibility. If GitHub still reports Apache-2.0, the README and release notes must explicitly state that the active license is `FSL-1.1-ALv2`.
- [ ] GitHub Actions release secrets are configured:
  - `CODESIGN_CERTIFICATE_BASE64`
  - `CODESIGN_CERTIFICATE_PASSWORD`
  - `CODESCRIBE_CODESIGN_IDENTITY`
  - `APPLE_ID`
  - `APPLE_TEAM_ID`
  - `APPLE_APP_SPECIFIC_PASSWORD`
- [ ] Optional repository variable `CODESCRIBE_BUNDLE_ID` is set, or the workflow default `com.vetcoders.codescribe` is accepted.
- [ ] `CHANGELOG.md` has a current `0.12.x` release section.
- [ ] The hardened release workflow in this branch has landed on `main`; do not tag from the old `main` workflow that still builds an ad-hoc `make dmg` artifact.
- [ ] Tag `v0.12.2` is created only after the release notes and signing secrets are ready.
- [ ] The `Release DMG` workflow produces both release variants:
  - `Codescribe_0.12.2.dmg` with embedded Silero + embedder and runtime Whisper cache/download.
  - `Codescribe_0.12.2_full.dmg` with embedded Silero + embedder + Whisper.
- [ ] Both DMGs are Developer ID signed, notarized, stapled, and pass Gatekeeper on a machine outside the developer environment.
- [ ] **Payload gate (fail-closed):** every release DMG passes
      `make verify-dmg DMG=<path> VARIANT=slim|full VERSION=<X.Y.Z>`
      (`scripts/verify-dmg-payload.sh`). This refuses incomplete payloads that
      still codesign/notarize (regression class: 0.13.2 missing MiniLM, ≈89 MB
      DMG / ≈30 MB dylib). `release-standard` and `release-full` run this gate
      automatically at the end of the build.
- [ ] Landing page primary CTA does not promise a DMG until a current notarized DMG exists.
- [ ] README install section names source install as the guaranteed path until the current DMG is verified.

## Transport security: ATS + the update feed (S-3)

macOS 27 keeps tightening App Transport Security, and the updater is the one component that
fetches remote content and then *executes* what it fetched. These are release-gate items, not
code cuts.

- [ ] **No ATS exception has been added.** The app ships **no** `NSAppTransportSecurity` key at
      all — verified 2026-08-08: zero occurrences of `NSAppTransportSecurity` /
      `NSAllowsArbitraryLoads` across the repo. That means the OS defaults apply (HTTPS required,
      TLS 1.2+, forward secrecy). This is the desired state; the check is that nobody added an
      exception to make a staging host work. If an exception ever becomes necessary, it must be a
      scoped `NSExceptionDomains` entry with a justification in this checklist — never
      `NSAllowsArbitraryLoads`.
- [ ] **`SUFeedURL` is HTTPS on a host we control.** Currently
      `https://vetcoders.github.io/codescribe/appcast.xml` (`macos/project.yml`). A plain-HTTP or
      third-party feed is an update-channel takeover, not a convenience.
- [ ] **The feed actually resolves.** `curl -sSI "$(/usr/libexec/PlistBuddy -c 'Print :SUFeedURL' \
      /Applications/Codescribe.app/Contents/Info.plist)"` returns `200`, not `404`.
      `site/public/appcast.xml` exists on feature branches, but GitHub Pages deploys from the
      default branch and `.github/workflows/release.yml` states that publishing to the live feed
      "stays an operator PR" — so a signed appcast can exist as a release artifact while the live
      feed is still absent. **Verify the live URL, not the file in your worktree.**
- [ ] **`SUPublicEDKey` is present in the shipped bundle.** Empty means the updater is
      fail-closed (it refuses every update) — correct for a local build, a shipped-release defect.
      `scripts/smoke-macos27.sh` reports this row as `INFO` for local builds and `PASS` only when
      the key is present; `scripts/verify-dmg-payload.sh` is the release-side gate.
- [ ] **The DMG is fetched over HTTPS too.** The `enclosure url` in the published appcast points
      at the GitHub Releases HTTPS asset URL, and the EdDSA signature in the appcast matches the
      artifact that was actually notarized and stapled.

## First Public Release Drill

1. Confirm the tree is clean and `make check` passes.
2. Confirm `gh release list` does not already contain `v0.12.2`.
3. Create and push tag `v0.12.2`.
4. Watch `.github/workflows/release.yml` until the release is published.
5. Download both DMGs from GitHub Releases, mount each one, drag the app into `/Applications`, launch it, and verify:
   - Gatekeeper accepts it without a workaround.
   - onboarding opens cleanly,
   - microphone/accessibility/input-monitoring prompts are understandable,
   - the app's About window (menu-bar tray) reports `0.12.2`.
6. Run the fail-closed payload gate on both downloaded artifacts before
   promoting them:
   - `make verify-dmg DMG=Codescribe_<ver>.dmg VARIANT=slim VERSION=<ver>`
   - `make verify-dmg DMG=Codescribe_<ver>_full.dmg VARIANT=full VERSION=<ver>`
     Refuse any DMG that fails (size floors catch missing MiniLM/Whisper even
     when the signature is valid).
7. Only then switch the landing CTA from source install to release DMG.

## Current Known External Gaps

- The latest live GitHub release observed on 2026-06-23 was `v0.8.0`, while the source version is `0.12.2`.
- GitHub license detection still needs final review because the active repository license is `FSL-1.1-ALv2` while GitHub may display Apache-2.0.
- GitHub Actions signing/notary secrets were not listed by `gh secret list` on 2026-06-23; configure them before tagging.
- The live GitHub Pages deployment still served the 2026-05-07 landing as of 2026-06-23; merge/deploy the branch before public announcement.
- A current signed and notarized `v0.12.2` DMG still needs to be produced by GitHub Actions and smoke-tested.

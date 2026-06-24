# Public Release Checklist

CodeScribe is close enough to go public only when the public surfaces tell the same truth as the code.

Current source version: `0.12.2`

## Must Be True Before Public Announcement

- [x] GitHub repository visibility is intentionally changed from private to public.
- [x] GitHub repository description matches the product:
      `Native macOS tray dictation and assistive voice overlay with local Whisper live preview.`
- [x] GitHub repository homepage points to `https://vetcoders.github.io/CodeScribe/`.
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
  - `CodeScribe_0.12.2.dmg` with embedded Silero + embedder and runtime Whisper cache/download.
  - `CodeScribe_0.12.2_full.dmg` with embedded Silero + embedder + Whisper.
- [ ] Both DMGs are Developer ID signed, notarized, stapled, and pass Gatekeeper on a machine outside the developer environment.
- [ ] Landing page primary CTA does not promise a DMG until a current notarized DMG exists.
- [ ] README install section names source install as the guaranteed path until the current DMG is verified.

## First Public Release Drill

1. Confirm the tree is clean and `make check` passes.
2. Confirm `gh release list` does not already contain `v0.12.2`.
3. Create and push tag `v0.12.2`.
4. Watch `.github/workflows/release.yml` until the release is published.
5. Download both DMGs from GitHub Releases, mount each one, drag the app into `/Applications`, launch it, and verify:
   - Gatekeeper accepts it without a workaround.
   - onboarding opens cleanly,
   - microphone/accessibility/input-monitoring prompts are understandable,
   - `codescribe --version` reports `0.12.2`.
6. Only then switch the landing CTA from source install to release DMG.

## Current Known External Gaps

- The latest live GitHub release observed on 2026-06-23 was `v0.8.0`, while the source version is `0.12.2`.
- GitHub license detection still needs final review because the active repository license is `FSL-1.1-ALv2` while GitHub may display Apache-2.0.
- GitHub Actions signing/notary secrets were not listed by `gh secret list` on 2026-06-23; configure them before tagging.
- The live GitHub Pages deployment still served the 2026-05-07 landing as of 2026-06-23; merge/deploy the branch before public announcement.
- A current signed and notarized `v0.12.2` DMG still needs to be produced by GitHub Actions and smoke-tested.

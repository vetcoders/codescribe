# Release & Sparkle Appcast Rollback Runbook

Operational runbook for withdrawing a broken notarized macOS release (`vX.Y.Z`)
and recovering clients across GitHub Releases, the Sparkle update feed
(`appcast.xml`), and the public website.

> **Authority:** Every action in this runbook is a one-way public distribution
> operation (**Always Human**). Agents may prepare the revert PR or run
> verification checks, but the maintainer executes release withdrawal and
> signing.

## 1. Stop new auto-updates immediately (`appcast.xml`)

Sparkle clients poll `appcast.xml` and compare `sparkle:version` (`CFBundleVersion`).
To halt automatic updates to a bad `vX.Y.Z` release within minutes:

1. Remove the offending `<item>` entry for `vX.Y.Z` from the published
   `appcast.xml` (or revert the release publication commit on `main` that
   updated the feed/site metadata) so the top `<item>` points back to the last
   known-good release `vX.Y.(Z-1)`.
2. Trigger the Pages deployment workflow:
   ```sh
   gh workflow run pages.yml --repo vetcoders/codescribe --ref main
   ```
3. Download the live `appcast.xml` feed to a temporary file and verify that its
   release identity matches `X.Y.(Z-1)` (and no longer advertises `X.Y.Z`):
   ```sh
   curl -fsSL https://github.com/vetcoders/codescribe/releases/latest/download/appcast.xml -o /tmp/appcast.xml
   python3 scripts/verify-appcast-version.py /tmp/appcast.xml --version X.Y.(Z-1)
   ```

## 2. Withdraw the GitHub Release asset (`gh release`)

Prevent direct downloads of the broken DMG from GitHub Releases and the website
download button:

1. Convert the broken GitHub Release `vX.Y.Z` to draft (or mark as pre-release
   while investigating) so `/releases/latest` resolves to `vX.Y.(Z-1)`:
   ```sh
   gh release edit vX.Y.Z --repo vetcoders/codescribe --draft
   ```
2. Verify `/releases/latest` points to the previous healthy tag:
   ```sh
   gh api repos/vetcoders/codescribe/releases/latest --jq .tag_name
   ```

## 3. Ship a forward fix (`vX.Y.(Z+1)`) for clients already on `vX.Y.Z`

Sparkle does **not** downgrade an installed app when an `<item>` is deleted from
`appcast.xml` — removing the item only stops new clients from upgrading into the
bad version. Clients that already installed `vX.Y.Z` need a release with a
**strictly higher `CFBundleVersion` (`sparkle:version`) and `CFBundleShortVersionString` (`vX.Y.(Z+1)`)**:

1. Either cherry-pick the fix onto the release branch or build the known-good
   tree stamped as `vX.Y.(Z+1)` with an incremented build number.
2. Build, sign, notarize, and verify the replacement DMG:
   ```sh
   make release-standard
   make verify-dmg DMG=Codescribe_vX.Y.(Z+1).dmg VARIANT=slim VERSION=X.Y.(Z+1)
   ```
3. Regenerate and sign `appcast.xml` with `scripts/generate-appcast.sh`:
   ```sh
   SPARKLE_ED_PRIVATE_KEY="<base64-ed25519-private-key>" \
   SPARKLE_ED_PUBLIC_KEY="<base64-ed25519-public-key>" \
     scripts/generate-appcast.sh . -o appcast.xml \
       --download-url-prefix "https://github.com/vetcoders/codescribe/releases/download/vX.Y.(Z+1)/"
   ```
4. Publish `vX.Y.(Z+1)` and verify both the local signed `appcast.xml` and the
   live published feed:
   ```sh
   python3 scripts/verify-appcast-version.py appcast.xml --version X.Y.(Z+1)
   curl -fsSL https://github.com/vetcoders/codescribe/releases/latest/download/appcast.xml -o /tmp/appcast-live.xml
   python3 scripts/verify-appcast-version.py /tmp/appcast-live.xml --version X.Y.(Z+1)
   bash tests/release_appcast_contract.sh
   python3 scripts/tests/test_appcast_version.py
   ```

## 4. Recovery when `vX.Y.Z` crashes before Sparkle can check for updates

If the defect in `vX.Y.Z` aborts on launch before Sparkle's updater initializes:

1. User data (`~/.codescribe/config/settings.json`, `~/.codescribe/agent-bridge/`,
   history, and custom lexicon) and macOS Keychain credentials live outside
   `/Applications/Codescribe.app` and remain intact across bundle replacement.
2. Direct affected users to download `Codescribe_vX.Y.(Z+1).dmg` (or
   `vX.Y.(Z-1)`) from `https://github.com/vetcoders/codescribe/releases/latest`
   and drag `Codescribe.app` into `/Applications` (replacing the broken bundle).
3. Verify the recovered local installation:
   ```sh
   bin/control-codescribe health --sha <expected-commit-sha> --json
   ```

𝚅𝚒𝚋𝚎𝚌𝚛𝚊𝚏𝚝𝚎𝚍. with AI Agents by Vetcoders (c)2024-2026 LibraxisAI

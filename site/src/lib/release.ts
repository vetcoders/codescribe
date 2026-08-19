// Single source of truth for what the site offers a stranger to download.
//
// Both CTAs on the landing page (Hero and Install) must point at the SAME
// artifact: two buttons on one page that resolve to different builds is the
// defect review P1-4 set out to kill, and it was only half-closed — Install
// was pinned while Hero still used the floating `releases/latest/` alias.
//
// SITE_VERSION must name a version that is PUBLISHED on GitHub Releases, not
// the version built locally. `.github/workflows/pages.yml` parses SITE_VERSION
// out of this file and curls the asset with --fail before building, so a pin
// that runs ahead of the published release fails the whole Pages deploy rather
// than shipping a dead button. Bump this only in the move that publishes the tag.
export const SITE_VERSION = "0.13.3";

export const DOWNLOAD_URL = `https://github.com/vetcoders/codescribe/releases/download/v${SITE_VERSION}/Codescribe.dmg`;

// Decimal MB, matching how Finder and browser download managers report size.
// The published asset is the standard (slim) build: Silero + embedder embedded,
// Whisper downloads on first use — a stranger on a metered connection deserves
// to know that before clicking.
export const DOWNLOAD_SIZE = "530 MB";

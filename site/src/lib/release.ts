import { loadLatestRelease } from "./github-release.ts";

// Resolve once during the static build. Every CTA and its version/size label
// come from the same published GitHub release; local app versions are irrelevant.
const release = await loadLatestRelease();

export const SITE_VERSION = release.version;
export const DOWNLOAD_URL = release.url;
export const DOWNLOAD_SIZE = release.size;

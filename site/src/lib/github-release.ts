const REPOSITORY_URL = "https://github.com/vetcoders/codescribe";
const RELEASES_API =
  "https://api.github.com/repos/vetcoders/codescribe/releases";

export interface ReleaseDownload {
  version: string;
  url: string;
  size: string;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null;
}

// A published stable release and its uploaded DMG are one download offer.
// Refuse missing or inconsistent metadata instead of advertising an old build.
export function parseLatestRelease(value: unknown): ReleaseDownload {
  if (
    !isRecord(value) ||
    value.draft !== false ||
    value.prerelease !== false ||
    typeof value.tag_name !== "string" ||
    !/^v?\d+\.\d+\.\d+$/.test(value.tag_name) ||
    !Array.isArray(value.assets)
  ) {
    throw new Error(
      "GitHub did not return a published stable Codescribe release"
    );
  }
  const assets = value.assets.filter(
    (asset: unknown) => isRecord(asset) && asset.name === "Codescribe.dmg"
  );
  const dmg: unknown = assets[0];
  const url = `${REPOSITORY_URL}/releases/download/${encodeURIComponent(
    value.tag_name
  )}/Codescribe.dmg`;
  if (
    assets.length !== 1 ||
    !isRecord(dmg) ||
    dmg.state !== "uploaded" ||
    typeof dmg.size !== "number" ||
    !Number.isSafeInteger(dmg.size) ||
    dmg.size <= 0 ||
    dmg.browser_download_url !== url
  ) {
    throw new Error(
      "Latest Codescribe release has no valid uploaded Codescribe.dmg"
    );
  }
  return {
    version: value.tag_name.replace(/^v/, ""),
    url,
    size: `${Math.ceil(dmg.size / 1_000_000)} MB`,
  };
}

type FetchRelease = (url: string, options?: RequestInit) => Promise<Response>;

export function selectNewestPublishedRelease(values: unknown[]): unknown {
  let newest: unknown;
  let newestTime = Number.NEGATIVE_INFINITY;
  for (const value of values) {
    if (
      !isRecord(value) ||
      value.draft !== false ||
      value.prerelease !== false
    ) {
      continue;
    }
    const publishedTime =
      typeof value.published_at === "string"
        ? Date.parse(value.published_at)
        : Number.NaN;
    if (!Number.isFinite(publishedTime)) {
      throw new Error(
        "Stable Codescribe release has no valid publication time"
      );
    }
    if (publishedTime > newestTime) {
      newest = value;
      newestTime = publishedTime;
    }
  }
  if (newest === undefined) {
    throw new Error("GitHub returned no published stable Codescribe release");
  }
  return newest;
}

export async function loadLatestRelease(
  request: FetchRelease = fetch
): Promise<ReleaseDownload> {
  const releases: unknown[] = [];
  for (let page = 1; ; page++) {
    const response = await request(
      `${RELEASES_API}?per_page=100&page=${page}`,
      {
        headers: {
          Accept: "application/vnd.github+json",
          "X-GitHub-Api-Version": "2022-11-28",
          "User-Agent": "codescribe-website",
        },
        signal: AbortSignal.timeout(15_000),
      }
    );
    if (!response.ok) {
      throw new Error(
        `Cannot resolve latest Codescribe release: GitHub HTTP ${response.status}`
      );
    }
    const values: unknown = await response.json();
    if (!Array.isArray(values)) {
      throw new Error("GitHub did not return a release list");
    }
    releases.push(...values);
    if (values.length < 100) break;
  }
  // A newly published release may refer to an older commit. Never select by
  // commit creation time or silently return an older DMG when this one is bad.
  const release = parseLatestRelease(selectNewestPublishedRelease(releases));
  const asset = await request(release.url, {
    method: "HEAD",
    redirect: "follow",
    signal: AbortSignal.timeout(15_000),
  });
  if (!asset.ok) {
    throw new Error(
      `Published Codescribe DMG is unavailable: HTTP ${asset.status}`
    );
  }
  return release;
}

const REPOSITORY_URL = "https://github.com/vetcoders/codescribe";
const LATEST_RELEASE_API =
  "https://api.github.com/repos/vetcoders/codescribe/releases/latest";

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

export async function loadLatestRelease(
  request: FetchRelease = fetch
): Promise<ReleaseDownload> {
  const response = await request(LATEST_RELEASE_API, {
    headers: {
      Accept: "application/vnd.github+json",
      "X-GitHub-Api-Version": "2022-11-28",
      "User-Agent": "codescribe-website",
    },
    signal: AbortSignal.timeout(15_000),
  });
  if (!response.ok) {
    throw new Error(
      `Cannot resolve latest Codescribe release: GitHub HTTP ${response.status}`
    );
  }
  const release = parseLatestRelease(await response.json());
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

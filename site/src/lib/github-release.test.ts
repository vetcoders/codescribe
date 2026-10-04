import assert from "node:assert/strict";
import test from "node:test";
import { loadLatestRelease, parseLatestRelease } from "./github-release.ts";

function publishedRelease() {
  return {
    tag_name: "v0.16.0",
    draft: false,
    prerelease: false,
    assets: [
      {
        name: "appcast.xml",
        state: "uploaded",
        size: 908,
        browser_download_url: "",
      },
      {
        name: "Codescribe.dmg",
        state: "uploaded",
        size: 41_891_591,
        browser_download_url:
          "https://github.com/vetcoders/codescribe/releases/download/v0.16.0/Codescribe.dmg",
      },
    ],
  };
}

test("version, pinned URL and decimal size come from one future release", () => {
  assert.deepEqual(parseLatestRelease(publishedRelease()), {
    version: "0.16.0",
    url: "https://github.com/vetcoders/codescribe/releases/download/v0.16.0/Codescribe.dmg",
    size: "42 MB",
  });
});

test("draft and prerelease metadata cannot become a stable download", () => {
  for (const key of ["draft", "prerelease"] as const) {
    const release = publishedRelease();
    release[key] = true;
    assert.throws(() => parseLatestRelease(release), /published stable/);
  }
});

test("missing, duplicate or incompletely uploaded DMGs refuse publication", () => {
  const missing = publishedRelease();
  missing.assets.pop();
  assert.throws(() => parseLatestRelease(missing), /valid uploaded/);
  const duplicate = publishedRelease();
  duplicate.assets.push({ ...duplicate.assets[1] });
  assert.throws(() => parseLatestRelease(duplicate), /valid uploaded/);
  const uploading = publishedRelease();
  uploading.assets[1].state = "new";
  assert.throws(() => parseLatestRelease(uploading), /valid uploaded/);
});

test("a DMG from a different release or repository is rejected", () => {
  for (const url of [
    "https://github.com/vetcoders/codescribe/releases/download/v0.13.3/Codescribe.dmg",
    "https://example.com/Codescribe.dmg",
  ]) {
    const release = publishedRelease();
    release.assets[1].browser_download_url = url;
    assert.throws(() => parseLatestRelease(release), /valid uploaded/);
  }
});

test("invalid, empty and fractional asset sizes cannot become a size label", () => {
  for (const size of [0, -1, 1.5, Number.NaN, Number.POSITIVE_INFINITY]) {
    const release = publishedRelease();
    release.assets[1].size = size;
    assert.throws(() => parseLatestRelease(release), /valid uploaded/);
  }
});

test("malformed API payloads and tags are refused", () => {
  for (const value of [
    null,
    [],
    {},
    { ...publishedRelease(), tag_name: "../other" },
  ]) {
    assert.throws(() => parseLatestRelease(value), /published stable/);
  }
});

test("latest release is queried and its exact DMG is checked before advertising", async () => {
  const calls: Array<{ url: string; options?: RequestInit }> = [];
  const release = await loadLatestRelease(async (url, options) => {
    calls.push({ url, options });
    return options?.method === "HEAD"
      ? new Response(null, { status: 200 })
      : Response.json(publishedRelease());
  });
  assert.equal(calls.length, 2);
  assert.equal(
    calls[0].url,
    "https://api.github.com/repos/vetcoders/codescribe/releases/latest"
  );
  assert.equal(calls[1].url, release.url);
  assert.equal(calls[1].options?.method, "HEAD");
  assert.equal(calls[1].options?.redirect, "follow");
});

test("GitHub API failure refuses the build instead of returning a stale release", async () => {
  await assert.rejects(
    loadLatestRelease(async () => new Response(null, { status: 403 })),
    /GitHub HTTP 403/
  );
});

test("missing published asset refuses the build", async () => {
  await assert.rejects(
    loadLatestRelease(async (_url, options) =>
      options?.method === "HEAD"
        ? new Response(null, { status: 404 })
        : Response.json(publishedRelease())
    ),
    /DMG is unavailable: HTTP 404/
  );
});

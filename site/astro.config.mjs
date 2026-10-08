// @ts-check
import { defineConfig } from "astro/config";
import process from "node:process";

const isPagesDeployment = process.env.PAGES_DEPLOYMENT === "true";

// Caddy serves the canonical site at /. Pages serves its copy at /codescribe/.
// Canonical site URL is always https://codescribe.vetcoders.io for sitemaps and indexing.
export default defineConfig({
  site: "https://codescribe.vetcoders.io",
  base: isPagesDeployment ? "/codescribe" : "/",
  trailingSlash: "ignore",
  build: {
    // Emit index.html at the site root (no /page/index.html rewrites needed here).
    format: "directory",
  },
});

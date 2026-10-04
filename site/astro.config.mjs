// @ts-check
import { defineConfig } from "astro/config";
import process from "node:process";

const isPagesDeployment = process.env.PAGES_DEPLOYMENT === "true";

// Caddy serves the canonical site at /. Pages serves its copy at /codescribe/.
// Public assets and navigation use the resulting BASE_URL through asset.ts.
export default defineConfig({
  site: isPagesDeployment
    ? "https://vetcoders.github.io"
    : "https://codescribe.vetcoders.io",
  base: isPagesDeployment ? "/codescribe" : "/",
  trailingSlash: "ignore",
  build: {
    // Emit index.html at the site root (no /page/index.html rewrites needed here).
    format: "directory",
  },
});

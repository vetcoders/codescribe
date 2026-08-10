// @ts-check
import { defineConfig } from "astro/config";

// Canonical host: https://codescribe.vetcoders.io — static files under Caddy on
// the ops VPS, same pattern as pensieve.vetcoders.io (operator, 2026-08-09).
// The GitHub Pages project path (vetcoders.github.io/codescribe) was never
// live: Pages deploys from main, site/ lived only on feature branches, and the
// baked SUFeedURL 404'd for every build that carried it.
// Asset paths still route through src/lib/asset.ts (BASE_URL-aware), so a
// future base change stays a one-line edit here.
export default defineConfig({
  site: "https://codescribe.vetcoders.io",
  base: "/",
  trailingSlash: "ignore",
  build: {
    // Emit index.html at the site root (no /page/index.html rewrites needed here).
    format: "directory",
  },
});

// @ts-check
import { defineConfig } from 'astro/config';

// GitHub Pages project site: https://vetcoders.github.io/codescribe
// Every reference to public/ assets must be prefixed with import.meta.env.BASE_URL
// (see src/lib/asset.ts) so it resolves under /codescribe in production.
export default defineConfig({
  site: 'https://vetcoders.github.io',
  base: '/codescribe',
  trailingSlash: 'ignore',
  build: {
    // Emit index.html at the site root (no /page/index.html rewrites needed here).
    format: 'directory',
  },
});

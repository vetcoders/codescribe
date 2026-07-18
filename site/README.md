# codescribe — marketing website

Static, dark, cinematic marketing site for **codescribe** (a macOS voice control
layer for text, code & AI agents). Built with **Astro** + TypeScript, plain CSS
with custom properties, self-hosted fonts, and exactly two tiny interactive
islands. No Tailwind, no UI libraries.

Design source of truth: `WEBSITE_SPEC.md` in the design handoff. The approved
render is `reference/site-a-cinematic.html`.

## Run / build / preview

```bash
cd site
npm ci            # install exact deps from package-lock.json
npm run dev       # local dev server (http://localhost:4321/codescribe)
npm run build     # static output → site/dist
npm run preview   # serve the built dist locally
npm run check     # astro check (TypeScript / template diagnostics)
```

> The site is configured with `base: '/codescribe'`, so in dev **and** preview
> the site lives under `/codescribe` (e.g. `http://localhost:4321/codescribe`),
> matching production.

## Deploy

GitHub Pages **project site** at `https://vetcoders.github.io/codescribe`.

- `astro.config.mjs` sets `site: 'https://vetcoders.github.io'` and
  `base: '/codescribe'`.
- `.github/workflows/pages.yml` (repo root) builds `site/` and deploys
  `site/dist` on every push to `main` that touches `site/**`. No manual step.

Because of the base path, **never** hardcode `/shots/...` or `/icon.png`. Route
every `public/` asset through the helper in `src/lib/asset.ts`:

```astro
---
import { asset } from '../lib/asset';
---
<img src={asset('shots/overlay-final-transparent.webp')} … />
```

`asset()` prefixes `import.meta.env.BASE_URL` so paths resolve under `/codescribe`
in production. A hardcoded absolute path would 404 on Pages.

## Where the design tokens live

All color / font tokens are centralized as CSS custom properties in **one
`:root`** block in `src/styles/global.css` (ported from `WEBSITE_SPEC.md` §2).
The ambient keyframes (`breathe`, `ripple`, `softpulse`, `glowpulse`, `drift`,
`floatIn`) and the global `prefers-reduced-motion` handling also live there.
Component styles are scoped `<style>` blocks that reference the tokens via
`var(--…)`; change a token once and it propagates everywhere.

## Structure

```
site/
├── astro.config.mjs        # site + base (/codescribe), static output
├── public/
│   ├── icon.png            # brand mark
│   ├── shots/*.webp        # product screenshots (transparent variants)
│   ├── robots.txt
│   └── sitemap.xml
└── src/
    ├── layouts/Layout.astro    # head / meta / OG / fonts / global CSS
    ├── lib/asset.ts            # base-path-aware public asset helper
    ├── styles/global.css       # design tokens + keyframes + reduced-motion
    ├── pages/index.astro       # assembles all sections
    └── components/             # one component per section
        ├── Nav.astro
        ├── Hero.astro            # interactive island A: live console
        ├── LivesOverYourWork.astro
        ├── Modes.astro           # interactive island B: modes spotlight
        ├── Formatting.astro      # verbatim Polish copy (needs latin-ext)
        ├── Selection.astro
        ├── AgentChat.astro
        ├── VoiceDrawer.astro
        ├── Prompts.astro
        ├── AgentStack.astro
        ├── ControlLayer.astro
        ├── MacNative.astro
        ├── Install.astro
        └── Footer.astro
```

## How to swap a screenshot

1. Drop the new file into `site/public/shots/` (keep the `-transparent.webp`
   naming; the dark site relies on transparent window edges).
2. Read its intrinsic dimensions so the `width`/`height` attributes stay
   correct (prevents layout shift / CLS):

   ```bash
   sips -g pixelWidth -g pixelHeight site/public/shots/your-shot.webp
   ```

3. Update the corresponding component (e.g. `MacNative.astro`,
   `AgentChat.astro`, `LivesOverYourWork.astro`, `Prompts.astro`): change the
   `src` via `asset('shots/your-shot.webp')` and set the new `width`/`height`.
4. `npm run build` and spot-check.

Two image slots are known placeholders (see `WEBSITE_SPEC.md` §5): the "Lives
over your work" overlay-over-another-app shot and the dedicated Voice Drawer
capture (`VoiceDrawer.astro` currently reuses `agent-threads-transparent.webp`).
Swap 1:1 when real captures land; alt text is already correct.

## Interactive islands

Both live as small inline `<script type="module">` blocks (Astro bundles them):

- **Hero live console** (`Hero.astro`) — word-by-word reveal of raw speech →
  structured intent, on a loop. Server-renders the resolved frame.
- **Modes spotlight** (`Modes.astro`) — color-cycling pill + rotating example.

Both honor `prefers-reduced-motion`: the script bails out and leaves the
server-rendered resolved frame; the global CSS media query freezes the ambient
keyframes. Timings and word lists are per `WEBSITE_SPEC.md` §6.

## Fonts

Self-hosted via `@fontsource` (Space Grotesk 400/500/600/700, JetBrains Mono
400/500/600), imported in `Layout.astro`. Per-weight CSS includes the
`latin-ext` subset via `unicode-range`, so Polish glyphs render without hotlinking
Google Fonts. `font-display: swap` is the @fontsource default.

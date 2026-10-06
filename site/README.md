# codescribe — marketing website

Static product website for **Codescribe**: dictation, text editing, and
connected agent conversations on macOS. Built with **Astro** + TypeScript, plain CSS
with custom properties, self-hosted fonts, and exactly two tiny interactive
islands. No Tailwind, no UI libraries.

## Product copy and evidence

The public copy describes user actions and setup requirements. It must not
turn a source invariant, a single demo, or a delivery acknowledgment into an
accuracy, latency, privacy, or task-completion guarantee. Animated examples
are labeled as illustrations and carry no measured performance numbers.

Local speech recognition, optional AI processing, remote agent sessions,
spoken replies, and website licence issuance have separate data boundaries.
Keep the privacy page consistent with those paths. The licence issuer receives
the email over HTTPS and logs its hash, client IP, and issue time. The Pages
copy links to the canonical licence form rather than posting to a missing API.

The homepage presents the workflow, product controls, actual screenshots,
setup, and a published download. `/fleet/` explains agent channels and their
prerequisites. Transport and internal ledger machinery are engineering details.

## Run / build / preview

```bash
cd site
npm ci            # install exact deps from package-lock.json
npm run dev       # local dev server (http://localhost:4321)
npm run build     # static output → site/dist
npm run preview   # serve the built dist locally
npm run check     # astro check (TypeScript / template diagnostics)
```

> The site is configured with `base: '/'`, so in dev **and** preview
> the site lives at `/` (e.g. `http://localhost:4321`),
> matching production.

Download links, displayed version and DMG size are resolved together from
GitHub's most recently published stable release during the build, selected by
`published_at` across the paginated release list, regardless of commit age. The build requires
outbound access to GitHub and checks that the uploaded `Codescribe.dmg` is
reachable; missing metadata or assets stop publication rather than retaining a
stale version. Run `npm run test:release` for the hermetic release contract tests.
All download buttons share this build snapshot, including without JavaScript.

## Deploy

Production is `https://codescribe.vetcoders.io`, served by Caddy from
`/srv/codescribe-landing` on `libraxis-vm`. This host also serves the signed
`/appcast.xml` used by the installed application. Use the site redeploy commands
in [the deployment contract](../services/license-issuer/README.md#deployment-ops-vps-same-box-as-pensievevetcodersio).

- `astro.config.mjs` defaults to `site: 'https://codescribe.vetcoders.io'` and `base: '/'`.
- `.github/workflows/pages.yml` publishes a separate copy at
  `https://vetcoders.github.io/codescribe/`, setting `PAGES_DEPLOYMENT=true` so
  assets and navigation use `/codescribe/`. To verify this build locally, run
  `PAGES_DEPLOYMENT=true npm run build`. A successful Pages run does not
  update the production domain or its Sparkle feed.
- After production deployment, verify all download links and the signed
  enclosure at the canonical domain against the published GitHub release.
- A published GitHub release also triggers the Pages rebuild using current
  `main` website source. This updates the GitHub Pages copy; the canonical Caddy
  site still requires the production redeploy described above.

Route every `public/` asset through `src/lib/asset.ts`:

```astro
---
import { asset } from '../lib/asset';
---
<img src={asset('shots/overlay-final-transparent.webp')} … />
```

`asset()` prefixes `import.meta.env.BASE_URL`, keeping assets aligned with the
configured site root.

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
├── astro.config.mjs        # canonical site + base (/), static output
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
        ├── Prompts.astro
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

Product screenshots must describe what the image actually shows. Do not reuse
an agent-thread capture as evidence of a different history interface.

## Interactive islands

Both live as small inline `<script type="module">` blocks (Astro bundles them):

- **Hero live console** (`Hero.astro`) — word-by-word reveal of raw speech →
  a punctuation example, on a loop. Server-renders the readable frame.
- **Modes spotlight** (`Modes.astro`) — color-cycling pill + rotating example.

Both honor `prefers-reduced-motion`: the script bails out and leaves the
server-rendered resolved frame; the global CSS media query freezes the ambient
keyframes. Timings and word lists are per `WEBSITE_SPEC.md` §6.

## Fonts

Self-hosted via `@fontsource` (Space Grotesk 400/500/600/700, JetBrains Mono
400/500/600), imported in `Layout.astro`. Per-weight CSS includes the
`latin-ext` subset via `unicode-range`, so Polish glyphs render without hotlinking
Google Fonts. `font-display: swap` is the @fontsource default.

# Showcase Site — Design Spec

**Date:** 2026-05-02
**Status:** Draft, pending implementation plan
**Author:** Brainstorming session, 2026-05-02

## Goal

Ship a single-page marketing site at `https://seanbarzilay.github.io/transcoderr/`
that turns repo visitors and HN/Reddit referrals into installs. Out-of-scope: a
documentation portal — the existing `docs/*.md` files render fine on GitHub.

## Tech stack

- **Astro 4.x** — static-first, zero-runtime by default. The page is mostly
  content; no client-side interactivity beyond a "copy" button.
- **`@astrojs/tailwind`** — Tailwind for styling. Hand-writing CSS for one
  page wastes time; component classes keep markup readable.
- **`astro-icon`** — icon component, lucide icon set.
- **`sharp`** — Node image library used by both Astro's `<Image/>` component
  and our redaction script.
- **`oxipng`** (CLI, called from a build hook) — lossless PNG re-compression.
- **No JS framework** (no React/Vue/Svelte). Astro components are enough.

## Hosting

- **Source on `main`** under `site/` (isolated from `crates/`, `web/`, `docs/`).
- **GitHub Actions** (`.github/workflows/pages.yml`) builds `site/` and
  publishes via `actions/deploy-pages`. Pages source = "GitHub Actions".
- **URL:** `https://seanbarzilay.github.io/transcoderr/`. Astro `base: '/transcoderr'`.
- Custom domain explicitly out of scope; layerable later via CNAME.

## Repo layout

```
site/
  ├── src/
  │   ├── pages/index.astro              # the one page
  │   ├── components/
  │   │   ├── Hero.astro
  │   │   ├── FlowDiagram.astro          # SVG version of README ASCII diagram
  │   │   ├── FeatureGrid.astro
  │   │   ├── FeatureCard.astro
  │   │   ├── ScreenshotShowcase.astro   # tabbed screenshot viewer
  │   │   ├── CodeSplit.astro            # YAML | ffmpeg side-by-side
  │   │   ├── PluginShowcase.astro
  │   │   ├── McpSection.astro
  │   │   ├── DeployBlock.astro          # docker-compose + copy button
  │   │   ├── FlavorTable.astro
  │   │   └── Footer.astro
  │   ├── layouts/Base.astro             # html/head, dark background, fonts
  │   ├── styles/global.css              # tailwind directives, CSS vars
  │   └── content/
  │       ├── flow-example.yaml
  │       ├── compose.yaml
  │       └── ffmpeg-invocation.txt
  ├── public/
  │   ├── screenshots/                   # committed, redacted PNGs
  │   ├── og-image.png                   # 1200×630 social card
  │   └── favicon.svg
  ├── astro.config.mjs                   # site, base, integrations
  ├── tailwind.config.mjs
  ├── package.json
  └── .gitignore                         # screenshots/raw/

scripts/
  ├── capture.mjs                        # agent-browser → raw PNGs
  ├── redact.mjs                         # raw → redacted PNGs
  ├── shots.config.mjs                   # capture targets
  └── redact.config.mjs                  # per-image redaction regions

.github/workflows/pages.yml              # build + deploy
```

## Page structure

One page, 9 sections, top-to-bottom. Each component has a single
responsibility and is independently testable.

### 1. Hero (`<Hero/>`)
- Wordmark "transcoderr" + tagline: *"Push-driven, single-binary media transcoder."*
- Sub: *"Webhook in. ffmpeg out. One pass. Configurable in between."*
- Buttons: **Download** → latest GitHub release · **GitHub** · **Docs**
- Below the fold: `<FlowDiagram/>`, an SVG version of the README's ASCII
  diagram (Radarr/Sonarr → transcoderr → .mkv replaced).

### 2. What it does (`<FeatureGrid/>` of `<FeatureCard/>`)
Six cards, each ~2 sentences + lucide icon:
1. **Push-driven** — Typed webhook adapters for Radarr/Sonarr/Lidarr plus
   a generic `/webhook/:name`. No library scanning.
2. **Plan-then-execute** — Compose declarative `plan.*` steps; one
   `plan.execute` materializes the whole flow into a single ffmpeg call.
3. **Hardware-aware** — Boot-time probe of NVENC/QSV/VAAPI/VideoToolbox,
   per-device concurrency, runtime CPU fallback.
4. **Live observability** — Per-run progress bar, ffmpeg status streamed
   live, structured timeline, Prometheus `/metrics`.
5. **Plugins** — Browse a catalog, click Install, sha256-verified,
   live-reloads without restart. JSON-RPC over stdin/stdout.
6. **Single binary** — Rust + embedded SQLite + embedded React SPA. One
   image, one volume, no broker, no external DB.

### 3. See it work (`<ScreenshotShowcase/>`)
Tabbed viewer cycling 5 screenshots, each with a one-line caption:
- Runs dashboard
- Live run with progress bar mid-encode
- Flows editor (YAML left, visual mirror right)
- Browse & manual transcode (with codec/resolution filters)
- Plugins → Browse

### 4. One ffmpeg pass (`<CodeSplit/>`)
Two-column. Left: abridged `hevc-normalize.yaml` (loaded from
`content/flow-example.yaml`). Right: the resulting ffmpeg invocation
(loaded from `content/ffmpeg-invocation.txt`). One-paragraph caption
explaining no chained tmp files.

### 5. Plugins (`<PluginShowcase/>`)
One paragraph + Plugins → Browse and Plugins → Installed screenshots
side-by-side. Link to `seanbarzilay/transcoderr-plugins`.

### 6. MCP / AI (`<McpSection/>`)
Pitch paragraph + Claude Desktop config snippet (with `tcr_xxxxxxxx`
placeholder) + bullet list of MCP tools. No AI client screenshot.

### 7. Deploy in 30 seconds (`<DeployBlock/>`)
The docker-compose snippet from README's Quickstart, syntax-highlighted
via Astro's built-in shiki, with a "Copy" button (the only client-side
JS on the page).

### 8. Image flavors (`<FlavorTable/>`)
Same table from README. Two rows beneath showing example pull commands.

### 9. Footer (`<Footer/>`)
License placeholder · GitHub · Docs · Releases · small Hardware-page
thumbnail link.

## Screenshots

Captured manually on demand, not in CI. Server: `http://192.168.1.176:8099`.

| # | Name (file slug) | Page / URL | Used in |
|---|---|---|---|
| 1 | `runs-dashboard` | `/` | Section 3 |
| 2 | `live-run` | `/runs/<id>` mid-encode | Section 3 |
| 3 | `flows-editor` | `/flows/<id>/edit` | Sections 3, 4 |
| 4 | `browse-manual` | `/browse/<source>` | Section 3 |
| 5 | `plugins-browse` | `/plugins#browse` | Sections 3, 5 |
| 6 | `plugins-installed` | `/plugins#installed` | Section 5 |
| 7 | `plugins-catalogs` | `/plugins#catalogs` | Section 5 |
| 8 | `sources-list` | `/settings/sources` | Section 2 (push-driven card) |
| 9 | `notifiers-list` | `/settings/notifiers` | Section 6 (MCP) |
| 10 | `hardware-probe` | `/settings/hw` | Section 2 (hardware card) + footer |

### Capture flow (`scripts/capture.mjs`)

1. agent-browser drives Chromium against the prod server.
2. Login: env `TRANSCODERR_PASSWORD`, fill `/login`, wait for session cookie.
3. For #2 (live-run), capture script runs a `triggerTranscode` setup hook
   first: POST `/api/transcode/file` with a small file path provided via
   env `TRANSCODERR_LIVE_TRANSCODE_PATH`, then navigate to the new run
   page and wait ~5s for the progress bar and ffmpeg status line.
4. Each entry in `scripts/shots.config.mjs` declares: `name`, `url`,
   `viewport`, optional `wait` selector, optional `setup` hook.
5. Output: `site/public/screenshots/raw/<name>.png`. `raw/` is gitignored.

### Redaction (`scripts/redact.mjs`)

- Per-image config in `scripts/redact.config.mjs` lists rectangles
  `{ x, y, w, h, label }` to overlay.
- `sharp` draws **solid black fills**, not blur. Blur can be
  reverse-engineered; solid fills can't.
- Output: `site/public/screenshots/<name>.png` (committed).
- **Manual review gate:** before commit, every PNG in
  `site/public/screenshots/` is visually inspected.
- Known sensitive regions:
  - `sources-list`: secret token columns / fields
  - `notifiers-list`: bot tokens, webhook URLs, ntfy auth
  - `plugins-catalogs`: `auth_header` (server already redacts to `***`,
    but verify)
  - `live-run`: file paths *may* expose library layout; redact at
    operator's discretion during review

### Image optimization

- Astro `<Image/>` component generates `srcset` and lazy-loads.
- Build hook runs `oxipng -o 4` over `dist/_astro/*.png`. Average
  1440×900 dashboard PNG → ~150-300KB.

## Build & deploy

`.github/workflows/pages.yml`:

```yaml
name: pages
on:
  push:
    branches: [main]
    paths: ['site/**', '.github/workflows/pages.yml']
  workflow_dispatch:
permissions:
  contents: read
  pages: write
  id-token: write
concurrency:
  group: pages
  cancel-in-progress: true
jobs:
  build:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: actions/setup-node@v4
        with: { node-version: 20, cache: npm, cache-dependency-path: site/package-lock.json }
      - run: npm ci
        working-directory: site
      - run: npm run build
        working-directory: site
      - uses: actions/upload-pages-artifact@v3
        with: { path: site/dist }
  deploy:
    needs: build
    runs-on: ubuntu-latest
    environment:
      name: github-pages
      url: ${{ steps.deployment.outputs.page_url }}
    steps:
      - id: deployment
        uses: actions/deploy-pages@v4
```

After first merge: Settings → Pages → Source = "GitHub Actions" (manual,
once).

## Styling

- **Palette:** dark background `#0b0d10` (matches transcoderr UI),
  surface `#14171c`, accent `#7dd3fc` (cyan), muted text `#94a3b8`.
- **Typography:** system font stack, mono code via `ui-monospace, …,
  monospace`. Headlines slightly tracked-out, weight 600.
- **Layout:** max-width 72ch for prose blocks, full-bleed for
  screenshot showcase. Subtle 1px grid background as page texture.

## Copy

All marketing copy drafted directly in this spec / the implementation
plan, then reviewed before merge. Tone: terse and technical, matches
README ("ffmpeg in", "one binary, one volume"). No buzzwords.

## Out of scope

- Multi-page documentation site (could layer later under `/docs/*`).
- Custom domain.
- Analytics / Plausible / GA — not needed for v1.
- Dark/light mode toggle — dark only.
- Animated GIFs or autoplaying videos. Static screenshots only.
- A "demo" / live-instance link — exposes the prod server.
- AI-client screenshot for the MCP section.
- CI capture of screenshots (kept manual; capture script is for
  operator use, not GH Actions).

## Risks

- **Token leak via screenshot.** Mitigations: post-capture solid-fill
  redaction + manual review of every PNG before commit. Risk remains
  if redaction config misses a region — manual review is the backstop.
- **Astro upgrade churn.** Pin major version in `package.json`; Astro
  has stable APIs at 4.x.
- **Page bloat.** All 10 screenshots compressed → ~3MB total. Lazy-load
  everything below the fold. First contentful paint stays fast.

## Success criteria

- v1 ships at `https://seanbarzilay.github.io/transcoderr/`.
- All 10 screenshots present, redacted, manually reviewed.
- Lighthouse: Performance ≥ 90, Accessibility ≥ 95, no console errors.
- Page weight ≤ 3MB total, first contentful paint < 1.5s on a slow
  cable connection.
- README's "Documentation" list links the site.

# Showcase Site Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship a single-page marketing site at `https://seanbarzilay.github.io/transcoderr/` showcasing what transcoderr does, with redacted screenshots from the live server.

**Architecture:** Astro 4 + Tailwind static site under `site/` (no JS framework, only one client-side script for a Copy button). 9 components, one per section. GitHub Actions builds and deploys via `actions/deploy-pages`. Screenshots captured via `scripts/capture.mjs` (operator-driven, not in CI) and post-redacted with solid black fills via `scripts/redact.mjs` before commit.

**Tech Stack:** Astro 4.x, Tailwind 3.x via `@astrojs/tailwind`, `astro-icon` + `@iconify-json/lucide` for icons, `sharp` for redaction (and built into Astro's `<Image/>`), `oxipng` (apt) for build-time PNG compression, `playwright` for screenshot capture, GitHub Actions for build + deploy.

**Branch:** all tasks land on a fresh `feat/showcase-site` branch off `main`. Implementer creates the branch before Task 1 and stays on it through Task 17. PR opens after Track A (Tasks 1-14) so reviewers can see the site working with placeholder screenshots; Track B (Tasks 15-17) lands as additional commits on the same branch.

---

## File Structure

**New files (Track A — build):**
- `site/package.json` — Astro project deps and scripts
- `site/astro.config.mjs` — `site:`, `base:`, integrations
- `site/tailwind.config.mjs` — dark palette tokens
- `site/tsconfig.json` — Astro defaults
- `site/.gitignore` — `node_modules/`, `dist/`, `public/screenshots/raw/`, `.astro/`
- `site/src/layouts/Base.astro` — html/head, fonts, page background
- `site/src/styles/global.css` — Tailwind directives + base styles
- `site/src/pages/index.astro` — the one page; composes the 9 components
- `site/src/components/Hero.astro`
- `site/src/components/FlowDiagram.astro` — inline SVG
- `site/src/components/FeatureGrid.astro`
- `site/src/components/FeatureCard.astro` — single card, takes props
- `site/src/components/ScreenshotShowcase.astro` — radio-button CSS tabs (no JS)
- `site/src/components/CodeSplit.astro` — YAML | ffmpeg side-by-side
- `site/src/components/PluginShowcase.astro`
- `site/src/components/McpSection.astro`
- `site/src/components/DeployBlock.astro` — compose snippet + Copy button
- `site/src/components/FlavorTable.astro`
- `site/src/components/Footer.astro`
- `site/src/content/flow-example.yaml`
- `site/src/content/ffmpeg-invocation.txt`
- `site/src/content/compose.yaml`
- `site/public/favicon.svg`
- `site/public/og-image.png` — 1200×630 social card (placeholder ok for v1)
- `site/public/screenshots/.gitkeep`
- `.github/workflows/pages.yml`

**New files (Track B — screenshots):**
- `scripts/capture.mjs` — Playwright-driven screenshot capture
- `scripts/redact.mjs` — sharp-based redaction
- `scripts/placeholders.mjs` — one-shot generator for dev-time PNG placeholders
- `scripts/shots.config.mjs` — 10-entry capture target list
- `scripts/redact.config.mjs` — per-image rectangles
- `scripts/package.json` — Playwright + sharp deps (separate from `site/`)
- `scripts/.gitignore` — `node_modules/`
- `site/public/screenshots/*.png` — 10 redacted PNGs (committed)

**Modified files:**
- `README.md` — add "Showcase" link in the Documentation section

---

## Track A — Build the Site

### Task 1: Astro project skeleton

**Files:**
- Create: `site/package.json`
- Create: `site/astro.config.mjs`
- Create: `site/tailwind.config.mjs`
- Create: `site/tsconfig.json`
- Create: `site/.gitignore`
- Create: `site/src/layouts/Base.astro`
- Create: `site/src/styles/global.css`
- Create: `site/src/pages/index.astro` (minimal)
- Create: `site/public/favicon.svg`

- [ ] **Step 1: Initialize the Astro project deps**

```bash
mkdir -p site/src/{layouts,components,pages,styles,content}
mkdir -p site/public/screenshots
cd site
npm init -y
npm install astro@^4 @astrojs/tailwind@^5 tailwindcss@^3 astro-icon@^1 @iconify-json/lucide@^1
```

- [ ] **Step 2: Write `site/package.json` `scripts` block**

Replace the auto-generated `scripts` block with:

```json
{
  "scripts": {
    "dev": "astro dev",
    "build": "astro build",
    "preview": "astro preview",
    "astro": "astro"
  }
}
```

Set `"private": true` and `"type": "module"`.

- [ ] **Step 3: Write `site/astro.config.mjs`**

```js
import { defineConfig } from 'astro/config';
import tailwind from '@astrojs/tailwind';
import icon from 'astro-icon';

export default defineConfig({
  site: 'https://seanbarzilay.github.io',
  base: '/transcoderr',
  integrations: [tailwind(), icon()],
  build: { assets: '_astro' },
});
```

- [ ] **Step 4: Write `site/tailwind.config.mjs`**

```js
/** @type {import('tailwindcss').Config} */
export default {
  content: ['./src/**/*.{astro,html,md,js,ts,jsx,tsx}'],
  theme: {
    extend: {
      colors: {
        bg: '#0b0d10',
        surface: '#14171c',
        'surface-2': '#1c2027',
        accent: '#7dd3fc',
        muted: '#94a3b8',
        text: '#e5e7eb',
      },
      fontFamily: {
        mono: ['ui-monospace', 'SFMono-Regular', 'Menlo', 'Consolas', 'monospace'],
        sans: ['system-ui', '-apple-system', 'Segoe UI', 'Roboto', 'sans-serif'],
      },
      maxWidth: { prose: '72ch' },
    },
  },
};
```

- [ ] **Step 5: Write `site/tsconfig.json`**

```json
{
  "extends": "astro/tsconfigs/strict",
  "include": ["src/**/*"]
}
```

- [ ] **Step 6: Write `site/.gitignore`**

```
node_modules/
dist/
.astro/
public/screenshots/raw/
```

- [ ] **Step 7: Write `site/src/styles/global.css`**

```css
@tailwind base;
@tailwind components;
@tailwind utilities;

:root {
  color-scheme: dark;
}

html, body {
  background: theme('colors.bg');
  color: theme('colors.text');
  font-family: theme('fontFamily.sans');
}

body {
  background-image: radial-gradient(
    circle at 50% -20%,
    rgba(125, 211, 252, 0.06),
    transparent 60%
  );
  background-attachment: fixed;
}

::selection {
  background: theme('colors.accent');
  color: theme('colors.bg');
}
```

- [ ] **Step 8: Write `site/src/layouts/Base.astro`**

```astro
---
import '../styles/global.css';

interface Props {
  title?: string;
  description?: string;
}
const {
  title = 'transcoderr — push-driven media transcoder',
  description = 'Webhook in. ffmpeg out. One pass. Configurable in between.',
} = Astro.props;
---
<!doctype html>
<html lang="en">
  <head>
    <meta charset="utf-8" />
    <meta name="viewport" content="width=device-width, initial-scale=1" />
    <title>{title}</title>
    <meta name="description" content={description} />
    <meta property="og:title" content={title} />
    <meta property="og:description" content={description} />
    <meta property="og:type" content="website" />
    <meta property="og:image" content={`${Astro.site}${import.meta.env.BASE_URL}og-image.png`} />
    <link rel="icon" type="image/svg+xml" href={`${import.meta.env.BASE_URL}/favicon.svg`} />
  </head>
  <body class="min-h-screen antialiased">
    <slot />
  </body>
</html>
```

- [ ] **Step 9: Write `site/src/pages/index.astro` (minimal, expanded later)**

```astro
---
import Base from '../layouts/Base.astro';
---
<Base>
  <main class="mx-auto max-w-5xl px-6 py-24">
    <h1 class="font-mono text-5xl font-semibold tracking-tight">transcoderr</h1>
    <p class="mt-4 text-muted">Push-driven, single-binary media transcoder.</p>
  </main>
</Base>
```

- [ ] **Step 10: Write `site/public/favicon.svg`** (a single accent-colored "t")

```svg
<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 64 64">
  <rect width="64" height="64" rx="12" fill="#0b0d10"/>
  <text x="32" y="44" text-anchor="middle" font-family="ui-monospace, monospace"
        font-size="40" font-weight="600" fill="#7dd3fc">t</text>
</svg>
```

- [ ] **Step 11: Run dev server, verify**

```bash
cd site
npm run dev
```

Expected: Astro dev server on `http://localhost:4321/transcoderr/`. Visit; you see "transcoderr" wordmark in cyan-on-dark. Stop the server (Ctrl+C).

- [ ] **Step 12: Run a build smoke test**

```bash
cd site
npm run build
```

Expected: build succeeds, `dist/` produced. No type errors, no warnings about missing components.

- [ ] **Step 13: Commit**

```bash
cd ..
git add site/
git commit -m "site: Astro + Tailwind skeleton with dark palette"
```

---

### Task 2: Top-level scripts/ skeleton + screenshots dir contract

**Files:**
- Create: `scripts/package.json`
- Create: `scripts/.gitignore`
- Create: `scripts/shots.config.mjs`
- Create: `scripts/redact.config.mjs`
- Create: `scripts/capture.mjs` (stub)
- Create: `scripts/redact.mjs` (stub)
- Create: `site/public/screenshots/.gitkeep`

- [ ] **Step 1: Create scripts dir + npm init**

```bash
mkdir -p scripts
cd scripts
npm init -y
npm install playwright@^1.49 sharp@^0.33
npx playwright install chromium
```

- [ ] **Step 2: Set `scripts/package.json` to module + add scripts**

In `scripts/package.json`, set `"private": true`, `"type": "module"`, and:

```json
{
  "scripts": {
    "capture": "node capture.mjs",
    "redact": "node redact.mjs"
  }
}
```

- [ ] **Step 3: Write `scripts/.gitignore`**

```
node_modules/
```

- [ ] **Step 4: Write `scripts/shots.config.mjs`** (full target list)

```js
// Capture targets. Each entry is one PNG. URLs are relative to the
// server base. `viewport` is [width, height]. `wait` is a CSS selector
// to wait for before screenshotting; falsy = wait for networkidle only.
// `setup` names a hook in capture.mjs's `setupHooks` map; falsy = no hook.
export default [
  { name: 'runs-dashboard',    url: '/',                       viewport: [1440, 900], wait: 'main' },
  { name: 'live-run',          url: '/runs/CURRENT',           viewport: [1440, 900], setup: 'triggerTranscode' },
  { name: 'flows-editor',      url: '/flows/CURRENT/edit',     viewport: [1600, 1000], wait: 'textarea, .cm-editor' },
  { name: 'browse-manual',     url: '/browse/CURRENT',         viewport: [1440, 900], wait: 'main' },
  { name: 'plugins-browse',    url: '/plugins#browse',         viewport: [1440, 900], wait: 'main' },
  { name: 'plugins-installed', url: '/plugins#installed',      viewport: [1440, 900], wait: 'main' },
  { name: 'plugins-catalogs',  url: '/plugins#catalogs',       viewport: [1440, 900], wait: 'main' },
  { name: 'sources-list',      url: '/settings/sources',       viewport: [1440, 900], wait: 'main' },
  { name: 'notifiers-list',    url: '/settings/notifiers',     viewport: [1440, 900], wait: 'main' },
  { name: 'hardware-probe',    url: '/settings/hw',            viewport: [1440, 900], wait: 'main' },
];
```

- [ ] **Step 5: Write `scripts/redact.config.mjs`** (empty regions for now; populated in Task 15)

```js
// Per-image solid-fill rectangles. {x,y,w,h} in pixels of the raw PNG.
// Manually filled in Task 15 after eyeballing the captured raws.
export default {
  'runs-dashboard':    [],
  'live-run':          [],
  'flows-editor':      [],
  'browse-manual':     [],
  'plugins-browse':    [],
  'plugins-installed': [],
  'plugins-catalogs':  [],
  'sources-list':      [],
  'notifiers-list':    [],
  'hardware-probe':    [],
};
```

- [ ] **Step 6: Write `scripts/capture.mjs`** (stub — full implementation in Task 14)

```js
#!/usr/bin/env node
// Captures screenshots from a running transcoderr server into
// site/public/screenshots/raw/. See shots.config.mjs for the target list.
// Full implementation lands in Task 14.
console.error('capture.mjs: stub. Run after Task 14 implements it.');
process.exit(1);
```

- [ ] **Step 7: Write `scripts/redact.mjs`** (stub — full implementation in Task 15)

```js
#!/usr/bin/env node
// Reads site/public/screenshots/raw/*.png, applies solid black fills
// from redact.config.mjs, writes to site/public/screenshots/*.png.
// Full implementation lands in Task 15.
console.error('redact.mjs: stub. Run after Task 15 implements it.');
process.exit(1);
```

- [ ] **Step 8: Add `site/public/screenshots/.gitkeep`**

```bash
cd ..
touch site/public/screenshots/.gitkeep
```

- [ ] **Step 9: Commit**

```bash
git add scripts/ site/public/screenshots/.gitkeep
git commit -m "scripts: skeleton for screenshot capture + redaction"
```

Note: `scripts/node_modules/` and `scripts/package-lock.json` — commit `package-lock.json`, gitignore `node_modules/`.

---

### Task 3: Hero + FlowDiagram

**Files:**
- Create: `site/src/components/Hero.astro`
- Create: `site/src/components/FlowDiagram.astro`
- Modify: `site/src/pages/index.astro`

- [ ] **Step 1: Write `site/src/components/FlowDiagram.astro`**

The README's ASCII diagram, rendered as inline SVG. `currentColor` for non-accented strokes lets it inherit from the parent text color.

```astro
---
// FlowDiagram: Radarr/Sonarr -> transcoderr -> .mkv replaced.
---
<svg viewBox="0 0 800 240" xmlns="http://www.w3.org/2000/svg"
     role="img" aria-label="Radarr/Sonarr webhook to transcoderr to replaced .mkv"
     class="w-full h-auto text-muted">
  <defs>
    <marker id="arrow" viewBox="0 0 10 10" refX="9" refY="5"
            markerWidth="8" markerHeight="8" orient="auto">
      <path d="M 0 0 L 10 5 L 0 10 z" fill="currentColor"/>
    </marker>
  </defs>

  <rect x="20" y="80" width="180" height="80" rx="6"
        fill="none" stroke="currentColor" stroke-width="1.5"/>
  <text x="110" y="115" text-anchor="middle"
        font-family="ui-monospace,monospace" font-size="14" fill="currentColor">Radarr</text>
  <text x="110" y="135" text-anchor="middle"
        font-family="ui-monospace,monospace" font-size="14" fill="currentColor">Sonarr</text>

  <line x1="200" y1="120" x2="320" y2="120" stroke="currentColor"
        stroke-width="1.5" marker-end="url(#arrow)"/>
  <text x="260" y="105" text-anchor="middle"
        font-family="ui-monospace,monospace" font-size="11" fill="#7dd3fc">webhook</text>

  <rect x="320" y="80" width="200" height="80" rx="6"
        fill="none" stroke="#7dd3fc" stroke-width="1.5"/>
  <text x="420" y="115" text-anchor="middle"
        font-family="ui-monospace,monospace" font-size="14" fill="#7dd3fc">transcoderr</text>
  <text x="420" y="135" text-anchor="middle"
        font-family="ui-monospace,monospace" font-size="12" fill="#7dd3fc">flow engine</text>

  <line x1="520" y1="120" x2="640" y2="120" stroke="currentColor"
        stroke-width="1.5" marker-end="url(#arrow)"/>
  <text x="580" y="105" text-anchor="middle"
        font-family="ui-monospace,monospace" font-size="11" fill="#7dd3fc">one ffmpeg pass</text>

  <rect x="640" y="80" width="140" height="80" rx="6"
        fill="none" stroke="currentColor" stroke-width="1.5"/>
  <text x="710" y="115" text-anchor="middle"
        font-family="ui-monospace,monospace" font-size="14" fill="currentColor">.mkv</text>
  <text x="710" y="135" text-anchor="middle"
        font-family="ui-monospace,monospace" font-size="12" fill="currentColor">replaced</text>
</svg>
```

- [ ] **Step 2: Write `site/src/components/Hero.astro`**

```astro
---
import FlowDiagram from './FlowDiagram.astro';

const githubUrl   = 'https://github.com/seanbarzilay/transcoderr';
const releasesUrl = 'https://github.com/seanbarzilay/transcoderr/releases/latest';
const docsUrl     = 'https://github.com/seanbarzilay/transcoderr/tree/main/docs';
---
<section class="mx-auto max-w-5xl px-6 pt-24 pb-16">
  <h1 class="font-mono text-5xl sm:text-6xl font-semibold tracking-tight">
    transcoderr
  </h1>
  <p class="mt-6 text-2xl sm:text-3xl text-text/90 max-w-prose">
    Push-driven, single-binary media transcoder.
  </p>
  <p class="mt-3 text-lg text-muted max-w-prose">
    Webhook in. ffmpeg out. One pass. Configurable in between.
  </p>

  <div class="mt-10 flex flex-wrap gap-3">
    <a href={releasesUrl}
       class="inline-flex items-center rounded-md bg-accent text-bg px-4 py-2 font-medium hover:bg-accent/90">
      Download
    </a>
    <a href={githubUrl}
       class="inline-flex items-center rounded-md border border-muted/40 px-4 py-2 font-medium hover:border-muted">
      GitHub
    </a>
    <a href={docsUrl}
       class="inline-flex items-center rounded-md border border-muted/40 px-4 py-2 font-medium hover:border-muted">
      Docs
    </a>
  </div>

  <div class="mt-16">
    <FlowDiagram />
  </div>
</section>
```

- [ ] **Step 3: Wire Hero into `site/src/pages/index.astro`**

```astro
---
import Base from '../layouts/Base.astro';
import Hero from '../components/Hero.astro';
---
<Base>
  <main>
    <Hero />
  </main>
</Base>
```

- [ ] **Step 4: Verify in dev**

```bash
cd site && npm run dev
```

Visit `http://localhost:4321/transcoderr/`. Hero renders with wordmark, tagline, three buttons, and the SVG flow diagram below. Stop server.

- [ ] **Step 5: Build smoke test**

```bash
cd site && npm run build
```

Build succeeds.

- [ ] **Step 6: Commit**

```bash
cd ..
git add site/src/
git commit -m "site: hero + svg flow diagram"
```

---

### Task 4: FeatureGrid + 6 FeatureCards

**Files:**
- Create: `site/src/components/FeatureCard.astro`
- Create: `site/src/components/FeatureGrid.astro`
- Modify: `site/src/pages/index.astro`

- [ ] **Step 1: Write `site/src/components/FeatureCard.astro`**

```astro
---
import { Icon } from 'astro-icon/components';

interface Props {
  icon: string;       // e.g. "lucide:webhook"
  title: string;
  body: string;
}
const { icon, title, body } = Astro.props;
---
<div class="rounded-lg border border-muted/15 bg-surface p-6 hover:border-accent/40 transition-colors">
  <Icon name={icon} class="h-6 w-6 text-accent" />
  <h3 class="mt-4 font-mono text-lg font-semibold">{title}</h3>
  <p class="mt-2 text-muted leading-relaxed">{body}</p>
</div>
```

- [ ] **Step 2: Write `site/src/components/FeatureGrid.astro`** (full copy verbatim)

```astro
---
import FeatureCard from './FeatureCard.astro';

const features = [
  {
    icon: 'lucide:webhook',
    title: 'Push-driven',
    body: 'Typed webhook adapters for Radarr, Sonarr, and Lidarr plus a generic /webhook/:name. No library scanning, no polling.',
  },
  {
    icon: 'lucide:list-checks',
    title: 'Plan-then-execute',
    body: 'Compose declarative plan.* steps. One plan.execute materializes the whole flow into a single ffmpeg call. No chained tmp files.',
  },
  {
    icon: 'lucide:cpu',
    title: 'Hardware-aware',
    body: 'Boot-time probe of NVENC, QSV, VAAPI, and VideoToolbox. Per-device concurrency limits, runtime CPU fallback when a GPU encode fails mid-job.',
  },
  {
    icon: 'lucide:activity',
    title: 'Live observability',
    body: 'Per-run progress bar, ffmpeg status streamed live, structured timeline of every step decision, Prometheus-compatible /metrics.',
  },
  {
    icon: 'lucide:puzzle',
    title: 'Plugins',
    body: 'Browse a catalog, click Install. Tarballs are sha256-verified, deps run, and the step registry live-reloads — no restart.',
  },
  {
    icon: 'lucide:package',
    title: 'Single binary',
    body: 'Rust + embedded SQLite + embedded React SPA. One image, one volume mount, no broker, no external DB.',
  },
];
---
<section class="mx-auto max-w-5xl px-6 py-24">
  <h2 class="font-mono text-3xl font-semibold">What it does</h2>
  <div class="mt-10 grid grid-cols-1 sm:grid-cols-2 lg:grid-cols-3 gap-4">
    {features.map((f) => <FeatureCard {...f} />)}
  </div>
</section>
```

- [ ] **Step 3: Add FeatureGrid to index**

In `site/src/pages/index.astro`, import and place after `<Hero />`:

```astro
---
import Base from '../layouts/Base.astro';
import Hero from '../components/Hero.astro';
import FeatureGrid from '../components/FeatureGrid.astro';
---
<Base>
  <main>
    <Hero />
    <FeatureGrid />
  </main>
</Base>
```

- [ ] **Step 4: Verify dev + build**

```bash
cd site && npm run dev
```

6 cards, 3-column on lg, 2-column on sm, 1-column on mobile. Each card has a cyan icon. Hover lifts border to cyan-ish. Stop server, run `npm run build`.

- [ ] **Step 5: Commit**

```bash
cd ..
git add site/src/
git commit -m "site: feature grid (6 cards) with copy + lucide icons"
```

---

### Task 5: ScreenshotShowcase (CSS-only tabs)

**Files:**
- Create: `site/src/components/ScreenshotShowcase.astro`
- Modify: `site/src/pages/index.astro`

CSS-only tabs via the radio-button trick — no JS. Each radio's `:checked` state activates the matching panel. Accessible via keyboard (radio focus).

- [ ] **Step 1: Write `site/src/components/ScreenshotShowcase.astro`**

```astro
---
const BASE = import.meta.env.BASE_URL;

const shots = [
  {
    id: 'runs',
    label: 'Runs',
    file: 'runs-dashboard.png',
    caption: 'Every job, with status, source, file, and timing.',
  },
  {
    id: 'live',
    label: 'Live run',
    file: 'live-run.png',
    caption: 'Per-run progress bar plus the ffmpeg status line, streamed live.',
  },
  {
    id: 'flows',
    label: 'Flows editor',
    file: 'flows-editor.png',
    caption: 'YAML on the left, visual mirror on the right. Edits round-trip.',
  },
  {
    id: 'browse',
    label: 'Browse',
    file: 'browse-manual.png',
    caption: 'Search any source\u2019s library. Click a file, queue it against every matching flow.',
  },
  {
    id: 'plugins',
    label: 'Plugins',
    file: 'plugins-browse.png',
    caption: 'Default catalog plus any private ones you add. Click Install, sha256-verified.',
  },
];
---
<section class="mx-auto max-w-6xl px-6 py-24">
  <h2 class="font-mono text-3xl font-semibold">See it work</h2>

  <div class="mt-10 showcase">
    {shots.map((s, i) => (
      <input
        type="radio"
        name="showcase"
        id={`shot-${s.id}`}
        class="sr-only"
        checked={i === 0}
      />
    ))}

    <div class="flex flex-wrap gap-2">
      {shots.map((s) => (
        <label
          for={`shot-${s.id}`}
          class="cursor-pointer rounded-md border border-muted/30 px-3 py-1.5 font-mono text-sm hover:border-muted"
          data-shot={s.id}
        >
          {s.label}
        </label>
      ))}
    </div>

    <div class="mt-6">
      {shots.map((s) => (
        <figure class="shot-panel" data-shot={s.id}>
          <img
            src={`${BASE}/screenshots/${s.file}`}
            alt={s.caption}
            class="w-full rounded-lg border border-muted/15"
            loading="lazy"
            decoding="async"
          />
          <figcaption class="mt-3 text-muted">{s.caption}</figcaption>
        </figure>
      ))}
    </div>
  </div>
</section>

<style>
  /* Default: hide all panels and reset label borders */
  .showcase .shot-panel { display: none; }

  /* Wire each radio's :checked state to its matching panel + label */
  .showcase input[id="shot-runs"]:checked    ~ * .shot-panel[data-shot="runs"],
  .showcase input[id="shot-live"]:checked    ~ * .shot-panel[data-shot="live"],
  .showcase input[id="shot-flows"]:checked   ~ * .shot-panel[data-shot="flows"],
  .showcase input[id="shot-browse"]:checked  ~ * .shot-panel[data-shot="browse"],
  .showcase input[id="shot-plugins"]:checked ~ * .shot-panel[data-shot="plugins"] {
    display: block;
  }

  .showcase input[id="shot-runs"]:checked    ~ * label[data-shot="runs"],
  .showcase input[id="shot-live"]:checked    ~ * label[data-shot="live"],
  .showcase input[id="shot-flows"]:checked   ~ * label[data-shot="flows"],
  .showcase input[id="shot-browse"]:checked  ~ * label[data-shot="browse"],
  .showcase input[id="shot-plugins"]:checked ~ * label[data-shot="plugins"] {
    border-color: theme('colors.accent');
    color: theme('colors.accent');
  }

  /* Keyboard focus visibility */
  .showcase input:focus-visible + * label[data-shot] {
    outline: 2px solid theme('colors.accent');
    outline-offset: 2px;
  }
</style>
```

- [ ] **Step 2: Drop placeholder PNGs**

For development before real captures land, generate 1440×900 placeholder PNGs into `site/public/screenshots/` so the layout doesn't shift after redaction. Add a tiny `scripts/placeholders.mjs`:

```js
#!/usr/bin/env node
// One-shot placeholder generator. Replaced by capture+redact in Track B.
import sharp from 'sharp';
import { resolve, dirname } from 'node:path';
import { fileURLToPath } from 'node:url';

const __dirname = dirname(fileURLToPath(import.meta.url));
const OUT = resolve(__dirname, '..', 'site/public/screenshots');

const names = [
  'runs-dashboard', 'live-run', 'flows-editor', 'browse-manual',
  'plugins-browse', 'plugins-installed', 'plugins-catalogs',
  'sources-list', 'notifiers-list', 'hardware-probe',
];

for (const n of names) {
  await sharp({ create: { width: 1440, height: 900, channels: 3, background: '#14171c' } })
    .png()
    .toFile(resolve(OUT, `${n}.png`));
  console.log('placeholder:', n);
}
```

Then run it once:

```bash
cd scripts && node placeholders.mjs
```

This script is committed but only runs on demand.

- [ ] **Step 3: Add ScreenshotShowcase to index**

```astro
---
import Base from '../layouts/Base.astro';
import Hero from '../components/Hero.astro';
import FeatureGrid from '../components/FeatureGrid.astro';
import ScreenshotShowcase from '../components/ScreenshotShowcase.astro';
---
<Base>
  <main>
    <Hero />
    <FeatureGrid />
    <ScreenshotShowcase />
  </main>
</Base>
```

- [ ] **Step 4: Verify dev + build**

`npm run dev`. Click each tab; only the matching panel is visible, label highlights cyan. Tab key cycles through radios. Stop server. `npm run build`.

- [ ] **Step 5: Commit**

```bash
cd ../..
git add site/
git commit -m "site: screenshot showcase (CSS-only tabs, 5 panels)"
```

---

### Task 6: CodeSplit (YAML | ffmpeg side-by-side)

**Files:**
- Create: `site/src/content/flow-example.yaml`
- Create: `site/src/content/ffmpeg-invocation.txt`
- Create: `site/src/components/CodeSplit.astro`
- Modify: `site/src/pages/index.astro`

- [ ] **Step 1: Write `site/src/content/flow-example.yaml`**

```yaml
name: hevc-normalize
triggers:
  - radarr: [downloaded, upgraded]
  - sonarr: [downloaded]

steps:
  - use: probe
  - use: plan.init
  - use: plan.streams.drop_cover_art

  - if: probe.streams[0].codec_name == "hevc"
    then: []
    else:
      - use: plan.video.encode
        with:
          codec: x265
          crf: 19
          preset: fast
          hw: { prefer: [nvenc, qsv, vaapi, videotoolbox], fallback: cpu }

  - use: plan.audio.ensure
    with: { codec: ac3, channels: 6, language: eng }

  - use: plan.execute        # ONE ffmpeg pass
  - use: verify.playable
  - use: output
    with: { mode: replace }
```

- [ ] **Step 2: Write `site/src/content/ffmpeg-invocation.txt`**

```
ffmpeg -hwaccel cuda -i input.mkv \
  -map 0:v:0 -c:v hevc_nvenc -preset p5 -cq 19 -pix_fmt p010le \
  -map 0:a:0 -c:a ac3 -b:a 640k -ac 6 -metadata:s:a:0 language=eng \
  -map 0:s? -c:s copy \
  -map -0:t? -map -0:V? \
  -movflags +faststart -y output.mkv
```

- [ ] **Step 3: Write `site/src/components/CodeSplit.astro`**

Astro has built-in shiki via `<Code/>` from `astro:components`. Use it for syntax highlighting (the `github-dark` theme matches our palette).

```astro
---
import { Code } from 'astro:components';
import flowYaml from '../content/flow-example.yaml?raw';
import ffmpegCmd from '../content/ffmpeg-invocation.txt?raw';
---
<section class="mx-auto max-w-6xl px-6 py-24">
  <h2 class="font-mono text-3xl font-semibold">One ffmpeg pass</h2>
  <p class="mt-4 max-w-prose text-muted">
    No chained tmp files, no per-step IO churn. The flow engine builds a stream
    plan in memory; one <code class="font-mono text-accent">plan.execute</code> step
    materializes it into a single ffmpeg call.
  </p>

  <div class="mt-10 grid grid-cols-1 lg:grid-cols-2 gap-4">
    <div class="rounded-lg border border-muted/15 bg-surface overflow-hidden">
      <div class="px-4 py-2 border-b border-muted/15 font-mono text-xs text-muted">
        flow.yaml
      </div>
      <Code code={flowYaml} lang="yaml" theme="github-dark" />
    </div>
    <div class="rounded-lg border border-muted/15 bg-surface overflow-hidden">
      <div class="px-4 py-2 border-b border-muted/15 font-mono text-xs text-muted">
        plan.execute &mdash; the ffmpeg call
      </div>
      <Code code={ffmpegCmd} lang="bash" theme="github-dark" />
    </div>
  </div>
</section>
```

- [ ] **Step 4: Add to index**

```astro
---
import Base from '../layouts/Base.astro';
import Hero from '../components/Hero.astro';
import FeatureGrid from '../components/FeatureGrid.astro';
import ScreenshotShowcase from '../components/ScreenshotShowcase.astro';
import CodeSplit from '../components/CodeSplit.astro';
---
<Base>
  <main>
    <Hero />
    <FeatureGrid />
    <ScreenshotShowcase />
    <CodeSplit />
  </main>
</Base>
```

- [ ] **Step 5: Verify dev + build**

YAML on left, ffmpeg on right. Both syntax-highlighted. Stack on mobile. `npm run build` passes.

- [ ] **Step 6: Commit**

```bash
git add site/src/
git commit -m "site: code-split (flow yaml + resulting ffmpeg invocation)"
```

---

### Task 7: PluginShowcase

**Files:**
- Create: `site/src/components/PluginShowcase.astro`
- Modify: `site/src/pages/index.astro`

- [ ] **Step 1: Write `site/src/components/PluginShowcase.astro`**

```astro
---
const BASE = import.meta.env.BASE_URL;
const catalogUrl = 'https://github.com/seanbarzilay/transcoderr-plugins';
---
<section class="mx-auto max-w-6xl px-6 py-24">
  <h2 class="font-mono text-3xl font-semibold">Plugins</h2>
  <p class="mt-4 max-w-prose text-muted">
    Extend flows with new step kinds. Browse the
    <a href={catalogUrl} class="text-accent hover:underline">default catalog</a>
    or add your own. Install pulls the tarball, sha256-verifies it, runs any
    deps the plugin declared, and live-reloads the step registry &mdash; no
    restart. Plugins are any executable that speaks JSON-RPC over stdin/stdout:
    POSIX shell, Python, Go, anything.
  </p>

  <div class="mt-10 grid grid-cols-1 lg:grid-cols-2 gap-4">
    <figure>
      <img
        src={`${BASE}/screenshots/plugins-browse.png`}
        alt="Plugins -> Browse tab showing the catalog"
        class="w-full rounded-lg border border-muted/15"
        loading="lazy"
        decoding="async"
      />
      <figcaption class="mt-3 text-sm text-muted">Browse a catalog, click Install.</figcaption>
    </figure>
    <figure>
      <img
        src={`${BASE}/screenshots/plugins-installed.png`}
        alt="Plugins -> Installed tab"
        class="w-full rounded-lg border border-muted/15"
        loading="lazy"
        decoding="async"
      />
      <figcaption class="mt-3 text-sm text-muted">Installed plugins live-reload without a restart.</figcaption>
    </figure>
  </div>
</section>
```

- [ ] **Step 2: Add to index after `<CodeSplit />`**

```astro
import PluginShowcase from '../components/PluginShowcase.astro';
// ...
<CodeSplit />
<PluginShowcase />
```

- [ ] **Step 3: Verify dev + build**

- [ ] **Step 4: Commit**

```bash
git add site/src/
git commit -m "site: plugin showcase section"
```

---

### Task 8: McpSection

**Files:**
- Create: `site/src/components/McpSection.astro`
- Modify: `site/src/pages/index.astro`

- [ ] **Step 1: Write `site/src/components/McpSection.astro`**

Full copy + config snippet inline so the implementer doesn't paraphrase.

```astro
---
import { Code } from 'astro:components';

const tools = [
  ['Runs',    'list, get, cancel, rerun'],
  ['Flows',   'list, get, create, update, delete, dry-run'],
  ['Sources', 'list, get, create, update, delete'],
  ['Notifiers', 'list, get, create, update, delete, test'],
  ['Plugins',   'list, get, browse, install, uninstall (catalog management stays operator-owned)'],
  ['Library',   'list movies / series / episodes, transcode a specific file'],
  ['Server',    'health, queue depth, hardware capabilities, metrics, time-series'],
];

const config = `{
  "mcpServers": {
    "transcoderr": {
      "command": "/usr/local/bin/transcoderr-mcp",
      "env": {
        "TRANSCODERR_URL": "http://192.168.1.176:8099",
        "TRANSCODERR_TOKEN": "tcr_xxxxxxxxxxxxxxxx"
      }
    }
  }
}`;
---
<section class="mx-auto max-w-6xl px-6 py-24">
  <h2 class="font-mono text-3xl font-semibold">Drive it from your AI client</h2>
  <p class="mt-4 max-w-prose text-muted">
    transcoderr-mcp lets AI clients talk to transcoderr over stdio. Point Claude
    Desktop, Cursor, or any MCP client at the binary, then ask in plain English:
    <em>queue every non-HEVC movie. Re-encode every 1080p episode of this show.
    Show me the failed runs from this week.</em>
  </p>

  <div class="mt-10 grid grid-cols-1 lg:grid-cols-5 gap-6">
    <div class="lg:col-span-3 rounded-lg border border-muted/15 bg-surface overflow-hidden">
      <div class="px-4 py-2 border-b border-muted/15 font-mono text-xs text-muted">
        claude_desktop_config.json
      </div>
      <Code code={config} lang="json" theme="github-dark" />
    </div>
    <div class="lg:col-span-2">
      <h3 class="font-mono text-sm uppercase tracking-wide text-muted">Tools</h3>
      <ul class="mt-3 space-y-2">
        {tools.map(([k, v]) => (
          <li>
            <span class="font-mono text-accent">{k}</span>
            <span class="text-muted"> &mdash; {v}</span>
          </li>
        ))}
      </ul>
    </div>
  </div>
</section>
```

- [ ] **Step 2: Add to index**

```astro
import McpSection from '../components/McpSection.astro';
// ...
<PluginShowcase />
<McpSection />
```

- [ ] **Step 3: Verify dev + build**

- [ ] **Step 4: Commit**

```bash
git add site/src/
git commit -m "site: MCP section (pitch + config snippet + tool list)"
```

---

### Task 9: DeployBlock (compose + Copy button — only client-side JS)

**Files:**
- Create: `site/src/content/compose.yaml`
- Create: `site/src/components/DeployBlock.astro`
- Modify: `site/src/pages/index.astro`

- [ ] **Step 1: Write `site/src/content/compose.yaml`**

```yaml
# docker-compose.yml
services:
  transcoderr:
    image: ghcr.io/seanbarzilay/transcoderr:cpu-latest
    restart: unless-stopped
    ports: ["8099:8080"]
    volumes:
      - ./data:/data
      # IMPORTANT: mount your media at the SAME path it has in Radarr/Sonarr.
      - /mnt/movies:/mnt/movies
```

- [ ] **Step 2: Write `site/src/components/DeployBlock.astro`**

Inline `<script>` is fine in Astro — it ships only this 12-line script and only on this section. No bundler config needed.

```astro
---
import { Code } from 'astro:components';
import compose from '../content/compose.yaml?raw';
---
<section class="mx-auto max-w-5xl px-6 py-24">
  <h2 class="font-mono text-3xl font-semibold">Deploy in 30 seconds</h2>

  <div class="mt-8 relative rounded-lg border border-muted/15 bg-surface overflow-hidden">
    <div class="flex items-center justify-between px-4 py-2 border-b border-muted/15">
      <span class="font-mono text-xs text-muted">docker-compose.yml</span>
      <button
        type="button"
        data-copy
        data-copy-target="deploy-compose"
        class="font-mono text-xs text-muted hover:text-accent transition-colors"
      >
        Copy
      </button>
    </div>
    <div id="deploy-compose"><Code code={compose} lang="yaml" theme="github-dark" /></div>
  </div>

  <p class="mt-6 text-muted">
    Then <code class="font-mono text-accent">docker compose up -d</code> and
    open <code class="font-mono">http://localhost:8099</code>. The web UI walks
    you through sources, notifiers, plugins, and your first flow.
  </p>
</section>

<script is:inline>
  // The only client-side JS on the page: a Copy button.
  document.querySelectorAll('button[data-copy]').forEach((btn) => {
    btn.addEventListener('click', async () => {
      const id = btn.getAttribute('data-copy-target');
      const node = document.getElementById(id);
      if (!node) return;
      const text = node.innerText.trim();
      try {
        await navigator.clipboard.writeText(text);
        const original = btn.textContent;
        btn.textContent = 'Copied';
        setTimeout(() => { btn.textContent = original; }, 1500);
      } catch (e) {
        btn.textContent = 'Copy failed';
      }
    });
  });
</script>
```

- [ ] **Step 3: Add to index**

```astro
import DeployBlock from '../components/DeployBlock.astro';
// ...
<McpSection />
<DeployBlock />
```

- [ ] **Step 4: Verify dev + build**

Click Copy → button text changes to "Copied" for 1.5s, clipboard contains the compose YAML. `npm run build` passes; `npm run preview` works the same.

- [ ] **Step 5: Commit**

```bash
git add site/src/
git commit -m "site: deploy block (compose snippet + copy button)"
```

---

### Task 10: FlavorTable

**Files:**
- Create: `site/src/components/FlavorTable.astro`
- Modify: `site/src/pages/index.astro`

- [ ] **Step 1: Write `site/src/components/FlavorTable.astro`**

```astro
---
const rows = [
  { tag: ':cpu-latest',    base: 'debian:bookworm-slim + ffmpeg',     accel: 'software only' },
  { tag: ':intel-latest',  base: 'bookworm + intel-media-va-driver',  accel: 'QSV / VAAPI' },
  { tag: ':nvidia-latest', base: 'jrottenberg/ffmpeg-nvidia',         accel: 'NVENC / NVDEC' },
  { tag: ':full-latest',   base: 'NVIDIA base + Intel runtime',       accel: 'NVENC + QSV / VAAPI' },
];
---
<section class="mx-auto max-w-5xl px-6 py-24">
  <h2 class="font-mono text-3xl font-semibold">Image flavors</h2>

  <div class="mt-8 overflow-x-auto rounded-lg border border-muted/15">
    <table class="min-w-full text-sm">
      <thead class="bg-surface">
        <tr class="text-left">
          <th class="px-4 py-3 font-mono text-muted">tag</th>
          <th class="px-4 py-3 font-mono text-muted">base</th>
          <th class="px-4 py-3 font-mono text-muted">hardware accel</th>
        </tr>
      </thead>
      <tbody class="divide-y divide-muted/15">
        {rows.map((r) => (
          <tr class="hover:bg-surface/60">
            <td class="px-4 py-3 font-mono text-accent">{r.tag}</td>
            <td class="px-4 py-3 text-muted">{r.base}</td>
            <td class="px-4 py-3">{r.accel}</td>
          </tr>
        ))}
      </tbody>
    </table>
  </div>

  <p class="mt-6 text-sm text-muted">
    Each tag also exists pinned to a version (<code class="font-mono">:cpu-v0.27.0</code>, etc.).
    Static binaries (<code class="font-mono">linux-amd64</code>, <code class="font-mono">linux-arm64</code>,
    <code class="font-mono">darwin-arm64</code>) ship attached to each
    <a href="https://github.com/seanbarzilay/transcoderr/releases" class="text-accent hover:underline">GitHub Release</a>.
  </p>
</section>
```

- [ ] **Step 2: Add to index**

```astro
import FlavorTable from '../components/FlavorTable.astro';
// ...
<DeployBlock />
<FlavorTable />
```

- [ ] **Step 3: Verify dev + build**

- [ ] **Step 4: Commit**

```bash
git add site/src/
git commit -m "site: image flavors table"
```

---

### Task 11: Footer

**Files:**
- Create: `site/src/components/Footer.astro`
- Modify: `site/src/pages/index.astro`

- [ ] **Step 1: Write `site/src/components/Footer.astro`**

```astro
---
const BASE = import.meta.env.BASE_URL;
const repo = 'https://github.com/seanbarzilay/transcoderr';
---
<footer class="mt-24 border-t border-muted/15">
  <div class="mx-auto max-w-5xl px-6 py-12 grid grid-cols-1 sm:grid-cols-4 gap-8">
    <div class="sm:col-span-2">
      <h3 class="font-mono text-lg font-semibold">transcoderr</h3>
      <p class="mt-2 text-sm text-muted max-w-prose">
        Push-driven, single-binary media transcoder. Webhook in. ffmpeg out.
      </p>
    </div>

    <div>
      <h4 class="font-mono text-xs uppercase tracking-wide text-muted">Project</h4>
      <ul class="mt-2 space-y-1 text-sm">
        <li><a href={repo} class="hover:text-accent">GitHub</a></li>
        <li><a href={`${repo}/releases`} class="hover:text-accent">Releases</a></li>
        <li><a href={`${repo}/tree/main/docs`} class="hover:text-accent">Docs</a></li>
      </ul>
    </div>

    <div>
      <h4 class="font-mono text-xs uppercase tracking-wide text-muted">Hardware</h4>
      <a href={`${BASE}/screenshots/hardware-probe.png`}>
        <img
          src={`${BASE}/screenshots/hardware-probe.png`}
          alt="Hardware probe page"
          class="mt-2 w-full rounded border border-muted/15 hover:border-accent/50"
          loading="lazy"
          decoding="async"
        />
      </a>
    </div>
  </div>

  <div class="border-t border-muted/15">
    <div class="mx-auto max-w-5xl px-6 py-6 flex flex-wrap justify-between text-xs text-muted">
      <span>License: TBD by the project owner.</span>
      <span>seanbarzilay.github.io / transcoderr</span>
    </div>
  </div>
</footer>
```

- [ ] **Step 2: Add to index**

```astro
import Footer from '../components/Footer.astro';
// ...
<FlavorTable />
  </main>
  <Footer />
```

- [ ] **Step 3: Verify dev + build**

- [ ] **Step 4: Commit**

```bash
git add site/src/
git commit -m "site: footer"
```

---

### Task 12: Push-driven and MCP supporting screenshots (sources-list, notifiers-list)

The spec embeds `sources-list` near the Push-driven feature card and `notifiers-list` near the MCP section as small supporting visuals. Wire them now so they're in place when the screenshots ship.

**Files:**
- Modify: `site/src/components/FeatureGrid.astro`
- Modify: `site/src/components/McpSection.astro`

- [ ] **Step 1: Add a small thumbnail under the Push-driven card**

Wrap `FeatureGrid.astro`'s push-driven card with a small image link beneath the grid (or, simpler, append a single full-width thumbnail under the grid):

In `site/src/components/FeatureGrid.astro`, after the `</div>` that closes the grid, before the section closes:

```astro
  <a
    href={`${import.meta.env.BASE_URL}/screenshots/sources-list.png`}
    class="mt-8 block group"
  >
    <img
      src={`${import.meta.env.BASE_URL}/screenshots/sources-list.png`}
      alt="Sources list — typed adapters for Radarr, Sonarr, Lidarr, plus generic webhooks"
      class="w-full rounded-lg border border-muted/15 group-hover:border-accent/50 transition-colors"
      loading="lazy"
      decoding="async"
    />
    <div class="mt-2 text-xs text-muted">Sources &mdash; typed adapters per *arr, plus generic webhooks.</div>
  </a>
```

- [ ] **Step 2: Add a notifiers thumbnail row to McpSection**

In `site/src/components/McpSection.astro`, after the existing two-column grid `</div>`, before the `</section>`:

```astro
  <a
    href={`${import.meta.env.BASE_URL}/screenshots/notifiers-list.png`}
    class="mt-10 block group"
  >
    <img
      src={`${import.meta.env.BASE_URL}/screenshots/notifiers-list.png`}
      alt="Notifiers list — Discord, ntfy, Telegram, generic webhook, Jellyfin"
      class="w-full rounded-lg border border-muted/15 group-hover:border-accent/50 transition-colors"
      loading="lazy"
      decoding="async"
    />
    <div class="mt-2 text-xs text-muted">Notifiers &mdash; Discord, ntfy, Telegram, generic webhook, Jellyfin.</div>
  </a>
```

- [ ] **Step 3: Verify dev + build**

- [ ] **Step 4: Commit**

```bash
git add site/src/
git commit -m "site: supporting thumbnails (sources, notifiers)"
```

---

### Task 13: GitHub Actions deploy workflow

**Files:**
- Create: `.github/workflows/pages.yml`

- [ ] **Step 1: Write `.github/workflows/pages.yml`**

```yaml
name: pages

on:
  push:
    branches: [main]
    paths:
      - 'site/**'
      - '.github/workflows/pages.yml'
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

      - name: Setup Node 20
        uses: actions/setup-node@v4
        with:
          node-version: 20
          cache: npm
          cache-dependency-path: site/package-lock.json

      - name: Install deps
        working-directory: site
        run: npm ci

      - name: Install oxipng
        run: sudo apt-get update && sudo apt-get install -y --no-install-recommends oxipng

      - name: Build site
        working-directory: site
        run: npm run build

      - name: Compress PNGs (lossless)
        working-directory: site
        run: |
          find dist -name '*.png' -exec oxipng -o 4 --strip safe {} +

      - uses: actions/configure-pages@v5

      - uses: actions/upload-pages-artifact@v3
        with:
          path: site/dist

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

- [ ] **Step 2: Manual prerequisite (one-time, after first PR merges)**

Document this in the PR body: after merging, go to **Settings → Pages → Source = "GitHub Actions"**. Until that's set, the deploy job will fail with "GitHub Pages is not enabled for this repository."

- [ ] **Step 3: Build smoke test (locally simulating the workflow's build steps)**

```bash
cd site
npm ci
npm run build
ls dist/index.html
ls dist/_astro/
```

Expected: `dist/index.html` exists, `_astro/` contains hashed assets.

- [ ] **Step 4: Commit**

```bash
cd ..
git add .github/workflows/pages.yml
git commit -m "ci: GitHub Pages deploy workflow"
```

---

### Task 14: README link to the live site

**Files:**
- Modify: `README.md` (Documentation section, near the catalog repo line)

- [ ] **Step 1: Add a Showcase link**

Find the existing `## Documentation` section (currently containing `docs/deploy.md`, `docs/mcp.md`, etc.). Add as the first item:

```markdown
- [`seanbarzilay.github.io/transcoderr`](https://seanbarzilay.github.io/transcoderr/) —
  the showcase site (one-page tour with live UI screenshots)
```

- [ ] **Step 2: Commit**

```bash
git add README.md
git commit -m "docs: link the showcase site from README"
```

---

## Track B — Capture & Redact Screenshots

Track B starts after Track A's components, scripts skeleton, and workflow are in place. Track B fills in the placeholder PNGs with real, redacted captures from the live server.

### Task 15: Screenshot capture script

**Files:**
- Modify: `scripts/capture.mjs` (replace stub with full impl)

The script logs in once, runs setup hooks where declared, navigates to each target, waits, screenshots, and writes to `site/public/screenshots/raw/`.

- [ ] **Step 1: Write `scripts/capture.mjs`**

```js
#!/usr/bin/env node
/**
 * Capture screenshots from a running transcoderr server.
 *
 * Usage:
 *   TRANSCODERR_URL=http://192.168.1.176:8099 \
 *   TRANSCODERR_PASSWORD=xxx \
 *   TRANSCODERR_LIVE_TRANSCODE_PATH=/mnt/movies/Foo/Foo.mkv \
 *   node scripts/capture.mjs
 *
 * Outputs site/public/screenshots/raw/<name>.png (gitignored).
 */
import { chromium } from 'playwright';
import { mkdir } from 'node:fs/promises';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import shots from './shots.config.mjs';

const __dirname = dirname(fileURLToPath(import.meta.url));
const REPO_ROOT = resolve(__dirname, '..');
const RAW_DIR   = resolve(REPO_ROOT, 'site/public/screenshots/raw');

const BASE_URL = process.env.TRANSCODERR_URL || 'http://192.168.1.176:8099';
const PASSWORD = process.env.TRANSCODERR_PASSWORD;
const TRANSCODE_PATH = process.env.TRANSCODERR_LIVE_TRANSCODE_PATH;

if (!PASSWORD) {
  console.error('TRANSCODERR_PASSWORD is required');
  process.exit(1);
}

await mkdir(RAW_DIR, { recursive: true });

const setupHooks = {
  /**
   * Trigger a manual transcode of TRANSCODE_PATH against the first enabled
   * flow that matches its source kind, then return a URL to the new run.
   */
  triggerTranscode: async (page, _api) => {
    if (!TRANSCODE_PATH) {
      throw new Error('TRANSCODERR_LIVE_TRANSCODE_PATH is required for live-run capture');
    }
    // Use the same authenticated session via cookies for the API call.
    const ctx = page.context();
    const res = await ctx.request.post(`${BASE_URL}/api/transcode/file`, {
      data: { path: TRANSCODE_PATH },
      failOnStatusCode: true,
    });
    const body = await res.json();
    const runId = body.run_id || body.id || body.runId;
    if (!runId) throw new Error(`No run id in response: ${JSON.stringify(body)}`);
    // Replace CURRENT in the configured URL.
    return `/runs/${runId}`;
  },
};

const browser = await chromium.launch();
const context = await browser.newContext();
const page    = await context.newPage();

// Login (works whether auth is on or off — the login page just no-ops if off).
await page.goto(`${BASE_URL}/login`, { waitUntil: 'networkidle' });
const passwordInput = page.locator('input[type="password"]');
if (await passwordInput.isVisible({ timeout: 1500 }).catch(() => false)) {
  await passwordInput.fill(PASSWORD);
  await page.locator('button[type="submit"]').click();
  await page.waitForLoadState('networkidle');
}

for (const shot of shots) {
  console.log(`[capture] ${shot.name}`);
  await page.setViewportSize({ width: shot.viewport[0], height: shot.viewport[1] });

  let url = shot.url;
  if (shot.setup) {
    const hook = setupHooks[shot.setup];
    if (!hook) throw new Error(`Unknown setup hook: ${shot.setup}`);
    url = await hook(page);
  }

  await page.goto(`${BASE_URL}${url}`, { waitUntil: 'networkidle' });

  if (shot.wait) {
    await page.waitForSelector(shot.wait, { timeout: 10_000 });
  }

  // Live run: give the progress bar and ffmpeg status line time to render.
  if (shot.setup === 'triggerTranscode') {
    await page.waitForTimeout(5_000);
  }

  await page.screenshot({
    path: resolve(RAW_DIR, `${shot.name}.png`),
    fullPage: false,
  });
}

await browser.close();
console.log(`[capture] wrote ${shots.length} screenshots to ${RAW_DIR}`);
```

- [ ] **Step 2: Run the capture against the live server**

```bash
cd scripts
TRANSCODERR_URL=http://192.168.1.176:8099 \
TRANSCODERR_PASSWORD=<your-ui-password> \
TRANSCODERR_LIVE_TRANSCODE_PATH=<a-real-file-path> \
node capture.mjs
```

Expected: 10 PNGs in `site/public/screenshots/raw/`. The `live-run` shot should show a job in progress.

If a particular shot fails (URL 404, selector timeout), iterate on `shots.config.mjs` until each one captures cleanly. Do **not** commit the raws — they're gitignored.

- [ ] **Step 3: Commit the script**

```bash
cd ..
git add scripts/capture.mjs
git commit -m "scripts: implement screenshot capture (playwright)"
```

---

### Task 16: Screenshot redaction script + per-image config

**Files:**
- Modify: `scripts/redact.mjs` (replace stub with full impl)
- Modify: `scripts/redact.config.mjs` (fill in regions per the captured raws)

- [ ] **Step 1: Write `scripts/redact.mjs`**

```js
#!/usr/bin/env node
/**
 * Read site/public/screenshots/raw/*.png, overlay solid black rectangles per
 * scripts/redact.config.mjs, write to site/public/screenshots/<name>.png.
 *
 * Solid fills (NOT blur) so the redaction can't be reverse-engineered.
 */
import sharp from 'sharp';
import { readdir, mkdir } from 'node:fs/promises';
import { dirname, resolve, basename, extname } from 'node:path';
import { fileURLToPath } from 'node:url';
import config from './redact.config.mjs';

const __dirname = dirname(fileURLToPath(import.meta.url));
const REPO_ROOT = resolve(__dirname, '..');
const RAW_DIR   = resolve(REPO_ROOT, 'site/public/screenshots/raw');
const OUT_DIR   = resolve(REPO_ROOT, 'site/public/screenshots');

await mkdir(OUT_DIR, { recursive: true });

const files = (await readdir(RAW_DIR)).filter((f) => f.endsWith('.png'));
if (files.length === 0) {
  console.error(`No PNGs in ${RAW_DIR}. Run capture.mjs first.`);
  process.exit(1);
}

for (const file of files) {
  const name = basename(file, extname(file));
  const regions = config[name] ?? [];
  const inPath  = resolve(RAW_DIR, file);
  const outPath = resolve(OUT_DIR, file);

  if (regions.length === 0) {
    // Pass-through copy. Still re-encode through sharp so PNGs are uniform.
    await sharp(inPath).png().toFile(outPath);
    console.log(`[redact] ${name}: no regions, copied`);
    continue;
  }

  const overlays = regions.map((r) => ({
    input: {
      create: { width: r.w, height: r.h, channels: 3, background: '#000000' },
    },
    left: r.x,
    top:  r.y,
  }));

  await sharp(inPath).composite(overlays).png().toFile(outPath);
  console.log(`[redact] ${name}: ${regions.length} region(s) filled`);
}

console.log(`[redact] wrote ${files.length} files to ${OUT_DIR}`);
```

- [ ] **Step 2: Eyeball each raw PNG and fill `scripts/redact.config.mjs`**

Open each PNG in `site/public/screenshots/raw/` in a viewer that shows pixel coordinates (Preview.app on macOS shows them in the bottom-left). For each known sensitive region, note `{x, y, w, h}` and add to `redact.config.mjs`. Examples (your numbers will differ — measure):

```js
export default {
  'runs-dashboard':    [], // file paths can stay; no token columns visible
  'live-run':          [], // verify file path doesn't expose anything sensitive
  'flows-editor':      [], // YAML only
  'browse-manual':     [], // file paths only
  'plugins-browse':    [], // public catalog data
  'plugins-installed': [], // public plugin list
  'plugins-catalogs':  [{ x: 320, y: 240, w: 360, h: 24, label: 'auth_header' }],
  'sources-list':      [{ x: 380, y: 200, w: 280, h: 24, label: 'secret_token' }],
  'notifiers-list':    [{ x: 380, y: 200, w: 320, h: 24, label: 'bot_token_or_webhook_url' }],
  'hardware-probe':    [], // no secrets
};
```

- [ ] **Step 3: Run redact**

```bash
cd scripts
node redact.mjs
```

Expected: 10 PNGs in `site/public/screenshots/`, each printed.

- [ ] **Step 4: Manual review (the gate)**

Open every PNG in `site/public/screenshots/`. Visually scan for any text that looks like a secret token (`tcr_`, hex strings, long base64), webhook URLs (`https://discord.com/api/webhooks/...`), bot tokens (`123456:ABC-...` for Telegram). If anything slipped, update `redact.config.mjs` and re-run.

Run a final grep on the source files referenced too: confirm the page text is clean.

- [ ] **Step 5: Commit**

```bash
cd ..
git add scripts/redact.mjs scripts/redact.config.mjs site/public/screenshots/*.png
git commit -m "site: capture + redact 10 screenshots from live server"
```

---

### Task 17: Replace placeholder PNGs and verify the deployed site

**Files:**
- Modify: `site/public/screenshots/*.png` (replaced by Task 16)

After Task 16 lands, the placeholder PNGs are already replaced. This task is the verification that everything ties together end-to-end.

- [ ] **Step 1: Local preview**

```bash
cd site
npm run build
npm run preview
```

Visit `http://localhost:4321/transcoderr/`. Click through every section. Confirm every screenshot shows real UI. Click each tab in the screenshot showcase. Click Copy on the deploy block, paste somewhere — full compose.yml is on the clipboard.

- [ ] **Step 2: Push to a PR, watch CI**

If a PR isn't already open from after Task 14:

```bash
git push -u origin feat/showcase-site
gh pr create --title "feat: showcase site (GH Pages, Astro)" --body "$(cat <<'EOF'
## Summary

Single-page marketing site at \`https://seanbarzilay.github.io/transcoderr/\`.
Astro + Tailwind, dark theme, 9 sections (hero, what-it-does, screenshot
showcase, plan-then-execute side-by-side, plugins, MCP, deploy block, image
flavors, footer). Built and deployed by GH Actions on push to main when
\`site/**\` changes.

10 screenshots from the live server, redacted via solid black fills (not
blur — blur is reversible). Each PNG visually reviewed before commit.

## One-time setup after merge

- Settings → Pages → Source = "GitHub Actions" (the deploy job fails until
  this is set).

## Test plan

- [ ] CI \`pages\` workflow build job passes
- [ ] After enabling Pages source, the deploy job goes green
- [ ] \`https://seanbarzilay.github.io/transcoderr/\` renders all 9 sections
- [ ] Click each tab in the screenshot showcase
- [ ] Click Copy on the deploy block, paste — full compose.yml on clipboard
- [ ] Lighthouse: Performance ≥ 90, Accessibility ≥ 95
- [ ] No console errors in DevTools

🤖 Generated with [Claude Code](https://claude.com/claude-code)
EOF
)"
```

Watch the `pages` workflow run. The first time, the `deploy` job will fail until **Settings → Pages → Source = "GitHub Actions"** is set. Set it once.

- [ ] **Step 3: Visit the deployed site**

After the workflow's deploy job goes green, visit `https://seanbarzilay.github.io/transcoderr/`. Spot-check on mobile (DevTools responsive mode). Run a Lighthouse audit; aim for Performance ≥ 90, Accessibility ≥ 95.

- [ ] **Step 4: No commit needed for verification.**

If issues surface, fix in follow-up commits on the same branch.

---

## Self-Review Notes

This plan covers every section, file, and pipeline element from the spec:
- Sections 1-9 → Tasks 3-11
- Supporting thumbnails (sources, notifiers) → Task 12
- GH Actions workflow → Task 13
- README link → Task 14
- Screenshot pipeline (capture, redact, review) → Tasks 15-16
- End-to-end verification → Task 17

The placeholder script in Task 5 lets Track A be developed and reviewed without waiting for screenshots. Track B (Tasks 15-17) replaces those placeholders with redacted real ones.

Each task is one coherent commit. Task 1 sets the project shape; Task 2 establishes the screenshot directory contract. Tasks 3-12 each ship a visible page section. Task 13 is the deploy plumbing. Tasks 15-17 are operator-driven and require live-server access.

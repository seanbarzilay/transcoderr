#!/usr/bin/env node
/**
 * Capture screenshots from a running transcoderr server.
 *
 * Usage:
 *   TRANSCODERR_URL=http://192.168.1.176:8099 node scripts/capture.mjs
 *
 * Auth is assumed OFF on the operator's server. If you turn auth on,
 * extend this script with a /login flow.
 *
 * Outputs site/.screenshots-raw/<name>.png (gitignored, kept outside
 * site/public/ so Astro doesn't copy raws into dist/).
 */
import { chromium } from 'playwright';
import { mkdir } from 'node:fs/promises';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import shots from './shots.config.mjs';

const __dirname = dirname(fileURLToPath(import.meta.url));
const REPO_ROOT = resolve(__dirname, '..');
const RAW_DIR   = resolve(REPO_ROOT, 'site/.screenshots-raw');

const BASE_URL = process.env.TRANSCODERR_URL || 'http://192.168.1.176:8099';

await mkdir(RAW_DIR, { recursive: true });

/**
 * Setup hooks resolve the URL for a shot at runtime. They return the
 * relative URL to navigate to.
 */
const setupHooks = {
  /**
   * Find the latest run, rerun it, and return the URL of the new run.
   */
  rerunLatest: async (page) => {
    const ctx = page.context();
    const list = await ctx.request.get(`${BASE_URL}/api/runs?limit=1`, {
      failOnStatusCode: true,
    });
    const runs = await list.json();
    if (!runs.length) throw new Error('No runs to rerun.');
    const latestId = runs[0].id;

    const rerun = await ctx.request.post(`${BASE_URL}/api/runs/${latestId}/rerun`, {
      failOnStatusCode: true,
    });
    const body = await rerun.json();
    const newId = body.id;
    if (!newId) throw new Error(`No new run id in rerun response: ${JSON.stringify(body)}`);
    console.log(`[capture] rerunLatest: rerun ${latestId} -> new ${newId}`);
    return `/runs/${newId}`;
  },
};

const browser = await chromium.launch();
const context = await browser.newContext();
const page    = await context.newPage();

for (const shot of shots) {
  console.log(`[capture] ${shot.name}`);
  await page.setViewportSize({ width: shot.viewport[0], height: shot.viewport[1] });

  let url = shot.url;
  if (shot.setup) {
    const hook = setupHooks[shot.setup];
    if (!hook) throw new Error(`Unknown setup hook: ${shot.setup}`);
    url = await hook(page);
  }
  if (!url) throw new Error(`Shot ${shot.name} has no url and no setup`);

  await page.goto(`${BASE_URL}${url}`, { waitUntil: 'domcontentloaded' });

  if (shot.wait) {
    try {
      await page.waitForSelector(shot.wait, { timeout: 15_000 });
    } catch (e) {
      console.warn(`[capture] ${shot.name}: wait selector "${shot.wait}" timed out, screenshot anyway`);
    }
  } else {
    // No specific selector — give the page a moment to render.
    await page.waitForTimeout(2_000);
  }

  if (shot.tabClick) {
    // Plugins page: click the matching .plugin-tab button.
    const tab = page.locator(`.plugin-tab`, { hasText: shot.tabClick });
    await tab.click({ timeout: 5_000 });
    await page.waitForTimeout(800); // settle after tab switch
  }

  // Live run: give the progress bar and ffmpeg status line time to render.
  if (shot.setup === 'rerunLatest') {
    await page.waitForTimeout(6_000);
  }

  await page.screenshot({
    path: resolve(RAW_DIR, `${shot.name}.png`),
    fullPage: false,
  });
}

await browser.close();
console.log(`[capture] wrote ${shots.length} screenshots to ${RAW_DIR}`);

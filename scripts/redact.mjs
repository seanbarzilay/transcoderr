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

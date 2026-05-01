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

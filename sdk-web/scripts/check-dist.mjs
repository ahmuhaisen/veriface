#!/usr/bin/env node
// Post-build gate: the shipped JavaScript must contain no network endpoints (no CDN defaults, no
// telemetry) and all expected entry points must exist. Runs as the last step of `npm run build`.
import { existsSync, readdirSync, readFileSync, statSync } from 'node:fs';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';

const dist = fileURLToPath(new URL('../dist', import.meta.url));
const REQUIRED = [
  'index.js',
  'element.js',
  'face-capture.worker.js',
  'face-capture.worker.licenses.txt',
  'types/index.d.ts',
  'types/element.d.ts',
];
const FORBIDDEN = [/odml\.pa\.googleapis\.com/, /_mediapipeLoggerGetEncodedApiKey/, /sendBeacon/, /cdn\.jsdelivr\.net/, /unpkg\.com/];
const URL_PATTERN = /\bhttps?:\/\/[^\s"'`)]+/g;

function files(dir) {
  return readdirSync(dir).flatMap((name) => {
    const path = join(dir, name);
    return statSync(path).isDirectory() ? files(path) : [path];
  });
}

const problems = [];
for (const file of REQUIRED) if (!existsSync(join(dist, file))) problems.push(`missing dist/${file}`);
for (const file of files(dist).filter((f) => f.endsWith('.js'))) {
  const code = readFileSync(file, 'utf8');
  for (const pattern of FORBIDDEN) if (pattern.test(code)) problems.push(`${file}: contains ${pattern}`);
  for (const url of code.match(URL_PATTERN) ?? []) problems.push(`${file}: contains the URL ${url}`);
}

if (problems.length > 0) {
  console.error(`dist check failed:\n  ${problems.join('\n  ')}`);
  process.exit(1);
}
console.log('dist check passed: entry points present; no network endpoints in the shipped JavaScript');

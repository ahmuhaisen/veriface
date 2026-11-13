#!/usr/bin/env node
// Development helper: downloads the two MediaPipe model files the SDK expects from Google's model
// storage, verifies their SHA-256 and writes them to <dir>. This is an explicit, one-off developer
// action; the SDK itself never downloads anything at runtime. For production, take the files from
// your approved model pack and serve them from your own origin.
import { createHash } from 'node:crypto';
import { existsSync, mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { join, resolve } from 'node:path';

// Apache-2.0 models. The hashes must match MODEL_SHA256 in src/analysis/assets.ts.
const MODELS = [
  {
    file: 'face_landmarker.task',
    url: 'https://storage.googleapis.com/mediapipe-models/face_landmarker/face_landmarker/float16/1/face_landmarker.task',
    sha256: '64184e229b263107bc2b804c6625db1341ff2bb731874b0bcc2fe6544e0bc9ff',
  },
  {
    file: 'blaze_face_short_range.tflite',
    url: 'https://storage.googleapis.com/mediapipe-models/face_detector/blaze_face_short_range/float16/1/blaze_face_short_range.tflite',
    sha256: 'b4578f35940bf5a1a655214a1cce5cab13eba73c1297cd78e1a04c2380b0152f',
  },
];

const target = process.argv[2];
if (target === undefined) {
  console.error('Usage: node scripts/fetch-models.mjs <models-dir>');
  process.exit(1);
}
const out = resolve(target);
mkdirSync(out, { recursive: true });

const sha256 = (bytes) => createHash('sha256').update(bytes).digest('hex');

for (const model of MODELS) {
  const path = join(out, model.file);
  if (existsSync(path) && sha256(readFileSync(path)) === model.sha256) {
    console.log(`${model.file}: present, SHA-256 verified`);
    continue;
  }
  const response = await fetch(model.url);
  if (!response.ok) throw new Error(`${model.file}: HTTP ${response.status}`);
  const bytes = Buffer.from(await response.arrayBuffer());
  const actual = sha256(bytes);
  if (actual !== model.sha256) throw new Error(`${model.file}: SHA-256 mismatch (got ${actual})`);
  writeFileSync(path, bytes);
  console.log(`${model.file}: downloaded, SHA-256 verified`);
}

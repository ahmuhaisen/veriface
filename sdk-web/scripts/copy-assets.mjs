#!/usr/bin/env node
// Copies the runtime assets of @veriface/capture into a directory your web server serves, so the
// SDK loads everything from your own origin (no CDN). Run it at build or deploy time, for example:
//
//   npx veriface-capture-assets public/face-capture
//
// The models are not on npm; see the README ("Self-hosting the assets") and scripts/fetch-models.mjs.
import { copyFileSync, existsSync, mkdirSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const USAGE = `Usage: veriface-capture-assets <target-dir>

Copies into <target-dir>:
  face-capture.worker.js                        the SDK's analysis worker
  face-capture.worker.licenses.txt              third-party notices (MediaPipe, Apache-2.0)
  wasm/vision_wasm_internal.js|.wasm            MediaPipe Tasks Vision runtime (Wasm SIMD)
  wasm/vision_wasm_nosimd_internal.js|.wasm     fallback for browsers without Wasm SIMD

Place the model files yourself in <target-dir>/models/:
  face_landmarker.task, blaze_face_short_range.tflite`;

const WASM_FILES = [
  'vision_wasm_internal.js',
  'vision_wasm_internal.wasm',
  'vision_wasm_nosimd_internal.js',
  'vision_wasm_nosimd_internal.wasm',
];

const target = process.argv[2];
if (target === undefined || target === '--help' || target === '-h') {
  console.log(USAGE);
  process.exit(target === undefined ? 1 : 0);
}

const here = dirname(fileURLToPath(import.meta.url));
const worker = join(here, '..', 'dist', 'face-capture.worker.js');
const notices = join(here, '..', 'dist', 'face-capture.worker.licenses.txt');
for (const file of [worker, notices]) {
  if (!existsSync(file)) {
    console.error(`Missing ${file}. Build the SDK first (npm run build).`);
    process.exit(1);
  }
}

// The tasks-vision package exports its Wasm files; resolve them from this package's location.
const wasmDir = dirname(fileURLToPath(import.meta.resolve('@mediapipe/tasks-vision/vision_wasm_internal.js')));

const out = resolve(target);
mkdirSync(join(out, 'wasm'), { recursive: true });
mkdirSync(join(out, 'models'), { recursive: true });
copyFileSync(worker, join(out, 'face-capture.worker.js'));
copyFileSync(notices, join(out, 'face-capture.worker.licenses.txt'));
for (const file of WASM_FILES) copyFileSync(join(wasmDir, file), join(out, 'wasm', file));

console.log(`Copied the worker, its third-party notices and ${WASM_FILES.length} MediaPipe Wasm files to ${out}`);
const missing = ['face_landmarker.task', 'blaze_face_short_range.tflite'].filter(
  (file) => !existsSync(join(out, 'models', file)),
);
if (missing.length > 0) console.log(`Still needed in ${join(out, 'models')}: ${missing.join(', ')}`);

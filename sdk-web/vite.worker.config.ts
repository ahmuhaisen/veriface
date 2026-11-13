import { readFileSync } from 'node:fs';
import { defineConfig, type Plugin } from 'vite';
import pkg from './package.json' with { type: 'json' };

const mediapipe = pkg.dependencies['@mediapipe/tasks-vision'];
const NOTICES_FILE = 'face-capture.worker.licenses.txt';

/**
 * The worker redistributes MediaPipe Tasks Vision (Apache-2.0), whose npm package ships no licence
 * file, so the notices (attribution plus the full licence text) are written next to the worker.
 */
function thirdPartyNotices(): Plugin {
  return {
    name: 'veriface-third-party-notices',
    generateBundle() {
      const apache = readFileSync(new URL('./third-party/Apache-2.0.txt', import.meta.url), 'utf8');
      const source = [
        'Third-party software in the @veriface/capture runtime assets',
        '================================================================',
        '',
        `@mediapipe/tasks-vision ${mediapipe}`,
        '  Bundled in face-capture.worker.js; its Wasm files are served from wasm/.',
        '  Copyright The MediaPipe Authors.',
        '  Licence: Apache License, Version 2.0 (text below).',
        '',
        'MediaPipe models face_landmarker.task and blaze_face_short_range.tflite (served from models/)',
        '  Copyright Google LLC / The MediaPipe Authors.',
        '  Licence: Apache License, Version 2.0 (text below).',
        '',
        'MediaPipe canonical face model (asset mp-canonical-face: 468 vertices, 33 Procrustes weights)',
        '  Bundled in face-capture.worker.js and in the SDK JavaScript, for the head-pose fit.',
        '  Copyright The MediaPipe Authors.',
        '  Licence: Apache License, Version 2.0 (text below).',
        '',
        '----------------------------------------------------------------',
        '',
        apache,
      ].join('\n');
      this.emitFile({ type: 'asset', fileName: NOTICES_FILE, source });
    },
  };
}

// The analysis Web Worker, built as ONE self-contained classic script (IIFE) with MediaPipe
// bundled in. A classic worker lets MediaPipe load its Wasm loader with importScripts(), and a
// single file is easy to self-host next to the Wasm and model files (see README).
export default defineConfig({
  define: {
    __SDK_VERSION__: JSON.stringify(pkg.version),
  },
  plugins: [thirdPartyNotices()],
  build: {
    outDir: 'dist',
    emptyOutDir: false,
    target: 'es2022',
    sourcemap: true,
    minify: true,
    lib: {
      entry: 'src/analysis/worker.ts',
      formats: ['iife'],
      name: 'VerifaceCaptureWorker',
      fileName: () => 'face-capture.worker.js',
    },
    rolldownOptions: {
      output: {
        // After minification, so the attribution survives.
        postBanner:
          `/*! @veriface/capture ${pkg.version} analysis worker. Bundles @mediapipe/tasks-vision ${mediapipe}, ` +
          `Copyright The MediaPipe Authors, Apache License 2.0; see ${NOTICES_FILE}. */`,
      },
    },
  },
});

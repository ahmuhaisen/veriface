import { fileURLToPath } from 'node:url';
import { defineConfig } from 'vite';
import pkg from '../../package.json' with { type: 'json' };

// Dev server for the example page. Runtime assets are served from public/face-capture/, filled by
// `npm run example` (worker + Wasm) and `npm run example:models` (models, one-off download).
export default defineConfig({
  root: fileURLToPath(new URL('.', import.meta.url)),
  define: {
    __SDK_VERSION__: JSON.stringify(pkg.version),
  },
  server: {
    port: 5175,
    fs: { allow: [fileURLToPath(new URL('../..', import.meta.url))] },
  },
});

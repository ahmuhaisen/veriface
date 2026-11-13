import { defineConfig } from 'vite';
import pkg from './package.json' with { type: 'json' };

// ESM library build: the public entry points. MediaPipe stays external (it is the one runtime
// dependency) and is only reached through a lazily imported chunk used by the main-thread
// fallback; the Web Worker gets its own self-contained build (vite.worker.config.ts).
export default defineConfig({
  define: {
    __SDK_VERSION__: JSON.stringify(pkg.version),
  },
  build: {
    outDir: 'dist',
    emptyOutDir: true,
    target: 'es2022',
    sourcemap: true,
    lib: {
      entry: {
        index: 'src/index.ts',
        element: 'src/element.ts',
      },
      formats: ['es'],
      fileName: (_format, entryName) => `${entryName}.js`,
    },
    rolldownOptions: {
      external: ['@mediapipe/tasks-vision'],
      output: {
        chunkFileNames: 'chunks/[name]-[hash].js',
      },
    },
  },
});

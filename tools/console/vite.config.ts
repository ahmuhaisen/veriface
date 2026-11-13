// DEV-ONLY console server.
//
// - /api/*          -> the Veriface service. The proxy injects X-API-Key from .env.local (it plays the
//                      "project backend"), so the key never reaches browser code.
// - /samples/*      -> read-only listing and files from testdata/images and testdata/documents.
// - /sdk/*          -> sdk-web/dist (optional guided capture), /face-capture/* -> its worker, Wasm and models.
import { createReadStream, existsSync, readdirSync, readFileSync, statSync } from "node:fs";
import type { ServerResponse } from "node:http";
import { dirname, extname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { defineConfig, loadEnv, type Plugin } from "vite";

const here = dirname(fileURLToPath(import.meta.url));
const repo = resolve(here, "..", "..");
const testdata = join(repo, "testdata");
const sdkDist = join(repo, "sdk-web", "dist");
const tasksVision = join(repo, "sdk-web", "node_modules", "@mediapipe", "tasks-vision", "wasm");
const upstreamModels = join(repo, "models", "upstream");

const TYPES: Record<string, string> = {
  ".jpg": "image/jpeg",
  ".jpeg": "image/jpeg",
  ".png": "image/png",
  ".webp": "image/webp",
  ".js": "text/javascript",
  ".map": "application/json",
  ".wasm": "application/wasm",
  ".json": "application/json",
  ".task": "application/octet-stream",
  ".tflite": "application/octet-stream",
};
const SAFE_NAME = /^[A-Za-z0-9._-]+$/;

function send(res: ServerResponse, file: string): void {
  if (!existsSync(file) || !statSync(file).isFile()) {
    res.statusCode = 404;
    res.end("not found");
    return;
  }
  res.setHeader("Content-Type", TYPES[extname(file).toLowerCase()] ?? "application/octet-stream");
  res.setHeader("Cache-Control", "no-store");
  createReadStream(file).pipe(res);
}

/** Serve `root/<sub-path>` for a URL prefix, refusing anything that is not a plain relative path. */
function serveUnder(prefix: string, url: string, root: string, res: ServerResponse): boolean {
  if (!url.startsWith(prefix)) return false;
  const rel = decodeURIComponent(url.slice(prefix.length).split("?")[0] ?? "");
  const parts = rel.split("/");
  if (parts.some((p) => p === "" || p === ".." || !SAFE_NAME.test(p))) {
    res.statusCode = 400;
    res.end("bad path");
    return true;
  }
  send(res, join(root, ...parts));
  return true;
}

function devServerRoutes(): Plugin {
  return {
    name: "veriface-console-routes",
    configureServer(server) {
      server.middlewares.use((req, res, next) => {
        const url = req.url ?? "";
        if (!url.startsWith("/samples/") && !url.startsWith("/sdk") && !url.startsWith("/face-capture/")) return next();
        if (req.method !== "GET" && req.method !== "HEAD") {
          res.statusCode = 405;
          res.end("read-only");
          return;
        }
        if (url.startsWith("/samples/index.json")) {
          const list = (dir: string) =>
            existsSync(join(testdata, dir))
              ? readdirSync(join(testdata, dir)).filter((f) => /\.(jpe?g|png|webp)$/i.test(f)).sort()
              : [];
          let notes: Record<string, string> = {};
          const prov = join(testdata, "provenance.json");
          if (existsSync(prov)) {
            const p = JSON.parse(readFileSync(prov, "utf8"));
            for (const d of p.documents ?? []) notes[d.id] = `${d.variant}; expected ${d.expected}`;
            for (const d of p.derived ?? []) notes[d.id] = `${d.transform} of ${d.parent}`;
            for (const d of p.portraits ?? []) if (d.note) notes[d.id] = d.note;
          } else notes = {};
          res.setHeader("Content-Type", "application/json");
          res.end(JSON.stringify({ images: list("images"), documents: list("documents"), notes }));
          return;
        }
        if (serveUnder("/samples/images/", url, join(testdata, "images"), res)) return;
        if (serveUnder("/samples/documents/", url, join(testdata, "documents"), res)) return;
        if (url.startsWith("/sdk-status")) {
          const available = existsSync(join(sdkDist, "element.js")) && existsSync(join(tasksVision, "vision_wasm_internal.wasm"));
          res.setHeader("Content-Type", "application/json");
          res.end(JSON.stringify({ available }));
          return;
        }
        if (serveUnder("/sdk/", url, sdkDist, res)) return;
        // SDK asset layout (sdk-web README "Self-hosting the assets"): worker, wasm/, models/.
        if (url.startsWith("/face-capture/face-capture.worker.js")) return send(res, join(sdkDist, "face-capture.worker.js"));
        if (serveUnder("/face-capture/wasm/", url, tasksVision, res)) return;
        // The two browser models are byte-identical to the lab's pinned upstream files (facelab models bom).
        if (serveUnder("/face-capture/models/", url, upstreamModels, res)) return;
        next();
      });
    },
  };
}

export default defineConfig(({ mode }) => {
  const env = loadEnv(mode, here, ""); // reads .env.local; nothing without a VITE_ prefix reaches the client
  const target = env.VERIFACE_URL || "http://127.0.0.1:8080";
  const key = env.VERIFACE_API_KEY;
  if (!key) {
    console.warn("[console] VERIFACE_API_KEY is not set: run `npm run setup` (or set it in .env.local). /api calls will get 401.");
  }
  return {
    plugins: [devServerRoutes()],
    server: {
      host: "127.0.0.1",
      port: 5173,
      strictPort: false,
      proxy: {
        "/api": {
          target,
          changeOrigin: true,
          rewrite: (p) => p.replace(/^\/api/, ""),
          configure: (proxy) => {
            proxy.on("proxyReq", (proxyReq) => {
              proxyReq.removeHeader("x-api-key"); // never trust a key coming from the browser
              if (key) proxyReq.setHeader("X-API-Key", key);
            });
          },
        },
      },
    },
  };
});

#!/usr/bin/env node
/* global window, document, self, Image, XMLHttpRequest, EventSource */
// Network soak test: runs continuous selfie analysis (then document analysis) in Chromium on a
// synthetic camera, and fails if the page or the analysis worker contacts anything except the local
// dev server. Run it before any @mediapipe/tasks-vision upgrade:
//
//   npm run soak:network                      # 180 s selfie + 30 s document (defaults)
//   npm run soak:network -- --selfie 600 --document 60
//
// It records three independent views:
//   1. Chromium's NetLog (--log-net-log): every URL request, DNS lookup and socket the browser made.
//   2. Playwright request / websocket events for the page and its workers.
//   3. JavaScript-level attempts: fetch, XMLHttpRequest, navigator.sendBeacon, WebSocket,
//      EventSource and importScripts are wrapped in the page AND the worker, so a request the
//      SDK's fetch allow-list blocks (and that therefore never reaches the network) is still seen.
// Pass condition: no network destination other than the dev server, and no attempt (allowed or not)
// to any URL outside the dev server origin.
//
// Needs: `npm run build` (the npm script does it), the example models (`npm run example:models`),
// and the Playwright Chromium build matching the playwright-core devDependency
// (`npx playwright-core install chromium`). Uses public-domain test images from ../testdata.
import { spawnSync } from 'node:child_process';
import { existsSync, mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { chromium } from 'playwright-core';
import { createServer } from 'vite';

const root = fileURLToPath(new URL('..', import.meta.url));
const exampleDir = join(root, 'examples/basic');
const publicAssets = join(exampleDir, 'public/face-capture');

function arg(name, fallback) {
  const i = process.argv.indexOf(`--${name}`);
  return i > 0 && process.argv[i + 1] !== undefined ? process.argv[i + 1] : fallback;
}
const selfieSeconds = Number(arg('selfie', '180'));
const documentSeconds = Number(arg('document', '30'));
const outDir = resolve(arg('out', join(tmpdir(), `veriface-soak-${new Date().toISOString().replace(/[:.]/g, '-')}`)));
const headed = process.argv.includes('--headed');
const selfTest = process.argv.includes('--self-test');
mkdirSync(outDir, { recursive: true });

const mediapipeVersion = JSON.parse(
  readFileSync(join(root, 'node_modules/@mediapipe/tasks-vision/package.json'), 'utf8'),
).version;

// Prerequisites: the built worker, its Wasm, and the models, all served from the dev server.
if (!existsSync(join(root, 'dist/face-capture.worker.js'))) {
  console.error('dist/ is missing: run `npm run build` first.');
  process.exit(2);
}
const copy = spawnSync(process.execPath, [join(root, 'scripts/copy-assets.mjs'), publicAssets], { stdio: 'inherit' });
if (copy.status !== 0) process.exit(2);
for (const model of ['face_landmarker.task', 'blaze_face_short_range.tflite']) {
  if (!existsSync(join(publicAssets, 'models', model))) {
    console.error(`Missing ${model}: run \`npm run example:models\` first.`);
    process.exit(2);
  }
}

const testdata = join(root, '../testdata');
const dataUrl = (path) => `data:image/jpeg;base64,${readFileSync(path).toString('base64')}`;
const selfieImage = dataUrl(join(testdata, 'images/nasa-almansoori.jpg'));
const documentImage = dataUrl(join(testdata, 'documents/doc-watkins.jpg'));

// --- In-browser instrumentation ------------------------------------------------------------------

/** Wraps every request API of a global scope (page or worker) and logs each call. Idempotent. */
function installNetworkHooks() {
  const g = self;
  if (!g.__soak) g.__soak = { log: [], depth: 0 };
  const state = g.__soak;
  const record = (kind, url) => {
    if (state.depth === 0) state.log.push({ kind, url: String(url), t: Date.now() });
  };
  const wrap = (fn, kind, toUrl) => {
    const hooked = function (...args) {
      record(kind, toUrl(...args));
      state.depth++;
      try {
        return new.target ? Reflect.construct(fn, args, new.target) : fn.apply(this ?? g, args);
      } finally {
        state.depth--;
      }
    };
    hooked.__soak = true;
    if (fn.prototype) hooked.prototype = fn.prototype;
    return hooked;
  };
  const urlOf = (input) => (typeof input === 'string' ? input : input instanceof URL ? input.href : input?.url);
  // fetch is re-wrapped whenever something (the SDK's allow-list) replaced it, so blocked calls are seen.
  if (typeof g.fetch === 'function' && !g.fetch.__soak) g.fetch = wrap(g.fetch, 'fetch', urlOf);
  if (typeof XMLHttpRequest === 'function' && !XMLHttpRequest.prototype.open.__soak) {
    XMLHttpRequest.prototype.open = wrap(XMLHttpRequest.prototype.open, 'xhr', (_method, url) => url);
  }
  if (typeof WebSocket === 'function' && !WebSocket.__soak) g.WebSocket = wrap(WebSocket, 'websocket', urlOf);
  if (typeof EventSource === 'function' && !EventSource.__soak) g.EventSource = wrap(EventSource, 'eventsource', urlOf);
  if (typeof g.importScripts === 'function' && !g.importScripts.__soak) {
    g.importScripts = wrap(g.importScripts, 'importScripts', (...urls) => urls.join(' '));
  }
  if (typeof navigator.sendBeacon === 'function' && !navigator.sendBeacon.__soak) {
    navigator.sendBeacon = wrap(navigator.sendBeacon.bind(navigator), 'sendBeacon', urlOf);
  }
}

/** Page init script: hooks plus a synthetic camera built on canvas.captureStream(). */
function pageInit() {
  installNetworkHooks();
  setInterval(installNetworkHooks, 1000);
  const image = new Image();
  window.__soakSetImage = async (src) => {
    image.src = src;
    await image.decode();
  };
  navigator.mediaDevices.getUserMedia = async () => {
    const canvas = document.createElement('canvas');
    canvas.width = 1280;
    canvas.height = 960;
    const ctx = canvas.getContext('2d');
    let tick = 0;
    setInterval(() => {
      tick++;
      ctx.fillStyle = '#7a7a7a';
      ctx.fillRect(0, 0, canvas.width, canvas.height);
      if (image.naturalWidth > 0) {
        const scale = Math.min(canvas.width / image.naturalWidth, canvas.height / image.naturalHeight) * 0.9;
        const w = image.naturalWidth * scale;
        const h = image.naturalHeight * scale;
        // A slight drift and sensor-like noise row keep every frame different.
        const dx = Math.sin(tick / 15) * 2;
        ctx.drawImage(image, (canvas.width - w) / 2 + dx, (canvas.height - h) / 2, w, h);
        ctx.fillStyle = `rgb(${tick % 255},0,0)`;
        ctx.fillRect(0, canvas.height - 1, 4, 1);
      }
    }, 33);
    return canvas.captureStream(30);
  };
}

// --- Run -----------------------------------------------------------------------------------------

const server = await createServer({
  configFile: join(exampleDir, 'vite.config.ts'),
  logLevel: 'warn',
  server: { host: '127.0.0.1', port: 5199, strictPort: false, hmr: false },
});
await server.listen();
const address = server.httpServer?.address();
const origin = `http://127.0.0.1:${typeof address === 'object' && address ? address.port : 5199}`;
const netlogPath = join(outDir, 'netlog.json');

const browser = await chromium.launch({
  headless: !headed,
  args: [`--log-net-log=${netlogPath}`, '--net-log-capture-mode=Default', '--use-fake-ui-for-media-stream'],
});
const context = await browser.newContext({ permissions: ['camera'] });
const requests = [];
const websockets = [];
const consoleErrors = [];
const workers = [];
context.on('request', (r) => requests.push({ url: r.url(), method: r.method(), type: r.resourceType(), t: Date.now() }));
const hookSource = `(${installNetworkHooks.toString()})`;
await context.addInitScript(`${installNetworkHooks.toString()}\n(${pageInit.toString()})();`);

const page = await context.newPage();
page.on('websocket', (ws) => websockets.push({ url: ws.url(), t: Date.now() }));
page.on('console', (m) => {
  if (m.type() === 'error' || m.type() === 'warning') consoleErrors.push(`page ${m.type()}: ${m.text()}`);
});
// Each worker: (re)install the hooks every second, and drain its log, so a worker that is later
// terminated (mode switch) still reports what it attempted.
const drainWorker = async (worker, entry) => {
  const drained = await worker
    .evaluate(`${hookSource}(); self.__soak.log.splice(0)`)
    .catch(() => []);
  entry.log.push(...drained);
};
page.on('worker', (worker) => {
  const entry = { url: worker.url(), log: [], worker };
  workers.push(entry);
  void drainWorker(worker, entry);
  entry.timer = setInterval(() => void drainWorker(worker, entry), 1000);
  worker.on('close', () => {
    clearInterval(entry.timer);
    entry.closed = true;
  });
});

await page.goto(`${origin}/`);
await page.evaluate(() => {
  const el = document.querySelector('veriface-capture');
  window.__soakStats = { guidance: 0, codes: {}, statuses: [], errors: [] };
  el.addEventListener('guidance', (e) => {
    const s = window.__soakStats;
    s.guidance++;
    s.codes[e.detail.frameCode] = (s.codes[e.detail.frameCode] ?? 0) + 1;
    s.lastMeasurements = e.detail.measurements;
  });
  el.addEventListener('statuschange', (e) => window.__soakStats.statuses.push(e.detail));
  el.addEventListener('error', (e) => window.__soakStats.errors.push(String(e.detail?.code ?? e.detail)));
});

async function phase(mode, image, seconds) {
  await page.evaluate(
    async ({ mode, image }) => {
      const el = document.querySelector('veriface-capture');
      el.stop();
      await window.__soakSetImage(image);
      el.mode = mode;
      el.options = { autoCapture: false };
      window.__soakStats.guidance = 0;
      window.__soakStats.codes = {};
      await el.start();
    },
    { mode, image },
  );
  const start = Date.now();
  const samples = [];
  while (Date.now() - start < seconds * 1000) {
    await page.waitForTimeout(Math.min(30_000, seconds * 1000 - (Date.now() - start)));
    const stats = await page.evaluate(() => window.__soakStats.guidance);
    samples.push({ atSeconds: Math.round((Date.now() - start) / 1000), guidanceEvents: stats });
    console.log(`  ${mode}: ${samples.at(-1).atSeconds} s, ${stats} analysed frames`);
  }
  const stats = await page.evaluate(() => window.__soakStats);
  return {
    mode,
    seconds,
    guidanceEvents: stats.guidance,
    frameCodes: stats.codes,
    lastMeasurements: stats.lastMeasurements,
    samples,
  };
}

console.log(`Soak: @mediapipe/tasks-vision ${mediapipeVersion}, dev server ${origin}, output ${outDir}`);
const phases = [];
phases.push(await phase('selfie', selfieImage, selfieSeconds));
phases.push(await phase('document', documentImage, documentSeconds));

if (selfTest) {
  // `.invalid` never resolves (RFC 2606), so the control cannot reach Google even if unblocked.
  const probe = 'https://play.googleapis.com.invalid/log?soak-self-test';
  await page.evaluate((url) => navigator.sendBeacon(url, 'soak-self-test'), probe);
  for (const entry of workers) {
    if (!entry.closed) await entry.worker.evaluate(`fetch(${JSON.stringify(probe)}).catch(() => undefined)`);
  }
  await page.waitForTimeout(2000);
}

const pageAttempts = await page.evaluate(() => self.__soak.log);
for (const entry of workers) if (!entry.closed) await drainWorker(entry.worker, entry);
const finalStats = await page.evaluate(() => window.__soakStats);
await page.evaluate(() => document.querySelector('veriface-capture').stop());
for (const w of workers) clearInterval(w.timer);
await context.close();
await browser.close();
await server.close();

// --- NetLog: every destination the browser touched -----------------------------------------------

// Chromium completes the file on a clean exit; if it did not, close the events array ourselves.
const netlogText = readFileSync(netlogPath, 'utf8').trimEnd();
let netlog;
try {
  netlog = JSON.parse(netlogText);
} catch {
  netlog = JSON.parse(`${netlogText.replace(/,$/, '')}]}`);
}
const eventName = Object.fromEntries(Object.entries(netlog.constants.logEventTypes).map(([k, v]) => [v, k]));
const sourceName = Object.fromEntries(Object.entries(netlog.constants.logSourceType).map(([k, v]) => [v, k]));
const events = (netlog.events ?? []).map((e) => ({
  id: e.source.id,
  source: sourceName[e.source.type],
  type: eventName[e.type],
  params: e.params ?? {},
}));
// Every URL the network stack started, every host it resolved, every socket it connected.
const netUrls = new Set(
  events.filter((e) => e.type === 'URL_REQUEST_START_JOB' && typeof e.params.url === 'string').map((e) => e.params.url),
);
const netHosts = new Set(events.filter((e) => typeof e.params.host === 'string').map((e) => e.params.host));
const connects = events
  .filter((e) => /^(?:TCP_CONNECT|TCP_CONNECT_ATTEMPT|UDP_CONNECT|SOCKET_CONNECT)$/.test(e.type) && e.params.address)
  .map((e) => ({ id: e.id, source: e.source, type: e.type, address: e.params.address }));
const sentOn = new Set(
  events.filter((e) => /BYTES_SENT/.test(e.type)).map((e) => e.id),
);
// Chromium's IPv6 reachability check (HostResolverManager::IsGloballyReachable) connect()s a UDP
// socket to Google Public DNS (2001:4860:4860::8888) to see whether a global IPv6 route exists.
// A UDP connect sends no packet; it is listed as a browser probe, and fails the soak only if a
// datagram was actually sent on that socket.
const isReachabilityProbe = (c) =>
  /UDP/.test(c.source) && /^\[2001:4860:4860::8888\]:443$|^8\.8\.8\.8:443$/.test(c.address) && !sentOn.has(c.id);
const browserProbes = connects.filter(isReachabilityProbe);
const netAddresses = new Set(connects.filter((c) => !isReachabilityProbe(c)).map((c) => c.address));

const isLocalUrl = (url) => {
  if (/^(?:data|blob|about|chrome|chrome-extension|devtools):/.test(url)) return true;
  try {
    const u = new URL(url, origin);
    return u.origin === origin || (u.protocol === 'ws:' && u.host === new URL(origin).host);
  } catch {
    return false;
  }
};
const isLocalHost = (host) => /^(?:127\.0\.0\.1|localhost|\[?::1\]?|0\.0\.0\.0)(?::\d+)?$/.test(host);

const attempts = [
  ...pageAttempts.map((a) => ({ scope: 'page', ...a })),
  ...workers.flatMap((w) => w.log.map((a) => ({ scope: `worker ${w.url}`, ...a }))),
];
const foreign = {
  netlogUrls: [...netUrls].filter((u) => !isLocalUrl(u)),
  netlogHosts: [...netHosts].filter((h) => (/^[a-z]+:\/\//.test(h) ? !isLocalUrl(h) : !isLocalHost(h))),
  netlogSockets: [...netAddresses].filter((a) => !isLocalHost(a)),
  playwrightRequests: requests.filter((r) => !isLocalUrl(r.url)),
  websockets: websockets.filter((w) => !isLocalUrl(w.url)),
  jsAttempts: attempts.filter((a) => !isLocalUrl(a.url)),
};
const googleRe = /googleapis|gstatic|google\.com|clearcut|doubleclick|firebase|play\.google/i;
const googleAttempts = attempts.filter((a) => googleRe.test(a.url));
const clean =
  Object.values(foreign).every((list) => list.length === 0) &&
  googleAttempts.length === 0 &&
  phases.every((p) => p.guidanceEvents > 0);
// --self-test is a positive control: the injected beacon (page) and the fetch the SDK's allow-list
// blocks (worker) must both be detected, otherwise the soak's instrumentation is broken.
const selfTestDetected =
  googleAttempts.some((a) => a.scope === 'page' && a.kind === 'sendBeacon') &&
  googleAttempts.some((a) => a.scope.startsWith('worker') && a.kind === 'fetch');
const passed = selfTest ? selfTestDetected && phases.every((p) => p.guidanceEvents > 0) : clean;

const summarise = (urls) => {
  const counts = {};
  for (const u of urls) {
    const key = u.startsWith('data:') ? 'data:' : u.startsWith('blob:') ? 'blob:' : u.replace(/\?.*$/, '');
    counts[key] = (counts[key] ?? 0) + 1;
  }
  return counts;
};
const report = {
  passed,
  selfTest: selfTest ? { detected: selfTestDetected } : undefined,
  mediapipeVersion,
  browser: 'Chromium (playwright-core)',
  origin,
  phases,
  finalStatuses: finalStats.statuses.slice(-5),
  sdkErrors: finalStats.errors,
  requests: { total: requests.length, byUrl: summarise(requests.map((r) => r.url)) },
  websockets,
  jsAttempts: { total: attempts.length, byKindAndUrl: summarise(attempts.map((a) => `${a.scope} ${a.kind} ${a.url}`)) },
  netlog: {
    urls: [...netUrls].sort(),
    hosts: [...netHosts].sort(),
    sockets: [...netAddresses].sort(),
    browserProbes: browserProbes.map((c) => `${c.source} ${c.type} ${c.address} (no datagram sent)`),
  },
  googleAttempts,
  foreign,
  consoleErrors: consoleErrors.slice(0, 50),
};
writeFileSync(join(outDir, 'soak-report.json'), JSON.stringify(report, null, 2));

const originOf = (url) => (/^(?:data|blob):/.test(url) ? url.slice(0, 5) : new URL(url).origin);
console.log(
  JSON.stringify(
    {
      passed,
      mediapipeVersion,
      phases: phases.map((p) => ({ mode: p.mode, seconds: p.seconds, frames: p.guidanceEvents, codes: p.frameCodes })),
      requests: { total: requests.length, origins: [...new Set(requests.map((r) => originOf(r.url)))] },
      websockets: websockets.map((w) => w.url.replace(/\?.*$/, '')),
      jsAttempts: report.jsAttempts.byKindAndUrl,
      netlog: {
        urlRequests: netUrls.size,
        urlOrigins: [...new Set([...netUrls].map(originOf))],
        hosts: report.netlog.hosts,
        sockets: report.netlog.sockets,
        browserProbes: report.netlog.browserProbes,
      },
      googleAttempts: googleAttempts.length,
      foreign,
      sdkErrors: report.sdkErrors,
    },
    null,
    2,
  ),
);
if (selfTest) console.log(`\nSelf-test: injected Google-bound requests detected: ${String(selfTestDetected)}`);
console.log(`\n${passed ? 'PASS' : 'FAIL'}: report at ${join(outDir, 'soak-report.json')}`);
process.exit(passed ? 0 : 1);

#!/usr/bin/env node
// Veriface load and soak test. Node >= 18, no dependencies.
//
// Replays selfie (/v1/quality), document (/v1/document-portraits) and verification
// (/v1/verifications) requests built from the public-domain images in testdata/, with a fixed
// number of concurrent workers for a fixed duration. Reports throughput, latency percentiles per
// endpoint and the status / problem-code / outcome mix. In soak mode it also samples the
// service's resident memory every --sample-interval seconds and flags growth after warm-up.
//
// The API key is read from the VERIFACE_API_KEY environment variable, or from --env-file
// (a KEY=VALUE file such as tools/console/.env.local). It is never printed.
//
// Usage: node ops/loadtest/loadtest.mjs --help

import { readFileSync, readdirSync, writeFileSync, existsSync } from 'node:fs';
import { join, dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { execFileSync } from 'node:child_process';
import { performance } from 'node:perf_hooks';

const HERE = dirname(fileURLToPath(import.meta.url));
const REPO = resolve(HERE, '..', '..');

const HELP = `Veriface load / soak test

  node ops/loadtest/loadtest.mjs [options]

Options
  --url <base>            Service base URL               (default http://127.0.0.1:8080)
  --env-file <path>       KEY=VALUE file holding VERIFACE_API_KEY (else the env variable)
  --concurrency <n>       Concurrent workers             (default 4)
  --duration <s>          Test duration in seconds       (default 60)
  --mix <spec>            Request weights                (default quality:2,document:1,verify:1)
  --profile <name>        ?profile= to send              (default: the key's default profile).
                          With the dev console config use "fixtures": the small public-domain
                          document portraits fail INTER_EYE_DISTANCE under "standard", which turns
                          every verification into an early "retake" (no embedding).
  --output                Send ?output=true on quality/document requests
  --timeout <ms>          Client timeout per request     (default 120000)
  --soak                  Soak mode: sample service memory and check for growth
  --pid <pid>             Service process id for memory sampling (default: found from the port)
  --sample-interval <s>   Memory sampling interval        (default 30)
  --warmup <s>            Soak warm-up excluded from the growth baseline (default 120)
  --growth-limit <pct>    Flag growth above this percentage (default 10)
  --json <path>           Write the full result as JSON
  --label <text>          Free-text label stored in the JSON result
  --help

Exit code: 0 ok, 1 a leak signal in soak mode, 2 usage or setup error.
`;

// ---------------------------------------------------------------------------------- arguments

function parseArgs(argv) {
  const opts = {
    url: 'http://127.0.0.1:8080',
    envFile: null,
    concurrency: 4,
    duration: 60,
    mix: 'quality:2,document:1,verify:1',
    profile: null,
    output: false,
    timeout: 120000,
    soak: false,
    pid: null,
    sampleInterval: 30,
    warmup: 120,
    growthLimit: 10,
    json: null,
    label: '',
  };
  const flags = new Set(['soak', 'output', 'help']);
  for (let i = 0; i < argv.length; i++) {
    const arg = argv[i];
    if (!arg.startsWith('--')) die(`unexpected argument ${arg}`);
    const name = arg.slice(2).replace(/-([a-z])/g, (_, c) => c.toUpperCase());
    if (name === 'help') {
      process.stdout.write(HELP);
      process.exit(0);
    }
    if (!(name in opts)) die(`unknown option ${arg}`);
    if (flags.has(name)) {
      opts[name] = true;
      continue;
    }
    const value = argv[++i];
    if (value === undefined) die(`${arg} needs a value`);
    opts[name] = typeof opts[name] === 'number' || ['pid'].includes(name) ? Number(value) : value;
    if (Number.isNaN(opts[name])) die(`${arg}: not a number`);
  }
  opts.url = opts.url.replace(/\/+$/, '');
  return opts;
}

function die(message) {
  process.stderr.write(`loadtest: ${message}\n`);
  process.exit(2);
}

function readApiKey(envFile) {
  if (envFile) {
    const text = readFileSync(envFile, 'utf8');
    for (const line of text.split(/\r?\n/)) {
      const m = line.match(/^\s*(?:export\s+)?VERIFACE_API_KEY\s*=\s*(.*)\s*$/);
      if (m) return m[1].replace(/^['"]|['"]$/g, '');
    }
    die(`${envFile} has no VERIFACE_API_KEY`);
  }
  if (process.env.VERIFACE_API_KEY) return process.env.VERIFACE_API_KEY;
  die('no API key: set VERIFACE_API_KEY or pass --env-file');
}

// ------------------------------------------------------------------------------------ fixtures

function loadImages(dir) {
  const full = join(REPO, 'testdata', dir);
  return readdirSync(full)
    .filter((f) => /\.(jpe?g|png|webp)$/i.test(f))
    .sort()
    .map((f) => ({
      name: f,
      bytes: readFileSync(join(full, f)),
      type: /\.png$/i.test(f) ? 'image/png' : /\.webp$/i.test(f) ? 'image/webp' : 'image/jpeg',
    }));
}

function parseMix(spec) {
  const kinds = { quality: 1, document: 1, verify: 1 };
  const entries = spec.split(',').map((part) => {
    const [kind, weight] = part.split(':');
    if (!(kind in kinds)) die(`--mix: unknown request kind ${kind} (quality, document, verify)`);
    const w = Number(weight ?? 1);
    if (!(w >= 0)) die(`--mix: bad weight for ${kind}`);
    return { kind, weight: w };
  });
  const total = entries.reduce((s, e) => s + e.weight, 0);
  if (total <= 0) die('--mix: all weights are zero');
  return { entries, total };
}

// ------------------------------------------------------------------------------------- requests

const ENDPOINT = {
  quality: '/v1/quality',
  document: '/v1/document-portraits',
  verify: '/v1/verifications',
};

let seed = 0x2545f491;
function rand() {
  // xorshift32: reproducible request sequence across runs.
  seed ^= seed << 13;
  seed ^= seed >>> 17;
  seed ^= seed << 5;
  return (seed >>> 0) / 4294967296;
}
const pick = (list) => list[Math.floor(rand() * list.length)];

function pickKind(mix) {
  let r = rand() * mix.total;
  for (const e of mix.entries) {
    if ((r -= e.weight) < 0) return e.kind;
  }
  return mix.entries[mix.entries.length - 1].kind;
}

function buildRequest(kind, fixtures, opts) {
  const form = new FormData();
  const blob = (img) => new Blob([img.bytes], { type: img.type });
  const query = new URLSearchParams();
  if (opts.profile) query.set('profile', opts.profile);
  if (kind === 'quality') {
    const img = pick(fixtures.selfies);
    form.append('image', blob(img), img.name);
    if (opts.output) query.set('output', 'true');
  } else if (kind === 'document') {
    const img = pick(fixtures.documents);
    form.append('document', blob(img), img.name);
    if (opts.output) query.set('output', 'true');
  } else {
    // Half genuine pairs (doc-<name>-*.jpg with nasa-<name>.jpg), half random (mostly impostors).
    const reference = pick(fixtures.documents);
    const person = reference.name.match(/^doc-([a-z]+)/)?.[1];
    const genuine = fixtures.selfies.find((s) => s.name === `nasa-${person}.jpg`);
    const probe = genuine && rand() < 0.5 ? genuine : pick(fixtures.selfies);
    form.append('probe', blob(probe), probe.name);
    form.append('reference', blob(reference), reference.name);
  }
  const qs = query.toString();
  return { url: `${opts.url}${ENDPOINT[kind]}${qs ? `?${qs}` : ''}`, form };
}

// ------------------------------------------------------------------------------------- results

function newStats() {
  return {
    latencies: [], // ms, every completed HTTP response
    statuses: {}, // "200": n
    codes: {}, // problem code: n
    results: {}, // verdict/outcome: n
    transport: {}, // network error name: n
  };
}

const inc = (obj, key) => {
  obj[key] = (obj[key] ?? 0) + 1;
};

async function oneRequest(kind, fixtures, opts, key, stats) {
  const { url, form } = buildRequest(kind, fixtures, opts);
  const start = performance.now();
  try {
    const res = await fetch(url, {
      method: 'POST',
      headers: { 'X-API-Key': key },
      body: form,
      signal: AbortSignal.timeout(opts.timeout),
    });
    const text = await res.text();
    const ms = performance.now() - start;
    const s = stats[kind];
    s.latencies.push(ms);
    inc(s.statuses, String(res.status));
    let body = null;
    try {
      body = JSON.parse(text);
    } catch {
      /* not JSON */
    }
    if (res.status >= 400) {
      inc(s.codes, body?.code ?? `http-${res.status}`);
    } else if (body) {
      inc(s.results, body.outcome ?? body.verdict ?? 'unknown');
    }
    return res.status;
  } catch (e) {
    inc(stats[kind].transport, e?.name === 'TimeoutError' ? 'client-timeout' : (e?.cause?.code ?? e?.name ?? 'error'));
    // A refused or reset connection fails instantly: pause so a worker does not spin.
    await new Promise((r) => setTimeout(r, 100));
    return 0;
  }
}

function percentile(sorted, p) {
  if (sorted.length === 0) return null;
  const idx = Math.min(sorted.length - 1, Math.max(0, Math.ceil((p / 100) * sorted.length) - 1));
  return sorted[idx];
}

function summarize(stats, seconds) {
  const out = {};
  const all = newStats();
  for (const [kind, s] of Object.entries(stats)) {
    if (s.latencies.length === 0 && Object.keys(s.transport).length === 0) continue;
    out[kind] = summarizeOne(s, seconds);
    all.latencies.push(...s.latencies);
    for (const f of ['statuses', 'codes', 'results', 'transport']) {
      for (const [k, v] of Object.entries(s[f])) all[f][k] = (all[f][k] ?? 0) + v;
    }
  }
  out.all = summarizeOne(all, seconds);
  return out;
}

function summarizeOne(s, seconds) {
  const sorted = [...s.latencies].sort((a, b) => a - b);
  const ok = Object.entries(s.statuses)
    .filter(([k]) => k.startsWith('2'))
    .reduce((n, [, v]) => n + v, 0);
  const round = (v) => (v == null ? null : Math.round(v));
  return {
    requests: sorted.length,
    ok,
    throughputRps: +(sorted.length / seconds).toFixed(2),
    okRps: +(ok / seconds).toFixed(2),
    latencyMs: {
      min: round(sorted[0]),
      p50: round(percentile(sorted, 50)),
      p90: round(percentile(sorted, 90)),
      p95: round(percentile(sorted, 95)),
      p99: round(percentile(sorted, 99)),
      max: round(sorted[sorted.length - 1]),
      mean: round(sorted.reduce((a, b) => a + b, 0) / (sorted.length || 1)),
    },
    statuses: s.statuses,
    problemCodes: s.codes,
    results: s.results,
    transportErrors: s.transport,
  };
}

// -------------------------------------------------------------------------------------- memory

function findPidByPort(url) {
  const port = new URL(url).port || '80';
  try {
    if (process.platform === 'win32') {
      const out = execFileSync('netstat', ['-ano', '-p', 'TCP'], { encoding: 'utf8' });
      for (const line of out.split(/\r?\n/)) {
        const cols = line.trim().split(/\s+/);
        if (cols[0] === 'TCP' && cols[1]?.endsWith(`:${port}`) && cols[3] === 'LISTENING') {
          return Number(cols[4]);
        }
      }
    } else {
      const out = execFileSync('sh', ['-c', `ss -ltnp 'sport = :${port}'`], { encoding: 'utf8' });
      const m = out.match(/pid=(\d+)/);
      if (m) return Number(m[1]);
    }
  } catch {
    /* fall through */
  }
  return null;
}

/// Resident memory of the service process in bytes: Windows working set and private bytes,
/// Linux VmRSS. Null when the process is gone.
function sampleMemory(pid) {
  try {
    if (process.platform === 'win32') {
      const out = execFileSync(
        'powershell.exe',
        [
          '-NoProfile',
          '-NonInteractive',
          '-Command',
          `$p = Get-Process -Id ${pid} -ErrorAction Stop; "$($p.WorkingSet64) $($p.PrivateMemorySize64) $($p.Threads.Count) $($p.HandleCount)"`,
        ],
        { encoding: 'utf8', timeout: 20000 },
      ).trim();
      const [rss, priv, threads, handles] = out.split(/\s+/).map(Number);
      return { rss, private: priv, threads, handles };
    }
    const status = readFileSync(`/proc/${pid}/status`, 'utf8');
    const kb = (name) => Number(status.match(new RegExp(`${name}:\\s+(\\d+)`))?.[1] ?? 0) * 1024;
    const threads = Number(status.match(/Threads:\s+(\d+)/)?.[1] ?? 0);
    return { rss: kb('VmRSS'), private: kb('RssAnon'), threads, handles: null };
  } catch {
    return null;
  }
}

function analyzeMemory(samples, warmupS, limitPct) {
  const after = samples.filter((s) => s.t >= warmupS && s.rss);
  if (after.length < 4) {
    return { verdict: 'insufficient-samples', samplesAfterWarmup: after.length };
  }
  const k = Math.min(3, Math.floor(after.length / 3));
  const mean = (arr, f) => arr.reduce((a, s) => a + s[f], 0) / arr.length;
  const result = {};
  for (const field of ['rss', 'private']) {
    const base = mean(after.slice(0, k), field);
    const end = mean(after.slice(-k), field);
    const peak = Math.max(...after.map((s) => s[field]));
    // Least-squares slope, bytes per minute.
    const n = after.length;
    const mt = after.reduce((a, s) => a + s.t, 0) / n;
    const mv = mean(after, field);
    const num = after.reduce((a, s) => a + (s.t - mt) * (s[field] - mv), 0);
    const den = after.reduce((a, s) => a + (s.t - mt) ** 2, 0) || 1;
    result[field] = {
      baselineMiB: +(base / 2 ** 20).toFixed(1),
      endMiB: +(end / 2 ** 20).toFixed(1),
      peakMiB: +(peak / 2 ** 20).toFixed(1),
      growthPct: +(((end - base) / base) * 100).toFixed(2),
      slopeMiBPerMin: +(((num / den) * 60) / 2 ** 20).toFixed(2),
    };
  }
  const flagged = result.rss.growthPct > limitPct || result.private.growthPct > limitPct;
  return {
    verdict: flagged ? 'LEAK-SIGNAL' : 'ok',
    limitPct,
    samplesAfterWarmup: after.length,
    baselineSamples: k,
    ...result,
  };
}

// ---------------------------------------------------------------------------------------- main

async function main() {
  const opts = parseArgs(process.argv.slice(2));
  const key = readApiKey(opts.envFile);
  const mix = parseMix(opts.mix);
  const fixtures = { selfies: loadImages('images'), documents: loadImages('documents') };
  if (!fixtures.selfies.length || !fixtures.documents.length) die('testdata images not found');

  const ready = await fetch(`${opts.url}/health/ready`).catch((e) => die(`service unreachable: ${e.message}`));
  if (ready.status !== 200) die(`service not ready (${ready.status})`);

  const pid = opts.soak ? (opts.pid ?? findPidByPort(opts.url)) : opts.pid;
  if (opts.soak && !pid) die('soak mode: could not find the service pid; pass --pid');

  const stats = { quality: newStats(), document: newStats(), verify: newStats() };
  const memory = [];
  const t0 = performance.now();
  const deadline = t0 + opts.duration * 1000;
  const elapsed = () => (performance.now() - t0) / 1000;

  log(
    `start: ${opts.url} concurrency=${opts.concurrency} duration=${opts.duration}s mix=${opts.mix}` +
      `${opts.soak ? ` soak pid=${pid} every ${opts.sampleInterval}s warmup=${opts.warmup}s` : ''}` +
      ` (${fixtures.selfies.length} selfies, ${fixtures.documents.length} documents)`,
  );

  let sampler = null;
  if (pid) {
    const take = () => {
      const m = sampleMemory(pid);
      if (m) {
        const s = { t: Math.round(elapsed()), ...m };
        memory.push(s);
        if (opts.soak) {
          const done = Object.values(stats).reduce((n, x) => n + x.latencies.length, 0);
          log(
            `t=${s.t}s rss=${(m.rss / 2 ** 20).toFixed(1)}MiB private=${(m.private / 2 ** 20).toFixed(1)}MiB` +
              ` threads=${m.threads} handles=${m.handles ?? '-'} requests=${done}`,
          );
        }
      } else {
        log(`memory sample failed: pid ${pid} not found (service down?)`);
      }
    };
    take();
    sampler = setInterval(take, opts.sampleInterval * 1000);
  }

  const worker = async () => {
    while (performance.now() < deadline) {
      await oneRequest(pickKind(mix), fixtures, opts, key, stats);
    }
  };
  await Promise.all(Array.from({ length: opts.concurrency }, worker));
  const seconds = elapsed();
  if (sampler) {
    clearInterval(sampler);
    const last = sampleMemory(pid);
    if (last) memory.push({ t: Math.round(seconds), ...last });
  }

  const summary = summarize(stats, seconds);
  const result = {
    label: opts.label,
    startedAt: new Date(Date.now() - seconds * 1000).toISOString(),
    durationS: +seconds.toFixed(1),
    url: opts.url,
    concurrency: opts.concurrency,
    mix: opts.mix,
    profile: opts.profile,
    output: opts.output,
    host: { platform: process.platform, node: process.version },
    summary,
  };
  if (memory.length) {
    result.memory = {
      pid,
      samples: memory,
      analysis: opts.soak ? analyzeMemory(memory, opts.warmup, opts.growthLimit) : undefined,
    };
  }

  printSummary(result);
  if (opts.json) writeFileSync(opts.json, JSON.stringify(result, null, 2));
  if (result.memory?.analysis?.verdict === 'LEAK-SIGNAL') process.exitCode = 1;
}

function log(msg) {
  process.stderr.write(`[${new Date().toISOString().slice(11, 19)}] ${msg}\n`);
}

function printSummary(r) {
  const rows = [['endpoint', 'reqs', 'req/s', 'p50', 'p90', 'p95', 'p99', 'max', 'statuses', 'problems', 'results']];
  for (const [kind, s] of Object.entries(r.summary)) {
    const fmt = (o) =>
      Object.entries(o)
        .map(([k, v]) => `${k}:${v}`)
        .join(' ') || '-';
    rows.push([
      kind,
      s.requests,
      s.throughputRps,
      s.latencyMs.p50,
      s.latencyMs.p90,
      s.latencyMs.p95,
      s.latencyMs.p99,
      s.latencyMs.max,
      fmt({ ...s.statuses, ...s.transportErrors }),
      fmt(s.problemCodes),
      fmt(s.results),
    ]);
  }
  const widths = rows[0].map((_, i) => Math.max(...rows.map((row) => String(row[i] ?? '').length)));
  const lines = rows.map((row) => row.map((c, i) => String(c ?? '').padEnd(widths[i])).join('  '));
  process.stdout.write(
    `\n${r.label ? `${r.label}: ` : ''}${r.durationS}s at concurrency ${r.concurrency} (latency ms)\n${lines.join('\n')}\n`,
  );
  const a = r.memory?.analysis;
  if (a) {
    process.stdout.write(
      a.rss
        ? `memory: ${a.verdict} (limit ${a.limitPct}%): rss ${a.rss.baselineMiB} -> ${a.rss.endMiB} MiB` +
            ` (${a.rss.growthPct}%, peak ${a.rss.peakMiB}, slope ${a.rss.slopeMiBPerMin} MiB/min);` +
            ` private ${a.private.baselineMiB} -> ${a.private.endMiB} MiB (${a.private.growthPct}%)\n`
        : `memory: ${a.verdict} (${a.samplesAfterWarmup} samples after warm-up)\n`,
    );
  }
}

if (!existsSync(join(REPO, 'testdata'))) die(`testdata/ not found under ${REPO}`);
main().catch((e) => die(e.stack ?? String(e)));

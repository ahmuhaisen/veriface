#!/usr/bin/env node
// Generates ops/grafana/veriface.json (Grafana 10+/11 dashboard). Edit this file, then run:
//   node ops/grafana/build-dashboard.mjs
// Metric names: service/crates/face-server/src/metrics.rs.

import { writeFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const DS = { type: 'prometheus', uid: '${datasource}' };
const SEL = 'job=~"$job", instance=~"$instance"';
const V1 = `endpoint=~"/v1/.*"`;
const W = '$__rate_interval';

// HTTP status -> problem code(s) from contracts/openapi.yaml (the metrics carry the status only).
const PROBLEMS = [
  ['400', 'invalid-request'],
  ['401', 'unauthorized'],
  ['403', 'profile-forbidden'],
  ['404', 'profile-not-found / not-found'],
  ['405', 'method-not-allowed'],
  ['413', 'payload-too-large'],
  ['415', 'unsupported-media-type'],
  ['422', 'image-decode-failed / image-too-large'],
  ['429', 'rate-limited'],
  ['500', 'internal'],
  ['503', 'overloaded / not-ready'],
];
const withProblem = (expr) =>
  PROBLEMS.reduce((e, [status, code]) => `label_replace(${e}, "problem", "${status} ${code}", "status", "${status}")`, expr);

let nextId = 1;
const panels = [];
let y = 0;

function row(title) {
  panels.push({ type: 'row', title, id: nextId++, collapsed: false, gridPos: { h: 1, w: 24, x: 0, y }, panels: [] });
  y += 1;
}

/** Lay out panels of the given widths on one line. */
function line(height, ...items) {
  let x = 0;
  for (const [w, p] of items) {
    panels.push({ ...p, id: nextId++, datasource: DS, gridPos: { h: height, w, x, y } });
    x += w;
  }
  y += height;
}

const target = (expr, legendFormat = '', extra = {}) => ({ refId: String.fromCharCode(64 + nextRef()), datasource: DS, expr, legendFormat, ...extra });
let refCounter = 0;
function nextRef() {
  refCounter = (refCounter % 26) + 1;
  return refCounter;
}

function ts(title, targets, { unit = 'short', description = '', stack = false, min = undefined, max = undefined, overrides = [], thresholds } = {}) {
  refCounter = 0;
  return {
    type: 'timeseries',
    title,
    description,
    targets: targets.map((t) => (Array.isArray(t) ? target(...t) : t)),
    fieldConfig: {
      defaults: {
        unit,
        min,
        max,
        custom: {
          drawStyle: 'line',
          lineWidth: 1,
          fillOpacity: stack ? 25 : 8,
          showPoints: 'never',
          stacking: { mode: stack ? 'normal' : 'none', group: 'A' },
          spanNulls: false,
          thresholdsStyle: { mode: thresholds ? 'line' : 'off' },
        },
        ...(thresholds ? { thresholds: { mode: 'absolute', steps: thresholds } } : {}),
      },
      overrides,
    },
    options: { legend: { displayMode: 'table', placement: 'right', calcs: ['lastNotNull', 'max'] }, tooltip: { mode: 'multi', sort: 'desc' } },
  };
}

function stat(title, expr, { unit = 'short', description = '', thresholds, decimals, mappings = [] } = {}) {
  refCounter = 0;
  return {
    type: 'stat',
    title,
    description,
    targets: [target(expr)],
    fieldConfig: {
      defaults: {
        unit,
        decimals,
        mappings,
        thresholds: { mode: 'absolute', steps: thresholds ?? [{ color: 'green', value: null }] },
      },
      overrides: [],
    },
    options: { reduceOptions: { calcs: ['lastNotNull'], fields: '', values: false }, colorMode: 'background', graphMode: 'area', textMode: 'auto' },
  };
}

function text(title, content) {
  return { type: 'text', title, options: { mode: 'markdown', content } };
}

const quantile = (q, by, metric, filter) =>
  `histogram_quantile(${q}, sum by (le${by ? `, ${by}` : ''}) (rate(${metric}_bucket{${SEL}${filter ? `, ${filter}` : ''}}[${W}])))`;

// ------------------------------------------------------------------------------------ overview
row('Overview');
line(
  4,
  [3, stat('Scrape up', `min(up{${SEL}})`, {
    description: 'Prometheus can scrape /metrics. Readiness itself (/health/ready) needs a blackbox probe: see ops/README.md.',
    mappings: [{ type: 'value', options: { 0: { text: 'DOWN', color: 'red' }, 1: { text: 'UP', color: 'green' } } }],
    thresholds: [{ color: 'red', value: null }, { color: 'green', value: 1 }],
  })],
  [3, stat('Ready (probe)', `min(probe_success{job=~"$probe_job"})`, {
    description: 'Blackbox exporter probe of /health/ready (200 only after warm-up and until shutdown). No data = no probe configured.',
    mappings: [{ type: 'value', options: { 0: { text: 'NOT READY', color: 'red' }, 1: { text: 'READY', color: 'green' } } }],
    thresholds: [{ color: 'red', value: null }, { color: 'green', value: 1 }],
  })],
  [4, stat('Requests /s (v1)', `sum(rate(face_requests_total{${SEL}, ${V1}}[${W}]))`, { unit: 'reqps', decimals: 2 })],
  [4, stat('Errors (5xx) %', `100 * sum(rate(face_requests_total{${SEL}, ${V1}, status=~"5.."}[${W}])) / clamp_min(sum(rate(face_requests_total{${SEL}, ${V1}}[${W}])), 1e-9)`, {
    unit: 'percent', decimals: 2,
    thresholds: [{ color: 'green', value: null }, { color: 'orange', value: 1 }, { color: 'red', value: 5 }],
  })],
  [4, stat('p95 latency (v1)', quantile(0.95, '', 'face_request_duration_seconds', V1), {
    unit: 's', decimals: 2,
    thresholds: [{ color: 'green', value: null }, { color: 'orange', value: 2 }, { color: 'red', value: 5 }],
  })],
  [3, stat('In flight', `sum(face_inflight{${SEL}})`, { description: 'Requests holding a processing slot (max = server.max_concurrency).' })],
  [3, stat('Queued', `sum(face_queue_depth{${SEL}})`, {
    description: 'Requests waiting for a slot. At server.queue_limit (default 32) new requests get 503 overloaded.',
    thresholds: [{ color: 'green', value: null }, { color: 'orange', value: 16 }, { color: 'red', value: 28 }],
  })],
);

// ------------------------------------------------------------------------------------- traffic
row('Traffic and errors');
line(
  8,
  [12, ts('Request rate by endpoint', [[`sum by (endpoint) (rate(face_requests_total{${SEL}, endpoint=~"$endpoint"}[${W}]))`, '{{endpoint}}']], { unit: 'reqps' })],
  [12, ts('Responses by status', [[`sum by (status) (rate(face_requests_total{${SEL}, ${V1}}[${W}]))`, '{{status}}']], {
    unit: 'reqps', stack: true,
    overrides: [
      { matcher: { id: 'byRegexp', options: '5..' }, properties: [{ id: 'color', value: { mode: 'fixed', fixedColor: 'red' } }] },
      { matcher: { id: 'byRegexp', options: '4..' }, properties: [{ id: 'color', value: { mode: 'fixed', fixedColor: 'orange' } }] },
      { matcher: { id: 'byName', options: '200' }, properties: [{ id: 'color', value: { mode: 'fixed', fixedColor: 'green' } }] },
    ],
  })],
);
line(
  8,
  [12, ts('Errors by problem code', [[withProblem(`sum by (endpoint, status) (rate(face_requests_total{${SEL}, ${V1}, status!~"2.."}[${W}]))`), '{{problem}} {{endpoint}}']], {
    unit: 'reqps', stack: true,
    description: 'The metrics carry the HTTP status, not the problem code; the status is mapped to its problem code(s) from contracts/openapi.yaml. 503 covers both overloaded (queue full / timeout) and not-ready; 422 covers image-decode-failed and image-too-large. Logs carry the exact code.',
  })],
  [12, ts('Error ratio by endpoint', [
    [`sum by (endpoint) (rate(face_requests_total{${SEL}, ${V1}, status=~"5.."}[${W}])) / clamp_min(sum by (endpoint) (rate(face_requests_total{${SEL}, ${V1}}[${W}])), 1e-9)`, '5xx {{endpoint}}'],
    [`sum by (endpoint) (rate(face_requests_total{${SEL}, ${V1}, status="503"}[${W}])) / clamp_min(sum by (endpoint) (rate(face_requests_total{${SEL}, ${V1}}[${W}])), 1e-9)`, '503 {{endpoint}}'],
    [`sum by (endpoint) (rate(face_requests_total{${SEL}, ${V1}, status=~"4.."}[${W}])) / clamp_min(sum by (endpoint) (rate(face_requests_total{${SEL}, ${V1}}[${W}])), 1e-9)`, '4xx {{endpoint}}'],
  ], { unit: 'percentunit', min: 0 })],
);

// ------------------------------------------------------------------------------------- latency
row('Latency');
line(
  9,
  [8, ts('p50 by endpoint', [[quantile(0.5, 'endpoint', 'face_request_duration_seconds', `endpoint=~"$endpoint"`), '{{endpoint}}']], { unit: 's' })],
  [8, ts('p95 by endpoint', [[quantile(0.95, 'endpoint', 'face_request_duration_seconds', `endpoint=~"$endpoint"`), '{{endpoint}}']], { unit: 's' })],
  [8, ts('p99 by endpoint', [[quantile(0.99, 'endpoint', 'face_request_duration_seconds', `endpoint=~"$endpoint"`), '{{endpoint}}']], {
    unit: 's',
    description: 'Histogram buckets: 5 ms ... 30 s. A p99 at 30 s means requests ran into server.request_timeout_ms.',
  })],
);
line(
  9,
  [12, {
    type: 'heatmap',
    title: 'Latency distribution (v1)',
    targets: [{ refId: 'A', datasource: DS, expr: `sum by (le) (increase(face_request_duration_seconds_bucket{${SEL}, ${V1}, endpoint=~"$endpoint"}[${W}]))`, format: 'heatmap', legendFormat: '{{le}}' }],
    options: { calculate: false, yAxis: { unit: 's' }, color: { scheme: 'Oranges', mode: 'scheme' }, cellGap: 1 },
  }],
  [12, ts('Mean latency by endpoint', [[`sum by (endpoint) (rate(face_request_duration_seconds_sum{${SEL}, ${V1}}[${W}])) / sum by (endpoint) (rate(face_request_duration_seconds_count{${SEL}, ${V1}}[${W}]))`, '{{endpoint}}']], { unit: 's' })],
);

// -------------------------------------------------------------------------------------- stages
row('Processing stages');
line(
  9,
  [12, ts('Stage p95', [[quantile(0.95, 'stage', 'face_stage_duration_seconds', `stage=~"$stage"`), '{{stage}}']], {
    unit: 's',
    description: 'queue = waiting for a processing slot; upload = reading the multipart body; then decode, detect, landmarks, segment, measure, document, risk, embed, evaluate, output. A rising queue p95 is the first sign of saturation.',
  })],
  [12, ts('Time spent per stage (seconds per second)', [[`sum by (stage) (rate(face_stage_duration_seconds_sum{${SEL}, stage=~"$stage"}[${W}]))`, '{{stage}}']], {
    unit: 's', stack: true,
    description: 'Busy time by stage across all requests. With parallel_stages on, stages of one verification overlap, so this can exceed wall-clock time per request.',
  })],
);

// ------------------------------------------------------------------------------------ capacity
row('Capacity');
line(
  8,
  [12, ts('In flight and queued', [
    [`sum(face_inflight{${SEL}})`, 'in flight'],
    [`sum(face_queue_depth{${SEL}})`, 'queued'],
    [`max_over_time(sum(face_queue_depth{${SEL}})[5m:])`, 'queued (5m max)'],
  ], { description: 'Gauges sampled at scrape time. in flight <= max_concurrency; queued <= queue_limit.' })],
  [12, ts('Overload rejections (503)', [[`sum by (endpoint) (rate(face_requests_total{${SEL}, ${V1}, status="503"}[${W}]))`, '{{endpoint}}']], { unit: 'reqps', stack: true })],
);

// -------------------------------------------------------------------------------------- checks
row('Checks (drift)');
line(
  9,
  [12, ts('Reject rate per check (fail / evaluated)', [[
    `(sum by (check) (rate(face_check_status_total{${SEL}, check=~"$check", status="fail"}[${W}])) or 0 * sum by (check) (rate(face_check_status_total{${SEL}, check=~"$check", status=~"pass|fail|undetermined"}[${W}]))) / clamp_min(sum by (check) (rate(face_check_status_total{${SEL}, check=~"$check", status=~"pass|fail|undetermined"}[${W}])), 1e-9)`,
    '{{check}}',
  ]], {
    unit: 'percentunit', min: 0, max: 1,
    description: 'Share of evaluated results that failed, per check. A check whose reject rate climbs while traffic is steady signals drift: capture conditions, client app change, or a model/profile change. Selfie and document results are not separated in the metric.',
  })],
  [12, ts('Reject rate vs. 7 days ago (ratio)', [[
    `(sum by (check) (rate(face_check_status_total{${SEL}, check=~"$check", status="fail"}[1h])) / clamp_min(sum by (check) (rate(face_check_status_total{${SEL}, check=~"$check", status=~"pass|fail|undetermined"}[1h])), 1e-9))
 / clamp_min(sum by (check) (rate(face_check_status_total{${SEL}, check=~"$check", status="fail"}[1h] offset 7d)) / clamp_min(sum by (check) (rate(face_check_status_total{${SEL}, check=~"$check", status=~"pass|fail|undetermined"}[1h] offset 7d)), 1e-9), 0.005)`,
    '{{check}}',
  ]], {
    unit: 'short', min: 0,
    description: 'Current hourly reject rate divided by the same hour a week ago. 1 = unchanged. The drift alert fires on this signal.',
    thresholds: [{ color: 'green', value: null }, { color: 'red', value: 2 }],
  })],
);
line(
  9,
  [12, ts('Undetermined rate per check', [[
    `(sum by (check) (rate(face_check_status_total{${SEL}, check=~"$check", status="undetermined"}[${W}])) or 0 * sum by (check) (rate(face_check_status_total{${SEL}, check=~"$check", status=~"pass|fail|undetermined"}[${W}]))) / clamp_min(sum by (check) (rate(face_check_status_total{${SEL}, check=~"$check", status=~"pass|fail|undetermined"}[${W}])), 1e-9)`,
    '{{check}}',
  ]], { unit: 'percentunit', min: 0, max: 1, description: 'A check that could not be measured (e.g. no face found upstream).' })],
  [12, {
    type: 'table',
    title: 'Check results (selected range)',
    targets: [{ refId: 'A', datasource: DS, expr: `sum by (check, status) (increase(face_check_status_total{${SEL}, check=~"$check"}[$__range]))`, format: 'table', instant: true }],
    transformations: [
      { id: 'pivot', options: {} },
      { id: 'groupingToMatrix', options: { columnField: 'status', rowField: 'check', valueField: 'Value' } },
      { id: 'sortBy', options: { sort: [{ field: 'fail', desc: true }] } },
    ],
    fieldConfig: { defaults: { unit: 'short', decimals: 0 }, overrides: [] },
    options: { showHeader: true },
  }],
);

// -------------------------------------------------------------------------------- verification
row('Verification outcomes');
line(
  8,
  [12, ts('Outcomes', [[`sum by (outcome) (rate(face_verification_outcome_total{${SEL}}[${W}]))`, '{{outcome}}']], {
    unit: 'reqps', stack: true,
    overrides: [
      { matcher: { id: 'byName', options: 'verified' }, properties: [{ id: 'color', value: { mode: 'fixed', fixedColor: 'green' } }] },
      { matcher: { id: 'byName', options: 'rejected' }, properties: [{ id: 'color', value: { mode: 'fixed', fixedColor: 'red' } }] },
      { matcher: { id: 'byName', options: 'review' }, properties: [{ id: 'color', value: { mode: 'fixed', fixedColor: 'orange' } }] },
      { matcher: { id: 'byName', options: 'retake' }, properties: [{ id: 'color', value: { mode: 'fixed', fixedColor: 'blue' } }] },
    ],
  })],
  [12, ts('Outcome share', [[`sum by (outcome) (rate(face_verification_outcome_total{${SEL}}[${W}])) / ignoring(outcome) group_left sum(rate(face_verification_outcome_total{${SEL}}[${W}]))`, '{{outcome}}']], {
    unit: 'percentunit', stack: true, min: 0, max: 1,
    description: 'A rising rejected or review share at steady traffic is a drift or abuse signal; a rising retake share points at capture quality.',
  })],
);

// ------------------------------------------------------------------------------------- process
// ------------------------------------------------------- early rejects, rate limits, reloads
row('Early rejects, rate limits and config reloads');
line(
  8,
  [8, ts('Unread request bodies (early rejects)', [[`sum by (outcome) (rate(face_unread_body_total{${SEL}}[${W}]))`, '{{outcome}}']], {
    unit: 'reqps', stack: true,
    description: 'Bodies left unread when the service answers early (401, 413, 429, 503...). drained: read and discarded, the client gets the response. not_sent: Expect: 100-continue, never uploaded. too_large / busy / timeout / error: the connection was closed with the body unread, so a client still uploading may see a connection reset instead of the response. busy (no drain slot) and timeout above zero are alerted.',
    overrides: [
      { matcher: { id: 'byName', options: 'drained' }, properties: [{ id: 'color', value: { mode: 'fixed', fixedColor: 'green' } }] },
      { matcher: { id: 'byName', options: 'not_sent' }, properties: [{ id: 'color', value: { mode: 'fixed', fixedColor: 'blue' } }] },
      { matcher: { id: 'byRegexp', options: 'busy|timeout|error' }, properties: [{ id: 'color', value: { mode: 'fixed', fixedColor: 'red' } }] },
      { matcher: { id: 'byName', options: 'too_large' }, properties: [{ id: 'color', value: { mode: 'fixed', fixedColor: 'orange' } }] },
    ],
  })],
  [8, ts('Rate limited (429) by project', [[`sum by (project, reason) (rate(face_rate_limited_total{${SEL}}[${W}]))`, '{{project}} ({{reason}})']], {
    unit: 'reqps', stack: true,
    description: "429 rate-limited answers by API key project and reason: rate (the key's request rate) or concurrency (the key's concurrent-request limit). No data until per-key limits are configured.",
  })],
  [8, ts('Config reloads', [[`sum by (result) (increase(face_config_reloads_total{${SEL}}[${W}]))`, '{{result}}']], {
    unit: 'short', stack: true,
    description: 'Hot reloads of API keys and profiles (P4: no data until reloads land). error: a reload was refused (details in the log). Any error is alerted.',
    overrides: [
      { matcher: { id: 'byName', options: 'ok' }, properties: [{ id: 'color', value: { mode: 'fixed', fixedColor: 'green' } }] },
      { matcher: { id: 'byName', options: 'error' }, properties: [{ id: 'color', value: { mode: 'fixed', fixedColor: 'red' } }] },
    ],
  })],
);

row('Process');
line(
  8,
  [8, ts('Resident memory', [
    [`process_resident_memory_bytes{${SEL}}`, 'rss {{instance}}'],
    [`windows_process_working_set_private_bytes{process="veriface"}`, 'private WS {{instance}}'],
    [`container_memory_working_set_bytes{container="veriface"}`, 'container WS {{pod}}'],
  ], {
    unit: 'bytes',
    description: 'veriface does not export process_* metrics yet (the prometheus crate is built without its "process" feature). This panel shows windows_exporter (process collector) or cAdvisor series when they exist.',
  })],
  [8, ts('CPU (cores)', [
    [`rate(process_cpu_seconds_total{${SEL}}[${W}])`, '{{instance}}'],
    [`sum by (instance) (rate(windows_process_cpu_time_total{process="veriface"}[${W}]))`, 'windows {{instance}}'],
    [`sum by (pod) (rate(container_cpu_usage_seconds_total{container="veriface"}[${W}]))`, 'container {{pod}}'],
  ], { unit: 'short' })],
  [8, text('Notes', [
    '**Metrics source:** `service/crates/face-server/src/metrics.rs`.',
    '',
    '- Error panels map HTTP status to problem codes; the exact code is in the JSON logs.',
    '- Check metrics do not separate selfie, document and verification results.',
    '- Readiness needs a blackbox probe of `/health/ready` (job selected by `$probe_job`).',
    '- Process memory/CPU need windows_exporter or cAdvisor until the service exports `process_*`.',
    '- Rate-limit and config-reload panels stay empty until those features are configured / released.',
    '- Alerts: `ops/prometheus/alerts.yml`.',
  ].join('\n'))],
);

// ----------------------------------------------------------------------------------- dashboard
const variable = (name, label, query, { multi = true, includeAll = true, allValue = '.*' } = {}) => ({
  name,
  label,
  type: 'query',
  datasource: DS,
  query: { query, refId: `${name}-var` },
  definition: query,
  refresh: 2,
  multi,
  includeAll,
  allValue,
  current: { selected: true, text: ['All'], value: ['$__all'] },
  sort: 1,
});

const dashboard = {
  uid: 'veriface',
  title: 'Veriface',
  description: 'Veriface service: traffic, errors, latency, stages, capacity, check drift, verification outcomes.',
  tags: ['veriface', 'face'],
  timezone: 'browser',
  editable: true,
  graphTooltip: 1,
  refresh: '30s',
  schemaVersion: 39,
  time: { from: 'now-6h', to: 'now' },
  templating: {
    list: [
      { name: 'datasource', label: 'Data source', type: 'datasource', query: 'prometheus', current: {}, hide: 0 },
      variable('job', 'Job', 'label_values(face_requests_total, job)'),
      variable('instance', 'Instance', 'label_values(face_requests_total{job=~"$job"}, instance)'),
      variable('endpoint', 'Endpoint', 'label_values(face_requests_total{job=~"$job"}, endpoint)', { allValue: '/v1/.*' }),
      variable('stage', 'Stage', 'label_values(face_stage_duration_seconds_count{job=~"$job"}, stage)'),
      variable('check', 'Check', 'label_values(face_check_status_total{job=~"$job"}, check)'),
      { name: 'probe_job', label: 'Readiness probe job', type: 'custom', query: 'veriface-ready', current: { text: 'veriface-ready', value: 'veriface-ready' }, options: [], hide: 0 },
    ],
  },
  annotations: { list: [{ builtIn: 1, datasource: { type: 'grafana', uid: '-- Grafana --' }, enable: true, hide: true, iconColor: 'rgba(0, 211, 255, 1)', name: 'Annotations & Alerts', type: 'dashboard' }] },
  panels,
};

const out = join(dirname(fileURLToPath(import.meta.url)), 'veriface.json');
writeFileSync(out, `${JSON.stringify(dashboard, null, 2)}\n`);
console.log(`wrote ${out} (${panels.length} panels)`);

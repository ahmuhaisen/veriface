#!/usr/bin/env node
// Runs every query of ops/grafana/veriface.json against a Prometheus server and reports parse
// errors and empty results. Dashboard variables are replaced with "match everything" values.
//
//   node ops/grafana/check-queries.mjs [http://127.0.0.1:9090]
//
// Exit 1 when a query fails to parse/evaluate. Empty results are listed but are not failures
// (e.g. process or probe series that a given deployment does not have).

import { readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const prom = (process.argv[2] ?? 'http://127.0.0.1:9090').replace(/\/+$/, '');
const dash = JSON.parse(readFileSync(join(dirname(fileURLToPath(import.meta.url)), 'veriface.json'), 'utf8'));

const vars = {
  $__rate_interval: '1m',
  $__range: '10m',
  $job: '.*',
  $instance: '.*',
  $endpoint: '/v1/.*',
  $stage: '.*',
  $check: '.*',
  $probe_job: 'veriface-ready',
};
const subst = (expr) => Object.entries(vars).reduce((e, [k, v]) => e.split(k).join(v), expr);

let failed = 0;
for (const panel of dash.panels) {
  for (const t of panel.targets ?? []) {
    const expr = subst(t.expr);
    const res = await fetch(`${prom}/api/v1/query?${new URLSearchParams({ query: expr })}`);
    const body = await res.json();
    let line;
    if (body.status !== 'success') {
      failed++;
      line = `ERROR  ${body.error}`;
    } else {
      const n = body.data.result.length;
      line = n ? `ok     ${n} series` : 'empty  0 series';
    }
    console.log(`${line.padEnd(28)} ${panel.title} [${t.refId}]`);
  }
}
console.log(failed ? `${failed} queries failed` : 'all queries parsed and evaluated');
process.exitCode = failed ? 1 : 0;

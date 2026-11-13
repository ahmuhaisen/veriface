#!/usr/bin/env node
// Reproduces: the service answers some requests before reading their body (401 unauthorized,
// 413 payload-too-large, 503 overloaded) and then closes the connection without draining the
// rest of the upload. A client still sending the body gets a TCP reset and never sees the
// response (Node fetch/undici: ECONNABORTED or ECONNRESET; curl without `Expect: 100-continue`:
// usually the response, sometimes exit 56 under load).
//
//   node ops/loadtest/early-reject-repro.mjs [--url http://127.0.0.1:18100] [--mib 12] [--tries 5]
//
// Uses a wrong API key, so no key is needed. Expected: 401 every time. Exit 1 when any attempt
// fails at the transport level.

const args = Object.fromEntries(
  process.argv.slice(2).reduce((acc, a, i, all) => (a.startsWith('--') ? [...acc, [a.slice(2), all[i + 1]]] : acc), []),
);
const url = (args.url ?? 'http://127.0.0.1:18100').replace(/\/+$/, '');
const mib = Number(args.mib ?? 12);
const tries = Number(args.tries ?? 5);

const body = new Uint8Array(Math.round(mib * 1024 * 1024));
body.set([0xff, 0xd8, 0xff]);
let failures = 0;
for (let i = 0; i < tries; i++) {
  const form = new FormData();
  form.append('image', new Blob([body], { type: 'image/jpeg' }), 'big.jpg');
  try {
    const res = await fetch(`${url}/v1/quality`, { method: 'POST', headers: { 'X-API-Key': 'not-a-valid-key' }, body: form });
    await res.text();
    console.log(`attempt ${i + 1}: HTTP ${res.status}`);
  } catch (e) {
    failures++;
    console.log(`attempt ${i + 1}: transport error ${e?.cause?.code ?? e?.message} (the 401 response was lost)`);
  }
}
console.log(`${failures}/${tries} attempts lost the response`);
process.exitCode = failures ? 1 : 0;

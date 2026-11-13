# Load, soak and overload results

Measured 2 October 2026 by the reliability agent. Raw output: `results/*.json` (one file per run,
with memory samples) and `results/soak-30m-c4.log`.

## Setup

| | |
|---|---|
| Machine | AMD Ryzen 7 250 (8 cores, 16 logical), 29.7 GB RAM, Windows 11 Pro 26200 |
| Shared with | the user's own service on :8080 (idle during the runs) and other agent sessions. Runs that overlapped my own builds were discarded and rerun. |
| Binary | `service/target-lead/release/veriface.exe` (built 2 Oct 16:32), my own instance on `127.0.0.1:18100`. The overload rerun used a copy of `service/target-live/release/veriface.exe` on `:18101`. |
| Config | `tools/console/.dev/config.toml` with `bind` changed and JSON logging at `warn`: `max_concurrency = 0` (16), `queue_limit = 32`, `request_timeout_ms = 60000`, engine defaults (`sessions_per_model` 0 -> 4, `parallel_stages` on, AuraFace and segmenter at 2 threads) |
| Profile | `fixtures` (dev only: `standard` with `INTER_EYE_DISTANCE` min 35). Under `standard`, the small document portraits in `testdata/` fail `INTER_EYE_DISTANCE`, so every verification ends early as `retake` without an embedding, which would understate the cost of a verification. |
| Client | `loadtest.mjs`, Node 26.7, same machine. Mix 2 quality : 1 document : 1 verification. Half of the verifications pair a document with the same person's selfie. 29 selfies and 14 documents from `testdata/`; no `output=true`. |

Latency is client-side (request sent to body received), in milliseconds.

## Load: concurrency 1, 4 and 16 (120 s each)

| Concurrency | Requests | Throughput (req/s) | p50 | p90 | p95 | p99 | max | Errors |
|---|---|---|---|---|---|---|---|---|
| 1 | 357 | 2.97 | 265 | 615 | 890 | 1199 | 1397 | 0 |
| 4 | 911 | 7.58 | 367 | 1002 | 1059 | 1413 | 1557 | 0 |
| 16 | 1487 | 12.29 | 1150 | 2072 | 2407 | 3457 | 4491 | 0 |

Per endpoint (p50 / p95 / p99 in ms, throughput in req/s):

| Concurrency | `/v1/quality` | `/v1/document-portraits` | `/v1/verifications` |
|---|---|---|---|
| 1 | 1.57: 173 / 295 / 310 | 0.62: 268 / 512 / 851 | 0.77: 471 / 1045 / 1397 |
| 4 | 3.75: 325 / 373 / 413 | 1.84: 535 / 1004 / 1078 | 1.99: 961 / 1410 / 1525 |
| 16 | 6.12: 978 / 1348 / 1489 | 2.92: 1235 / 2797 / 3462 | 3.24: 1864 / 3125 / 4221 |

Every response was a `200`. Outcomes at concurrency 16: quality 671 accepted / 70 rejected;
documents 248 accepted / 106 rejected; verifications 21 verified, 134 review (mostly
`PROBE_REFERENCE_DUPLICATE`: the fixture documents reuse the selfie photo), 137 rejected, 100
retake.

Throughput rises from 1 to 4 to 16 (2.97 -> 7.58 -> 12.29 req/s), and from 4 to 16 latency grows
about 3x. That fits a service that is CPU-bound well before 16 concurrent requests on 8 physical
cores: parallel stages and the 2-thread models already use more than one core per request.
The service's resident memory went from 1.21 GiB idle to 1.42 GiB under concurrency 16 and
back to 1.24 GiB within a minute of the run ending.

## Overload: concurrency 64 (60 s)

64 clients against 16 slots + 32 queue places, with no client back-off. The client does not
honour `Retry-After`; a worker pauses 100 ms after a transport error only.

| Build | HTTP 200 | HTTP 503 `overloaded` | Transport errors (response lost) | OK throughput | p50 / p99 of all responses |
|---|---|---|---|---|---|
| `target-lead` (16:32) | 719 | 144 | **6,058** (5,976 `ECONNABORTED`, 82 `ECONNRESET`) | 11.1 req/s | 3961 / 6890 ms |
| `target-live` (with body drain) | 772 | 20,377 | **0** | 12.2 req/s | 40 / 4297 ms |

On the older build, most 503s were lost: the service answered before reading the upload and then
closed the connection, so a client still sending its body got a reset instead of the 503. See
"Bug found" below. On the build with the unread-body drain, every 503 arrived (fast, about 40 ms),
`face_unread_body_total{outcome="drained"}` counted 20,388, and goodput stayed at full capacity.
No 500s on either build.

## Soak: concurrency 4, 30 minutes

`--soak --duration 1800 --warmup 300 --sample-interval 30`, `target-lead` build, process
working set and private bytes sampled through `Get-Process`.

| | |
|---|---|
| Requests | 14,439 (8.02 req/s): 7,182 quality, 3,596 document, 3,661 verification |
| Errors | **0** (all `200`) |
| Latency, all | p50 365, p90 995, p95 1104, p99 1568, max 3541 ms |
| Latency per endpoint (p50 / p95 / p99) | quality 294 / 426 / 584; document 463 / 927 / 1222; verification 860 / 1501 / 1949 |
| Working set | 61 samples, 1,164 to 1,341 MiB throughout. After warm-up: baseline (first 3 samples) 1,326 MiB, end (last 3) 1,225 MiB, **-7.6%**, least-squares slope -3.1 MiB/min |
| Private bytes | 1,315 -> 1,260 MiB (**-4.2%**), slope -1.1 MiB/min |
| Threads / handles | 21-24 / 213-217, flat |
| Leak verdict | **No growth** (limit 10%). Memory oscillates by about 175 MiB with the request mix and returns to the same band. |

Run: 2 Oct 2026, 17:16-17:46 local (UTC+3).

## Bug found: early responses lost for clients still uploading (fixed in `target-live`)

**Symptom (`target-lead` build).** When the service answers before reading the request body
(`401 unauthorized`, `413 payload-too-large`, `503 overloaded`), it closes the connection with the
body unread. A client still uploading gets a TCP reset and never sees the response, so it cannot
honour `Retry-After` and logs a network error instead of the problem code.

**Reproduction.**

```bash
node ops/loadtest/early-reject-repro.mjs --url http://127.0.0.1:18100            # 12 MiB, wrong key
node ops/loadtest/early-reject-repro.mjs --url http://127.0.0.1:18100 --mib 1
```

- `target-lead`: 5/5 (12 MiB) and 3/3 (1 MiB) attempts lost the 401 (Node fetch: `ECONNABORTED`).
- Under overload, curl without `Expect: 100-continue` also failed (exit 56) on 2 of 6 attempts,
  with the upload cut off at about 193 KB of about 624 KB.
- curl hides the problem for bodies over 1 MiB, because it then sends
  `Expect: 100-continue` and never uploads.

**After the fix (`target-live`).** 5/5 and 3/3 delivered as `401`, counted as
`face_unread_body_total{outcome="drained"}`. The overload run above lost 0 responses.
`VerifaceUnreadBodiesDropped` alerts when drains fail (`busy`, `timeout`).

## Not run

- No load test with `output=true` (output image encoding).
- No WebP or PNG uploads in the load mix (`testdata/` has JPEG only); the fuzzers cover those
  decoders.
- No run on Linux or in a container.
- No soak on the `target-live` build. Its overload behaviour was measured, its memory over time
  was not.

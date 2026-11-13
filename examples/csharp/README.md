# .NET 10 example client

A minimal console client for Veriface. It sends a selfie and an ID-document photo to
`POST /v1/verifications` with `HttpClient` and `MultipartFormDataContent` (no third-party
packages), then maps the result to an action. Read the [integration guide](../../docs/integration-guide.md)
first.

## Run

```powershell
$env:VERIFACE_API_KEY = "<key from the service operator>"
$env:VERIFACE_URL = "http://127.0.0.1:8080"   # optional; this is the default
dotnet run -- selfie.jpg id-card.jpg             # profile defaults to "standard"
dotnet run -- selfie.jpg id-card.jpg icao-strict
```

Output:

```text
requestId:   4fef2654-8db4-48d1-a951-b4fefe357b64
outcome:     retake (decision undetermined)
calibration: provisional-2026-10-01 (provisional)
profile:     standard 0.1.0
reasons:     reference:INTER_EYE_DISTANCE, reference:SHARPNESS
action:      RetakeDocument
```

Exit codes: `0` a result was received (whatever the outcome), `1` service error or unreachable,
`2` usage or missing API key.

## What it shows

| Topic | Where | Notes |
|---|---|---|
| API key from the environment | `Program.Main` | `VERIFACE_API_KEY`. Never hard-code the key, ship it to a browser, or log it |
| Multipart upload | `FaceClient.VerifyAsync` | Parts `probe` and `reference`, `image/jpeg`. The service detects the format from the bytes |
| `Expect: 100-continue` | `FaceClient.VerifyAsync` | Lets the service refuse a request (401, 404, 503, ...) before the images are sent. Without it, .NET reports an early refusal as a broken upload |
| Request id | `FaceClient.VerifyAsync` | One `X-Request-Id` for all attempts of a call, so the service log ties retries together |
| Retries | `FaceClient.VerifyAsync` | Only `503` (`overloaded`, `not-ready`), honouring `Retry-After` plus jitter, at most 3 attempts. The multipart body is rebuilt per attempt |
| Errors | `FaceServiceException` | RFC 9457 problem: branch on `Code`, quote `RequestId` |
| Outcome to action | `OutcomeMapper.ToAction` | `verified` → accept; `retake` → retake the selfie or the document, from the first reason's `source`; `review` → human review; `rejected` → reject (limited retry, then review). Unknown outcomes fail safe to review |
| Tolerant parsing | records at the end | Only the fields used are declared, so new optional response fields do not break the client |
| Timeout | `Program.Main` | 35 s, slightly above the service's default `request_timeout_ms` (30 s) |

The score is not printed on purpose: never show it to end users, and keep it out of general
application logs.

## In a real backend

- Register `HttpClient` through `IHttpClientFactory` (or one shared instance) instead of creating
  one per call.
- Take the images from your upload endpoint, validate their size first, and do not write them to
  disk unless your retention policy says so.
- Store the request id, outcome, decision, reasons, `profile.name`/`version`/`sha256` and
  `calibration.id` with your business record. Not the images, not the score in general logs.
- Localise `reasons` with the starter tables in the integration guide, section 7.

## Build

```powershell
dotnet build
```

Built on 2026-10-02 with the .NET SDK 10.0.401: 0 warnings, 0 errors (`TreatWarningsAsErrors` is
on). It was run against a local development build of the service on the repository's
public-domain fixtures: `verified`, `retake` (reference), `404 profile-not-found` and
`401 unauthorized` were all handled as described.

# ADR 0001: Rust for the service, Python for the lab, TypeScript for the browser SDK

- Status: accepted (2026-10-01)

## Context

The product must be lightweight, fast, easy to deploy on air-gapped Windows Server and on Linux, and safe when decoding untrusted image uploads that come from public web portals.

Model execution time is the same in every host language: ONNX Runtime runs the same C++ engine, and its maintainers describe binding overhead as negligible.

## Decision

- **Service: Rust.**
  - Web framework: axum.
  - Model runtime: the `ort` crate with `load-dynamic`, loading Microsoft's official ONNX Runtime library.
  - Image decoding: pure-Rust decoders (PNG/WebP unsafe-free; JPEG uses SIMD, see the correction below).
- **Lab: Python 3.12.** Internal only; never shipped.
- **Browser SDK: TypeScript**, using MediaPipe Tasks Vision for web.

## Why

1. **Deployment.** One executable that registers itself as a Windows service, and the same code builds a small Linux container. Nothing to install on the server: no interpreter, no package bundle, no obfuscation layer.
2. **Untrusted input.** The `png` and `image-webp` crates forbid unsafe code. `zune-jpeg` uses unsafe SIMD paths in our build (see the correction below). The `image` crate has had two advisories ever. The native codecs the other stacks wrap have recent critical CVEs:
   - libwebp CVE-2023-4863
   - OpenCV CVE-2025-53644 (CVSS 9.8, reachable through `imdecode`)
   - several libpng CVEs in 2025–26
3. **Footprint.** No GC and no JIT warm-up, which gives the smallest memory use and the fastest cold start.

## Correction (2026-10-02): JPEG decoding is not unsafe-free

`image` 0.25 pulls in `zune-jpeg` with its default SIMD features (x86, neon), which use `unsafe`. Cargo features are additive, so they can't be switched off through `image`. Measured on a 12 MP JPEG: about 50 ms with SIMD vs about 90 ms scalar (safe).

**Decision: keep SIMD.** The ~40 ms per image matters more than the marginal gain over a fuzzed, widely used decoder. PNG and WebP decoding remain `forbid(unsafe_code)`.

Mitigations:
- Size and pixel limits are enforced before decode.
- The decoder is fuzzed (`service/fuzz/`).

Revisit option: decode with `zune-jpeg` (default-features off) plus a pure-Rust encoder, if a deployment requires fully memory-safe decoding.

## Consequences and mitigations

- **The `ort` binding is a community project that is still a release candidate** (2.0.0-rc.13). Pin the exact version and load Microsoft's official DLL, so the inference engine itself is the official build.
- **The team must learn Rust.** The service is small (estimated 3–5k lines), so the cost is bounded.
- **Windows builds need the Visual Studio C++ Build Tools** (MSVC linker and Windows SDK).
- **Rejected alternatives:**
  - .NET 10: official ORT binding and LTS to Nov 2028, but image decoding would rely either on native codecs or on ImageSharp (paid licence, 7 advisories in 2024–25).
  - Python in production: heaviest deployment, and Python 3.10 reaches end-of-life in Oct 2026.

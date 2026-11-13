# service/fuzz: mutation fuzzing of the untrusted-input parsers

A standalone crate with its own `[workspace]`: it is **not** a member of `service/Cargo.toml` and
builds on the pinned stable toolchain (cargo-fuzz needs nightly and libFuzzer, which do not run on
Windows MSVC here). No third-party dependencies beyond the service's own crates.

It is a random-mutation fuzzer with "novel outcome" feedback (an input whose result signature
was not seen before joins the corpus). It is **not coverage-guided**; findings are as good as the
mutators and seeds.

| Target | Calls | Invariants checked on success |
|---|---|---|
| `decode` | `face_imaging::codec::decode` with the `standard` profile limits (10 MiB, 40 MP) and with tight limits (64 KiB, 2000 px, 1 MiB alloc) | non-empty image; `w*h <= max_pixels`; buffer size `w*h*3`; format = sniffed format and allowed; quality in [0, 100], PNG = 100; tight limits never accept what production limits rejected as corrupt/unsupported |
| `quality` | `decode`, header-focused mutation of JPEG DQT/SOF segments and the WebP VP8/VP8L/VP8X headers, to drive the private JPEG and WebP quality estimators | as above |
| `profile` | `face_core::Profile::from_yaml` with the embedded catalogue | accepted profiles: enabled checks have finite bounds and min <= max, operating points in range, default operating point exists, input limits valid |

The quality estimators (`JpegHeader::estimate_quality`, `webp_quality`) are private, so they are
reached through `decode`.

Every input runs on a worker thread under `catch_unwind` (a panic is a finding), a watchdog
(`--hang-ms`, default 10 s: a hang is a finding and stops the run) and an allocation tracker
(a peak above `--max-alloc-mib`, default 1024, is a finding). The input being run is written to
`out/current-<target>.bin` first, so an abort (stack overflow, out of memory) leaves its
reproducer.

## Run

```bash
cd service/fuzz
export CARGO_TARGET_DIR=../target-fuzz        # any service/target*/ directory is git-ignored
cargo build --release
$CARGO_TARGET_DIR/release/veriface-fuzz run decode  --seconds 600
$CARGO_TARGET_DIR/release/veriface-fuzz run quality --seconds 600
$CARGO_TARGET_DIR/release/veriface-fuzz run profile --seconds 600
$CARGO_TARGET_DIR/release/veriface-fuzz selftest              # hand-written edge cases
$CARGO_TARGET_DIR/release/veriface-fuzz repro decode crashes/<file>
```

Options: `--seed N` (runs are reproducible for a given seed and corpus), `--hang-ms`,
`--max-alloc-mib`, `--out DIR` (default `out/`, git-ignored: evolved corpus, stats JSON),
`--crashes DIR` (default `crashes/`), `--repo DIR`.

Findings are saved as `crashes/<target>-<kind>-<hash>.bin` (one per distinct message) and listed
in `out/stats-<target>.json`. Exit code: 0 clean, 1 findings, 3 hang.

## Seeds

- `seeds/image/`: small **synthetic** images (gradients and seeded noise, no faces) in JPEG
  (baseline, progressive, 4:4:4, grayscale, CMYK, restart markers, EXIF orientation), PNG (RGB,
  RGBA, gray, gray16, palette, 1-bit) and WebP (lossy, lossless, alpha, animated). Regenerate with
  `lab/.venv/Scripts/python service/fuzz/make_seeds.py` (Pillow).
- `seeds/profile/`: copies of the shipped `profiles/*.yaml`.
- At run time: the public-domain `testdata/` JPEGs, plus in-memory PNG/JPEG re-encodings of small
  crops of them.

## Results (2 October 2026)

Ryzen 7 250, Windows 11, release build (debug assertions and overflow checks on), seed
`20261002`, production limits as above, three targets in parallel, one core each.

| Target | Duration | Execs | Execs/s | Outcomes (ok / rejected) | Max peak alloc | Slowest input | Findings |
|---|---|---|---|---|---|---|---|
| `decode` | 720 s | 381,646 | 530 | 6,344 / 375,302 | 118 MiB | 241 ms | **none** |
| `quality` | 720 s | 363,466 | 505 | 5,382 / 358,084 | 148 MiB | 91 ms | **none** |
| `profile` | 720 s | 208,371 | 289 | 52 / 208,319 | 0.2 MiB | 5.9 ms | **none** |
| `selftest` | 14 cases | | | all rejected cleanly, each < 2 ms | 0.1 MiB | | **none** |

No panic, invariant violation, hang (10 s watchdog) or allocation peak above 1 GiB. `crashes/`
stayed empty.

Notes, stated honestly:

- The first `quality` run died after a few seconds from a bug in the **harness** (the WebP
  mutator indexed past a 20-byte input), not in the service. That was fixed and the run repeated
  for the full 720 s (the row above). `decode` and `profile` ran with the harness before that fix;
  the bug only affects that mutator on 20-byte WebP inputs.
- The corpus hit its 4,096-entry cap within minutes: decoder error messages vary more than the
  digit-stripping signature expects, so "novel outcome" feedback saturates quickly. Later
  mutations draw from that capped corpus.
- `profile`: few mutated profiles stay valid (52 of 208k), so the validation logic past parsing
  is exercised less than the parser. Profiles are operator-supplied files, not request input.
- The selftest's alias-expansion ("billion laughs") and 100,000-deep nesting inputs are rejected
  at the first mistyped field: the typed, `deny_unknown_fields` deserialization stops before any
  expansion or deep recursion.

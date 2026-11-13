//! Stable-toolchain mutation fuzzer for Veriface's untrusted-input parsers.
//!
//! cargo-fuzz needs nightly and libFuzzer; this harness runs on the pinned stable toolchain on
//! Windows MSVC and Linux. It is a random-mutation fuzzer with "novel outcome" feedback (an input
//! whose result signature was never seen joins the corpus), not coverage-guided.
//!
//! Each input runs on a worker thread with:
//! - `catch_unwind`: a panic is a finding;
//! - a watchdog: no result within `--hang-ms` is a finding (the input is saved, the run stops);
//! - allocation tracking: a peak above `--max-alloc-mib` while handling one input is a finding;
//! - invariants on the result (see `targets.rs`): a violation is a finding.
//!
//! The current input is written to `<out>/current-<target>.bin` before it runs, so an abort (stack
//! overflow, out of memory) still leaves the reproducer behind.

mod alloc;
mod mutate;
mod targets;

use std::collections::HashSet;
use std::panic::{self, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use targets::{Outcome, Target};

#[global_allocator]
static GLOBAL: alloc::Tracking = alloc::Tracking;

const USAGE: &str = "\
usage:
  veriface-fuzz run <target> [--seconds N] [--seed N] [--hang-ms N] [--max-alloc-mib N]
                                [--out DIR] [--crashes DIR] [--repo DIR]
  veriface-fuzz repro <target> <file>...
  veriface-fuzz selftest

targets: decode, quality, profile
";

struct Opts {
    seconds: u64,
    seed: u64,
    hang_ms: u64,
    max_alloc_mib: usize,
    out: PathBuf,
    crashes: PathBuf,
    repo: PathBuf,
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let here = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let mut opts = Opts {
        seconds: 600,
        seed: 0x00C0_FFEE,
        hang_ms: 10_000,
        max_alloc_mib: 1024,
        out: here.join("out"),
        crashes: here.join("crashes"),
        repo: here.join("..").join(".."),
    };
    let quiet_hook = install_panic_hook();
    match args.first().map(String::as_str) {
        Some("run") if args.len() >= 2 => {
            let target = parse_target(&args[1]);
            let mut it = args[2..].iter();
            while let Some(flag) = it.next() {
                let value = it.next().unwrap_or_else(|| usage());
                match flag.as_str() {
                    "--seconds" => opts.seconds = value.parse().unwrap_or_else(|_| usage()),
                    "--seed" => opts.seed = value.parse().unwrap_or_else(|_| usage()),
                    "--hang-ms" => opts.hang_ms = value.parse().unwrap_or_else(|_| usage()),
                    "--max-alloc-mib" => {
                        opts.max_alloc_mib = value.parse().unwrap_or_else(|_| usage())
                    }
                    "--out" => opts.out = value.into(),
                    "--crashes" => opts.crashes = value.into(),
                    "--repo" => opts.repo = value.into(),
                    _ => usage(),
                }
            }
            std::process::exit(run(target, &opts, quiet_hook));
        }
        Some("repro") if args.len() >= 3 => {
            let target = parse_target(&args[1]);
            let mut failed = false;
            for file in &args[2..] {
                let data = std::fs::read(file).unwrap_or_else(|e| fail(&format!("{file}: {e}")));
                let start = Instant::now();
                let base = alloc::reset();
                let caught = panic::catch_unwind(AssertUnwindSafe(|| target.exec(&data)));
                let (peak, largest) = alloc::window(base);
                let elapsed = start.elapsed();
                let msg = quiet_hook.lock().unwrap().take();
                let verdict = match &caught {
                    Ok(o) => format!("{o:?}"),
                    Err(_) => {
                        failed = true;
                        format!("PANIC: {}", msg.unwrap_or_default())
                    }
                };
                if let Ok(Outcome::Violation(_)) = &caught {
                    failed = true;
                }
                println!(
                    "{file}: {verdict}\n  {} bytes, {:.1} ms, peak {:.1} MiB, largest allocation {:.1} MiB",
                    data.len(),
                    elapsed.as_secs_f64() * 1e3,
                    peak as f64 / 1048576.0,
                    largest as f64 / 1048576.0
                );
            }
            std::process::exit(i32::from(failed));
        }
        Some("selftest") => std::process::exit(targets::selftest(&quiet_hook)),
        _ => usage(),
    }
}

fn usage() -> ! {
    eprint!("{USAGE}");
    std::process::exit(2);
}

fn fail(msg: &str) -> ! {
    eprintln!("veriface-fuzz: {msg}");
    std::process::exit(2);
}

fn parse_target(name: &str) -> Target {
    Target::from_name(name).unwrap_or_else(|| usage())
}

/// Panics are expected findings: keep their message for the report instead of printing it.
fn install_panic_hook() -> Arc<Mutex<Option<String>>> {
    let slot = Arc::new(Mutex::new(None));
    let s = slot.clone();
    panic::set_hook(Box::new(move |info| {
        let location = info
            .location()
            .map(|l| format!(" at {}:{}", l.file(), l.line()))
            .unwrap_or_default();
        let payload = info
            .payload()
            .downcast_ref::<&str>()
            .map(|s| (*s).to_owned())
            .or_else(|| info.payload().downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "<non-string panic>".to_owned());
        // Only panics inside the targets are findings; anything else is a harness bug: print it.
        if std::thread::current().name() == Some("main") {
            eprintln!("harness panic: {payload}{location}");
        }
        if let Ok(mut g) = s.lock() {
            *g = Some(format!("{payload}{location}"));
        }
    }));
    slot
}

struct Finding {
    kind: &'static str,
    detail: String,
    file: PathBuf,
}

fn run(target: Target, opts: &Opts, panic_msg: Arc<Mutex<Option<String>>>) -> i32 {
    std::fs::create_dir_all(&opts.out)
        .unwrap_or_else(|e| fail(&format!("{}: {e}", opts.out.display())));
    std::fs::create_dir_all(&opts.crashes)
        .unwrap_or_else(|e| fail(&format!("{}: {e}", opts.crashes.display())));
    let mut corpus = target.seeds(&opts.repo);
    if corpus.is_empty() {
        fail("no seed inputs found");
    }
    let seed_count = corpus.len();
    let current = opts.out.join(format!("current-{}.bin", target.name()));

    // Worker thread: runs one input at a time and reports the outcome.
    let (to_worker, inputs) = mpsc::channel::<Arc<Vec<u8>>>();
    let (to_main, results) = mpsc::channel();
    let msg_slot = panic_msg.clone();
    std::thread::Builder::new()
        .name("fuzz-worker".into())
        .spawn(move || {
            for data in inputs {
                let start = Instant::now();
                let base = alloc::reset();
                let caught = panic::catch_unwind(AssertUnwindSafe(|| target.exec(&data)));
                let (peak, largest) = alloc::window(base);
                let msg = msg_slot.lock().ok().and_then(|mut g| g.take());
                let outcome = caught.unwrap_or_else(|_| Outcome::Panic(msg.unwrap_or_default()));
                if to_main
                    .send((outcome, start.elapsed(), peak, largest))
                    .is_err()
                {
                    return;
                }
            }
        })
        .expect("spawn worker");

    let mut rng = mutate::Rng::new(opts.seed);
    let mut signatures: HashSet<String> = HashSet::new();
    let mut findings: Vec<Finding> = Vec::new();
    let mut finding_sigs: HashSet<String> = HashSet::new();
    let (mut execs, mut max_peak, mut max_largest, mut slowest) =
        (0u64, 0usize, 0usize, Duration::ZERO);
    let mut outcome_counts: std::collections::BTreeMap<String, u64> = Default::default();
    let started = Instant::now();
    let deadline = started + Duration::from_secs(opts.seconds);
    let mut next_report = started + Duration::from_secs(30);
    let mut hung = false;

    eprintln!(
        "fuzz {}: {} seeds, {} s, seed {}, hang {} ms, alloc limit {} MiB",
        target.name(),
        seed_count,
        opts.seconds,
        opts.seed,
        opts.hang_ms,
        opts.max_alloc_mib
    );

    while Instant::now() < deadline {
        // The first pass runs every seed unchanged.
        let data = if (execs as usize) < seed_count {
            corpus[execs as usize].clone()
        } else {
            let base = rng.pick(&corpus).clone();
            target.mutate(&mut rng, base, &corpus)
        };
        let _ = std::fs::write(&current, &data);
        let data = Arc::new(data);
        to_worker.send(data.clone()).expect("worker alive");
        let (outcome, elapsed, peak, largest) =
            match results.recv_timeout(Duration::from_millis(opts.hang_ms)) {
                Ok(r) => r,
                Err(_) => {
                    let file = save(&opts.crashes, target.name(), "hang", &data);
                    findings.push(Finding {
                        kind: "hang",
                        detail: format!("no result after {} ms", opts.hang_ms),
                        file,
                    });
                    hung = true;
                    break;
                }
            };
        execs += 1;
        max_peak = max_peak.max(peak);
        max_largest = max_largest.max(largest);
        slowest = slowest.max(elapsed);

        let mut finding = match &outcome {
            Outcome::Panic(m) => Some(("panic", m.clone())),
            Outcome::Violation(m) => Some(("invariant", m.clone())),
            _ => None,
        };
        if finding.is_none() && peak > opts.max_alloc_mib << 20 {
            finding = Some((
                "oversized-allocation",
                format!(
                    "peak {} MiB, largest single {} MiB ({:?})",
                    peak >> 20,
                    largest >> 20,
                    outcome
                ),
            ));
        }
        if let Some((kind, detail)) = finding {
            // One reproducer per distinct message (digits stripped).
            let sig = format!("{kind}:{}", strip_digits(&detail));
            if finding_sigs.insert(sig) {
                let file = save(&opts.crashes, target.name(), kind, &data);
                eprintln!("FINDING {kind}: {detail}\n  saved {}", file.display());
                findings.push(Finding { kind, detail, file });
            }
        }

        let sig = outcome.signature();
        *outcome_counts
            .entry(outcome.class().to_owned())
            .or_default() += 1;
        if signatures.insert(sig) && data.len() <= 1 << 20 && corpus.len() < 4096 {
            corpus.push((*data).clone());
        }

        if Instant::now() >= next_report {
            next_report += Duration::from_secs(30);
            let secs = started.elapsed().as_secs_f64();
            eprintln!(
                "  {:>5.0}s execs {} ({:.0}/s) corpus {} signatures {} findings {} max peak {:.0} MiB",
                secs,
                execs,
                execs as f64 / secs,
                corpus.len(),
                signatures.len(),
                findings.len(),
                max_peak as f64 / 1048576.0
            );
        }
    }

    let secs = started.elapsed().as_secs_f64();
    let summary = format!(
        "{{\n  \"target\": \"{}\",\n  \"seconds\": {:.1},\n  \"execs\": {},\n  \"execsPerSecond\": {:.1},\n  \"seeds\": {},\n  \"corpus\": {},\n  \"signatures\": {},\n  \"outcomes\": {{{}}},\n  \"maxPeakMiB\": {:.1},\n  \"maxSingleAllocMiB\": {:.1},\n  \"slowestMs\": {:.1},\n  \"hang\": {},\n  \"findings\": [{}]\n}}\n",
        target.name(),
        secs,
        execs,
        execs as f64 / secs,
        seed_count,
        corpus.len(),
        signatures.len(),
        outcome_counts
            .iter()
            .map(|(k, v)| format!("\"{k}\": {v}"))
            .collect::<Vec<_>>()
            .join(", "),
        max_peak as f64 / 1048576.0,
        max_largest as f64 / 1048576.0,
        slowest.as_secs_f64() * 1e3,
        hung,
        findings
            .iter()
            .map(|f| format!(
                "{{\"kind\": \"{}\", \"detail\": {:?}, \"file\": {:?}}}",
                f.kind,
                f.detail,
                f.file.display().to_string()
            ))
            .collect::<Vec<_>>()
            .join(", ")
    );
    let _ = std::fs::write(
        opts.out.join(format!("stats-{}.json", target.name())),
        &summary,
    );
    print!("{summary}");
    // Save the evolved corpus (outside git) for later runs to inspect.
    let cdir = opts.out.join(format!("corpus-{}", target.name()));
    let _ = std::fs::create_dir_all(&cdir);
    for (i, c) in corpus.iter().enumerate().skip(seed_count) {
        let _ = std::fs::write(cdir.join(format!("{i:05}.bin")), c);
    }
    if hung {
        // The worker is stuck inside the target; it cannot be stopped from here.
        std::process::exit(3);
    }
    i32::from(!findings.is_empty())
}

fn save(dir: &Path, target: &str, kind: &str, data: &[u8]) -> PathBuf {
    // FNV-1a for a stable, content-derived name.
    let hash = data.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, &b| {
        (h ^ u64::from(b)).wrapping_mul(0x100_0000_01b3)
    });
    let file = dir.join(format!("{target}-{kind}-{hash:016x}.bin"));
    let _ = std::fs::write(&file, data);
    file
}

fn strip_digits(s: &str) -> String {
    s.chars().filter(|c| !c.is_ascii_digit()).collect()
}

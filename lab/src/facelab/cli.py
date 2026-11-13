"""``python -m facelab <group> <command>``."""

from __future__ import annotations

import argparse


def _models_fetch(args: argparse.Namespace) -> int:
    from facelab import upstream

    for key, path in upstream.fetch_all(force=args.force, record=args.record).items():
        print(f"ok {key:28s} {path.name}")
    return 0


def _models_build(args: argparse.Namespace) -> int:
    from facelab import build

    build.build_all(skip_fidelity=args.skip_fidelity)
    return 0


def _testdata_portraits(args: argparse.Namespace) -> int:
    from facelab.testdata import portraits

    portraits.build()
    return 0


def _testdata_documents(args: argparse.Namespace) -> int:
    from facelab.testdata import documents

    documents.build()
    return 0


def _models_pack(args: argparse.Namespace) -> int:
    from pathlib import Path

    from facelab import pack

    pack.write_pack(args.version, Path(args.out) if args.out else pack.DEFAULT_OUT)
    return 0


def _models_bom(args: argparse.Namespace) -> int:
    import json

    from facelab.paths import MANIFEST_PATH

    m = json.loads(MANIFEST_PATH.read_text(encoding="utf-8"))
    print("| id | role | licence | training data: commercial use | training data (statement) | review |")
    print("|---|---|---|---|---|---|")
    for e in m["models"] + m.get("assets", []):
        lic = e["licence"]
        td = lic["trainingData"]
        summary = td["statement"].replace("|", "/")
        print(
            f"| `{e['id']}` | {e.get('role', 'asset')} | {lic['spdx']} | {td['commercialUse']} | {summary} | "
            f"{e.get('review', {}).get('status', 'n/a')} |"
        )
    from facelab.upstream import SOURCES, WEB_SDK_ASSETS

    print(
        "\n**Web SDK assets** (loaded in the browser by sdk-web; self-hosted, never fetched from Google at runtime)\n"
    )
    print("| file | licence | upstream | sha256 |")
    print("|---|---|---|---|")
    for key in WEB_SDK_ASSETS:
        s = SOURCES[key]
        print(f"| `{s.filename}` | Apache-2.0 | {s.url} | `{s.sha256}` |")
    return 0


def _exp_int8(args: argparse.Namespace) -> int:
    from facelab.experiments import auraface_int8

    auraface_int8.run()
    return 0


def _eval_run(args: argparse.Namespace) -> int:
    from pathlib import Path

    from facelab.eval import run

    r = run.run(
        Path(args.manifest),
        Path(args.out),
        tuple(k for k in args.match_on.split(",") if k),
        Path(args.scores) if args.scores else None,
        Path(args.predictions) if args.predictions else None,
    )
    print(f"wrote {args.out}/report.md (threshold basis: {r['thresholdBasis']})")
    return 0


def _eval_fixtures(args: argparse.Namespace) -> int:
    from facelab.eval import run
    from facelab.paths import WORK_DIR

    run.run_fixtures()
    print(f"wrote {WORK_DIR / 'eval-fixtures' / 'report.md'} (ILLUSTRATIVE)")
    return 0


def _parity(args: argparse.Namespace) -> int:
    import sys
    import traceback
    from pathlib import Path

    from facelab import parity

    # Exit codes: 0 gate passed, 1 gate failed, 2 the run itself crashed (never to be read as a gate result).
    try:
        r = parity.run(
            args.service,
            parity.key_from_env(args.api_key_env),
            args.profile,
            Path(args.out) if args.out else parity.DEFAULT_OUT,
            args.require_complete,
        )
    except Exception:  # noqa: BLE001
        traceback.print_exc()
        print("parity run CRASHED (exit 2)", file=sys.stderr)
        return 2
    g = r["gate"]
    print(f"parity gate {'PASSED' if g['passed'] else 'FAILED'}: {g['fails']} fail, {g['undetermined']} undetermined")
    return 0 if g["passed"] else 1


def _golden(args: argparse.Namespace) -> int:
    from facelab import golden

    golden.build(args.ids or None)
    return 0


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(prog="facelab")
    groups = parser.add_subparsers(dest="group", required=True)

    models = groups.add_parser("models", help="upstream fetch, ONNX conversion, manifest")
    mcmd = models.add_subparsers(dest="command", required=True)
    p = mcmd.add_parser("fetch", help="download pinned upstream artifacts into models/upstream/")
    p.add_argument("--force", action="store_true", help="re-download even if cached")
    p.add_argument("--record", action="store_true", help="accept and print hashes of unpinned sources")
    p.set_defaults(func=_models_fetch)
    p = mcmd.add_parser("build", help="convert to ONNX, check fidelity, write models/manifest.json")
    p.add_argument("--skip-fidelity", action="store_true", help="development only; the manifest says so")
    p.set_defaults(func=_models_build)
    p = mcmd.add_parser("pack", help="write the versioned, reproducible model pack zip (default service/dist/)")
    p.add_argument("--version", required=True, help="model pack version, e.g. 1.0.0")
    p.add_argument("--out", default=None, help="output directory (default service/dist/, git-ignored)")
    p.set_defaults(func=_models_pack)
    p = mcmd.add_parser("bom", help="print the model bill of materials (markdown) from models/manifest.json")
    p.set_defaults(func=_models_bom)

    td = groups.add_parser("testdata", help="licence-clean test images (L4)")
    tcmd = td.add_subparsers(dest="command", required=True)
    p = tcmd.add_parser("portraits", help="fetch + verify PD NASA portraits into testdata/images/")
    p.set_defaults(func=_testdata_portraits)
    p = tcmd.add_parser("documents", help="synthesise SPECIMEN ID-1 cards into testdata/documents/")
    p.set_defaults(func=_testdata_documents)

    ex = groups.add_parser("experiment", help="one-off lab experiments (results in lab/work/)")
    ecmd = ex.add_subparsers(dest="command", required=True)
    p = ecmd.add_parser("auraface-int8", help="L6: INT8 AuraFace size / latency / drift vs FP32")
    p.set_defaults(func=_exp_int8)

    ev = groups.add_parser("eval", help="L7 evaluation / calibration harness")
    vcmd = ev.add_subparsers(dest="command", required=True)
    p = vcmd.add_parser("run", help="evaluate a dataset manifest (data and output must be outside the repo)")
    p.add_argument("--manifest", required=True)
    p.add_argument("--out", required=True, help="output dir outside the repository (or under lab/work/)")
    p.add_argument("--match-on", default="", help="comma-separated subject attributes for matched impostors")
    p.add_argument("--scores", help="CSV probe,reference,score from the service, instead of the lab scorer")
    p.add_argument("--predictions", help="JSONL {sample, checks:{CHECK_ID: status}} for quality confusion")
    p.set_defaults(func=_eval_run)
    p = vcmd.add_parser("fixtures", help="ILLUSTRATIVE end-to-end run on the PD fixtures -> lab/work/eval-fixtures/")
    p.set_defaults(func=_eval_fixtures)

    p = groups.add_parser("parity", help="L8: run the fixtures through the service API and compare with goldens")
    p.add_argument("--service", required=True, help="e.g. http://127.0.0.1:8080")
    p.add_argument("--api-key-env", default="VERIFACE_API_KEY", help="env var holding the API key")
    p.add_argument("--profile", default=None)
    p.add_argument("--out", default=None, help="output dir (default lab/work/parity; never inside the repo otherwise)")
    p.add_argument("--require-complete", action="store_true", help="also fail on undetermined comparisons")
    p.set_defaults(func=_parity)

    p = groups.add_parser("golden", help="write testdata/golden/ (L5)")
    p.add_argument("ids", nargs="*", help="image ids from testdata/provenance.json (default: all)")
    p.set_defaults(func=_golden)

    args = parser.parse_args(argv)
    return args.func(args)

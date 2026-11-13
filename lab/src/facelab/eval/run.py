"""``facelab eval run`` / ``facelab eval fixtures``: end-to-end evaluation and calibration report."""

from __future__ import annotations

import csv
import datetime as dt
import json
from pathlib import Path

import numpy as np
import yaml

from facelab.eval import dataset as dsm
from facelab.eval import metrics, pitch, protocol, scorer
from facelab.paths import TESTDATA_DIR, WORK_DIR
from facelab.testdata import provenance

TARGET_FMRS = (1e-2, 1e-3, 1e-4)
OP_IDS = {1e-2: "fmr-1e-2", 1e-3: "fmr-1e-3", 1e-4: "fmr-1e-4"}
SCORE_SOURCE_LAB = "lab reference (MediaPipe Tasks landmarks -> ArcFace 5-point alignment -> AuraFace, ONNX Runtime)"


def _score_all(ds: dsm.Dataset, out: Path) -> dict[str, scorer.Scored]:
    cache_path = out / "embeddings.npz"  # derived biometric data: stays in the (non-repo) output dir
    cached = dict(np.load(cache_path, allow_pickle=True)) if cache_path.exists() else {}
    result = {}
    for s in ds.samples:
        if s.id in cached:
            rec = cached[s.id].item()
            result[s.id] = scorer.Scored(s.id, rec["found"], rec["emb"], rec["pose"], rec["faces"])
            continue
        r = scorer.score_sample(s)
        result[s.id] = r
        cached[s.id] = np.array({"found": r.face_found, "emb": r.embedding, "pose": r.pose, "faces": r.faces})
    np.savez(cache_path, **cached)
    return result


def _load_scores_file(path: Path) -> dict[tuple[str, str], float | None]:
    """probe,reference,score[,...] CSV; an empty score (the service returned none) is None."""
    with path.open(encoding="utf-8") as f:
        return {
            (row["probe"], row["reference"]): float(row["score"]) if row["score"].strip() else None
            for row in csv.DictReader(f)
        }


def _det_png(curves: dict[str, tuple], path: Path) -> None:
    import matplotlib

    matplotlib.use("Agg")
    import matplotlib.pyplot as plt
    from scipy.stats import norm

    fig, ax = plt.subplots(figsize=(5, 5))
    for label, (_, f, n) in curves.items():
        keep = (f > 0) & (n > 0) & (f < 1) & (n < 1)
        ax.plot(norm.ppf(f[keep]), norm.ppf(n[keep]), label=label)
    ticks = [1e-4, 1e-3, 1e-2, 0.05, 0.2, 0.5]
    ax.set_xticks(norm.ppf(ticks), [f"{t:g}" for t in ticks])
    ax.set_yticks(norm.ppf(ticks), [f"{t:g}" for t in ticks])
    ax.set_xlabel("FMR")
    ax.set_ylabel("FNMR")
    ax.set_title("DET")
    ax.grid(True, alpha=0.3)
    ax.legend()
    fig.tight_layout()
    fig.savefig(path, dpi=120)
    plt.close(fig)


def run(
    manifest: Path,
    out: Path,
    match_on: tuple[str, ...] = (),
    scores_file: Path | None = None,
    predictions_file: Path | None = None,
) -> dict:
    dsm.check_output_dir(out)
    out.mkdir(parents=True, exist_ok=True)
    ds = dsm.load(manifest)
    scored = _score_all(ds, out)
    usable = {k for k, v in scored.items() if v.face_found}
    fta = {
        role: {
            "samples": sum(1 for s in ds.samples if s.role == role),
            "failuresToAcquire": sum(1 for s in ds.samples if s.role == role and s.id not in usable),
        }
        for role in ("probe", "reference")
    }

    external = _load_scores_file(scores_file) if scores_file else None

    def scores_for(c: protocol.Comparisons) -> np.ndarray:
        if external is not None:
            return np.array([external[(p, r)] for p, r in zip(c.probe, c.reference, strict=True)])
        return np.array(
            [float(scored[p].embedding @ scored[r].embedding) for p, r in zip(c.probe, c.reference, strict=True)]
        )

    sets = {"all": protocol.build(ds, usable, "all")}
    if match_on:
        sets["matched"] = protocol.build(ds, usable, "matched", match_on)
    # With external scores, a comparison the service gave no score for (e.g. no face, retake) is dropped and counted.
    without_score: dict[str, int] = {}
    if external is not None:
        for name, comp in list(sets.items()):
            keep = np.array(
                [external.get((p, r)) is not None for p, r in zip(comp.probe, comp.reference, strict=True)], bool
            )
            without_score[name] = int((~keep).sum())
            sets[name] = comp.subset(keep)
    results: dict = {
        "generatedAt": dt.datetime.now(dt.UTC).replace(microsecond=0).isoformat().replace("+00:00", "Z"),
        "dataset": {
            "name": ds.name,
            "illustrative": ds.illustrative,
            "subjects": len(ds.subjects),
            "samples": len(ds.samples),
            "failureToAcquire": fta,
        },
        "scoreSource": f"external scores file {scores_file.name}" if scores_file else SCORE_SOURCE_LAB,
        "matchOn": list(match_on),
        "impostorSets": {},
    }
    curves = {}
    for name, comp in sets.items():
        sc = scores_for(comp)
        g, imp = sc[comp.genuine], sc[~comp.genuine]
        res: dict = {"genuine": int(len(g)), "impostor": int(len(imp))}
        if name in without_score:
            res["comparisonsWithoutScore"] = without_score[name]
        if len(g) and len(imp):
            e, et = metrics.eer(g, imp)
            res["eer"] = {"eer": round(e, 5), "threshold": round(et, 4)}
            gen_subj = comp.probe_subject[comp.genuine]
            imp_pairs = np.column_stack([comp.probe_subject[~comp.genuine], comp.reference_subject[~comp.genuine]])
            ops = []
            for target in TARGET_FMRS:
                op = metrics.operating_point(g, imp, target)
                if len(np.unique(imp_pairs)) >= 4:
                    op.threshold_ci = metrics.subject_bootstrap_threshold(gen_subj, imp_pairs, imp, target)
                ops.append(op.to_json())
            res["operatingPoints"] = ops
            order = np.argsort(-imp)[:10]
            imp_idx = np.flatnonzero(~comp.genuine)
            res["highestImpostors"] = [
                {
                    "probe": comp.probe[imp_idx[i]],
                    "reference": comp.reference[imp_idx[i]],
                    "score": round(float(imp[i]), 4),
                }
                for i in order
            ]
            res["lowestGenuine"] = [
                {"probe": comp.probe[j], "reference": comp.reference[j], "score": round(float(sc[j]), 4)}
                for j in np.flatnonzero(comp.genuine)[np.argsort(g)[:10]]
            ]
            curves[f"{name} impostors"] = metrics.det_curve(g, imp)
            with (out / f"det-{name}.csv").open("w", newline="", encoding="utf-8") as f:
                w = csv.writer(f)
                w.writerow(["threshold", "fmr", "fnmr"])
                for t, fm, fn in zip(*curves[f"{name} impostors"], strict=True):
                    w.writerow([t, fm, fn])
        results["impostorSets"][name] = res

    # Thresholds must come from the matched (harder) impostor set when there is one.
    basis = "matched" if "matched" in sets and results["impostorSets"]["matched"].get("operatingPoints") else "all"
    results["thresholdBasis"] = basis
    if curves:
        _det_png(curves, out / "det.png")

    # Differentials at each proposed threshold, grouped by the match attributes.
    if match_on and results["impostorSets"][basis].get("operatingPoints"):
        comp = sets[basis]
        sc = scores_for(comp)
        gen_groups = np.array([protocol.group_of(ds, s, match_on) for s in comp.probe_subject[comp.genuine]], object)
        imp_groups = np.array(
            [
                protocol.group_of(ds, a, match_on)
                if protocol.group_of(ds, a, match_on) == protocol.group_of(ds, b, match_on)
                else None
                for a, b in zip(comp.probe_subject[~comp.genuine], comp.reference_subject[~comp.genuine], strict=True)
            ],
            object,
        )
        results["differentials"] = {}
        for op in results["impostorSets"][basis]["operatingPoints"]:
            t = (op["claimable"] or op["empirical"])["threshold"]
            results["differentials"][OP_IDS[op["targetFmr"]]] = {
                "threshold": t,
                **metrics.group_differentials(sc[comp.genuine], gen_groups, sc[~comp.genuine], imp_groups, t),
            }

    # Quality checks: ground truth vs predictions.
    preds: dict[str, dict[str, str]] = {}
    if predictions_file:
        for line in predictions_file.read_text(encoding="utf-8").splitlines():
            if line.strip():
                rec = json.loads(line)
                preds[rec["sample"]] = rec["checks"]
        pred_source = f"predictions file {predictions_file.name}"
    else:
        for sid, r in scored.items():  # what the lab scorer can say on its own
            role = next(s.role for s in ds.samples if s.id == sid)
            found = "pass" if r.face_found else "fail"
            preds[sid] = {"FACE_PRESENT" if role == "probe" else "PORTRAIT_FOUND": found}
            if r.face_found:
                preds[sid]["SINGLE_FACE"] = "pass" if r.faces == 1 else "fail"
        pred_source = "lab scorer (face found / face count only)"
    results["quality"] = {"predictionSource": pred_source, "checks": quality_summary(ds.samples, preds)}

    # Pitch offset on frontal-labelled samples.
    frontal = [s for s in ds.samples if s.frontal and s.id in usable]
    if frontal:
        subj = np.array([s.subject for s in frontal], object)
        elev = np.array([np.nan if s.camera_elevation_deg is None else s.camera_elevation_deg for s in frontal])
        results["poseBias"] = {
            "pitch": pitch.estimate(subj, np.array([scored[s.id].pose["pitch"] for s in frontal]), elev),
            "yaw": pitch.estimate(subj, np.array([scored[s.id].pose["yaw"] for s in frontal])),
            "roll": pitch.estimate(subj, np.array([scored[s.id].pose["roll"] for s in frontal])),
        }

    results["proposedProfile"] = _proposal(results, ds)
    (out / "results.json").write_text(json.dumps(results, indent=2) + "\n", encoding="utf-8")
    (out / "report.md").write_text(render_report(results), encoding="utf-8")
    return results


def quality_summary(samples: list[dsm.Sample], preds: dict[str, dict[str, str]]) -> dict:
    """Per check: confusion on AGREED labels only. Where a second annotator disagrees, the sample goes to the
    'ambiguous' pool (reported with its prediction counts, excluded from the matrix). Cohen's kappa is computed
    on samples labelled by both annotators."""
    out = {}
    for check in sorted({c for s in samples for c in s.labels}):
        labs, ps, a1, a2 = [], [], [], []
        ambiguous: dict[str, int] = {"pass": 0, "fail": 0, "undetermined": 0, "skipped": 0}
        for s in samples:
            if check not in s.labels:
                continue
            pred = preds.get(s.id, {}).get(check, "skipped")
            if check in s.labels2:
                a1.append(s.labels[check])
                a2.append(s.labels2[check])
                if s.labels2[check] != s.labels[check]:
                    ambiguous[pred] += 1
                    continue
            labs.append(s.labels[check])
            ps.append(pred)
        c = metrics.confusion(labs, ps)
        c["agreedSamples"] = len(labs)
        c["doubleAnnotated"] = len(a1)
        c["cohenKappa"] = metrics.cohen_kappa(a1, a2)
        c["ambiguous"] = {"samples": sum(ambiguous.values()), "predicted": ambiguous}
        out[check] = c
    return out


def _proposal(results: dict, ds: dsm.Dataset) -> dict:
    basis = results["thresholdBasis"]
    ops = results["impostorSets"][basis].get("operatingPoints") or []
    points, notes = [], []
    for op in ops:
        chosen = op["claimable"] or op["empirical"]
        points.append({"id": OP_IDS[op["targetFmr"]], "targetFmr": op["targetFmr"], "threshold": chosen["threshold"]})
        if op["claimable"] is None:
            notes.append(
                f"{OP_IDS[op['targetFmr']]}: NOT SUPPORTED by sample size ({op['impostorComparisons']} {basis} "
                f"impostor comparisons, {op['comparisonsNeededForClaim']} needed); empirical threshold shown only "
                "for reference"
            )
    status = "provisional" if (ds.illustrative or notes) else "calibrated"
    return {
        "verification": {
            "operatingPoints": points,
            "calibration": {
                "id": f"lab-{ds.name}-{results['generatedAt'][:10]}",
                "status": status,
                "dataset": f"{ds.name}: {results['dataset']['subjects']} subjects, "
                f"{results['dataset']['samples']} samples"
                + (" (ILLUSTRATIVE fixtures, not calibration data)" if ds.illustrative else ""),
                "date": results["generatedAt"][:10],
                "notes": "; ".join(notes) or f"thresholds from the {basis} impostor set, 95% exact upper bounds",
            },
        }
    }


def _f(v) -> str:
    """Compact number for tables; '-' when undefined (no comparisons, or all rates zero)."""
    return "-" if v is None else f"{v:.3g}"


def render_report(r: dict) -> str:
    L: list[str] = []
    d = r["dataset"]
    L.append(f"# Face verification evaluation: {d['name']}\n")
    if d["illustrative"]:
        L.append(
            "> **ILLUSTRATIVE ONLY. NOT A CALIBRATION.** This run uses the committed public-domain fixtures to "
            "exercise the harness end to end. The sample is far too small, and its genuine pairs reuse the same "
            "photograph on synthetic cards. Grouping attributes are synthetic and are **not** demographic. "
            "Do not use any number here as a threshold.\n"
        )
    L.append(f"Generated {r['generatedAt']}. Scores: {r['scoreSource']}.\n")
    L.append("## Data\n")
    L.append(f"- Subjects: {d['subjects']}; samples: {d['samples']}")
    for role, v in d["failureToAcquire"].items():
        L.append(f"- {role}s: {v['samples']}, failures to acquire (no face): {v['failuresToAcquire']}")
    L.append(f"- Impostor sets: {', '.join(r['impostorSets'])}; match attributes: {r['matchOn'] or 'none'}")
    L.append(f"- Thresholds are taken from the **{r['thresholdBasis']}** impostor set.\n")
    for name, s in r["impostorSets"].items():
        L.append(f"## Verification: {name} impostors\n")
        L.append(
            f"Genuine comparisons {s['genuine']}, impostor comparisons {s['impostor']}."
            + (
                f" Dropped (no score from the service): {s['comparisonsWithoutScore']}."
                if s.get("comparisonsWithoutScore")
                else ""
            )
        )
        if "eer" in s:
            L.append(f"EER {s['eer']['eer']:.4f} at threshold {s['eer']['threshold']:.4f}.\n")
            L.append(
                "| target FMR | needed for a claim | empirical t | FMR (FM) | FNMR | t CI95 (subject bootstrap) "
                "| claimable t | FMR upper 95% | FNMR |"
            )
            L.append("|---|---|---|---|---|---|---|---|---|")
            for op in s["operatingPoints"]:
                e, c = op["empirical"], op["claimable"]
                ci = op["empiricalThresholdCi95SubjectBootstrap"]
                L.append(
                    f"| {op['targetFmr']:g} | {op['comparisonsNeededForClaim']} | {e['threshold']:.4f} | "
                    f"{e['fmr']:.2e} ({e['falseMatches']}) | {e['fnmr']:.4f} | {ci if ci else '-'} | "
                    + (
                        f"{c['threshold']:.4f} | {c['fmrUpper95']:.2e} | {c['fnmr']:.4f} |"
                        if c
                        else "not supported | - | - |"
                    )
                )
            L.append("\nHighest-scoring impostor comparisons (ids only):\n")
            for x in s["highestImpostors"][:5]:
                L.append(f"- {x['probe']} vs {x['reference']}: {x['score']}")
            L.append("\nLowest-scoring genuine comparisons:\n")
            for x in s["lowestGenuine"][:5]:
                L.append(f"- {x['probe']} vs {x['reference']}: {x['score']}")
            L.append("")
    if "differentials" in r:
        L.append("## Differential performance (ISO/IEC 19795-10 style)\n")
        L.append("Per-group FMR uses within-group impostor comparisons; upper bounds are exact one-sided 95%.\n")
        for op_id, dd in r["differentials"].items():
            L.append(f"### {op_id} (threshold {dd['threshold']})\n")
            L.append("| group | impostor n | FM | FMR | FMR upper | genuine n | FNM | FNMR | FNMR upper |")
            L.append("|---|---|---|---|---|---|---|---|---|")
            for g, v in dd["groups"].items():
                L.append(
                    f"| {g} | {v['impostorComparisons']} | {v['falseMatches']} | {_f(v['fmr'])} | "
                    f"{_f(v['fmrUpper95'])} | {v['genuineComparisons']} | {v['falseNonMatches']} | "
                    f"{_f(v['fnmr'])} | {_f(v['fnmrUpper95'])} |"
                )
            L.append(
                f"\nFMR max/min ratio: {_f(dd['fmrMaxMinRatio'])}; FNMR max/min ratio: {_f(dd['fnmrMaxMinRatio'])} "
                "('-' = undefined: a group without comparisons, or every rate zero)\n"
            )
    L.append("## Quality checks\n")
    L.append(f"Predictions: {r['quality']['predictionSource']}.\n")
    for check, c in r["quality"]["checks"].items():
        L.append(
            f"**{check}**: false reject {_f(c['falseRejectRate'])}, false accept {_f(c['falseAcceptRate'])}, "
            f"undetermined {_f(c['undeterminedRate'])}\n"
        )
        L.append(
            f"Agreed labels: {c['agreedSamples']}; double-annotated: {c['doubleAnnotated']}, Cohen's kappa "
            f"{_f(c['cohenKappa'])}; ambiguous (annotators disagree, excluded): {c['ambiguous']['samples']} "
            f"{c['ambiguous']['predicted'] if c['ambiguous']['samples'] else ''}\n"
        )
        L.append("| agreed label \\ predicted | pass | fail | undetermined | skipped |\n|---|---|---|---|---|")
        for lab, row in c["matrix"].items():
            L.append(f"| {lab} | {row['pass']} | {row['fail']} | {row['undetermined']} | {row['skipped']} |")
        L.append("")
    if "poseBias" in r:
        L.append("## Pose bias on frontal-labelled samples\n")
        for k, v in r["poseBias"].items():
            extra = f", slope {v['slopePerElevationDeg']} {v['slopeCi95']}" if "slopePerElevationDeg" in v else ""
            conf = " (confounded with camera height: no elevation data)" if v["confoundedWithCameraHeight"] else ""
            L.append(
                f"- {k}: bias {v['biasDeg']} deg, CI95 {v['biasCi95']}, method {v['method']}, "
                f"{v['subjects']} subjects{extra}{conf}"
            )
        L.append("")
    L.append("## Proposed profile values\n")
    L.append("```yaml\n" + yaml.safe_dump(r["proposedProfile"], sort_keys=False) + "```\n")
    L.append("## Method\n")
    L.append(
        "- Comparisons: every probe against every reference. Genuine means the same subject. Impostors are either "
        "all cross-subject pairs, or only pairs whose subjects match on every listed attribute (the demographically "
        "matched set). Thresholds are chosen on the matched set, which is the harder case.\n"
        "- Empirical threshold: the lowest threshold with measured FMR <= target. Claimable threshold: the lowest "
        "threshold whose exact one-sided 95% Clopper-Pearson upper bound on FMR is <= target. With zero false "
        "matches that bound is about 3/N (the rule of 3), so a target FMR needs at least about 3/target impostor "
        "comparisons.\n"
        "- Threshold CI: subject-level bootstrap, because comparisons that share a subject are not independent. "
        "The Clopper-Pearson bound behind the claimable threshold treats comparisons as independent. Comparisons "
        "that share subjects carry less information than their count suggests, so the bound is optimistic, even "
        "with zero false matches. Read it together with the bootstrap CI, and prefer one probe and one reference "
        "per subject when sizing a claim.\n"
        "- Pose bias: the intercept of pitch on camera elevation (OLS), with a subject-bootstrap CI. It is the "
        "median when no elevation is recorded, and is then confounded with camera height.\n"
    )
    return "\n".join(L) + "\n"


def fixtures_manifest(path: Path) -> Path:
    """Illustrative manifest over the committed PD fixtures. Groups are SYNTHETIC (not demographic): we do not
    record perceived sex, age or skin tone of identifiable people in the repository."""
    images = provenance.all_images()
    data = provenance.load()
    person = {r["id"]: r["id"] for r in data["portraits"]}
    person.update({r["id"]: r["parent"] for r in data.get("derived", [])})
    for r in data["documents"]:
        if len(r["sources"]) == 1:
            person[r["id"]] = r["sources"][0]
    people = sorted(set(person.values()))
    group = {p: "ABC"[i % 3] for i, p in enumerate(people)}
    samples = []
    for sid, subj in person.items():
        row = images[sid]
        is_doc = sid.startswith("doc-")
        s = {
            "id": sid,
            "subject": subj,
            "path": row["file"],
            "role": "reference" if is_doc else "probe",
            "kind": "document" if is_doc else "portrait",
        }
        if is_doc:
            s["labels"] = {"PORTRAIT_FOUND": "pass" if row["groundTruth"]["mainPortraits"] >= 1 else "fail"}
        elif "exp49" not in sid:
            s["frontal"] = True  # official portraits: approximately frontal, camera height unknown
            s["labels"] = {"SINGLE_FACE": "pass"}
        else:
            s["labels"] = {"SINGLE_FACE": "fail"}
        samples.append(s)
    manifest = {
        "formatVersion": 1,
        "name": "pd-fixtures",
        "illustrative": True,
        "root": str(TESTDATA_DIR),
        "subjects": [{"id": p, "attributes": {"fixtureGroup": group[p]}} for p in people],
        "samples": samples,
    }
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(yaml.safe_dump(manifest, sort_keys=False), encoding="utf-8")
    return path


def run_fixtures() -> dict:
    out = WORK_DIR / "eval-fixtures"
    manifest = fixtures_manifest(out / "manifest.yaml")
    return run(manifest, out, match_on=("fixtureGroup",))

// Renders API results: verdict/outcome, checks, reasons, risk, overlays, output image, timings, raw JSON.
import type {
  CheckResult,
  DocumentPortraitResult,
  EngineInfo,
  OutputImage,
  QualityResult,
  SubjectAssessment,
  VerificationResult,
} from "./api";
import { ApiError } from "./api";
import { fmt, h } from "./dom";
import { overlay, type OverlayData } from "./overlay";

const VERDICT_CLASS: Record<string, string> = {
  accepted: "ok",
  verified: "ok",
  match: "ok",
  rejected: "bad",
  no_match: "bad",
  retake: "warn",
  review: "warn",
  undetermined: "warn",
};

function badge(text: string): HTMLElement {
  return h(`span.badge.${VERDICT_CLASS[text] ?? "neutral"}`, null, text);
}

function engineBanner(engine: EngineInfo): HTMLElement | null {
  if (!engine.version.endsWith("+stub")) return null;
  return h(
    "div.banner.warn",
    null,
    `Engine ${engine.version}: the STUB engine. Every check is undetermined by design. This tests the plumbing, not the pipeline.`,
  );
}

function bounds(c: CheckResult): string {
  const lo = c.min ?? null;
  const hi = c.max ?? null;
  if (lo === null && hi === null) return "–";
  if (lo !== null && hi !== null) return `${fmt(lo)} … ${fmt(hi)}`;
  return lo !== null ? `≥ ${fmt(lo)}` : `≤ ${fmt(hi)}`;
}

export function checksTable(checks: CheckResult[], reasons: string[] = [], warnings: string[] = []): HTMLElement {
  return h(
    "table.checks",
    null,
    h(
      "thead",
      null,
      h("tr", null, ...["check", "group", "status", "value", "allowed", "severity", ""].map((t) => h("th", null, t))),
    ),
    h(
      "tbody",
      null,
      ...checks.map((c) =>
        h(
          `tr.${reasons.includes(c.id) ? "is-reason" : warnings.includes(c.id) ? "is-warning" : "plain"}`,
          null,
          h("td.mono", null, c.id),
          h("td.muted", null, c.group),
          h("td", null, h(`span.status.${c.status}`, null, c.status)),
          h("td.num", null, c.value === null ? "–" : `${fmt(c.value)} ${c.unit}`),
          h("td.num", null, bounds(c)),
          h("td", null, c.severity),
          h("td", null, c.experimental ? h("span.tag", null, "experimental") : null),
        ),
      ),
    ),
  );
}

function reasonList(title: string, ids: string[]): HTMLElement | null {
  if (!ids.length) return null;
  return h("div.reasons", null, h("strong", null, `${title}: `), ids.map((r, i) => `${i + 1}. ${r}`).join("  "));
}

function subject(title: string, a: SubjectAssessment, imageUrl: string | null, extra: OverlayData = {}): HTMLElement {
  const face = a.face;
  return h(
    "section.subject",
    null,
    h("h3", null, title, " ", badge(a.verdict)),
    reasonList("Reasons (errors, priority order)", a.reasons),
    reasonList("Warnings", a.warnings),
    h(
      "div.split",
      null,
      imageUrl ? overlay(imageUrl, { face, ...extra }) : null,
      h(
        "div",
        null,
        face
          ? h(
              "dl.facts",
              null,
              h("dt", null, "inter-eye distance"),
              h("dd", null, `${fmt(face.interEyeDistancePx, 1)} px`),
              face.pose ? h("dt", null, "pose (yaw / pitch / roll)") : null,
              face.pose ? h("dd", null, `${fmt(face.pose.yaw, 1)}° / ${fmt(face.pose.pitch, 1)}° / ${fmt(face.pose.roll, 1)}°`) : null,
            )
          : h("p.muted", null, "No face geometry returned."),
        checksTable(a.checks, a.reasons, a.warnings),
      ),
    ),
  );
}

function outputImage(o: OutputImage | null | undefined): HTMLElement | null {
  if (!o) return null;
  return h(
    "section.output",
    null,
    h("h3", null, "Output image"),
    h("img", { src: `data:${o.mediaType};base64,${o.data}`, alt: "normalised output" }),
    h("div.muted", null, `${o.width}×${o.height} · ${o.mediaType} · ${(o.bytes / 1024).toFixed(1)} KB`),
  );
}

function timings(t: Record<string, number>, clientMs: number, requestId: string): HTMLElement {
  return h(
    "section.timings",
    null,
    h("h3", null, "Timings"),
    h(
      "table.kv",
      null,
      h("tbody", null, ...Object.entries(t).map(([k, v]) => h("tr", null, h("td", null, k), h("td.num", null, `${fmt(v, 1)} ms`)))),
      h("tr", null, h("td", null, "round trip (browser)"), h("td.num", null, `${fmt(clientMs, 1)} ms`)),
    ),
    h("div.muted.mono", null, `requestId ${requestId}`),
  );
}

function raw(data: unknown): HTMLElement {
  const copy = JSON.parse(JSON.stringify(data)) as Record<string, any>;
  if (copy.output?.data) copy.output.data = `<${copy.output.data.length} base64 chars>`;
  return h("details.raw", null, h("summary", null, "Raw JSON"), h("pre", null, JSON.stringify(copy, null, 2)));
}

function header(engine: EngineInfo, profile: { name: string; version: string }): HTMLElement {
  return h("div.meta.muted", null, `profile ${profile.name} ${profile.version} · engine ${engine.version}`);
}

export function renderQuality(r: QualityResult, imageUrl: string | null, ms: number): HTMLElement {
  return h(
    "div.result",
    null,
    engineBanner(r.engine),
    header(r.engine, r.profile),
    subject("Selfie", r, imageUrl),
    outputImage(r.output),
    timings(r.timings, ms, r.requestId),
    raw(r),
  );
}

export function renderDocument(r: DocumentPortraitResult, imageUrl: string | null, ms: number): HTMLElement {
  const d = r.document;
  return h(
    "div.result",
    null,
    engineBanner(r.engine),
    header(r.engine, r.profile),
    h(
      "div.summary",
      null,
      h("span", null, `document detected: ${d.detected ? "yes" : "no"} (confidence ${fmt(d.confidence, 2)})`),
      h("span", null, `orientation applied: ${d.orientationDegrees}° clockwise`),
    ),
    subject("Document portrait", r, imageUrl, { document: d, portraitSourceBox: r.portraitSourceBox }),
    outputImage(r.output),
    timings(r.timings, ms, r.requestId),
    raw(r),
  );
}

export function renderVerification(
  r: VerificationResult,
  probeUrl: string | null,
  refUrl: string | null,
  ms: number,
): HTMLElement {
  const provisional = r.calibration.status === "provisional";
  return h(
    "div.result",
    null,
    engineBanner(r.engine),
    provisional
      ? h(
          "div.banner.warn",
          null,
          `Calibration "${r.calibration.id}" is PROVISIONAL: the threshold is not yet calibrated on representative data, so the target FMR (${r.operatingPoint.targetFmr}) is not a real guarantee. Do not rely on it.`,
        )
      : null,
    header(r.engine, r.profile),
    h(
      "div.summary.big",
      null,
      h("span", null, "outcome ", badge(r.outcome)),
      h("span", null, "decision ", badge(r.decision)),
      h(
        "span",
        null,
        `score ${r.score === null ? "–" : fmt(r.score, 4)} vs threshold ${fmt(r.threshold, 4)} `,
        h("span.muted", null, `(operating point ${r.operatingPoint.id}, target FMR ${r.operatingPoint.targetFmr})`),
      ),
    ),
    r.reasons.length
      ? h(
          "div.reasons",
          null,
          h("strong", null, "Why: "),
          r.reasons.map((x, i) => `${i + 1}. ${x.source}:${x.code}`).join("  "),
        )
      : null,
    h(
      "section.subject",
      null,
      h("h3", null, "Risk checks ", r.risk.flagged ? badge("rejected") : h("span.badge.neutral", null, "not flagged")),
      checksTable(r.risk.checks),
    ),
    subject("Probe (selfie)", r.probe, probeUrl),
    subject(`Reference (${r.reference.kind})`, r.reference, refUrl, { document: r.reference.document ?? null }),
    timings(r.timings, ms, r.requestId),
    raw(r),
  );
}

export function renderError(e: unknown): HTMLElement {
  if (e instanceof ApiError) {
    return h(
      "div.result",
      null,
      h(
        "div.banner.bad",
        null,
        e.problem
          ? `${e.problem.status} ${e.problem.code}: ${e.problem.title}${e.problem.detail ? ` — ${e.problem.detail}` : ""}`
          : `HTTP ${e.status}. Is the service running? (${e.body.slice(0, 200)})`,
      ),
      e.problem?.instance ? h("div.muted.mono", null, `request ${e.problem.instance}`) : null,
    );
  }
  return h("div.result", null, h("div.banner.bad", null, String(e)));
}

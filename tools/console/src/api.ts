// Thin client for the Veriface API through the dev proxy (/api -> service; the proxy adds X-API-Key).
// Shapes follow contracts/openapi.yaml.

export type Status = "pass" | "fail" | "undetermined" | "skipped";
export type Point = [number, number];

export interface CheckResult {
  id: string;
  group: string;
  status: Status;
  severity: "error" | "warning";
  value: number | null;
  unit: string;
  min?: number | null;
  max?: number | null;
  experimental?: boolean;
}

export interface Box {
  x: number;
  y: number;
  width: number;
  height: number;
}

export interface FaceInfo {
  box: Box;
  landmarks5: Point[];
  interEyeDistancePx: number;
  pose?: { yaw: number; pitch: number; roll: number };
}

export interface DocumentInfo {
  detected: boolean;
  confidence: number;
  corners?: Point[] | null;
  orientationDegrees: number;
}

export interface OutputImage {
  mediaType: string;
  width: number;
  height: number;
  bytes: number;
  data: string;
}

export interface SubjectAssessment {
  verdict: "accepted" | "rejected" | "undetermined";
  checks: CheckResult[];
  reasons: string[];
  warnings: string[];
  face?: FaceInfo | null;
}

export interface ProfileRef {
  name: string;
  version: string;
  sha256: string;
  description?: string;
}

export interface EngineInfo {
  version: string;
  catalogueVersion: number;
  models: { id: string; role: string; sha256: string; reviewStatus: string }[];
}

export interface QualityResult extends SubjectAssessment {
  requestId: string;
  subject: "selfie";
  profile: ProfileRef;
  output?: OutputImage | null;
  engine: EngineInfo;
  timings: Record<string, number>;
}

export interface DocumentPortraitResult extends SubjectAssessment {
  requestId: string;
  subject: "document";
  profile: ProfileRef;
  document: DocumentInfo;
  portraitSourceBox?: Box | null;
  output?: OutputImage | null;
  engine: EngineInfo;
  timings: Record<string, number>;
}

export interface VerificationResult {
  requestId: string;
  outcome: "verified" | "rejected" | "review" | "retake";
  decision: "match" | "no_match" | "undetermined";
  score: number | null;
  threshold: number;
  operatingPoint: { id: string; targetFmr: number };
  calibration: { id: string; status: "provisional" | "calibrated" };
  probe: SubjectAssessment;
  reference: SubjectAssessment & { kind: "document" | "portrait"; document?: DocumentInfo | null };
  risk: { flagged: boolean; checks: CheckResult[] };
  reasons: { source: string; code: string }[];
  profile: ProfileRef;
  engine: EngineInfo;
  timings: Record<string, number>;
}

export interface Problem {
  type: string;
  title: string;
  status: number;
  detail?: string;
  instance?: string;
  code: string;
}

export class ApiError extends Error {
  constructor(
    readonly status: number,
    readonly problem: Problem | null,
    readonly body: string,
  ) {
    super(problem ? `${problem.status} ${problem.code}: ${problem.detail ?? problem.title}` : `HTTP ${status}`);
  }
}

async function call<T>(path: string, init?: RequestInit): Promise<{ data: T; ms: number; requestId: string | null }> {
  const t0 = performance.now();
  const res = await fetch(`/api${path}`, init);
  const ms = performance.now() - t0;
  const text = await res.text();
  if (!res.ok) {
    let problem: Problem | null = null;
    try {
      problem = JSON.parse(text) as Problem;
    } catch {
      /* not JSON (e.g. the proxy could not reach the service) */
    }
    throw new ApiError(res.status, problem, text);
  }
  return { data: JSON.parse(text) as T, ms, requestId: res.headers.get("x-request-id") };
}

export const api = {
  ready: async (): Promise<boolean> => {
    try {
      return (await fetch("/api/health/ready")).ok;
    } catch {
      return false;
    }
  },
  info: () => call<EngineInfo>("/v1/info"),
  profiles: () => call<{ profiles: ProfileRef[] }>("/v1/profiles"),
  profile: (name: string) => call<{ sha256: string; profile: Record<string, any> }>(`/v1/profiles/${encodeURIComponent(name)}`),
  quality: (image: Blob, q: URLSearchParams) => call<QualityResult>(`/v1/quality?${q}`, { method: "POST", body: form({ image }) }),
  document: (document: Blob, q: URLSearchParams) =>
    call<DocumentPortraitResult>(`/v1/document-portraits?${q}`, { method: "POST", body: form({ document }) }),
  verify: (probe: Blob, reference: Blob, q: URLSearchParams) =>
    call<VerificationResult>(`/v1/verifications?${q}`, { method: "POST", body: form({ probe, reference }) }),
};

function form(parts: Record<string, Blob>): FormData {
  const f = new FormData();
  for (const [k, v] of Object.entries(parts)) f.append(k, v, v instanceof File ? v.name : `${k}.jpg`);
  return f;
}

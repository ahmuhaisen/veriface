import { appliesTo, CHECK_PRIORITY, CHECKS, type CheckId, type CheckStatus, type Subject } from '../contract';
import type { CheckSettings } from '../thresholds';
import type { CheckEvaluation, FrameGuidance, GuidanceCode, GuidanceIssue } from './types';

/** Measured values keyed by check ID; null or missing means "could not be assessed". */
export type CheckValues = Readonly<Partial<Record<CheckId, number | null>>>;

/** True when `value` lies within the inclusive range; a missing bound is unbounded. */
export function withinBounds(value: number, min: number | undefined, max: number | undefined): boolean {
  return (min === undefined || value >= min) && (max === undefined || value <= max);
}

/**
 * Evaluates checks with the catalogue's status rules (contracts/checks.yaml):
 * - skipped: disabled, or not listed in the settings;
 * - undetermined: a prerequisite failed or is undetermined, or there is no value;
 * - pass / fail: the value is inside / outside the inclusive [min, max] range.
 *
 * Prerequisites whose subject is not part of the assessment, or that are skipped, are ignored.
 * Results come in catalogue priority order, which always lists prerequisites first.
 */
export function evaluateChecks(
  values: CheckValues,
  settings: CheckSettings,
  subjects: readonly Subject[],
): CheckEvaluation[] {
  const statuses = new Map<CheckId, CheckStatus>();
  const results: CheckEvaluation[] = [];
  for (const id of CHECK_PRIORITY) {
    if (!appliesTo(id, subjects)) continue;
    const definition = CHECKS[id];
    const setting = settings[id];
    const raw = values[id];
    const value = raw === undefined || raw === null || !Number.isFinite(raw) ? null : raw;
    let status: CheckStatus;
    if (setting?.enabled !== true) {
      status = 'skipped';
    } else if (
      definition.prerequisites.some((p) => {
        if (!appliesTo(p, subjects)) return false;
        const s = statuses.get(p);
        return s === 'fail' || s === 'undetermined';
      })
    ) {
      status = 'undetermined';
    } else if (value === null) {
      status = 'undetermined';
    } else {
      status = withinBounds(value, setting.min, setting.max) ? 'pass' : 'fail';
    }
    statuses.set(id, status);
    results.push({
      id,
      status,
      severity: setting?.severity ?? definition.defaultSeverity,
      value,
      unit: definition.unit,
      min: setting?.min ?? null,
      max: setting?.max ?? null,
    });
  }
  return results;
}

/** Issue for a failed check; `direction` and `code` are chosen by the caller. */
export function issueFor<C extends GuidanceCode>(
  evaluation: CheckEvaluation,
  code: C,
  extra?: Pick<GuidanceIssue<C>, 'direction'>,
): GuidanceIssue<C> {
  return {
    code,
    checkId: evaluation.id,
    severity: evaluation.severity,
    value: evaluation.value,
    min: evaluation.min,
    max: evaluation.max,
    ...extra,
  };
}

/** Whether a failed range check fell below its minimum (as opposed to above its maximum). */
export function belowMinimum(evaluation: CheckEvaluation): boolean {
  return evaluation.value !== null && evaluation.min !== null && evaluation.value < evaluation.min;
}

/** Splits issues into blocking (error) and advisory (warning) lists. */
export function toFrameGuidance<C extends GuidanceCode>(
  checks: readonly CheckEvaluation[],
  issues: readonly GuidanceIssue<C>[],
): FrameGuidance<C> {
  const blocking = issues.filter((i) => i.severity === 'error');
  const advisories = issues.filter((i) => i.severity === 'warning');
  return { checks, issues: blocking, advisories, passed: blocking.length === 0 };
}

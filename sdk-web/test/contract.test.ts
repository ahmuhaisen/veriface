import { describe, expect, it } from 'vitest';
import { CATALOGUE_VERSION, CHECK_IDS, CHECK_PRIORITY, CHECKS, type CheckId } from '../src/contract';
import { DOCUMENT_GUIDANCE_CODES, SELFIE_GUIDANCE_CODES } from '../src/guidance/types';
import { readRepoYaml } from './helpers';

interface CatalogueCheck {
  id: string;
  subjects: string[];
  unit: string;
  prerequisites: string[];
  defaultSeverity: 'error' | 'warning';
}

interface Catalogue {
  catalogueVersion: number;
  reasons: { priority: string[] };
  checks: CatalogueCheck[];
}

const catalogue = readRepoYaml('contracts/checks.yaml') as Catalogue;
const byId = new Map(catalogue.checks.map((check) => [check.id, check]));
const sdkIds = new Set<string>(CHECK_IDS);

describe('contract alignment with contracts/checks.yaml', () => {
  it('targets the same catalogue version', () => {
    expect(CATALOGUE_VERSION).toBe(catalogue.catalogueVersion);
  });

  it.each(CHECK_IDS)('%s exists in the catalogue with the same subjects, unit and default severity', (id) => {
    const check = byId.get(id);
    expect(check, `${id} is missing from checks.yaml`).toBeDefined();
    expect([...CHECKS[id].subjects].sort()).toEqual([...(check?.subjects ?? [])].sort());
    expect(CHECKS[id].unit).toBe(check?.unit);
    expect(CHECKS[id].defaultSeverity).toBe(check?.defaultSeverity);
  });

  it.each(CHECK_IDS)('%s lists every catalogue prerequisite the SDK measures', (id) => {
    const expected = (byId.get(id)?.prerequisites ?? []).filter((p) => sdkIds.has(p));
    expect([...CHECKS[id].prerequisites].sort()).toEqual(expected.sort());
  });

  it('orders checks by the catalogue reasons.priority', () => {
    const catalogueOrder = catalogue.reasons.priority.filter((id) => sdkIds.has(id));
    expect([...CHECK_PRIORITY]).toEqual(catalogueOrder);
    expect(new Set(CHECK_PRIORITY)).toEqual(sdkIds);
  });

  it('lists every prerequisite before the checks that depend on it', () => {
    for (const id of CHECK_PRIORITY) {
      for (const prerequisite of CHECKS[id].prerequisites) {
        expect(CHECK_PRIORITY.indexOf(prerequisite)).toBeLessThan(CHECK_PRIORITY.indexOf(id));
      }
    }
  });

  it('maps every guidance code except HOLD_STILL and READY to a check', () => {
    // The mapping itself is exercised by the guidance tests; this guards the code lists.
    const extra = new Set(['HOLD_STILL', 'READY']);
    expect(SELFIE_GUIDANCE_CODES.filter((c) => !extra.has(c)).length).toBeGreaterThanOrEqual(12);
    expect(DOCUMENT_GUIDANCE_CODES.filter((c) => !extra.has(c)).length).toBeGreaterThanOrEqual(7);
    expect(SELFIE_GUIDANCE_CODES.at(-2)).toBe('HOLD_STILL');
    expect(SELFIE_GUIDANCE_CODES.at(-1)).toBe('READY');
    expect(DOCUMENT_GUIDANCE_CODES.at(-2)).toBe('HOLD_STILL');
    expect(DOCUMENT_GUIDANCE_CODES.at(-1)).toBe('READY');
  });

  it('documents the pose sign conventions the SDK implements', () => {
    // The SDK's pose maths (see pose.test.ts) assumes exactly these conventions.
    const raw = readRepoYaml('contracts/checks.yaml') as { checks: { id: CheckId; value: string }[] };
    const value = (id: CheckId) => raw.checks.find((c) => c.id === id)?.value ?? '';
    expect(value('HEAD_POSE_YAW')).toContain('> 0 = face points towards image-right');
    expect(value('HEAD_POSE_PITCH')).toContain('> 0 = face points up');
    expect(value('HEAD_POSE_ROLL')).toContain('> 0 = clockwise in the image');
  });
});

import { existsSync, readdirSync, readFileSync } from 'node:fs';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';
import { readSdkFile } from './helpers';

interface PackageJson {
  name?: string;
  version?: string;
  license?: string | { type?: string };
  dependencies?: Record<string, string>;
}

const pkg = JSON.parse(readSdkFile('package.json')) as PackageJson;
const nodeModules = fileURLToPath(new URL('../node_modules', import.meta.url));

describe('runtime dependency policy', () => {
  it('has exactly one runtime dependency, @mediapipe/tasks-vision, pinned to an exact version', () => {
    expect(Object.keys(pkg.dependencies ?? {})).toEqual(['@mediapipe/tasks-vision']);
    expect(pkg.dependencies?.['@mediapipe/tasks-vision']).toMatch(/^\d+\.\d+\.\d+$/);
  });

  it('installs the pinned, Apache-2.0 MediaPipe release', () => {
    const installed = JSON.parse(readSdkFile('node_modules/@mediapipe/tasks-vision/package.json')) as PackageJson;
    expect(installed.version).toBe(pkg.dependencies?.['@mediapipe/tasks-vision']);
    expect(installed.license).toBe('Apache-2.0');
  });

  it('uses a MediaPipe build without usage telemetry (MediaPipe 1.0+ posts metrics to Google)', () => {
    const bundle = readSdkFile('node_modules/@mediapipe/tasks-vision/vision_bundle.mjs');
    expect(bundle).not.toContain('odml.pa.googleapis.com');
    expect(bundle).not.toContain('_mediapipeLoggerGetEncodedApiKey');
    expect(bundle.match(/https?:\/\/[^\s"'`]+/g) ?? []).toEqual([]);
  });
});

describe('licences of the installed tree (dev tooling included)', () => {
  const forbidden = /\b(A?GPL|LGPL|SSPL)\b|Commons-Clause|non-?commercial/i;

  function packages(dir: string): string[] {
    const found: string[] = [];
    if (!existsSync(dir)) return found;
    for (const entry of readdirSync(dir, { withFileTypes: true })) {
      if (!entry.isDirectory() || entry.name.startsWith('.')) continue;
      const path = join(dir, entry.name);
      if (entry.name.startsWith('@')) {
        found.push(...packages(path));
        continue;
      }
      if (existsSync(join(path, 'package.json'))) found.push(path);
      found.push(...packages(join(path, 'node_modules')));
    }
    return found;
  }

  it('contains no copyleft or non-commercial licence', () => {
    const offenders: string[] = [];
    for (const path of packages(nodeModules)) {
      const manifest = JSON.parse(readFileSync(join(path, 'package.json'), 'utf8')) as PackageJson;
      const license = typeof manifest.license === 'string' ? manifest.license : (manifest.license?.type ?? '');
      if (forbidden.test(license)) offenders.push(`${manifest.name ?? path}@${manifest.version ?? '?'}: ${license}`);
    }
    expect(offenders).toEqual([]);
  });
});

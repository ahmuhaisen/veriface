import { describe, expect, it } from 'vitest';
import { DOCUMENT_GUIDANCE_CODES, SELFIE_GUIDANCE_CODES } from '../src/guidance/types';
import {
  documentMessageKey,
  MESSAGES,
  resolveLocale,
  selfieMessageKey,
  textDirection,
  translate,
  type MessageKey,
} from '../src/i18n';

const en = MESSAGES.en;
const ar = MESSAGES.ar;
const ARABIC = /[؀-ۿ]/;

describe('messages', () => {
  it('English and Arabic define exactly the same keys', () => {
    expect(Object.keys(ar).sort()).toEqual(Object.keys(en).sort());
  });

  it('has no empty or untranslated Arabic strings', () => {
    for (const key of Object.keys(en) as MessageKey[]) {
      expect(en[key].trim(), key).not.toBe('');
      expect(ar[key].trim(), key).not.toBe('');
      expect(ARABIC.test(ar[key]), `${key} is not Arabic`).toBe(true);
      expect(ar[key], key).not.toBe(en[key]);
    }
  });

  it('keeps placeholders in step between languages', () => {
    const placeholders = (text: string) => (text.match(/\{[^}]+\}/g) ?? []).sort();
    for (const key of Object.keys(en) as MessageKey[]) {
      expect(placeholders(ar[key]), key).toEqual(placeholders(en[key]));
    }
  });

  it('covers every guidance code in both languages', () => {
    for (const code of SELFIE_GUIDANCE_CODES) {
      expect(en[selfieMessageKey(code)]).toBeTruthy();
      expect(ar[selfieMessageKey(code)]).toBeTruthy();
    }
    for (const code of DOCUMENT_GUIDANCE_CODES) {
      expect(en[documentMessageKey(code)]).toBeTruthy();
      expect(ar[documentMessageKey(code)]).toBeTruthy();
    }
  });
});

describe('locale handling', () => {
  it.each([
    ['ar', 'ar'],
    ['ar-AE', 'ar'],
    ['AR_ae', 'ar'],
    ['en', 'en'],
    ['en-GB', 'en'],
    ['fr', 'en'],
    ['', 'en'],
    [undefined, 'en'],
    [null, 'en'],
  ] as const)('resolveLocale(%s) = %s', (input, expected) => {
    expect(resolveLocale(input)).toBe(expected);
  });

  it('is right-to-left for Arabic', () => {
    expect(textDirection('ar')).toBe('rtl');
    expect(textDirection('en')).toBe('ltr');
  });

  it('prefers host overrides', () => {
    expect(translate('selfie.TOO_FAR', 'en')).toBe(en['selfie.TOO_FAR']);
    expect(translate('selfie.TOO_FAR', 'ar', { 'selfie.TOO_FAR': 'Custom' })).toBe('Custom');
    expect(translate('selfie.TOO_CLOSE', 'ar', { 'selfie.TOO_FAR': 'Custom' })).toBe(ar['selfie.TOO_CLOSE']);
  });
});

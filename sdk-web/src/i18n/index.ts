import type { DocumentGuidanceCode, SelfieGuidanceCode } from '../guidance/types';
import { ar } from './ar';
import { en } from './en';
import type { MessageKey, Messages } from './keys';

export type { MessageKey, Messages, UiMessageKey } from './keys';

export type Locale = 'en' | 'ar';

/** Built-in strings. Both languages define exactly the same keys (enforced by the type and a test). */
export const MESSAGES: Readonly<Record<Locale, Messages>> = { en, ar };

/** Picks a supported locale from a BCP 47 tag such as `ar-AE`; anything else falls back to English. */
export function resolveLocale(lang: string | null | undefined): Locale {
  return lang?.trim().toLowerCase().split(/[-_]/)[0] === 'ar' ? 'ar' : 'en';
}

export function textDirection(locale: Locale): 'ltr' | 'rtl' {
  return locale === 'ar' ? 'rtl' : 'ltr';
}

/** Looks up a message; host overrides win over the built-in strings. */
export function translate(key: MessageKey, locale: Locale = 'en', overrides?: Partial<Messages>): string {
  return overrides?.[key] ?? MESSAGES[locale][key];
}

export function selfieMessageKey(code: SelfieGuidanceCode): MessageKey {
  return `selfie.${code}`;
}

export function documentMessageKey(code: DocumentGuidanceCode): MessageKey {
  return `document.${code}`;
}

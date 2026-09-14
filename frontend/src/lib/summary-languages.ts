export interface LanguageOption {
  code: string;
}

/**
 * Language options offered in the summary language pickers.
 * Codes must stay in sync with `language_name_from_code` in
 * `frontend/src-tauri/src/summary/processor.rs`.
 */
export const LANGUAGE_OPTIONS: LanguageOption[] = [
  { code: 'en' },
  { code: 'zh' },
  { code: 'zh-tw' },
  { code: 'de' },
  { code: 'es' },
  { code: 'ru' },
  { code: 'ko' },
  { code: 'fr' },
  { code: 'ja' },
  { code: 'pt' },
  { code: 'it' },
  { code: 'nl' },
  { code: 'pl' },
  { code: 'ar' },
  { code: 'hi' },
  { code: 'ta' },
  { code: 'tr' },
  { code: 'vi' },
  { code: 'th' },
  { code: 'id' },
  { code: 'sv' },
  { code: 'cs' },
  { code: 'da' },
  { code: 'fi' },
  { code: 'el' },
  { code: 'he' },
  { code: 'hu' },
  { code: 'no' },
  { code: 'ro' },
  { code: 'uk' },
];

export const AUTO_VALUE = '__auto__' as const;

const SUPPORTED_CODES: ReadonlySet<string> = new Set(LANGUAGE_OPTIONS.map((o) => o.code));

/**
 * Normalises a raw locale string (from transcription or storage) into a code we
 * can translate into. Handles BCP-47 regional tags: `pt-BR` -> `pt`, `en_GB` -> `en`.
 * Returns null for unsupported languages so callers can fall back to English
 * rather than sending a code Rust will silently drop.
 */
export function normaliseLanguageCode(raw: string | null | undefined): string | null {
  if (!raw) return null;
  const lower = raw.toLowerCase().replace(/_/g, '-');
  if (SUPPORTED_CODES.has(lower)) return lower;
  const base = lower.split('-')[0];
  if (SUPPORTED_CODES.has(base)) return base;
  return null;
}

export function localizedLabelForCode(code: string, locale: string): string {
  try {
    return new Intl.DisplayNames([locale], { type: 'language' }).of(code) ?? code;
  } catch {
    return code;
  }
}

export function labelForCode(code: string, locale = 'en'): string {
  return localizedLabelForCode(code, locale);
}

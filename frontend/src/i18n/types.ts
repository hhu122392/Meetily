export const UI_LOCALE_STORAGE_KEY = "meetily.uiLocale" as const;

export const UI_LOCALE_PREFERENCES = ["system", "en", "zh-CN"] as const;
export type UiLocalePreference = (typeof UI_LOCALE_PREFERENCES)[number];

export const SUPPORTED_UI_LOCALES = ["en", "zh-CN"] as const;
export type SupportedUiLocale = (typeof SUPPORTED_UI_LOCALES)[number];

export const I18N_NAMESPACES = [
  "analytics",
  "common",
  "import",
  "meetings",
  "moss",
  "models",
  "navigation",
  "onboarding",
  "recording",
  "settings",
  "summary",
  "templates",
  "transcription",
  "updates",
] as const;

export type I18nNamespace = (typeof I18N_NAMESPACES)[number];

export interface UiLocaleBootstrapState {
  preference: UiLocalePreference;
  locale: SupportedUiLocale;
}

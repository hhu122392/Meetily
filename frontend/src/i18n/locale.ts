import {
  UI_LOCALE_PREFERENCES,
  UI_LOCALE_STORAGE_KEY,
  type SupportedUiLocale,
  type UiLocaleBootstrapState,
  type UiLocalePreference,
} from "./types";

type ReadableStorage = Pick<Storage, "getItem">;
type WritableStorage = Pick<Storage, "setItem" | "removeItem">;

export function isUiLocalePreference(value: unknown): value is UiLocalePreference {
  return (
    typeof value === "string" &&
    UI_LOCALE_PREFERENCES.includes(value as UiLocalePreference)
  );
}
export function matchSupportedLocale(
  requestedLocales: readonly string[] | null | undefined,
): SupportedUiLocale {
  for (const requestedLocale of requestedLocales ?? []) {
    const locale = requestedLocale.trim().replaceAll("_", "-").toLowerCase();

    if (locale === "en" || locale.startsWith("en-")) {
      return "en";
    }

    if (
      locale === "zh-cn" ||
      locale === "zh-sg" ||
      locale === "zh-hans" ||
      locale.startsWith("zh-hans-")
    ) {
      return "zh-CN";
    }
  }

  return "en";
}

export function resolveUiLocale(
  preference: UiLocalePreference | string | null | undefined,
  systemLocales: readonly string[] | null | undefined = [],
): SupportedUiLocale {
  if (preference === "en" || preference === "zh-CN") {
    return preference;
  }

  return matchSupportedLocale(systemLocales);
}

export function readUiLocalePreference(
  storage: ReadableStorage | null | undefined = getBrowserStorage(),
): UiLocalePreference {
  if (!storage) {
    return "system";
  }

  try {
    const storedValue = storage.getItem(UI_LOCALE_STORAGE_KEY);
    return isUiLocalePreference(storedValue) ? storedValue : "system";
  } catch {
    return "system";
  }
}

export function writeUiLocalePreference(
  preference: UiLocalePreference,
  storage: WritableStorage | null | undefined = getBrowserStorage(),
): void {
  if (!storage) {
    return;
  }

  try {
    storage.setItem(UI_LOCALE_STORAGE_KEY, preference);
  } catch {
    // A denied or full storage backend must not prevent the UI from switching.
  }
}

export function getSystemLocales(
  browserNavigator: Pick<Navigator, "language" | "languages"> | null | undefined =
    typeof navigator === "undefined" ? null : navigator,
): string[] {
  if (!browserNavigator) {
    return [];
  }

  return Array.from(
    new Set(
      [...(browserNavigator.languages ?? []), browserNavigator.language].filter(
        (locale): locale is string => Boolean(locale),
      ),
    ),
  );
}

export function getInitialUiLocaleState(): UiLocaleBootstrapState {
  if (typeof window !== "undefined" && window.__MEETILY_UI_LOCALE_BOOTSTRAP__) {
    return window.__MEETILY_UI_LOCALE_BOOTSTRAP__;
  }

  const preference = readUiLocalePreference();
  return {
    preference,
    locale: resolveUiLocale(preference, getSystemLocales()),
  };
}

export function applyDocumentLocale(
  locale: SupportedUiLocale,
  documentElement: Pick<HTMLElement, "lang" | "dir"> | null | undefined =
    typeof document === "undefined" ? null : document.documentElement,
): void {
  if (!documentElement) {
    return;
  }

  documentElement.lang = locale;
  documentElement.dir = "ltr";
}

function getBrowserStorage(): Storage | null {
  if (typeof window === "undefined") {
    return null;
  }

  try {
    return window.localStorage;
  } catch {
    return null;
  }
}

// Runs before React hydration. It only establishes document metadata and a
// bootstrap value; translated React content remains hidden until i18next is ready.
export const UI_LOCALE_BOOTSTRAP_SCRIPT = `(() => {
  const key = ${JSON.stringify(UI_LOCALE_STORAGE_KEY)};
  const valid = new Set(["system", "en", "zh-CN"]);
  let preference = "system";
  try {
    const stored = window.localStorage.getItem(key);
    if (valid.has(stored)) preference = stored;
  } catch (_) {}

  const requested = preference === "system"
    ? [...(window.navigator.languages || []), window.navigator.language].filter(Boolean)
    : [preference];
  let locale = "en";
  for (const value of requested) {
    const normalized = String(value).trim().replaceAll("_", "-").toLowerCase();
    if (normalized === "en" || normalized.startsWith("en-")) {
      locale = "en";
      break;
    }
    if (
      normalized === "zh-cn" ||
      normalized === "zh-sg" ||
      normalized === "zh-hans" ||
      normalized.startsWith("zh-hans-")
    ) {
      locale = "zh-CN";
      break;
    }
  }

  document.documentElement.lang = locale;
  document.documentElement.dir = "ltr";
  window.__MEETILY_UI_LOCALE_BOOTSTRAP__ = { preference, locale };
})();`;

declare global {
  interface Window {
    __MEETILY_UI_LOCALE_BOOTSTRAP__?: UiLocaleBootstrapState;
  }
}

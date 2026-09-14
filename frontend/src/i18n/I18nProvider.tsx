"use client";

import {
  default as React,
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useState,
  type ReactNode,
} from "react";
import { I18nextProvider } from "react-i18next";
import { i18n } from "./index";
import {
  applyDocumentLocale,
  getSystemLocales,
  readUiLocalePreference,
  resolveUiLocale,
  writeUiLocalePreference,
} from "./locale";
import {
  UI_LOCALE_STORAGE_KEY,
  type SupportedUiLocale,
  type UiLocalePreference,
} from "./types";
import { syncNativeUiLocale } from "@/lib/native-i18n";

interface I18nContextValue {
  isReady: boolean;
  locale: SupportedUiLocale;
  preference: UiLocalePreference;
  setPreference: (preference: UiLocalePreference) => Promise<void>;
}

const MeetilyI18nContext = createContext<I18nContextValue | null>(null);

export function MeetilyI18nProvider({ children }: { children: ReactNode }) {
  // Keep server and first client render identical. The pre-hydration script
  // updates only document metadata; actual translations are applied in effect
  // while this provider's visibility gate remains closed.
  const [preference, setPreferenceState] = useState<UiLocalePreference>("system");
  const [locale, setLocale] = useState<SupportedUiLocale>("en");
  const [isReady, setIsReady] = useState(false);

  const activateLocale = useCallback(
    async (nextPreference: UiLocalePreference, persist: boolean) => {
      const nextLocale = resolveUiLocale(nextPreference, getSystemLocales());

      setIsReady(false);

      if (persist) {
        writeUiLocalePreference(nextPreference);
      }

      setPreferenceState(nextPreference);
      setLocale(nextLocale);
      applyDocumentLocale(nextLocale);
      await i18n.changeLanguage(nextLocale);
      await syncNativeUiLocale(nextPreference, nextLocale);
      setIsReady(true);
    },
    [],
  );

  const setPreference = useCallback(
    (nextPreference: UiLocalePreference) => activateLocale(nextPreference, true),
    [activateLocale],
  );

  useEffect(() => {
    let active = true;

    const initialize = async () => {
      const currentPreference = readUiLocalePreference();
      const nextLocale = resolveUiLocale(currentPreference, getSystemLocales());
      applyDocumentLocale(nextLocale);
      await i18n.changeLanguage(nextLocale);
      await syncNativeUiLocale(currentPreference, nextLocale);

      if (active) {
        setPreferenceState(currentPreference);
        setLocale(nextLocale);
        setIsReady(true);
      }
    };

    void initialize();
    return () => {
      active = false;
    };
  }, []);

  useEffect(() => {
    const handleSystemLanguageChange = () => {
      if (preference === "system") {
        void activateLocale("system", false);
      }
    };

    const handleStorageChange = (event: StorageEvent) => {
      if (event.key === UI_LOCALE_STORAGE_KEY) {
        void activateLocale(readUiLocalePreference(), false);
      }
    };

    window.addEventListener("languagechange", handleSystemLanguageChange);
    window.addEventListener("storage", handleStorageChange);
    return () => {
      window.removeEventListener("languagechange", handleSystemLanguageChange);
      window.removeEventListener("storage", handleStorageChange);
    };
  }, [activateLocale, preference]);

  const value = useMemo<I18nContextValue>(
    () => ({ isReady, locale, preference, setPreference }),
    [isReady, locale, preference, setPreference],
  );

  return (
    <I18nextProvider i18n={i18n}>
      <MeetilyI18nContext.Provider value={value}>
        <div
          data-i18n-ready={isReady ? "true" : "false"}
          style={{ visibility: isReady ? "visible" : "hidden" }}
        >
          {children}
        </div>
      </MeetilyI18nContext.Provider>
    </I18nextProvider>
  );
}

export function useMeetilyI18n(): I18nContextValue {
  const context = useContext(MeetilyI18nContext);
  if (!context) {
    throw new Error("useMeetilyI18n must be used within MeetilyI18nProvider");
  }

  return context;
}

import i18n from "i18next";
import { initReactI18next } from "react-i18next";
import { resources } from "./resources";
import { I18N_NAMESPACES, SUPPORTED_UI_LOCALES } from "./types";

export function reportMissingKey(namespace: string, key: string): void {
  if (process.env.NODE_ENV === "development") {
    console.warn(`[i18n] Missing translation key: ${namespace}:${key}`);
  }
}

if (!i18n.isInitialized) {
  void i18n.use(initReactI18next).init({
    resources,
    lng: "en",
    fallbackLng: "en",
    supportedLngs: [...SUPPORTED_UI_LOCALES],
    ns: [...I18N_NAMESPACES],
    defaultNS: "common",
    fallbackNS: "common",
    load: "currentOnly",
    interpolation: {
      escapeValue: false,
    },
    react: {
      useSuspense: false,
    },
    saveMissing: process.env.NODE_ENV === "development",
    missingKeyHandler: (_languages, namespace, key) => reportMissingKey(namespace, key),
    initAsync: false,
    returnEmptyString: false,
  });
}

export { i18n };
export * from "./formatters";
export * from "./locale";
export * from "./types";

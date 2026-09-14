"use client";

import { HelpHint } from "./ui/help-hint";
import { Languages } from "lucide-react";
import { useState } from "react";
import { useTranslation } from "react-i18next";
import { useMeetilyI18n } from "@/i18n/I18nProvider";
import type { UiLocalePreference } from "@/i18n/types";

export function DisplayLanguageSettings() {
  const { t } = useTranslation("settings");
  const { locale, preference, setPreference } = useMeetilyI18n();
  const [isApplying, setIsApplying] = useState(false);
  const resolvedLocaleLabel =
    locale === "zh-CN"
      ? t("displayLanguage.resolvedLocales.zhCN")
      : t("displayLanguage.resolvedLocales.en");

  const handleChange = async (nextPreference: UiLocalePreference) => {
    setIsApplying(true);
    try {
      await setPreference(nextPreference);
    } finally {
      setIsApplying(false);
    }
  };

  return (
    <section
      className="bg-white rounded-lg border border-gray-200 p-6 shadow-sm"
      aria-labelledby="display-language-heading"
    >
      <div className="flex flex-wrap items-start justify-between gap-4">
        <div className="flex gap-3">
          <Languages className="mt-0.5 h-5 w-5 shrink-0 text-blue-600" aria-hidden="true" />
          <div>
            <h3 id="display-language-heading" className="flex items-center gap-1 text-base font-semibold text-gray-900">
              {t("displayLanguage.title")}<HelpHint><p>{t("displayLanguage.description")}</p><p>{t("displayLanguage.restartNotRequired")}</p></HelpHint>
            </h3>
          </div>
        </div>

        <select
          aria-label={t("displayLanguage.title")}
          className="min-w-48 rounded-md border border-gray-300 bg-white px-3 py-2 text-sm text-gray-900 shadow-sm focus:border-blue-500 focus:outline-none focus:ring-2 focus:ring-blue-200 disabled:cursor-wait disabled:opacity-60"
          value={preference}
          disabled={isApplying}
          onChange={(event) => {
            void handleChange(event.target.value as UiLocalePreference);
          }}
        >
          <option value="system">{t("displayLanguage.options.system")}</option>
          <option value="en">{t("displayLanguage.options.en")}</option>
          <option value="zh-CN">{t("displayLanguage.options.zhCN")}</option>
        </select>
      </div>

      <div className="mt-2 text-xs text-gray-500">
        <p>{t("displayLanguage.activeLocale", { locale: resolvedLocaleLabel })}</p>
      </div>
    </section>
  );
}

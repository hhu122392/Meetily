import assert from "node:assert/strict";
import test from "node:test";
import vm from "node:vm";
import i18next from "i18next";
import {
  applyDocumentLocale,
  getSystemLocales,
  isUiLocalePreference,
  matchSupportedLocale,
  readUiLocalePreference,
  resolveUiLocale,
  UI_LOCALE_BOOTSTRAP_SCRIPT,
  writeUiLocalePreference,
} from "../../src/i18n/locale";
import {
  formatDateTime,
  formatLanguageName,
  formatNumber,
  formatPercent,
  formatRelativeTime,
} from "../../src/i18n/formatters";
import { resources } from "../../src/i18n/resources";
import {
  I18N_NAMESPACES,
  UI_LOCALE_STORAGE_KEY,
} from "../../src/i18n/types";
import { reportMissingKey } from "../../src/i18n/index";

class MemoryStorage {
  private readonly values = new Map<string, string>();

  getItem(key: string): string | null {
    return this.values.get(key) ?? null;
  }

  setItem(key: string, value: string): void {
    this.values.set(key, value);
  }

  removeItem(key: string): void {
    this.values.delete(key);
  }
}

test("validates only the three public UI locale preferences", () => {
  assert.equal(isUiLocalePreference("system"), true);
  assert.equal(isUiLocalePreference("en"), true);
  assert.equal(isUiLocalePreference("zh-CN"), true);
  assert.equal(isUiLocalePreference("zh-TW"), false);
  assert.equal(isUiLocalePreference("unknown"), false);
  assert.equal(isUiLocalePreference(null), false);
});

test("resolves explicit, system, regional, and unknown locales deterministically", () => {
  assert.equal(resolveUiLocale("en", ["zh-CN"]), "en");
  assert.equal(resolveUiLocale("zh-CN", ["en-US"]), "zh-CN");
  assert.equal(resolveUiLocale("system", ["zh-CN"]), "zh-CN");
  assert.equal(resolveUiLocale("system", ["zh-Hans-SG"]), "zh-CN");
  assert.equal(resolveUiLocale("system", ["en-US"]), "en");
  assert.equal(resolveUiLocale("system", ["fr-FR", "zh-SG"]), "zh-CN");
  assert.equal(resolveUiLocale("unknown", ["unknown"]), "en");
  assert.equal(matchSupportedLocale(["zh-TW"]), "en");
});

test("collects navigator languages without duplicates", () => {
  assert.deepEqual(
    getSystemLocales({ language: "en-US", languages: ["zh-CN", "en-US"] }),
    ["zh-CN", "en-US"],
  );
  assert.deepEqual(getSystemLocales(null), []);
});

test("persists UI locale under its dedicated key without touching model languages", () => {
  const storage = new MemoryStorage();
  storage.setItem("primaryLanguage", "ja");
  storage.setItem("summaryLanguage", "de");

  writeUiLocalePreference("zh-CN", storage);

  assert.equal(storage.getItem(UI_LOCALE_STORAGE_KEY), "zh-CN");
  assert.equal(readUiLocalePreference(storage), "zh-CN");
  assert.equal(storage.getItem("primaryLanguage"), "ja");
  assert.equal(storage.getItem("summaryLanguage"), "de");
});

test("invalid or unavailable storage safely falls back to system", () => {
  const invalidStorage = new MemoryStorage();
  invalidStorage.setItem(UI_LOCALE_STORAGE_KEY, "pirate");
  assert.equal(readUiLocalePreference(invalidStorage), "system");

  const throwingStorage = {
    getItem(): string | null {
      throw new Error("storage denied");
    },
  };
  assert.equal(readUiLocalePreference(throwingStorage), "system");
});

test("updates document language metadata", () => {
  const documentElement = { lang: "en", dir: "rtl" };
  applyDocumentLocale("zh-CN", documentElement);
  assert.deepEqual(documentElement, { lang: "zh-CN", dir: "ltr" });
});

test("bootstrap script resolves metadata before React and honors persisted preference", () => {
  const context = {
    Set,
    String,
    window: {
      localStorage: { getItem: () => "zh-CN" },
      navigator: { languages: ["en-US"], language: "en-US" },
    },
    document: { documentElement: { lang: "en", dir: "rtl" } },
  };

  vm.runInNewContext(UI_LOCALE_BOOTSTRAP_SCRIPT, context);

  assert.equal(context.document.documentElement.lang, "zh-CN");
  assert.equal(context.document.documentElement.dir, "ltr");
  const bootstrapState = (
    context.window as typeof context.window & {
      __MEETILY_UI_LOCALE_BOOTSTRAP__: unknown;
    }
  ).__MEETILY_UI_LOCALE_BOOTSTRAP__;
  assert.deepEqual(
    JSON.parse(JSON.stringify(bootstrapState)),
    { preference: "zh-CN", locale: "zh-CN" },
  );
});

test("registers every namespace for English and Simplified Chinese", () => {
  for (const locale of ["en", "zh-CN"] as const) {
    assert.deepEqual(Object.keys(resources[locale]).sort(), [...I18N_NAMESPACES].sort());
  }
});

test("completed settings and common keys render in Simplified Chinese", async () => {
  const instance = i18next.createInstance();
  await instance.init({
    resources,
    lng: "zh-CN",
    fallbackLng: "en",
    supportedLngs: ["en", "zh-CN"],
    ns: [...I18N_NAMESPACES],
    defaultNS: "common",
    initAsync: false,
    interpolation: { escapeValue: false },
  });

  assert.equal(instance.t("displayLanguage.title", { ns: "settings" }), "显示语言");
  assert.equal(
    instance.t("labels.dataStorageLocations", { ns: "settings" }),
    "数据存储位置",
  );
  assert.equal(
    instance.t("status.downloadingValue", {
      ns: "common",
      modelName: "Whisper",
    }),
    "正在下载 Whisper",
  );
});

test("development mode reports missing keys without enabling a network backend", () => {
  const mutableEnvironment = process.env as Record<string, string | undefined>;
  const previousNodeEnv = mutableEnvironment.NODE_ENV;
  const previousWarn = console.warn;
  const warnings: string[] = [];
  mutableEnvironment.NODE_ENV = "development";
  console.warn = (message?: unknown) => warnings.push(String(message));

  try {
    reportMissingKey("settings", "missing.example");
    assert.deepEqual(warnings, ["[i18n] Missing translation key: settings:missing.example"]);
  } finally {
    console.warn = previousWarn;
    if (previousNodeEnv === undefined) delete mutableEnvironment.NODE_ENV;
    else mutableEnvironment.NODE_ENV = previousNodeEnv;
  }
});

test("Intl formatters are locale-aware and deterministic", () => {
  const timestamp = Date.UTC(2026, 7, 23, 8, 30, 0);
  const dateOptions: Intl.DateTimeFormatOptions = {
    year: "numeric",
    month: "2-digit",
    day: "2-digit",
    timeZone: "UTC",
  };

  assert.notEqual(
    formatDateTime(timestamp, "en", dateOptions),
    formatDateTime(timestamp, "zh-CN", dateOptions),
  );
  assert.equal(formatNumber(1234567.89, "en"), "1,234,567.89");
  assert.equal(formatNumber(1234567.89, "zh-CN"), "1,234,567.89");
  assert.equal(formatPercent(0.42, "en"), "42%");
  assert.equal(formatRelativeTime(-1, "day", "en"), "yesterday");
  assert.equal(formatRelativeTime(-1, "day", "zh-CN"), "昨天");
  assert.equal(formatLanguageName("zh-CN", "en"), "Chinese (China)");
  assert.equal(formatLanguageName("en", "zh-CN"), "英语");
});

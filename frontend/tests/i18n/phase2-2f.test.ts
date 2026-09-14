import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import test from "node:test";
import i18next from "i18next";
import { resources } from "../../src/i18n/resources";

const repositoryRoot = path.resolve(process.cwd(), "..");
const namespaces = ["updates", "analytics", "common"] as const;

function read(relativePath: string): string {
  return fs.readFileSync(path.join(repositoryRoot, relativePath), "utf8");
}

function runPhase2Audit(batch: "2E" | "2F") {
  const temporaryDirectory = fs.mkdtempSync(path.join(os.tmpdir(), "meetily-phase2-audit-"));
  const reportPath = path.join(temporaryDirectory, "resource-and-source-audit.json");
  try {
    execFileSync(
      process.execPath,
      [
        path.join(repositoryRoot, "docs/i18n/scripts/audit-phase2.mjs"),
        repositoryRoot,
        `--batch=${batch}`,
        `--report=${reportPath}`,
      ],
      { cwd: repositoryRoot, stdio: "pipe" },
    );
    return JSON.parse(fs.readFileSync(reportPath, "utf8"));
  } finally {
    fs.rmSync(temporaryDirectory, { recursive: true, force: true });
  }
}

function flatten(value: Record<string, unknown>, prefix = "", result = new Map<string, string>()) {
  for (const [key, child] of Object.entries(value)) {
    const next = prefix ? `${prefix}.${key}` : key;
    if (child && typeof child === "object" && !Array.isArray(child)) {
      flatten(child as Record<string, unknown>, next, result);
    } else {
      assert.equal(typeof child, "string", `${next} must be a string`);
      result.set(next, child as string);
    }
  }
  return result;
}

function placeholders(value: string): string[] {
  return [...value.matchAll(/{{\s*([^},\s]+)[^}]*}}/g)].map((match) => match[1]).sort();
}

test("phase 2F resources are symmetric, translated, and placeholder-safe", () => {
  const minimums = { updates: 34, analytics: 63, common: 75 };
  for (const namespace of namespaces) {
    const en = flatten(resources.en[namespace]);
    const zhCN = flatten(resources["zh-CN"][namespace]);
    assert.deepEqual([...zhCN.keys()].sort(), [...en.keys()].sort(), namespace);
    assert.ok(en.size >= minimums[namespace], `${namespace} coverage`);
    for (const [key, enValue] of en) {
      const zhValue = zhCN.get(key);
      assert.ok(zhValue?.trim(), `${namespace}:${key} must have a Chinese value`);
      assert.deepEqual(placeholders(zhValue!), placeholders(enValue), `${namespace}:${key} placeholders`);
      assert.doesNotMatch(zhValue!, /\b(?:TODO|TBD|TRANSLATE_ME)\b/i);
    }
  }
});

test("phase 2F preserves every frozen English baseline key and value", () => {
  for (const namespace of namespaces) {
    const baseline = flatten(JSON.parse(read(`docs/i18n/baseline/locales/en/${namespace}.json`)));
    const current = flatten(resources.en[namespace]);
    for (const [key, value] of baseline) {
      assert.equal(current.get(key), value, `${namespace}:${key}`);
    }
  }
});

test("updates, analytics, About, and Beta switch locale without mutating business state", async () => {
  const instance = i18next.createInstance();
  await instance.init({
    resources,
    lng: "en",
    fallbackLng: "en",
    supportedLngs: ["en", "zh-CN"],
    ns: [...namespaces],
    defaultNS: "common",
    initAsync: false,
    interpolation: { escapeValue: false },
  });
  const t = instance.t.bind(instance) as (key: string, options?: Record<string, unknown>) => string;
  const state = { version: "9.8.7", optedIn: true, userId: "audit-user-id", betaEnabled: false, downloadProgress: 42 };
  assert.equal(t("updates:messages.newVersionAvailable", { version: state.version }), "A new version (9.8.7) is available");
  assert.equal(t("analytics:labels.whatAnalyticsCollects"), "What Analytics Collects");
  assert.equal(t("common:labels.betaFeatures"), "Beta Features");
  await instance.changeLanguage("zh-CN");
  assert.equal(t("updates:messages.newVersionAvailable", { version: state.version }), "发现新版本（9.8.7）");
  assert.equal(t("analytics:labels.whatAnalyticsCollects"), "分析功能会收集什么");
  assert.equal(t("common:labels.betaFeatures"), "Beta 功能");
  assert.deepEqual(state, { version: "9.8.7", optedIn: true, userId: "audit-user-id", betaEnabled: false, downloadProgress: 42 });
});

test("update and analytics failures use controlled copy and restore state", () => {
  const about = read("frontend/src/components/About.tsx");
  const updateDialog = read("frontend/src/components/UpdateDialog.tsx");
  const analytics = read("frontend/src/components/AnalyticsConsentSwitch.tsx");
  assert.doesNotMatch(about, /toast\.error\([^\n]*(?:error\.message|String\(error)/);
  assert.doesNotMatch(updateDialog, /setError\((?:err|error)(?:\.message)?\b/);
  assert.doesNotMatch(updateDialog, /toast\.error\([^\n]*(?:err\.message|String\(err)/);
  assert.match(updateDialog, /type UpdateErrorCode = [^;]+;/);
  assert.match(updateDialog, /setError\('prepareFailed'\)/);
  assert.match(updateDialog, /setError\('downloadInstallFailed'\)/);
  assert.match(analytics, /rollbackStore\.set\('analyticsOptedIn', !enabled\)/);
  assert.match(analytics, /await Analytics\.disable\(\)/);
  assert.match(analytics, /setIsAnalyticsOptedIn\(!enabled\);[\s\S]*?errors\.updatePreferenceFailed/);
  assert.doesNotMatch(analytics, /toast\.error\([^\n]*(?:error\.message|String\(error)/);
});

test("Beta control flow uses stable keys instead of translated display strings", () => {
  const types = read("frontend/src/types/betaFeatures.ts");
  const settings = read("frontend/src/components/BetaSettings.tsx");
  assert.match(types, /BETA_FEATURE_I18N_KEYS/);
  assert.match(types, /nameKey: 'labels\.importAndRetranscribe'/);
  assert.doesNotMatch(types, /BETA_FEATURE_NAMES|BETA_FEATURE_DESCRIPTIONS/);
  assert.match(settings, /checked=\{betaFeatures\[featureKey\]\}/);
  assert.match(settings, /toggleBetaFeature\(featureKey, checked\)/);
  assert.match(settings, /aria-label=\{t\(BETA_FEATURE_I18N_KEYS\[featureKey\]\.nameKey\)\}/);
});

test("analytics remains explicit opt-in and exposes accessible transparency controls", () => {
  const provider = read("frontend/src/components/AnalyticsProvider.tsx");
  const consent = read("frontend/src/components/AnalyticsConsentSwitch.tsx");
  const modal = read("frontend/src/components/AnalyticsDataModal.tsx");
  assert.match(provider, /analyticsOptedIn:\s*false/);
  assert.match(provider, /set\('analyticsOptedIn', false\)/);
  assert.match(consent, /aria-label=\{t\('accessibility\.toggleAnalytics'\)\}/);
  assert.match(modal, /role="dialog"/);
  assert.match(modal, /aria-modal="true"/);
  assert.match(modal, /aria-labelledby="analytics-transparency-title"/);
  assert.match(modal, /aria-label=\{t\('accessibility\.closeTransparencyDialog'\)\}/);
});

test("update formatting follows the active locale and download state cannot be dismissed", () => {
  const dialog = read("frontend/src/components/UpdateDialog.tsx");
  assert.match(dialog, /Intl\.DateTimeFormat\(i18n\.resolvedLanguage \|\| i18n\.language/);
  assert.match(dialog, /Intl\.NumberFormat\(locale/);
  assert.match(dialog, /if \(!newOpen && isDownloading\) \{\s*return;/);
  assert.match(dialog, /onEscapeKeyDown=\{handleEscapeKeyDown\}/);
  assert.match(dialog, /onInteractOutside=\{handleInteractOutside\}/);
});

test("phase 2F machine audit is clean and exemptions are limited to brand and technical sample data", () => {
  const audit = runPhase2Audit("2F");
  assert.equal(audit.batch, "2F");
  assert.deepEqual(audit.summary, { resourceFailures: 0, sourceFailures: 0, totalFailures: 0, passed: true });
  const allowlist = JSON.parse(read("docs/i18n/phase-2/untranslated-allowlist.json"));
  assert.deepEqual(allowlist.resourceKeys, []);
  assert.deepEqual(allowlist.sourceFindings, [
    "frontend/src/components/AnalyticsDataModal.tsx:133:jsx-expression",
    "frontend/src/components/Logo.tsx:12:jsx-attribute:alt",
    "frontend/src/components/Logo.tsx:19:jsx-text",
  ]);
});

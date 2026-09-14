#!/usr/bin/env node

import crypto from "node:crypto";
import fs from "node:fs/promises";
import path from "node:path";

const root = path.resolve(process.argv[2] || ".");
const outputDirectory = path.join(root, "docs/i18n/audit/phase-5-release");
const sha256 = (content) =>
  crypto.createHash("sha256").update(content).digest("hex").toUpperCase();
const placeholders = (value) =>
  [...String(value).matchAll(/\{\{\s*([A-Za-z0-9_.-]+)\s*\}\}/g)]
    .map((match) => match[1])
    .sort();

async function walk(directory) {
  const entries = await fs.readdir(directory, { withFileTypes: true });
  const files = [];
  for (const entry of entries) {
    const target = path.join(directory, entry.name);
    if (entry.isDirectory()) files.push(...(await walk(target)));
    else if (entry.isFile()) files.push(target);
  }
  return files;
}

function flatten(value, prefix = "", output = new Map()) {
  for (const [key, child] of Object.entries(value)) {
    const next = prefix ? `${prefix}.${key}` : key;
    if (child && typeof child === "object" && !Array.isArray(child)) flatten(child, next, output);
    else output.set(next, child);
  }
  return output;
}

const localeRoot = path.join(root, "frontend/src/i18n/locales");
const englishFiles = (await walk(path.join(localeRoot, "en")))
  .filter((file) => file.endsWith(".json"))
  .map((file) => path.relative(path.join(localeRoot, "en"), file).replaceAll("\\", "/"))
  .sort();
const chineseFiles = (await walk(path.join(localeRoot, "zh-CN")))
  .filter((file) => file.endsWith(".json"))
  .map((file) => path.relative(path.join(localeRoot, "zh-CN"), file).replaceAll("\\", "/"))
  .sort();
const resourceResults = [];
for (const file of englishFiles) {
  const en = flatten(JSON.parse(await fs.readFile(path.join(localeRoot, "en", file), "utf8")));
  const zh = flatten(JSON.parse(await fs.readFile(path.join(localeRoot, "zh-CN", file), "utf8")));
  const enKeys = [...en.keys()].sort();
  const zhKeys = [...zh.keys()].sort();
  const placeholderMismatches = enKeys.filter(
    (key) => JSON.stringify(placeholders(en.get(key))) !== JSON.stringify(placeholders(zh.get(key))),
  );
  resourceResults.push({
    file,
    keys: enKeys.length,
    keyParity: JSON.stringify(enKeys) === JSON.stringify(zhKeys),
    nonEmpty: [...en.values(), ...zh.values()].every(
      (value) => typeof value === "string" && value.trim().length > 0,
    ),
    placeholderMismatches,
    pendingMarkers: [...zh.entries()]
      .filter(([, value]) => /TODO|TBD|TRANSLATE_ME|待翻|待译/i.test(String(value)))
      .map(([key]) => key),
  });
}

const nativeEn = flatten(
  JSON.parse(await fs.readFile(path.join(root, "frontend/src-tauri/src/i18n/locales/en.json"), "utf8")),
);
const nativeZh = flatten(
  JSON.parse(await fs.readFile(path.join(root, "frontend/src-tauri/src/i18n/locales/zh-CN.json"), "utf8")),
);
const templateAudit = JSON.parse(
  await fs.readFile(path.join(root, "docs/i18n/audit/phase-4-content/static-audit.json"), "utf8"),
);
const configText = await fs.readFile(path.join(root, "frontend/src-tauri/tauri.conf.json"), "utf8");
const config = JSON.parse(configText);
const hook = await fs.readFile(
  path.join(root, "frontend/src-tauri/scripts/nsis-installer-hooks.nsh"),
  "utf8",
);
const runtimePath = path.join(outputDirectory, "runtime/runtime-audit.json");
const runtime = await fs.readFile(runtimePath, "utf8").then(JSON.parse).catch(() => null);
const requiredDocuments = [
  "docs/i18n/phase-5/rollback-runbook.md",
  "docs/i18n/phase-5/platform-test-matrix.md",
  "docs/i18n/phase-5/privacy-legal-review.md",
  "docs/i18n/phase-5/known-issues-and-risk-acceptance.md",
  "docs/i18n/phase-5/end-to-end-regression-report.md",
  "docs/i18n/phase-5/visual-accessibility-report.md",
  "docs/i18n/phase-5/windows-install-upgrade-uninstall-report.md",
  "docs/i18n/phase-5/performance-stability-report.md",
  "docs/i18n/phase-5/phase-5-final-release-audit.md",
];
const documentStats = await Promise.all(
  requiredDocuments.map(async (file) => ({ file, bytes: (await fs.stat(path.join(root, file))).size })),
);

const nativeKeysEn = [...nativeEn.keys()].sort();
const nativeKeysZh = [...nativeZh.keys()].sort();
const checks = {
  frontendLocaleFileParity: JSON.stringify(englishFiles) === JSON.stringify(chineseFiles),
  frontendKeyParity: resourceResults.every((result) => result.keyParity),
  frontendValuesNonEmpty: resourceResults.every((result) => result.nonEmpty),
  frontendPlaceholdersMatch: resourceResults.every(
    (result) => result.placeholderMismatches.length === 0,
  ),
  noPendingTranslationMarkers: resourceResults.every(
    (result) => result.pendingMarkers.length === 0,
  ),
  nativeKeyParity: JSON.stringify(nativeKeysEn) === JSON.stringify(nativeKeysZh),
  nativeValuesNonEmpty: [...nativeEn.values(), ...nativeZh.values()].every(
    (value) => typeof value === "string" && value.trim().length > 0,
  ),
  templateAuditPasses: templateAudit.passed === true && templateAudit.totals.unresolvedCandidates === 0,
  windowsInstallerIsBilingual:
    config.bundle.windows.nsis.languages.includes("English") &&
    config.bundle.windows.nsis.languages.includes("SimpChinese"),
  downgradeProtectionConfigured:
    config.bundle.windows.allowDowngrades === false &&
    hook.includes("SemverCompare") &&
    hook.includes("SetErrorLevel 3"),
  uninstallDataDeletionGuarded:
    hook.includes("NSIS_HOOK_PREUNINSTALL") &&
    hook.includes("GetFullPathName") &&
    hook.includes("$APPDATA\\${BUNDLEID}") &&
    hook.includes("$APPDATA\\Meetily\\templates") === false,
  requiredDocumentsPresent: documentStats.every((entry) => entry.bytes > 200),
  runtimeAuditPasses: runtime?.passed === true,
};
const report = {
  phase: "15.8-stage-5-chinese-qa-release-and-rollback",
  generatedAt: new Date().toISOString(),
  passed: Object.values(checks).every(Boolean),
  checks,
  totals: {
    frontendNamespaces: englishFiles.length,
    frontendKeys: resourceResults.reduce((sum, result) => sum + result.keys, 0),
    nativeKeys: nativeKeysEn.length,
    templateContentCandidates: templateAudit.totals.contentCandidates,
    unresolvedTemplateContentCandidates: templateAudit.totals.unresolvedCandidates,
  },
  resourceResults,
  requiredDocuments: documentStats,
  fingerprints: {
    tauriConfig: sha256(configText),
    installerHook: sha256(hook),
  },
};
await fs.mkdir(outputDirectory, { recursive: true });
await fs.writeFile(path.join(outputDirectory, "static-audit.json"), JSON.stringify(report, null, 2) + "\n");
process.stdout.write(
  JSON.stringify({
    passed: report.passed,
    checks: Object.keys(checks).length,
    failed: Object.entries(checks).filter(([, value]) => !value).map(([key]) => key),
    totals: report.totals,
  }) + "\n",
);
if (!report.passed) process.exitCode = 1;

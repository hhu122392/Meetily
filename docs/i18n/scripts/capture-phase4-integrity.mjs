#!/usr/bin/env node

import crypto from "node:crypto";
import fs from "node:fs/promises";
import path from "node:path";

const root = path.resolve(process.argv[2] || ".");
const baseline = JSON.parse(
  await fs.readFile(
    path.join(root, "docs/i18n/audit/phase-4-content/pre-edit-baseline.json"),
    "utf8",
  ),
);
const productFiles = [
  "frontend/src-tauri/schemas/template-v2.schema.json",
  "frontend/src-tauri/src/summary/templates/content_locale.rs",
  "frontend/src-tauri/src/summary/templates/defaults.rs",
  "frontend/src-tauri/src/summary/templates/repository.rs",
  "frontend/src-tauri/src/summary/templates/service.rs",
  "frontend/src-tauri/src/summary/template_commands_v2.rs",
  "frontend/src-tauri/src/summary/processor.rs",
  "frontend/src-tauri/tauri.conf.json",
  "frontend/src-tauri/tauri.phase4.conf.json",
  "frontend/src/services/templateService.ts",
  "frontend/src/hooks/meeting-details/useTemplates.ts",
  "frontend/src/hooks/useTemplateLibrary.ts",
  "frontend/src/components/SummaryTemplateSettings.tsx",
  "frontend/src/components/templates/TemplateLibraryPage.tsx",
  ...["en", "zh-CN"].flatMap((locale) =>
    [
      "daily_standup",
      "project_sync",
      "psychatric_session",
      "retrospective",
      "sales_marketing_client_call",
      "standard_meeting",
    ].map((id) => `frontend/src-tauri/templates/${locale}/${id}.json`),
  ),
];
const sha256 = (buffer) =>
  crypto.createHash("sha256").update(buffer).digest("hex").toUpperCase();

async function inspect(relativePath) {
  const absolutePath = path.join(root, relativePath);
  const [content, stat] = await Promise.all([fs.readFile(absolutePath), fs.stat(absolutePath)]);
  return {
    path: relativePath,
    bytes: content.length,
    sha256: sha256(content),
    modifiedAt: stat.mtime.toISOString(),
  };
}

const auditBinary = await inspect("target-phase4-content/release/meetily.exe");
const formalBinary = await inspect("target/release/meetily.exe");
const products = await Promise.all(productFiles.map(inspect));
const assertions = {
  auditBinaryExists: auditBinary.bytes > 0,
  formalBinaryUnchanged:
    formalBinary.bytes === baseline.protectedReleaseBinary.bytes &&
    formalBinary.sha256 === baseline.protectedReleaseBinary.sha256,
  productFilesPredateFinalBinary: products.every(
    (entry) => Date.parse(entry.modifiedAt) <= Date.parse(auditBinary.modifiedAt),
  ),
  expectedIdentifier:
    JSON.parse(
      await fs.readFile(path.join(root, "frontend/src-tauri/tauri.phase4.conf.json"), "utf8"),
    ).identifier === "com.meetily.ai.phase4audit",
  localizedResourcesIncluded:
    products.filter((entry) => /templates\/(en|zh-CN)\//.test(entry.path)).length === 12,
};
const report = {
  phase: "15.7-stage-4-template-and-ai-content-i18n",
  generatedAt: new Date().toISOString(),
  passed: Object.values(assertions).every(Boolean),
  assertions,
  auditBinary,
  formalBinary,
  productFiles: products,
};
const outputPath = path.join(root, "docs/i18n/audit/phase-4-content/post-build-integrity.json");
await fs.writeFile(outputPath, JSON.stringify(report, null, 2) + "\n");
process.stdout.write(JSON.stringify({ outputPath, passed: report.passed, assertions }) + "\n");
if (!report.passed) process.exitCode = 1;

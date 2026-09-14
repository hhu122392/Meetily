#!/usr/bin/env node

import crypto from "node:crypto";
import fs from "node:fs/promises";
import path from "node:path";

const root = path.resolve(process.argv[2] || ".");
const baseline = JSON.parse(
  await fs.readFile(
    path.join(root, "docs/i18n/audit/phase-3-native/pre-edit-baseline.json"),
    "utf8",
  ),
);
const productFiles = [
  "frontend/src-tauri/src/i18n/mod.rs",
  "frontend/src-tauri/src/i18n/error.rs",
  "frontend/src-tauri/src/i18n/locales/en.json",
  "frontend/src-tauri/src/i18n/locales/zh-CN.json",
  "frontend/src-tauri/src/tray.rs",
  "frontend/src-tauri/src/notifications/types.rs",
  "frontend/src-tauri/src/notifications/commands.rs",
  "frontend/src-tauri/src/notifications/manager.rs",
  "frontend/src-tauri/src/notifications/system.rs",
  "frontend/src-tauri/src/lib.rs",
  "frontend/src/i18n/I18nProvider.tsx",
  "frontend/src/lib/native-i18n.ts",
  "frontend/src-tauri/tauri.phase3.conf.json",
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

const auditBinary = await inspect("target-phase3-native/release/meetily.exe");
const formalBinary = await inspect("target/release/meetily.exe");
const products = await Promise.all(productFiles.map(inspect));
const phase3Processes = [];
// Process absence is checked by the PowerShell handoff; Node remains portable
// and records only filesystem integrity here.
const assertions = {
  auditBinaryExists: auditBinary.bytes > 0,
  formalBinaryUnchanged:
    formalBinary.bytes === baseline.protectedReleaseBinary.bytes &&
    formalBinary.sha256 === baseline.protectedReleaseBinary.sha256,
  productFilesPredateFinalBinary: products.every(
    (entry) => Date.parse(entry.modifiedAt) <= Date.parse(auditBinary.modifiedAt),
  ),
  expectedIdentifier:
    JSON.parse(await fs.readFile(path.join(root, "frontend/src-tauri/tauri.phase3.conf.json"), "utf8"))
      .identifier === "com.meetily.ai.phase3audit",
};
const report = {
  phase: "15.6-stage-3-tauri-native-i18n",
  generatedAt: new Date().toISOString(),
  passed: Object.values(assertions).every(Boolean),
  assertions,
  auditBinary,
  formalBinary,
  productFiles: products,
  phase3Processes,
};
const outputPath = path.join(root, "docs/i18n/audit/phase-3-native/post-build-integrity.json");
await fs.writeFile(outputPath, JSON.stringify(report, null, 2) + "\n");
process.stdout.write(JSON.stringify({ outputPath, passed: report.passed, assertions }) + "\n");
if (!report.passed) process.exitCode = 1;

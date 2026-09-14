#!/usr/bin/env node

import crypto from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const scriptDir = path.dirname(fileURLToPath(import.meta.url));
const repoRoot = path.resolve(scriptDir, "../../..");
const sourceI18nRoot = path.join(repoRoot, "docs/i18n");
const deliveryDocsRoot = path.join(repoRoot, "target/release/docs");
const deliveryI18nRoot = path.join(deliveryDocsRoot, "i18n");
const outputPath = path.join(
  deliveryI18nRoot,
  "audit/phase-5a1-source/delivery-integrity.json",
);

function sha256(filePath) {
  return crypto.createHash("sha256").update(fs.readFileSync(filePath)).digest("hex").toUpperCase();
}

function walk(directory) {
  return fs.readdirSync(directory, { withFileTypes: true }).flatMap((entry) => {
    const absolutePath = path.join(directory, entry.name);
    return entry.isDirectory() ? walk(absolutePath) : [absolutePath];
  });
}

const sourceFiles = [
  ...walk(path.join(sourceI18nRoot, "phase-5a1")),
  ...walk(path.join(sourceI18nRoot, "audit/phase-5a1-source")),
  path.join(sourceI18nRoot, "scripts/audit-phase5a1-release-source.mjs"),
  path.join(sourceI18nRoot, "scripts/verify-phase5a1-delivery.mjs"),
].sort((left, right) => left.localeCompare(right));

const comparisons = sourceFiles.map((sourcePath) => {
  const relativePath = path.relative(sourceI18nRoot, sourcePath);
  const deliveryPath = path.join(deliveryI18nRoot, relativePath);
  const delivered = fs.existsSync(deliveryPath);
  const sourceHash = sha256(sourcePath);
  const deliveryHash = delivered ? sha256(deliveryPath) : null;
  return {
    path: relativePath.replaceAll("\\", "/"),
    sourceSha256: sourceHash,
    deliverySha256: deliveryHash,
    matches: delivered && sourceHash === deliveryHash,
  };
});

const sourcePlanPath = path.join(sourceI18nRoot, "i18n-plan.zh-CN.md");
const deliveryPlanPath = path.join(deliveryDocsRoot, "Meetily 多语言架构与中文化实施方案.md");
const formalExePath = path.join(repoRoot, "target/release/meetily.exe");
const formalInstallerPath = path.join(
  repoRoot,
  "target/release/bundle/nsis/meetily_0.4.0_x64-setup.exe",
);
const assertions = {
  everyPhase5A1DocumentDelivered: comparisons.every((entry) => entry.matches),
  topLevelPlanMatchesSource: sha256(sourcePlanPath) === sha256(deliveryPlanPath),
  formalExecutableStillFrozen:
    sha256(formalExePath) === "1873D2CCA9B06EEBE8B2D42DC8530B6888BCB91C4267BBD6E6724F6A78257823",
  formalInstallerStillFrozen:
    sha256(formalInstallerPath) === "C6A5BD12F947AEDECAC890C33A48FDEA94E4C11C29FCA03EEF753C25AEE30434",
};

const report = {
  schemaVersion: 1,
  phase: "5A-1-release-source-consolidation",
  generatedAt: new Date().toISOString(),
  comparedFileCount: comparisons.length,
  assertions,
  passed: Object.values(assertions).every(Boolean),
  mismatches: comparisons.filter((entry) => !entry.matches),
};

fs.mkdirSync(path.dirname(outputPath), { recursive: true });
fs.writeFileSync(outputPath, `${JSON.stringify(report, null, 2)}\n`);
console.log(JSON.stringify({ outputPath, ...report }, null, 2));
if (!report.passed) process.exitCode = 1;

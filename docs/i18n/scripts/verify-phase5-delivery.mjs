import crypto from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const scriptDir = path.dirname(fileURLToPath(import.meta.url));
const repoRoot = path.resolve(scriptDir, "../../..");
const sourceI18nRoot = path.join(repoRoot, "docs/i18n");
const deliveryDocsRoot = path.join(repoRoot, "target/release/docs");
const deliveryI18nRoot = path.join(deliveryDocsRoot, "i18n");
const sourceManifestPath = path.join(
  sourceI18nRoot,
  "audit/phase-5-release/evidence-manifest.json",
);
const deliveryManifestPath = path.join(
  deliveryI18nRoot,
  "audit/phase-5-release/evidence-manifest.json",
);
const outputPath = path.join(
  deliveryI18nRoot,
  "audit/phase-5-release/delivery-integrity.json",
);

const sha256 = (filePath) =>
  crypto.createHash("sha256").update(fs.readFileSync(filePath)).digest("hex").toUpperCase();
const manifest = JSON.parse(fs.readFileSync(sourceManifestPath, "utf8"));
const documentEntries = manifest.evidence.filter((entry) => entry.path.startsWith("docs/i18n/"));

const comparisons = documentEntries.map((entry) => {
  const sourcePath = path.join(repoRoot, entry.path);
  const relativeI18nPath = entry.path.slice("docs/i18n/".length);
  const deliveredPath = path.join(deliveryI18nRoot, relativeI18nPath);
  const deliveredExists = fs.existsSync(deliveredPath);
  const deliveredHash = deliveredExists ? sha256(deliveredPath) : null;
  return {
    path: entry.path,
    expectedSha256: entry.sha256,
    deliveredSha256: deliveredHash,
    matches: deliveredExists && deliveredHash === entry.sha256,
  };
});

const sourcePlanPath = path.join(sourceI18nRoot, "i18n-plan.zh-CN.md");
const topLevelPlanPath = path.join(deliveryDocsRoot, "Meetily 多语言架构与中文化实施方案.md");
const formalExePath = path.join(repoRoot, "target/release/meetily.exe");
const formalInstallerPath = path.join(
  repoRoot,
  "target/release/bundle/nsis/meetily_0.4.0_x64-setup.exe",
);
const assertions = {
  everyManifestDocumentDelivered: comparisons.every((entry) => entry.matches),
  sourceAndDeliveredManifestMatch: sha256(sourceManifestPath) === sha256(deliveryManifestPath),
  topLevelPlanMatchesSource: sha256(sourcePlanPath) === sha256(topLevelPlanPath),
  formalExecutableStillFrozen:
    sha256(formalExePath) === "1873D2CCA9B06EEBE8B2D42DC8530B6888BCB91C4267BBD6E6724F6A78257823",
  formalInstallerStillFrozen:
    sha256(formalInstallerPath) === "C6A5BD12F947AEDECAC890C33A48FDEA94E4C11C29FCA03EEF753C25AEE30434",
};

const report = {
  schemaVersion: 1,
  generatedAt: new Date().toISOString(),
  sourceManifestSha256: sha256(sourceManifestPath),
  deliveredManifestSha256: sha256(deliveryManifestPath),
  comparedDocumentCount: comparisons.length,
  assertions,
  passed: Object.values(assertions).every(Boolean),
  mismatches: comparisons.filter((entry) => !entry.matches),
};

fs.writeFileSync(outputPath, `${JSON.stringify(report, null, 2)}\n`);
console.log(JSON.stringify({ outputPath, ...report }, null, 2));
if (!report.passed) {
  process.exitCode = 1;
}

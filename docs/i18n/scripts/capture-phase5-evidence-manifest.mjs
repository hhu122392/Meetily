import crypto from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import { execFileSync } from "node:child_process";
import { fileURLToPath } from "node:url";

const scriptDir = path.dirname(fileURLToPath(import.meta.url));
const repoRoot = path.resolve(scriptDir, "../../..");
const auditRoot = path.join(repoRoot, "docs/i18n/audit/phase-5-release");
const outputPath = path.join(auditRoot, "evidence-manifest.json");

const protectedArtifacts = [
  "target/release/meetily.exe",
  "target/release/bundle/nsis/meetily_0.4.0_x64-setup.exe",
];
const candidateArtifacts = [
  "target-phase5-release/release/meetily.exe",
  "target-phase5-release/release/bundle/nsis/Meetily Phase 5 Audit_0.4.0_x64-setup.exe",
];
const evidenceRoots = [
  "docs/i18n/phase-5",
  "docs/i18n/audit/phase-5-release",
];
const evidenceFiles = [
  "docs/i18n/i18n-plan.zh-CN.md",
  "docs/i18n/scripts/freeze-phase5-baseline.mjs",
  "docs/i18n/scripts/capture-phase5-release-integrity.mjs",
  "docs/i18n/scripts/audit-phase5-release.mjs",
  "docs/i18n/scripts/runtime-phase5-release-audit.mjs",
  "docs/i18n/scripts/audit-phase5-windows-install.ps1",
  "docs/i18n/scripts/summarize-phase5-regression.mjs",
  "docs/i18n/scripts/audit-phase5-release-gate.mjs",
  "docs/i18n/scripts/capture-phase5-evidence-manifest.mjs",
  "frontend/src-tauri/tauri.phase5.conf.json",
  "frontend/src-tauri/scripts/nsis-installer-hooks.nsh",
];

const sha256 = (absolutePath) =>
  crypto.createHash("sha256").update(fs.readFileSync(absolutePath)).digest("hex").toUpperCase();

function walk(directory) {
  return fs.readdirSync(directory, { withFileTypes: true }).flatMap((entry) => {
    const absolutePath = path.join(directory, entry.name);
    return entry.isDirectory() ? walk(absolutePath) : [absolutePath];
  });
}

const discovered = evidenceRoots.flatMap((relativeRoot) => walk(path.join(repoRoot, relativeRoot)));
const explicit = evidenceFiles.map((relativePath) => path.join(repoRoot, relativePath));
const files = [...new Set([...discovered, ...explicit])]
  .filter((absolutePath) => path.resolve(absolutePath) !== path.resolve(outputPath))
  .sort((left, right) => left.localeCompare(right));

const entries = files.map((absolutePath) => {
  const stats = fs.statSync(absolutePath);
  return {
    path: path.relative(repoRoot, absolutePath).replaceAll("\\", "/"),
    bytes: stats.size,
    sha256: sha256(absolutePath),
  };
});

const artifactEntry = (relativePath) => {
  const absolutePath = path.join(repoRoot, relativePath);
  const stats = fs.statSync(absolutePath);
  return {
    path: relativePath,
    bytes: stats.size,
    sha256: sha256(absolutePath),
  };
};

const gitHead = execFileSync("git", ["rev-parse", "HEAD"], {
  cwd: repoRoot,
  encoding: "utf8",
}).trim();
const dirtyEntries = execFileSync("git", ["status", "--porcelain=v1", "--untracked-files=normal"], {
  cwd: repoRoot,
  encoding: "utf8",
  maxBuffer: 16 * 1024 * 1024,
})
  .split(/\r?\n/u)
  .filter(Boolean);

const releaseGate = JSON.parse(fs.readFileSync(path.join(auditRoot, "release-gate.json"), "utf8"));
const manifest = {
  schemaVersion: 1,
  phase: "15.8-stage-5-chinese-qa-release-and-rollback",
  generatedAt: new Date().toISOString(),
  source: {
    gitHead,
    dirtyEntryCount: dirtyEntries.length,
    note: "The shared worktree was already dirty; user changes were preserved and no clean-tree claim is made.",
  },
  releaseGate: {
    technicalSuitePassed: releaseGate.technicalSuitePassed,
    verdict: releaseGate.verdict,
    formalReleaseAuthorized: releaseGate.formalReleaseAuthorized,
    openBlockerCount: releaseGate.openBlockerCount,
  },
  protectedFormalArtifacts: protectedArtifacts.map(artifactEntry),
  isolatedCandidateArtifacts: candidateArtifacts.map(artifactEntry),
  evidenceFileCount: entries.length,
  evidence: entries,
};

fs.writeFileSync(outputPath, `${JSON.stringify(manifest, null, 2)}\n`);
console.log(
  JSON.stringify(
    {
      outputPath,
      evidenceFileCount: manifest.evidenceFileCount,
      verdict: manifest.releaseGate.verdict,
      formalReleaseAuthorized: manifest.releaseGate.formalReleaseAuthorized,
    },
    null,
    2,
  ),
);

#!/usr/bin/env node

import crypto from "node:crypto";
import fs from "node:fs/promises";
import path from "node:path";
import { execFileSync } from "node:child_process";

const root = path.resolve(process.argv[2] || ".");
const outputPath = path.join(root, "docs/i18n/audit/phase-5-release/pre-edit-baseline.json");
const sha256 = (content) =>
  crypto.createHash("sha256").update(content).digest("hex").toUpperCase();

async function inspect(relativePath, optional = false) {
  const absolutePath = path.join(root, relativePath);
  try {
    const [content, stat] = await Promise.all([fs.readFile(absolutePath), fs.stat(absolutePath)]);
    return {
      path: relativePath.replaceAll("\\", "/"),
      exists: true,
      bytes: content.length,
      sha256: sha256(content),
      modifiedAt: stat.mtime.toISOString(),
    };
  } catch (error) {
    if (!optional) throw error;
    return { path: relativePath.replaceAll("\\", "/"), exists: false };
  }
}

const priorEvidence = [
  "docs/i18n/audit/phase-0-baseline/phase0-audit-report.md",
  "docs/i18n/audit/phase-1-runtime/phase1-audit-report.md",
  "docs/i18n/audit/phase-2-react/2F/phase2-2F-audit-report.md",
  "docs/i18n/audit/phase-3-native/phase3-audit-report.md",
  "docs/i18n/audit/phase-4-content/phase4-audit-report.md",
  "docs/i18n/audit/phase-4-content/static-audit.json",
  "docs/i18n/audit/phase-4-content/runtime-audit.json",
  "docs/i18n/audit/phase-4-content/post-build-integrity.json",
];
const releaseArtifacts = [
  "target/release/meetily.exe",
  "target/release/bundle/nsis/meetily_0.4.0_x64-setup.exe",
];
const configuration = [
  "frontend/src-tauri/tauri.conf.json",
  "frontend/src-tauri/Cargo.toml",
  "frontend/package.json",
  "frontend/src-tauri/scripts/sign-windows.ps1",
  "frontend/src-tauri/scripts/nsis-installer-hooks.nsh",
];

let gitHead = null;
let gitStatus = [];
try {
  gitHead = execFileSync("git", ["rev-parse", "HEAD"], { cwd: root, encoding: "utf8" }).trim();
  gitStatus = execFileSync("git", ["status", "--short"], { cwd: root, encoding: "utf8" })
    .trim()
    .split(/\r?\n/)
    .filter(Boolean);
} catch {
  // File hashes remain authoritative when Git metadata is unavailable.
}

const report = {
  phase: "15.8-stage-5-chinese-qa-release-and-rollback",
  generatedAt: new Date().toISOString(),
  gitHead,
  gitStatusCount: gitStatus.length,
  gitStatus,
  environment: {
    platform: process.platform,
    architecture: process.arch,
    node: process.version,
    timezone: Intl.DateTimeFormat().resolvedOptions().timeZone,
  },
  invariants: {
    formalReleaseMustRemainUnchanged: true,
    isolatedTargetDirectory: "target-phase5-release",
    phase5AuditIdentifier: "com.meetily.ai.phase5audit",
  },
  releaseArtifacts: await Promise.all(releaseArtifacts.map((file) => inspect(file, true))),
  configuration: await Promise.all(configuration.map((file) => inspect(file))),
  priorEvidence: await Promise.all(priorEvidence.map((file) => inspect(file))),
};

await fs.mkdir(path.dirname(outputPath), { recursive: true });
await fs.writeFile(outputPath, JSON.stringify(report, null, 2) + "\n");
process.stdout.write(
  JSON.stringify({
    outputPath,
    gitHead,
    gitStatusCount: gitStatus.length,
    releaseArtifacts: report.releaseArtifacts,
    priorEvidence: report.priorEvidence.length,
  }) + "\n",
);

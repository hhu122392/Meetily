#!/usr/bin/env node

import crypto from "node:crypto";
import fs from "node:fs/promises";
import path from "node:path";
import { execFileSync } from "node:child_process";

const root = path.resolve(process.argv[2] || ".");
const auditDirectory = path.join(root, "docs/i18n/audit/phase-5-release");
const baseline = JSON.parse(
  await fs.readFile(path.join(auditDirectory, "pre-edit-baseline.json"), "utf8"),
);
const sha256 = (content) =>
  crypto.createHash("sha256").update(content).digest("hex").toUpperCase();

async function inspect(absoluteOrRelativePath) {
  const absolutePath = path.isAbsolute(absoluteOrRelativePath)
    ? absoluteOrRelativePath
    : path.join(root, absoluteOrRelativePath);
  const [content, stat] = await Promise.all([fs.readFile(absolutePath), fs.stat(absolutePath)]);
  return {
    path: path.relative(root, absolutePath).replaceAll("\\", "/"),
    bytes: content.length,
    sha256: sha256(content),
    modifiedAt: stat.mtime.toISOString(),
  };
}

function signatureStatus(absolutePath) {
  const escaped = absolutePath.replaceAll("'", "''");
  return execFileSync(
    "pwsh.exe",
    [
      "-NoProfile",
      "-Command",
      `(Get-AuthenticodeSignature -LiteralPath '${escaped}').Status.ToString()`,
    ],
    { encoding: "utf8" },
  ).trim();
}

const releaseDirectory = path.join(root, "target-phase5-release/release");
const candidateExePath = path.join(releaseDirectory, "meetily.exe");
const nsisDirectory = path.join(releaseDirectory, "bundle/nsis");
const installerNames = (await fs.readdir(nsisDirectory)).filter((file) => file.endsWith("-setup.exe"));
if (installerNames.length !== 1) {
  throw new Error(`Expected exactly one Phase 5 NSIS installer, found ${installerNames.length}`);
}
const installerPath = path.join(nsisDirectory, installerNames[0]);
const [candidateExe, installer, formalExe, formalInstaller] = await Promise.all([
  inspect(candidateExePath),
  inspect(installerPath),
  inspect("target/release/meetily.exe"),
  inspect("target/release/bundle/nsis/meetily_0.4.0_x64-setup.exe"),
]);
const baselineFormalExe = baseline.releaseArtifacts.find(
  (entry) => entry.path === "target/release/meetily.exe",
);
const baselineFormalInstaller = baseline.releaseArtifacts.find(
  (entry) => entry.path === "target/release/bundle/nsis/meetily_0.4.0_x64-setup.exe",
);
const configuration = JSON.parse(
  await fs.readFile(path.join(root, "frontend/src-tauri/tauri.phase5.conf.json"), "utf8"),
);
const signatures = {
  candidateExe: signatureStatus(candidateExePath),
  installer: signatureStatus(installerPath),
};
const assertions = {
  candidateExeExists: candidateExe.bytes > 0,
  installerExists: installer.bytes > 0,
  isolatedIdentifier: configuration.identifier === "com.meetily.ai.phase5audit",
  explicitlyUnsignedAuditCandidate:
    signatures.candidateExe === "NotSigned" && signatures.installer === "NotSigned",
  formalExeUnchanged:
    formalExe.bytes === baselineFormalExe.bytes && formalExe.sha256 === baselineFormalExe.sha256,
  formalInstallerUnchanged:
    formalInstaller.bytes === baselineFormalInstaller.bytes &&
    formalInstaller.sha256 === baselineFormalInstaller.sha256,
};
const report = {
  phase: "15.8-stage-5-chinese-qa-release-and-rollback",
  generatedAt: new Date().toISOString(),
  passed: Object.values(assertions).every(Boolean),
  assertions,
  candidate: { exe: candidateExe, installer, signatures },
  protectedFormalArtifacts: { exe: formalExe, installer: formalInstaller },
};
await fs.writeFile(
  path.join(auditDirectory, "release-integrity.json"),
  JSON.stringify(report, null, 2) + "\n",
);
process.stdout.write(JSON.stringify({ passed: report.passed, assertions, candidate: report.candidate }) + "\n");
if (!report.passed) process.exitCode = 1;

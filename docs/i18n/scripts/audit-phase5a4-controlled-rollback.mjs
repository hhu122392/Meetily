#!/usr/bin/env node

import crypto from 'node:crypto';
import fs from 'node:fs';
import path from 'node:path';
import process from 'node:process';
import { fileURLToPath } from 'node:url';

const scriptDir = path.dirname(fileURLToPath(import.meta.url));
const repoRoot = path.resolve(scriptDir, '..', '..', '..');
const formalReleaseRoot = path.resolve(repoRoot, '..', 'target', 'release');
const defaultOutput = path.join(
  repoRoot,
  'docs',
  'i18n',
  'audit',
  'phase-5a4',
  'phase-5a4a-static-audit.json'
);

const outputFlag = process.argv.indexOf('--output');
const outputPath =
  outputFlag >= 0 && process.argv[outputFlag + 1]
    ? path.resolve(process.argv[outputFlag + 1])
    : defaultOutput;
const sandboxReportFlag = process.argv.indexOf('--sandbox-report');
const sandboxReportPath =
  sandboxReportFlag >= 0 && process.argv[sandboxReportFlag + 1]
    ? path.resolve(process.argv[sandboxReportFlag + 1])
    : path.join(repoRoot, 'docs', 'i18n', 'audit', 'phase-5a4', 'controlled-rollback-sandbox-final.json');
const toolReportFlag = process.argv.indexOf('--tool-report');
const toolReportPath =
  toolReportFlag >= 0 && process.argv[toolReportFlag + 1]
    ? path.resolve(process.argv[toolReportFlag + 1])
    : path.join(repoRoot, 'docs', 'i18n', 'audit', 'phase-5a4', 'controlled-rollback-tool-final.json');

const assertions = [];

function record(id, passed, evidence) {
  assertions.push({ id, passed: Boolean(passed), evidence });
}

function readText(relativePath) {
  return fs.readFileSync(path.join(repoRoot, relativePath), 'utf8');
}

function readJson(relativePath) {
  return JSON.parse(readText(relativePath).replace(/^\uFEFF/, ''));
}

function sha256(filePath) {
  return crypto.createHash('sha256').update(fs.readFileSync(filePath)).digest('hex').toUpperCase();
}

function resolveRelease(manifest, version, visited = new Set()) {
  if (visited.has(version)) throw new Error(`Ownership alias cycle: ${version}`);
  visited.add(version);
  const release = manifest.releases.find((item) => item.version === version);
  if (!release) throw new Error(`Ownership release is missing: ${version}`);
  if (!release.sameOwnedResourcesAs) return release;
  return resolveRelease(manifest, release.sameOwnedResourcesAs, visited);
}

function isSafeRelativePath(value) {
  const normalized = value.replaceAll('\\', '/');
  return (
    normalized.length > 0 &&
    !path.posix.isAbsolute(normalized) &&
    !/^[a-zA-Z]:/.test(normalized) &&
    !normalized.split('/').some((part) => part === '' || part === '.' || part === '..')
  );
}

const manifest = readJson('docs/i18n/phase-5a4/install-resource-ownership.v1.json');
const moduleText = readText('docs/i18n/scripts/Meetily.Rollback.psm1');
const wrapperText = readText('docs/i18n/scripts/invoke-meetily-controlled-rollback.ps1');
const failureTestText = readText('docs/i18n/scripts/test-phase5a4-controlled-rollback.ps1');
const updaterText = readText('frontend/src/services/updateService.ts');
const semverText = readText('frontend/src/lib/semanticVersion.ts');
const semverTestText = readText('frontend/tests/lib/semantic-version.test.ts');
const nsisHookText = readText('frontend/src-tauri/scripts/nsis-installer-hooks.nsh');
const tauriConfig = readJson('frontend/src-tauri/tauri.conf.json');
const sandboxReport = JSON.parse(fs.readFileSync(sandboxReportPath, 'utf8').replace(/^\uFEFF/, ''));
const toolReport = JSON.parse(fs.readFileSync(toolReportPath, 'utf8').replace(/^\uFEFF/, ''));
const finalSandboxScriptHashes = {
  rollbackScriptSha256: sha256(
    path.join(repoRoot, 'docs', 'i18n', 'scripts', 'invoke-meetily-controlled-rollback.ps1')
  ),
  rollbackModuleSha256: sha256(
    path.join(repoRoot, 'docs', 'i18n', 'scripts', 'Meetily.Rollback.psm1')
  ),
  sandboxAuditScriptSha256: sha256(
    path.join(
      repoRoot,
      'docs',
      'i18n',
      'scripts',
      'audit-phase5a4-sandbox-controlled-rollback.ps1'
    )
  ),
};

record(
  'ownership.identity-and-schema',
  manifest.schemaVersion === 1 &&
    manifest.product?.displayName === 'meetily' &&
    manifest.product?.identifier === 'com.meetily.ai',
  {
    schemaVersion: manifest.schemaVersion,
    product: manifest.product,
  }
);

const expectedProtectedRoots = [
  '%APPDATA%/com.meetily.ai',
  '%APPDATA%/Meetily/templates',
  '%APPDATA%/meetily',
  '%USERPROFILE%/Music/meetily-recordings',
];
const actualProtectedRoots = manifest.policy.protectedDataRoots.map((root) => root.pathTemplate);
record(
  'ownership.fail-closed-policy',
  manifest.policy.unknownResidualFile === 'block' &&
    manifest.policy.ownedPathHashMismatch === 'block' &&
    manifest.policy.removeDirectoriesOnlyWhenEmpty === true &&
    JSON.stringify(actualProtectedRoots) === JSON.stringify(expectedProtectedRoots),
  {
    unknownResidualFile: manifest.policy.unknownResidualFile,
    ownedPathHashMismatch: manifest.policy.ownedPathHashMismatch,
    removeDirectoriesOnlyWhenEmpty: manifest.policy.removeDirectoriesOnlyWhenEmpty,
    protectedDataRoots: actualProtectedRoots,
  }
);

const expectedVersions = ['0.3.0', '0.4.0', '0.4.1', '0.4.2'];
record(
  'ownership.release-coverage',
  JSON.stringify(manifest.releases.map((release) => release.version)) ===
    JSON.stringify(expectedVersions) &&
    manifest.releases.find((release) => release.version === '0.4.1')?.sameOwnedResourcesAs ===
      '0.4.0' &&
    manifest.releases.find((release) => release.version === '0.4.2')?.sameOwnedResourcesAs ===
      '0.4.0',
  { versions: manifest.releases.map((release) => release.version) }
);

const ownershipHashEvidence = [];
let ownershipHashesMatch = true;
for (const version of ['0.3.0', '0.4.0', '0.4.1', '0.4.2']) {
  const release = resolveRelease(manifest, version);
  const files = release.ownedResidualFiles ?? [];
  const expectedCount = version === '0.3.0' ? 6 : 18;
  const uniquePaths = new Set(files.map((entry) => entry.path));
  if (files.length !== expectedCount || uniquePaths.size !== files.length) {
    ownershipHashesMatch = false;
  }
  for (const entry of files) {
    if (!isSafeRelativePath(entry.path)) {
      ownershipHashesMatch = false;
      ownershipHashEvidence.push({ version, path: entry.path, error: 'unsafe relative path' });
      continue;
    }
    const sourcePath = path.join(repoRoot, 'frontend', 'src-tauri', ...entry.path.split('/'));
    if (!fs.existsSync(sourcePath)) {
      ownershipHashesMatch = false;
      ownershipHashEvidence.push({ version, path: entry.path, error: 'source missing' });
      continue;
    }
    const actual = sha256(sourcePath).toLowerCase();
    if (actual !== entry.sha256) ownershipHashesMatch = false;
    ownershipHashEvidence.push({
      version,
      path: entry.path,
      expectedSha256: entry.sha256,
      actualSha256: actual,
      matched: actual === entry.sha256,
    });
  }
}
record('ownership.exact-resource-hashes', ownershipHashesMatch, {
  checkedEntries: ownershipHashEvidence.length,
  uniqueSourceFiles: new Set(ownershipHashEvidence.map((entry) => entry.path)).size,
  mismatches: ownershipHashEvidence.filter((entry) => entry.matched === false || entry.error),
});

const strictUpdateCalls = updaterText.match(
  /isStrictlyNewerVersion\(update\.version, currentVersion\)/g
);
record(
  'updater.strict-upgrade-only',
  strictUpdateCalls?.length === 2 &&
    updaterText.includes("throw new Error('UPDATE_VERSION_NOT_NEWER')") &&
    semverText.includes('compareSemanticVersions(candidate, current) === 1') &&
    semverText.includes('if (!parsedLeft || !parsedRight) return null'),
  {
    strictGateCallCount: strictUpdateCalls?.length ?? 0,
    discoveryGate: updaterText.includes('update?.available'),
    installGateError: updaterText.includes('UPDATE_VERSION_NOT_NEWER'),
    invalidVersionFailsClosed: semverText.includes('if (!parsedLeft || !parsedRight) return null'),
  }
);

record(
  'updater.semver-unit-coverage',
  semverTestText.includes('implements SemVer precedence including prereleases') &&
    semverTestText.includes(
      'strict updater gate fails closed for downgrade, repair, and invalid metadata'
    ) &&
    semverTestText.includes('rejects malformed and ambiguous versions'),
  { testFile: 'frontend/tests/lib/semantic-version.test.ts' }
);

record(
  'installer.silent-downgrade-block',
  tauriConfig.bundle?.windows?.allowDowngrades === false &&
    tauriConfig.bundle?.windows?.nsis?.installerHooks === 'scripts/nsis-installer-hooks.nsh' &&
    nsisHookText.includes('!macro NSIS_HOOK_PREINSTALL') &&
    nsisHookText.includes('nsis_tauri_utils::SemverCompare "${VERSION}" $R8') &&
    nsisHookText.includes('${IfNot} ${Silent}') &&
    nsisHookText.includes('SetErrorLevel 3') &&
    nsisHookText.includes('Quit'),
  {
    allowDowngrades: tauriConfig.bundle?.windows?.allowDowngrades,
    hook: tauriConfig.bundle?.windows?.nsis?.installerHooks,
  }
);

record(
  'rollback.module-fail-closed-cleanup',
  moduleText.includes("unknownResidualFile -cne 'block'") &&
    moduleText.includes("ownedPathHashMismatch -cne 'block'") &&
    moduleText.includes('Residual file is a reparse point') &&
    moduleText.includes('Residual hash mismatch blocks rollback cleanup') &&
    moduleText.includes('Unknown residual file blocks rollback cleanup'),
  {
    unknownResidualBlock: moduleText.includes('Unknown residual file blocks rollback cleanup'),
    hashMismatchBlock: moduleText.includes('Residual hash mismatch blocks rollback cleanup'),
    reparsePointBlock: moduleText.includes('Residual file is a reparse point'),
  }
);

record(
  'rollback.snapshot-integrity-and-version',
  moduleText.includes('Snapshot version does not match rollback target') &&
    moduleText.includes('Snapshot file integrity mismatch') &&
    moduleText.includes('Duplicate snapshot file declaration') &&
    moduleText.includes('Duplicate snapshot protected-root id') &&
    moduleText.includes('Snapshot contains undeclared files') &&
    moduleText.includes('Test-MeetilyRestoredData'),
  {
    versionBound: true,
    hashAndLengthBound: true,
    duplicateRootsAndFilesBlocked: true,
    undeclaredFilesBlocked: true,
    restoreVerified: true,
  }
);

record(
  'rollback.explicit-confirmation-and-signatures',
  wrapperText.includes("Rollback mode requires the explicit -ConfirmRollback switch") &&
    wrapperText.includes('[switch]$AuditAllowUnsignedInstaller') &&
    wrapperText.includes("[ValidateSet('Production', 'Audit')][string]$SecurityMode = 'Production'") &&
    wrapperText.includes("$AuditAllowUnsignedInstaller -and $SecurityMode -cne 'Audit'") &&
    wrapperText.includes('-SigningRole HistoricalRollbackTarget') &&
    wrapperText.includes('-SigningRole RecoveryInstaller') &&
    moduleText.includes('Test-MeetilySignedFile') &&
    moduleText.includes("$AuditAllowUnsigned -and $SecurityMode -cne 'Audit'"),
  {
    explicitRollbackConfirmation: true,
    productionSignatureDefault: true,
    auditOnlyUnsignedOverride: true,
  }
);

record(
  'rollback.automatic-recovery-path',
  wrapperText.includes('New-MeetilyDataSnapshot') &&
    wrapperText.includes('emergencyDataRestored') &&
    wrapperText.includes('recoveryInstall') &&
    wrapperText.includes('$report.recoveredAfterFailure = $true'),
  {
    emergencySnapshotBeforeMutation: true,
    emergencyRestoreOnFailure: true,
    recoveryInstallerOnFailure: true,
  }
);

record(
  'rollback.failure-injection-coverage',
  failureTestText.includes('Unknown residual file blocks') &&
    failureTestText.includes('Residual hash mismatch blocks') &&
    failureTestText.includes('Snapshot file integrity mismatch') &&
    failureTestText.includes('Duplicate snapshot file declaration') &&
    failureTestText.includes('Duplicate snapshot protected-root id') &&
    failureTestText.includes('exactHashOwnershipCleanup'),
  {
    expectedAssertions: 10,
    unknownResidual: true,
    hashMismatch: true,
    corruptedSnapshot: true,
    duplicateSnapshotFile: true,
    duplicateSnapshotRoot: true,
  }
);

record(
  'sandbox.final-report-pass',
  sandboxReport.passed === true &&
    Object.values(sandboxReport.assertions).every((value) => value === true) &&
    Object.entries(finalSandboxScriptHashes).every(
      ([key, value]) => sandboxReport.inputs?.[key] === value
    ) &&
    sandboxReport.observations?.preflight?.auditReport?.mutated === false &&
    sandboxReport.observations?.rollbackIdentity?.displayVersion === '0.3.0' &&
    sandboxReport.observations?.rollbackIdentity?.localizedResidualCount === 0 &&
    sandboxReport.observations?.rollbackIdentity?.signatureStatus === 'Valid',
  {
    startedAtUtc: sandboxReport.startedAtUtc,
    completedAtUtc: sandboxReport.completedAtUtc,
    assertions: sandboxReport.assertions,
    finalScriptHashes: finalSandboxScriptHashes,
    recordedScriptHashes: {
      rollbackScriptSha256: sandboxReport.inputs?.rollbackScriptSha256,
      rollbackModuleSha256: sandboxReport.inputs?.rollbackModuleSha256,
      sandboxAuditScriptSha256: sandboxReport.inputs?.sandboxAuditScriptSha256,
    },
    rollbackIdentity: sandboxReport.observations?.rollbackIdentity,
  }
);

record(
  'sandbox.tool-report-pass',
  toolReport.passed === true &&
    toolReport.mutated === true &&
    toolReport.recoveredAfterFailure === false &&
    toolReport.inputs?.installedVersion === '0.4.1' &&
    toolReport.inputs?.targetVersion === '0.3.0' &&
    toolReport.inputs?.targetInstaller?.SignatureStatus === 'Valid' &&
    toolReport.observations?.targetDataSnapshotRestored === true &&
    toolReport.observations?.installedTargetVersion === '0.3.0',
  {
    installedVersion: toolReport.inputs?.installedVersion,
    targetVersion: toolReport.inputs?.targetVersion,
    targetInstallerSha256: toolReport.inputs?.targetInstaller?.Sha256,
    targetInstallerSignature: toolReport.inputs?.targetInstaller?.SignatureStatus,
    restoredSnapshot: toolReport.observations?.targetDataSnapshotRestored,
    recoveredAfterFailure: toolReport.recoveredAfterFailure,
  }
);

const formalArtifacts = [
  {
    path: path.join(formalReleaseRoot, 'meetily.exe'),
    expectedSha256: '1873D2CCA9B06EEBE8B2D42DC8530B6888BCB91C4267BBD6E6724F6A78257823',
  },
  {
    path: path.join(formalReleaseRoot, 'bundle', 'nsis', 'meetily_0.4.0_x64-setup.exe'),
    expectedSha256: 'C6A5BD12F947AEDECAC890C33A48FDEA94E4C11C29FCA03EEF753C25AEE30434',
  },
];
const formalEvidence = formalArtifacts.map((artifact) => ({
  ...artifact,
  exists: fs.existsSync(artifact.path),
  actualSha256: fs.existsSync(artifact.path) ? sha256(artifact.path) : null,
}));
record(
  'formal-release.artifacts-unchanged',
  formalEvidence.every(
    (artifact) => artifact.exists && artifact.actualSha256 === artifact.expectedSha256
  ),
  formalEvidence
);

const failedAssertions = assertions.filter((assertion) => !assertion.passed);
const report = {
  schemaVersion: 1,
  phase: '5A-4A',
  scope: 'Controlled rollback and exact install-resource ownership static audit',
  generatedAtUtc: new Date().toISOString(),
  repoRoot,
  passed: failedAssertions.length === 0,
  summary: {
    total: assertions.length,
    passed: assertions.length - failedAssertions.length,
    failed: failedAssertions.length,
  },
  ownershipManifestSha256: sha256(
    path.join(repoRoot, 'docs', 'i18n', 'phase-5a4', 'install-resource-ownership.v1.json')
  ),
  assertions,
};

fs.mkdirSync(path.dirname(outputPath), { recursive: true });
fs.writeFileSync(outputPath, `${JSON.stringify(report, null, 2)}\n`, 'utf8');
console.log(JSON.stringify(report.summary));
console.log(`report=${outputPath}`);
if (!report.passed) process.exitCode = 1;

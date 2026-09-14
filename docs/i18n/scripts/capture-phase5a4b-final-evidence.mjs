import fs from 'node:fs';
import path from 'node:path';
import crypto from 'node:crypto';
import process from 'node:process';

const args = new Map();
for (let index = 2; index < process.argv.length; index += 2) {
  const key = process.argv[index];
  const value = process.argv[index + 1];
  if (!key?.startsWith('--') || value === undefined) throw new Error(`Invalid argument near ${key ?? '<end>'}`);
  args.set(key.slice(2), value);
}
const required = [
  'repo', 'static-audit', 'ps7-tests', 'ps51-tests', 'sandbox-report', 'sandbox-tool-report',
  'rollback-static-audit', 'updater-positive', 'updater-tampered-negative', 'unsigned-release-gate',
  'frontend-unit-log', 'frontend-i18n-log', 'formal-exe', 'formal-installer', 'output',
];
for (const name of required) if (!args.has(name)) throw new Error(`--${name} is required`);

const repo = path.resolve(args.get('repo'));
const output = path.resolve(args.get('output'));
const readJson = (name) => JSON.parse(fs.readFileSync(path.resolve(args.get(name)), 'utf8').replace(/^\uFEFF/, ''));
const hashFile = (file) => crypto.createHash('sha256').update(fs.readFileSync(file)).digest('hex').toUpperCase();
const assertions = [];
const check = (name, passed, evidence = null) => assertions.push({ name, passed: Boolean(passed), evidence });

const staticAudit = readJson('static-audit');
const ps7 = readJson('ps7-tests');
const ps51 = readJson('ps51-tests');
const sandbox = readJson('sandbox-report');
const sandboxTool = readJson('sandbox-tool-report');
const rollbackStatic = readJson('rollback-static-audit');
const updaterPositive = readJson('updater-positive');
const updaterTampered = readJson('updater-tampered-negative');
const unsignedReleaseGate = readJson('unsigned-release-gate');
const unitLog = fs.readFileSync(path.resolve(args.get('frontend-unit-log')), 'utf8');
const i18nLog = fs.readFileSync(path.resolve(args.get('frontend-i18n-log')), 'utf8');

check('production-signing-static-audit', staticAudit.passed && staticAudit.failedCount === 0,
  { assertions: staticAudit.assertionCount, failed: staticAudit.failedCount });
check('powershell-7-signing-tests', ps7.passed && ps7.failed === 0 && ps7.total === 18,
  { version: ps7.powershellVersion, total: ps7.total, failed: ps7.failed });
check('windows-powershell-5.1-signing-tests', ps51.passed && ps51.failed === 0 && ps51.total === 18,
  { version: ps51.powershellVersion, total: ps51.total, failed: ps51.failed });
check('sandbox-controlled-rollback-regression',
  sandbox.passed && Object.values(sandbox.assertions).every((value) => value === true),
  { startedAtUtc: sandbox.startedAtUtc, completedAtUtc: sandbox.completedAtUtc, assertions: sandbox.assertions });
check('sandbox-tool-explicit-audit-mode',
  sandboxTool.passed && sandboxTool.mutated && sandboxTool.inputs.securityMode === 'Audit' &&
  sandboxTool.inputs.targetInstaller.AuditUnsignedOverride === true &&
  sandboxTool.inputs.recoveryInstaller.AuditUnsignedOverride === true,
  { securityMode: sandboxTool.inputs.securityMode, mutated: sandboxTool.mutated });
check('rollback-static-regression', rollbackStatic.passed && rollbackStatic.summary?.failed === 0,
  rollbackStatic.summary);
check('real-updater-signature-positive', updaterPositive.passed === true &&
  updaterPositive.assertions.mainSignatureValid === true &&
  updaterPositive.assertions.trustedCommentSignatureValid === true &&
  updaterPositive.assertions.keyIdsMatch === true,
  { keyId: updaterPositive.signature.keyId });
check('tampered-updater-artifact-negative', updaterTampered.passed === false &&
  updaterTampered.assertions.mainSignatureValid === false,
  updaterTampered.assertions);
check('unsigned-current-release-gate-negative', unsignedReleaseGate.passed === false &&
  unsignedReleaseGate.artifacts.length > 0 && unsignedReleaseGate.failures.length > 0,
  { artifacts: unsignedReleaseGate.artifacts.length, failures: unsignedReleaseGate.failures.length });
check('frontend-unit-regression', /# pass 55\b/.test(unitLog) && /# fail 0\b/.test(unitLog));
check('frontend-i18n-regression', /# pass 54\b/.test(i18nLog) && /# fail 0\b/.test(i18nLog));
check('workflow-yaml-parse', args.get('yaml-parse-pass') === 'true');

const formalExe = path.resolve(args.get('formal-exe'));
const formalInstaller = path.resolve(args.get('formal-installer'));
const formalExeExpected = '1873D2CCA9B06EEBE8B2D42DC8530B6888BCB91C4267BBD6E6724F6A78257823';
const formalInstallerExpected = 'C6A5BD12F947AEDECAC890C33A48FDEA94E4C11C29FCA03EEF753C25AEE30434';
const formalExeActual = hashFile(formalExe);
const formalInstallerActual = hashFile(formalInstaller);
check('formal-portable-exe-unchanged', formalExeActual === formalExeExpected,
  { path: formalExe, expectedSha256: formalExeExpected, actualSha256: formalExeActual });
check('formal-installer-unchanged', formalInstallerActual === formalInstallerExpected,
  { path: formalInstaller, expectedSha256: formalInstallerExpected, actualSha256: formalInstallerActual });

const sourceFiles = [
  '.github/workflows/build.yml',
  '.github/workflows/build-windows.yml',
  '.github/workflows/build-devtest.yml',
  'frontend/src-tauri/scripts/sign-windows.ps1',
  'frontend/src-tauri/scripts/prepare-digicert-signing.ps1',
  'docs/i18n/phase-5a4/windows-signing-policy.v1.json',
  'docs/i18n/scripts/Meetily.Signing.psm1',
  'docs/i18n/scripts/audit-phase5a4b-release-signing.ps1',
  'docs/i18n/scripts/verify-tauri-updater-signature.mjs',
  'docs/i18n/scripts/test-phase5a4b-signing-policy.ps1',
  'docs/i18n/scripts/invoke-meetily-controlled-rollback.ps1',
  'docs/i18n/scripts/Meetily.Rollback.psm1',
];
const sourceEvidence = sourceFiles.map((relative) => {
  const file = path.join(repo, relative);
  return { path: relative, bytes: fs.statSync(file).size, sha256: hashFile(file) };
});

const failed = assertions.filter((item) => !item.passed);
const report = {
  schemaVersion: 1,
  phase: '15.13 / Stage 5A-4B',
  scope: 'Production signing chain, trusted timestamp, updater signature, and controlled rollback integration',
  generatedAtUtc: new Date().toISOString(),
  technicalImplementationPassed: failed.length === 0,
  productionReady: false,
  releaseDecision: 'NO-GO',
  assertionCount: assertions.length,
  failedCount: failed.length,
  assertions,
  productionBlockers: [
    'The productionActive publisher-certificate thumbprint set is intentionally empty pending release-owner approval.',
    'DigiCert/SMCTL production credentials and the Tauri updater private key are not present in the local audit environment.',
    'No current Chinese release binary and installer have passed the real production signing gate.',
  ],
  sourceEvidence,
};
fs.mkdirSync(path.dirname(output), { recursive: true });
fs.writeFileSync(output, `${JSON.stringify(report, null, 2)}\n`, 'utf8');
process.stdout.write(`${JSON.stringify({
  technicalImplementationPassed: report.technicalImplementationPassed,
  productionReady: report.productionReady,
  releaseDecision: report.releaseDecision,
  assertionCount: report.assertionCount,
  failedCount: report.failedCount,
  output,
}, null, 2)}\n`);
if (!report.technicalImplementationPassed) process.exitCode = 1;

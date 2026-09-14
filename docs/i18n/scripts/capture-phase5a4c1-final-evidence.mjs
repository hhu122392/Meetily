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
  'repo', 'static-audit', 'ps7-tests', 'ps51-tests', 'local-preflight', 'negative-gates',
  'phase5a4b-static', 'phase5a4b-ps7', 'phase5a4b-ps51', 'formal-exe', 'formal-installer', 'output',
  'frontend-unit-log', 'frontend-i18n-log', 'frontend-lint-log', 'syntax-audit',
];
for (const name of required) if (!args.has(name)) throw new Error(`--${name} is required`);

const repo = path.resolve(args.get('repo'));
const output = path.resolve(args.get('output'));
const readJsonArg = (name) => JSON.parse(fs.readFileSync(path.resolve(args.get(name)), 'utf8').replace(/^\uFEFF/, ''));
const hashFile = (file) => crypto.createHash('sha256').update(fs.readFileSync(file)).digest('hex').toUpperCase();
const assertions = [];
const check = (name, passed, evidence = null) => assertions.push({ name, passed: Boolean(passed), evidence });

const staticAudit = readJsonArg('static-audit');
const ps7 = readJsonArg('ps7-tests');
const ps51 = readJsonArg('ps51-tests');
const local = readJsonArg('local-preflight');
const negative = readJsonArg('negative-gates');
const phase5a4bStatic = readJsonArg('phase5a4b-static');
const phase5a4bPs7 = readJsonArg('phase5a4b-ps7');
const phase5a4bPs51 = readJsonArg('phase5a4b-ps51');
const syntaxAudit = readJsonArg('syntax-audit');
const frontendUnitLog = fs.readFileSync(path.resolve(args.get('frontend-unit-log')), 'utf8');
const frontendI18nLog = fs.readFileSync(path.resolve(args.get('frontend-i18n-log')), 'utf8');
const frontendLintLog = fs.readFileSync(path.resolve(args.get('frontend-lint-log')), 'utf8');

check('phase5a4c1-static-controls', staticAudit.passed && staticAudit.failedCount === 0 && staticAudit.assertionCount === 48,
  { assertions: staticAudit.assertionCount, failed: staticAudit.failedCount });
check('phase5a4c1-powershell-7', ps7.passed && ps7.failed === 0 && ps7.total === 32,
  { version: ps7.powershellVersion, total: ps7.total, failed: ps7.failed });
check('phase5a4c1-windows-powershell-5.1', ps51.passed && ps51.failed === 0 && ps51.total === 32,
  { version: ps51.powershellVersion, total: ps51.total, failed: ps51.failed });
check('real-local-preflight-is-fail-closed',
  local.admissionStatus === 'PendingCandidateEvidence' && local.admissionReady === false &&
  local.admissionDecision === 'NO-GO' && local.releaseDecision === 'NO-GO');
check('real-local-preflight-does-not-claim-production-ready', local.productionReady === false && local.signedRcRequired === true);
check('real-local-preflight-reports-no-secret-values',
  local.security.secretValuesEmitted === false && local.security.environmentValuesEmitted === false &&
  local.security.keypairAliasEmitted === false && local.security.privateKeyMaterialRead === false);
check('real-local-preflight-confirms-signtool', local.evidence.tools.signToolAvailable === true);
check('real-local-preflight-confirms-smctl-blocker', local.evidence.tools.smctlAvailable === false);
check('real-negative-gates-return-nonzero',
  negative.enforceReadyExitCode !== 0 && negative.prepareDigiCertExitCode !== 0, negative);
check('secret-pattern-scan-zero', negative.secretPatternHitCount === 0, { hits: negative.secretPatternHitCount });
check('phase5a4b-static-regression', phase5a4bStatic.passed && phase5a4bStatic.failedCount === 0,
  { assertions: phase5a4bStatic.assertionCount, failed: phase5a4bStatic.failedCount });
check('phase5a4b-ps7-regression', phase5a4bPs7.passed && phase5a4bPs7.failed === 0,
  { total: phase5a4bPs7.total, failed: phase5a4bPs7.failed });
check('phase5a4b-ps51-regression', phase5a4bPs51.passed && phase5a4bPs51.failed === 0,
  { total: phase5a4bPs51.total, failed: phase5a4bPs51.failed });
check('frontend-unit-regression', /# pass 55\b/.test(frontendUnitLog) && /# fail 0\b/.test(frontendUnitLog));
check('frontend-i18n-regression', /# pass 54\b/.test(frontendI18nLog) && /# fail 0\b/.test(frontendI18nLog));
check('frontend-lint-regression', negative.frontendLintExitCode === 0,
  { exitCode: negative.frontendLintExitCode, outputBytes: Buffer.byteLength(frontendLintLog) });
check('powershell-node-json-syntax-audit', syntaxAudit.passed === true && syntaxAudit.failed === 0,
  { total: syntaxAudit.total, failed: syntaxAudit.failed });

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
  'docs/i18n/phase-5a4/windows-signing-policy.v1.json',
  'docs/i18n/phase-5a4/windows-certificate-admission.v1.json',
  'docs/i18n/phase-5a4/phase-5a4c1-certificate-admission-audit.md',
  'docs/i18n/phase-5a4/phase-5a4c1-certificate-admission-runbook.zh-CN.md',
  'docs/i18n/scripts/Meetily.Signing.psm1',
  'docs/i18n/scripts/Meetily.CertificateAdmission.psm1',
  'docs/i18n/scripts/audit-phase5a4c1-certificate-admission.ps1',
  'docs/i18n/scripts/audit-phase5a4c1-certificate-admission.mjs',
  'docs/i18n/scripts/test-phase5a4c1-certificate-admission.ps1',
  'docs/i18n/scripts/capture-phase5a4c1-final-evidence.mjs',
  'frontend/src-tauri/scripts/prepare-digicert-signing.ps1',
  'docs/i18n/i18n-plan.zh-CN.md',
  'docs/i18n/README.md',
];
const sourceEvidence = sourceFiles.map((relative) => {
  const file = path.join(repo, relative);
  return { path: relative, bytes: fs.statSync(file).size, sha256: hashFile(file) };
});
const failed = assertions.filter((item) => !item.passed);
const report = {
  schemaVersion: 1,
  phase: '15.14 / Stage 5A-4C-1',
  scope: 'Windows production certificate admission preflight and fail-closed integration',
  generatedAtUtc: new Date().toISOString(),
  technicalImplementationPassed: failed.length === 0,
  certificateAdmissionReady: false,
  productionReady: false,
  releaseDecision: 'NO-GO',
  assertionCount: assertions.length,
  failedCount: failed.length,
  assertions,
  blockers: [
    'The controlled admission record remains PendingCandidateEvidence and productionActive remains empty.',
    'No current production certificate public evidence or independent approvals are available.',
    'smctl, DigiCert environment entries, and Tauri updater private-key entries are absent in the audited local environment.',
    'No disposable-artifact proof of possession or real signed Chinese RC has been executed.',
  ],
  sourceEvidence,
};
fs.mkdirSync(path.dirname(output), { recursive: true });
fs.writeFileSync(output, `${JSON.stringify(report, null, 2)}\n`, 'utf8');
process.stdout.write(`${JSON.stringify({
  technicalImplementationPassed: report.technicalImplementationPassed,
  certificateAdmissionReady: report.certificateAdmissionReady,
  productionReady: report.productionReady,
  releaseDecision: report.releaseDecision,
  assertionCount: report.assertionCount,
  failedCount: report.failedCount,
  output,
}, null, 2)}\n`);
if (!report.technicalImplementationPassed) process.exitCode = 1;

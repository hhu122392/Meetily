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
const repo = path.resolve(args.get('repo') ?? process.cwd());
const reportPath = args.has('report') ? path.resolve(args.get('report')) : null;
const read = (relative) => fs.readFileSync(path.join(repo, relative), 'utf8');
const sha256 = (relative) => crypto.createHash('sha256').update(fs.readFileSync(path.join(repo, relative))).digest('hex').toUpperCase();
const assertions = [];
const check = (name, passed, detail = '') => assertions.push({ name, passed: Boolean(passed), detail });

const policyRelative = 'docs/i18n/phase-5a4/windows-signing-policy.v1.json';
const admissionRelative = 'docs/i18n/phase-5a4/windows-certificate-admission.v1.json';
const policy = JSON.parse(read(policyRelative));
const admission = JSON.parse(read(admissionRelative));
const moduleText = read('docs/i18n/scripts/Meetily.CertificateAdmission.psm1');
const auditText = read('docs/i18n/scripts/audit-phase5a4c1-certificate-admission.ps1');
const testText = read('docs/i18n/scripts/test-phase5a4c1-certificate-admission.ps1');
const prepareText = read('frontend/src-tauri/scripts/prepare-digicert-signing.ps1');
const tauriConfig = JSON.parse(read('frontend/src-tauri/tauri.conf.json'));

check('admission-schema-v1', admission.schemaVersion === 1 && admission.admissionId === 'meetily-windows-production-certificate-admission-v1');
check('admission-targets-production-policy', admission.targetPolicyId === policy.policyId && admission.targetThumbprintSet === 'productionActive');
check('admission-intentionally-pending', admission.status === 'PendingCandidateEvidence');
check('production-active-remains-empty', policy.authenticode.signerThumbprintSets.productionActive.length === 0);
check('candidate-public-fields-remain-unset', Object.values(admission.candidate).every((value) => value === null));
check('independent-approvals-required',
  admission.requirements.requiredIndependentApprovals.length === 2 &&
  admission.requirements.requiredIndependentApprovals.includes('releaseOwner') &&
  admission.requirements.requiredIndependentApprovals.includes('securityReviewer'));
check('independent-approvals-remain-pending',
  admission.approvals.releaseOwner.status === 'Pending' && admission.approvals.securityReviewer.status === 'Pending');
check('code-signing-eku-frozen', admission.requirements.requiredCodeSigningEkuOid === '1.3.6.1.5.5.7.3.3');
check('minimum-validity-frozen', Number.isInteger(admission.requirements.minimumRemainingValidityDays) && admission.requirements.minimumRemainingValidityDays >= 30);
check('trusted-chain-required', admission.requirements.requireTrustedCertificateChain === true);
check('self-signed-forbidden', admission.requirements.forbidSelfSignedCertificate === true);
check('historical-reuse-forbidden', admission.requirements.forbidHistoricalThumbprintReuse === true);
check('digicert-connectivity-required',
  admission.requirements.requireDigiCertHealthcheck === true && admission.requirements.requireDigiCertCertificateSync === true);
check('proof-of-possession-required', admission.requirements.requireProofOfPossession === true);
check('updater-private-key-presence-required',
  admission.requirements.requireTauriUpdaterPrivateKeyPresence === true &&
  admission.requirements.requireTauriUpdaterPrivateKeyPasswordPresence === true);
check('approval-alone-does-not-authorize-release',
  admission.controls.approvalDoesNotAuthorizeReleaseByItself === true && admission.controls.realSignedRcGateStillRequired === true);

for (const fn of [
  'Read-MeetilyWindowsCertificateAdmission',
  'Get-MeetilyEnvironmentPresence',
  'Get-MeetilyTauriPublicKeyId',
  'Get-MeetilyCertificatePublicEvidence',
  'Test-MeetilyCertificateApprovalRecord',
  'Test-MeetilyCertificatePublicEvidence',
  'Test-MeetilyCertificateAdmissionReadiness',
]) check(`module-exports-${fn}`, moduleText.includes(`'${fn}'`));
check('module-validates-online-chain',
  moduleText.includes('X509RevocationMode]::Online') && moduleText.includes('X509VerificationFlags]::NoFlag'));
check('module-enforces-exact-active-candidate', moduleText.includes('productionActive must contain exactly the approved candidate thumbprint.'));
check('module-enforces-two-reviewable-approvals',
  moduleText.includes('has no valid approval timestamp') && moduleText.includes('has no reviewable approval reference'));
check('module-enforces-certificate-identity',
  moduleText.includes('subject does not match the admission record') &&
  moduleText.includes('thumbprint does not match the admission record') &&
  moduleText.includes('serial number does not match the admission record'));

check('audit-connectivity-is-explicit', auditText.includes('[switch]$RunDigiCertConnectivity'));
check('audit-ready-enforcement-is-explicit', auditText.includes('[switch]$EnforceReady'));
check('audit-smctl-output-suppressed',
  auditText.includes('$null = @(& $smctlCommand.Source healthcheck 2>&1)') &&
  auditText.includes('$null = @(& $smctlCommand.Source windows certsync'));
check('audit-proof-output-suppressed', auditText.includes('$null = @(') && auditText.includes('& $smctlCommand.Source sign'));
check('audit-proof-is-disposable-and-safely-cleaned',
  auditText.includes('MeetilyPhase5A4C1Proof-') && auditText.includes('Refusing to remove an unexpected proof-of-possession directory.'));
check('audit-proof-requires-timestamp-and-signtool',
  auditText.includes('--timestamp=true') && auditText.includes('signToolDefaultAuthenticodePassed'));
check('audit-never-claims-production-ready', auditText.includes('productionReady = $false') && auditText.includes("releaseDecision = 'NO-GO'"));
check('audit-declares-no-secret-output',
  auditText.includes('secretValuesEmitted = $false') && auditText.includes('privateKeyMaterialRead = $false'));
check('audit-does-not-print-environment-values', !/Write-(?:Host|Output|Verbose|Information)[^\n]*(?:SM_API_KEY|SM_CLIENT_CERT_PASSWORD|TAURI_SIGNING_PRIVATE_KEY|DIGICERT_KEYPAIR_ALIAS)/i.test(auditText));
check('audit-does-not-read-private-key-content', !/(?:Get-Content|ReadAllBytes|readFileSync)[^\n]*(?:TAURI_SIGNING_PRIVATE_KEY|private.?key)/i.test(auditText));
check('audit-does-not-list-keypairs', !/smctlCommand\.Source\s+keypair\s+(?:ls|list)/i.test(auditText));

check('prepare-imports-admission-module', prepareText.includes('Meetily.CertificateAdmission.psm1'));
check('prepare-fails-before-digicert-use-when-unapproved',
  prepareText.indexOf('Test-MeetilyCertificateApprovalRecord') < prepareText.indexOf('smctl healthcheck'));
check('prepare-enforces-synchronized-public-evidence', prepareText.includes('Test-MeetilyCertificatePublicEvidence'));
check('prepare-keeps-existing-health-and-sync-gates', prepareText.includes('smctl healthcheck') && prepareText.includes('smctl windows certsync'));
check('negative-tests-cover-secret-redaction', testText.includes('environment-audit-emits-presence-not-secret-value'));
check('negative-tests-cover-historical-reuse', testText.includes('reject-historical-thumbprint-reuse'));
check('negative-tests-cover-trust-and-expiry',
  testText.includes('reject-untrusted-certificate-chain') && testText.includes('reject-expiring-certificate'));
check('negative-tests-cover-possession-and-updater',
  testText.includes('reject-missing-proof-of-possession') && testText.includes('reject-updater-public-key-id-mismatch'));

const decodedPublicKey = Buffer.from(tauriConfig.plugins.updater.pubkey, 'base64').toString('utf8');
check('tauri-public-key-id-still-frozen', decodedPublicKey.includes(`minisign public key: ${policy.updater.publicKeyId}`));
check('historical-thumbprint-remains-role-isolated',
  policy.authenticode.signerThumbprintSets.upstreamHistorical.includes('0472869976D42A9F74D03B8B9CE60CF7A3983A3B') &&
  policy.roles.HistoricalRollbackTarget.signerThumbprintSet === 'upstreamHistorical');

const failed = assertions.filter((item) => !item.passed);
const report = {
  schemaVersion: 1,
  phase: '5A-4C-1',
  audit: 'Production certificate admission static controls',
  generatedAtUtc: new Date().toISOString(),
  passed: failed.length === 0,
  assertionCount: assertions.length,
  failedCount: failed.length,
  sourceEvidence: [policyRelative, admissionRelative,
    'docs/i18n/scripts/Meetily.CertificateAdmission.psm1',
    'docs/i18n/scripts/audit-phase5a4c1-certificate-admission.ps1',
    'docs/i18n/scripts/test-phase5a4c1-certificate-admission.ps1',
    'frontend/src-tauri/scripts/prepare-digicert-signing.ps1',
  ].map((relative) => ({ path: relative, sha256: sha256(relative) })),
  assertions,
};
if (reportPath) {
  fs.mkdirSync(path.dirname(reportPath), { recursive: true });
  fs.writeFileSync(reportPath, `${JSON.stringify(report, null, 2)}\n`, 'utf8');
}
process.stdout.write(`${JSON.stringify(report, null, 2)}\n`);
if (!report.passed) process.exitCode = 1;

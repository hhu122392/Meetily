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
const sha256 = (file) => crypto.createHash('sha256').update(fs.readFileSync(file)).digest('hex').toUpperCase();
const assertions = [];
const check = (name, passed, detail = '') => assertions.push({ name, passed: Boolean(passed), detail });

const policyRelative = 'docs/i18n/phase-5a4/windows-signing-policy.v1.json';
const policyPath = path.join(repo, policyRelative);
const policy = JSON.parse(read(policyRelative));
const signScript = read('frontend/src-tauri/scripts/sign-windows.ps1');
const prepareScript = read('frontend/src-tauri/scripts/prepare-digicert-signing.ps1');
const signingModule = read('docs/i18n/scripts/Meetily.Signing.psm1');
const rollbackModule = read('docs/i18n/scripts/Meetily.Rollback.psm1');
const rollbackWrapper = read('docs/i18n/scripts/invoke-meetily-controlled-rollback.ps1');
const releaseAudit = read('docs/i18n/scripts/audit-phase5a4b-release-signing.ps1');
const updaterVerifier = read('docs/i18n/scripts/verify-tauri-updater-signature.mjs');
const tauriConfig = JSON.parse(read('frontend/src-tauri/tauri.conf.json'));
const workflows = [
  '.github/workflows/build.yml',
  '.github/workflows/build-windows.yml',
  '.github/workflows/build-devtest.yml',
].map((relative) => ({ relative, text: read(relative) }));
const releaseWorkflow = read('.github/workflows/release.yml');

check('policy-schema-v1', policy.schemaVersion === 1 && policy.policyId === 'meetily-windows-production-signing-v1');
check('policy-sha256-authenticode', policy.authenticode.fileDigestAlgorithm === 'SHA256');
check('policy-code-signing-eku', policy.authenticode.codeSigningEkuOid === '1.3.6.1.5.5.7.3.3');
check('policy-trust-and-timestamp-required',
  policy.authenticode.requireTrustedSignerChain === true &&
  policy.authenticode.requireTrustedTimestampChain === true &&
  policy.authenticode.requireTimestampCertificate === true);
check('historical-signer-frozen',
  policy.authenticode.signerThumbprintSets.upstreamHistorical.includes('0472869976D42A9F74D03B8B9CE60CF7A3983A3B'));
check('production-signer-intentionally-locked',
  policy.authenticode.signerThumbprintSets.productionActive.length === 0 &&
  policy.productionControls.failWhenProductionThumbprintSetIsEmpty === true,
  'Production remains NO-GO until the release owner approves a current publisher certificate thumbprint.');
check('role-separation',
  policy.roles.ProductionArtifact.signerThumbprintSet === 'productionActive' &&
  policy.roles.RecoveryInstaller.signerThumbprintSet === 'productionActive' &&
  policy.roles.HistoricalRollbackTarget.signerThumbprintSet === 'upstreamHistorical');

check('sign-command-no-silent-skip', !/Skipping signing|exit\s+0\s*#?\s*skip/i.test(signScript));
check('sign-command-production-default', signScript.includes("'Production'") && signScript.includes('Unknown Windows signing mode'));
check('sign-command-explicit-timestamp', signScript.includes('--timestamp=true') && signScript.includes('--tool=signtool'));
check('sign-command-policy-verification', signScript.includes('Test-MeetilySignedFile') && signScript.includes('-Role ProductionArtifact'));
check('sign-command-audit-isolation',
  signScript.includes('MEETILY_ALLOW_UNSIGNED_WINDOWS_BUILD') &&
  signScript.includes('GITHUB_REF_TYPE') && signScript.includes("-ceq 'tag'") &&
  signScript.includes('GITHUB_EVENT_NAME') && signScript.includes("-ceq 'release'"));
check('prepare-script-policy-lock',
  prepareScript.includes('productionActive') && prepareScript.includes('Code Signing EKU') &&
  prepareScript.includes('certsync') && prepareScript.includes('healthcheck'));
check('signing-module-default-authenticode',
  signingModule.includes('verify /pa /all /tw /u 1.3.6.1.5.5.7.3.3 /q') &&
  signingModule.includes('signToolDefaultAuthenticodePassed'));
check('rollback-production-audit-override-forbidden',
  rollbackWrapper.includes("[ValidateSet('Production', 'Audit')][string]$SecurityMode = 'Production'") &&
  rollbackWrapper.includes("$AuditAllowUnsignedInstaller -and $SecurityMode -cne 'Audit'") &&
  rollbackModule.includes("$AuditAllowUnsigned -and $SecurityMode -cne 'Audit'"));
check('rollback-role-specific-verification',
  rollbackWrapper.includes('-SigningRole HistoricalRollbackTarget') &&
  rollbackWrapper.includes('-SigningRole RecoveryInstaller'));

const workflowText = workflows.map(({ text }) => text).join('\n');
for (const forbidden of [
  ['no-api-key-prefix-logging', /SM_API_KEY[^\n]*Substring|Write-Host[^\n]*SM_API_KEY[^\n]*\$env:SM_API_KEY/],
  ['no-keypair-list-logging', /smctl\s+keypair\s+(?:ls|list)|Write-Host\s+\$keypairOutput/],
  ['no-keypair-alias-value-logging', /Write-Host[^\n]*(?:DIGICERT_KEYPAIR_ALIAS=|\$keypairAlias)/],
  ['no-continue-after-certificate-failure', /Signing may fail\. Continuing|may cause issues with signing, but we(?:'|’)ll continue/i],
]) {
  check(forbidden[0], !forbidden[1].test(workflowText));
}
check('all-windows-workflows-use-policy-preflight', workflows.every(({ text }) => text.includes('prepare-digicert-signing.ps1')));
check('all-windows-workflows-use-release-gate', workflows.every(({ text }) => text.includes('audit-phase5a4b-release-signing.ps1')));
check('all-windows-workflows-use-explicit-signing-mode', workflows.every(({ text }) => text.includes('MEETILY_WINDOWS_SIGNING_MODE')));
check('production-release-forces-signing', releaseWorkflow.includes('uses: ./.github/workflows/build.yml') && releaseWorkflow.includes('sign-binaries: true'));
check('digi-cert-alias-is-a-secret', workflowText.includes('secrets.DIGICERT_KEYPAIR_ALIAS'));
check('certificate-material-cleanup', workflows.every(({ text }) => text.includes('Remove DigiCert client certificate material')));

const decodedUpdaterKey = Buffer.from(tauriConfig.plugins.updater.pubkey, 'base64').toString('utf8');
check('updater-artifacts-enabled', tauriConfig.bundle.createUpdaterArtifacts === true);
check('updater-public-key-id-frozen', decodedUpdaterKey.includes(policy.updater.publicKeyId));
check('release-gate-requires-updater-signatures',
  releaseAudit.includes('RequireUpdaterEnvironment') && releaseAudit.includes("$installer.FullName + '.sig'") &&
  releaseAudit.includes('updaterSignaturePresent') && releaseAudit.includes('updaterSignatureCryptographicallyValid'));
check('updater-signature-cryptographic-verifier',
  updaterVerifier.includes("createHash('blake2b512')") && updaterVerifier.includes('crypto.verify') &&
  updaterVerifier.includes('timingSafeEqual') && updaterVerifier.includes('trustedCommentSignatureValid'));

const formalArtifacts = [
  {
    flag: 'formal-exe',
    expectedFlag: 'formal-exe-sha256',
    label: 'formal-portable-exe',
  },
  {
    flag: 'formal-installer',
    expectedFlag: 'formal-installer-sha256',
    label: 'formal-nsis-installer',
  },
];
const formalEvidence = [];
for (const artifact of formalArtifacts) {
  if (!args.has(artifact.flag) && !args.has(artifact.expectedFlag)) continue;
  const file = path.resolve(args.get(artifact.flag) ?? '');
  const expected = (args.get(artifact.expectedFlag) ?? '').toUpperCase();
  const exists = fs.existsSync(file) && fs.statSync(file).isFile();
  const actual = exists ? sha256(file) : null;
  check(`${artifact.label}-exists`, exists, file);
  check(`${artifact.label}-hash-unchanged`, exists && /^[A-F0-9]{64}$/.test(expected) && actual === expected,
    `expected=${expected}; actual=${actual}`);
  formalEvidence.push({ label: artifact.label, path: file, expectedSha256: expected, actualSha256: actual });
}

const failed = assertions.filter((item) => !item.passed);
const productionBlockers = [];
if (policy.authenticode.signerThumbprintSets.productionActive.length === 0) {
  productionBlockers.push('No current production publisher certificate thumbprint has been approved.');
}
productionBlockers.push('No DigiCert/SMCTL credentials or Tauri updater private key are available in the local audit environment.');
productionBlockers.push('No current Chinese release artifact has passed real production Authenticode and updater-signature verification.');

const report = {
  schemaVersion: 1,
  audit: 'Meetily phase 5A-4B production signing static audit',
  generatedAtUtc: new Date().toISOString(),
  repo,
  passed: failed.length === 0,
  productionReady: false,
  assertionCount: assertions.length,
  failedCount: failed.length,
  assertions,
  policy: {
    path: policyPath,
    sha256: sha256(policyPath),
    productionActiveThumbprintCount: policy.authenticode.signerThumbprintSets.productionActive.length,
    historicalThumbprints: policy.authenticode.signerThumbprintSets.upstreamHistorical,
    updaterPublicKeyId: policy.updater.publicKeyId,
  },
  formalEvidence,
  productionBlockers,
};

if (reportPath) {
  fs.mkdirSync(path.dirname(reportPath), { recursive: true });
  fs.writeFileSync(reportPath, `${JSON.stringify(report, null, 2)}\n`, 'utf8');
}
process.stdout.write(`${JSON.stringify(report, null, 2)}\n`);
if (!report.passed) process.exitCode = 1;

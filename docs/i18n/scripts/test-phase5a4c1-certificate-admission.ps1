[CmdletBinding()]
param(
    [string]$PolicyPath,
    [string]$AdmissionPath,
    [string]$TauriConfigPath,
    [string]$ReportPath
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
if ([string]::IsNullOrWhiteSpace($PolicyPath)) { $PolicyPath = Join-Path $PSScriptRoot '..\phase-5a4\windows-signing-policy.v1.json' }
if ([string]::IsNullOrWhiteSpace($AdmissionPath)) { $AdmissionPath = Join-Path $PSScriptRoot '..\phase-5a4\windows-certificate-admission.v1.json' }
if ([string]::IsNullOrWhiteSpace($TauriConfigPath)) { $TauriConfigPath = Join-Path $PSScriptRoot '..\..\..\frontend\src-tauri\tauri.conf.json' }
Import-Module (Join-Path $PSScriptRoot 'Meetily.Signing.psm1') -Force -ErrorAction Stop
Import-Module (Join-Path $PSScriptRoot 'Meetily.CertificateAdmission.psm1') -Force -ErrorAction Stop

function Copy-JsonObject { param($Value) return ($Value | ConvertTo-Json -Depth 20 | ConvertFrom-Json) }
$policy = Read-MeetilyWindowsSigningPolicy -Path $PolicyPath
$pendingAdmission = Read-MeetilyWindowsCertificateAdmission -Path $AdmissionPath
$tests = New-Object System.Collections.Generic.List[object]
function Add-Test { param([string]$Name, [bool]$Passed, [string]$Detail = '') $tests.Add([ordered]@{ name = $Name; passed = $Passed; detail = $Detail }) }

$thumbprint = 'AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA'
$now = [DateTimeOffset]::UtcNow
$approvedPolicy = Copy-JsonObject $policy
$approvedPolicy.authenticode.signerThumbprintSets.productionActive = @($thumbprint)
$approvedAdmission = Copy-JsonObject $pendingAdmission
$approvedAdmission.status = 'Approved'
$approvedAdmission.candidate.subject = [string]$policy.authenticode.approvedSignerSubjects[0]
$approvedAdmission.candidate.sha1Thumbprint = $thumbprint
$approvedAdmission.candidate.serialNumber = '0123456789ABCDEF'
$approvedAdmission.candidate.issuer = 'CN=Trusted Public CA'
$approvedAdmission.candidate.notBeforeUtc = $now.AddDays(-10).ToString('o')
$approvedAdmission.candidate.notAfterUtc = $now.AddDays(365).ToString('o')
foreach ($name in @('releaseOwner', 'securityReviewer')) {
    $approvedAdmission.approvals.$name.status = 'Approved'
    $approvedAdmission.approvals.$name.approvedAtUtc = $now.ToString('o')
    $approvedAdmission.approvals.$name.approvalReference = "review:$name:fixture"
}
$certificate = [pscustomobject][ordered]@{
    found = $true
    subject = [string]$approvedAdmission.candidate.subject
    sha1Thumbprint = $thumbprint
    serialNumber = '0123456789ABCDEF'
    issuer = 'CN=Trusted Public CA'
    notBeforeUtc = $now.AddDays(-10).ToString('o')
    notAfterUtc = $now.AddDays(365).ToString('o')
    ekuOids = @('1.3.6.1.5.5.7.3.3')
    chainTrusted = $true
    chainStatus = @('NoError')
    selfSigned = $false
}
$presence = [ordered]@{}
foreach ($name in @($policy.productionControls.requiredDigiCertEnvironmentVariables) + @($policy.updater.privateKeyEnvironmentVariables)) { $presence[[string]$name] = $true }
$validEvidence = [pscustomobject][ordered]@{
    environmentPresence = [pscustomobject]$presence
    configuration = [pscustomobject]@{ configuredThumbprintPresent = $true; configuredThumbprintMatchesAdmission = $true }
    tools = [pscustomobject]@{ smctlAvailable = $true; signToolAvailable = $true }
    digiCert = [pscustomobject]@{ clientCertificateFilePresent = $true; healthcheckPassed = $true; certificateSyncPassed = $true }
    certificate = $certificate
    updater = [pscustomobject]@{ publicKeyId = [string]$policy.updater.publicKeyId }
    proofOfPossession = [pscustomobject]@{
        passed = $true
        signerSubject = [string]$approvedAdmission.candidate.subject
        signerThumbprint = $thumbprint
        timestampCertificatePresent = $true
        signToolVerificationPassed = $true
    }
}

$pendingGate = Test-MeetilyCertificateApprovalRecord -Policy $policy -Admission $pendingAdmission
Add-Test 'pending-admission-fails-closed' (-not $pendingGate.passed) ($pendingGate.failures -join ' ')
Add-Test 'production-active-remains-empty-before-approval' (@($policy.authenticode.signerThumbprintSets.productionActive).Count -eq 0)
$validGate = Test-MeetilyCertificateApprovalRecord -Policy $approvedPolicy -Admission $approvedAdmission
Add-Test 'approved-record-with-two-independent-approvals-passes' $validGate.passed ($validGate.failures -join ' ')
$validReadiness = Test-MeetilyCertificateAdmissionReadiness -Evidence $validEvidence -Policy $approvedPolicy -Admission $approvedAdmission
Add-Test 'complete-synthetic-readiness-passes' $validReadiness.passed ($validReadiness.failures -join ' ')

$casePolicy = Copy-JsonObject $approvedPolicy; $casePolicy.authenticode.signerThumbprintSets.productionActive = @($thumbprint, 'BBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB')
$case = Test-MeetilyCertificateApprovalRecord -Policy $casePolicy -Admission $approvedAdmission
Add-Test 'reject-multiple-production-active-thumbprints' (-not $case.passed) ($case.failures -join ' ')
$historicalPolicy = Copy-JsonObject $approvedPolicy; $historicalThumb = [string]$policy.authenticode.signerThumbprintSets.upstreamHistorical[0]; $historicalPolicy.authenticode.signerThumbprintSets.productionActive = @($historicalThumb)
$historicalAdmission = Copy-JsonObject $approvedAdmission; $historicalAdmission.candidate.sha1Thumbprint = $historicalThumb
$case = Test-MeetilyCertificateApprovalRecord -Policy $historicalPolicy -Admission $historicalAdmission
Add-Test 'reject-historical-thumbprint-reuse' (-not $case.passed -and @($case.failures) -contains 'Historical signer thumbprints cannot be admitted for current production signing.') ($case.failures -join ' ')
$caseAdmission = Copy-JsonObject $approvedAdmission; $caseAdmission.candidate.subject = 'CN=Unapproved Publisher'
$case = Test-MeetilyCertificateApprovalRecord -Policy $approvedPolicy -Admission $caseAdmission
Add-Test 'reject-unapproved-subject' (-not $case.passed) ($case.failures -join ' ')
$caseAdmission = Copy-JsonObject $approvedAdmission; $caseAdmission.candidate.sha1Thumbprint = 'BAD'
$case = Test-MeetilyCertificateApprovalRecord -Policy $approvedPolicy -Admission $caseAdmission
Add-Test 'reject-malformed-candidate-thumbprint' (-not $case.passed) ($case.failures -join ' ')
$caseAdmission = Copy-JsonObject $approvedAdmission; $caseAdmission.approvals.securityReviewer.status = 'Pending'; $caseAdmission.approvals.securityReviewer.approvalReference = $null
$case = Test-MeetilyCertificateApprovalRecord -Policy $approvedPolicy -Admission $caseAdmission
Add-Test 'reject-missing-independent-approval' (-not $case.passed) ($case.failures -join ' ')
$caseAdmission = Copy-JsonObject $approvedAdmission; $caseAdmission.candidate.serialNumber = $null
$case = Test-MeetilyCertificateApprovalRecord -Policy $approvedPolicy -Admission $caseAdmission
Add-Test 'reject-incomplete-candidate-public-fields' (-not $case.passed -and @($case.failures) -contains 'Approved candidate serial number is missing.') ($case.failures -join ' ')
$caseAdmission = Copy-JsonObject $approvedAdmission; $caseAdmission.approvals.PSObject.Properties.Remove('securityReviewer')
$case = Test-MeetilyCertificateApprovalRecord -Policy $approvedPolicy -Admission $caseAdmission
Add-Test 'reject-missing-approval-object' (-not $case.passed -and @($case.failures) -contains "Required approval 'securityReviewer' is not Approved.") ($case.failures -join ' ')

function Test-CertificateNegative {
    param([string]$Name, [scriptblock]$Mutate, [string]$Expected)
    $candidate = Copy-JsonObject $certificate
    & $Mutate $candidate
    $result = Test-MeetilyCertificatePublicEvidence -Evidence $candidate -Policy $approvedPolicy -Admission $approvedAdmission -EvaluatedAtUtc $now
    Add-Test $Name ((-not $result.passed) -and (@($result.failures) -contains $Expected)) ($result.failures -join ' ')
}
Test-CertificateNegative 'reject-certificate-not-found' { param($x) $x.found = $false } 'Configured certificate was not found in the Windows certificate stores.'
Test-CertificateNegative 'reject-certificate-thumbprint-mismatch' { param($x) $x.sha1Thumbprint = 'BBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB' } 'Synchronized certificate thumbprint does not match the admission record.'
Test-CertificateNegative 'reject-self-signed-certificate' { param($x) $x.selfSigned = $true } 'Self-signed certificates are forbidden for production admission.'
Test-CertificateNegative 'reject-missing-code-signing-eku' { param($x) $x.ekuOids = @('1.3.6.1.5.5.7.3.1') } 'Synchronized certificate does not contain the Code Signing EKU.'
Test-CertificateNegative 'reject-untrusted-certificate-chain' { param($x) $x.chainTrusted = $false } 'Synchronized certificate chain is not trusted.'
Test-CertificateNegative 'reject-expiring-certificate' { param($x) $x.notAfterUtc = $now.AddDays(2).ToString('o') } 'Synchronized certificate does not meet the minimum remaining-validity requirement.'
Test-CertificateNegative 'reject-certificate-validity-record-mismatch' { param($x) $x.notBeforeUtc = $now.AddDays(-9).ToString('o') } 'Synchronized certificate validity interval does not match the admission record.'

function Test-ReadinessNegative {
    param([string]$Name, [scriptblock]$Mutate, [string]$Expected)
    $candidate = Copy-JsonObject $validEvidence
    & $Mutate $candidate
    $result = Test-MeetilyCertificateAdmissionReadiness -Evidence $candidate -Policy $approvedPolicy -Admission $approvedAdmission
    Add-Test $Name ((-not $result.passed) -and (@($result.failures) -contains $Expected)) ($result.failures -join ' ')
}
Test-ReadinessNegative 'reject-missing-smctl' { param($x) $x.tools.smctlAvailable = $false } 'smctl is not available on PATH.'
Test-ReadinessNegative 'reject-missing-signtool' { param($x) $x.tools.signToolAvailable = $false } 'Windows SDK SignTool is not available.'
Test-ReadinessNegative 'reject-failed-digicert-healthcheck' { param($x) $x.digiCert.healthcheckPassed = $false } 'DigiCert healthcheck has not passed.'
Test-ReadinessNegative 'reject-failed-certificate-sync' { param($x) $x.digiCert.certificateSyncPassed = $false } 'DigiCert certificate synchronization has not passed.'
Test-ReadinessNegative 'reject-updater-public-key-id-mismatch' { param($x) $x.updater.publicKeyId = '0000000000000000' } 'Tauri updater public-key ID does not match signing policy.'
Test-ReadinessNegative 'reject-missing-updater-private-key' { param($x) $x.environmentPresence.TAURI_SIGNING_PRIVATE_KEY = $false } "Required environment entry 'TAURI_SIGNING_PRIVATE_KEY' is absent."
Test-ReadinessNegative 'reject-missing-proof-of-possession' { param($x) $x.proofOfPossession.passed = $false } 'Disposable-artifact proof of possession has not passed.'
Test-ReadinessNegative 'reject-missing-client-certificate-file' { param($x) $x.digiCert.clientCertificateFilePresent = $false } 'DigiCert client certificate file is absent.'
Test-ReadinessNegative 'reject-configured-thumbprint-mismatch' { param($x) $x.configuration.configuredThumbprintMatchesAdmission = $false } 'Configured DigiCert thumbprint does not match the admission record.'
Test-ReadinessNegative 'reject-proof-subject-mismatch' { param($x) $x.proofOfPossession.signerSubject = 'CN=Wrong Proof Signer' } 'Proof-of-possession signer subject does not match the admitted certificate.'
Test-ReadinessNegative 'reject-proof-without-timestamp' { param($x) $x.proofOfPossession.timestampCertificatePresent = $false } 'Proof-of-possession timestamp certificate is missing.'
Test-ReadinessNegative 'reject-proof-without-signtool-verification' { param($x) $x.proofOfPossession.signToolVerificationPassed = $false } 'Proof-of-possession SignTool verification has not passed.'

$actualKeyId = Get-MeetilyTauriPublicKeyId -TauriConfigPath $TauriConfigPath
Add-Test 'tauri-public-key-id-matches-policy' ($actualKeyId -ceq [string]$policy.updater.publicKeyId) "actual=$actualKeyId"
$secretName = 'MEETILY_PHASE5A4C1_TEST_SECRET'
$secretValue = 'SENTINEL-SECRET-MUST-NOT-APPEAR-7F3A'
$savedSecret = [Environment]::GetEnvironmentVariable($secretName)
try {
    [Environment]::SetEnvironmentVariable($secretName, $secretValue)
    $redactedPresence = Get-MeetilyEnvironmentPresence -Names @($secretName)
    $serializedPresence = $redactedPresence | ConvertTo-Json
    Add-Test 'environment-audit-emits-presence-not-secret-value' ($redactedPresence.$secretName -eq $true -and $serializedPresence -notmatch [regex]::Escape($secretValue)) $serializedPresence
} finally {
    [Environment]::SetEnvironmentVariable($secretName, $savedSecret)
}

$failed = @($tests | Where-Object { -not $_.passed })
$report = [ordered]@{
    schemaVersion = 1
    testSuite = 'Meetily phase 5A-4C-1 certificate admission'
    generatedAtUtc = [DateTime]::UtcNow.ToString('o')
    powershellVersion = $PSVersionTable.PSVersion.ToString()
    passed = ($failed.Count -eq 0)
    total = $tests.Count
    failed = $failed.Count
    tests = @($tests.ToArray())
}
if (-not [string]::IsNullOrWhiteSpace($ReportPath)) {
    $resolvedReport = [IO.Path]::GetFullPath($ReportPath)
    $parent = Split-Path -Parent $resolvedReport
    if (-not (Test-Path -LiteralPath $parent -PathType Container)) { New-Item -ItemType Directory -Path $parent -Force | Out-Null }
    $report | ConvertTo-Json -Depth 12 | Set-Content -LiteralPath $resolvedReport -Encoding UTF8
}
$report | ConvertTo-Json -Depth 12
if (-not $report.passed) { exit 1 }

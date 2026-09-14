[CmdletBinding()]
param(
    [string]$PolicyPath,
    [string]$AdmissionPath
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
if ([string]::IsNullOrWhiteSpace($PolicyPath)) {
    $PolicyPath = Join-Path $PSScriptRoot '..\..\..\docs\i18n\phase-5a4\windows-signing-policy.v1.json'
}
if ([string]::IsNullOrWhiteSpace($AdmissionPath)) {
    $AdmissionPath = Join-Path $PSScriptRoot '..\..\..\docs\i18n\phase-5a4\windows-certificate-admission.v1.json'
}

$modulePath = Join-Path $PSScriptRoot '..\..\..\docs\i18n\scripts\Meetily.Signing.psm1'
Import-Module $modulePath -Force -ErrorAction Stop
$admissionModulePath = Join-Path $PSScriptRoot '..\..\..\docs\i18n\scripts\Meetily.CertificateAdmission.psm1'
Import-Module $admissionModulePath -Force -ErrorAction Stop
$policy = Read-MeetilyWindowsSigningPolicy -Path $PolicyPath
$admission = Read-MeetilyWindowsCertificateAdmission -Path $AdmissionPath
$approvalGate = Test-MeetilyCertificateApprovalRecord -Policy $policy -Admission $admission
if (-not $approvalGate.passed) {
    throw "The production certificate admission record is not approved: $($approvalGate.failures -join ' ')"
}

$required = @($policy.productionControls.requiredDigiCertEnvironmentVariables)
$missing = @(
    foreach ($name in $required) {
        if ([string]::IsNullOrWhiteSpace([Environment]::GetEnvironmentVariable([string]$name))) {
            [string]$name
        }
    }
)
if ($missing.Count -gt 0) {
    throw "DigiCert production signing is not configured. Missing: $($missing -join ', ')."
}
if (-not (Get-Command smctl -ErrorAction SilentlyContinue)) { throw 'smctl is not available on PATH.' }
if (-not (Test-Path -LiteralPath $env:SM_CLIENT_CERT_FILE -PathType Leaf)) {
    throw 'The DigiCert client authentication certificate file does not exist.'
}

$activeThumbprints = @(
    @($policy.authenticode.signerThumbprintSets.productionActive) |
        ForEach-Object { ConvertTo-MeetilyNormalizedThumbprint ([string]$_) }
)
if ($activeThumbprints.Count -eq 0) {
    throw 'The productionActive signer thumbprint set is empty. Production signing remains locked.'
}
$configuredThumbprint = ConvertTo-MeetilyNormalizedThumbprint $env:SM_CODE_SIGNING_CERT_SHA1_HASH
if ($activeThumbprints -cnotcontains $configuredThumbprint) {
    throw 'The configured code-signing certificate thumbprint is not approved by policy.'
}

$null = @(& smctl healthcheck 2>&1)
if ($LASTEXITCODE -ne 0) { throw "DigiCert healthcheck failed with exit code $LASTEXITCODE." }
$null = @(& smctl windows certsync --keypair-alias $env:DIGICERT_KEYPAIR_ALIAS 2>&1)
if ($LASTEXITCODE -ne 0) { throw "DigiCert certificate synchronization failed with exit code $LASTEXITCODE." }

$certificate = @(
    Get-ChildItem -Path Cert:\CurrentUser\My -ErrorAction SilentlyContinue
    Get-ChildItem -Path Cert:\LocalMachine\My -ErrorAction SilentlyContinue
) | Where-Object {
    (ConvertTo-MeetilyNormalizedThumbprint $_.Thumbprint) -ceq $configuredThumbprint
} | Select-Object -First 1
if (-not $certificate) { throw 'The approved code-signing certificate was not found after synchronization.' }
if (@($policy.authenticode.approvedSignerSubjects) -cnotcontains [string]$certificate.Subject) {
    throw 'The synchronized certificate subject is not approved by policy.'
}
$ekuOids = @(
    foreach ($extension in @($certificate.Extensions)) {
        if ($extension.Oid.Value -eq '2.5.29.37') {
            foreach ($eku in @($extension.EnhancedKeyUsages)) { [string]$eku.Value }
        }
    }
)
if ($ekuOids -cnotcontains [string]$policy.authenticode.codeSigningEkuOid) {
    throw 'The synchronized certificate does not contain the Code Signing EKU.'
}
$now = Get-Date
if ($now -lt $certificate.NotBefore -or $now -gt $certificate.NotAfter) {
    throw 'The synchronized code-signing certificate is not currently valid.'
}
$publicEvidence = Get-MeetilyCertificatePublicEvidence -Certificate $certificate
$certificateGate = Test-MeetilyCertificatePublicEvidence `
    -Evidence $publicEvidence `
    -Policy $policy `
    -Admission $admission
if (-not $certificateGate.passed) {
    throw "The synchronized code-signing certificate failed admission policy: $($certificateGate.failures -join ' ')"
}

Write-Host 'DigiCert production signing prerequisites passed.'
Write-Host "Approved signer subject: $($certificate.Subject)"
Write-Host "Approved signer thumbprint: $configuredThumbprint"

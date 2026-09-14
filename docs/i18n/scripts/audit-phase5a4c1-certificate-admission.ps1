[CmdletBinding()]
param(
    [string]$PolicyPath,
    [string]$AdmissionPath,
    [string]$TauriConfigPath,
    [string]$ReportPath,
    [string]$ProofOfPossessionPePath,
    [switch]$RunDigiCertConnectivity,
    [switch]$EnforceReady
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
if ([string]::IsNullOrWhiteSpace($PolicyPath)) {
    $PolicyPath = Join-Path $PSScriptRoot '..\phase-5a4\windows-signing-policy.v1.json'
}
if ([string]::IsNullOrWhiteSpace($AdmissionPath)) {
    $AdmissionPath = Join-Path $PSScriptRoot '..\phase-5a4\windows-certificate-admission.v1.json'
}
if ([string]::IsNullOrWhiteSpace($TauriConfigPath)) {
    $TauriConfigPath = Join-Path $PSScriptRoot '..\..\..\frontend\src-tauri\tauri.conf.json'
}

Import-Module (Join-Path $PSScriptRoot 'Meetily.Signing.psm1') -Force -ErrorAction Stop
Import-Module (Join-Path $PSScriptRoot 'Meetily.CertificateAdmission.psm1') -Force -ErrorAction Stop
$policy = Read-MeetilyWindowsSigningPolicy -Path $PolicyPath
$admission = Read-MeetilyWindowsCertificateAdmission -Path $AdmissionPath
$environmentNames = @($policy.productionControls.requiredDigiCertEnvironmentVariables) + @($policy.updater.privateKeyEnvironmentVariables)
$environmentPresence = Get-MeetilyEnvironmentPresence -Names $environmentNames

$smctlCommand = Get-Command smctl -ErrorAction SilentlyContinue | Select-Object -First 1
$signToolPath = $null
try { $signToolPath = Resolve-MeetilySignTool } catch { $signToolPath = $null }
$digiCertPresence = @($policy.productionControls.requiredDigiCertEnvironmentVariables | ForEach-Object {
    [bool]$environmentPresence.PSObject.Properties[[string]$_].Value
})
$digiCertEnvironmentReady = ($digiCertPresence.Count -gt 0 -and @($digiCertPresence | Where-Object { -not $_ }).Count -eq 0)
$clientCertificateFilePresent = $false
if ([bool]$environmentPresence.SM_CLIENT_CERT_FILE) {
    $clientCertificateFilePresent = Test-Path -LiteralPath ([Environment]::GetEnvironmentVariable('SM_CLIENT_CERT_FILE')) -PathType Leaf
}

$healthcheckExecuted = $false
$healthcheckPassed = $false
$certificateSyncExecuted = $false
$certificateSyncPassed = $false
if ($RunDigiCertConnectivity -and $smctlCommand -and $digiCertEnvironmentReady -and $clientCertificateFilePresent) {
    $healthcheckExecuted = $true
    $null = @(& $smctlCommand.Source healthcheck 2>&1)
    $healthcheckPassed = ($LASTEXITCODE -eq 0)
    if ($healthcheckPassed) {
        $certificateSyncExecuted = $true
        $null = @(& $smctlCommand.Source windows certsync --keypair-alias ([Environment]::GetEnvironmentVariable('DIGICERT_KEYPAIR_ALIAS')) 2>&1)
        $certificateSyncPassed = ($LASTEXITCODE -eq 0)
    }
}

$configuredThumbprint = ''
if ([bool]$environmentPresence.SM_CODE_SIGNING_CERT_SHA1_HASH) {
    $configuredThumbprint = ConvertTo-MeetilyNormalizedThumbprint ([Environment]::GetEnvironmentVariable('SM_CODE_SIGNING_CERT_SHA1_HASH'))
}
$certificate = $null
if ($configuredThumbprint -cmatch '^[0-9A-F]{40}$') {
    $certificate = @(
        Get-ChildItem -Path Cert:\CurrentUser\My -ErrorAction SilentlyContinue
        Get-ChildItem -Path Cert:\LocalMachine\My -ErrorAction SilentlyContinue
    ) | Where-Object {
        (ConvertTo-MeetilyNormalizedThumbprint $_.Thumbprint) -ceq $configuredThumbprint
    } | Select-Object -First 1
}
$certificateEvidence = Get-MeetilyCertificatePublicEvidence -Certificate $certificate

$proof = [ordered]@{
    requested = -not [string]::IsNullOrWhiteSpace($ProofOfPossessionPePath)
    executed = $false
    passed = $false
    signerSubject = $null
    signerThumbprint = $null
    timestampCertificatePresent = $false
    signToolVerificationPassed = $false
}
$tempRoot = $null
try {
    $candidateThumbprint = ConvertTo-MeetilyNormalizedThumbprint ([string]$admission.candidate.sha1Thumbprint)
    $candidateSubject = [string]$admission.candidate.subject
    $historical = @($policy.authenticode.signerThumbprintSets.upstreamHistorical | ForEach-Object {
        ConvertTo-MeetilyNormalizedThumbprint ([string]$_)
    })
    $proofPrerequisitesPassed = (
        $proof.requested -and $healthcheckPassed -and $certificateSyncPassed -and $signToolPath -and
        $certificateEvidence.found -and $candidateThumbprint -cmatch '^[0-9A-F]{40}$' -and
        $configuredThumbprint -ceq $candidateThumbprint -and
        $historical -cnotcontains $candidateThumbprint -and
        @($policy.authenticode.approvedSignerSubjects) -ccontains $candidateSubject -and
        (Test-Path -LiteralPath $ProofOfPossessionPePath -PathType Leaf)
    )
    if ($proofPrerequisitesPassed) {
        $tempRoot = Join-Path ([IO.Path]::GetTempPath()) ('MeetilyPhase5A4C1Proof-' + [guid]::NewGuid().ToString('N'))
        New-Item -ItemType Directory -Path $tempRoot | Out-Null
        $proofPath = Join-Path $tempRoot 'disposable-proof.exe'
        Copy-Item -LiteralPath $ProofOfPossessionPePath -Destination $proofPath
        $proof.executed = $true
        $null = @(
            & $smctlCommand.Source sign `
                --keypair-alias ([Environment]::GetEnvironmentVariable('DIGICERT_KEYPAIR_ALIAS')) `
                --input $proofPath `
                --tool=signtool `
                --timestamp=true 2>&1
        )
        if ($LASTEXITCODE -eq 0) {
            $signed = Get-MeetilyAuthenticodeEvidence -Path $proofPath -SignToolPath $signToolPath
            $proof.signerSubject = $signed.signerSubject
            $proof.signerThumbprint = $signed.signerThumbprint
            $proof.timestampCertificatePresent = [bool]$signed.timestampCertificatePresent
            $proof.signToolVerificationPassed = [bool]$signed.signToolDefaultAuthenticodePassed
            $proof.passed = (
                [string]$signed.signatureStatus -ceq 'Valid' -and
                [string]$signed.signerSubject -ceq $candidateSubject -and
                [string]$signed.signerThumbprint -ceq $candidateThumbprint -and
                @($signed.signerEkuOids) -ccontains [string]$policy.authenticode.codeSigningEkuOid -and
                [bool]$signed.timestampCertificatePresent -and
                [bool]$signed.signToolDefaultAuthenticodePassed
            )
        }
    }
} finally {
    if (-not [string]::IsNullOrWhiteSpace($tempRoot) -and (Test-Path -LiteralPath $tempRoot -PathType Container)) {
        $resolvedTemp = [IO.Path]::GetFullPath($tempRoot)
        $expectedPrefix = [IO.Path]::GetFullPath([IO.Path]::GetTempPath()).TrimEnd('\') + '\MeetilyPhase5A4C1Proof-'
        if (-not $resolvedTemp.StartsWith($expectedPrefix, [StringComparison]::OrdinalIgnoreCase)) {
            throw 'Refusing to remove an unexpected proof-of-possession directory.'
        }
        Remove-Item -LiteralPath $resolvedTemp -Recurse -Force
    }
}

$updaterPublicKeyId = Get-MeetilyTauriPublicKeyId -TauriConfigPath $TauriConfigPath
$evidence = [pscustomobject][ordered]@{
    environmentPresence = $environmentPresence
    configuration = [pscustomobject][ordered]@{
        configuredThumbprintPresent = ($configuredThumbprint -cmatch '^[0-9A-F]{40}$')
        configuredThumbprintMatchesAdmission = (
            $configuredThumbprint -cmatch '^[0-9A-F]{40}$' -and
            $configuredThumbprint -ceq (ConvertTo-MeetilyNormalizedThumbprint ([string]$admission.candidate.sha1Thumbprint))
        )
    }
    tools = [pscustomobject][ordered]@{
        smctlAvailable = ($null -ne $smctlCommand)
        signToolAvailable = (-not [string]::IsNullOrWhiteSpace($signToolPath))
    }
    digiCert = [pscustomobject][ordered]@{
        clientCertificateFilePresent = $clientCertificateFilePresent
        healthcheckExecuted = $healthcheckExecuted
        healthcheckPassed = $healthcheckPassed
        certificateSyncExecuted = $certificateSyncExecuted
        certificateSyncPassed = $certificateSyncPassed
    }
    certificate = $certificateEvidence
    updater = [pscustomobject][ordered]@{
        publicKeyId = $updaterPublicKeyId
    }
    proofOfPossession = [pscustomobject]$proof
}
$readiness = Test-MeetilyCertificateAdmissionReadiness -Evidence $evidence -Policy $policy -Admission $admission
$report = [ordered]@{
    schemaVersion = 1
    phase = '5A-4C-1'
    audit = 'Windows production certificate admission preflight'
    generatedAtUtc = [DateTime]::UtcNow.ToString('o')
    policyPath = [IO.Path]::GetFullPath($PolicyPath)
    policySha256 = (Get-FileHash -Algorithm SHA256 -LiteralPath $PolicyPath).Hash
    admissionPath = [IO.Path]::GetFullPath($AdmissionPath)
    admissionSha256 = (Get-FileHash -Algorithm SHA256 -LiteralPath $AdmissionPath).Hash
    admissionStatus = [string]$admission.status
    admissionReady = [bool]$readiness.passed
    signedRcRequired = $true
    productionReady = $false
    admissionDecision = if ($readiness.passed) { 'GO-FOR-SIGNED-RC' } else { 'NO-GO' }
    releaseDecision = 'NO-GO'
    readiness = $readiness
    evidence = $evidence
    security = [ordered]@{
        secretValuesEmitted = $false
        environmentValuesEmitted = $false
        keypairAliasEmitted = $false
        privateKeyMaterialRead = $false
    }
}
if (-not [string]::IsNullOrWhiteSpace($ReportPath)) {
    $resolvedReport = [IO.Path]::GetFullPath($ReportPath)
    $parent = Split-Path -Parent $resolvedReport
    if (-not (Test-Path -LiteralPath $parent -PathType Container)) { New-Item -ItemType Directory -Path $parent -Force | Out-Null }
    $report | ConvertTo-Json -Depth 12 | Set-Content -LiteralPath $resolvedReport -Encoding UTF8
}
$report | ConvertTo-Json -Depth 12
if ($EnforceReady -and -not $readiness.passed) { exit 1 }

[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)][string]$ArtifactRoot,
    [string]$PolicyPath,
    [string]$TauriConfigPath,
    [string]$ReportPath,
    [switch]$RequireUpdaterEnvironment
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
if ([string]::IsNullOrWhiteSpace($PolicyPath)) {
    $PolicyPath = Join-Path $PSScriptRoot '..\phase-5a4\windows-signing-policy.v1.json'
}
if ([string]::IsNullOrWhiteSpace($TauriConfigPath)) {
    $TauriConfigPath = Join-Path $PSScriptRoot '..\..\..\frontend\src-tauri\tauri.conf.json'
}
Import-Module Microsoft.PowerShell.Utility -ErrorAction Stop
Import-Module (Join-Path $PSScriptRoot 'Meetily.Signing.psm1') -Force -ErrorAction Stop

$root = [IO.Path]::GetFullPath($ArtifactRoot)
if (-not (Test-Path -LiteralPath $root -PathType Container)) { throw "Artifact root was not found: $root" }
$policy = Read-MeetilyWindowsSigningPolicy -Path $PolicyPath
$configPath = [IO.Path]::GetFullPath($TauriConfigPath)
$config = Get-Content -LiteralPath $configPath -Raw | ConvertFrom-Json
$failures = New-Object System.Collections.Generic.List[string]
$nodeCommand = Get-Command node.exe -ErrorAction SilentlyContinue | Select-Object -First 1
if (-not $nodeCommand) { $nodeCommand = Get-Command node -ErrorAction SilentlyContinue | Select-Object -First 1 }
if (-not $nodeCommand) { $failures.Add('Node.js is required to verify Tauri updater signatures.') }
$updaterVerifier = Join-Path $PSScriptRoot 'verify-tauri-updater-signature.mjs'

if (-not [bool]$config.bundle.createUpdaterArtifacts) { $failures.Add('Tauri createUpdaterArtifacts is not enabled.') }
$decodedPublicKey = ''
try {
    $decodedPublicKey = [Text.Encoding]::UTF8.GetString([Convert]::FromBase64String([string]$config.plugins.updater.pubkey))
} catch {
    $failures.Add('Tauri updater public key is not valid base64.')
}
if ($decodedPublicKey -notmatch [regex]::Escape([string]$policy.updater.publicKeyId)) {
    $failures.Add('Tauri updater public key ID does not match signing policy.')
}
if ($RequireUpdaterEnvironment) {
    foreach ($name in @($policy.updater.privateKeyEnvironmentVariables)) {
        if ([string]::IsNullOrWhiteSpace([Environment]::GetEnvironmentVariable([string]$name))) {
            $failures.Add("Updater signing environment variable is missing: $name.")
        }
    }
}

$installers = @(
    Get-ChildItem -LiteralPath $root -Recurse -File -ErrorAction Stop |
        Where-Object { $_.Extension -in @('.exe', '.msi') }
)
if ($installers.Count -eq 0) { $failures.Add('No Windows EXE or MSI release artifact was found.') }
$artifactResults = @()
foreach ($installer in $installers) {
    $verification = Test-MeetilySignedFile `
        -Path $installer.FullName `
        -PolicyPath $PolicyPath `
        -Role ProductionArtifact
    $signaturePath = $installer.FullName + '.sig'
    $signaturePresent = Test-Path -LiteralPath $signaturePath -PathType Leaf
    $signatureBytes = if ($signaturePresent) { (Get-Item -LiteralPath $signaturePath).Length } else { 0 }
    $updaterVerification = $null
    if (-not $verification.passed) {
        $failures.Add("Authenticode policy failed for $($installer.Name): $($verification.failures -join ' ')")
    }
    if (-not $signaturePresent -or $signatureBytes -le 0) {
        $failures.Add("Tauri updater detached signature is missing or empty for $($installer.Name).")
    } elseif ($nodeCommand) {
        $verificationOutput = @(
            & $nodeCommand.Source $updaterVerifier `
                --artifact $installer.FullName `
                --signature $signaturePath `
                --tauri-config $configPath 2>&1
        )
        $verificationExitCode = $LASTEXITCODE
        if ($verificationExitCode -eq 0) {
            try {
                $updaterVerification = ($verificationOutput -join [Environment]::NewLine) | ConvertFrom-Json
            } catch {
                $failures.Add("Updater signature verifier returned malformed evidence for $($installer.Name).")
            }
        } else {
            $failures.Add("Tauri updater cryptographic signature verification failed for $($installer.Name).")
        }
        if ($updaterVerification -and -not [bool]$updaterVerification.passed) {
            $failures.Add("Tauri updater cryptographic signature verification did not pass for $($installer.Name).")
        }
    }
    $artifactResults += [ordered]@{
        name = $installer.Name
        path = $installer.FullName
        sha256 = $verification.evidence.sha256
        authenticodePassed = $verification.passed
        signerSubject = $verification.evidence.signerSubject
        signerThumbprint = $verification.evidence.signerThumbprint
        timestampCertificatePresent = $verification.evidence.timestampCertificatePresent
        signToolVerificationPassed = $verification.evidence.signToolDefaultAuthenticodePassed
        updaterSignaturePath = $signaturePath
        updaterSignaturePresent = $signaturePresent
        updaterSignatureBytes = $signatureBytes
        updaterSignatureCryptographicallyValid = ($null -ne $updaterVerification -and [bool]$updaterVerification.passed)
        updaterSignatureKeyId = if ($updaterVerification) { $updaterVerification.signature.keyId } else { $null }
    }
}

$report = [ordered]@{
    schemaVersion = 1
    audit = 'Meetily phase 5A-4B Windows release signing gate'
    generatedAtUtc = [DateTime]::UtcNow.ToString('o')
    passed = ($failures.Count -eq 0)
    policy = [ordered]@{
        path = [IO.Path]::GetFullPath($PolicyPath)
        sha256 = (Get-FileHash -Algorithm SHA256 -LiteralPath $PolicyPath).Hash
        policyId = $policy.policyId
        productionActiveThumbprintCount = @($policy.authenticode.signerThumbprintSets.productionActive).Count
    }
    updater = [ordered]@{
        publicKeyId = $policy.updater.publicKeyId
        configPublicKeyMatches = ($decodedPublicKey -match [regex]::Escape([string]$policy.updater.publicKeyId))
        environmentRequired = [bool]$RequireUpdaterEnvironment
    }
    artifactRoot = $root
    artifacts = $artifactResults
    failures = @($failures.ToArray())
}
if (-not [string]::IsNullOrWhiteSpace($ReportPath)) {
    $resolvedReport = [IO.Path]::GetFullPath($ReportPath)
    $parent = Split-Path -Parent $resolvedReport
    if (-not (Test-Path -LiteralPath $parent -PathType Container)) { New-Item -ItemType Directory -Path $parent -Force | Out-Null }
    $report | ConvertTo-Json -Depth 10 | Set-Content -LiteralPath $resolvedReport -Encoding UTF8
}
$report | ConvertTo-Json -Depth 10
if (-not $report.passed) { exit 1 }

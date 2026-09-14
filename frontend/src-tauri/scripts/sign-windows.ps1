[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string]$FilePath,

    [string]$PolicyPath
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
if ([string]::IsNullOrWhiteSpace($PolicyPath)) {
    $PolicyPath = Join-Path $PSScriptRoot '..\..\..\docs\i18n\phase-5a4\windows-signing-policy.v1.json'
}

$modulePath = Join-Path $PSScriptRoot '..\..\..\docs\i18n\scripts\Meetily.Signing.psm1'
Import-Module $modulePath -Force -ErrorAction Stop
$policy = Read-MeetilyWindowsSigningPolicy -Path $PolicyPath
$resolvedFile = [IO.Path]::GetFullPath($FilePath)
if (-not (Test-Path -LiteralPath $resolvedFile -PathType Leaf)) {
    throw "Signing input was not found: $resolvedFile"
}

$signingMode = if ([string]::IsNullOrWhiteSpace($env:MEETILY_WINDOWS_SIGNING_MODE)) {
    'Production'
} else {
    $env:MEETILY_WINDOWS_SIGNING_MODE
}
if ($signingMode -ceq [string]$policy.productionControls.nonProductionUnsignedMode) {
    if ($env:MEETILY_ALLOW_UNSIGNED_WINDOWS_BUILD -cne [string]$policy.productionControls.nonProductionUnsignedConfirmation) {
        throw 'The non-production unsigned mode requires the exact audited confirmation token.'
    }
    if ($env:GITHUB_REF_TYPE -ceq 'tag' -or $env:GITHUB_EVENT_NAME -ceq 'release') {
        throw 'Unsigned Windows output is forbidden for a tag or release event.'
    }
    Write-Warning "Explicit non-production unsigned mode: $([IO.Path]::GetFileName($resolvedFile)). This artifact must not be published as a release."
    exit 0
}
if ($signingMode -cne 'Production') { throw "Unknown Windows signing mode: $signingMode" }

$missingVariables = @(
    foreach ($name in @($policy.productionControls.requiredDigiCertEnvironmentVariables)) {
        $value = [Environment]::GetEnvironmentVariable([string]$name)
        if ([string]::IsNullOrWhiteSpace($value)) { [string]$name }
    }
)
if ($missingVariables.Count -gt 0) {
    throw "Production signing is not configured. Missing required environment variable(s): $($missingVariables -join ', '). Unsigned output is forbidden."
}
if (-not (Get-Command smctl -ErrorAction SilentlyContinue)) {
    throw 'Production signing requires smctl on PATH.'
}

$activeThumbprints = @(
    @($policy.authenticode.signerThumbprintSets.productionActive) |
        ForEach-Object { ConvertTo-MeetilyNormalizedThumbprint ([string]$_) }
)
if ($activeThumbprints.Count -eq 0) {
    throw 'Production signing policy has no approved active signer thumbprint. Approve the current publisher certificate in a reviewed policy change before building a release.'
}
$configuredThumbprint = ConvertTo-MeetilyNormalizedThumbprint $env:SM_CODE_SIGNING_CERT_SHA1_HASH
if ($activeThumbprints -cnotcontains $configuredThumbprint) {
    throw 'The configured DigiCert certificate thumbprint is not approved by the production signing policy.'
}

Write-Host "Signing Windows artifact: $([IO.Path]::GetFileName($resolvedFile))"
# DigiCert documents timestamping as enabled by default; it is explicit here so a
# future client-default change cannot silently produce a non-timestamped release.
$null = @(
    & smctl sign `
        --keypair-alias $env:DIGICERT_KEYPAIR_ALIAS `
        --input $resolvedFile `
        --tool=signtool `
        --timestamp=true 2>&1
)
$signExitCode = $LASTEXITCODE
if ($signExitCode -ne 0) {
    throw "DigiCert signing failed with exit code $signExitCode. Signing output is intentionally suppressed to avoid leaking credential or keypair metadata."
}

$verification = Test-MeetilySignedFile `
    -Path $resolvedFile `
    -PolicyPath $PolicyPath `
    -Role ProductionArtifact `
    -ThrowOnFailure

Write-Host "Windows signature policy passed: $([IO.Path]::GetFileName($resolvedFile))"
Write-Host "Signer subject: $($verification.evidence.signerSubject)"
Write-Host "Signer thumbprint: $($verification.evidence.signerThumbprint)"
Write-Host "Trusted timestamp certificate: $($verification.evidence.timestampCertificatePresent)"

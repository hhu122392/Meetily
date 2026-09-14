[CmdletBinding()]
param(
    [string]$PolicyPath,
    [string]$OfficialHistoricalInstaller,
    [string]$OfficialUpdaterSignature,
    [string]$UnsignedCandidateInstaller,
    [string]$TauriConfigPath,
    [string]$ReportPath,
    [switch]$RequireRealArtifacts
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
Import-Module (Join-Path $PSScriptRoot 'Meetily.Rollback.psm1') -Force -ErrorAction Stop
Import-Module (Join-Path $PSScriptRoot 'Meetily.Signing.psm1') -Force -ErrorAction Stop

$policy = Read-MeetilyWindowsSigningPolicy -Path $PolicyPath
$tests = New-Object System.Collections.Generic.List[object]
function Add-TestResult {
    param([string]$Name, [bool]$Passed, [string]$Detail)
    $tests.Add([ordered]@{ name = $Name; passed = $Passed; detail = $Detail })
}
function Copy-Evidence {
    param($Evidence)
    return ($Evidence | ConvertTo-Json -Depth 8 | ConvertFrom-Json)
}
function Test-Rejected {
    param([string]$Name, $Evidence, [string]$ExpectedFailure)
    $result = Test-MeetilySignerEvidence -Evidence $Evidence -Policy $policy -Role HistoricalRollbackTarget
    $matched = (-not $result.passed) -and (@($result.failures) -contains $ExpectedFailure)
    Add-TestResult -Name $Name -Passed $matched -Detail ($result.failures -join ' ')
}

$validEvidence = [pscustomobject][ordered]@{
    signatureStatus = 'Valid'
    signerSubject = [string]$policy.authenticode.approvedSignerSubjects[0]
    signerThumbprint = [string]$policy.authenticode.signerThumbprintSets.upstreamHistorical[0]
    signerEkuOids = @([string]$policy.authenticode.codeSigningEkuOid)
    timestampCertificatePresent = $true
    signToolDefaultAuthenticodePassed = $true
}
$validResult = Test-MeetilySignerEvidence -Evidence $validEvidence -Policy $policy -Role HistoricalRollbackTarget
Add-TestResult 'synthetic-approved-historical-evidence' $validResult.passed ($validResult.failures -join ' ')

$productionResult = Test-MeetilySignerEvidence -Evidence $validEvidence -Policy $policy -Role ProductionArtifact
Add-TestResult 'empty-production-thumbprint-set-locks-release' `
    ((-not $productionResult.passed) -and (@($productionResult.failures) -contains "Signer thumbprint set 'productionActive' is empty.")) `
    ($productionResult.failures -join ' ')

$case = Copy-Evidence $validEvidence; $case.signatureStatus = 'NotSigned'
Test-Rejected 'reject-unsigned' $case 'Authenticode status is not Valid.'
$case = Copy-Evidence $validEvidence; $case.signerSubject = 'CN=Unapproved Publisher'
Test-Rejected 'reject-wrong-subject' $case 'Signer subject is not approved.'
$case = Copy-Evidence $validEvidence; $case.signerThumbprint = '1111111111111111111111111111111111111111'
Test-Rejected 'reject-wrong-thumbprint' $case 'Signer thumbprint is not approved for this role.'
$case = Copy-Evidence $validEvidence; $case.signerEkuOids = @('1.3.6.1.5.5.7.3.1')
Test-Rejected 'reject-missing-code-signing-eku' $case 'Code Signing EKU is missing.'
$case = Copy-Evidence $validEvidence; $case.timestampCertificatePresent = $false
Test-Rejected 'reject-missing-timestamp' $case 'Timestamp certificate is missing.'
$case = Copy-Evidence $validEvidence; $case.signToolDefaultAuthenticodePassed = $false
Test-Rejected 'reject-untrusted-chain-or-timestamp' $case 'SignTool Default Authenticode, timestamp, EKU, or trust-chain verification failed.'

$unknownRoleRejected = $false
try { $null = Test-MeetilySignerEvidence -Evidence $validEvidence -Policy $policy -Role UnknownRole } catch { $unknownRoleRejected = $true }
Add-TestResult 'reject-unknown-role' $unknownRoleRejected 'Unknown role must throw.'

$realArtifactsExecuted = $false
$tempRoot = Join-Path ([IO.Path]::GetTempPath()) ('MeetilyPhase5A4B-' + [guid]::NewGuid().ToString('N'))
try {
    if (-not [string]::IsNullOrWhiteSpace($OfficialHistoricalInstaller) -and
        -not [string]::IsNullOrWhiteSpace($OfficialUpdaterSignature) -and
        -not [string]::IsNullOrWhiteSpace($UnsignedCandidateInstaller)) {
        $realArtifactsExecuted = $true
        $officialHash = (Get-FileHash -Algorithm SHA256 -LiteralPath $OfficialHistoricalInstaller).Hash
        $official = Test-MeetilySignedFile `
            -Path $OfficialHistoricalInstaller `
            -PolicyPath $PolicyPath `
            -Role HistoricalRollbackTarget `
            -ExpectedSha256 $officialHash
        Add-TestResult 'real-upstream-0.3.0-valid-chain-and-timestamp' $official.passed ($official.failures -join ' ')

        $nodeCommand = Get-Command node.exe -ErrorAction SilentlyContinue | Select-Object -First 1
        if (-not $nodeCommand) { $nodeCommand = Get-Command node -ErrorAction SilentlyContinue | Select-Object -First 1 }
        if ($nodeCommand) {
            $updaterVerifier = Join-Path $PSScriptRoot 'verify-tauri-updater-signature.mjs'
            $updaterOutput = @(
                & $nodeCommand.Source $updaterVerifier `
                    --artifact $OfficialHistoricalInstaller `
                    --signature $OfficialUpdaterSignature `
                    --tauri-config $TauriConfigPath 2>&1
            )
            $updaterExitCode = $LASTEXITCODE
            $updaterResult = if ($updaterOutput.Count -gt 0) {
                ($updaterOutput -join [Environment]::NewLine) | ConvertFrom-Json
            } else { $null }
            Add-TestResult 'real-upstream-0.3.0-updater-signature-valid' `
                ($updaterExitCode -eq 0 -and $updaterResult -and [bool]$updaterResult.passed) `
                "exit=$updaterExitCode; keyId=$(if ($updaterResult) { $updaterResult.signature.keyId } else { 'none' })"
        } else {
            Add-TestResult 'real-upstream-0.3.0-updater-signature-valid' $false 'Node.js is required.'
        }

        $candidate = Test-MeetilySignedFile `
            -Path $UnsignedCandidateInstaller `
            -PolicyPath $PolicyPath `
            -Role RecoveryInstaller
        Add-TestResult 'real-unsigned-0.4.1-recovery-rejected' (-not $candidate.passed) ($candidate.failures -join ' ')

        New-Item -ItemType Directory -Path $tempRoot | Out-Null
        $signScript = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..\..\..\frontend\src-tauri\scripts\sign-windows.ps1'))
        $powershellExe = (Get-Command powershell.exe -ErrorAction Stop).Source
        $environmentNames = @(
            'SM_HOST', 'SM_API_KEY', 'SM_CLIENT_CERT_FILE', 'SM_CLIENT_CERT_PASSWORD',
            'SM_CODE_SIGNING_CERT_SHA1_HASH', 'DIGICERT_KEYPAIR_ALIAS',
            'MEETILY_WINDOWS_SIGNING_MODE', 'MEETILY_ALLOW_UNSIGNED_WINDOWS_BUILD',
            'GITHUB_REF_TYPE', 'GITHUB_EVENT_NAME'
        )
        $savedEnvironment = @{}
        foreach ($name in $environmentNames) {
            $savedEnvironment[$name] = [Environment]::GetEnvironmentVariable($name)
            [Environment]::SetEnvironmentVariable($name, $null)
        }
        try {
            $arguments = "-NoProfile -ExecutionPolicy Bypass -File `"$signScript`" -FilePath `"$UnsignedCandidateInstaller`" -PolicyPath `"$PolicyPath`""
            $productionProcess = Start-Process `
                -FilePath $powershellExe `
                -ArgumentList $arguments `
                -RedirectStandardOutput (Join-Path $tempRoot 'production.stdout.txt') `
                -RedirectStandardError (Join-Path $tempRoot 'production.stderr.txt') `
                -WindowStyle Hidden `
                -Wait `
                -PassThru
            Add-TestResult 'sign-command-production-missing-credentials-fails' ($productionProcess.ExitCode -ne 0) "exit=$($productionProcess.ExitCode)"

            [Environment]::SetEnvironmentVariable('MEETILY_WINDOWS_SIGNING_MODE', [string]$policy.productionControls.nonProductionUnsignedMode)
            [Environment]::SetEnvironmentVariable('MEETILY_ALLOW_UNSIGNED_WINDOWS_BUILD', [string]$policy.productionControls.nonProductionUnsignedConfirmation)
            $beforeAuditHash = (Get-FileHash -Algorithm SHA256 -LiteralPath $UnsignedCandidateInstaller).Hash
            $auditProcess = Start-Process `
                -FilePath $powershellExe `
                -ArgumentList $arguments `
                -RedirectStandardOutput (Join-Path $tempRoot 'audit.stdout.txt') `
                -RedirectStandardError (Join-Path $tempRoot 'audit.stderr.txt') `
                -WindowStyle Hidden `
                -Wait `
                -PassThru
            $afterAuditHash = (Get-FileHash -Algorithm SHA256 -LiteralPath $UnsignedCandidateInstaller).Hash
            Add-TestResult 'explicit-audit-unsigned-mode-is-non-mutating' `
                ($auditProcess.ExitCode -eq 0 -and $beforeAuditHash -ceq $afterAuditHash) `
                "exit=$($auditProcess.ExitCode); unchanged=$($beforeAuditHash -ceq $afterAuditHash)"

            [Environment]::SetEnvironmentVariable('GITHUB_REF_TYPE', 'tag')
            $tagProcess = Start-Process `
                -FilePath $powershellExe `
                -ArgumentList $arguments `
                -RedirectStandardOutput (Join-Path $tempRoot 'tag.stdout.txt') `
                -RedirectStandardError (Join-Path $tempRoot 'tag.stderr.txt') `
                -WindowStyle Hidden `
                -Wait `
                -PassThru
            Add-TestResult 'tag-event-rejects-audit-unsigned-mode' ($tagProcess.ExitCode -ne 0) "exit=$($tagProcess.ExitCode)"
        } finally {
            foreach ($name in $environmentNames) {
                [Environment]::SetEnvironmentVariable($name, $savedEnvironment[$name])
            }
        }

        $tamperedPath = Join-Path $tempRoot 'tampered-official.exe'
        Copy-Item -LiteralPath $OfficialHistoricalInstaller -Destination $tamperedPath
        $stream = [IO.File]::Open($tamperedPath, [IO.FileMode]::Append, [IO.FileAccess]::Write, [IO.FileShare]::None)
        try { $stream.WriteByte(0) } finally { $stream.Dispose() }
        $tampered = Test-MeetilySignedFile -Path $tamperedPath -PolicyPath $PolicyPath -Role HistoricalRollbackTarget
        Add-TestResult 'real-tampered-signed-file-rejected' (-not $tampered.passed) ($tampered.failures -join ' ')
        if ($nodeCommand) {
            $tamperedUpdaterOutput = @(
                & $nodeCommand.Source $updaterVerifier `
                    --artifact $tamperedPath `
                    --signature $OfficialUpdaterSignature `
                    --tauri-config $TauriConfigPath 2>&1
            )
            $tamperedUpdaterExitCode = $LASTEXITCODE
            $tamperedUpdaterResult = if ($tamperedUpdaterOutput.Count -gt 0) {
                ($tamperedUpdaterOutput -join [Environment]::NewLine) | ConvertFrom-Json
            } else { $null }
            Add-TestResult 'real-tampered-updater-artifact-rejected' `
                ($tamperedUpdaterExitCode -ne 0 -and $tamperedUpdaterResult -and -not [bool]$tamperedUpdaterResult.passed) `
                "exit=$tamperedUpdaterExitCode"
        }

        $overrideRejected = $false
        try {
            $null = Get-MeetilyFileEvidence `
                -Path $OfficialHistoricalInstaller `
                -ExpectedSha256 $officialHash `
                -SecurityMode Production `
                -SigningPolicyPath $PolicyPath `
                -SigningRole HistoricalRollbackTarget `
                -AuditAllowUnsigned
        } catch { $overrideRejected = $_.Exception.Message -match 'forbidden' }
        Add-TestResult 'production-rejects-audit-unsigned-override' $overrideRejected 'Production override must fail before mutation.'
    } elseif ($RequireRealArtifacts) {
        Add-TestResult 'required-real-artifacts-present' $false 'Official installer, updater signature, and unsigned candidate paths are required.'
    }
} finally {
    if (Test-Path -LiteralPath $tempRoot -PathType Container) {
        $resolvedTemp = [IO.Path]::GetFullPath($tempRoot)
        $expectedPrefix = [IO.Path]::GetFullPath([IO.Path]::GetTempPath()).TrimEnd('\') + '\MeetilyPhase5A4B-'
        if ($resolvedTemp.StartsWith($expectedPrefix, [StringComparison]::OrdinalIgnoreCase)) {
            Remove-Item -LiteralPath $resolvedTemp -Recurse -Force
        }
    }
}

$failed = @($tests | Where-Object { -not $_.passed })
$report = [ordered]@{
    schemaVersion = 1
    testSuite = 'Meetily phase 5A-4B signing policy'
    generatedAtUtc = [DateTime]::UtcNow.ToString('o')
    powershellVersion = $PSVersionTable.PSVersion.ToString()
    policyPath = [IO.Path]::GetFullPath($PolicyPath)
    policySha256 = (Get-FileHash -Algorithm SHA256 -LiteralPath $PolicyPath).Hash
    realArtifactsExecuted = $realArtifactsExecuted
    passed = ($failed.Count -eq 0)
    total = $tests.Count
    failed = $failed.Count
    tests = @($tests.ToArray())
}
if (-not [string]::IsNullOrWhiteSpace($ReportPath)) {
    $resolvedReport = [IO.Path]::GetFullPath($ReportPath)
    $parent = Split-Path -Parent $resolvedReport
    if (-not (Test-Path -LiteralPath $parent -PathType Container)) { New-Item -ItemType Directory -Path $parent -Force | Out-Null }
    $report | ConvertTo-Json -Depth 10 | Set-Content -LiteralPath $resolvedReport -Encoding UTF8
}
$report | ConvertTo-Json -Depth 10
if (-not $report.passed) { exit 1 }

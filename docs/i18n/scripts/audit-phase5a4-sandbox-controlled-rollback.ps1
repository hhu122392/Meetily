[CmdletBinding()]
param()

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
Import-Module Microsoft.PowerShell.Utility -ErrorAction Stop

$officialInstaller = 'C:\MeetilyOfficial\meetily_0.3.0_x64-setup.exe'
$candidateInstaller = 'C:\MeetilyCandidate\meetily_0.4.1_x64-setup.exe'
$webView2Installer = 'C:\WebView2Input\MicrosoftEdgeWebView2RuntimeInstallerX64.exe'
$fixtureRoot = 'C:\MeetilyFixture'
$scriptsRoot = 'C:\MeetilyI18n\scripts'
$outputRoot = 'C:\MeetilyOutput'
$auditRoot = Join-Path $env:USERPROFILE 'AppData\Local\Temp\MeetilyPhase5A4Audit'
$ownershipManifest = Join-Path $scriptsRoot '..\phase-5a4\install-resource-ownership.v1.json'
$rollbackScript = Join-Path $scriptsRoot 'invoke-meetily-controlled-rollback.ps1'
$modulePath = Join-Path $scriptsRoot 'Meetily.Rollback.psm1'
$reportPath = Join-Path $outputRoot 'phase-5a4a-controlled-rollback-sandbox-audit.json'
$compatibleSnapshot = Join-Path $auditRoot 'compatible-v030-snapshot'
$emergencySnapshot = Join-Path $auditRoot 'emergency-v041-snapshot'
$officialHash = '900C10A4EA05D991A06AB670886DE45DE08904903298A862A94FDC66F25420D9'
$candidateHash = 'AD6D1B4B702873CBECBAD9CADD4F7166BC2C505DE70ACFCA226E1F8C46EF79EC'
$webView2Hash = '82B2D8A7013E0C0EA15D48FF4742EE3778BA16BD8B7B4A47876645B3E48D4016'
$activeProcess = $null
$progressPath = Join-Path $outputRoot 'phase-5a4a-sandbox-progress.json'

function Set-AuditProgress {
  param([Parameter(Mandatory = $true)][string]$Stage, [string]$Detail = '')
  [ordered]@{
    stage = $Stage
    detail = $Detail
    updatedAtUtc = [DateTime]::UtcNow.ToString('o')
  } | ConvertTo-Json | Set-Content -LiteralPath $progressPath -Encoding UTF8
}

function Get-MeetilyEntry {
  foreach ($root in @(
      'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\*',
      'HKLM:\Software\Microsoft\Windows\CurrentVersion\Uninstall\*',
      'HKLM:\Software\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall\*')) {
    $entry = Get-ItemProperty -Path $root -ErrorAction SilentlyContinue |
      Where-Object {
        $_.PSObject.Properties.Name -contains 'DisplayName' -and
        [string]$_.DisplayName -ceq 'meetily'
      } | Select-Object -First 1
    if ($null -ne $entry) { return $entry }
  }
  return $null
}

function Invoke-ProcessChecked {
  param(
    [Parameter(Mandatory = $true)][string]$Path,
    [string[]]$Arguments = @(),
    [int]$TimeoutSeconds = 180
  )
  $process = Start-Process -FilePath $Path -ArgumentList $Arguments -PassThru -WindowStyle Hidden
  if (-not $process.WaitForExit($TimeoutSeconds * 1000)) {
    & taskkill.exe /PID $process.Id /T /F 2>$null | Out-Null
    throw "Process timed out: $Path"
  }
  if ($process.ExitCode -ne 0) { throw "Process exited $($process.ExitCode): $Path" }
  [ordered]@{ processId = $process.Id; exitCode = $process.ExitCode }
}

function Invoke-RollbackChild {
  param([Parameter(Mandatory = $true)][string]$Name, [Parameter(Mandatory = $true)][string[]]$Arguments)
  $allArguments = @('-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', $rollbackScript) + $Arguments
  $auditReportArgumentIndex = [Array]::IndexOf($Arguments, '-AuditReportPath')
  $childReportPath = if ($auditReportArgumentIndex -ge 0 -and $auditReportArgumentIndex + 1 -lt $Arguments.Count) {
    $Arguments[$auditReportArgumentIndex + 1]
  } else { $null }
  $process = Start-Process -FilePath 'powershell.exe' -ArgumentList $allArguments -PassThru -WindowStyle Hidden
  if (-not $process.WaitForExit(300000)) {
    & taskkill.exe /PID $process.Id /T /F 2>$null | Out-Null
    throw "Rollback child timed out: $Name"
  }
  $process.Refresh()
  $exitCode = $process.ExitCode
  $childReport = if ($childReportPath -and (Test-Path -LiteralPath $childReportPath -PathType Leaf)) {
    Get-Content -LiteralPath $childReportPath -Raw | ConvertFrom-Json
  } else { $null }
  $record = [ordered]@{
    processId = $process.Id
    exitCode = $exitCode
    auditReportPath = $childReportPath
    auditReport = $childReport
  }
  if ($exitCode -ne 0) {
    throw "Rollback child failed with exit code $exitCode`: $Name; report=$($childReport | ConvertTo-Json -Depth 8 -Compress)"
  }
  return $record
}

function Reset-SandboxDirectory {
  param([Parameter(Mandatory = $true)][string]$Path)
  $resolved = [IO.Path]::GetFullPath($Path)
  $sandboxRoot = [IO.Path]::GetFullPath($env:USERPROFILE).TrimEnd('\')
  if (-not $resolved.StartsWith($sandboxRoot + '\', [StringComparison]::OrdinalIgnoreCase)) {
    throw "Unsafe Sandbox reset path: $resolved"
  }
  if (Test-Path -LiteralPath $resolved) { [IO.Directory]::Delete($resolved, $true) }
  New-Item -ItemType Directory -Path $resolved -Force | Out-Null
}

function Copy-DirectoryContents {
  param([Parameter(Mandatory = $true)][string]$Source, [Parameter(Mandatory = $true)][string]$Destination)
  New-Item -ItemType Directory -Path $Destination -Force | Out-Null
  Get-ChildItem -LiteralPath $Source -Force | ForEach-Object {
    Copy-Item -LiteralPath $_.FullName -Destination $Destination -Recurse -Force
  }
}

function Stop-AuditApp {
  if ($null -eq $script:activeProcess) { return }
  if (-not $script:activeProcess.HasExited) {
    $null = $script:activeProcess.CloseMainWindow()
    if (-not $script:activeProcess.WaitForExit(5000)) {
      & taskkill.exe /PID $script:activeProcess.Id /T /F 2>$null | Out-Null
      $script:activeProcess.WaitForExit(5000) | Out-Null
    }
  }
  $script:activeProcess = $null
  Start-Sleep -Seconds 2
}

function Start-AuditAppProbe {
  param([Parameter(Mandatory = $true)][string]$Path, [int]$Seconds = 12)
  $script:activeProcess = Start-Process -FilePath $Path -PassThru -WindowStyle Hidden
  Start-Sleep -Seconds $Seconds
  $alive = -not $script:activeProcess.HasExited
  $record = [ordered]@{
    processId = $script:activeProcess.Id
    aliveAfterSeconds = $Seconds
    alive = $alive
    exitCode = if ($script:activeProcess.HasExited) { $script:activeProcess.ExitCode } else { $null }
  }
  Stop-AuditApp
  return $record
}

$result = [ordered]@{
  schemaVersion = 1
  scope = 'Disposable Windows Sandbox controlled rollback: official 0.3.0 snapshot -> 0.4.1 audit candidate -> fail-closed preflight -> controlled rollback -> 0.3.0'
  startedAtUtc = [DateTime]::UtcNow.ToString('o')
  completedAtUtc = $null
  passed = $false
  error = $null
  inputs = [ordered]@{}
  observations = [ordered]@{}
  assertions = [ordered]@{}
}

try {
  New-Item -ItemType Directory -Path $outputRoot -Force | Out-Null
  Set-AuditProgress -Stage 'started'
  Reset-SandboxDirectory -Path $auditRoot
  foreach ($file in @($officialInstaller, $candidateInstaller, $webView2Installer, $rollbackScript, $modulePath, $ownershipManifest)) {
    if (-not (Test-Path -LiteralPath $file -PathType Leaf)) { throw "Required input missing: $file" }
  }
  $result.inputs.officialInstallerSha256 = (Get-FileHash -Algorithm SHA256 -LiteralPath $officialInstaller).Hash
  $result.inputs.candidateInstallerSha256 = (Get-FileHash -Algorithm SHA256 -LiteralPath $candidateInstaller).Hash
  $result.inputs.webView2InstallerSha256 = (Get-FileHash -Algorithm SHA256 -LiteralPath $webView2Installer).Hash
  $result.inputs.ownershipManifestSha256 = (Get-FileHash -Algorithm SHA256 -LiteralPath $ownershipManifest).Hash
  $result.inputs.rollbackScriptSha256 = (Get-FileHash -Algorithm SHA256 -LiteralPath $rollbackScript).Hash
  $result.inputs.rollbackModuleSha256 = (Get-FileHash -Algorithm SHA256 -LiteralPath $modulePath).Hash
  $result.inputs.sandboxAuditScriptSha256 = (Get-FileHash -Algorithm SHA256 -LiteralPath $PSCommandPath).Hash
  $result.assertions.frozenInputsMatch = (
    $result.inputs.officialInstallerSha256 -ceq $officialHash -and
    $result.inputs.candidateInstallerSha256 -ceq $candidateHash -and
    $result.inputs.webView2InstallerSha256 -ceq $webView2Hash)
  if (-not $result.assertions.frozenInputsMatch) { throw 'Frozen input hash mismatch.' }

  Set-AuditProgress -Stage 'installing-webview2'
  $result.observations.webView2Install = Invoke-ProcessChecked -Path $webView2Installer -Arguments @('/silent', '/install')
  Set-AuditProgress -Stage 'installing-official-v030'
  $result.observations.officialInstall = Invoke-ProcessChecked -Path $officialInstaller -Arguments @('/S')
  $officialEntry = Get-MeetilyEntry
  if ($null -eq $officialEntry -or $officialEntry.DisplayVersion -cne '0.3.0') { throw 'Official 0.3.0 registration missing.' }

  $appData = Join-Path $env:APPDATA 'com.meetily.ai'
  $recordings = Join-Path $env:USERPROFILE 'Music\meetily-recordings'
  $configuration = Join-Path $env:APPDATA 'meetily'
  Reset-SandboxDirectory -Path $appData
  Reset-SandboxDirectory -Path $recordings
  Reset-SandboxDirectory -Path $configuration
  Copy-DirectoryContents -Source (Join-Path $fixtureRoot 'app-data') -Destination $appData
  Copy-DirectoryContents -Source (Join-Path $fixtureRoot 'recordings') -Destination $recordings
  Copy-DirectoryContents -Source (Join-Path $fixtureRoot 'config\meetily') -Destination $configuration

  Set-AuditProgress -Stage 'capturing-compatible-v030-snapshot'
  $result.observations.captureCompatibleSnapshot = Invoke-RollbackChild -Name 'capture-compatible-snapshot' -Arguments @(
    '-Mode', 'CaptureSnapshot',
    '-OwnershipManifestPath', $ownershipManifest,
    '-SnapshotRoot', $compatibleSnapshot,
    '-AuditReportPath', (Join-Path $auditRoot 'capture-compatible-snapshot.report.json'))

  Set-AuditProgress -Stage 'upgrading-to-candidate-v041'
  $result.observations.candidateUpgrade = Invoke-ProcessChecked -Path $candidateInstaller -Arguments @('/S')
  $candidateEntry = Get-MeetilyEntry
  if ($null -eq $candidateEntry -or $candidateEntry.DisplayVersion -cne '0.4.1') { throw 'Candidate 0.4.1 registration missing.' }
  $candidateExe = Join-Path ([IO.Path]::GetFullPath(([string]$candidateEntry.InstallLocation).Trim('"'))) 'meetily.exe'
  Set-AuditProgress -Stage 'probing-candidate-v041'
  $result.observations.candidateLaunch = Start-AuditAppProbe -Path $candidateExe -Seconds 12
  if (-not $result.observations.candidateLaunch.alive) { throw 'Candidate did not remain alive for launch probe.' }

  $commonArguments = @(
    '-OwnershipManifestPath', $ownershipManifest,
    '-SnapshotRoot', $compatibleSnapshot,
    '-TargetInstaller', $officialInstaller,
    '-ExpectedTargetInstallerSha256', $officialHash,
    '-TargetVersion', '0.3.0',
    '-RecoveryInstaller', $candidateInstaller,
    '-ExpectedRecoveryInstallerSha256', $candidateHash,
    '-SecurityMode', 'Audit',
    '-AuditAllowUnsignedInstaller')
  Set-AuditProgress -Stage 'controlled-rollback-preflight'
  $result.observations.preflight = Invoke-RollbackChild -Name 'preflight' -Arguments (@(
    '-Mode', 'Preflight',
    '-AuditReportPath', (Join-Path $auditRoot 'preflight.report.json')) + $commonArguments)
  Set-AuditProgress -Stage 'executing-controlled-rollback'
  $result.observations.controlledRollback = Invoke-RollbackChild -Name 'rollback' -Arguments (@(
    '-Mode', 'Rollback',
    '-EmergencyBackupRoot', $emergencySnapshot,
    '-ConfirmRollback',
    '-AuditReportPath', (Join-Path $outputRoot 'controlled-rollback-tool-report.json')) + $commonArguments)

  $rollbackEntry = Get-MeetilyEntry
  if ($null -eq $rollbackEntry) { throw 'Rollback target registration missing.' }
  $rollbackRoot = [IO.Path]::GetFullPath(([string]$rollbackEntry.InstallLocation).Trim('"'))
  $rollbackExe = Join-Path $rollbackRoot 'meetily.exe'
  $localizedResiduals = @(
    Get-ChildItem -LiteralPath (Join-Path $rollbackRoot 'templates') -File -Recurse -ErrorAction SilentlyContinue |
      Where-Object { $_.FullName -match '\\templates\\(en|zh-CN)\\' }
  )
  Import-Module $modulePath -Force
  $snapshotManifest = Test-MeetilyDataSnapshot -SnapshotRoot $compatibleSnapshot -OwnershipManifest (Read-MeetilyOwnershipManifest -Path $ownershipManifest) -ExpectedVersion '0.3.0'
  $restoredDataMatches = Test-MeetilyRestoredData -SnapshotRoot $compatibleSnapshot -SnapshotManifest $snapshotManifest
  $result.observations.rollbackLaunch = Start-AuditAppProbe -Path $rollbackExe -Seconds 12
  $rollbackSignature = Get-AuthenticodeSignature -LiteralPath $rollbackExe
  $result.observations.rollbackIdentity = [ordered]@{
    displayVersion = [string]$rollbackEntry.DisplayVersion
    executableSha256 = (Get-FileHash -Algorithm SHA256 -LiteralPath $rollbackExe).Hash
    signatureStatus = $rollbackSignature.Status.ToString()
    localizedResidualCount = $localizedResiduals.Count
  }
  $result.assertions.preflightPassedWithoutMutation = (
    (Get-Content -LiteralPath (Join-Path $auditRoot 'preflight.report.json') -Raw | ConvertFrom-Json).mutated -eq $false)
  $result.assertions.controlledRollbackInstalledTarget = (
    $rollbackEntry.DisplayVersion -ceq '0.3.0' -and
    $result.observations.rollbackIdentity.executableSha256 -ceq '0C1A60560490CF78BC9C14303A1598DB0F05DC887A7E8EB7BD00B7EC04408045' -and
    $result.observations.rollbackIdentity.signatureStatus -ceq 'Valid')
  $result.assertions.crossVersionLocalizedResidualsAbsent = ($localizedResiduals.Count -eq 0)
  $result.assertions.compatibleDataRestoredExactly = ($restoredDataMatches -eq $true)
  $result.assertions.rollbackTargetLaunches = $result.observations.rollbackLaunch.alive
  $result.passed = -not ($result.assertions.Values -contains $false)
} catch {
  $result.error = $_.Exception.Message
  $result.passed = $false
  Set-AuditProgress -Stage 'failed' -Detail $result.error
} finally {
  Stop-AuditApp
  try {
    $remaining = Get-MeetilyEntry
    if ($null -ne $remaining -and $remaining.UninstallString) {
      $uninstaller = ([string]$remaining.UninstallString).Trim('"')
      if (Test-Path -LiteralPath $uninstaller -PathType Leaf) {
        $result.observations.finalSandboxUninstall = Invoke-ProcessChecked -Path $uninstaller -Arguments @('/S') -TimeoutSeconds 120
      }
    }
  } catch {
    $result.observations.finalSandboxUninstallError = $_.Exception.Message
  }
  $result.completedAtUtc = [DateTime]::UtcNow.ToString('o')
  $result | ConvertTo-Json -Depth 12 | Set-Content -LiteralPath $reportPath -Encoding UTF8
  Set-AuditProgress -Stage 'completed' -Detail "passed=$($result.passed)"
}

if (-not $result.passed) { exit 1 }

[CmdletBinding()]
param(
  [Parameter(Mandatory = $true)]
  [ValidateSet('CaptureSnapshot', 'Preflight', 'Rollback')]
  [string]$Mode,

  [string]$OwnershipManifestPath,
  [string]$SnapshotRoot,
  [string]$EmergencyBackupRoot,
  [string]$TargetInstaller,
  [string]$ExpectedTargetInstallerSha256,
  [string]$TargetVersion,
  [string]$RecoveryInstaller,
  [string]$ExpectedRecoveryInstallerSha256,
  [string]$SigningPolicyPath,
  [ValidateSet('Production', 'Audit')][string]$SecurityMode = 'Production',
  [string[]]$AdditionalProtectedDataPath = @(),
  [string]$AuditReportPath,
  [switch]$ConfirmRollback,
  [switch]$AuditAllowUnsignedInstaller
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
if ([string]::IsNullOrWhiteSpace($OwnershipManifestPath)) {
  $OwnershipManifestPath = Join-Path $PSScriptRoot '..\phase-5a4\install-resource-ownership.v1.json'
}
if ([string]::IsNullOrWhiteSpace($SigningPolicyPath)) {
  $SigningPolicyPath = Join-Path $PSScriptRoot '..\phase-5a4\windows-signing-policy.v1.json'
}
Import-Module Microsoft.PowerShell.Utility -ErrorAction Stop
Import-Module (Join-Path $PSScriptRoot 'Meetily.Rollback.psm1') -Force

function Assert-RequiredValue {
  param([Parameter(Mandatory = $true)][string]$Name, [AllowEmptyString()][string]$Value)
  if ([string]::IsNullOrWhiteSpace($Value)) { throw "Parameter -$Name is required in $Mode mode." }
}

function Write-AuditReport {
  param([Parameter(Mandatory = $true)]$Report)
  if ([string]::IsNullOrWhiteSpace($AuditReportPath)) { return }
  $resolved = [IO.Path]::GetFullPath($AuditReportPath)
  $parent = Split-Path -Parent $resolved
  if (-not (Test-Path -LiteralPath $parent)) { New-Item -ItemType Directory -Path $parent -Force | Out-Null }
  $Report | ConvertTo-Json -Depth 12 | Set-Content -LiteralPath $resolved -Encoding UTF8
}

function Assert-NoMeetilyProcessInInstallRoot {
  param([Parameter(Mandatory = $true)][string]$InstallRoot)
  $root = [IO.Path]::GetFullPath($InstallRoot).TrimEnd('\')
  $running = @(
    Get-Process -ErrorAction SilentlyContinue | ForEach-Object {
      try {
        if ($_.Path -and [IO.Path]::GetFullPath($_.Path).StartsWith(
            $root + '\', [StringComparison]::OrdinalIgnoreCase)) { $_ }
      } catch { }
    }
  )
  if ($running.Count -gt 0) {
    $details = ($running | ForEach-Object { "$($_.ProcessName)[$($_.Id)]" }) -join ', '
    throw "Meetily processes are still running inside the install directory: $details"
  }
}

function Get-CurrentInstallation {
  $entry = Get-MeetilyRegistryEntry
  if ($null -eq $entry) { throw 'Meetily is not registered as installed.' }
  foreach ($property in @('DisplayVersion', 'InstallLocation', 'UninstallString')) {
    if ([string]::IsNullOrWhiteSpace([string]$entry.$property)) {
      throw "Installed Meetily registration is missing $property."
    }
  }
  $installRoot = [IO.Path]::GetFullPath(([string]$entry.InstallLocation).Trim('"')).TrimEnd('\')
  $uninstaller = Get-MeetilyExecutableFromCommand -Command ([string]$entry.UninstallString)
  if (-not $uninstaller.StartsWith($installRoot + '\', [StringComparison]::OrdinalIgnoreCase)) {
    throw 'Registered uninstaller is outside the registered Meetily install directory.'
  }
  if (-not (Test-Path -LiteralPath $uninstaller -PathType Leaf)) {
    throw "Registered uninstaller does not exist: $uninstaller"
  }
  [pscustomobject]@{
    Entry = $entry
    Version = [string]$entry.DisplayVersion
    InstallRoot = $installRoot
    Uninstaller = $uninstaller
  }
}

function Assert-RegistryRemoved {
  $stopwatch = [Diagnostics.Stopwatch]::StartNew()
  while ($stopwatch.Elapsed.TotalSeconds -lt 30) {
    if ($null -eq (Get-MeetilyRegistryEntry)) { return }
    Start-Sleep -Milliseconds 250
  }
  throw 'Meetily uninstall registration remained after uninstall.'
}

function Remove-RegisteredMeetily {
  param([Parameter(Mandatory = $true)]$Installation, [Parameter(Mandatory = $true)]$OwnershipManifest)
  Assert-NoMeetilyProcessInInstallRoot -InstallRoot $Installation.InstallRoot
  $process = Invoke-MeetilyProcess -Path $Installation.Uninstaller -Arguments @('/S') -TimeoutSeconds 120
  Assert-RegistryRemoved
  $absent = Wait-MeetilyPathAbsent -Path $Installation.InstallRoot -TimeoutSeconds 30
  $cleanup = $null
  if (-not $absent) {
    $cleanup = Remove-MeetilyOwnedResiduals `
      -InstallRoot $Installation.InstallRoot `
      -InstalledVersion $Installation.Version `
      -OwnershipManifest $OwnershipManifest `
      -Confirm:$false
  }
  [pscustomobject]@{ Process = $process; ExactOwnershipCleanup = $cleanup }
}

$report = [ordered]@{
  schemaVersion = 1
  operation = $Mode
  startedAtUtc = [DateTime]::UtcNow.ToString('o')
  completedAtUtc = $null
  passed = $false
  mutated = $false
  recoveredAfterFailure = $false
  error = $null
  inputs = [ordered]@{}
  observations = [ordered]@{}
}

try {
  if ($AuditAllowUnsignedInstaller -and $SecurityMode -cne 'Audit') {
    throw '-AuditAllowUnsignedInstaller is forbidden unless -SecurityMode Audit is explicit.'
  }
  $ownershipPath = [IO.Path]::GetFullPath($OwnershipManifestPath)
  $ownership = Read-MeetilyOwnershipManifest -Path $ownershipPath
  $report.inputs.ownershipManifest = [ordered]@{
    path = $ownershipPath
    sha256 = (Get-FileHash -Algorithm SHA256 -LiteralPath $ownershipPath).Hash
  }

  if ($Mode -eq 'CaptureSnapshot') {
    Assert-RequiredValue -Name 'SnapshotRoot' -Value $SnapshotRoot
    $current = Get-CurrentInstallation
    Get-MeetilyOwnedRelease -Manifest $ownership -Version $current.Version | Out-Null
    $snapshot = New-MeetilyDataSnapshot `
      -Destination $SnapshotRoot `
      -InstalledVersion $current.Version `
      -OwnershipManifest $ownership `
      -AdditionalProtectedDataPath $AdditionalProtectedDataPath
    $report.observations.currentVersion = $current.Version
    $report.observations.snapshot = $snapshot
    $report.passed = $true
  } else {
    foreach ($required in @(
        @{ name = 'SnapshotRoot'; value = $SnapshotRoot },
        @{ name = 'TargetInstaller'; value = $TargetInstaller },
        @{ name = 'ExpectedTargetInstallerSha256'; value = $ExpectedTargetInstallerSha256 },
        @{ name = 'TargetVersion'; value = $TargetVersion },
        @{ name = 'RecoveryInstaller'; value = $RecoveryInstaller },
        @{ name = 'ExpectedRecoveryInstallerSha256'; value = $ExpectedRecoveryInstallerSha256 })) {
      Assert-RequiredValue -Name $required.name -Value $required.value
    }
    if ($Mode -eq 'Rollback') {
      Assert-RequiredValue -Name 'EmergencyBackupRoot' -Value $EmergencyBackupRoot
      if (-not $ConfirmRollback) { throw 'Rollback mode requires the explicit -ConfirmRollback switch.' }
    }

    $current = Get-CurrentInstallation
    if ((Compare-MeetilySemVer -Left $current.Version -Right $TargetVersion) -ne 1) {
      throw "Target version $TargetVersion is not lower than installed version $($current.Version)."
    }
    Get-MeetilyOwnedRelease -Manifest $ownership -Version $current.Version | Out-Null
    Get-MeetilyOwnedRelease -Manifest $ownership -Version $TargetVersion | Out-Null
    $targetEvidence = Get-MeetilyFileEvidence `
      -Path $TargetInstaller `
      -ExpectedSha256 $ExpectedTargetInstallerSha256 `
      -SecurityMode $SecurityMode `
      -SigningPolicyPath $SigningPolicyPath `
      -SigningRole HistoricalRollbackTarget `
      -AuditAllowUnsigned:$AuditAllowUnsignedInstaller
    $recoveryEvidence = Get-MeetilyFileEvidence `
      -Path $RecoveryInstaller `
      -ExpectedSha256 $ExpectedRecoveryInstallerSha256 `
      -SecurityMode $SecurityMode `
      -SigningPolicyPath $SigningPolicyPath `
      -SigningRole RecoveryInstaller `
      -AuditAllowUnsigned:$AuditAllowUnsignedInstaller
    $snapshotManifest = Test-MeetilyDataSnapshot `
      -SnapshotRoot $SnapshotRoot `
      -OwnershipManifest $ownership `
      -ExpectedVersion $TargetVersion `
      -AdditionalProtectedDataPath $AdditionalProtectedDataPath
    Assert-NoMeetilyProcessInInstallRoot -InstallRoot $current.InstallRoot

    $report.inputs.installedVersion = $current.Version
    $report.inputs.securityMode = $SecurityMode
    $report.inputs.targetVersion = $TargetVersion
    $report.inputs.targetInstaller = $targetEvidence
    $report.inputs.recoveryInstaller = $recoveryEvidence
    $report.inputs.compatibleSnapshot = [ordered]@{
      path = [IO.Path]::GetFullPath($SnapshotRoot)
      installedVersion = [string]$snapshotManifest.installedVersion
      manifestSha256 = (Get-FileHash -Algorithm SHA256 -LiteralPath (Join-Path $SnapshotRoot 'snapshot-manifest.json')).Hash
    }
    $report.observations.preflight = [ordered]@{
      strictDowngradeConfirmed = $true
      ownershipKnownForInstalledVersion = $true
      ownershipKnownForTargetVersion = $true
      compatibleSnapshotVerified = $true
      appProcessesClosed = $true
      signaturesRequired = -not $AuditAllowUnsignedInstaller
    }

    if ($Mode -eq 'Preflight') {
      $report.passed = $true
    } else {
      $emergency = New-MeetilyDataSnapshot `
        -Destination $EmergencyBackupRoot `
        -InstalledVersion $current.Version `
        -OwnershipManifest $ownership `
        -AdditionalProtectedDataPath $AdditionalProtectedDataPath
      $report.observations.emergencyBackup = $emergency
      $report.mutated = $true

      try {
        $report.observations.currentVersionRemoval = Remove-RegisteredMeetily -Installation $current -OwnershipManifest $ownership
        Restore-MeetilyDataSnapshot `
          -SnapshotRoot $SnapshotRoot `
          -OwnershipManifest $ownership `
          -ExpectedVersion $TargetVersion `
          -AdditionalProtectedDataPath $AdditionalProtectedDataPath
        $report.observations.targetDataSnapshotRestored = $true
        $report.observations.targetInstall = Invoke-MeetilyProcess -Path $targetEvidence.Path -Arguments @('/S') -TimeoutSeconds 180
        $targetInstallation = Get-CurrentInstallation
        if ($targetInstallation.Version -cne $TargetVersion) {
          throw "Installed rollback target version is $($targetInstallation.Version), expected $TargetVersion."
        }
        $report.observations.installedTargetVersion = $targetInstallation.Version
        $report.passed = $true
      } catch {
        $rollbackError = $_.Exception.Message
        $recoveryErrors = @()
        try {
          $partial = Get-MeetilyRegistryEntry
          if ($null -ne $partial) {
            $partialInstallation = Get-CurrentInstallation
            $report.observations.partialTargetRemoval = Remove-RegisteredMeetily -Installation $partialInstallation -OwnershipManifest $ownership
          }
        } catch { $recoveryErrors += "partial target removal: $($_.Exception.Message)" }
        try {
          Restore-MeetilyDataSnapshot `
            -SnapshotRoot $EmergencyBackupRoot `
            -OwnershipManifest $ownership `
            -ExpectedVersion $current.Version `
            -AdditionalProtectedDataPath $AdditionalProtectedDataPath
          $report.observations.emergencyDataRestored = $true
        } catch { $recoveryErrors += "emergency data restore: $($_.Exception.Message)" }
        try {
          $report.observations.recoveryInstall = Invoke-MeetilyProcess -Path $recoveryEvidence.Path -Arguments @('/S') -TimeoutSeconds 180
          $recovered = Get-CurrentInstallation
          if ($recovered.Version -cne $current.Version) {
            throw "Recovery installed version $($recovered.Version), expected $($current.Version)."
          }
          $report.recoveredAfterFailure = $true
        } catch { $recoveryErrors += "recovery install: $($_.Exception.Message)" }
        $detail = if ($recoveryErrors.Count -eq 0) { 'automatic recovery completed' } else { $recoveryErrors -join '; ' }
        throw "Controlled rollback failed: $rollbackError; $detail"
      }
    }
  }
} catch {
  $report.error = $_.Exception.Message
  $report.passed = $false
} finally {
  $report.completedAtUtc = [DateTime]::UtcNow.ToString('o')
  Write-AuditReport -Report $report
}

$report | ConvertTo-Json -Depth 12
if (-not $report.passed) { exit 1 }
exit 0

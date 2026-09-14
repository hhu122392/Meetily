[CmdletBinding()]
param()

$ErrorActionPreference = 'Stop'

$worktreeRoot = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..\..\..'))
$workspaceRoot = [System.IO.Path]::GetFullPath((Join-Path $worktreeRoot '..'))
$buildRoot = Join-Path $workspaceRoot 'target-phase5a2-rc-build'
$oldInstaller = Join-Path $buildRoot 'cargo\release\bundle\nsis\Meetily Phase 5A2 RC_0.3.9_x64-setup.exe'
$newInstaller = Join-Path $buildRoot 'artifacts\Meetily Phase 5A2 RC_0.4.0_x64-setup.exe'
$newCandidateExe = Join-Path $buildRoot 'artifacts\Meetily-Phase-5A2-RC-0.4.0.exe'
$sourceSidecarRoot = Join-Path $worktreeRoot 'frontend\src-tauri\binaries'
$sourceLlama = Join-Path $sourceSidecarRoot 'llama-helper-x86_64-pc-windows-msvc.exe'
$sourceFfmpeg = Join-Path $sourceSidecarRoot 'ffmpeg-x86_64-pc-windows-msvc.exe'
$formalExe = Join-Path $workspaceRoot 'target\release\meetily.exe'
$formalInstaller = Join-Path $workspaceRoot 'target\release\bundle\nsis\meetily_0.4.0_x64-setup.exe'
$reportPath = Join-Path $worktreeRoot 'docs\i18n\audit\phase-5a2\windows-upgrade-downgrade-audit.json'
$productName = 'Meetily Phase 5A2 RC'
$bundleId = 'com.meetily.ai.phase5a2rc'
$oldInstallerHash = '939A6DD29983C453CBC62A9F44C0D61CC3ADB7C3D956A15A60414311C6FCBADC'
$newInstallerHash = '5CFE9C41360EC135C407FF8FC8D00EE23BAE46CB2E1E8632DAC51EE91BA61FBE'
$newCandidateExeHash = 'D4C8E31D9504B25DC8C77DC30C0C5A506055E2C20BE082347B763901AA640D84'
$expectedFormalExeHash = '1873D2CCA9B06EEBE8B2D42DC8530B6888BCB91C4267BBD6E6724F6A78257823'
$expectedFormalInstallerHash = 'C6A5BD12F947AEDECAC890C33A48FDEA94E4C11C29FCA03EEF753C25AEE30434'
$appDataRoot = [System.IO.Path]::GetFullPath($env:APPDATA)
$localAppDataRoot = [System.IO.Path]::GetFullPath($env:LOCALAPPDATA)
$testData = [System.IO.Path]::GetFullPath((Join-Path $appDataRoot $bundleId))
$expectedInstallDir = [System.IO.Path]::GetFullPath((Join-Path $localAppDataRoot $productName))
$sentinel = Join-Path $testData 'phase5a2-upgrade-retention-sentinel.txt'
$launchedProcess = $null
$uninstallAttempted = $false

function Get-AuditRegistryEntry {
  Get-ItemProperty -Path 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\*' -ErrorAction SilentlyContinue |
    Where-Object { $_.DisplayName -eq $productName } |
    Select-Object -First 1
}

function Invoke-SilentExecutable {
  param([Parameter(Mandatory = $true)][string]$FilePath)
  $process = Start-Process -FilePath $FilePath -ArgumentList @('/S') -Wait -PassThru -WindowStyle Hidden
  return $process.ExitCode
}

function Assert-SafeAuditPath {
  param(
    [Parameter(Mandatory = $true)][string]$Path,
    [Parameter(Mandatory = $true)][string]$Root,
    [Parameter(Mandatory = $true)][string]$ExpectedLeaf
  )

  $resolvedPath = [System.IO.Path]::GetFullPath($Path)
  $resolvedRoot = [System.IO.Path]::GetFullPath($Root)
  if (-not $resolvedPath.StartsWith(
      $resolvedRoot + [System.IO.Path]::DirectorySeparatorChar,
      [System.StringComparison]::OrdinalIgnoreCase)) {
    throw "Unsafe audit path outside expected root: $resolvedPath"
  }
  if ((Split-Path -Leaf $resolvedPath) -cne $ExpectedLeaf) {
    throw "Unexpected audit path leaf: $resolvedPath"
  }
}

function Assert-NotReparsePoint {
  param([Parameter(Mandatory = $true)][string]$Path)
  if (Test-Path -LiteralPath $Path) {
    $item = Get-Item -LiteralPath $Path -Force
    if (($item.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0) {
      throw "Refusing recursive cleanup through a reparse point: $Path"
    }
  }
}

function Get-ArtifactRecord {
  param([Parameter(Mandatory = $true)][string]$Path)
  $item = Get-Item -LiteralPath $Path
  return [ordered]@{
    path = $item.FullName
    bytes = $item.Length
    sha256 = (Get-FileHash -LiteralPath $item.FullName -Algorithm SHA256).Hash
    fileVersion = $item.VersionInfo.FileVersion
    productVersion = $item.VersionInfo.ProductVersion
    productName = $item.VersionInfo.ProductName
    signatureStatus = (Get-AuthenticodeSignature -LiteralPath $item.FullName).Status.ToString()
  }
}

Assert-SafeAuditPath -Path $testData -Root $appDataRoot -ExpectedLeaf $bundleId
Assert-SafeAuditPath -Path $expectedInstallDir -Root $localAppDataRoot -ExpectedLeaf $productName

$result = [ordered]@{
  schemaVersion = 1
  phase = '15.9 / Stage 5A-2'
  generatedAt = (Get-Date).ToUniversalTime().ToString('o')
  scope = 'Synthetic 0.3.9 to 0.4.0 Windows NSIS upgrade, downgrade rejection, launch, payload, uninstall, and retention audit'
  productName = $productName
  bundleId = $bundleId
  releaseCandidateBuildCommit = '4edc53116b5d5e186d306dccdc5b45729777f487'
  syntheticFixtureBuildCommit = 'dd1cbac21f911df0fdb3fea5d7025132775ef51a'
  syntheticFixture = $true
  closesHistoricalDataMigrationGate = $false
  artifacts = [ordered]@{}
  observations = [ordered]@{}
  assertions = [ordered]@{}
  cleanup = [ordered]@{}
  passed = $false
}

try {
  foreach ($requiredPath in @(
      $oldInstaller, $newInstaller, $newCandidateExe, $sourceLlama, $sourceFfmpeg,
      $formalExe, $formalInstaller)) {
    if (-not (Test-Path -LiteralPath $requiredPath -PathType Leaf)) {
      throw "Required artifact is missing: $requiredPath"
    }
  }

  $result.artifacts.oldInstaller = Get-ArtifactRecord -Path $oldInstaller
  $result.artifacts.newInstaller = Get-ArtifactRecord -Path $newInstaller
  $result.artifacts.newCandidateExecutable = Get-ArtifactRecord -Path $newCandidateExe
  $result.artifacts.sourceLlamaSidecar = Get-ArtifactRecord -Path $sourceLlama
  $result.artifacts.sourceFfmpegSidecar = Get-ArtifactRecord -Path $sourceFfmpeg
  $result.assertions.artifactHashesMatchFrozenInputs = (
    $result.artifacts.oldInstaller.sha256 -eq $oldInstallerHash -and
    $result.artifacts.newInstaller.sha256 -eq $newInstallerHash -and
    $result.artifacts.newCandidateExecutable.sha256 -eq $newCandidateExeHash)
  $result.assertions.artifactVersionIdentityCorrect = (
    $result.artifacts.oldInstaller.productName -eq $productName -and
    $result.artifacts.oldInstaller.productVersion -eq '0.3.9' -and
    $result.artifacts.newInstaller.productName -eq $productName -and
    $result.artifacts.newInstaller.productVersion -eq '0.4.0')
  $result.assertions.cleanRegistryBeforeInstall = ($null -eq (Get-AuditRegistryEntry))
  $result.assertions.cleanInstallDirectoryBeforeInstall = (-not (Test-Path -LiteralPath $expectedInstallDir))
  $result.assertions.cleanAppDataBeforeInstall = (-not (Test-Path -LiteralPath $testData))
  if ($result.assertions.Values -contains $false) {
    throw 'The isolated 5A-2 product identity or its frozen artifacts failed preflight.'
  }

  $oldInstallExitCode = Invoke-SilentExecutable -FilePath $oldInstaller
  $oldEntry = Get-AuditRegistryEntry
  if ($null -eq $oldEntry) {
    throw 'The synthetic 0.3.9 uninstall registry entry was not created.'
  }
  $oldInstallDir = [System.IO.Path]::GetFullPath($oldEntry.InstallLocation.Trim('"'))
  Assert-SafeAuditPath -Path $oldInstallDir -Root $localAppDataRoot -ExpectedLeaf $productName
  $oldInstalledExe = Join-Path $oldInstallDir 'meetily.exe'
  $result.artifacts.oldInstalledExecutable = Get-ArtifactRecord -Path $oldInstalledExe
  $result.observations.oldInstall = [ordered]@{
    exitCode = $oldInstallExitCode
    displayVersion = $oldEntry.DisplayVersion
    installLocation = $oldInstallDir
  }
  $result.assertions.syntheticOldVersionInstalled = (
    $oldInstallExitCode -eq 0 -and
    $oldEntry.DisplayVersion -eq '0.3.9' -and
    $result.artifacts.oldInstalledExecutable.productVersion -eq '0.3.9')

  New-Item -ItemType Directory -Path $testData -Force | Out-Null
  [System.IO.File]::WriteAllText(
    $sentinel,
    "Meetily Phase 5A-2 synthetic upgrade retention audit`n2026-08-23",
    [System.Text.UTF8Encoding]::new($false))
  $sentinelHashBeforeUpgrade = (Get-FileHash -LiteralPath $sentinel -Algorithm SHA256).Hash

  $upgradeExitCode = Invoke-SilentExecutable -FilePath $newInstaller
  $newEntry = Get-AuditRegistryEntry
  if ($null -eq $newEntry) {
    throw 'The 0.4.0 uninstall registry entry was not present after upgrade.'
  }
  $newInstallDir = [System.IO.Path]::GetFullPath($newEntry.InstallLocation.Trim('"'))
  Assert-SafeAuditPath -Path $newInstallDir -Root $localAppDataRoot -ExpectedLeaf $productName
  $newInstalledExe = Join-Path $newInstallDir 'meetily.exe'
  $newUninstaller = [System.IO.Path]::GetFullPath($newEntry.UninstallString.Trim('"'))
  if (-not $newUninstaller.StartsWith(
      $newInstallDir + [System.IO.Path]::DirectorySeparatorChar,
      [System.StringComparison]::OrdinalIgnoreCase)) {
    throw "Unsafe upgraded uninstaller path: $newUninstaller"
  }
  $llamaMatches = @(Get-ChildItem -LiteralPath $newInstallDir -File -Filter 'llama-helper*.exe')
  $ffmpegMatches = @(Get-ChildItem -LiteralPath $newInstallDir -File -Filter 'ffmpeg*.exe')
  if ($llamaMatches.Count -ne 1 -or $ffmpegMatches.Count -ne 1) {
    throw "Unexpected upgraded sidecar counts: llama=$($llamaMatches.Count), ffmpeg=$($ffmpegMatches.Count)"
  }
  $result.artifacts.upgradedExecutable = Get-ArtifactRecord -Path $newInstalledExe
  $result.artifacts.upgradedLlamaSidecar = Get-ArtifactRecord -Path $llamaMatches[0].FullName
  $result.artifacts.upgradedFfmpegSidecar = Get-ArtifactRecord -Path $ffmpegMatches[0].FullName
  $sentinelHashAfterUpgrade = (Get-FileHash -LiteralPath $sentinel -Algorithm SHA256).Hash
  $result.observations.upgrade = [ordered]@{
    exitCode = $upgradeExitCode
    displayVersion = $newEntry.DisplayVersion
    installLocation = $newInstallDir
    sentinelHashBefore = $sentinelHashBeforeUpgrade
    sentinelHashAfter = $sentinelHashAfterUpgrade
  }
  $result.assertions.upgradeSucceeded = (
    $upgradeExitCode -eq 0 -and
    $newEntry.DisplayVersion -eq '0.4.0' -and
    $result.artifacts.upgradedExecutable.productVersion -eq '0.4.0')
  $result.assertions.upgradePreservedApplicationData = (
    $sentinelHashBeforeUpgrade -eq $sentinelHashAfterUpgrade)
  $result.assertions.upgradedLlamaHashMatchesBuildInput = (
    $result.artifacts.upgradedLlamaSidecar.sha256 -eq $result.artifacts.sourceLlamaSidecar.sha256)
  $result.assertions.upgradedFfmpegHashMatchesBuildInput = (
    $result.artifacts.upgradedFfmpegSidecar.sha256 -eq $result.artifacts.sourceFfmpegSidecar.sha256)

  $env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = '--remote-debugging-port=9358'
  $launchedProcess = Start-Process -FilePath $newInstalledExe -PassThru -WindowStyle Hidden
  Start-Sleep -Seconds 5
  $alive = -not $launchedProcess.HasExited
  $result.observations.launchAfterUpgrade = [ordered]@{
    processId = $launchedProcess.Id
    aliveAfterFiveSeconds = $alive
    workingSetBytes = if ($alive) { (Get-Process -Id $launchedProcess.Id).WorkingSet64 } else { $null }
  }
  $result.assertions.upgradedApplicationLaunches = $alive
  if ($alive) {
    Stop-Process -Id $launchedProcess.Id -Force
    $launchedProcess.WaitForExit()
  }
  $launchedProcess = $null

  $installedHashBeforeDowngrade = (Get-FileHash -LiteralPath $newInstalledExe -Algorithm SHA256).Hash
  $downgradeExitCode = Invoke-SilentExecutable -FilePath $oldInstaller
  $entryAfterDowngradeAttempt = Get-AuditRegistryEntry
  $installedHashAfterDowngrade = (Get-FileHash -LiteralPath $newInstalledExe -Algorithm SHA256).Hash
  $sentinelHashAfterDowngrade = (Get-FileHash -LiteralPath $sentinel -Algorithm SHA256).Hash
  $result.observations.downgradeAttempt = [ordered]@{
    exitCode = $downgradeExitCode
    expectedExitCode = 3
    displayVersionAfter = $entryAfterDowngradeAttempt.DisplayVersion
    installedHashBefore = $installedHashBeforeDowngrade
    installedHashAfter = $installedHashAfterDowngrade
    sentinelHashAfter = $sentinelHashAfterDowngrade
  }
  $result.assertions.silentDowngradeRejected = ($downgradeExitCode -eq 3)
  $result.assertions.rejectedDowngradeLeftVersionAndPayloadUntouched = (
    $null -ne $entryAfterDowngradeAttempt -and
    $entryAfterDowngradeAttempt.DisplayVersion -eq '0.4.0' -and
    $installedHashBeforeDowngrade -eq $installedHashAfterDowngrade)
  $result.assertions.rejectedDowngradePreservedApplicationData = (
    $sentinelHashBeforeUpgrade -eq $sentinelHashAfterDowngrade)

  $uninstallExitCode = Invoke-SilentExecutable -FilePath $newUninstaller
  $uninstallAttempted = $true
  Start-Sleep -Seconds 2
  $sentinelStillExists = Test-Path -LiteralPath $sentinel -PathType Leaf
  $sentinelHashAfterUninstall = if ($sentinelStillExists) {
    (Get-FileHash -LiteralPath $sentinel -Algorithm SHA256).Hash
  } else { $null }
  $result.observations.uninstall = [ordered]@{
    exitCode = $uninstallExitCode
    registryEntryRemoved = ($null -eq (Get-AuditRegistryEntry))
    installDirectoryRemoved = (-not (Test-Path -LiteralPath $newInstallDir))
    sentinelPreserved = $sentinelStillExists
    sentinelHashAfter = $sentinelHashAfterUninstall
  }
  $result.assertions.upgradedVersionUninstallSucceeded = ($uninstallExitCode -eq 0)
  $result.assertions.uninstallRemovedRegistration = $result.observations.uninstall.registryEntryRemoved
  $result.assertions.uninstallRemovedProgramFiles = $result.observations.uninstall.installDirectoryRemoved
  $result.assertions.defaultUninstallPreservedApplicationData = (
    $sentinelStillExists -and $sentinelHashAfterUninstall -eq $sentinelHashBeforeUpgrade)

  $formalExeHashAfter = (Get-FileHash -LiteralPath $formalExe -Algorithm SHA256).Hash
  $formalInstallerHashAfter = (Get-FileHash -LiteralPath $formalInstaller -Algorithm SHA256).Hash
  $result.artifacts.formalFrozenExecutable = [ordered]@{
    path = $formalExe
    sha256 = $formalExeHashAfter
  }
  $result.artifacts.formalFrozenInstaller = [ordered]@{
    path = $formalInstaller
    sha256 = $formalInstallerHashAfter
  }
  $result.assertions.formalExecutableUnchanged = ($formalExeHashAfter -eq $expectedFormalExeHash)
  $result.assertions.formalInstallerUnchanged = ($formalInstallerHashAfter -eq $expectedFormalInstallerHash)
  $result.assertions.unsignedStatusExplicitlyRecorded = (
    $result.artifacts.oldInstaller.signatureStatus -eq 'NotSigned' -and
    $result.artifacts.newInstaller.signatureStatus -eq 'NotSigned' -and
    $result.artifacts.newCandidateExecutable.signatureStatus -eq 'NotSigned' -and
    $result.artifacts.upgradedExecutable.signatureStatus -eq 'NotSigned')

  $result.passed = -not ($result.assertions.Values -contains $false)
}
finally {
  if ($null -ne $launchedProcess -and -not $launchedProcess.HasExited) {
    Stop-Process -Id $launchedProcess.Id -Force -ErrorAction SilentlyContinue
  }

  $remainingEntry = Get-AuditRegistryEntry
  if ($null -ne $remainingEntry -and -not $uninstallAttempted) {
    $remainingUninstaller = [System.IO.Path]::GetFullPath($remainingEntry.UninstallString.Trim('"'))
    if ($remainingUninstaller.StartsWith(
        $expectedInstallDir + [System.IO.Path]::DirectorySeparatorChar,
        [System.StringComparison]::OrdinalIgnoreCase) -and
        (Test-Path -LiteralPath $remainingUninstaller -PathType Leaf)) {
      $null = Invoke-SilentExecutable -FilePath $remainingUninstaller
    }
  }

  if (Test-Path -LiteralPath $testData) {
    Assert-SafeAuditPath -Path $testData -Root $appDataRoot -ExpectedLeaf $bundleId
    Assert-NotReparsePoint -Path $testData
    [System.IO.Directory]::Delete($testData, $true)
  }

  $result.cleanup.registryEntryAbsent = ($null -eq (Get-AuditRegistryEntry))
  $result.cleanup.installDirectoryAbsent = (-not (Test-Path -LiteralPath $expectedInstallDir))
  $result.cleanup.isolatedAppDataAbsent = (-not (Test-Path -LiteralPath $testData))
  $result.cleanup.userRecordingAndSharedTemplateLocationsTouched = $false

  $reportDirectory = Split-Path -Parent $reportPath
  New-Item -ItemType Directory -Path $reportDirectory -Force | Out-Null
  [System.IO.File]::WriteAllText(
    $reportPath,
    ($result | ConvertTo-Json -Depth 8),
    [System.Text.UTF8Encoding]::new($false))
}

$result | ConvertTo-Json -Depth 8
if (-not $result.passed) { exit 1 }

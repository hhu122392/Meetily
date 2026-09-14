[CmdletBinding()]
param()

$ErrorActionPreference = 'Stop'

$worktreeRoot = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..\..\..'))
$workspaceRoot = [System.IO.Path]::GetFullPath((Join-Path $worktreeRoot '..'))
$candidateRoot = Join-Path $workspaceRoot 'target-phase5a2-rc-build\cargo\release'
$installer = Join-Path $candidateRoot 'bundle\nsis\Meetily Phase 5A2 RC_0.4.0_x64-setup.exe'
$candidateExe = Join-Path $candidateRoot 'meetily.exe'
$sourceSidecarRoot = Join-Path $worktreeRoot 'frontend\src-tauri\binaries'
$sourceLlama = Join-Path $sourceSidecarRoot 'llama-helper-x86_64-pc-windows-msvc.exe'
$sourceFfmpeg = Join-Path $sourceSidecarRoot 'ffmpeg-x86_64-pc-windows-msvc.exe'
$formalExe = Join-Path $workspaceRoot 'target\release\meetily.exe'
$formalInstaller = Join-Path $workspaceRoot 'target\release\bundle\nsis\meetily_0.4.0_x64-setup.exe'
$reportPath = Join-Path $worktreeRoot 'docs\i18n\audit\phase-5a2\windows-install-audit.json'
$productName = 'Meetily Phase 5A2 RC'
$bundleId = 'com.meetily.ai.phase5a2rc'
$expectedFormalExeHash = '1873D2CCA9B06EEBE8B2D42DC8530B6888BCB91C4267BBD6E6724F6A78257823'
$expectedFormalInstallerHash = 'C6A5BD12F947AEDECAC890C33A48FDEA94E4C11C29FCA03EEF753C25AEE30434'
$appDataRoot = [System.IO.Path]::GetFullPath($env:APPDATA)
$localAppDataRoot = [System.IO.Path]::GetFullPath($env:LOCALAPPDATA)
$testData = [System.IO.Path]::GetFullPath((Join-Path $appDataRoot $bundleId))
$expectedInstallDir = [System.IO.Path]::GetFullPath((Join-Path $localAppDataRoot $productName))
$sentinel = Join-Path $testData 'phase5a2-data-retention-sentinel.txt'
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
    signatureStatus = (Get-AuthenticodeSignature -LiteralPath $item.FullName).Status.ToString()
  }
}

Assert-SafeAuditPath -Path $testData -Root $appDataRoot -ExpectedLeaf $bundleId
Assert-SafeAuditPath -Path $expectedInstallDir -Root $localAppDataRoot -ExpectedLeaf $productName

$result = [ordered]@{
  schemaVersion = 1
  phase = '15.9 / Stage 5A-2'
  generatedAt = (Get-Date).ToUniversalTime().ToString('o')
  scope = 'Clean-commit isolated Windows NSIS install, launch, repair, payload, uninstall, and retention audit'
  productName = $productName
  bundleId = $bundleId
  buildCommit = '4edc53116b5d5e186d306dccdc5b45729777f487'
  artifacts = [ordered]@{}
  observations = [ordered]@{}
  assertions = [ordered]@{}
  cleanup = [ordered]@{}
  passed = $false
}

try {
  foreach ($requiredPath in @(
      $installer, $candidateExe, $sourceLlama, $sourceFfmpeg, $formalExe, $formalInstaller)) {
    if (-not (Test-Path -LiteralPath $requiredPath -PathType Leaf)) {
      throw "Required artifact is missing: $requiredPath"
    }
  }

  $result.assertions.cleanRegistryBeforeInstall = ($null -eq (Get-AuditRegistryEntry))
  $result.assertions.cleanInstallDirectoryBeforeInstall = (-not (Test-Path -LiteralPath $expectedInstallDir))
  $result.assertions.cleanAppDataBeforeInstall = (-not (Test-Path -LiteralPath $testData))
  if ($result.assertions.Values -contains $false) {
    throw 'The isolated 5A-2 product identity was not clean before installation.'
  }

  $result.artifacts.installer = Get-ArtifactRecord -Path $installer
  $result.artifacts.candidateExecutable = Get-ArtifactRecord -Path $candidateExe
  $result.artifacts.sourceLlamaSidecar = Get-ArtifactRecord -Path $sourceLlama
  $result.artifacts.sourceFfmpegSidecar = Get-ArtifactRecord -Path $sourceFfmpeg

  $installExitCode = Invoke-SilentExecutable -FilePath $installer
  $entry = Get-AuditRegistryEntry
  if ($null -eq $entry) {
    throw 'The expected isolated uninstall registry entry was not created.'
  }

  $installDir = [System.IO.Path]::GetFullPath($entry.InstallLocation.Trim('"'))
  Assert-SafeAuditPath -Path $installDir -Root $localAppDataRoot -ExpectedLeaf $productName
  $installedExe = Join-Path $installDir 'meetily.exe'
  $uninstaller = [System.IO.Path]::GetFullPath($entry.UninstallString.Trim('"'))
  if (-not $uninstaller.StartsWith(
      $installDir + [System.IO.Path]::DirectorySeparatorChar,
      [System.StringComparison]::OrdinalIgnoreCase)) {
    throw "Unsafe uninstaller path: $uninstaller"
  }

  $llamaMatches = @(Get-ChildItem -LiteralPath $installDir -File -Filter 'llama-helper*.exe')
  $ffmpegMatches = @(Get-ChildItem -LiteralPath $installDir -File -Filter 'ffmpeg*.exe')
  if ($llamaMatches.Count -ne 1 -or $ffmpegMatches.Count -ne 1) {
    throw "Unexpected installed sidecar counts: llama=$($llamaMatches.Count), ffmpeg=$($ffmpegMatches.Count)"
  }
  $installedLlama = $llamaMatches[0].FullName
  $installedFfmpeg = $ffmpegMatches[0].FullName

  $result.observations.install = [ordered]@{
    exitCode = $installExitCode
    displayName = $entry.DisplayName
    displayVersion = $entry.DisplayVersion
    installLocation = $installDir
    uninstallPath = $uninstaller
    installedFiles = @(Get-ChildItem -LiteralPath $installDir -File | Select-Object -ExpandProperty Name)
  }
  $result.artifacts.installedExecutable = Get-ArtifactRecord -Path $installedExe
  $result.artifacts.installedLlamaSidecar = Get-ArtifactRecord -Path $installedLlama
  $result.artifacts.installedFfmpegSidecar = Get-ArtifactRecord -Path $installedFfmpeg
  $result.assertions.firstInstallSucceeded = ($installExitCode -eq 0)
  $result.assertions.registryMetadataCorrect = (
    $entry.DisplayName -eq $productName -and
    $entry.DisplayVersion -eq '0.4.0' -and
    $installDir -eq $expectedInstallDir)
  $result.assertions.installedExecutableExists = (Test-Path -LiteralPath $installedExe -PathType Leaf)
  $result.assertions.installedPayloadSizeMatchesCandidate = (
    $result.artifacts.installedExecutable.bytes -eq $result.artifacts.candidateExecutable.bytes)
  $result.assertions.llamaSidecarHashMatchesBuildInput = (
    $result.artifacts.installedLlamaSidecar.sha256 -eq $result.artifacts.sourceLlamaSidecar.sha256)
  $result.assertions.ffmpegSidecarHashMatchesBuildInput = (
    $result.artifacts.installedFfmpegSidecar.sha256 -eq $result.artifacts.sourceFfmpegSidecar.sha256)

  $env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = '--remote-debugging-port=9357'
  $launchedProcess = Start-Process -FilePath $installedExe -PassThru -WindowStyle Hidden
  Start-Sleep -Seconds 5
  $alive = -not $launchedProcess.HasExited
  $result.observations.launch = [ordered]@{
    processId = $launchedProcess.Id
    aliveAfterFiveSeconds = $alive
    workingSetBytes = if ($alive) { (Get-Process -Id $launchedProcess.Id).WorkingSet64 } else { $null }
  }
  $result.assertions.installedApplicationLaunches = $alive
  if ($alive) {
    Stop-Process -Id $launchedProcess.Id -Force
    $launchedProcess.WaitForExit()
  }
  $launchedProcess = $null

  New-Item -ItemType Directory -Path $testData -Force | Out-Null
  [System.IO.File]::WriteAllText(
    $sentinel,
    "Meetily Phase 5A-2 isolated uninstall retention audit`n2026-08-23",
    [System.Text.UTF8Encoding]::new($false))
  $sentinelHashBeforeRepair = (Get-FileHash -LiteralPath $sentinel -Algorithm SHA256).Hash

  $repairExitCode = Invoke-SilentExecutable -FilePath $installer
  $sentinelHashAfterRepair = (Get-FileHash -LiteralPath $sentinel -Algorithm SHA256).Hash
  $result.observations.repair = [ordered]@{
    exitCode = $repairExitCode
    sentinelHashBefore = $sentinelHashBeforeRepair
    sentinelHashAfter = $sentinelHashAfterRepair
  }
  $result.assertions.sameVersionRepairSucceeded = ($repairExitCode -eq 0)
  $result.assertions.repairPreservedApplicationData = (
    $sentinelHashBeforeRepair -eq $sentinelHashAfterRepair)

  $uninstallExitCode = Invoke-SilentExecutable -FilePath $uninstaller
  $uninstallAttempted = $true
  Start-Sleep -Seconds 2
  $sentinelStillExists = Test-Path -LiteralPath $sentinel -PathType Leaf
  $sentinelHashAfterUninstall = if ($sentinelStillExists) {
    (Get-FileHash -LiteralPath $sentinel -Algorithm SHA256).Hash
  } else { $null }
  $result.observations.uninstall = [ordered]@{
    exitCode = $uninstallExitCode
    registryEntryRemoved = ($null -eq (Get-AuditRegistryEntry))
    installDirectoryRemoved = (-not (Test-Path -LiteralPath $installDir))
    sentinelPreserved = $sentinelStillExists
    sentinelHashAfter = $sentinelHashAfterUninstall
  }
  $result.assertions.defaultUninstallSucceeded = ($uninstallExitCode -eq 0)
  $result.assertions.uninstallRemovedRegistration = $result.observations.uninstall.registryEntryRemoved
  $result.assertions.uninstallRemovedProgramFiles = $result.observations.uninstall.installDirectoryRemoved
  $result.assertions.defaultUninstallPreservedApplicationData = (
    $sentinelStillExists -and $sentinelHashAfterUninstall -eq $sentinelHashBeforeRepair)

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
    $result.artifacts.installer.signatureStatus -eq 'NotSigned' -and
    $result.artifacts.candidateExecutable.signatureStatus -eq 'NotSigned' -and
    $result.artifacts.installedExecutable.signatureStatus -eq 'NotSigned')

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

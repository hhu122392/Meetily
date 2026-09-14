[CmdletBinding()]
param()

$ErrorActionPreference = 'Stop'

$repoRoot = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..\..\..'))
$deliveryRoot = Join-Path $repoRoot 'target-local-usable\bundle\current-vulkan-nsis'
$installer = Join-Path $deliveryRoot 'Meetily Phase 5 Audit_0.4.0_x64-setup.exe'
$candidateExe = Join-Path $deliveryRoot 'meetily.phase5-vulkan-nsis.exe'
$previousInstaller = Join-Path $repoRoot 'target-phase5-release\release\bundle\nsis\Meetily Phase 5 Audit_0.4.0_x64-setup.exe'
$approvedDirectExe = Join-Path $repoRoot 'target-local-usable\release\meetily.exe'
$formalExe = Join-Path $repoRoot 'target\release\meetily.exe'
$formalInstaller = Join-Path $repoRoot 'target\release\bundle\nsis\meetily_0.4.0_x64-setup.exe'
$reportPath = Join-Path $repoRoot 'target\release\docs\phase-0-custom-summary-templates\audit\current-vulkan-nsis\current-vulkan-nsis-install-final.json'

$productName = 'Meetily Phase 5 Audit'
$bundleId = 'com.meetily.ai.phase5audit'
$expectedInstallerHash = '9A2130F792197AE48C83DC06A8DE83A93C95224F4A1A562C72949C3AAE3A3A15'
$expectedCandidateHash = '8E981BE268DE48EA786009C8D23CD2DDB60CE9313C5B87633A88E4E0C52C1D96'
$expectedPreviousInstallerHash = '71777BAA1EC9F0DBAD2643638B884DC3DF6706AFFDF61EBF9CECFBED133A4101'
$expectedApprovedDirectHash = '8C083C42BDA8DCAF376763640EDA1D99063FA98B77BA17DB26A50EB288E07B8C'
$expectedFormalExeHash = '1873D2CCA9B06EEBE8B2D42DC8530B6888BCB91C4267BBD6E6724F6A78257823'
$expectedFormalInstallerHash = 'C6A5BD12F947AEDECAC890C33A48FDEA94E4C11C29FCA03EEF753C25AEE30434'

$roamingRoot = [System.IO.Path]::GetFullPath($env:APPDATA)
$localAppDataRoot = [System.IO.Path]::GetFullPath($env:LOCALAPPDATA)
$testData = [System.IO.Path]::GetFullPath((Join-Path $roamingRoot $bundleId))
$preservedAppData = [System.IO.Path]::GetFullPath((Join-Path $roamingRoot "$bundleId.pre-current-vulkan-nsis-audit"))
$expectedInstallDir = [System.IO.Path]::GetFullPath((Join-Path $localAppDataRoot $productName))
$sentinel = Join-Path $testData 'current-vulkan-nsis-retention-sentinel.txt'

$launchedProcess = $null
$uninstallAttempted = $false
$installedExe = $null
$installDir = $null
$preexistingManifest = @()

function Assert-SafeChildPath {
  param(
    [Parameter(Mandatory = $true)][string]$Path,
    [Parameter(Mandatory = $true)][string]$Root,
    [Parameter(Mandatory = $true)][string]$ExpectedLeaf
  )

  $resolvedPath = [System.IO.Path]::GetFullPath($Path)
  $resolvedRoot = [System.IO.Path]::GetFullPath($Root)
  if (-not $resolvedPath.StartsWith($resolvedRoot + [System.IO.Path]::DirectorySeparatorChar)) {
    throw "Unsafe path outside expected root: $resolvedPath"
  }
  if ((Split-Path -Leaf $resolvedPath) -ne $ExpectedLeaf) {
    throw "Unexpected path leaf: $resolvedPath"
  }
}

function Get-AuditRegistryEntry {
  Get-ItemProperty -Path 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\*' -ErrorAction SilentlyContinue |
    Where-Object { $_.DisplayName -eq $productName } |
    Select-Object -First 1
}

function Invoke-SilentExecutable {
  param(
    [Parameter(Mandatory = $true)][string]$FilePath,
    [string[]]$Arguments = @('/S')
  )

  $process = Start-Process -FilePath $FilePath -ArgumentList $Arguments -Wait -PassThru -WindowStyle Hidden
  return $process.ExitCode
}

if (-not ('MeetilyAudit.BinaryComparer' -as [type])) {
  Add-Type -TypeDefinition @'
using System;
using System.Collections.Generic;
using System.IO;

namespace MeetilyAudit
{
    public sealed class BinaryDiffResult
    {
        public long LeftLength { get; set; }
        public long RightLength { get; set; }
        public long DifferentByteCount { get; set; }
        public long[] FirstOffsets { get; set; }
        public byte[] FirstLeftBytes { get; set; }
        public byte[] FirstRightBytes { get; set; }
    }

    public static class BinaryComparer
    {
        public static BinaryDiffResult Compare(string leftPath, string rightPath, int detailLimit)
        {
            var offsets = new List<long>();
            var leftBytes = new List<byte>();
            var rightBytes = new List<byte>();
            long different = 0;
            long absoluteOffset = 0;
            var leftBuffer = new byte[1024 * 1024];
            var rightBuffer = new byte[1024 * 1024];

            using (var left = File.OpenRead(leftPath))
            using (var right = File.OpenRead(rightPath))
            {
                while (true)
                {
                    int leftRead = left.Read(leftBuffer, 0, leftBuffer.Length);
                    int rightRead = right.Read(rightBuffer, 0, rightBuffer.Length);
                    int common = Math.Min(leftRead, rightRead);
                    for (int index = 0; index < common; index++)
                    {
                        if (leftBuffer[index] != rightBuffer[index])
                        {
                            different++;
                            if (offsets.Count < detailLimit)
                            {
                                offsets.Add(absoluteOffset + index);
                                leftBytes.Add(leftBuffer[index]);
                                rightBytes.Add(rightBuffer[index]);
                            }
                        }
                    }

                    if (leftRead != rightRead)
                    {
                        different += Math.Abs((long)leftRead - rightRead);
                    }
                    absoluteOffset += common;
                    if (leftRead == 0 && rightRead == 0)
                    {
                        break;
                    }
                }

                return new BinaryDiffResult
                {
                    LeftLength = left.Length,
                    RightLength = right.Length,
                    DifferentByteCount = different,
                    FirstOffsets = offsets.ToArray(),
                    FirstLeftBytes = leftBytes.ToArray(),
                    FirstRightBytes = rightBytes.ToArray()
                };
            }
        }
    }
}
'@
}

function Get-BinaryDifference {
  param(
    [Parameter(Mandatory = $true)][string]$LeftPath,
    [Parameter(Mandatory = $true)][string]$RightPath
  )

  $difference = [MeetilyAudit.BinaryComparer]::Compare($LeftPath, $RightPath, 32)
  return [ordered]@{
    leftLength = $difference.LeftLength
    rightLength = $difference.RightLength
    differentByteCount = $difference.DifferentByteCount
    firstOffsets = @($difference.FirstOffsets)
    firstLeftByteValues = @($difference.FirstLeftBytes)
    firstRightByteValues = @($difference.FirstRightBytes)
    firstLeftAscii = [System.Text.Encoding]::ASCII.GetString($difference.FirstLeftBytes)
    firstRightAscii = [System.Text.Encoding]::ASCII.GetString($difference.FirstRightBytes)
  }
}

function Get-FileManifest {
  param([Parameter(Mandatory = $true)][string]$Root)

  if (-not (Test-Path -LiteralPath $Root -PathType Container)) {
    return @()
  }

  return @(
    Get-ChildItem -LiteralPath $Root -File -Recurse |
      Sort-Object FullName |
      ForEach-Object {
        [ordered]@{
          relativePath = $_.FullName.Substring($Root.Length + 1)
          sizeBytes = $_.Length
          sha256 = (Get-FileHash -LiteralPath $_.FullName -Algorithm SHA256).Hash
        }
      }
  )
}

function Test-ManifestsEqual {
  param(
    [Parameter(Mandatory = $true)][array]$Left,
    [Parameter(Mandatory = $true)][array]$Right
  )

  if ($Left.Count -ne $Right.Count) {
    return $false
  }

  for ($index = 0; $index -lt $Left.Count; $index++) {
    if ($Left[$index].relativePath -ne $Right[$index].relativePath -or
        $Left[$index].sizeBytes -ne $Right[$index].sizeBytes -or
        $Left[$index].sha256 -ne $Right[$index].sha256) {
      return $false
    }
  }

  return $true
}

function Get-ProcessDescendants {
  param([Parameter(Mandatory = $true)][int]$ParentId)

  $all = @(Get-CimInstance Win32_Process)
  $descendants = @()
  $frontier = @($ParentId)
  while ($frontier.Count -gt 0) {
    $next = @()
    foreach ($id in $frontier) {
      $children = @($all | Where-Object { $_.ParentProcessId -eq $id })
      foreach ($child in $children) {
        $descendants += $child
        $next += [int]$child.ProcessId
      }
    }
    $frontier = $next
  }
  return $descendants
}

function Stop-AuditApplication {
  param([Parameter(Mandatory = $true)][System.Diagnostics.Process]$Process)

  $graceful = $false
  if (-not $Process.HasExited) {
    try {
      $graceful = $Process.CloseMainWindow()
      if ($graceful) {
        $null = $Process.WaitForExit(5000)
      }
    } catch {
      $graceful = $false
    }
  }

  if (-not $Process.HasExited) {
    Stop-Process -Id $Process.Id -Force -ErrorAction SilentlyContinue
    $Process.WaitForExit()
  }

  Start-Sleep -Seconds 2
  return $graceful
}

Assert-SafeChildPath -Path $testData -Root $roamingRoot -ExpectedLeaf $bundleId
Assert-SafeChildPath -Path $preservedAppData -Root $roamingRoot -ExpectedLeaf "$bundleId.pre-current-vulkan-nsis-audit"
Assert-SafeChildPath -Path $expectedInstallDir -Root $localAppDataRoot -ExpectedLeaf $productName

$result = [ordered]@{
  schemaVersion = 1
  auditId = 'current-vulkan-nsis-install-20260824'
  generatedAt = (Get-Date).ToString('o')
  timezone = 'Asia/Taipei'
  scope = 'Latest-source Vulkan isolated NSIS install, launch, repair, rollback, restore, uninstall, and prior-state restoration'
  productName = $productName
  bundleId = $bundleId
  artifacts = [ordered]@{}
  preconditions = [ordered]@{}
  install = [ordered]@{}
  launch = [ordered]@{}
  repair = [ordered]@{}
  rollback = [ordered]@{}
  restoreCurrent = [ordered]@{}
  uninstall = [ordered]@{}
  cleanup = [ordered]@{}
  assertions = [ordered]@{}
  passed = $false
}

try {
  foreach ($requiredPath in @($installer, $candidateExe, $previousInstaller, $approvedDirectExe, $formalExe, $formalInstaller)) {
    if (-not (Test-Path -LiteralPath $requiredPath -PathType Leaf)) {
      throw "Required artifact is missing: $requiredPath"
    }
  }

  $preexistingManifest = Get-FileManifest -Root $preservedAppData
  $result.preconditions.preservedPriorAuditDataExists = (Test-Path -LiteralPath $preservedAppData -PathType Container)
  $result.preconditions.preservedPriorAuditDataFileCount = $preexistingManifest.Count
  $result.preconditions.cleanRegistry = ($null -eq (Get-AuditRegistryEntry))
  $result.preconditions.cleanInstallDirectory = (-not (Test-Path -LiteralPath $expectedInstallDir))
  $result.preconditions.cleanActiveAuditAppData = (-not (Test-Path -LiteralPath $testData))

  $result.artifacts.installer = [ordered]@{
    path = $installer
    sizeBytes = (Get-Item -LiteralPath $installer).Length
    sha256 = (Get-FileHash -LiteralPath $installer -Algorithm SHA256).Hash
    signatureStatus = (Get-AuthenticodeSignature -LiteralPath $installer).Status.ToString()
  }
  $result.artifacts.packagedCandidateExecutable = [ordered]@{
    path = $candidateExe
    sizeBytes = (Get-Item -LiteralPath $candidateExe).Length
    sha256 = (Get-FileHash -LiteralPath $candidateExe -Algorithm SHA256).Hash
    signatureStatus = (Get-AuthenticodeSignature -LiteralPath $candidateExe).Status.ToString()
    tauriBundleType = 'nsis'
    enabledFeatures = @('default', 'platform-default', 'vulkan')
  }
  $result.artifacts.previousInstaller = [ordered]@{
    path = $previousInstaller
    sizeBytes = (Get-Item -LiteralPath $previousInstaller).Length
    sha256 = (Get-FileHash -LiteralPath $previousInstaller -Algorithm SHA256).Hash
    signatureStatus = (Get-AuthenticodeSignature -LiteralPath $previousInstaller).Status.ToString()
  }
  $result.artifacts.approvedDirectExecutable = [ordered]@{
    path = $approvedDirectExe
    sizeBytes = (Get-Item -LiteralPath $approvedDirectExe).Length
    sha256 = (Get-FileHash -LiteralPath $approvedDirectExe -Algorithm SHA256).Hash
    signatureStatus = (Get-AuthenticodeSignature -LiteralPath $approvedDirectExe).Status.ToString()
  }

  $result.assertions.preconditionsClean = (
    $result.preconditions.preservedPriorAuditDataExists -and
    $result.preconditions.preservedPriorAuditDataFileCount -gt 0 -and
    $result.preconditions.cleanRegistry -and
    $result.preconditions.cleanInstallDirectory -and
    $result.preconditions.cleanActiveAuditAppData
  )
  $result.assertions.expectedArtifactHashesMatch = (
    $result.artifacts.installer.sha256 -eq $expectedInstallerHash -and
    $result.artifacts.packagedCandidateExecutable.sha256 -eq $expectedCandidateHash -and
    $result.artifacts.previousInstaller.sha256 -eq $expectedPreviousInstallerHash -and
    $result.artifacts.approvedDirectExecutable.sha256 -eq $expectedApprovedDirectHash
  )

  $installExitCode = Invoke-SilentExecutable -FilePath $installer
  $entry = Get-AuditRegistryEntry
  if ($null -eq $entry) {
    throw 'The expected isolated uninstall registry entry was not created.'
  }

  $installDir = [System.IO.Path]::GetFullPath($entry.InstallLocation.Trim('"'))
  Assert-SafeChildPath -Path $installDir -Root $localAppDataRoot -ExpectedLeaf $productName
  $installedExe = Join-Path $installDir 'meetily.exe'
  $uninstaller = [System.IO.Path]::GetFullPath($entry.UninstallString.Trim('"'))
  $installedTemplates = @(Get-ChildItem -LiteralPath (Join-Path $installDir 'templates') -File -Recurse -Filter '*.json' -ErrorAction SilentlyContinue)
  $installedRootFiles = @(Get-ChildItem -LiteralPath $installDir -File -ErrorAction SilentlyContinue)
  $candidateToInstalledDifference = Get-BinaryDifference -LeftPath $candidateExe -RightPath $installedExe

  $result.install = [ordered]@{
    exitCode = $installExitCode
    displayName = $entry.DisplayName
    displayVersion = $entry.DisplayVersion
    installLocation = $installDir
    uninstallPath = $uninstaller
    installedExecutable = [ordered]@{
      path = $installedExe
      sizeBytes = (Get-Item -LiteralPath $installedExe).Length
      sha256 = (Get-FileHash -LiteralPath $installedExe -Algorithm SHA256).Hash
      signatureStatus = (Get-AuthenticodeSignature -LiteralPath $installedExe).Status.ToString()
    }
    rootFileCount = $installedRootFiles.Count
    templateFileCount = $installedTemplates.Count
    englishTemplateCount = @($installedTemplates | Where-Object { $_.FullName -match '\\templates\\en\\' }).Count
    simplifiedChineseTemplateCount = @($installedTemplates | Where-Object { $_.FullName -match '\\templates\\zh-CN\\' }).Count
    ffmpegPresent = (Test-Path -LiteralPath (Join-Path $installDir 'ffmpeg.exe') -PathType Leaf)
    llamaHelperPresent = (Test-Path -LiteralPath (Join-Path $installDir 'llama-helper.exe') -PathType Leaf)
    directMlPresent = (Test-Path -LiteralPath (Join-Path $installDir 'DirectML.dll') -PathType Leaf)
    candidateToInstalledBinaryDifference = $candidateToInstalledDifference
  }

  $result.assertions.installSucceeded = ($installExitCode -eq 0)
  $result.assertions.registryMetadataCorrect = (
    $entry.DisplayName -eq $productName -and
    $entry.DisplayVersion -eq '0.4.0' -and
    $installDir -eq $expectedInstallDir
  )
  $result.assertions.installedPayloadMatchesExpectedTauriNsisPatch = (
    $candidateToInstalledDifference.leftLength -eq $candidateToInstalledDifference.rightLength -and
    $candidateToInstalledDifference.differentByteCount -eq 3 -and
    $candidateToInstalledDifference.firstOffsets.Count -eq 3 -and
    $candidateToInstalledDifference.firstOffsets[1] -eq ($candidateToInstalledDifference.firstOffsets[0] + 1) -and
    $candidateToInstalledDifference.firstOffsets[2] -eq ($candidateToInstalledDifference.firstOffsets[1] + 1) -and
    $candidateToInstalledDifference.firstLeftAscii -eq 'UNK' -and
    $candidateToInstalledDifference.firstRightAscii -eq 'NSS'
  )
  $result.assertions.runtimePayloadComplete = (
    $result.install.templateFileCount -eq 12 -and
    $result.install.englishTemplateCount -eq 6 -and
    $result.install.simplifiedChineseTemplateCount -eq 6 -and
    $result.install.ffmpegPresent -and
    $result.install.llamaHelperPresent -and
    $result.install.directMlPresent
  )

  $previousBrowserArguments = $env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS
  $env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = '--remote-debugging-port=9362'
  try {
    $launchedProcess = Start-Process -FilePath $installedExe -PassThru -WindowStyle Hidden
    Start-Sleep -Seconds 8
    $alive = -not $launchedProcess.HasExited
    $responding = if ($alive) { (Get-Process -Id $launchedProcess.Id).Responding } else { $false }
    $descendants = if ($alive) { @(Get-ProcessDescendants -ParentId $launchedProcess.Id) } else { @() }
    $webviews = @($descendants | Where-Object { $_.Name -eq 'msedgewebview2.exe' })
    $debugChildren = @($descendants | Where-Object { $_.CommandLine -match 'remote-debugging-port=9362' })
    $debugTargets = @()
    try {
      $debugTargets = @(Invoke-RestMethod -Uri 'http://127.0.0.1:9362/json/list' -TimeoutSec 3)
    } catch {
      $debugTargets = @()
    }

    $result.launch = [ordered]@{
      processId = $launchedProcess.Id
      aliveAfterEightSeconds = $alive
      responding = $responding
      webviewChildCount = $webviews.Count
      remoteDebugChildCount = $debugChildren.Count
      debugTargetCount = $debugTargets.Count
      debugUrls = @($debugTargets | ForEach-Object { $_.url })
    }
    $result.assertions.installedApplicationLaunches = (
      $alive -and $responding -and $webviews.Count -gt 0
    )

    $result.launch.gracefulCloseRequested = Stop-AuditApplication -Process $launchedProcess
    $launchedProcess = $null
  } finally {
    $env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = $previousBrowserArguments
  }

  New-Item -ItemType Directory -Path $testData -Force | Out-Null
  [System.IO.File]::WriteAllText(
    $sentinel,
    "Meetily current Vulkan NSIS isolated retention audit`n2026-08-24",
    [System.Text.UTF8Encoding]::new($false)
  )
  $sentinelHash = (Get-FileHash -LiteralPath $sentinel -Algorithm SHA256).Hash

  $repairExitCode = Invoke-SilentExecutable -FilePath $installer
  $repairInstalledHash = (Get-FileHash -LiteralPath $installedExe -Algorithm SHA256).Hash
  $result.repair = [ordered]@{
    exitCode = $repairExitCode
    installedExecutableSha256 = $repairInstalledHash
    sentinelPreserved = (Test-Path -LiteralPath $sentinel -PathType Leaf)
    sentinelSha256 = (Get-FileHash -LiteralPath $sentinel -Algorithm SHA256).Hash
  }
  $result.assertions.sameVersionRepairSucceeded = (
    $repairExitCode -eq 0 -and
    $repairInstalledHash -eq $result.install.installedExecutable.sha256 -and
    $result.repair.sentinelSha256 -eq $sentinelHash
  )

  $rollbackExitCode = Invoke-SilentExecutable -FilePath $previousInstaller
  $rollbackInstalledHash = (Get-FileHash -LiteralPath $installedExe -Algorithm SHA256).Hash
  $result.rollback = [ordered]@{
    exitCode = $rollbackExitCode
    installedExecutableSha256 = $rollbackInstalledHash
    differsFromCurrentPayload = ($rollbackInstalledHash -ne $result.install.installedExecutable.sha256)
    sentinelPreserved = (Test-Path -LiteralPath $sentinel -PathType Leaf)
    sentinelSha256 = (Get-FileHash -LiteralPath $sentinel -Algorithm SHA256).Hash
  }
  $result.assertions.rollbackToPreviousIsolatedInstallerSucceeded = (
    $rollbackExitCode -eq 0 -and
    $result.rollback.differsFromCurrentPayload -and
    $result.rollback.sentinelSha256 -eq $sentinelHash
  )

  $restoreExitCode = Invoke-SilentExecutable -FilePath $installer
  $restoredInstalledHash = (Get-FileHash -LiteralPath $installedExe -Algorithm SHA256).Hash
  $result.restoreCurrent = [ordered]@{
    exitCode = $restoreExitCode
    installedExecutableSha256 = $restoredInstalledHash
    currentPayloadRestored = ($restoredInstalledHash -eq $result.install.installedExecutable.sha256)
    sentinelPreserved = (Test-Path -LiteralPath $sentinel -PathType Leaf)
    sentinelSha256 = (Get-FileHash -LiteralPath $sentinel -Algorithm SHA256).Hash
  }
  $result.assertions.restoreCurrentInstallerSucceeded = (
    $restoreExitCode -eq 0 -and
    $result.restoreCurrent.currentPayloadRestored -and
    $result.restoreCurrent.sentinelSha256 -eq $sentinelHash
  )

  $entry = Get-AuditRegistryEntry
  if ($null -eq $entry) {
    throw 'Uninstall registry entry disappeared before the uninstall audit.'
  }
  $uninstaller = [System.IO.Path]::GetFullPath($entry.UninstallString.Trim('"'))
  $uninstallExitCode = Invoke-SilentExecutable -FilePath $uninstaller
  $uninstallAttempted = $true
  Start-Sleep -Seconds 3
  $result.uninstall = [ordered]@{
    exitCode = $uninstallExitCode
    registryEntryRemoved = ($null -eq (Get-AuditRegistryEntry))
    installDirectoryRemoved = (-not (Test-Path -LiteralPath $installDir))
    sentinelPreserved = (Test-Path -LiteralPath $sentinel -PathType Leaf)
    sentinelSha256 = if (Test-Path -LiteralPath $sentinel -PathType Leaf) {
      (Get-FileHash -LiteralPath $sentinel -Algorithm SHA256).Hash
    } else {
      $null
    }
  }
  $result.assertions.defaultUninstallSucceeded = (
    $uninstallExitCode -eq 0 -and
    $result.uninstall.registryEntryRemoved -and
    $result.uninstall.installDirectoryRemoved -and
    $result.uninstall.sentinelPreserved -and
    $result.uninstall.sentinelSha256 -eq $sentinelHash
  )

  $result.artifacts.formalFrozenExecutableSha256 = (Get-FileHash -LiteralPath $formalExe -Algorithm SHA256).Hash
  $result.artifacts.formalFrozenInstallerSha256 = (Get-FileHash -LiteralPath $formalInstaller -Algorithm SHA256).Hash
  $result.artifacts.approvedDirectExecutableSha256After = (Get-FileHash -LiteralPath $approvedDirectExe -Algorithm SHA256).Hash
  $result.assertions.protectedArtifactsUnchanged = (
    $result.artifacts.formalFrozenExecutableSha256 -eq $expectedFormalExeHash -and
    $result.artifacts.formalFrozenInstallerSha256 -eq $expectedFormalInstallerHash -and
    $result.artifacts.approvedDirectExecutableSha256After -eq $expectedApprovedDirectHash
  )
  $result.assertions.unsignedBoundaryExplicit = (
    $result.artifacts.installer.signatureStatus -eq 'NotSigned' -and
    $result.artifacts.packagedCandidateExecutable.signatureStatus -eq 'NotSigned' -and
    $result.artifacts.previousInstaller.signatureStatus -eq 'NotSigned' -and
    $result.artifacts.approvedDirectExecutable.signatureStatus -eq 'NotSigned' -and
    $result.install.installedExecutable.signatureStatus -eq 'NotSigned'
  )
} finally {
  if ($null -ne $launchedProcess -and -not $launchedProcess.HasExited) {
    $null = Stop-AuditApplication -Process $launchedProcess
  }

  $remainingEntry = Get-AuditRegistryEntry
  if ($null -ne $remainingEntry -and -not $uninstallAttempted) {
    $remainingUninstaller = [System.IO.Path]::GetFullPath($remainingEntry.UninstallString.Trim('"'))
    if (Test-Path -LiteralPath $remainingUninstaller -PathType Leaf) {
      $null = Invoke-SilentExecutable -FilePath $remainingUninstaller
      Start-Sleep -Seconds 3
    }
  }

  if (Test-Path -LiteralPath $testData) {
    Assert-SafeChildPath -Path $testData -Root $roamingRoot -ExpectedLeaf $bundleId
    [System.IO.Directory]::Delete($testData, $true)
  }

  if (Test-Path -LiteralPath $preservedAppData -PathType Container) {
    if (Test-Path -LiteralPath $testData) {
      throw "Cannot restore preserved AppData because the destination exists: $testData"
    }
    Move-Item -LiteralPath $preservedAppData -Destination $testData
  }

  $restoredManifest = Get-FileManifest -Root $testData
  $currentDirectProcesses = @(
    Get-Process -Name 'meetily' -ErrorAction SilentlyContinue |
      Where-Object { $_.Path -eq $approvedDirectExe }
  )
  $approvedDirectStartedForRestore = $false
  if ($currentDirectProcesses.Count -eq 0) {
    $restoredDirectProcess = Start-Process -FilePath $approvedDirectExe -PassThru -WindowStyle Hidden
    $approvedDirectStartedForRestore = $true
    Start-Sleep -Seconds 5
    $currentDirectProcesses = @(
      Get-Process -Name 'meetily' -ErrorAction SilentlyContinue |
        Where-Object { $_.Path -eq $approvedDirectExe }
    )
  }
  $remainingAuditProcesses = @(
    Get-CimInstance Win32_Process -Filter "Name='meetily.exe'" |
      Where-Object {
        $_.ExecutablePath -like '*Meetily Phase 5 Audit*' -or
        $_.ExecutablePath -like '*target-phase5-release*'
      }
  )

  $result.cleanup = [ordered]@{
    registryEntryAbsent = ($null -eq (Get-AuditRegistryEntry))
    installDirectoryAbsent = (-not (Test-Path -LiteralPath $expectedInstallDir))
    priorAuditAppDataRestored = (Test-ManifestsEqual -Left $preexistingManifest -Right $restoredManifest)
    restoredPriorAuditDataFileCount = $restoredManifest.Count
    preservedBackupPathAbsent = (-not (Test-Path -LiteralPath $preservedAppData))
    approvedDirectProcessCount = $currentDirectProcesses.Count
    approvedDirectProcessIds = @($currentDirectProcesses | ForEach-Object { $_.Id })
    approvedDirectStartedForRestore = $approvedDirectStartedForRestore
    approvedDirectProcessResponding = ($currentDirectProcesses.Count -gt 0 -and ($currentDirectProcesses | Where-Object { $_.Responding }).Count -gt 0)
    remainingAuditProcessCount = $remainingAuditProcesses.Count
    remoteDebugPortProcessCount = @(
      Get-CimInstance Win32_Process |
        Where-Object { $_.CommandLine -match 'remote-debugging-port=9362' }
    ).Count
    userRecordingAndSharedTemplateLocationsTouched = $false
  }
  $result.assertions.cleanupAndPriorStateRestorationSucceeded = (
    $result.cleanup.registryEntryAbsent -and
    $result.cleanup.installDirectoryAbsent -and
    $result.cleanup.priorAuditAppDataRestored -and
    $result.cleanup.preservedBackupPathAbsent -and
    $result.cleanup.approvedDirectProcessResponding -and
    $result.cleanup.remainingAuditProcessCount -eq 0 -and
    $result.cleanup.remoteDebugPortProcessCount -eq 0
  )

  $result.passed = -not ($result.assertions.Values -contains $false)
  $reportDirectory = Split-Path -Parent $reportPath
  New-Item -ItemType Directory -Path $reportDirectory -Force | Out-Null
  [System.IO.File]::WriteAllText(
    $reportPath,
    ($result | ConvertTo-Json -Depth 12),
    [System.Text.UTF8Encoding]::new($false)
  )
}

$result | ConvertTo-Json -Depth 12
if (-not $result.passed) {
  exit 1
}

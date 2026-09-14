[CmdletBinding()]
param()

$ErrorActionPreference = 'Stop'

$repoRoot = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..\..\..'))
$installer = Join-Path $repoRoot 'target-phase5-release\release\bundle\nsis\Meetily Phase 5 Audit_0.4.0_x64-setup.exe'
$candidateExe = Join-Path $repoRoot 'target-phase5-release\release\meetily.exe'
$formalExe = Join-Path $repoRoot 'target\release\meetily.exe'
$formalInstaller = Join-Path $repoRoot 'target\release\bundle\nsis\meetily_0.4.0_x64-setup.exe'
$reportPath = Join-Path $repoRoot 'docs\i18n\audit\phase-5-release\windows-install-audit.json'
$productName = 'Meetily Phase 5 Audit'
$bundleId = 'com.meetily.ai.phase5audit'
$expectedFormalExeHash = '1873D2CCA9B06EEBE8B2D42DC8530B6888BCB91C4267BBD6E6724F6A78257823'
$expectedFormalInstallerHash = 'C6A5BD12F947AEDECAC890C33A48FDEA94E4C11C29FCA03EEF753C25AEE30434'
$appDataRoot = [System.IO.Path]::GetFullPath($env:APPDATA)
$testData = [System.IO.Path]::GetFullPath((Join-Path $appDataRoot $bundleId))
$localAppDataRoot = [System.IO.Path]::GetFullPath($env:LOCALAPPDATA)
$expectedInstallDir = [System.IO.Path]::GetFullPath((Join-Path $localAppDataRoot $productName))
$sentinel = Join-Path $testData 'phase5-data-retention-sentinel.txt'
$launchedProcess = $null
$uninstallAttempted = $false

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

function Assert-SafeAuditPath {
  param(
    [Parameter(Mandatory = $true)][string]$Path,
    [Parameter(Mandatory = $true)][string]$Root,
    [Parameter(Mandatory = $true)][string]$ExpectedLeaf
  )

  $resolvedPath = [System.IO.Path]::GetFullPath($Path)
  $resolvedRoot = [System.IO.Path]::GetFullPath($Root)
  if (-not $resolvedPath.StartsWith($resolvedRoot + [System.IO.Path]::DirectorySeparatorChar)) {
    throw "Unsafe audit path outside expected root: $resolvedPath"
  }
  if ((Split-Path -Leaf $resolvedPath) -ne $ExpectedLeaf) {
    throw "Unexpected audit path leaf: $resolvedPath"
  }
}

Assert-SafeAuditPath -Path $testData -Root $appDataRoot -ExpectedLeaf $bundleId
Assert-SafeAuditPath -Path $expectedInstallDir -Root $localAppDataRoot -ExpectedLeaf $productName

$result = [ordered]@{
  schemaVersion = 1
  phase = '15.8 / Stage 5'
  generatedAt = (Get-Date).ToUniversalTime().ToString('o')
  scope = 'Isolated Windows NSIS install, launch, same-version repair, default uninstall, and data-retention audit'
  productName = $productName
  bundleId = $bundleId
  artifacts = [ordered]@{}
  observations = [ordered]@{}
  assertions = [ordered]@{}
  cleanup = [ordered]@{}
  passed = $false
}

try {
  foreach ($requiredPath in @($installer, $candidateExe, $formalExe, $formalInstaller)) {
    if (-not (Test-Path -LiteralPath $requiredPath -PathType Leaf)) {
      throw "Required artifact is missing: $requiredPath"
    }
  }

  $result.assertions.cleanRegistryBeforeInstall = ($null -eq (Get-AuditRegistryEntry))
  $result.assertions.cleanInstallDirectoryBeforeInstall = (-not (Test-Path -LiteralPath $expectedInstallDir))
  $result.assertions.cleanAppDataBeforeInstall = (-not (Test-Path -LiteralPath $testData))
  if (-not $result.assertions.cleanRegistryBeforeInstall -or
      -not $result.assertions.cleanInstallDirectoryBeforeInstall -or
      -not $result.assertions.cleanAppDataBeforeInstall) {
    throw 'The isolated audit product was not clean before installation.'
  }

  $result.artifacts.installer = [ordered]@{
    path = $installer
    bytes = (Get-Item -LiteralPath $installer).Length
    sha256 = (Get-FileHash -LiteralPath $installer -Algorithm SHA256).Hash
    signatureStatus = (Get-AuthenticodeSignature -LiteralPath $installer).Status.ToString()
  }
  $result.artifacts.candidateExecutable = [ordered]@{
    path = $candidateExe
    bytes = (Get-Item -LiteralPath $candidateExe).Length
    sha256 = (Get-FileHash -LiteralPath $candidateExe -Algorithm SHA256).Hash
    signatureStatus = (Get-AuthenticodeSignature -LiteralPath $candidateExe).Status.ToString()
  }

  $installExitCode = Invoke-SilentExecutable -FilePath $installer
  $entry = Get-AuditRegistryEntry
  if ($null -eq $entry) {
    throw 'The expected isolated uninstall registry entry was not created.'
  }

  $installDir = [System.IO.Path]::GetFullPath($entry.InstallLocation.Trim('"'))
  Assert-SafeAuditPath -Path $installDir -Root $localAppDataRoot -ExpectedLeaf $productName
  $installedExe = Join-Path $installDir 'meetily.exe'
  $uninstaller = [System.IO.Path]::GetFullPath($entry.UninstallString.Trim('"'))

  $result.observations.install = [ordered]@{
    exitCode = $installExitCode
    displayName = $entry.DisplayName
    displayVersion = $entry.DisplayVersion
    installLocation = $installDir
    uninstallPath = $uninstaller
  }
  $result.assertions.firstInstallSucceeded = ($installExitCode -eq 0)
  $result.assertions.registryMetadataCorrect = (
    $entry.DisplayName -eq $productName -and
    $entry.DisplayVersion -eq '0.4.0' -and
    $installDir -eq $expectedInstallDir
  )
  $result.assertions.installedExecutableExists = (Test-Path -LiteralPath $installedExe -PathType Leaf)

  $result.artifacts.installedExecutable = [ordered]@{
    path = $installedExe
    bytes = (Get-Item -LiteralPath $installedExe).Length
    sha256 = (Get-FileHash -LiteralPath $installedExe -Algorithm SHA256).Hash
    signatureStatus = (Get-AuthenticodeSignature -LiteralPath $installedExe).Status.ToString()
    bundlePatchNote = "Tauri's NSIS bundler changes its internal bundle-type marker from UNK to NSS; a payload hash difference from the unbundled release executable is expected."
  }
  $result.assertions.installedPayloadSizeMatchesCandidate = (
    $result.artifacts.installedExecutable.bytes -eq $result.artifacts.candidateExecutable.bytes
  )

  $env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = '--remote-debugging-port=9356'
  $launchedProcess = Start-Process -FilePath $installedExe -PassThru -WindowStyle Hidden
  Start-Sleep -Seconds 5
  $alive = -not $launchedProcess.HasExited
  $workingSet = if ($alive) { (Get-Process -Id $launchedProcess.Id).WorkingSet64 } else { $null }
  $result.observations.launch = [ordered]@{
    processId = $launchedProcess.Id
    aliveAfterFiveSeconds = $alive
    workingSetBytes = $workingSet
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
    "Meetily Phase 5 isolated uninstall retention audit`n2026-08-23",
    [System.Text.UTF8Encoding]::new($false)
  )
  $sentinelHashBeforeRepair = (Get-FileHash -LiteralPath $sentinel -Algorithm SHA256).Hash

  $repairExitCode = Invoke-SilentExecutable -FilePath $installer
  $sentinelHashAfterRepair = (Get-FileHash -LiteralPath $sentinel -Algorithm SHA256).Hash
  $result.observations.repair = [ordered]@{
    exitCode = $repairExitCode
    sentinelHashBefore = $sentinelHashBeforeRepair
    sentinelHashAfter = $sentinelHashAfterRepair
  }
  $result.assertions.sameVersionRepairSucceeded = ($repairExitCode -eq 0)
  $result.assertions.repairPreservedApplicationData = ($sentinelHashBeforeRepair -eq $sentinelHashAfterRepair)

  $uninstallExitCode = Invoke-SilentExecutable -FilePath $uninstaller
  $uninstallAttempted = $true
  Start-Sleep -Seconds 2
  $sentinelStillExists = Test-Path -LiteralPath $sentinel -PathType Leaf
  $sentinelHashAfterUninstall = if ($sentinelStillExists) {
    (Get-FileHash -LiteralPath $sentinel -Algorithm SHA256).Hash
  } else {
    $null
  }
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
    $sentinelStillExists -and $sentinelHashAfterUninstall -eq $sentinelHashBeforeRepair
  )

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
    $result.artifacts.installedExecutable.signatureStatus -eq 'NotSigned'
  )

  $result.passed = -not ($result.assertions.Values -contains $false)
}
finally {
  if ($null -ne $launchedProcess -and -not $launchedProcess.HasExited) {
    Stop-Process -Id $launchedProcess.Id -Force -ErrorAction SilentlyContinue
  }

  $remainingEntry = Get-AuditRegistryEntry
  if ($null -ne $remainingEntry -and -not $uninstallAttempted) {
    $remainingUninstaller = [System.IO.Path]::GetFullPath($remainingEntry.UninstallString.Trim('"'))
    if (Test-Path -LiteralPath $remainingUninstaller -PathType Leaf) {
      $null = Invoke-SilentExecutable -FilePath $remainingUninstaller
    }
  }

  if (Test-Path -LiteralPath $testData) {
    Assert-SafeAuditPath -Path $testData -Root $appDataRoot -ExpectedLeaf $bundleId
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
    [System.Text.UTF8Encoding]::new($false)
  )
}

$result | ConvertTo-Json -Depth 8
if (-not $result.passed) {
  exit 1
}

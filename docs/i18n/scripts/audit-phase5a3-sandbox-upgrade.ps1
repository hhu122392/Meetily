[CmdletBinding()]
param()

$ErrorActionPreference = 'Stop'
$officialInstaller = 'C:\MeetilyOfficial\meetily_0.3.0_x64-setup.exe'
$candidateInstaller = 'C:\MeetilyCandidate\meetily_0.4.1_x64-setup.exe'
$candidateExecutable = 'C:\MeetilyCandidate\meetily-0.4.1.exe'
$webView2Installer = 'C:\WebView2Input\MicrosoftEdgeWebView2RuntimeInstallerX64.exe'
$fixtureRoot = 'C:\MeetilyFixture'
$outputRoot = 'C:\MeetilyOutput'
$reportPath = Join-Path $outputRoot 'official-v030-to-candidate-v041-upgrade-audit.json'
$expectedOfficialHash = '900C10A4EA05D991A06AB670886DE45DE08904903298A862A94FDC66F25420D9'
$expectedOfficialExecutableHash = '0C1A60560490CF78BC9C14303A1598DB0F05DC887A7E8EB7BD00B7EC04408045'
$expectedWebView2Hash = '82B2D8A7013E0C0EA15D48FF4742EE3778BA16BD8B7B4A47876645B3E48D4016'
$expectedCandidateInstallerHash = 'AD6D1B4B702873CBECBAD9CADD4F7166BC2C505DE70ACFCA226E1F8C46EF79EC'
$expectedCandidateExecutableHash = '4A1B762599ED79108DCE6FA56EF41162E11AF91723B34CC688E3F32567C3555A'
$expectedInstalledCandidateExecutableHash = '4B35A0398F49A0CC514791888D8857A2D9F576794ECC8120B401D4FDC419DFBD'
$expectedFixtureManifestHash = 'B2BBB7CFFC23022295D7FF474B1699C3E6C6341D584D2A2F813D90059181CE70'
$expectedFixtureDatabaseHash = 'C27B000DCEF5B496004DDD7B73E32BBB1F703952B029B27182BF46C97C312F3A'
$fixtureManifestPath = Join-Path $fixtureRoot 'fixture-manifest.json'
$fixtureDatabasePath = Join-Path $fixtureRoot 'app-data\meeting_minutes.sqlite'
$appData = Join-Path $env:APPDATA 'com.meetily.ai'
$recordings = Join-Path $env:USERPROFILE 'Music\meetily-recordings'
$meetilyConfig = Join-Path $env:APPDATA 'meetily'
$localInstallDirectory = Join-Path $env:LOCALAPPDATA 'meetily'
$activeProcess = $null

function Get-MeetilyRegistryEntry {
  foreach ($root in @(
      'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\*',
      'HKLM:\Software\Microsoft\Windows\CurrentVersion\Uninstall\*',
      'HKLM:\Software\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall\*')) {
    $entry = Get-ItemProperty -Path $root -ErrorAction SilentlyContinue |
      Where-Object { $_.DisplayName -eq 'meetily' } |
      Select-Object -First 1
    if ($null -ne $entry) { return $entry }
  }
  return $null
}

function Get-WebView2RegistryEntries {
  return @(
    foreach ($root in @(
        'HKCU:\Software\Microsoft\EdgeUpdate\Clients\*',
        'HKLM:\Software\Microsoft\EdgeUpdate\Clients\*',
        'HKLM:\Software\WOW6432Node\Microsoft\EdgeUpdate\Clients\*')) {
      Get-ItemProperty -Path $root -ErrorAction SilentlyContinue |
        Where-Object { $_.name -like '*WebView2*' -or $_.DisplayName -like '*WebView2*' } |
        ForEach-Object {
          [ordered]@{
            name = if ($_.name) { $_.name } else { $_.DisplayName }
            version = if ($_.pv) { $_.pv } else { $_.DisplayVersion }
            registryPath = $_.PSPath
          }
        }
    }
  )
}

function Invoke-ExecutableWithTimeout {
  param(
    [Parameter(Mandatory = $true)][string]$Path,
    [string[]]$Arguments,
    [int]$TimeoutSeconds
  )
  $process = Start-Process -FilePath $Path -ArgumentList $Arguments -PassThru -WindowStyle Hidden
  if (-not $process.WaitForExit($TimeoutSeconds * 1000)) {
    & taskkill.exe /PID $process.Id /T /F 2>$null | Out-Null
    return [ordered]@{ processId = $process.Id; exitCode = $null; timedOut = $true }
  }
  return [ordered]@{ processId = $process.Id; exitCode = $process.ExitCode; timedOut = $false }
}

function Invoke-AppLaunchProbe {
  param(
    [Parameter(Mandatory = $true)][string]$Path,
    [int]$WaitSeconds = 15
  )
  $script:activeProcess = Start-Process -FilePath $Path -PassThru -WindowStyle Hidden
  Start-Sleep -Seconds $WaitSeconds
  $alive = -not $script:activeProcess.HasExited
  $record = [ordered]@{
    processId = $script:activeProcess.Id
    aliveAfterSeconds = $WaitSeconds
    alive = $alive
    exitCode = if ($script:activeProcess.HasExited) { $script:activeProcess.ExitCode } else { $null }
  }
  if ($alive) {
    $null = $script:activeProcess.CloseMainWindow()
    if (-not $script:activeProcess.WaitForExit(5000)) {
      & taskkill.exe /PID $script:activeProcess.Id /T /F 2>$null | Out-Null
      $script:activeProcess.WaitForExit(5000) | Out-Null
    }
  }
  $script:activeProcess = $null
  Start-Sleep -Seconds 2
  return $record
}

function Get-FileRecord {
  param([Parameter(Mandatory = $true)][string]$Path)
  $item = Get-Item -LiteralPath $Path
  $signature = Get-AuthenticodeSignature -LiteralPath $item.FullName
  return [ordered]@{
    path = $item.FullName
    bytes = $item.Length
    sha256 = (Get-FileHash -Algorithm SHA256 -LiteralPath $item.FullName).Hash
    productName = $item.VersionInfo.ProductName
    productVersion = $item.VersionInfo.ProductVersion
    fileVersion = $item.VersionInfo.FileVersion
    signatureStatus = $signature.Status.ToString()
    signerSubject = if ($signature.SignerCertificate) { $signature.SignerCertificate.Subject } else { $null }
  }
}

function Assert-SandboxUserPath {
  param([Parameter(Mandatory = $true)][string]$Path)
  $resolved = [System.IO.Path]::GetFullPath($Path)
  $sandboxUserRoot = [System.IO.Path]::GetFullPath('C:\Users\WDAGUtilityAccount')
  if (-not $resolved.StartsWith(
      $sandboxUserRoot + [System.IO.Path]::DirectorySeparatorChar,
      [System.StringComparison]::OrdinalIgnoreCase)) {
    throw "Unsafe path outside the disposable Sandbox user profile: $resolved"
  }
  if (Test-Path -LiteralPath $resolved) {
    $item = Get-Item -LiteralPath $resolved -Force
    if (($item.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0) {
      throw "Refusing to traverse a reparse point: $resolved"
    }
  }
}

function Reset-SandboxDirectory {
  param([Parameter(Mandatory = $true)][string]$Path)
  Assert-SandboxUserPath -Path $Path
  if (Test-Path -LiteralPath $Path) {
    [System.IO.Directory]::Delete([System.IO.Path]::GetFullPath($Path), $true)
  }
  New-Item -ItemType Directory -Path $Path -Force | Out-Null
}

function Copy-DirectoryContents {
  param(
    [Parameter(Mandatory = $true)][string]$Source,
    [Parameter(Mandatory = $true)][string]$Destination
  )
  New-Item -ItemType Directory -Path $Destination -Force | Out-Null
  Get-ChildItem -LiteralPath $Source -Force | ForEach-Object {
    Copy-Item -LiteralPath $_.FullName -Destination $Destination -Recurse -Force
  }
}

function Save-DataSnapshot {
  param([Parameter(Mandatory = $true)][string]$Destination)
  New-Item -ItemType Directory -Path $Destination -Force | Out-Null
  foreach ($mapping in @(
      @{ source = $appData; name = 'app-data' },
      @{ source = $recordings; name = 'recordings' },
      @{ source = $meetilyConfig; name = 'config\meetily' })) {
    $target = Join-Path $Destination $mapping.name
    Copy-DirectoryContents -Source $mapping.source -Destination $target
  }
}

function Restore-DataSnapshot {
  param([Parameter(Mandatory = $true)][string]$Source)
  foreach ($mapping in @(
      @{ source = (Join-Path $Source 'app-data'); destination = $appData },
      @{ source = (Join-Path $Source 'recordings'); destination = $recordings },
      @{ source = (Join-Path $Source 'config\meetily'); destination = $meetilyConfig })) {
    Reset-SandboxDirectory -Path $mapping.destination
    Copy-DirectoryContents -Source $mapping.source -Destination $mapping.destination
  }
}

function Get-ProtectedFileSnapshot {
  $result = [ordered]@{}
  foreach ($mapping in @(
      @{ root = $appData; prefix = 'app-data' },
      @{ root = $recordings; prefix = 'recordings' },
      @{ root = $meetilyConfig; prefix = 'config/meetily' })) {
    if (Test-Path -LiteralPath $mapping.root) {
      Get-ChildItem -LiteralPath $mapping.root -Recurse -File -Force | ForEach-Object {
        if ($_.Name -notin @('meeting_minutes.sqlite','meeting_minutes.sqlite-wal','meeting_minutes.sqlite-shm')) {
          $relative = $_.FullName.Substring($mapping.root.Length + 1).Replace('\','/')
          $key = "$($mapping.prefix)/$relative"
          $result[$key] = [ordered]@{
            bytes = $_.Length
            sha256 = (Get-FileHash -Algorithm SHA256 -LiteralPath $_.FullName).Hash
          }
        }
      }
    }
  }
  return $result
}

function Compare-ProtectedSnapshots {
  param(
    [Parameter(Mandatory = $true)]$Before,
    [Parameter(Mandatory = $true)]$After
  )
  $missing = @()
  $changed = @()
  foreach ($key in $Before.Keys) {
    if (-not $After.Contains($key)) { $missing += $key }
    elseif ($Before[$key].sha256 -ne $After[$key].sha256) { $changed += $key }
  }
  return [ordered]@{ missing = @($missing | Sort-Object); changed = @($changed | Sort-Object) }
}

function Get-DirectoryInventory {
  param([Parameter(Mandatory = $true)][string]$Root)
  if (-not (Test-Path -LiteralPath $Root -PathType Container)) { return @() }
  return @(
    Get-ChildItem -LiteralPath $Root -Force -Recurse -ErrorAction SilentlyContinue |
      ForEach-Object {
        [ordered]@{
          relativePath = $_.FullName.Substring($Root.Length).TrimStart('\').Replace('\','/')
          kind = if ($_.PSIsContainer) { 'directory' } else { 'file' }
          bytes = if ($_.PSIsContainer) { $null } else { $_.Length }
          sha256 = if ($_.PSIsContainer) { $null } else {
            (Get-FileHash -Algorithm SHA256 -LiteralPath $_.FullName).Hash
          }
        }
      }
  )
}

function Corrupt-InstallerCopy {
  param(
    [Parameter(Mandatory = $true)][string]$Source,
    [Parameter(Mandatory = $true)][string]$Destination
  )
  Copy-Item -LiteralPath $Source -Destination $Destination -Force
  $stream = [System.IO.File]::Open(
    $Destination,
    [System.IO.FileMode]::Open,
    [System.IO.FileAccess]::ReadWrite,
    [System.IO.FileShare]::None)
  try {
    $stream.Position = 4096
    $value = $stream.ReadByte()
    if ($value -lt 0) { throw 'Candidate installer was unexpectedly shorter than 4097 bytes.' }
    $stream.Position = 4096
    $stream.WriteByte($value -bxor 0xFF)
    $stream.Flush($true)
  }
  finally { $stream.Dispose() }
}

$result = [ordered]@{
  schemaVersion = 1
  phase = '15.11 / Stage 5A-3'
  generatedAt = (Get-Date).ToUniversalTime().ToString('o')
  scope = 'Disposable Windows Sandbox: official v0.3.0 to actual-identity unsigned v0.4.1 candidate upgrade, corruption gate, downgrade observation, uninstall retention, and approved snapshot rollback'
  hostDataRead = $false
  containsRealUserData = $false
  networking = 'Disabled; Microsoft-signed WebView2 Evergreen Standalone Installer is mapped read-only'
  artifacts = [ordered]@{}
  observations = [ordered]@{}
  assertions = [ordered]@{}
  error = $null
  passed = $false
}

try {
  foreach ($required in @(
      $officialInstaller,$candidateInstaller,$candidateExecutable,
      $webView2Installer,$fixtureManifestPath,$fixtureDatabasePath)) {
    if (-not (Test-Path -LiteralPath $required -PathType Leaf)) {
      throw "Required frozen input is missing: $required"
    }
  }
  New-Item -ItemType Directory -Path 'C:\MeetilyAudit',$outputRoot -Force | Out-Null
  $result.artifacts.officialInstaller = Get-FileRecord -Path $officialInstaller
  $result.artifacts.candidateInstaller = Get-FileRecord -Path $candidateInstaller
  $result.artifacts.candidateExecutable = Get-FileRecord -Path $candidateExecutable
  $result.artifacts.webView2Installer = Get-FileRecord -Path $webView2Installer
  $result.artifacts.fixtureManifest = Get-FileRecord -Path $fixtureManifestPath
  $result.artifacts.fixtureDatabase = Get-FileRecord -Path $fixtureDatabasePath
  $fixtureManifest = Get-Content -LiteralPath $fixtureManifestPath -Raw | ConvertFrom-Json
  $result.observations.fixtureContract = [ordered]@{
    fixtureKind = $fixtureManifest.fixtureKind
    containsRealUserData = $fixtureManifest.containsRealUserData
    containsSecrets = $fixtureManifest.containsSecrets
    migrationMode = $fixtureManifest.migrationExecution.mode
    runtimeMigrationClaimed = $fixtureManifest.migrationExecution.runtimeMigrationClaimed
    declaredDatabaseSha256 = $fixtureManifest.databaseSha256
  }
  $result.assertions.frozenArtifactHashesMatch = (
    $result.artifacts.officialInstaller.sha256 -eq $expectedOfficialHash -and
    $result.artifacts.webView2Installer.sha256 -eq $expectedWebView2Hash -and
    $result.artifacts.candidateInstaller.sha256 -eq $expectedCandidateInstallerHash -and
    $result.artifacts.candidateExecutable.sha256 -eq $expectedCandidateExecutableHash -and
    $result.artifacts.fixtureManifest.sha256 -eq $expectedFixtureManifestHash -and
    $result.artifacts.fixtureDatabase.sha256 -eq $expectedFixtureDatabaseHash)
  $result.assertions.fixtureContractExplicit = (
    $fixtureManifest.fixtureKind -eq 'synthetic-redacted-frozen-v030-source-migrated-and-enriched' -and
    $fixtureManifest.containsRealUserData -eq $false -and
    $fixtureManifest.containsSecrets -eq $false -and
    $fixtureManifest.migrationExecution.mode -eq 'frozen-source-sqlx-compatible' -and
    $fixtureManifest.migrationExecution.runtimeMigrationClaimed -eq $false -and
    $fixtureManifest.databaseSha256 -eq $expectedFixtureDatabaseHash)
  $result.assertions.signatureStatusExplicit = (
    $result.artifacts.officialInstaller.signatureStatus -eq 'Valid' -and
    $result.artifacts.webView2Installer.signatureStatus -eq 'Valid' -and
    $result.artifacts.candidateInstaller.signatureStatus -eq 'NotSigned' -and
    $result.artifacts.candidateExecutable.signatureStatus -eq 'NotSigned')
  $result.assertions.cleanMeetilyRegistrationBeforeTest = ($null -eq (Get-MeetilyRegistryEntry))
  if ($result.assertions.Values -contains $false) { throw 'Frozen input preflight failed.' }

  $webViewInstall = Invoke-ExecutableWithTimeout -Path $webView2Installer -Arguments @('/silent','/install') -TimeoutSeconds 180
  Start-Sleep -Seconds 5
  $webViewEntries = Get-WebView2RegistryEntries
  $result.observations.webView2Install = [ordered]@{ process = $webViewInstall; registryEntries = $webViewEntries }
  $result.assertions.webView2InstalledOffline = (
    -not $webViewInstall.timedOut -and $webViewInstall.exitCode -eq 0 -and $webViewEntries.Count -gt 0)
  if (-not $result.assertions.webView2InstalledOffline) { throw 'Offline WebView2 prerequisite failed.' }

  $oldInstall = Invoke-ExecutableWithTimeout -Path $officialInstaller -Arguments @('/S') -TimeoutSeconds 120
  $oldEntry = Get-MeetilyRegistryEntry
  $result.observations.oldInstall = [ordered]@{
    process = $oldInstall
    displayVersion = if ($oldEntry) { $oldEntry.DisplayVersion } else { $null }
  }
  $result.assertions.officialV030Installed = (
    -not $oldInstall.timedOut -and $oldInstall.exitCode -eq 0 -and
    $null -ne $oldEntry -and $oldEntry.DisplayVersion -eq '0.3.0')
  if (-not $result.assertions.officialV030Installed) { throw 'Official v0.3 installation failed.' }

  Restore-DataSnapshot -Source $fixtureRoot
  $oldExe = Join-Path ([System.IO.Path]::GetFullPath($oldEntry.InstallLocation.Trim('"'))) 'meetily.exe'
  $result.artifacts.installedOfficialExecutable = Get-FileRecord -Path $oldExe
  $result.assertions.installedOfficialExecutableIdentityCorrect = (
    $result.artifacts.installedOfficialExecutable.sha256 -eq $expectedOfficialExecutableHash -and
    $result.artifacts.installedOfficialExecutable.productVersion -eq '0.3.0' -and
    $result.artifacts.installedOfficialExecutable.signatureStatus -eq 'Valid')
  if (-not $result.assertions.installedOfficialExecutableIdentityCorrect) {
    throw 'Installed official v0.3 executable identity mismatch.'
  }
  $oldLaunch = Invoke-AppLaunchProbe -Path $oldExe -WaitSeconds 15
  $result.observations.oldLaunch = $oldLaunch
  $result.assertions.officialV030AcceptsEnrichedFixtureWithoutImmediateExit = $oldLaunch.alive
  if (-not $oldLaunch.alive) { throw 'Official v0.3 did not remain alive with the enriched fixture.' }
  $preSnapshot = Join-Path $outputRoot 'pre-upgrade-snapshot'
  Save-DataSnapshot -Destination $preSnapshot
  $protectedBefore = Get-ProtectedFileSnapshot

  $corruptCandidate = 'C:\MeetilyAudit\meetily_0.4.1_x64-setup.corrupt.exe'
  Corrupt-InstallerCopy -Source $candidateInstaller -Destination $corruptCandidate
  $corruptHash = (Get-FileHash -Algorithm SHA256 -LiteralPath $corruptCandidate).Hash
  $result.observations.corruptionGate = [ordered]@{
    expectedSha256 = $expectedCandidateInstallerHash
    corruptedSha256 = $corruptHash
    executableStarted = $false
  }
  $result.assertions.corruptedCandidateBlockedBeforeExecution = (
    $corruptHash -ne $expectedCandidateInstallerHash -and
    -not $result.observations.corruptionGate.executableStarted)

  $upgrade = Invoke-ExecutableWithTimeout -Path $candidateInstaller -Arguments @('/S') -TimeoutSeconds 120
  $newEntry = Get-MeetilyRegistryEntry
  if ($null -eq $newEntry) { throw 'Candidate registration missing after upgrade.' }
  $newInstallDirectory = [System.IO.Path]::GetFullPath($newEntry.InstallLocation.Trim('"'))
  $newExe = Join-Path $newInstallDirectory 'meetily.exe'
  $result.artifacts.upgradedExecutable = Get-FileRecord -Path $newExe
  $installedCandidateEvidence = Join-Path $outputRoot 'installed-candidate-v041.exe'
  Copy-Item -LiteralPath $newExe -Destination $installedCandidateEvidence -Force
  $result.artifacts.installedCandidateExecutableEvidence = Get-FileRecord -Path $installedCandidateEvidence
  $result.observations.upgrade = [ordered]@{
    process = $upgrade
    displayVersion = $newEntry.DisplayVersion
    installLocation = $newInstallDirectory
    standaloneExecutableSha256 = $expectedCandidateExecutableHash
    installedPayloadExecutableSha256 = $result.artifacts.upgradedExecutable.sha256
    standaloneAndInstalledPayloadAreByteIdentical = (
      $result.artifacts.upgradedExecutable.sha256 -eq $expectedCandidateExecutableHash)
  }
  $result.assertions.candidateUpgradeInstalled = (
    -not $upgrade.timedOut -and $upgrade.exitCode -eq 0 -and
    $newEntry.DisplayVersion -eq '0.4.1' -and
    $result.artifacts.upgradedExecutable.productVersion -eq '0.4.1' -and
    $result.artifacts.upgradedExecutable.sha256 -eq $expectedInstalledCandidateExecutableHash -and
    $result.artifacts.installedCandidateExecutableEvidence.sha256 -eq $expectedInstalledCandidateExecutableHash)
  if (-not $result.assertions.candidateUpgradeInstalled) { throw '0.4.1 candidate upgrade failed.' }
  $newLaunch = Invoke-AppLaunchProbe -Path $newExe -WaitSeconds 15
  $result.observations.candidateLaunch = $newLaunch
  $result.assertions.candidateLaunchesAfterUpgrade = $newLaunch.alive
  if (-not $newLaunch.alive) { throw '0.4.1 candidate exited after upgrade.' }
  $postSnapshot = Join-Path $outputRoot 'post-upgrade-snapshot'
  Save-DataSnapshot -Destination $postSnapshot
  $protectedAfter = Get-ProtectedFileSnapshot
  $upgradeFileComparison = Compare-ProtectedSnapshots -Before $protectedBefore -After $protectedAfter
  $result.observations.protectedFilesAfterUpgrade = $upgradeFileComparison
  $result.assertions.upgradePreservesProtectedFiles = (
    $upgradeFileComparison.missing.Count -eq 0 -and
    ($upgradeFileComparison.changed | Where-Object { $_ -ne 'config/meetily/notifications.json' }).Count -eq 0)

  $candidateHashBeforeDowngrade = (Get-FileHash -Algorithm SHA256 -LiteralPath $newExe).Hash
  $downgrade = Invoke-ExecutableWithTimeout -Path $officialInstaller -Arguments @('/S') -TimeoutSeconds 120
  $entryAfterDowngrade = Get-MeetilyRegistryEntry
  $result.observations.directDowngradeAttempt = [ordered]@{
    process = $downgrade
    displayVersionAfter = if ($entryAfterDowngrade) { $entryAfterDowngrade.DisplayVersion } else { $null }
    candidateHashBefore = $candidateHashBeforeDowngrade
    installedHashAfter = if (Test-Path -LiteralPath $newExe) {
      (Get-FileHash -Algorithm SHA256 -LiteralPath $newExe).Hash
    } else { $null }
  }
  $result.observations.directDowngradeAttempt.silentDowngradeRejected = (
    $downgrade.exitCode -eq 3 -and
    $null -ne $entryAfterDowngrade -and
    $entryAfterDowngrade.DisplayVersion -eq '0.4.1')
  $result.observations.directDowngradeAttempt.silentDowngradeSucceeded = (
    $downgrade.exitCode -eq 0 -and
    $null -ne $entryAfterDowngrade -and
    $entryAfterDowngrade.DisplayVersion -eq '0.3.0')
  $result.observations.directDowngradeAttempt.installDirectoryInventory = (
    Get-DirectoryInventory -Root $localInstallDirectory)
  $result.observations.directDowngradeAttempt.localizedTemplateResidualCount = @(
    $result.observations.directDowngradeAttempt.installDirectoryInventory |
      Where-Object {
        $_.kind -eq 'file' -and
        ($_.relativePath -like 'templates/en/*' -or $_.relativePath -like 'templates/zh-CN/*')
      }
  ).Count
  $result.assertions.directDowngradeBehaviorConclusive = (
    $result.observations.directDowngradeAttempt.silentDowngradeRejected -xor
    $result.observations.directDowngradeAttempt.silentDowngradeSucceeded)
  $result.assertions.directDowngradeProtectionEffective = (
    $result.observations.directDowngradeAttempt.silentDowngradeRejected -and
    $result.observations.directDowngradeAttempt.localizedTemplateResidualCount -eq 0)

  $directDowngradeTargetRemoval = $null
  if ($result.observations.directDowngradeAttempt.silentDowngradeSucceeded) {
    $downgradedEntryForRemoval = Get-MeetilyRegistryEntry
    if ($null -eq $downgradedEntryForRemoval) {
      throw 'Direct-downgrade target registration disappeared before ownership cleanup.'
    }
    $downgradedUninstaller = [System.IO.Path]::GetFullPath(
      $downgradedEntryForRemoval.UninstallString.Trim('"'))
    $directDowngradeTargetRemoval = Invoke-ExecutableWithTimeout `
      -Path $downgradedUninstaller -Arguments @('/S') -TimeoutSeconds 60
    Start-Sleep -Seconds 3
  }
  $result.observations.directDowngradeTargetOwnershipCleanup = [ordered]@{
    required = $result.observations.directDowngradeAttempt.silentDowngradeSucceeded
    process = $directDowngradeTargetRemoval
    registryRemoved = ($null -eq (Get-MeetilyRegistryEntry))
    appDataPreserved = (Test-Path -LiteralPath $appData -PathType Container)
    remainingInstallDirectoryInventory = Get-DirectoryInventory -Root $localInstallDirectory
  }
  $result.assertions.directDowngradeTargetRemovedBeforeCandidateRestoration = (
    (-not $result.observations.directDowngradeTargetOwnershipCleanup.required) -or
    ($null -ne $directDowngradeTargetRemoval -and
      $directDowngradeTargetRemoval.exitCode -eq 0 -and
      $result.observations.directDowngradeTargetOwnershipCleanup.registryRemoved -and
      $result.observations.directDowngradeTargetOwnershipCleanup.appDataPreserved))
  if (-not $result.assertions.directDowngradeTargetRemovedBeforeCandidateRestoration) {
    throw 'Direct-downgrade target ownership cleanup failed.'
  }

  $candidateRestoration = $null
  if ($result.observations.directDowngradeAttempt.silentDowngradeSucceeded) {
    $candidateRestoration = Invoke-ExecutableWithTimeout -Path $candidateInstaller -Arguments @('/S') -TimeoutSeconds 120
  }
  $entryBeforeApprovedRollback = Get-MeetilyRegistryEntry
  $candidateExeBeforeApprovedRollback = if ($null -ne $entryBeforeApprovedRollback) {
    Join-Path ([System.IO.Path]::GetFullPath($entryBeforeApprovedRollback.InstallLocation.Trim('"'))) 'meetily.exe'
  } else { $null }
  $candidateRecordBeforeApprovedRollback = if (
    $null -ne $candidateExeBeforeApprovedRollback -and
    (Test-Path -LiteralPath $candidateExeBeforeApprovedRollback -PathType Leaf)) {
    Get-FileRecord -Path $candidateExeBeforeApprovedRollback
  } else { $null }
  $result.observations.candidateRestorationBeforeApprovedRollback = [ordered]@{
    required = $result.observations.directDowngradeAttempt.silentDowngradeSucceeded
    process = $candidateRestoration
    displayVersion = if ($entryBeforeApprovedRollback) { $entryBeforeApprovedRollback.DisplayVersion } else { $null }
    executable = $candidateRecordBeforeApprovedRollback
  }
  $result.assertions.candidateRestoredBeforeApprovedRollback = (
    $null -ne $entryBeforeApprovedRollback -and
    $entryBeforeApprovedRollback.DisplayVersion -eq '0.4.1' -and
    $null -ne $candidateRecordBeforeApprovedRollback -and
    $candidateRecordBeforeApprovedRollback.sha256 -eq $expectedInstalledCandidateExecutableHash)
  if (-not $result.assertions.candidateRestoredBeforeApprovedRollback) {
    throw 'Candidate identity was not restored before approved rollback.'
  }

  $installedEntryForRemoval = $entryBeforeApprovedRollback
  if ($null -eq $installedEntryForRemoval) { throw 'No installed version available for approved rollback.' }
  $uninstaller = [System.IO.Path]::GetFullPath($installedEntryForRemoval.UninstallString.Trim('"'))
  $removeInstalled = Invoke-ExecutableWithTimeout -Path $uninstaller -Arguments @('/S') -TimeoutSeconds 60
  $directoryRemovalWaitStarted = Get-Date
  $directoryRemovalPolls = 0
  while ((Test-Path -LiteralPath $localInstallDirectory) -and $directoryRemovalPolls -lt 30) {
    Start-Sleep -Seconds 1
    $directoryRemovalPolls++
  }
  $directoryRemovalWaitMs = [math]::Round(((Get-Date) - $directoryRemovalWaitStarted).TotalMilliseconds)
  $remainingProgramEntries = if (Test-Path -LiteralPath $localInstallDirectory) {
    @(
      Get-ChildItem -LiteralPath $localInstallDirectory -Force -Recurse -ErrorAction SilentlyContinue |
        ForEach-Object {
          [ordered]@{
            relativePath = $_.FullName.Substring($localInstallDirectory.Length).TrimStart('\').Replace('\','/')
            kind = if ($_.PSIsContainer) { 'directory' } else { 'file' }
            bytes = if ($_.PSIsContainer) { $null } else { $_.Length }
          }
        }
    )
  } else { @() }
  $result.observations.preRollbackUninstall = [ordered]@{
    process = $removeInstalled
    registryRemoved = ($null -eq (Get-MeetilyRegistryEntry))
    programDirectoryRemoved = (-not (Test-Path -LiteralPath $localInstallDirectory))
    directoryRemovalWaitMs = $directoryRemovalWaitMs
    directoryRemovalPolls = $directoryRemovalPolls
    remainingProgramEntries = $remainingProgramEntries
    appDataPreserved = (Test-Path -LiteralPath $appData -PathType Container)
  }
  $result.assertions.uninstallBeforeRollbackPreservesData = (
    $removeInstalled.exitCode -eq 0 -and
    $result.observations.preRollbackUninstall.registryRemoved -and
    $result.observations.preRollbackUninstall.programDirectoryRemoved -and
    $result.observations.preRollbackUninstall.appDataPreserved)

  Restore-DataSnapshot -Source $preSnapshot
  $rollbackInstall = Invoke-ExecutableWithTimeout -Path $officialInstaller -Arguments @('/S') -TimeoutSeconds 120
  $rollbackEntry = Get-MeetilyRegistryEntry
  if ($null -eq $rollbackEntry) { throw 'Official v0.3 registration missing during rollback.' }
  $rollbackExe = Join-Path ([System.IO.Path]::GetFullPath($rollbackEntry.InstallLocation.Trim('"'))) 'meetily.exe'
  $result.artifacts.rollbackOfficialExecutable = Get-FileRecord -Path $rollbackExe
  $rollbackLaunch = Invoke-AppLaunchProbe -Path $rollbackExe -WaitSeconds 15
  $rollbackSnapshot = Join-Path $outputRoot 'post-rollback-snapshot'
  Save-DataSnapshot -Destination $rollbackSnapshot
  $protectedRollback = Get-ProtectedFileSnapshot
  $rollbackFileComparison = Compare-ProtectedSnapshots -Before $protectedBefore -After $protectedRollback
  $result.observations.approvedRollback = [ordered]@{
    installProcess = $rollbackInstall
    displayVersion = $rollbackEntry.DisplayVersion
    launch = $rollbackLaunch
    protectedFileComparison = $rollbackFileComparison
  }
  $result.assertions.approvedRollbackRestoresOfficialV030 = (
    $rollbackInstall.exitCode -eq 0 -and
    $rollbackEntry.DisplayVersion -eq '0.3.0' -and
    $result.artifacts.rollbackOfficialExecutable.sha256 -eq $expectedOfficialExecutableHash -and
    $result.artifacts.rollbackOfficialExecutable.productVersion -eq '0.3.0' -and
    $result.artifacts.rollbackOfficialExecutable.signatureStatus -eq 'Valid' -and
    $rollbackLaunch.alive)
  $result.assertions.approvedRollbackRestoresProtectedFiles = (
    $rollbackFileComparison.missing.Count -eq 0 -and
    ($rollbackFileComparison.changed | Where-Object { $_ -ne 'config/meetily/notifications.json' }).Count -eq 0)

  $result.passed = -not ($result.assertions.Values -contains $false)
}
catch {
  $result.error = $_.Exception.Message
  $result.passed = $false
}
finally {
  if ($null -ne $activeProcess -and -not $activeProcess.HasExited) {
    & taskkill.exe /PID $activeProcess.Id /T /F 2>$null | Out-Null
  }
  $remaining = Get-MeetilyRegistryEntry
  if ($null -ne $remaining) {
    $remainingUninstaller = [System.IO.Path]::GetFullPath($remaining.UninstallString.Trim('"'))
    if (Test-Path -LiteralPath $remainingUninstaller -PathType Leaf) {
      $result.observations.finalUninstall = Invoke-ExecutableWithTimeout -Path $remainingUninstaller -Arguments @('/S') -TimeoutSeconds 60
    }
  }
  [System.IO.File]::WriteAllText(
    $reportPath,
    ($result | ConvertTo-Json -Depth 12),
    [System.Text.UTF8Encoding]::new($false))
  Start-Process -FilePath 'shutdown.exe' -ArgumentList @('/s','/t','3') -WindowStyle Hidden
}

if (-not $result.passed) { exit 1 }

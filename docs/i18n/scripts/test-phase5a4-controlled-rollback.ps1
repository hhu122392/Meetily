[CmdletBinding()]
param()

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
Import-Module Microsoft.PowerShell.Utility -ErrorAction Stop
Import-Module (Join-Path $PSScriptRoot 'Meetily.Rollback.psm1') -Force

$repositoryRoot = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..\..\..'))
$manifestPath = Join-Path $repositoryRoot 'docs\i18n\phase-5a4\install-resource-ownership.v1.json'
$testBase = [IO.Path]::GetFullPath((Join-Path ([IO.Path]::GetTempPath()) 'MeetilyPhase5A4Tests')).TrimEnd('\')
$testRoot = Join-Path $testBase ([Guid]::NewGuid().ToString('N'))
$results = [ordered]@{}

function Assert-True {
  param([Parameter(Mandatory = $true)][bool]$Condition, [Parameter(Mandatory = $true)][string]$Message)
  if (-not $Condition) { throw $Message }
}

function Assert-Throws {
  param([Parameter(Mandatory = $true)][scriptblock]$Action, [Parameter(Mandatory = $true)][string]$MessagePattern)
  try {
    & $Action
  } catch {
    if ($_.Exception.Message -notmatch $MessagePattern) {
      throw "Expected error /$MessagePattern/, received: $($_.Exception.Message)"
    }
    return
  }
  throw "Expected action to throw /$MessagePattern/."
}

function Assert-TestPath {
  param([Parameter(Mandatory = $true)][string]$Path)
  $resolved = [IO.Path]::GetFullPath($Path)
  if (-not $resolved.StartsWith($testBase + '\', [StringComparison]::OrdinalIgnoreCase)) {
    throw "Unsafe test path outside dedicated temporary root: $resolved"
  }
}

function Copy-OwnedResources {
  param([Parameter(Mandatory = $true)][string]$Destination, [Parameter(Mandatory = $true)]$Release)
  New-Item -ItemType Directory -Path $Destination -Force | Out-Null
  foreach ($entry in @($Release.ownedResidualFiles)) {
    $source = Join-Path $repositoryRoot (Join-Path 'frontend\src-tauri' ([string]$entry.path).Replace('/', '\'))
    $target = Join-Path $Destination ([string]$entry.path).Replace('/', '\')
    New-Item -ItemType Directory -Path (Split-Path -Parent $target) -Force | Out-Null
    Copy-Item -LiteralPath $source -Destination $target
  }
}

try {
  New-Item -ItemType Directory -Path $testRoot -Force | Out-Null
  $manifest = Read-MeetilyOwnershipManifest -Path $manifestPath
  $results.manifestSchemaAndPolicy = $true

  Assert-True ((Compare-MeetilySemVer -Left '0.4.2' -Right '0.4.1') -eq 1) 'Upgrade comparison failed.'
  Assert-True ((Compare-MeetilySemVer -Left '0.4.1' -Right '0.4.1') -eq 0) 'Equality comparison failed.'
  Assert-True ((Compare-MeetilySemVer -Left '0.3.0' -Right '0.4.1') -eq -1) 'Downgrade comparison failed.'
  Assert-True ((Compare-MeetilySemVer -Left '1.0.0-rc.1' -Right '1.0.0') -eq -1) 'Prerelease comparison failed.'
  Assert-Throws { Compare-MeetilySemVer -Left 'latest' -Right '0.4.1' | Out-Null } 'Invalid semantic version'
  $results.semanticVersionGate = $true

  $release040 = Get-MeetilyOwnedRelease -Manifest $manifest -Version '0.4.0'
  $release041 = Get-MeetilyOwnedRelease -Manifest $manifest -Version '0.4.1'
  Assert-True (@($release040.ownedResidualFiles).Count -eq 18) 'Expected 12 localized plus 6 inherited legacy resources for 0.4.0.'
  Assert-True (@($release041.ownedResidualFiles).Count -eq 18) 'Alias release did not resolve to 18 current and inherited resources.'
  foreach ($entry in @($release040.ownedResidualFiles)) {
    $source = Join-Path $repositoryRoot (Join-Path 'frontend\src-tauri' ([string]$entry.path).Replace('/', '\'))
    $actual = (Get-FileHash -Algorithm SHA256 -LiteralPath $source).Hash.ToLowerInvariant()
    Assert-True ($actual -ceq [string]$entry.sha256) "Ownership hash drift: $($entry.path)"
  }
  $results.ownershipHashesMatchSource = $true

  $exactRoot = Join-Path $testRoot 'exact-owned-residuals'
  Copy-OwnedResources -Destination $exactRoot -Release $release041
  $cleanup = Remove-MeetilyOwnedResiduals -InstallRoot $exactRoot -InstalledVersion '0.4.1' -OwnershipManifest $manifest -Confirm:$false
  Assert-True ($cleanup.InstallRootRemoved -and -not (Test-Path -LiteralPath $exactRoot)) 'Exact owned residual cleanup failed.'
  Assert-True ($cleanup.RemovedFiles.Count -eq 18) 'Exact cleanup removed an unexpected file count.'
  $results.exactHashOwnershipCleanup = $true

  $unknownRoot = Join-Path $testRoot 'unknown-residual'
  Copy-OwnedResources -Destination $unknownRoot -Release $release041
  Set-Content -LiteralPath (Join-Path $unknownRoot 'user-file.txt') -Value 'must survive' -Encoding UTF8
  Assert-Throws {
    Remove-MeetilyOwnedResiduals -InstallRoot $unknownRoot -InstalledVersion '0.4.1' -OwnershipManifest $manifest -Confirm:$false | Out-Null
  } 'Unknown residual file blocks'
  Assert-True (Test-Path -LiteralPath (Join-Path $unknownRoot 'user-file.txt')) 'Unknown file was modified.'
  $results.unknownResidualFailsClosed = $true

  $mismatchRoot = Join-Path $testRoot 'hash-mismatch'
  Copy-OwnedResources -Destination $mismatchRoot -Release $release041
  $mismatchPath = Join-Path $mismatchRoot 'templates\en\daily_standup.json'
  Add-Content -LiteralPath $mismatchPath -Value 'modified'
  Assert-Throws {
    Remove-MeetilyOwnedResiduals -InstallRoot $mismatchRoot -InstalledVersion '0.4.1' -OwnershipManifest $manifest -Confirm:$false | Out-Null
  } 'Residual hash mismatch blocks'
  Assert-True (Test-Path -LiteralPath $mismatchPath) 'Hash-mismatched file was modified.'
  $results.hashMismatchFailsClosed = $true

  $testManifest = $manifest | ConvertTo-Json -Depth 12 | ConvertFrom-Json
  $dataOne = Join-Path $testRoot 'protected-data-one'
  $dataTwo = Join-Path $testRoot 'protected-data-two'
  New-Item -ItemType Directory -Path $dataOne, $dataTwo -Force | Out-Null
  Set-Content -LiteralPath (Join-Path $dataOne 'database.sqlite') -Value 'target-compatible-db' -Encoding UTF8
  Set-Content -LiteralPath (Join-Path $dataTwo 'custom-template.json') -Value '{"name":"custom"}' -Encoding UTF8
  $testManifest.policy.protectedDataRoots = @(
    [pscustomobject]@{ id = 'data-one'; pathTemplate = $dataOne },
    [pscustomobject]@{ id = 'data-two'; pathTemplate = $dataTwo }
  )
  $snapshotRoot = Join-Path $testRoot 'snapshot-v030'
  $snapshot = New-MeetilyDataSnapshot -Destination $snapshotRoot -InstalledVersion '0.3.0' -OwnershipManifest $testManifest
  Test-MeetilyDataSnapshot -SnapshotRoot $snapshot.Path -OwnershipManifest $testManifest -ExpectedVersion '0.3.0' | Out-Null
  Set-Content -LiteralPath (Join-Path $dataOne 'database.sqlite') -Value 'newer-incompatible-db' -Encoding UTF8
  Restore-MeetilyDataSnapshot -SnapshotRoot $snapshot.Path -OwnershipManifest $testManifest -ExpectedVersion '0.3.0'
  Assert-True ((Get-Content -LiteralPath (Join-Path $dataOne 'database.sqlite') -Raw).Trim() -ceq 'target-compatible-db') 'Compatible snapshot restore failed.'
  $results.snapshotCaptureValidateRestore = $true

  $snapshotFile = Join-Path $snapshot.Path 'data\data-one\database.sqlite'
  Add-Content -LiteralPath $snapshotFile -Value 'corrupt'
  Assert-Throws {
    Test-MeetilyDataSnapshot -SnapshotRoot $snapshot.Path -OwnershipManifest $testManifest -ExpectedVersion '0.3.0' | Out-Null
  } 'Snapshot file integrity mismatch'
  $results.corruptedSnapshotFailsClosed = $true

  $duplicateFileSnapshotRoot = Join-Path $testRoot 'snapshot-duplicate-file'
  $duplicateFileSnapshot = New-MeetilyDataSnapshot -Destination $duplicateFileSnapshotRoot -InstalledVersion '0.3.0' -OwnershipManifest $testManifest
  $duplicateFileManifestPath = Join-Path $duplicateFileSnapshot.Path 'snapshot-manifest.json'
  $duplicateFileManifest = Get-Content -LiteralPath $duplicateFileManifestPath -Raw | ConvertFrom-Json
  $duplicateFileManifest.roots[0].files = @($duplicateFileManifest.roots[0].files) + @($duplicateFileManifest.roots[0].files[0])
  $duplicateFileManifest | ConvertTo-Json -Depth 12 | Set-Content -LiteralPath $duplicateFileManifestPath -Encoding UTF8
  Assert-Throws {
    Test-MeetilyDataSnapshot -SnapshotRoot $duplicateFileSnapshot.Path -OwnershipManifest $testManifest -ExpectedVersion '0.3.0' | Out-Null
  } 'Duplicate snapshot file declaration'
  $results.duplicateSnapshotFileFailsClosed = $true

  $duplicateRootSnapshotRoot = Join-Path $testRoot 'snapshot-duplicate-root'
  $duplicateRootSnapshot = New-MeetilyDataSnapshot -Destination $duplicateRootSnapshotRoot -InstalledVersion '0.3.0' -OwnershipManifest $testManifest
  $duplicateRootManifestPath = Join-Path $duplicateRootSnapshot.Path 'snapshot-manifest.json'
  $duplicateRootManifest = Get-Content -LiteralPath $duplicateRootManifestPath -Raw | ConvertFrom-Json
  $duplicateRootManifest.roots = @($duplicateRootManifest.roots[0], $duplicateRootManifest.roots[0])
  $duplicateRootManifest | ConvertTo-Json -Depth 12 | Set-Content -LiteralPath $duplicateRootManifestPath -Encoding UTF8
  Assert-Throws {
    Test-MeetilyDataSnapshot -SnapshotRoot $duplicateRootSnapshot.Path -OwnershipManifest $testManifest -ExpectedVersion '0.3.0' | Out-Null
  } 'Duplicate snapshot protected-root id'
  $results.duplicateSnapshotRootFailsClosed = $true

  [ordered]@{
    schemaVersion = 1
    passed = -not ($results.Values -contains $false)
    assertions = $results
  } | ConvertTo-Json -Depth 6
} finally {
  if (Test-Path -LiteralPath $testRoot) {
    Assert-TestPath -Path $testRoot
    [IO.Directory]::Delete([IO.Path]::GetFullPath($testRoot), $true)
  }
}

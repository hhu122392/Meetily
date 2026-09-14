Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
Import-Module Microsoft.PowerShell.Utility -ErrorAction Stop
Import-Module (Join-Path $PSScriptRoot 'Meetily.Signing.psm1') -Force -ErrorAction Stop

function ConvertTo-MeetilySemVer {
  param([Parameter(Mandatory = $true)][string]$Version)

  $pattern = '^v?(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)(?:-([0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*))?(?:\+[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*)?$'
  if ($Version.Trim() -notmatch $pattern) {
    throw "Invalid semantic version: $Version"
  }
  $pre = if ($Matches[4]) { @($Matches[4].Split('.')) } else { @() }
  foreach ($identifier in $pre) {
    if ($identifier -match '^\d+$' -and $identifier.Length -gt 1 -and $identifier.StartsWith('0')) {
      throw "Invalid numeric prerelease identifier: $Version"
    }
  }
  [pscustomobject]@{
    Major = [uint64]$Matches[1]
    Minor = [uint64]$Matches[2]
    Patch = [uint64]$Matches[3]
    Prerelease = @($pre)
  }
}

function Compare-MeetilySemVer {
  param(
    [Parameter(Mandatory = $true)][string]$Left,
    [Parameter(Mandatory = $true)][string]$Right
  )

  $a = ConvertTo-MeetilySemVer -Version $Left
  $b = ConvertTo-MeetilySemVer -Version $Right
  foreach ($field in @('Major', 'Minor', 'Patch')) {
    if ($a.$field -lt $b.$field) { return -1 }
    if ($a.$field -gt $b.$field) { return 1 }
  }
  $aPrerelease = @($a.Prerelease)
  $bPrerelease = @($b.Prerelease)
  if ($aPrerelease.Count -eq 0 -and $bPrerelease.Count -eq 0) { return 0 }
  if ($aPrerelease.Count -eq 0) { return 1 }
  if ($bPrerelease.Count -eq 0) { return -1 }
  $count = [Math]::Max($aPrerelease.Count, $bPrerelease.Count)
  for ($index = 0; $index -lt $count; $index++) {
    if ($index -ge $aPrerelease.Count) { return -1 }
    if ($index -ge $bPrerelease.Count) { return 1 }
    $leftPart = $aPrerelease[$index]
    $rightPart = $bPrerelease[$index]
    if ($leftPart -ceq $rightPart) { continue }
    $leftNumeric = $leftPart -match '^\d+$'
    $rightNumeric = $rightPart -match '^\d+$'
    if ($leftNumeric -and $rightNumeric) {
      if ([uint64]$leftPart -lt [uint64]$rightPart) { return -1 }
      return 1
    }
    if ($leftNumeric -ne $rightNumeric) {
      if ($leftNumeric) { return -1 }
      return 1
    }
    if ([string]::CompareOrdinal($leftPart, $rightPart) -lt 0) { return -1 }
    return 1
  }
  return 0
}

function Read-MeetilyOwnershipManifest {
  param([Parameter(Mandatory = $true)][string]$Path)

  $resolved = [IO.Path]::GetFullPath($Path)
  if (-not (Test-Path -LiteralPath $resolved -PathType Leaf)) {
    throw "Ownership manifest not found: $resolved"
  }
  try {
    $manifest = Get-Content -LiteralPath $resolved -Raw | ConvertFrom-Json
  } catch {
    throw "Ownership manifest is not valid JSON: $resolved"
  }
  Test-MeetilyOwnershipManifest -Manifest $manifest | Out-Null
  return $manifest
}

function Assert-MeetilyRelativePath {
  param([Parameter(Mandatory = $true)][string]$Path)

  $parts = @($Path.Split('/'))
  if ([string]::IsNullOrWhiteSpace($Path) -or
      [IO.Path]::IsPathRooted($Path) -or
      $Path.Contains('\') -or
      $parts -contains '' -or
      $parts -contains '..' -or
      $parts -contains '.' -or
      $Path.StartsWith('/') -or
      $Path.EndsWith('/')) {
    throw "Unsafe install-relative path in ownership manifest: $Path"
  }
  foreach ($part in $parts) {
    if ($part.IndexOfAny([IO.Path]::GetInvalidFileNameChars()) -ge 0 -or
        $part.EndsWith('.') -or
        $part.EndsWith(' ')) {
      throw "Unsafe install-relative path in ownership manifest: $Path"
    }
  }
}

function Get-MeetilyOwnedRelease {
  param(
    [Parameter(Mandatory = $true)]$Manifest,
    [Parameter(Mandatory = $true)][string]$Version
  )

  $seen = @{}
  $cursor = $Version
  while ($true) {
    if ($seen.ContainsKey($cursor)) {
      throw "Circular sameOwnedResourcesAs reference at version $cursor"
    }
    $seen[$cursor] = $true
    $release = @($Manifest.releases | Where-Object { $_.version -ceq $cursor })
    if ($release.Count -ne 1) {
      throw "Ownership manifest must contain exactly one release for version $cursor"
    }
    if ($release[0].PSObject.Properties.Name -contains 'sameOwnedResourcesAs') {
      $cursor = [string]$release[0].sameOwnedResourcesAs
      continue
    }
    return $release[0]
  }
}

function Test-MeetilyOwnershipManifest {
  param([Parameter(Mandatory = $true)]$Manifest)

  if ($Manifest.schemaVersion -ne 1) { throw 'Unsupported ownership manifest schemaVersion.' }
  if ($Manifest.product.displayName -cne 'meetily' -or
      $Manifest.product.identifier -cne 'com.meetily.ai') {
    throw 'Ownership manifest product identity does not match Meetily.'
  }
  if ($Manifest.policy.unknownResidualFile -cne 'block' -or
      $Manifest.policy.ownedPathHashMismatch -cne 'block' -or
      $Manifest.policy.removeDirectoriesOnlyWhenEmpty -ne $true) {
    throw 'Ownership manifest weakens the required fail-closed cleanup policy.'
  }

  $protectedRootIds = @{}
  foreach ($root in @($Manifest.policy.protectedDataRoots)) {
    $id = [string]$root.id
    if ($id -notmatch '^[a-z0-9][a-z0-9-]*$' -or
        [string]::IsNullOrWhiteSpace([string]$root.pathTemplate)) {
      throw 'Ownership manifest contains an invalid protected-data root.'
    }
    if ($protectedRootIds.ContainsKey($id)) {
      throw "Duplicate protected-data root id: $id"
    }
    $protectedRootIds[$id] = $true
  }
  if ($protectedRootIds.Count -eq 0) { throw 'Ownership manifest must define protected-data roots.' }

  $versions = @{}
  foreach ($release in @($Manifest.releases)) {
    ConvertTo-MeetilySemVer -Version ([string]$release.version) | Out-Null
    if ($versions.ContainsKey([string]$release.version)) {
      throw "Duplicate release version in ownership manifest: $($release.version)"
    }
    $versions[[string]$release.version] = $true
  }
  foreach ($release in @($Manifest.releases)) {
    if ($release.PSObject.Properties.Name -contains 'sameOwnedResourcesAs') {
      if (-not $versions.ContainsKey([string]$release.sameOwnedResourcesAs)) {
        throw "Unknown sameOwnedResourcesAs version: $($release.sameOwnedResourcesAs)"
      }
      continue
    }
    $paths = @{}
    foreach ($file in @($release.ownedResidualFiles)) {
      Assert-MeetilyRelativePath -Path ([string]$file.path)
      if ([string]$file.sha256 -notmatch '^[0-9a-f]{64}$') {
        throw "Invalid lowercase SHA-256 for owned resource: $($file.path)"
      }
      if ($paths.ContainsKey([string]$file.path)) {
        throw "Duplicate owned resource path: $($file.path)"
      }
      $paths[[string]$file.path] = $true
    }
    $directories = @{}
    foreach ($directory in @($release.ownedResidualDirectories)) {
      Assert-MeetilyRelativePath -Path ([string]$directory)
      if ($directories.ContainsKey([string]$directory)) {
        throw "Duplicate owned residual directory: $directory"
      }
      $directories[[string]$directory] = $true
    }
  }
  foreach ($release in @($Manifest.releases)) {
    Get-MeetilyOwnedRelease -Manifest $Manifest -Version ([string]$release.version) | Out-Null
  }
  return $true
}

function Join-MeetilySafeRelativePath {
  param(
    [Parameter(Mandatory = $true)][string]$Root,
    [Parameter(Mandatory = $true)][string]$RelativePath
  )

  Assert-MeetilyRelativePath -Path $RelativePath
  $resolvedRoot = [IO.Path]::GetFullPath($Root).TrimEnd('\')
  $candidate = [IO.Path]::GetFullPath((Join-Path $resolvedRoot ($RelativePath.Replace('/', '\'))))
  if (-not $candidate.StartsWith(
      $resolvedRoot + [IO.Path]::DirectorySeparatorChar,
      [StringComparison]::OrdinalIgnoreCase)) {
    throw "Owned resource escaped install root: $RelativePath"
  }
  return $candidate
}

function Remove-MeetilyOwnedResiduals {
  [CmdletBinding(SupportsShouldProcess = $true)]
  param(
    [Parameter(Mandatory = $true)][string]$InstallRoot,
    [Parameter(Mandatory = $true)][string]$InstalledVersion,
    [Parameter(Mandatory = $true)]$OwnershipManifest
  )

  $root = [IO.Path]::GetFullPath($InstallRoot).TrimEnd('\')
  if (-not (Test-Path -LiteralPath $root)) {
    return [pscustomobject]@{ RemovedFiles = @(); RemovedDirectories = @(); InstallRootRemoved = $true }
  }
  $rootItem = Get-Item -LiteralPath $root -Force
  if (-not $rootItem.PSIsContainer -or
      ($rootItem.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0) {
    throw "Install root is not a safe regular directory: $root"
  }

  $release = Get-MeetilyOwnedRelease -Manifest $OwnershipManifest -Version $InstalledVersion
  $owned = @{}
  foreach ($entry in @($release.ownedResidualFiles)) {
    $owned[[string]$entry.path] = ([string]$entry.sha256).ToUpperInvariant()
  }

  $files = @(Get-ChildItem -LiteralPath $root -File -Force -Recurse)
  foreach ($file in $files) {
    if (($file.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0) {
      throw "Residual file is a reparse point: $($file.FullName)"
    }
    $relative = $file.FullName.Substring($root.Length + 1).Replace('\', '/')
    if (-not $owned.ContainsKey($relative)) {
      throw "Unknown residual file blocks rollback cleanup: $relative"
    }
    $actual = (Get-FileHash -Algorithm SHA256 -LiteralPath $file.FullName).Hash
    if ($actual -cne $owned[$relative]) {
      throw "Residual hash mismatch blocks rollback cleanup: $relative"
    }
  }

  $removedFiles = @()
  foreach ($file in $files) {
    $relative = $file.FullName.Substring($root.Length + 1).Replace('\', '/')
    if ($PSCmdlet.ShouldProcess($file.FullName, 'Remove exact-hash installer-owned residual')) {
      Remove-Item -LiteralPath $file.FullName -Force
      $removedFiles += $relative
    }
  }

  $allowedDirectories = @{}
  foreach ($directory in @($release.ownedResidualDirectories)) {
    $allowedDirectories[[string]$directory] = $true
  }
  $directories = @(Get-ChildItem -LiteralPath $root -Directory -Force -Recurse |
      Sort-Object { $_.FullName.Length } -Descending)
  foreach ($directory in $directories) {
    if (($directory.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0) {
      throw "Residual directory is a reparse point: $($directory.FullName)"
    }
    $relative = $directory.FullName.Substring($root.Length + 1).Replace('\', '/')
    if (-not $allowedDirectories.ContainsKey($relative)) {
      throw "Unknown residual directory blocks rollback cleanup: $relative"
    }
  }

  $removedDirectories = @()
  foreach ($directory in $directories) {
    $relative = $directory.FullName.Substring($root.Length + 1).Replace('\', '/')
    if (Test-Path -LiteralPath $directory.FullName) {
      if (@(Get-ChildItem -LiteralPath $directory.FullName -Force).Count -ne 0) {
        throw "Owned residual directory is not empty after child cleanup: $relative"
      }
      if ($PSCmdlet.ShouldProcess($directory.FullName, 'Remove empty installer-owned directory')) {
        Remove-Item -LiteralPath $directory.FullName -Force
        $removedDirectories += $relative
      }
    }
  }
  if (@(Get-ChildItem -LiteralPath $root -Force).Count -ne 0) {
    throw 'Install root still contains unknown residual entries.'
  }
  if ($PSCmdlet.ShouldProcess($root, 'Remove empty install root')) {
    Remove-Item -LiteralPath $root -Force
  }
  return [pscustomobject]@{
    RemovedFiles = @($removedFiles | Sort-Object)
    RemovedDirectories = @($removedDirectories | Sort-Object)
    InstallRootRemoved = -not (Test-Path -LiteralPath $root)
  }
}

function Get-MeetilyRegistryEntry {
  $entries = @(
    foreach ($root in @(
        'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\*',
        'HKLM:\Software\Microsoft\Windows\CurrentVersion\Uninstall\*',
        'HKLM:\Software\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall\*')) {
      Get-ItemProperty -Path $root -ErrorAction SilentlyContinue |
        Where-Object {
          $_.PSObject.Properties.Name -contains 'DisplayName' -and
          [string]$_.DisplayName -ceq 'meetily'
        }
    }
  )
  if ($entries.Count -gt 1) { throw 'Multiple Meetily uninstall registrations were found.' }
  if ($entries.Count -eq 0) { return $null }
  return $entries[0]
}

function Get-MeetilyExecutableFromCommand {
  param([Parameter(Mandatory = $true)][string]$Command)

  $trimmed = $Command.Trim()
  if ($trimmed -match '^"([^"]+\.exe)"(?:\s|$)') { return [IO.Path]::GetFullPath($Matches[1]) }
  if ($trimmed -match '^(.+?\.exe)(?:\s|$)') { return [IO.Path]::GetFullPath($Matches[1]) }
  throw "Could not extract an executable path from command: $Command"
}

function Get-MeetilyFileEvidence {
  param(
    [Parameter(Mandatory = $true)][string]$Path,
    [Parameter(Mandatory = $true)][string]$ExpectedSha256,
    [Parameter(Mandatory = $true)][ValidateSet('Production', 'Audit')][string]$SecurityMode,
    [string]$SigningPolicyPath,
    [string]$SigningRole,
    [switch]$AuditAllowUnsigned
  )

  $resolved = [IO.Path]::GetFullPath($Path)
  if (-not (Test-Path -LiteralPath $resolved -PathType Leaf)) { throw "File not found: $resolved" }
  if ($ExpectedSha256 -notmatch '^[0-9A-Fa-f]{64}$') { throw 'Expected SHA-256 is malformed.' }
  $actual = (Get-FileHash -Algorithm SHA256 -LiteralPath $resolved).Hash
  if ($actual -cne $ExpectedSha256.ToUpperInvariant()) { throw "SHA-256 mismatch: $resolved" }
  if ($AuditAllowUnsigned -and $SecurityMode -cne 'Audit') {
    throw 'Unsigned installer override is forbidden outside explicit Audit security mode.'
  }
  if ($AuditAllowUnsigned) {
    $signature = Get-AuthenticodeSignature -LiteralPath $resolved
    return [pscustomobject]@{
      Path = $resolved
      Bytes = (Get-Item -LiteralPath $resolved).Length
      Sha256 = $actual
      SecurityMode = $SecurityMode
      SigningRole = $SigningRole
      SigningPolicyPassed = $false
      AuditUnsignedOverride = $true
      SignatureStatus = $signature.Status.ToString()
      SignerSubject = if ($signature.SignerCertificate) { $signature.SignerCertificate.Subject } else { $null }
      SignerThumbprint = if ($signature.SignerCertificate) { $signature.SignerCertificate.Thumbprint } else { $null }
      TimestampCertificatePresent = ($null -ne $signature.TimeStamperCertificate)
    }
  }
  if ([string]::IsNullOrWhiteSpace($SigningPolicyPath)) {
    throw 'A signing policy path is required when the audit unsigned override is not active.'
  }
  if ([string]::IsNullOrWhiteSpace($SigningRole)) { throw 'A signing role is required.' }
  $verification = Test-MeetilySignedFile `
    -Path $resolved `
    -PolicyPath $SigningPolicyPath `
    -Role $SigningRole `
    -ExpectedSha256 $ExpectedSha256 `
    -ThrowOnFailure
  return [pscustomobject]@{
    Path = $resolved
    Bytes = (Get-Item -LiteralPath $resolved).Length
    Sha256 = $actual
    SecurityMode = $SecurityMode
    SigningRole = $SigningRole
    SigningPolicyPassed = $verification.passed
    AuditUnsignedOverride = $false
    SignatureStatus = $verification.evidence.signatureStatus
    SignerSubject = $verification.evidence.signerSubject
    SignerThumbprint = $verification.evidence.signerThumbprint
    TimestampCertificatePresent = $verification.evidence.timestampCertificatePresent
    SignToolDefaultAuthenticodePassed = $verification.evidence.signToolDefaultAuthenticodePassed
  }
}

function Invoke-MeetilyProcess {
  param(
    [Parameter(Mandatory = $true)][string]$Path,
    [string[]]$Arguments = @(),
    [int]$TimeoutSeconds = 120
  )

  $process = Start-Process -FilePath $Path -ArgumentList $Arguments -PassThru -WindowStyle Hidden
  if (-not $process.WaitForExit($TimeoutSeconds * 1000)) {
    & taskkill.exe /PID $process.Id /T /F 2>$null | Out-Null
    throw "Process timed out after $TimeoutSeconds seconds: $Path"
  }
  if ($process.ExitCode -ne 0) { throw "Process exited with code $($process.ExitCode): $Path" }
  return [pscustomobject]@{ ProcessId = $process.Id; ExitCode = $process.ExitCode }
}

function Wait-MeetilyPathAbsent {
  param([Parameter(Mandatory = $true)][string]$Path, [int]$TimeoutSeconds = 30)
  $stopwatch = [Diagnostics.Stopwatch]::StartNew()
  while ((Test-Path -LiteralPath $Path) -and $stopwatch.Elapsed.TotalSeconds -lt $TimeoutSeconds) {
    Start-Sleep -Milliseconds 250
  }
  $stopwatch.Stop()
  return -not (Test-Path -LiteralPath $Path)
}

function Get-MeetilyProtectedDataRoots {
  param(
    [Parameter(Mandatory = $true)]$OwnershipManifest,
    [string[]]$AdditionalProtectedDataPath = @()
  )

  $roots = @()
  foreach ($entry in @($OwnershipManifest.policy.protectedDataRoots)) {
    $expanded = [Environment]::ExpandEnvironmentVariables(([string]$entry.pathTemplate).Replace('/', '\'))
    $roots += [pscustomobject]@{ Id = [string]$entry.id; Path = [IO.Path]::GetFullPath($expanded) }
  }
  $index = 0
  foreach ($path in $AdditionalProtectedDataPath) {
    $index++
    $roots += [pscustomobject]@{ Id = "additional-$index"; Path = [IO.Path]::GetFullPath($path) }
  }
  $unique = @{}
  foreach ($root in $roots) {
    $trimmed = $root.Path.TrimEnd('\')
    if ($trimmed -eq [IO.Path]::GetPathRoot($trimmed)) { throw "Refusing protected-data filesystem root: $trimmed" }
    if ($unique.ContainsKey($trimmed.ToLowerInvariant())) { continue }
    $unique[$trimmed.ToLowerInvariant()] = [pscustomobject]@{ Id = $root.Id; Path = $trimmed }
  }
  return @($unique.Values | Sort-Object Id)
}

function Assert-MeetilyNoReparsePoints {
  param([Parameter(Mandatory = $true)][string]$Root)
  if (-not (Test-Path -LiteralPath $Root)) { return }
  foreach ($item in @(Get-Item -LiteralPath $Root -Force) + @(Get-ChildItem -LiteralPath $Root -Recurse -Force)) {
    if (($item.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0) {
      throw "Protected data contains a reparse point: $($item.FullName)"
    }
  }
}

function New-MeetilyDataSnapshot {
  param(
    [Parameter(Mandatory = $true)][string]$Destination,
    [Parameter(Mandatory = $true)][string]$InstalledVersion,
    [Parameter(Mandatory = $true)]$OwnershipManifest,
    [string[]]$AdditionalProtectedDataPath = @()
  )

  ConvertTo-MeetilySemVer -Version $InstalledVersion | Out-Null
  $destinationRoot = [IO.Path]::GetFullPath($Destination).TrimEnd('\')
  if (Test-Path -LiteralPath $destinationRoot) { throw "Snapshot destination already exists: $destinationRoot" }
  $roots = @(Get-MeetilyProtectedDataRoots -OwnershipManifest $OwnershipManifest -AdditionalProtectedDataPath $AdditionalProtectedDataPath)
  foreach ($root in $roots) {
    if ($destinationRoot.StartsWith($root.Path + '\', [StringComparison]::OrdinalIgnoreCase) -or
        $root.Path.StartsWith($destinationRoot + '\', [StringComparison]::OrdinalIgnoreCase)) {
      throw 'Snapshot destination and protected data roots must not contain one another.'
    }
  }
  New-Item -ItemType Directory -Path $destinationRoot | Out-Null
  $records = @()
  foreach ($root in $roots) {
    $target = Join-Path $destinationRoot (Join-Path 'data' $root.Id)
    $exists = Test-Path -LiteralPath $root.Path -PathType Container
    $files = @()
    if ($exists) {
      Assert-MeetilyNoReparsePoints -Root $root.Path
      New-Item -ItemType Directory -Path $target -Force | Out-Null
      Get-ChildItem -LiteralPath $root.Path -Force | ForEach-Object {
        Copy-Item -LiteralPath $_.FullName -Destination $target -Recurse -Force
      }
      $files = @(
        Get-ChildItem -LiteralPath $target -File -Force -Recurse | ForEach-Object {
          [ordered]@{
            relativePath = $_.FullName.Substring($target.Length + 1).Replace('\', '/')
            bytes = $_.Length
            sha256 = (Get-FileHash -Algorithm SHA256 -LiteralPath $_.FullName).Hash.ToLowerInvariant()
          }
        } | Sort-Object relativePath
      )
    }
    $records += [ordered]@{ id = $root.Id; originalPath = $root.Path; existed = $exists; files = $files }
  }
  $manifest = [ordered]@{
    schemaVersion = 1
    productIdentifier = 'com.meetily.ai'
    installedVersion = $InstalledVersion
    createdAtUtc = [DateTime]::UtcNow.ToString('o')
    roots = $records
  }
  $manifestPath = Join-Path $destinationRoot 'snapshot-manifest.json'
  $manifest | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath $manifestPath -Encoding UTF8
  Test-MeetilyDataSnapshot -SnapshotRoot $destinationRoot -OwnershipManifest $OwnershipManifest -AdditionalProtectedDataPath $AdditionalProtectedDataPath | Out-Null
  return [pscustomobject]@{
    Path = $destinationRoot
    ManifestPath = $manifestPath
    ManifestSha256 = (Get-FileHash -Algorithm SHA256 -LiteralPath $manifestPath).Hash
  }
}

function Test-MeetilyDataSnapshot {
  param(
    [Parameter(Mandatory = $true)][string]$SnapshotRoot,
    [Parameter(Mandatory = $true)]$OwnershipManifest,
    [string]$ExpectedVersion,
    [string[]]$AdditionalProtectedDataPath = @()
  )

  $root = [IO.Path]::GetFullPath($SnapshotRoot).TrimEnd('\')
  $manifestPath = Join-Path $root 'snapshot-manifest.json'
  if (-not (Test-Path -LiteralPath $manifestPath -PathType Leaf)) { throw 'Snapshot manifest is missing.' }
  try { $manifest = Get-Content -LiteralPath $manifestPath -Raw | ConvertFrom-Json } catch { throw 'Snapshot manifest is invalid JSON.' }
  if ($manifest.schemaVersion -ne 1 -or $manifest.productIdentifier -cne 'com.meetily.ai') {
    throw 'Snapshot identity or schema is invalid.'
  }
  ConvertTo-MeetilySemVer -Version ([string]$manifest.installedVersion) | Out-Null
  if ($ExpectedVersion -and ([string]$manifest.installedVersion -cne $ExpectedVersion)) {
    throw "Snapshot version does not match rollback target $ExpectedVersion."
  }
  $expectedRoots = @{}
  foreach ($item in @(Get-MeetilyProtectedDataRoots -OwnershipManifest $OwnershipManifest -AdditionalProtectedDataPath $AdditionalProtectedDataPath)) {
    $expectedRoots[$item.Id] = $item.Path
  }
  if (@($manifest.roots).Count -ne $expectedRoots.Count) { throw 'Snapshot protected-root count mismatch.' }
  $seenRoots = @{}
  foreach ($record in @($manifest.roots)) {
    $rootId = [string]$record.id
    if ($seenRoots.ContainsKey($rootId)) { throw "Duplicate snapshot protected-root id: $rootId" }
    $seenRoots[$rootId] = $true
    if (-not $expectedRoots.ContainsKey($rootId) -or
        $expectedRoots[$rootId] -cne [IO.Path]::GetFullPath([string]$record.originalPath).TrimEnd('\')) {
      throw "Snapshot protected-root mapping mismatch: $rootId"
    }
    $dataRoot = Join-Path $root (Join-Path 'data' $rootId)
    $declared = @{}
    foreach ($file in @($record.files)) {
      $relativePath = [string]$file.relativePath
      Assert-MeetilyRelativePath -Path $relativePath
      if ($declared.ContainsKey($relativePath)) {
        throw "Duplicate snapshot file declaration: $rootId/$relativePath"
      }
      $path = Join-MeetilySafeRelativePath -Root $dataRoot -RelativePath $relativePath
      if (-not (Test-Path -LiteralPath $path -PathType Leaf)) { throw "Snapshot file missing: $($file.relativePath)" }
      $actual = (Get-FileHash -Algorithm SHA256 -LiteralPath $path).Hash.ToLowerInvariant()
      if ($actual -cne [string]$file.sha256 -or (Get-Item -LiteralPath $path).Length -ne [int64]$file.bytes) {
        throw "Snapshot file integrity mismatch: $($file.relativePath)"
      }
      $declared[$relativePath] = $true
    }
    $actualFiles = @(if (Test-Path -LiteralPath $dataRoot) {
      Get-ChildItem -LiteralPath $dataRoot -File -Force -Recurse
    })
    if (@($actualFiles).Count -ne $declared.Count) { throw "Snapshot contains undeclared files for root $rootId." }
  }
  if ($seenRoots.Count -ne $expectedRoots.Count) { throw 'Snapshot protected-root coverage is incomplete.' }
  return $manifest
}

function Restore-MeetilyDataSnapshot {
  param(
    [Parameter(Mandatory = $true)][string]$SnapshotRoot,
    [Parameter(Mandatory = $true)]$OwnershipManifest,
    [Parameter(Mandatory = $true)][string]$ExpectedVersion,
    [string[]]$AdditionalProtectedDataPath = @()
  )

  $manifest = Test-MeetilyDataSnapshot -SnapshotRoot $SnapshotRoot -OwnershipManifest $OwnershipManifest -ExpectedVersion $ExpectedVersion -AdditionalProtectedDataPath $AdditionalProtectedDataPath
  $root = [IO.Path]::GetFullPath($SnapshotRoot).TrimEnd('\')
  foreach ($record in @($manifest.roots)) {
    $destination = [IO.Path]::GetFullPath([string]$record.originalPath).TrimEnd('\')
    if (Test-Path -LiteralPath $destination) {
      Assert-MeetilyNoReparsePoints -Root $destination
      [IO.Directory]::Delete($destination, $true)
    }
    if ($record.existed -eq $true) {
      New-Item -ItemType Directory -Path $destination -Force | Out-Null
      $source = Join-Path $root (Join-Path 'data' ([string]$record.id))
      Get-ChildItem -LiteralPath $source -Force | ForEach-Object {
        Copy-Item -LiteralPath $_.FullName -Destination $destination -Recurse -Force
      }
    }
  }
  Test-MeetilyRestoredData -SnapshotRoot $SnapshotRoot -SnapshotManifest $manifest | Out-Null
}

function Test-MeetilyRestoredData {
  param(
    [Parameter(Mandatory = $true)][string]$SnapshotRoot,
    [Parameter(Mandatory = $true)]$SnapshotManifest
  )

  $root = [IO.Path]::GetFullPath($SnapshotRoot).TrimEnd('\')
  foreach ($record in @($SnapshotManifest.roots)) {
    $destination = [IO.Path]::GetFullPath([string]$record.originalPath).TrimEnd('\')
    if ($record.existed -ne $true) {
      if (Test-Path -LiteralPath $destination) {
        throw "A protected data root that was absent in the snapshot now exists: $($record.id)"
      }
      continue
    }
    if (-not (Test-Path -LiteralPath $destination -PathType Container)) {
      throw "Restored protected data root is missing: $($record.id)"
    }
    Assert-MeetilyNoReparsePoints -Root $destination
    $declared = @{}
    foreach ($file in @($record.files)) {
      $relative = [string]$file.relativePath
      $path = Join-MeetilySafeRelativePath -Root $destination -RelativePath $relative
      if (-not (Test-Path -LiteralPath $path -PathType Leaf)) {
        throw "Restored protected data file is missing: $($record.id)/$relative"
      }
      $actual = (Get-FileHash -Algorithm SHA256 -LiteralPath $path).Hash.ToLowerInvariant()
      if ($actual -cne [string]$file.sha256 -or (Get-Item -LiteralPath $path).Length -ne [int64]$file.bytes) {
        throw "Restored protected data integrity mismatch: $($record.id)/$relative"
      }
      $declared[$relative] = $true
    }
    $actualFiles = @(Get-ChildItem -LiteralPath $destination -File -Force -Recurse)
    if ($actualFiles.Count -ne $declared.Count) {
      throw "Restored protected data contains undeclared files: $($record.id)"
    }
  }
  return $true
}

Export-ModuleMember -Function @(
  'Compare-MeetilySemVer',
  'Read-MeetilyOwnershipManifest',
  'Test-MeetilyOwnershipManifest',
  'Get-MeetilyOwnedRelease',
  'Remove-MeetilyOwnedResiduals',
  'Get-MeetilyRegistryEntry',
  'Get-MeetilyExecutableFromCommand',
  'Get-MeetilyFileEvidence',
  'Invoke-MeetilyProcess',
  'Wait-MeetilyPathAbsent',
  'Get-MeetilyProtectedDataRoots',
  'New-MeetilyDataSnapshot',
  'Test-MeetilyDataSnapshot',
  'Test-MeetilyRestoredData',
  'Restore-MeetilyDataSnapshot'
)

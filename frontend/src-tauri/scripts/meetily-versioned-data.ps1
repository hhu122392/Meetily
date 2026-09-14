[CmdletBinding()]
param(
    [ValidateSet('Backup', 'Verify', 'Restore')][string]$Mode = 'Verify',
    [string]$DataRoot = '',
    [string]$BackupRoot = '',
    [string]$BackupDirectory = '',
    [string]$SourceVersion = '',
    [string]$TargetVersion = '',
    [ValidateSet('None', 'BeforeOriginalMove', 'AfterOriginalMove', 'AfterReplacement', 'BeforeFinalVerification')]
    [string]$FailurePoint = 'None',
    [string]$ResultPath = ''
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
$script:MeetilyVersionedDataToolPath = $PSCommandPath

function Write-MeetilyUtf8File {
    param(
        [Parameter(Mandatory = $true)][string]$Path,
        [Parameter(Mandatory = $true)][AllowEmptyString()][string]$Content
    )
    $full = Get-MeetilyFullPath $Path
    Assert-MeetilyNoReparsePathChain -Path $full -Label 'file write path' | Out-Null
    $parent = [System.IO.Path]::GetDirectoryName($full)
    if (-not [string]::IsNullOrWhiteSpace($parent)) {
        New-MeetilyDirectory -Path $parent -Label 'file write parent' | Out-Null
    }
    Assert-MeetilyNoReparsePathChain -Path $full -Label 'file write path' | Out-Null
    [System.IO.File]::WriteAllText((ConvertTo-MeetilyIoPath $full), $Content, [System.Text.UTF8Encoding]::new($false))
}

function Write-MeetilyJsonFile {
    param(
        [Parameter(Mandatory = $true)][string]$Path,
        [Parameter(Mandatory = $true)]$Value
    )
    Write-MeetilyUtf8File -Path $Path -Content (($Value | ConvertTo-Json -Depth 30) + "`n")
}

function Get-MeetilyFullPath {
    param([Parameter(Mandatory = $true)][string]$Path)
    if ([string]::IsNullOrWhiteSpace($Path)) { throw 'A required path is empty.' }
    $expanded = [Environment]::ExpandEnvironmentVariables($Path)
    if ($expanded.StartsWith('\\?\UNC\', [System.StringComparison]::OrdinalIgnoreCase)) {
        $expanded = '\\' + $expanded.Substring(8)
    } elseif ($expanded.StartsWith('\\?\', [System.StringComparison]::OrdinalIgnoreCase)) {
        $expanded = $expanded.Substring(4)
    }
    return [System.IO.Path]::GetFullPath($expanded)
}

function ConvertTo-MeetilyIoPath {
    param([Parameter(Mandatory = $true)][string]$Path)
    $full = Get-MeetilyFullPath $Path
    if ($full.StartsWith('\\', [System.StringComparison]::Ordinal)) {
        return '\\?\UNC\' + $full.Substring(2)
    }
    return '\\?\' + $full
}

function ConvertFrom-MeetilyIoPath {
    param([Parameter(Mandatory = $true)][string]$Path)
    if ($Path.StartsWith('\\?\UNC\', [System.StringComparison]::OrdinalIgnoreCase)) {
        return '\\' + $Path.Substring(8)
    }
    if ($Path.StartsWith('\\?\', [System.StringComparison]::OrdinalIgnoreCase)) {
        return $Path.Substring(4)
    }
    return $Path
}

function Get-MeetilyPathStateUnchecked {
    param([Parameter(Mandatory = $true)][string]$Path)
    $ioPath = ConvertTo-MeetilyIoPath $Path
    try {
        $attributes = [System.IO.File]::GetAttributes($ioPath)
        return [ordered]@{ exists = $true; attributes = $attributes }
    } catch {
        $exception = $_.Exception
        while ($null -ne $exception.InnerException) { $exception = $exception.InnerException }
        if ($exception -is [System.IO.FileNotFoundException] -or
            $exception -is [System.IO.DirectoryNotFoundException] -or
            $exception.HResult -eq -2147024894 -or
            $exception.HResult -eq -2147024893) {
            return [ordered]@{ exists = $false; attributes = [System.IO.FileAttributes]0 }
        }
        throw
    }
}

function Assert-MeetilyNoReparsePathChain {
    param(
        [Parameter(Mandatory = $true)][string]$Path,
        [Parameter(Mandatory = $true)][string]$Label
    )
    $full = Get-MeetilyFullPath $Path
    $root = [System.IO.Path]::GetPathRoot($full)
    if ([string]::IsNullOrWhiteSpace($root)) { throw "$Label has no filesystem root: $full" }
    $chain = [System.Collections.Generic.List[string]]::new()
    $chain.Add($root)
    $current = $root
    $remaining = $full.Substring($root.Length)
    foreach ($part in @($remaining.Split([char[]]@([char]'\', [char]'/'), [System.StringSplitOptions]::RemoveEmptyEntries))) {
        $current = [System.IO.Path]::Combine($current, $part)
        $chain.Add($current)
    }
    foreach ($candidate in $chain) {
        $state = Get-MeetilyPathStateUnchecked $candidate
        if (-not $state.exists) { break }
        if (($state.attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0) {
            throw "$Label contains a junction, symbolic link, or other reparse point: $candidate"
        }
    }
    return $full
}

function Test-MeetilyPathExists {
    param([Parameter(Mandatory = $true)][string]$Path, [string]$Label = 'path')
    $full = Assert-MeetilyNoReparsePathChain -Path $Path -Label $Label
    return [bool](Get-MeetilyPathStateUnchecked $full).exists
}

function Test-MeetilyDirectoryExists {
    param([Parameter(Mandatory = $true)][string]$Path, [string]$Label = 'directory')
    $full = Assert-MeetilyNoReparsePathChain -Path $Path -Label $Label
    $state = Get-MeetilyPathStateUnchecked $full
    return $state.exists -and (($state.attributes -band [System.IO.FileAttributes]::Directory) -ne 0)
}

function Test-MeetilyFileExists {
    param([Parameter(Mandatory = $true)][string]$Path, [string]$Label = 'file')
    $full = Assert-MeetilyNoReparsePathChain -Path $Path -Label $Label
    $state = Get-MeetilyPathStateUnchecked $full
    return $state.exists -and (($state.attributes -band [System.IO.FileAttributes]::Directory) -eq 0)
}

function New-MeetilyDirectory {
    param([Parameter(Mandatory = $true)][string]$Path, [string]$Label = 'directory creation path')
    $full = Assert-MeetilyNoReparsePathChain -Path $Path -Label $Label
    $state = Get-MeetilyPathStateUnchecked $full
    if ($state.exists -and (($state.attributes -band [System.IO.FileAttributes]::Directory) -eq 0)) {
        throw "$Label exists but is not a directory: $full"
    }
    [System.IO.Directory]::CreateDirectory((ConvertTo-MeetilyIoPath $full)) | Out-Null
    Assert-MeetilyNoReparsePathChain -Path $full -Label $Label | Out-Null
    return $full
}

function Get-MeetilyFileMetadata {
    param([Parameter(Mandatory = $true)][string]$Path)
    $full = Assert-MeetilyNoReparsePathChain -Path $Path -Label 'file read path'
    $state = Get-MeetilyPathStateUnchecked $full
    if (-not $state.exists -or (($state.attributes -band [System.IO.FileAttributes]::Directory) -ne 0)) {
        throw "File does not exist: $full"
    }
    $stream = [System.IO.File]::Open(
        (ConvertTo-MeetilyIoPath $full),
        [System.IO.FileMode]::Open,
        [System.IO.FileAccess]::Read,
        ([System.IO.FileShare]::ReadWrite -bor [System.IO.FileShare]::Delete)
    )
    $sha = [System.Security.Cryptography.SHA256]::Create()
    try {
        $length = [int64]$stream.Length
        $hash = ([System.BitConverter]::ToString($sha.ComputeHash($stream))).Replace('-', '')
        return [ordered]@{ bytes = $length; sha256 = $hash }
    } finally {
        $sha.Dispose()
        $stream.Dispose()
    }
}

function Read-MeetilyUtf8File {
    param([Parameter(Mandatory = $true)][string]$Path)
    $full = Assert-MeetilyNoReparsePathChain -Path $Path -Label 'file read path'
    if (-not (Test-MeetilyFileExists -Path $full -Label 'file read path')) { throw "File does not exist: $full" }
    return [System.IO.File]::ReadAllText((ConvertTo-MeetilyIoPath $full), [System.Text.Encoding]::UTF8)
}

function Test-MeetilyPathWithin {
    param(
        [Parameter(Mandatory = $true)][string]$Candidate,
        [Parameter(Mandatory = $true)][string]$Parent
    )
    $candidateFull = Get-MeetilyFullPath $Candidate
    $parentFull = (Get-MeetilyFullPath $Parent).TrimEnd([System.IO.Path]::DirectorySeparatorChar, [System.IO.Path]::AltDirectorySeparatorChar)
    return $candidateFull.StartsWith(
        $parentFull + [System.IO.Path]::DirectorySeparatorChar,
        [System.StringComparison]::OrdinalIgnoreCase
    )
}

function Assert-MeetilySafeRoot {
    param(
        [Parameter(Mandatory = $true)][string]$Path,
        [Parameter(Mandatory = $true)][string]$Label
    )
    $full = Get-MeetilyFullPath $Path
    $trimmed = $full.TrimEnd([System.IO.Path]::DirectorySeparatorChar, [System.IO.Path]::AltDirectorySeparatorChar)
    $dangerous = @(
        [System.IO.Path]::GetPathRoot($full),
        [Environment]::GetFolderPath([Environment+SpecialFolder]::UserProfile),
        [Environment]::GetFolderPath([Environment+SpecialFolder]::ApplicationData),
        [Environment]::GetFolderPath([Environment+SpecialFolder]::LocalApplicationData)
    ) | Where-Object { -not [string]::IsNullOrWhiteSpace($_) } | ForEach-Object {
        (Get-MeetilyFullPath $_).TrimEnd([System.IO.Path]::DirectorySeparatorChar, [System.IO.Path]::AltDirectorySeparatorChar)
    }
    if ($dangerous -contains $trimmed) { throw "$Label resolves to a protected broad directory: $full" }
    if ([string]::IsNullOrWhiteSpace([System.IO.Path]::GetFileName($trimmed))) { throw "$Label has no final directory name: $full" }
    return $trimmed
}

function Assert-MeetilyVersion {
    param([Parameter(Mandatory = $true)][string]$Version, [Parameter(Mandatory = $true)][string]$Label)
    if ($Version -notmatch '^[0-9]+\.[0-9]+\.[0-9]+(?:[-+][0-9A-Za-z.-]+)?$') {
        throw "$Label is not a supported semantic version: $Version"
    }
}

function Assert-MeetilyNoReparsePoints {
    param([Parameter(Mandatory = $true)][string]$Root)
    $rootFull = Assert-MeetilyNoReparsePathChain -Path $Root -Label 'directory tree root'
    if (-not (Test-MeetilyDirectoryExists -Path $rootFull -Label 'directory tree root')) { return }
    $pending = [System.Collections.Generic.Queue[string]]::new()
    $pending.Enqueue($rootFull)
    while ($pending.Count -gt 0) {
        $directory = $pending.Dequeue()
        foreach ($ioEntry in [System.IO.Directory]::EnumerateFileSystemEntries((ConvertTo-MeetilyIoPath $directory))) {
            $entry = ConvertFrom-MeetilyIoPath $ioEntry
            $state = Get-MeetilyPathStateUnchecked $entry
            if (-not $state.exists) { throw "Directory entry disappeared during safety inspection: $entry" }
            if (($state.attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0) {
                throw "Directory tree contains a junction, symbolic link, or other reparse point: $entry"
            }
            if (($state.attributes -band [System.IO.FileAttributes]::Directory) -ne 0) {
                $pending.Enqueue($entry)
            }
        }
    }
}

function Get-MeetilyDirectoryEntries {
    param([Parameter(Mandatory = $true)][string]$Root)
    $rootFull = Assert-MeetilyNoReparsePathChain -Path $Root -Label 'directory enumeration root'
    if (-not (Test-MeetilyDirectoryExists -Path $rootFull -Label 'directory enumeration root')) { return @() }
    Assert-MeetilyNoReparsePoints $rootFull
    $entries = @()
    $pending = [System.Collections.Generic.Queue[string]]::new()
    $pending.Enqueue($rootFull)
    while ($pending.Count -gt 0) {
        $directory = $pending.Dequeue()
        foreach ($ioEntry in [System.IO.Directory]::EnumerateFileSystemEntries((ConvertTo-MeetilyIoPath $directory))) {
            $entry = ConvertFrom-MeetilyIoPath $ioEntry
            $state = Get-MeetilyPathStateUnchecked $entry
            if (-not $state.exists) { throw "Directory entry disappeared during enumeration: $entry" }
            if (($state.attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0) {
                throw "Directory tree contains a junction, symbolic link, or other reparse point: $entry"
            }
            $isDirectory = (($state.attributes -band [System.IO.FileAttributes]::Directory) -ne 0)
            $entries += [ordered]@{ full_path = $entry; is_directory = $isDirectory }
            if ($isDirectory) { $pending.Enqueue($entry) }
        }
    }
    return @($entries)
}

function ConvertTo-MeetilyRelativePath {
    param(
        [Parameter(Mandatory = $true)][string]$Root,
        [Parameter(Mandatory = $true)][string]$FullPath
    )
    $rootFull = (Get-MeetilyFullPath $Root).TrimEnd([System.IO.Path]::DirectorySeparatorChar, [System.IO.Path]::AltDirectorySeparatorChar)
    $pathFull = Get-MeetilyFullPath $FullPath
    if (-not (Test-MeetilyPathWithin -Candidate $pathFull -Parent $rootFull)) {
        throw "File is outside the expected snapshot root: $pathFull"
    }
    $relative = $pathFull.Substring($rootFull.Length).TrimStart([System.IO.Path]::DirectorySeparatorChar, [System.IO.Path]::AltDirectorySeparatorChar)
    if ([string]::IsNullOrWhiteSpace($relative)) { throw "Snapshot file has an empty relative path: $pathFull" }
    return $relative.Replace('\', '/')
}

function Get-MeetilyDirectoryManifest {
    param([Parameter(Mandatory = $true)][string]$Root)
    $rootFull = Get-MeetilyFullPath $Root
    if (-not (Test-MeetilyDirectoryExists -Path $rootFull -Label 'manifest root')) { return @() }
    return @(
        Get-MeetilyDirectoryEntries $rootFull |
            Where-Object { -not $_.is_directory } |
            Sort-Object full_path |
            ForEach-Object {
                $metadata = Get-MeetilyFileMetadata $_.full_path
                [ordered]@{
                    relative_path = ConvertTo-MeetilyRelativePath -Root $rootFull -FullPath $_.full_path
                    bytes = [int64]$metadata.bytes
                    sha256 = [string]$metadata.sha256
                }
            }
    )
}

function Get-MeetilyManifestFingerprint {
    param([Parameter(Mandatory = $true)][AllowEmptyCollection()]$Files)
    # OrderedDictionary values and ConvertFrom-Json objects expose properties
    # differently in Windows PowerShell 5.1. Build plain strings first and sort
    # them ordinally so a manifest has the same fingerprint before and after it
    # is serialized, and on every supported PowerShell version.
    $canonicalLines = [System.Collections.Generic.List[string]]::new()
    foreach ($file in @($Files)) {
        $canonicalLines.Add(('{0}|{1}|{2}' -f ([string]$file.relative_path), ([int64]$file.bytes), ([string]$file.sha256).ToUpperInvariant()))
    }
    $canonicalLines.Sort([System.StringComparer]::Ordinal)
    $canonical = [string]::Join("`n", $canonicalLines)
    $bytes = [System.Text.Encoding]::UTF8.GetBytes($canonical)
    $sha = [System.Security.Cryptography.SHA256]::Create()
    try {
        return ([System.BitConverter]::ToString($sha.ComputeHash($bytes))).Replace('-', '')
    } finally {
        $sha.Dispose()
    }
}

function Test-MeetilyFileManifestsEqual {
    param(
        [Parameter(Mandatory = $true)][AllowEmptyCollection()]$Left,
        [Parameter(Mandatory = $true)][AllowEmptyCollection()]$Right
    )
    $leftItems = @($Left)
    $rightItems = @($Right)
    if ($leftItems.Count -ne $rightItems.Count) { return $false }
    return (Get-MeetilyManifestFingerprint $leftItems) -eq (Get-MeetilyManifestFingerprint $rightItems)
}

function Get-MeetilyManifestTotalBytes {
    param([Parameter(Mandatory = $true)][AllowEmptyCollection()]$Files)
    $total = [int64]0
    foreach ($file in @($Files)) {
        $total += [int64]$file.bytes
    }
    return $total
}

function Test-MeetilyRecordingPreferencesFile {
    param([Parameter(Mandatory = $true)][string]$DataRoot)
    $dataFull = Get-MeetilyFullPath $DataRoot
    $preferencePath = Assert-MeetilyNoReparsePathChain -Path (Join-Path $dataFull 'recording_preferences.json') -Label 'recording_preferences.json'
    if (-not (Test-MeetilyPathExists -Path $preferencePath -Label 'recording_preferences.json')) {
        return [ordered]@{
            status = 'ABSENT'
            layout = 'none'
            selected_recording_root = $null
            file = $null
        }
    }
    if (-not (Test-MeetilyFileExists -Path $preferencePath -Label 'recording_preferences.json')) {
        throw 'recording_preferences.json exists but is not a regular file.'
    }

    try {
        $document = Read-MeetilyUtf8File $preferencePath | ConvertFrom-Json -ErrorAction Stop
    } catch {
        throw "recording_preferences.json is not valid JSON; refusing default-folder fallback. $($_.Exception.Message)"
    }
    if ($null -eq $document -or $document -isnot [psobject]) {
        throw 'recording_preferences.json must contain a JSON object; refusing default-folder fallback.'
    }

    $preferencesProperty = $document.PSObject.Properties['preferences']
    if ($null -eq $preferencesProperty -or $null -eq $preferencesProperty.Value -or $preferencesProperty.Value -isnot [psobject]) {
        throw 'recording_preferences.json must contain the persisted preferences object; refusing default-folder fallback.'
    }
    $layout = 'store-root'
    $preferences = $preferencesProperty.Value

    $saveFolderProperty = $preferences.PSObject.Properties['save_folder']
    $autoSaveProperty = $preferences.PSObject.Properties['auto_save']
    $fileFormatProperty = $preferences.PSObject.Properties['file_format']
    if ($null -eq $saveFolderProperty -or $saveFolderProperty.Value -isnot [string] -or
        [string]::IsNullOrWhiteSpace([string]$saveFolderProperty.Value)) {
        throw 'recording_preferences.json save_folder must be a non-empty absolute path; refusing default-folder fallback.'
    }
    $selectedRoot = [string]$saveFolderProperty.Value
    if (-not [System.IO.Path]::IsPathRooted($selectedRoot) -or
        $selectedRoot -notmatch '^(?:[A-Za-z]:[\\/]|\\\\[^\\/]+[\\/][^\\/]+)') {
        throw 'recording_preferences.json save_folder must be an absolute Windows path; refusing default-folder fallback.'
    }
    try {
        $selectedRoot = Get-MeetilyFullPath $selectedRoot
    } catch {
        throw "recording_preferences.json save_folder is invalid; refusing default-folder fallback. $($_.Exception.Message)"
    }
    if ($null -eq $autoSaveProperty -or $autoSaveProperty.Value -isnot [bool]) {
        throw 'recording_preferences.json auto_save must be a JSON boolean; refusing default-folder fallback.'
    }
    if ($null -eq $fileFormatProperty -or $fileFormatProperty.Value -isnot [string] -or
        [string]::IsNullOrWhiteSpace([string]$fileFormatProperty.Value)) {
        throw 'recording_preferences.json file_format must be a non-empty string; refusing default-folder fallback.'
    }

    return [ordered]@{
        status = 'VALID'
        layout = $layout
        selected_recording_root = $selectedRoot
        auto_save = [bool]$autoSaveProperty.Value
        file_format = [string]$fileFormatProperty.Value
        file = Get-MeetilyFileMetadata $preferencePath
    }
}

function Get-MeetilyAvailableFreeBytes {
    param([Parameter(Mandatory = $true)][string]$Path)
    $full = Get-MeetilyFullPath $Path
    $volumeRoot = [System.IO.Path]::GetPathRoot($full)
    if ([string]::IsNullOrWhiteSpace($volumeRoot)) { throw "Could not resolve the backup volume for free-space preflight: $full" }
    $drive = [System.IO.DriveInfo]::new($volumeRoot)
    if (-not $drive.IsReady) { throw "Backup volume is not ready for free-space preflight: $volumeRoot" }
    return [int64]$drive.AvailableFreeSpace
}

function Assert-MeetilySufficientBackupSpace {
    param(
        [Parameter(Mandatory = $true)][string]$BackupRoot,
        [Parameter(Mandatory = $true)][int64]$PayloadBytes,
        [int64]$AvailableBytesOverride = -1
    )
    if ($PayloadBytes -lt 0) { throw 'Backup payload byte count cannot be negative.' }
    if ($AvailableBytesOverride -lt -1) { throw 'AvailableBytesOverride cannot be less than -1.' }
    $safetyMarginBytes = [int64](16MB)
    if ($PayloadBytes -gt [int64]::MaxValue - $safetyMarginBytes) { throw 'Backup payload byte count is too large.' }
    $requiredBytes = $PayloadBytes + $safetyMarginBytes
    $availableBytes = if ($AvailableBytesOverride -ge 0) {
        $AvailableBytesOverride
    } else {
        Get-MeetilyAvailableFreeBytes -Path $BackupRoot
    }
    if ($availableBytes -lt $requiredBytes) {
        throw "Insufficient free space for versioned backup: required=$requiredBytes available=$availableBytes."
    }
    return [ordered]@{
        payload_bytes = $PayloadBytes
        safety_margin_bytes = $safetyMarginBytes
        required_bytes = $requiredBytes
        available_bytes = $availableBytes
        passed = $true
        probe = if ($AvailableBytesOverride -ge 0) { 'test-override' } else { 'filesystem' }
    }
}

function Copy-MeetilyDirectoryContents {
    param(
        [Parameter(Mandatory = $true)][string]$Source,
        [Parameter(Mandatory = $true)][string]$Destination
    )
    $sourceFull = Assert-MeetilyNoReparsePathChain -Path $Source -Label 'copy source'
    $destinationFull = Assert-MeetilyNoReparsePathChain -Path $Destination -Label 'copy destination'
    if (-not (Test-MeetilyDirectoryExists -Path $sourceFull -Label 'copy source')) { return }

    # Inspect the complete source before creating or writing the destination. This
    # prevents a nested junction from redirecting a later copy outside our roots.
    $entries = @(Get-MeetilyDirectoryEntries $sourceFull)
    New-MeetilyDirectory -Path $destinationFull -Label 'copy destination' | Out-Null
    foreach ($entry in @($entries | Where-Object { $_.is_directory } | Sort-Object full_path)) {
        $relative = ConvertTo-MeetilyRelativePath -Root $sourceFull -FullPath $entry.full_path
        $target = Join-Path $destinationFull $relative.Replace('/', [System.IO.Path]::DirectorySeparatorChar)
        New-MeetilyDirectory -Path $target -Label 'copy destination directory' | Out-Null
    }
    foreach ($entry in @($entries | Where-Object { -not $_.is_directory } | Sort-Object full_path)) {
        $sourceFile = Assert-MeetilyNoReparsePathChain -Path $entry.full_path -Label 'copy source file'
        $relative = ConvertTo-MeetilyRelativePath -Root $sourceFull -FullPath $sourceFile
        $target = Join-Path $destinationFull $relative.Replace('/', [System.IO.Path]::DirectorySeparatorChar)
        $target = Assert-MeetilyNoReparsePathChain -Path $target -Label 'copy destination file'
        $targetParent = [System.IO.Path]::GetDirectoryName($target)
        New-MeetilyDirectory -Path $targetParent -Label 'copy destination parent' | Out-Null
        if ((Get-MeetilyPathStateUnchecked $target).exists) { throw "Copy destination file already exists: $target" }
        [System.IO.File]::Copy((ConvertTo-MeetilyIoPath $sourceFile), (ConvertTo-MeetilyIoPath $target), $false)
        Assert-MeetilyNoReparsePathChain -Path $target -Label 'copied destination file' | Out-Null
    }
}

function Move-MeetilyDirectory {
    param(
        [Parameter(Mandatory = $true)][string]$Source,
        [Parameter(Mandatory = $true)][string]$Destination,
        [string]$Label = 'directory move'
    )
    $sourceFull = Assert-MeetilyNoReparsePathChain -Path $Source -Label "$Label source"
    $destinationFull = Assert-MeetilyNoReparsePathChain -Path $Destination -Label "$Label destination"
    if (-not (Test-MeetilyDirectoryExists -Path $sourceFull -Label "$Label source")) {
        throw "$Label source directory does not exist: $sourceFull"
    }
    Assert-MeetilyNoReparsePoints $sourceFull
    if ((Get-MeetilyPathStateUnchecked $destinationFull).exists) {
        throw "$Label destination already exists: $destinationFull"
    }
    $destinationParent = [System.IO.Path]::GetDirectoryName($destinationFull)
    if ([string]::IsNullOrWhiteSpace($destinationParent) -or -not (Test-MeetilyDirectoryExists -Path $destinationParent -Label "$Label destination parent")) {
        throw "$Label destination parent does not exist: $destinationParent"
    }
    Assert-MeetilyNoReparsePathChain -Path $sourceFull -Label "$Label source" | Out-Null
    Assert-MeetilyNoReparsePathChain -Path $destinationFull -Label "$Label destination" | Out-Null
    [System.IO.Directory]::Move((ConvertTo-MeetilyIoPath $sourceFull), (ConvertTo-MeetilyIoPath $destinationFull))
    Assert-MeetilyNoReparsePathChain -Path $destinationFull -Label "$Label destination" | Out-Null
    Assert-MeetilyNoReparsePoints $destinationFull
    return $destinationFull
}

function Move-MeetilyFailedArtifact {
    param(
        [Parameter(Mandatory = $true)][string]$Path,
        [Parameter(Mandatory = $true)][string]$BackupRoot,
        [Parameter(Mandatory = $true)][string]$Category
    )
    $pathFull = Assert-MeetilyNoReparsePathChain -Path $Path -Label 'failed artifact source'
    if (-not (Test-MeetilyPathExists -Path $pathFull -Label 'failed artifact source')) { return $null }
    $failedRoot = Assert-MeetilyNoReparsePathChain -Path (Join-Path $BackupRoot $Category) -Label 'failed-artifacts root'
    New-MeetilyDirectory -Path $failedRoot -Label 'failed-artifacts root' | Out-Null
    $destination = Assert-MeetilyNoReparsePathChain -Path (Join-Path $failedRoot ('f-' + [guid]::NewGuid().ToString('N').Substring(0, 12))) -Label 'failed-artifacts destination'
    return Move-MeetilyDirectory -Source $pathFull -Destination $destination -Label 'failed artifact move'
}

function Assert-MeetilyRoots {
    param(
        [Parameter(Mandatory = $true)][string]$DataRoot,
        [Parameter(Mandatory = $true)][string]$BackupRoot
    )
    $dataFull = Assert-MeetilySafeRoot -Path $DataRoot -Label 'DataRoot'
    $backupFull = Assert-MeetilySafeRoot -Path $BackupRoot -Label 'BackupRoot'
    Assert-MeetilyNoReparsePathChain -Path $dataFull -Label 'DataRoot' | Out-Null
    Assert-MeetilyNoReparsePathChain -Path $backupFull -Label 'BackupRoot' | Out-Null
    $dataVolumeRoot = [System.IO.Path]::GetPathRoot($dataFull)
    $backupVolumeRoot = [System.IO.Path]::GetPathRoot($backupFull)
    if ([string]::IsNullOrWhiteSpace($dataVolumeRoot) -or
        [string]::IsNullOrWhiteSpace($backupVolumeRoot) -or
        -not $dataVolumeRoot.Equals($backupVolumeRoot, [System.StringComparison]::OrdinalIgnoreCase)) {
        throw 'DataRoot and BackupRoot must be on the same filesystem volume.'
    }
    if ($dataFull -eq $backupFull -or (Test-MeetilyPathWithin -Candidate $backupFull -Parent $dataFull) -or (Test-MeetilyPathWithin -Candidate $dataFull -Parent $backupFull)) {
        throw 'DataRoot and BackupRoot must be separate, non-nested directories.'
    }
    return [ordered]@{ data = $dataFull; backup = $backupFull }
}

function Install-MeetilyRollbackToolCopy {
    param([Parameter(Mandatory = $true)][string]$BackupRoot)
    if ([string]::IsNullOrWhiteSpace($script:MeetilyVersionedDataToolPath) -or -not (Test-MeetilyFileExists -Path $script:MeetilyVersionedDataToolPath -Label 'rollback tool source')) {
        return $null
    }
    $toolsRoot = Assert-MeetilyNoReparsePathChain -Path (Join-Path $BackupRoot 'tools') -Label 'tools directory'
    New-MeetilyDirectory -Path $toolsRoot -Label 'tools directory' | Out-Null
    Assert-MeetilyNoReparsePoints $toolsRoot
    $destination = Assert-MeetilyNoReparsePathChain -Path (Join-Path $toolsRoot 'meetily-versioned-data.ps1') -Label 'rollback tool destination'
    [System.IO.File]::Copy((ConvertTo-MeetilyIoPath $script:MeetilyVersionedDataToolPath), (ConvertTo-MeetilyIoPath $destination), $true)
    $metadata = Get-MeetilyFileMetadata $destination
    $hash = [string]$metadata.sha256
    Write-MeetilyUtf8File -Path ($destination + '.sha256') -Content ($hash + "  meetily-versioned-data.ps1`n")
    return [ordered]@{ path = $destination; bytes = [int64]$metadata.bytes; sha256 = $hash }
}

function New-MeetilyVersionedBackup {
    [CmdletBinding()]
    param(
        [Parameter(Mandatory = $true)][string]$DataRoot,
        [Parameter(Mandatory = $true)][string]$BackupRoot,
        [Parameter(Mandatory = $true)][string]$SourceVersion,
        [Parameter(Mandatory = $true)][string]$TargetVersion,
        [int64]$AvailableBytesOverride = -1
    )
    Assert-MeetilyVersion -Version $SourceVersion -Label 'SourceVersion'
    Assert-MeetilyVersion -Version $TargetVersion -Label 'TargetVersion'
    if ($SourceVersion -eq $TargetVersion) { throw 'SourceVersion and TargetVersion must differ for an upgrade backup.' }
    $roots = Assert-MeetilyRoots -DataRoot $DataRoot -BackupRoot $BackupRoot
    $dataFull = $roots.data
    $backupFull = $roots.backup
    $backupId = 'b-{0}-{1}' -f (Get-Date -Format 'yyMMddHHmmssfff'), ([guid]::NewGuid().ToString('N').Substring(0, 8))
    $stagingDirectory = Assert-MeetilyNoReparsePathChain -Path (Join-Path $backupFull ('.s-' + [guid]::NewGuid().ToString('N').Substring(0, 8))) -Label 'backup staging directory'
    $finalDirectory = Assert-MeetilyNoReparsePathChain -Path (Join-Path $backupFull $backupId) -Label 'final backup directory'
    $payloadDirectory = Assert-MeetilyNoReparsePathChain -Path (Join-Path $stagingDirectory 'payload') -Label 'backup payload directory'
    $toolsDirectory = Assert-MeetilyNoReparsePathChain -Path (Join-Path $backupFull 'tools') -Label 'tools directory'
    $failedBackupsDirectory = Assert-MeetilyNoReparsePathChain -Path (Join-Path $backupFull 'failed-backups') -Label 'failed-artifacts root'
    $failedArtifact = $null

    try {
        if ((Test-MeetilyPathExists -Path $stagingDirectory -Label 'backup staging directory') -or
            (Test-MeetilyPathExists -Path $finalDirectory -Label 'final backup directory')) {
            throw "A generated backup path already exists: $backupId"
        }

        # Check every predictable destination before the first write. In
        # particular, a pre-created tools/failed-artifacts junction must fail
        # before any snapshot is copied.
        Assert-MeetilyNoReparsePathChain -Path $toolsDirectory -Label 'tools directory' | Out-Null
        Assert-MeetilyNoReparsePathChain -Path $failedBackupsDirectory -Label 'failed-artifacts root' | Out-Null
        $dataPathExists = Test-MeetilyPathExists -Path $dataFull -Label 'DataRoot'
        $dataPresent = Test-MeetilyDirectoryExists -Path $dataFull -Label 'DataRoot'
        if ($dataPathExists -and -not $dataPresent) { throw 'DataRoot exists but is not a directory.' }
        $recordingPreferencesValidation = if ($dataPresent) {
            Test-MeetilyRecordingPreferencesFile -DataRoot $dataFull
        } else {
            [ordered]@{ status = 'ABSENT'; layout = 'none'; selected_recording_root = $null; file = $null }
        }
        $sourceBefore = @(if ($dataPresent) { Get-MeetilyDirectoryManifest $dataFull })
        $spacePreflight = Assert-MeetilySufficientBackupSpace -BackupRoot $backupFull `
            -PayloadBytes (Get-MeetilyManifestTotalBytes $sourceBefore) -AvailableBytesOverride $AvailableBytesOverride

        New-MeetilyDirectory -Path $backupFull -Label 'BackupRoot' | Out-Null
        Assert-MeetilyNoReparsePathChain -Path $stagingDirectory -Label 'backup staging directory' | Out-Null
        Assert-MeetilyNoReparsePathChain -Path $finalDirectory -Label 'final backup directory' | Out-Null
        New-MeetilyDirectory -Path $payloadDirectory -Label 'backup payload directory' | Out-Null
        if ($dataPresent) { Copy-MeetilyDirectoryContents -Source $dataFull -Destination $payloadDirectory }
        $sourceAfter = @(if ($dataPresent) { Get-MeetilyDirectoryManifest $dataFull })
        if (-not (Test-MeetilyFileManifestsEqual -Left $sourceBefore -Right $sourceAfter)) {
            throw 'DataRoot changed while the backup was being copied. The backup is rejected.'
        }
        $payloadFiles = @(Get-MeetilyDirectoryManifest $payloadDirectory)
        if (-not (Test-MeetilyFileManifestsEqual -Left $sourceBefore -Right $payloadFiles)) {
            throw 'The copied backup payload does not match the source data.'
        }
        $manifest = [ordered]@{
            schema_version = 1
            backup_id = $backupId
            created_at = (Get-Date).ToString('o')
            source_version = $SourceVersion
            upgrade_target_version = $TargetVersion
            data_root_leaf = Split-Path -Leaf $dataFull
            data_root_was_present = $dataPresent
            file_count = @($payloadFiles).Count
            total_bytes = Get-MeetilyManifestTotalBytes $payloadFiles
            files_fingerprint_sha256 = Get-MeetilyManifestFingerprint $payloadFiles
            files = @($payloadFiles)
        }
        $stagingManifestPath = Join-Path $stagingDirectory 'backup-manifest.json'
        Write-MeetilyJsonFile -Path $stagingManifestPath -Value $manifest
        Move-MeetilyDirectory -Source $stagingDirectory -Destination $finalDirectory -Label 'backup publish' | Out-Null
        $toolCopy = Install-MeetilyRollbackToolCopy -BackupRoot $backupFull
        $manifestPath = Join-Path $finalDirectory 'backup-manifest.json'
        $manifestMetadata = Get-MeetilyFileMetadata $manifestPath
        return [ordered]@{
            action = 'backup'
            status = 'PASS'
            backup_directory = $finalDirectory
            manifest_path = $manifestPath
            manifest_sha256 = [string]$manifestMetadata.sha256
            source_version = $SourceVersion
            target_version = $TargetVersion
            data_root_was_present = $dataPresent
            file_count = @($payloadFiles).Count
            total_bytes = $manifest.total_bytes
            tool_copy = $toolCopy
            recording_preferences_validation = $recordingPreferencesValidation
            space_preflight = $spacePreflight
        }
    } catch {
        if (Test-MeetilyPathExists -Path $stagingDirectory -Label 'backup staging directory') {
            try { $failedArtifact = Move-MeetilyFailedArtifact -Path $stagingDirectory -BackupRoot $backupFull -Category 'failed-backups' } catch { }
        }
        throw "Versioned backup failed; original data was not replaced. Failed artifact: $failedArtifact. $($_.Exception.Message)"
    }
}

function Read-MeetilyBackupManifest {
    param(
        [Parameter(Mandatory = $true)][string]$BackupDirectory,
        [Parameter(Mandatory = $true)][string]$BackupRoot,
        [Parameter(Mandatory = $true)][string]$ExpectedSourceVersion,
        [Parameter(Mandatory = $true)][string]$ExpectedTargetVersion,
        [Parameter(Mandatory = $true)][string]$ExpectedDataRootLeaf
    )
    Assert-MeetilyVersion -Version $ExpectedSourceVersion -Label 'ExpectedSourceVersion'
    Assert-MeetilyVersion -Version $ExpectedTargetVersion -Label 'ExpectedTargetVersion'
    $backupRootFull = Assert-MeetilySafeRoot -Path $BackupRoot -Label 'BackupRoot'
    $backupDirectoryFull = Assert-MeetilySafeRoot -Path $BackupDirectory -Label 'BackupDirectory'
    Assert-MeetilyNoReparsePathChain -Path $backupRootFull -Label 'BackupRoot' | Out-Null
    Assert-MeetilyNoReparsePathChain -Path $backupDirectoryFull -Label 'BackupDirectory' | Out-Null
    if (-not (Test-MeetilyPathWithin -Candidate $backupDirectoryFull -Parent $backupRootFull)) {
        throw 'BackupDirectory must be a child of BackupRoot.'
    }
    if (-not (Test-MeetilyDirectoryExists -Path $backupDirectoryFull -Label 'BackupDirectory')) { throw 'BackupDirectory does not exist.' }
    $manifestPath = Assert-MeetilyNoReparsePathChain -Path (Join-Path $backupDirectoryFull 'backup-manifest.json') -Label 'backup manifest path'
    $payloadPath = Assert-MeetilyNoReparsePathChain -Path (Join-Path $backupDirectoryFull 'payload') -Label 'backup payload directory'
    if (-not (Test-MeetilyFileExists -Path $manifestPath -Label 'backup manifest path')) { throw 'Backup manifest is missing.' }
    if (-not (Test-MeetilyDirectoryExists -Path $payloadPath -Label 'backup payload directory')) { throw 'Backup payload directory is missing.' }
    Assert-MeetilyNoReparsePoints $payloadPath
    $manifest = Read-MeetilyUtf8File $manifestPath | ConvertFrom-Json
    if ([int]$manifest.schema_version -ne 1) { throw "Unsupported backup manifest schema: $($manifest.schema_version)" }
    if ([string]$manifest.source_version -ne $ExpectedSourceVersion) { throw 'Backup source version does not match the requested restore version.' }
    if ([string]$manifest.upgrade_target_version -ne $ExpectedTargetVersion) { throw 'Backup target version does not match the currently installed upgrade version.' }
    if ([string]$manifest.data_root_leaf -ne $ExpectedDataRootLeaf) { throw 'Backup data-root identity does not match the restore target.' }
    $dataRootPresenceProperty = $manifest.PSObject.Properties['data_root_was_present']
    if ($null -eq $dataRootPresenceProperty -or $dataRootPresenceProperty.Value -isnot [bool]) {
        throw 'Backup manifest data_root_was_present must be a JSON boolean.'
    }

    $declaredFiles = @($manifest.files)
    $seen = [System.Collections.Generic.HashSet[string]]::new([System.StringComparer]::OrdinalIgnoreCase)
    foreach ($file in $declaredFiles) {
        $relative = [string]$file.relative_path
        if ([string]::IsNullOrWhiteSpace($relative) -or [System.IO.Path]::IsPathRooted($relative) -or $relative -match '(^|[\\/])\.\.([\\/]|$)') {
            throw "Backup manifest contains an unsafe relative path: $relative"
        }
        if (-not $seen.Add($relative)) { throw "Backup manifest contains a duplicate path: $relative" }
        if ([int64]$file.bytes -lt 0 -or [string]$file.sha256 -notmatch '^[0-9A-Fa-f]{64}$') {
            throw "Backup manifest contains invalid file metadata: $relative"
        }
        $resolved = Get-MeetilyFullPath (Join-Path $payloadPath $relative.Replace('/', [System.IO.Path]::DirectorySeparatorChar))
        if (-not (Test-MeetilyPathWithin -Candidate $resolved -Parent $payloadPath)) {
            throw "Backup manifest path escapes its payload: $relative"
        }
    }
    if ([int]$manifest.file_count -ne $declaredFiles.Count) { throw 'Backup manifest file count is inconsistent.' }
    if ([int64]$manifest.total_bytes -ne (Get-MeetilyManifestTotalBytes $declaredFiles)) { throw 'Backup manifest byte total is inconsistent.' }
    if ([string]$manifest.files_fingerprint_sha256 -ne (Get-MeetilyManifestFingerprint $declaredFiles)) { throw 'Backup manifest fingerprint is inconsistent.' }
    $actualFiles = @(Get-MeetilyDirectoryManifest $payloadPath)
    if (-not (Test-MeetilyFileManifestsEqual -Left $declaredFiles -Right $actualFiles)) { throw 'Backup payload size or SHA-256 does not match its manifest.' }
    return [ordered]@{
        backup_directory = $backupDirectoryFull
        manifest_path = $manifestPath
        payload_path = $payloadPath
        manifest = $manifest
        actual_files = $actualFiles
    }
}

function Test-MeetilyVersionedBackup {
    [CmdletBinding()]
    param(
        [Parameter(Mandatory = $true)][string]$DataRoot,
        [Parameter(Mandatory = $true)][string]$BackupRoot,
        [Parameter(Mandatory = $true)][string]$BackupDirectory,
        [Parameter(Mandatory = $true)][string]$SourceVersion,
        [Parameter(Mandatory = $true)][string]$TargetVersion
    )
    $roots = Assert-MeetilyRoots -DataRoot $DataRoot -BackupRoot $BackupRoot
    $verified = Read-MeetilyBackupManifest -BackupDirectory $BackupDirectory -BackupRoot $roots.backup -ExpectedSourceVersion $SourceVersion -ExpectedTargetVersion $TargetVersion -ExpectedDataRootLeaf (Split-Path -Leaf $roots.data)
    return [ordered]@{
        action = 'verify'
        status = 'PASS'
        backup_directory = $verified.backup_directory
        manifest_path = $verified.manifest_path
        manifest_sha256 = [string](Get-MeetilyFileMetadata $verified.manifest_path).sha256
        source_version = [string]$verified.manifest.source_version
        target_version = [string]$verified.manifest.upgrade_target_version
        data_root_was_present = [bool]$verified.manifest.data_root_was_present
        file_count = [int]$verified.manifest.file_count
        total_bytes = [int64]$verified.manifest.total_bytes
    }
}

function Restore-MeetilyVersionedBackup {
    [CmdletBinding()]
    param(
        [Parameter(Mandatory = $true)][string]$DataRoot,
        [Parameter(Mandatory = $true)][string]$BackupRoot,
        [Parameter(Mandatory = $true)][string]$BackupDirectory,
        [Parameter(Mandatory = $true)][string]$SourceVersion,
        [Parameter(Mandatory = $true)][string]$TargetVersion,
        [ValidateSet('None', 'BeforeOriginalMove', 'AfterOriginalMove', 'AfterReplacement', 'BeforeFinalVerification')]
        [string]$FailurePoint = 'None'
    )
    $roots = Assert-MeetilyRoots -DataRoot $DataRoot -BackupRoot $BackupRoot
    $dataFull = $roots.data
    $backupFull = $roots.backup
    if (-not (Test-MeetilyDirectoryExists -Path $backupFull -Label 'BackupRoot')) { throw 'BackupRoot does not exist.' }
    $verified = Read-MeetilyBackupManifest -BackupDirectory $BackupDirectory -BackupRoot $backupFull -ExpectedSourceVersion $SourceVersion -ExpectedTargetVersion $TargetVersion -ExpectedDataRootLeaf (Split-Path -Leaf $dataFull)

    $operationId = [guid]::NewGuid().ToString('N')
    $pathId = $operationId.Substring(0, 12)
    $parent = Split-Path -Parent $dataFull
    $stagingDirectory = Assert-MeetilyNoReparsePathChain -Path (Join-Path $parent ('.rs-' + $pathId)) -Label 'restore staging directory'
    $originalDirectory = Assert-MeetilyNoReparsePathChain -Path (Join-Path $parent ('.ro-' + $pathId)) -Label 'restore original directory'
    $recoveryRoot = Assert-MeetilyNoReparsePathChain -Path (Join-Path $backupFull 'recovery') -Label 'recovery root'
    $recoveryDirectory = Assert-MeetilyNoReparsePathChain -Path (Join-Path $recoveryRoot ('r-' + $pathId)) -Label 'recovery directory'
    $failedRestoresRoot = Assert-MeetilyNoReparsePathChain -Path (Join-Path $backupFull 'failed-restores') -Label 'failed-artifacts root'
    Assert-MeetilyNoReparsePathChain -Path $parent -Label 'DataRoot parent' | Out-Null
    Assert-MeetilyNoReparsePathChain -Path $verified.payload_path -Label 'backup payload directory' | Out-Null
    Assert-MeetilyNoReparsePathChain -Path $failedRestoresRoot -Label 'failed-artifacts root' | Out-Null
    $restoreDataRoot = [bool]$verified.manifest.data_root_was_present
    $originalPathExisted = Test-MeetilyPathExists -Path $dataFull -Label 'DataRoot'
    $originalExisted = Test-MeetilyDirectoryExists -Path $dataFull -Label 'DataRoot'
    if ($originalPathExisted -and -not $originalExisted) { throw 'DataRoot exists but is not a directory.' }
    $originalManifest = @(if ($originalExisted) { Get-MeetilyDirectoryManifest $dataFull })
    $originalMoved = $false
    $replacementInstalled = $false
    $originalArchived = $false
    $failedArtifacts = @()

    try {
        if ((Test-MeetilyPathExists -Path $stagingDirectory -Label 'restore staging directory') -or
            (Test-MeetilyPathExists -Path $originalDirectory -Label 'restore original directory') -or
            (Test-MeetilyPathExists -Path $recoveryDirectory -Label 'recovery directory')) {
            throw 'Generated restore staging path already exists.'
        }
        New-MeetilyDirectory -Path $stagingDirectory -Label 'restore staging directory' | Out-Null
        Copy-MeetilyDirectoryContents -Source $verified.payload_path -Destination $stagingDirectory
        $stagedFiles = @(Get-MeetilyDirectoryManifest $stagingDirectory)
        if (-not (Test-MeetilyFileManifestsEqual -Left @($verified.manifest.files) -Right $stagedFiles)) {
            throw 'Restore staging payload failed its size or SHA-256 verification.'
        }
        if ($FailurePoint -eq 'BeforeOriginalMove') { throw 'Injected restore failure at BeforeOriginalMove.' }

        if ($originalExisted) {
            Move-MeetilyDirectory -Source $dataFull -Destination $originalDirectory -Label 'original data isolation' | Out-Null
            $originalMoved = $true
        }
        if ($FailurePoint -eq 'AfterOriginalMove') { throw 'Injected restore failure at AfterOriginalMove.' }

        if ($restoreDataRoot) {
            Move-MeetilyDirectory -Source $stagingDirectory -Destination $dataFull -Label 'restore replacement publish' | Out-Null
            $replacementInstalled = $true
        } else {
            Assert-MeetilyNoReparsePathChain -Path $stagingDirectory -Label 'empty restore staging directory' | Out-Null
            Assert-MeetilyNoReparsePoints $stagingDirectory
            if (@([System.IO.Directory]::EnumerateFileSystemEntries((ConvertTo-MeetilyIoPath $stagingDirectory))).Count -ne 0) {
                throw 'An absent DataRoot backup unexpectedly produced a non-empty restore staging directory.'
            }
            [System.IO.Directory]::Delete((ConvertTo-MeetilyIoPath $stagingDirectory), $false)
        }
        if ($FailurePoint -eq 'AfterReplacement') { throw 'Injected restore failure at AfterReplacement.' }

        $restoredPathExists = Test-MeetilyPathExists -Path $dataFull -Label 'restored DataRoot'
        $restoredDirectoryExists = Test-MeetilyDirectoryExists -Path $dataFull -Label 'restored DataRoot'
        if ($restoreDataRoot -and -not $restoredDirectoryExists) {
            throw 'The backup requires DataRoot to exist after restore, but no restored directory was published.'
        }
        if (-not $restoreDataRoot -and $restoredPathExists) {
            throw 'The backup requires DataRoot to remain absent after restore.'
        }
        $restoredFiles = @(if ($restoredDirectoryExists) { Get-MeetilyDirectoryManifest $dataFull })
        if ($FailurePoint -eq 'BeforeFinalVerification') { throw 'Injected restore failure at BeforeFinalVerification.' }
        if (-not (Test-MeetilyFileManifestsEqual -Left @($verified.manifest.files) -Right $restoredFiles)) {
            throw 'Restored DataRoot failed final size or SHA-256 verification.'
        }
        if ($originalMoved) {
            New-MeetilyDirectory -Path $recoveryRoot -Label 'recovery root' | Out-Null
            Move-MeetilyDirectory -Source $originalDirectory -Destination $recoveryDirectory -Label 'previous data archival' | Out-Null
            $originalArchived = $true
            $originalMoved = $false
        }
        return [ordered]@{
            action = 'restore'
            status = 'PASS'
            operation_id = $operationId
            restored_version = $SourceVersion
            replaced_version = $TargetVersion
            backup_directory = $verified.backup_directory
            manifest_path = $verified.manifest_path
            manifest_sha256 = [string](Get-MeetilyFileMetadata $verified.manifest_path).sha256
            data_root_was_present = $restoreDataRoot
            restored_data_root_present = $restoredPathExists
            restored_file_count = @($restoredFiles).Count
            restored_total_bytes = Get-MeetilyManifestTotalBytes $restoredFiles
            previous_data_recovery_directory = if ($originalArchived) { $recoveryDirectory } else { $null }
        }
    } catch {
        $primaryError = $_.Exception.Message
        $recoveryErrors = @()
        try {
            if ($replacementInstalled -and (Test-MeetilyPathExists -Path $dataFull -Label 'failed replacement')) {
                $failed = Move-MeetilyFailedArtifact -Path $dataFull -BackupRoot $backupFull -Category 'failed-restores'
                if ($null -ne $failed) { $failedArtifacts += $failed }
                $replacementInstalled = $false
            }
        } catch { $recoveryErrors += "Could not preserve failed replacement: $($_.Exception.Message)" }
        try {
            if ($originalMoved -and (Test-MeetilyPathExists -Path $originalDirectory -Label 'isolated original data')) {
                Move-MeetilyDirectory -Source $originalDirectory -Destination $dataFull -Label 'original data recovery' | Out-Null
                $originalMoved = $false
            } elseif ($originalArchived -and (Test-MeetilyPathExists -Path $recoveryDirectory -Label 'archived original data')) {
                Move-MeetilyDirectory -Source $recoveryDirectory -Destination $dataFull -Label 'archived data recovery' | Out-Null
                $originalArchived = $false
            }
        } catch { $recoveryErrors += "Could not restore original data: $($_.Exception.Message)" }
        try {
            if (Test-MeetilyPathExists -Path $stagingDirectory -Label 'restore staging directory') {
                $failed = Move-MeetilyFailedArtifact -Path $stagingDirectory -BackupRoot $backupFull -Category 'failed-restores'
                if ($null -ne $failed) { $failedArtifacts += $failed }
            }
        } catch { $recoveryErrors += "Could not preserve restore staging data: $($_.Exception.Message)" }

        try {
            $currentExists = Test-MeetilyDirectoryExists -Path $dataFull -Label 'DataRoot'
            if ($currentExists -ne $originalExisted) {
                $recoveryErrors += 'Original data presence was not recovered.'
            } elseif ($originalExisted) {
                $currentManifest = @(Get-MeetilyDirectoryManifest $dataFull)
                if (-not (Test-MeetilyFileManifestsEqual -Left $originalManifest -Right $currentManifest)) {
                    $recoveryErrors += 'Original data bytes were not recovered.'
                }
            }
        } catch { $recoveryErrors += "Could not verify recovered original data: $($_.Exception.Message)" }

        $recoveryStatus = if ($recoveryErrors.Count -eq 0) { 'original data recovered' } else { 'RECOVERY ERROR: ' + ($recoveryErrors -join '; ') }
        throw "Versioned restore failed ($primaryError); $recoveryStatus; failed artifacts: $($failedArtifacts -join ', ')"
    }
}

function Invoke-MeetilyVersionedDataCli {
    $started = Get-Date
    $effectiveDataRoot = $DataRoot
    $effectiveBackupRoot = $BackupRoot
    $effectiveSourceVersion = $SourceVersion
    $effectiveTargetVersion = $TargetVersion
    $effectiveResultPath = $ResultPath
    if ($Mode -eq 'Backup') {
        if ([string]::IsNullOrWhiteSpace($effectiveDataRoot)) {
            $effectiveDataRoot = [Environment]::GetEnvironmentVariable('MEETILY_INSTALLER_DATA_ROOT', 'Process')
        }
        if ([string]::IsNullOrWhiteSpace($effectiveSourceVersion)) {
            $effectiveSourceVersion = [Environment]::GetEnvironmentVariable('MEETILY_INSTALLER_SOURCE_VERSION', 'Process')
        }
        if ([string]::IsNullOrWhiteSpace($effectiveTargetVersion)) {
            $effectiveTargetVersion = [Environment]::GetEnvironmentVariable('MEETILY_INSTALLER_TARGET_VERSION', 'Process')
        }
        if ([string]::IsNullOrWhiteSpace($effectiveBackupRoot) -and -not [string]::IsNullOrWhiteSpace($effectiveDataRoot)) {
            $effectiveBackupRoot = $effectiveDataRoot + '.rollback-backups'
        }
        if ([string]::IsNullOrWhiteSpace($effectiveResultPath) -and -not [string]::IsNullOrWhiteSpace($effectiveBackupRoot)) {
            $effectiveResultPath = Join-Path $effectiveBackupRoot 'last-upgrade-backup-result.json'
        }
    }
    try {
        if (-not [string]::IsNullOrWhiteSpace($effectiveResultPath)) {
            Assert-MeetilyNoReparsePathChain -Path $effectiveResultPath -Label 'result path' | Out-Null
        }
        $result = switch ($Mode) {
            'Backup' {
                New-MeetilyVersionedBackup -DataRoot $effectiveDataRoot -BackupRoot $effectiveBackupRoot -SourceVersion $effectiveSourceVersion -TargetVersion $effectiveTargetVersion
            }
            'Verify' {
                Test-MeetilyVersionedBackup -DataRoot $effectiveDataRoot -BackupRoot $effectiveBackupRoot -BackupDirectory $BackupDirectory -SourceVersion $effectiveSourceVersion -TargetVersion $effectiveTargetVersion
            }
            'Restore' {
                Restore-MeetilyVersionedBackup -DataRoot $effectiveDataRoot -BackupRoot $effectiveBackupRoot -BackupDirectory $BackupDirectory -SourceVersion $effectiveSourceVersion -TargetVersion $effectiveTargetVersion -FailurePoint $FailurePoint
            }
        }
        $record = [ordered]@{
            schema_version = 1
            mode = $Mode
            started_at = $started.ToString('o')
            completed_at = (Get-Date).ToString('o')
            exit_code = 0
            status = 'PASS'
            result = $result
        }
        if (-not [string]::IsNullOrWhiteSpace($effectiveResultPath)) { Write-MeetilyJsonFile -Path $effectiveResultPath -Value $record }
        $record | ConvertTo-Json -Depth 30 -Compress | Write-Output
        return 0
    } catch {
        $record = [ordered]@{
            schema_version = 1
            mode = $Mode
            started_at = $started.ToString('o')
            completed_at = (Get-Date).ToString('o')
            exit_code = 1
            status = 'FAIL'
            error = $_.Exception.Message
        }
        if (-not [string]::IsNullOrWhiteSpace($effectiveResultPath)) {
            try { Write-MeetilyJsonFile -Path $effectiveResultPath -Value $record } catch { }
        }
        Write-Error $_.Exception.Message
        return 1
    }
}

if ($MyInvocation.InvocationName -ne '.') {
    exit (Invoke-MeetilyVersionedDataCli)
}

[CmdletBinding()]
param(
    [string]$CacheRoot = ""
)

$ErrorActionPreference = "Stop"
$workspace = Split-Path -Parent $PSScriptRoot
$lockPath = Join-Path $workspace "frontend\src-tauri\runtime\webview2-fixed.lock.json"
$linkPath = Join-Path $workspace "frontend\src-tauri\runtime\webview2-fixed"
$lock = Get-Content -LiteralPath $lockPath -Raw | ConvertFrom-Json

if ([string]::IsNullOrWhiteSpace($CacheRoot)) {
    if (-not [string]::IsNullOrWhiteSpace($env:MEETILY_WEBVIEW2_CACHE_ROOT)) {
        $CacheRoot = $env:MEETILY_WEBVIEW2_CACHE_ROOT
    } elseif (Test-Path -LiteralPath "D:\MeetilyData\build-deps") {
        $CacheRoot = "D:\MeetilyData\build-deps\webview2-fixed"
    } else {
        $CacheRoot = Join-Path $env:LOCALAPPDATA "MeetilyBuildTools\webview2-fixed"
    }
}

$versionRoot = Join-Path $CacheRoot ("{0}-{1}" -f $lock.version, $lock.architecture)
$archivePath = Join-Path $versionRoot $lock.archive_file
$extractRoot = Join-Path $versionRoot "runtime"
$runtimeRoot = Join-Path $extractRoot $lock.extracted_directory
$runtimeExecutable = Join-Path $runtimeRoot $lock.runtime_executable

function Get-PinnedFileSha256 {
    param([Parameter(Mandatory = $true)][string]$Path)
    $stream = [System.IO.File]::OpenRead($Path)
    $sha256 = [System.Security.Cryptography.SHA256]::Create()
    try {
        return ([System.BitConverter]::ToString($sha256.ComputeHash($stream))).Replace('-', '').ToLowerInvariant()
    } finally {
        $sha256.Dispose()
        $stream.Dispose()
    }
}

New-Item -ItemType Directory -Path $versionRoot -Force | Out-Null
if (Test-Path -LiteralPath $archivePath -PathType Leaf) {
    $archive = Get-Item -LiteralPath $archivePath
    $archiveHash = Get-PinnedFileSha256 -Path $archivePath
    if ($archive.Length -ne [int64]$lock.archive_bytes -or $archiveHash -ne $lock.archive_sha256) {
        throw "Cached WebView2 archive does not match the pinned lock."
    }
}

if (-not (Test-Path -LiteralPath $runtimeExecutable -PathType Leaf)) {
    if (-not (Test-Path -LiteralPath $archivePath -PathType Leaf)) {
        $curl = (Get-Command curl.exe -ErrorAction Stop).Source
        & $curl --location --fail --retry 3 --retry-delay 2 --output $archivePath $lock.download_url
        if ($LASTEXITCODE -ne 0) {
            throw "WebView2 Fixed Runtime download failed with exit code $LASTEXITCODE"
        }
        $archive = Get-Item -LiteralPath $archivePath
        $archiveHash = Get-PinnedFileSha256 -Path $archivePath
        if ($archive.Length -ne [int64]$lock.archive_bytes -or $archiveHash -ne $lock.archive_sha256) {
            throw "Downloaded WebView2 archive does not match the pinned lock."
        }
    }
    if (Test-Path -LiteralPath $extractRoot) {
        throw "Incomplete WebView2 extraction already exists: $extractRoot"
    }
    New-Item -ItemType Directory -Path $extractRoot | Out-Null
    & "$env:SystemRoot\System32\expand.exe" $archivePath -F:* $extractRoot
    if ($LASTEXITCODE -ne 0 -or -not (Test-Path -LiteralPath $runtimeExecutable -PathType Leaf)) {
        throw "WebView2 Fixed Runtime extraction failed."
    }
}

$runtimeRootFull = [System.IO.Path]::GetFullPath($runtimeRoot).TrimEnd("\") + "\"
foreach ($relativePath in @($lock.excluded_paths)) {
    if ([string]::IsNullOrWhiteSpace($relativePath) -or
        [System.IO.Path]::IsPathRooted($relativePath)) {
        throw "Invalid WebView2 excluded path in lock file: $relativePath"
    }
    $excludedPath = [System.IO.Path]::GetFullPath((Join-Path $runtimeRoot $relativePath))
    if (-not $excludedPath.StartsWith(
        $runtimeRootFull,
        [System.StringComparison]::OrdinalIgnoreCase
    )) {
        throw "WebView2 excluded path escapes the pinned runtime: $relativePath"
    }
    if (Test-Path -LiteralPath $excludedPath) {
        Remove-Item -LiteralPath $excludedPath -Recurse -Force
    }
}

if (Test-Path -LiteralPath $linkPath) {
    $existing = Get-Item -LiteralPath $linkPath -Force
    $target = @($existing.Target)[0]
    if ($existing.LinkType -ne "Junction" -or
        -not [string]::Equals(
            [System.IO.Path]::GetFullPath($target),
            [System.IO.Path]::GetFullPath($runtimeRoot),
            [System.StringComparison]::OrdinalIgnoreCase
        )) {
        throw "WebView2 runtime path exists but is not the pinned cache junction: $linkPath"
    }
} else {
    New-Item -ItemType Junction -Path $linkPath -Target $runtimeRoot | Out-Null
}

Write-Host "Prepared WebView2 Fixed Runtime $($lock.version) $($lock.architecture)."
Write-Host "Runtime: $runtimeRoot"

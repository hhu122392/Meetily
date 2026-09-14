[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)][string]$AppDataRoot,
    [Parameter(Mandatory = $true)][string]$LocalAppDataRoot,
    [Parameter(Mandatory = $true)][ValidatePattern('^[A-Za-z0-9](?:[A-Za-z0-9._-]{0,126}[A-Za-z0-9])?$')][string]$BundleId
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

function Get-NormalizedFullPath {
    param([Parameter(Mandatory = $true)][string]$Path)
    $full = [System.IO.Path]::GetFullPath($Path)
    $root = [System.IO.Path]::GetPathRoot($full)
    if ($full.Equals($root, [System.StringComparison]::OrdinalIgnoreCase)) { return $root }
    return $full.TrimEnd('\', '/')
}

function Assert-ExactDirectChild {
    param(
        [Parameter(Mandatory = $true)][string]$Root,
        [Parameter(Mandatory = $true)][string]$Target
    )
    $rootFull = Get-NormalizedFullPath -Path $Root
    $targetFull = Get-NormalizedFullPath -Path $Target
    $expected = Get-NormalizedFullPath -Path (Join-Path $rootFull $BundleId)
    if (-not $targetFull.Equals($expected, [System.StringComparison]::OrdinalIgnoreCase)) {
        throw "Delete target is not the exact bundle directory: $targetFull"
    }
    if ($targetFull.Equals($rootFull, [System.StringComparison]::OrdinalIgnoreCase)) {
        throw "Delete target resolves to its broad root: $targetFull"
    }
}

function Assert-NoReparseAncestor {
    param([Parameter(Mandatory = $true)][string]$Path)
    $cursor = Get-NormalizedFullPath -Path $Path
    $volumeRoot = Get-NormalizedFullPath -Path ([System.IO.Path]::GetPathRoot($cursor))
    while ($true) {
        if (Test-Path -LiteralPath $cursor) {
            $item = Get-Item -LiteralPath $cursor -Force
            if (($item.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0) {
                throw "Delete target ancestor is a reparse point: $cursor"
            }
        }
        if ($cursor.Equals($volumeRoot, [System.StringComparison]::OrdinalIgnoreCase)) { break }
        $parent = [System.IO.Path]::GetDirectoryName($cursor)
        if ([string]::IsNullOrWhiteSpace($parent) -or $parent.Equals($cursor, [System.StringComparison]::OrdinalIgnoreCase)) {
            throw "Could not validate delete target to its volume root: $Path"
        }
        $cursor = Get-NormalizedFullPath -Path $parent
    }
}

function Assert-NoReparseTree {
    param(
        [Parameter(Mandatory = $true)][string]$Path,
        [Parameter(Mandatory = $true)][string]$Label
    )
    $root = Get-NormalizedFullPath -Path $Path
    if (-not (Test-Path -LiteralPath $root)) { return }

    $pending = [System.Collections.Generic.Queue[string]]::new()
    $pending.Enqueue($root)
    while ($pending.Count -gt 0) {
        $entryPath = $pending.Dequeue()
        $entry = Get-Item -LiteralPath $entryPath -Force
        if (($entry.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0) {
            throw "$Label contains a junction, symbolic link, or other reparse point: $entryPath"
        }
        if (($entry.Attributes -band [System.IO.FileAttributes]::Directory) -eq 0) { continue }

        foreach ($child in [System.IO.Directory]::EnumerateFileSystemEntries($entry.FullName)) {
            $pending.Enqueue($child)
        }
    }
}

if ($BundleId -in @('.', '..')) { throw 'BundleId is not a safe directory name.' }

$appRoot = Get-NormalizedFullPath -Path $AppDataRoot
$localRoot = Get-NormalizedFullPath -Path $LocalAppDataRoot
$dataTarget = Get-NormalizedFullPath -Path (Join-Path $appRoot $BundleId)
$webViewTarget = Get-NormalizedFullPath -Path (Join-Path $localRoot $BundleId)

Assert-ExactDirectChild -Root $appRoot -Target $dataTarget
Assert-ExactDirectChild -Root $localRoot -Target $webViewTarget
Assert-NoReparseAncestor -Path $dataTarget
Assert-NoReparseAncestor -Path $webViewTarget
Assert-NoReparseTree -Path $dataTarget -Label 'Data delete target'
Assert-NoReparseTree -Path $webViewTarget -Label 'WebView delete target'

[ordered]@{
    status = 'PASS'
    data_target = $dataTarget
    webview_target = $webViewTarget
} | ConvertTo-Json -Compress

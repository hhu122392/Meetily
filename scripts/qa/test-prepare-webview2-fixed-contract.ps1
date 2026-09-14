[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)][string]$OutputRoot,
    [string]$RepositoryRoot = ''
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
if ([string]::IsNullOrWhiteSpace($RepositoryRoot)) {
    $RepositoryRoot = Join-Path $PSScriptRoot '..\..'
}
$RepositoryRoot = [System.IO.Path]::GetFullPath($RepositoryRoot)
$OutputRoot = [System.IO.Path]::GetFullPath($OutputRoot)
$sourcePath = Join-Path $RepositoryRoot 'scripts\prepare-webview2-fixed.ps1'

function Get-ContractFileSha256 {
    param([Parameter(Mandatory = $true)][string]$Path)
    $stream = [System.IO.File]::OpenRead($Path)
    $sha256 = [System.Security.Cryptography.SHA256]::Create()
    try {
        return ([System.BitConverter]::ToString($sha256.ComputeHash($stream))).Replace('-', '')
    } finally {
        $sha256.Dispose()
        $stream.Dispose()
    }
}

if (Test-Path -LiteralPath $OutputRoot) {
    if (@(Get-ChildItem -LiteralPath $OutputRoot -Force).Count -ne 0) {
        throw "OutputRoot must be empty: $OutputRoot"
    }
} else {
    [System.IO.Directory]::CreateDirectory($OutputRoot) | Out-Null
}

$tokens = $null
$parseErrors = $null
[void][System.Management.Automation.Language.Parser]::ParseFile(
    $sourcePath,
    [ref]$tokens,
    [ref]$parseErrors
)
$source = Get-Content -LiteralPath $sourcePath -Raw -Encoding UTF8
$checks = @(
    [ordered]@{
        name = 'powershell_parser_accepts_script'
        passed = @($parseErrors).Count -eq 0
    },
    [ordered]@{
        name = 'does_not_depend_on_get_file_hash_module_autoload'
        passed = $source -notmatch '(?i)\bGet-FileHash\b'
    },
    [ordered]@{
        name = 'uses_dotnet_sha256_over_a_file_stream'
        passed = $source.Contains('[System.IO.File]::OpenRead($Path)') -and
            $source.Contains('[System.Security.Cryptography.SHA256]::Create()') -and
            $source.Contains('$sha256.ComputeHash($stream)')
    },
    [ordered]@{
        name = 'disposes_hash_and_file_stream'
        passed = $source.Contains('$sha256.Dispose()') -and $source.Contains('$stream.Dispose()')
    },
    [ordered]@{
        name = 'checks_cached_and_downloaded_archive_with_same_function'
        passed = ([regex]::Matches($source, 'Get-PinnedFileSha256 -Path \$archivePath')).Count -eq 2
    }
)
$result = [ordered]@{
    schema_version = 1
    suite = 'prepare-webview2-fixed-contract'
    total = $checks.Count
    passed = @($checks | Where-Object { $_.passed }).Count
    failed = @($checks | Where-Object { -not $_.passed }).Count
    verdict = if (@($checks | Where-Object { -not $_.passed }).Count -eq 0) { 'PASS' } else { 'FAIL' }
    source = [ordered]@{
        relative_path = 'scripts/prepare-webview2-fixed.ps1'
        bytes = [int64](Get-Item -LiteralPath $sourcePath).Length
        sha256 = Get-ContractFileSha256 -Path $sourcePath
    }
    checks = $checks
}
$resultPath = Join-Path $OutputRoot 'prepare-webview2-fixed-contract.json'
[System.IO.File]::WriteAllText(
    $resultPath,
    (($result | ConvertTo-Json -Depth 8) + [Environment]::NewLine),
    [System.Text.UTF8Encoding]::new($false)
)
$result | ConvertTo-Json -Depth 8
if ($result.verdict -ne 'PASS') { exit 1 }

[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)][ValidatePattern('^[0-9a-fA-F]{40}$')][string]$SourceCommit,
    [Parameter(Mandatory = $true)][string]$OutputRoot,
    [string]$RepositoryRoot = '',
    [string]$ProductName = 'meetily-p6-lifecycle',
    [string]$BundleId = 'com.meetily.ai.p6lifecycle',
    [string]$Version = '0.4.2'
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'

if ([string]::IsNullOrWhiteSpace($RepositoryRoot)) {
    $RepositoryRoot = Join-Path $PSScriptRoot '..\..'
}
$RepositoryRoot = [System.IO.Path]::GetFullPath($RepositoryRoot)
$OutputRoot = [System.IO.Path]::GetFullPath($OutputRoot)
$frontendRoot = [System.IO.Path]::GetFullPath((Join-Path $RepositoryRoot 'frontend'))
$producerPath = [System.IO.Path]::GetFullPath($MyInvocation.MyCommand.Path)
$manifestGenerator = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot 'new-install-build-manifest.ps1'))
$rollbackTool = [System.IO.Path]::GetFullPath((Join-Path $RepositoryRoot 'frontend\src-tauri\scripts\meetily-versioned-data.ps1'))
$logPath = Join-Path $OutputRoot 'candidate-build.log'
$attestationPath = Join-Path $OutputRoot 'candidate-build-attestation.private.json'
$manifestPath = Join-Path $OutputRoot 'candidate-build-manifest.private.json'
$approvedCommands = @(
    'pnpm sidecars:prepare',
    'pnpm exec tauri build --config src-tauri/tauri.lifecycle.conf.json -- --features vulkan'
)

function Get-NormalizedFullPath {
    param([Parameter(Mandatory = $true)][string]$Path)
    $full = [System.IO.Path]::GetFullPath($Path)
    $root = [System.IO.Path]::GetPathRoot($full)
    if ($full.Equals($root, [System.StringComparison]::OrdinalIgnoreCase)) { return $root }
    return $full.TrimEnd('\', '/')
}

function Test-StrictChildPath {
    param([Parameter(Mandatory = $true)][string]$Candidate, [Parameter(Mandatory = $true)][string]$Parent)
    $candidateFull = Get-NormalizedFullPath -Path $Candidate
    $parentFull = Get-NormalizedFullPath -Path $Parent
    return $candidateFull.StartsWith($parentFull.TrimEnd('\', '/') + [System.IO.Path]::DirectorySeparatorChar, [System.StringComparison]::OrdinalIgnoreCase)
}

function Assert-NoReparseAncestors {
    param(
        [Parameter(Mandatory = $true)][string]$Path,
        [string]$AllowedReparsePoint = '',
        [string]$ExpectedReparseTarget = ''
    )
    $cursor = Get-NormalizedFullPath -Path $Path
    $volume = Get-NormalizedFullPath -Path ([System.IO.Path]::GetPathRoot($cursor))
    while ($true) {
        if (Test-Path -LiteralPath $cursor) {
            $item = Get-Item -LiteralPath $cursor -Force
            if (($item.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0) {
                if ([string]::IsNullOrWhiteSpace($AllowedReparsePoint) -or
                    [string]::IsNullOrWhiteSpace($ExpectedReparseTarget) -or
                    -not $cursor.Equals((Get-NormalizedFullPath -Path $AllowedReparsePoint), [System.StringComparison]::OrdinalIgnoreCase)) {
                    throw "Path contains a reparse point: $cursor"
                }
                $targets = @($item.Target)
                if ($item.LinkType -ne 'Junction' -or $targets.Count -ne 1 -or
                    -not [System.IO.Path]::IsPathRooted([string]$targets[0])) {
                    throw "Approved reparse point is not one absolute junction: $cursor"
                }
                $actualTarget = Get-NormalizedFullPath -Path ([string]$targets[0])
                $expectedTarget = Get-NormalizedFullPath -Path $ExpectedReparseTarget
                if (-not $actualTarget.Equals($expectedTarget, [System.StringComparison]::OrdinalIgnoreCase)) {
                    throw "Pinned reparse target mismatch: $cursor"
                }
                if (-not (Test-Path -LiteralPath $expectedTarget -PathType Container)) {
                    throw "Pinned reparse target is missing: $expectedTarget"
                }
                Assert-NoReparseAncestors -Path $expectedTarget
            }
        }
        if ($cursor.Equals($volume, [System.StringComparison]::OrdinalIgnoreCase)) { break }
        $parent = [System.IO.Path]::GetDirectoryName($cursor)
        if ([string]::IsNullOrWhiteSpace($parent) -or $parent.Equals($cursor, [System.StringComparison]::OrdinalIgnoreCase)) {
            throw "Could not validate path ancestors: $Path"
        }
        $cursor = Get-NormalizedFullPath -Path $parent
    }
}

function Get-FileRecord {
    param(
        [Parameter(Mandatory = $true)][string]$Path,
        [Parameter(Mandatory = $true)][string]$RelativePath,
        [string]$Role,
        [string]$AllowedReparsePoint = '',
        [string]$ExpectedReparseTarget = ''
    )
    if (-not (Test-Path -LiteralPath $Path -PathType Leaf)) { throw "Build artifact is missing: $Path" }
    Assert-NoReparseAncestors -Path $Path -AllowedReparsePoint $AllowedReparsePoint -ExpectedReparseTarget $ExpectedReparseTarget
    $before = Get-Item -LiteralPath $Path -Force
    $hash = (Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash.ToUpperInvariant()
    $after = Get-Item -LiteralPath $Path -Force
    if ([int64]$before.Length -le 0 -or [int64]$before.Length -ne [int64]$after.Length -or
        $before.LastWriteTimeUtc.Ticks -ne $after.LastWriteTimeUtc.Ticks) {
        throw "Build artifact is empty or changed while hashing: $Path"
    }
    $record = [ordered]@{
        relative_path = $RelativePath.Replace('\', '/')
        bytes = [int64]$after.Length
        sha256 = $hash
    }
    if (-not [string]::IsNullOrWhiteSpace($Role)) { $record['role'] = $Role }
    return $record
}

function Write-JsonExclusiveAtomic {
    param([Parameter(Mandatory = $true)]$Value, [Parameter(Mandatory = $true)][string]$Path)
    if (Test-Path -LiteralPath $Path) { throw "Refusing to overwrite build evidence: $Path" }
    $temporary = Join-Path (Split-Path -Parent $Path) ('.' + [System.IO.Path]::GetFileName($Path) + '.' + [Guid]::NewGuid().ToString('N') + '.tmp')
    try {
        [System.IO.File]::WriteAllText($temporary, (($Value | ConvertTo-Json -Depth 20) + [Environment]::NewLine), [System.Text.UTF8Encoding]::new($false))
        Move-Item -LiteralPath $temporary -Destination $Path
    } finally {
        if (Test-Path -LiteralPath $temporary -PathType Leaf) { Remove-Item -LiteralPath $temporary -Force }
    }
}

function Append-LogLine {
    param([Parameter(Mandatory = $true)][AllowEmptyString()][string]$Line)
    [System.IO.File]::AppendAllText($logPath, $Line + [Environment]::NewLine, [System.Text.UTF8Encoding]::new($false))
}

function Get-RepositoryChanges {
    param([Parameter(Mandatory = $true)][string]$Root)

    $staged = @(& git -C $Root diff --cached --name-only --no-ext-diff --no-textconv HEAD --)
    if ($LASTEXITCODE -ne 0) { throw 'Could not compare the staged repository content with HEAD.' }
    $unstaged = @(& git -C $Root diff --name-only --no-ext-diff --no-textconv --)
    if ($LASTEXITCODE -ne 0) { throw 'Could not compare the worktree content with the index.' }
    $untracked = @(& git -C $Root ls-files --others --exclude-standard)
    if ($LASTEXITCODE -ne 0) { throw 'Could not list untracked repository files.' }

    return @(
        @($staged) + @($unstaged) + @($untracked) |
            Where-Object { -not [string]::IsNullOrWhiteSpace([string]$_) } |
            Sort-Object -Unique
    )
}

function Invoke-PnpmBuildStep {
    param([Parameter(Mandatory = $true)][string[]]$Arguments, [Parameter(Mandatory = $true)][string]$DisplayCommand)
    Append-LogLine -Line ("COMMAND " + $DisplayCommand)
    $global:LASTEXITCODE = 0
    $previousErrorActionPreference = $ErrorActionPreference
    try {
        # Windows PowerShell 5.1 wraps a native program's stderr as a non-terminating
        # error record.  The wrapper must capture that stream and judge success only
        # from the native exit code; otherwise normal pnpm progress can abort a build.
        $ErrorActionPreference = 'Continue'
        & $pnpmPath @Arguments 2>&1 | ForEach-Object {
            $line = [string]$_
            Write-Host $line
            Append-LogLine -Line $line
        }
        $exitCode = [int]$LASTEXITCODE
    } finally {
        $ErrorActionPreference = $previousErrorActionPreference
    }
    Append-LogLine -Line ("EXIT_CODE " + $exitCode)
    if ($exitCode -ne 0) { throw "Build command failed with exit code $exitCode`: $DisplayCommand" }
}

if (-not (Test-Path -LiteralPath $RepositoryRoot -PathType Container)) { throw "Repository root is missing: $RepositoryRoot" }
if (-not (Test-Path -LiteralPath $frontendRoot -PathType Container)) { throw "Frontend root is missing: $frontendRoot" }
if (-not (Test-StrictChildPath -Candidate $frontendRoot -Parent $RepositoryRoot)) { throw 'Frontend root escapes the repository.' }
$webViewLockPath = Join-Path $RepositoryRoot 'frontend\src-tauri\runtime\webview2-fixed.lock.json'
if (-not (Test-Path -LiteralPath $webViewLockPath -PathType Leaf)) { throw "Pinned WebView2 lock is missing: $webViewLockPath" }
$webViewLock = Get-Content -LiteralPath $webViewLockPath -Raw | ConvertFrom-Json
$webViewCacheRoot = if (Test-Path -LiteralPath 'D:\MeetilyData\build-deps' -PathType Container) {
    'D:\MeetilyData\build-deps\webview2-fixed'
} else {
    Join-Path $env:LOCALAPPDATA 'MeetilyBuildTools\webview2-fixed'
}
$webViewCacheRoot = Get-NormalizedFullPath -Path $webViewCacheRoot
$webViewVersionRoot = Join-Path $webViewCacheRoot ("{0}-{1}" -f $webViewLock.version, $webViewLock.architecture)
$webViewRuntimeRoot = [System.IO.Path]::GetFullPath((Join-Path $webViewVersionRoot (Join-Path 'runtime' ([string]$webViewLock.extracted_directory))))
$webViewLinkPath = [System.IO.Path]::GetFullPath((Join-Path $RepositoryRoot 'frontend\src-tauri\runtime\webview2-fixed'))
if (-not (Test-StrictChildPath -Candidate $webViewRuntimeRoot -Parent $webViewCacheRoot)) { throw 'Pinned WebView2 runtime escapes its cache root.' }
Assert-NoReparseAncestors -Path $webViewCacheRoot
if ((Get-NormalizedFullPath -Path $OutputRoot).Equals((Get-NormalizedFullPath -Path $RepositoryRoot), [System.StringComparison]::OrdinalIgnoreCase) -or
    (Test-StrictChildPath -Candidate $OutputRoot -Parent $RepositoryRoot) -or
    (Test-StrictChildPath -Candidate $RepositoryRoot -Parent $OutputRoot)) {
    throw 'Build evidence root must be outside and non-overlapping with the repository.'
}
Assert-NoReparseAncestors -Path $OutputRoot
if (Test-Path -LiteralPath $OutputRoot) {
    if (@(Get-ChildItem -LiteralPath $OutputRoot -Force).Count -ne 0) { throw "Build evidence root must be new or empty: $OutputRoot" }
} else {
    [System.IO.Directory]::CreateDirectory($OutputRoot) | Out-Null
}

$headBefore = (& git -C $RepositoryRoot rev-parse HEAD).Trim().ToLowerInvariant()
$statusBefore = @(Get-RepositoryChanges -Root $RepositoryRoot)
if ($LASTEXITCODE -ne 0 -or $headBefore -ne $SourceCommit.ToLowerInvariant()) { throw 'B-01 source commit does not match repository HEAD.' }
if ($statusBefore.Count -ne 0) { throw 'B-01 requires a completely clean worktree before building.' }

$pnpmPath = [System.IO.Path]::GetFullPath((Get-Command pnpm.cmd -ErrorAction Stop).Source)
$oldLibclang = $env:LIBCLANG_PATH
$oldVulkan = $env:VULKAN_SDK
$oldGenerator = $env:CMAKE_GENERATOR
$oldWebViewCacheRoot = $env:MEETILY_WEBVIEW2_CACHE_ROOT
$startedAt = (Get-Date).ToUniversalTime()
[System.IO.File]::WriteAllText($logPath, "SOURCE_COMMIT $($SourceCommit.ToLowerInvariant())$([Environment]::NewLine)", [System.Text.UTF8Encoding]::new($false))
try {
    $env:LIBCLANG_PATH = 'D:\MeetilyBuildTools\clang+llvm-19.1.5-x86_64-pc-windows-msvc\bin'
    $env:VULKAN_SDK = 'D:\VulkanSDK\1.4.357.0'
    $env:CMAKE_GENERATOR = 'NMake Makefiles'
    $env:MEETILY_WEBVIEW2_CACHE_ROOT = $webViewCacheRoot
    Push-Location $frontendRoot
    try {
        Invoke-PnpmBuildStep -Arguments @('sidecars:prepare') -DisplayCommand $approvedCommands[0]
        Invoke-PnpmBuildStep -Arguments @('exec', 'tauri', 'build', '--config', 'src-tauri/tauri.lifecycle.conf.json', '--', '--features', 'vulkan') -DisplayCommand $approvedCommands[1]
    } finally {
        Pop-Location
    }
} finally {
    $env:LIBCLANG_PATH = $oldLibclang
    $env:VULKAN_SDK = $oldVulkan
    $env:CMAKE_GENERATOR = $oldGenerator
    $env:MEETILY_WEBVIEW2_CACHE_ROOT = $oldWebViewCacheRoot
}
$completedAt = (Get-Date).ToUniversalTime()

$headAfter = (& git -C $RepositoryRoot rev-parse HEAD).Trim().ToLowerInvariant()
$statusAfter = @(Get-RepositoryChanges -Root $RepositoryRoot)
if ($LASTEXITCODE -ne 0 -or $headAfter -ne $SourceCommit.ToLowerInvariant()) { throw 'Repository HEAD changed during B-01.' }
if ($statusAfter.Count -ne 0) { throw 'Tracked or untracked source files changed during B-01.' }

$installerPath = [System.IO.Path]::GetFullPath((Join-Path $RepositoryRoot ("target\release\bundle\nsis\$ProductName" + '_' + $Version + '_x64-setup.exe')))
$artifactSpecs = @(
    [ordered]@{ role = 'main_executable'; relative_path = 'target/release/meetily.exe' },
    [ordered]@{ role = 'llama_helper'; relative_path = 'frontend/src-tauri/binaries/llama-helper-x86_64-pc-windows-msvc.exe' },
    [ordered]@{ role = 'moss_helper'; relative_path = 'frontend/src-tauri/binaries/moss-helper-x86_64-pc-windows-msvc.exe' },
    [ordered]@{ role = 'ffmpeg'; relative_path = 'frontend/src-tauri/binaries/ffmpeg-x86_64-pc-windows-msvc.exe' },
    [ordered]@{ role = 'directml'; relative_path = 'frontend/src-tauri/runtime/windows-x64/nsis/DirectML.dll' },
    [ordered]@{ role = 'webview2'; relative_path = 'frontend/src-tauri/runtime/webview2-fixed/msedgewebview2.exe' }
)
$artifacts = @(
    foreach ($spec in $artifactSpecs) {
        $path = [System.IO.Path]::GetFullPath((Join-Path $RepositoryRoot ([string]$spec.relative_path)))
        if ([string]$spec.role -eq 'webview2') {
            Get-FileRecord -Path $path -RelativePath ([string]$spec.relative_path) -Role ([string]$spec.role) `
                -AllowedReparsePoint $webViewLinkPath -ExpectedReparseTarget $webViewRuntimeRoot
        } else {
            Get-FileRecord -Path $path -RelativePath ([string]$spec.relative_path) -Role ([string]$spec.role)
        }
    }
    $installerRelative = $installerPath.Substring($RepositoryRoot.TrimEnd('\').Length + 1).Replace('\', '/')
    Get-FileRecord -Path $installerPath -RelativePath $installerRelative -Role 'nsis_installer'
)

$producer = Get-FileRecord -Path $producerPath -RelativePath 'scripts/qa/build-moss-functional-candidate.ps1'
$buildLog = Get-FileRecord -Path $logPath -RelativePath 'candidate-build.log'
$buildLog['path'] = $logPath
$attestation = [ordered]@{
    schema_version = 1
    stage = 'MOSS_FUNCTIONAL_CANDIDATE_BUILD'
    status = 'PASS'
    source_commit = $SourceCommit.ToLowerInvariant()
    repository_head_before = $headBefore
    repository_head_after = $headAfter
    worktree_clean_before = $true
    worktree_clean_after = $true
    product_name = $ProductName
    bundle_id = $BundleId
    version = $Version
    started_at = $startedAt.ToString('o')
    completed_at = $completedAt.ToString('o')
    commands = $approvedCommands
    executable = $pnpmPath
    environment = [ordered]@{
        LIBCLANG_PATH = 'D:\MeetilyBuildTools\clang+llvm-19.1.5-x86_64-pc-windows-msvc\bin'
        VULKAN_SDK = 'D:\VulkanSDK\1.4.357.0'
        CMAKE_GENERATOR = 'NMake Makefiles'
        MEETILY_WEBVIEW2_CACHE_ROOT = $webViewCacheRoot
    }
    producer = $producer
    build_log = $buildLog
    artifacts = $artifacts
}
Write-JsonExclusiveAtomic -Value $attestation -Path $attestationPath

& $manifestGenerator -Role candidate -Installer $installerPath -SourceCommit $SourceCommit -ExpectedVersion $Version `
    -Output $manifestPath -ProductName $ProductName -BundleId $BundleId -RepositoryRoot $RepositoryRoot `
    -RollbackTool $rollbackTool -BuildAttestation $attestationPath
if ($LASTEXITCODE -ne 0 -or -not (Test-Path -LiteralPath $manifestPath -PathType Leaf)) {
    throw 'Controlled candidate build completed, but the installed-file manifest did not pass.'
}

[ordered]@{
    status = 'PASS'
    source_commit = $SourceCommit.ToLowerInvariant()
    installer = Get-FileRecord -Path $installerPath -RelativePath ([System.IO.Path]::GetFileName($installerPath))
    build_attestation = Get-FileRecord -Path $attestationPath -RelativePath ([System.IO.Path]::GetFileName($attestationPath))
    build_manifest = Get-FileRecord -Path $manifestPath -RelativePath ([System.IO.Path]::GetFileName($manifestPath))
} | ConvertTo-Json -Depth 8

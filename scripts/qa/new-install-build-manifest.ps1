[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)][ValidateSet('candidate', 'baseline')][string]$Role,
    [Parameter(Mandatory = $true)][string]$Installer,
    [Parameter(Mandatory = $true)][ValidatePattern('^[0-9a-fA-F]{40}$')][string]$SourceCommit,
    [Parameter(Mandatory = $true)][string]$ExpectedVersion,
    [Parameter(Mandatory = $true)][string]$Output,
    [ValidatePattern('^[A-Za-z0-9](?:[A-Za-z0-9._-]{0,126}[A-Za-z0-9])?$')][string]$ProductName = 'meetily-p6-lifecycle',
    [ValidatePattern('^[A-Za-z0-9](?:[A-Za-z0-9._-]{0,126}[A-Za-z0-9])?$')][string]$BundleId = 'com.meetily.ai.p6lifecycle',
    [string]$RepositoryRoot = '',
    [string]$RollbackTool = '',
    [string]$BuildAttestation,
    [ValidateRange(30, 1200)][int]$TimeoutSeconds = 1200
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'

$approvedBaselineCommit = '7392eae159443822c80d3675ca9af388e94b2d71'
$Installer = [System.IO.Path]::GetFullPath($Installer)
$Output = [System.IO.Path]::GetFullPath($Output)
if ([string]::IsNullOrWhiteSpace($RepositoryRoot)) {
    $RepositoryRoot = Join-Path $PSScriptRoot '..\..'
}
$RepositoryRoot = [System.IO.Path]::GetFullPath($RepositoryRoot)
if ([string]::IsNullOrWhiteSpace($RollbackTool)) {
    $RollbackTool = Join-Path $RepositoryRoot 'frontend\src-tauri\scripts\meetily-versioned-data.ps1'
}
$RollbackTool = [System.IO.Path]::GetFullPath($RollbackTool)
$BuildAttestation = if ([string]::IsNullOrWhiteSpace($BuildAttestation)) { '' } else { [System.IO.Path]::GetFullPath($BuildAttestation) }
$installRoot = [System.IO.Path]::GetFullPath((Join-Path $env:LOCALAPPDATA $ProductName))
$dataRoot = [System.IO.Path]::GetFullPath((Join-Path $env:APPDATA $BundleId))
$backupRoot = $dataRoot + '.rollback-backups'
$webViewRoot = [System.IO.Path]::GetFullPath((Join-Path $env:LOCALAPPDATA $BundleId))
$registryPath = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\' + $ProductName

. (Join-Path $PSScriptRoot 'windows-native-arguments.ps1')

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
    $prefix = $parentFull.TrimEnd('\', '/') + [System.IO.Path]::DirectorySeparatorChar
    return $candidateFull.StartsWith($prefix, [System.StringComparison]::OrdinalIgnoreCase)
}

function Assert-NoReparsePath {
    param(
        [Parameter(Mandatory = $true)][string]$Path,
        [Parameter(Mandatory = $true)][string]$Boundary,
        [switch]$AllowMissing
    )
    $full = Get-NormalizedFullPath -Path $Path
    $root = Get-NormalizedFullPath -Path $Boundary
    if (-not $full.Equals($root, [System.StringComparison]::OrdinalIgnoreCase) -and
        -not (Test-StrictChildPath -Candidate $full -Parent $root)) {
        throw "Path is outside its approved boundary: $full"
    }
    $cursor = $full
    while ($true) {
        if (Test-Path -LiteralPath $cursor) {
            $item = Get-Item -LiteralPath $cursor -Force
            if (($item.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0) {
                throw "Path contains a reparse point: $cursor"
            }
        } elseif (-not $AllowMissing) {
            throw "Path component does not exist: $cursor"
        }
        if ($cursor.Equals($root, [System.StringComparison]::OrdinalIgnoreCase)) { break }
        $parent = [System.IO.Path]::GetDirectoryName($cursor)
        if ([string]::IsNullOrWhiteSpace($parent) -or $parent.Equals($cursor, [System.StringComparison]::OrdinalIgnoreCase)) {
            throw "Could not reach approved boundary: $root"
        }
        $cursor = Get-NormalizedFullPath -Path $parent
    }
}

function Test-PathsOverlap {
    param([Parameter(Mandatory = $true)][string]$Left, [Parameter(Mandatory = $true)][string]$Right)
    $leftFull = Get-NormalizedFullPath -Path $Left
    $rightFull = Get-NormalizedFullPath -Path $Right
    return $leftFull.Equals($rightFull, [System.StringComparison]::OrdinalIgnoreCase) -or
        (Test-StrictChildPath -Candidate $leftFull -Parent $rightFull) -or
        (Test-StrictChildPath -Candidate $rightFull -Parent $leftFull)
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

function Get-InstalledRolePaths {
    param(
        [Parameter(Mandatory = $true)][ValidateSet('candidate', 'baseline')][string]$ManifestRole,
        [Parameter(Mandatory = $true)][string]$ManifestProductName
    )
    return [ordered]@{
        main_executable = if ($ManifestRole -eq 'baseline') { "$ManifestProductName.exe" } else { 'meetily.exe' }
        llama_helper = 'llama-helper.exe'
        moss_helper = 'moss-helper.exe'
        ffmpeg = 'ffmpeg.exe'
        directml = 'DirectML.dll'
        webview2 = 'runtime/webview2-fixed/msedgewebview2.exe'
        uninstaller = 'uninstall.exe'
    }
}

function Get-FileEvidence {
    param(
        [Parameter(Mandatory = $true)][string]$Path,
        [Parameter(Mandatory = $true)][string]$RelativePath
    )
    if (-not (Test-Path -LiteralPath $Path -PathType Leaf)) { throw "Required file is missing: $Path" }
    $before = Get-Item -LiteralPath $Path -Force
    if (($before.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0) { throw "Required file is a reparse point: $Path" }
    $hash = (Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash.ToUpperInvariant()
    $after = Get-Item -LiteralPath $Path -Force
    if ([int64]$before.Length -ne [int64]$after.Length -or $before.LastWriteTimeUtc.Ticks -ne $after.LastWriteTimeUtc.Ticks) {
        throw "File changed while it was hashed: $Path"
    }
    return [ordered]@{
        relative_path = $RelativePath.Replace('\', '/')
        bytes = [int64]$after.Length
        sha256 = $hash
    }
}

function Assert-FileRecordMatches {
    param(
        [Parameter(Mandatory = $true)]$Record,
        [Parameter(Mandatory = $true)][string]$Path,
        [Parameter(Mandatory = $true)][string]$Label
    )
    $actual = Get-FileEvidence -Path $Path -RelativePath ([string]$Record.relative_path)
    if ([int64]$Record.bytes -ne [int64]$actual.bytes -or
        -not ([string]$Record.sha256).Equals([string]$actual.sha256, [System.StringComparison]::OrdinalIgnoreCase)) {
        throw "$Label bytes or SHA-256 do not match the real file."
    }
    return $actual
}

function Read-CandidateBuildAttestation {
    param([string]$Path)
    if ([string]::IsNullOrWhiteSpace($Path) -or -not (Test-Path -LiteralPath $Path -PathType Leaf)) {
        throw 'Candidate role requires the attestation produced by the controlled B-01 build wrapper.'
    }
    Assert-NoReparsePath -Path $Path -Boundary ([System.IO.Path]::GetPathRoot($Path))
    $document = Get-Content -LiteralPath $Path -Raw -Encoding UTF8 | ConvertFrom-Json
    if ([int]$document.schema_version -ne 1 -or [string]$document.stage -ne 'MOSS_FUNCTIONAL_CANDIDATE_BUILD' -or
        [string]$document.status -ne 'PASS') {
        throw 'Candidate build attestation has an unexpected schema, stage, or status.'
    }
    if (-not ([string]$document.source_commit).Equals($SourceCommit, [System.StringComparison]::OrdinalIgnoreCase) -or
        -not ([string]$document.repository_head_before).Equals($SourceCommit, [System.StringComparison]::OrdinalIgnoreCase) -or
        -not ([string]$document.repository_head_after).Equals($SourceCommit, [System.StringComparison]::OrdinalIgnoreCase) -or
        -not [bool]$document.worktree_clean_before -or -not [bool]$document.worktree_clean_after) {
        throw 'Candidate build attestation is not bound to one clean source commit before and after the build.'
    }
    if ([string]$document.product_name -ne $ProductName -or [string]$document.bundle_id -ne $BundleId -or
        [string]$document.version -ne $ExpectedVersion) {
        throw 'Candidate build attestation product identity does not match this manifest request.'
    }
    $expectedCommands = @(
        'pnpm sidecars:prepare',
        'pnpm exec tauri build --config src-tauri/tauri.lifecycle.conf.json -- --features vulkan'
    )
    $actualCommands = @($document.commands | ForEach-Object { [string]$_ })
    if ($actualCommands.Count -ne $expectedCommands.Count -or (Compare-Object -ReferenceObject $expectedCommands -DifferenceObject $actualCommands -SyncWindow 0)) {
        throw 'Candidate build attestation does not contain the exact approved build command sequence.'
    }
    if ([string]$document.environment.LIBCLANG_PATH -ne 'D:\MeetilyBuildTools\clang+llvm-19.1.5-x86_64-pc-windows-msvc\bin' -or
        [string]$document.environment.VULKAN_SDK -ne 'D:\VulkanSDK\1.4.357.0' -or
        [string]$document.environment.CMAKE_GENERATOR -ne 'NMake Makefiles') {
        throw 'Candidate build attestation does not contain the approved build environment.'
    }
    $producerPath = [System.IO.Path]::GetFullPath((Join-Path $RepositoryRoot 'scripts\qa\build-moss-functional-candidate.ps1'))
    if ([string]$document.producer.relative_path -ne 'scripts/qa/build-moss-functional-candidate.ps1') {
        throw 'Candidate build attestation names an unexpected producer.'
    }
    [void](Assert-FileRecordMatches -Record $document.producer -Path $producerPath -Label 'Build-attestation producer')
    $logPath = [System.IO.Path]::GetFullPath([string]$document.build_log.path)
    [void](Assert-FileRecordMatches -Record $document.build_log -Path $logPath -Label 'Build log')

    $expectedRolePaths = [ordered]@{
        main_executable = 'target/release/meetily.exe'
        llama_helper = 'frontend/src-tauri/binaries/llama-helper-x86_64-pc-windows-msvc.exe'
        moss_helper = 'frontend/src-tauri/binaries/moss-helper-x86_64-pc-windows-msvc.exe'
        ffmpeg = 'frontend/src-tauri/binaries/ffmpeg-x86_64-pc-windows-msvc.exe'
        directml = 'frontend/src-tauri/runtime/windows-x64/nsis/DirectML.dll'
        webview2 = 'frontend/src-tauri/runtime/webview2-fixed/msedgewebview2.exe'
    }
    $artifactByRole = @{}
    foreach ($entry in @($document.artifacts)) {
        $role = [string]$entry.role
        if ($artifactByRole.ContainsKey($role)) { throw "Candidate build attestation has duplicate artifact role: $role" }
        $artifactByRole[$role] = $entry
    }
    foreach ($role in $expectedRolePaths.Keys) {
        if (-not $artifactByRole.ContainsKey($role)) { throw "Candidate build attestation is missing artifact role: $role" }
        $entry = $artifactByRole[$role]
        if ([string]$entry.relative_path -ne [string]$expectedRolePaths[$role]) { throw "Candidate build attestation has an unexpected path for role: $role" }
        $artifactPath = [System.IO.Path]::GetFullPath((Join-Path $RepositoryRoot ([string]$entry.relative_path)))
        if (-not (Test-StrictChildPath -Candidate $artifactPath -Parent $RepositoryRoot)) { throw "Candidate build artifact escapes the repository: $role" }
        [void](Assert-FileRecordMatches -Record $entry -Path $artifactPath -Label "Candidate build artifact $role")
    }
    if (-not $artifactByRole.ContainsKey('nsis_installer')) { throw 'Candidate build attestation is missing the NSIS installer role.' }
    $installerRecord = $artifactByRole['nsis_installer']
    [void](Assert-FileRecordMatches -Record $installerRecord -Path $Installer -Label 'Candidate NSIS installer')
    if ($artifactByRole.Count -ne ($expectedRolePaths.Count + 1)) { throw 'Candidate build attestation contains an unapproved artifact role.' }
    $attestationEvidence = Get-FileEvidence -Path $Path -RelativePath 'candidate-build-attestation.private.json'
    $attestationEvidence['path'] = $Path
    $logEvidence = Get-FileEvidence -Path $logPath -RelativePath 'candidate-build.log'
    $logEvidence['path'] = $logPath
    return [ordered]@{
        evidence = $attestationEvidence
        build_log = $logEvidence
        commands = $actualCommands
        artifact_count = $artifactByRole.Count
    }
}

function Invoke-NativeProcess {
    param(
        [Parameter(Mandatory = $true)][string]$Executable,
        [string[]]$Arguments = @(),
        [Parameter(Mandatory = $true)][string]$Label
    )
    $start = [System.Diagnostics.ProcessStartInfo]::new()
    $start.FileName = $Executable
    $start.Arguments = (($Arguments | ForEach-Object { ConvertTo-NativeArgument -Value ([string]$_) }) -join ' ')
    $start.UseShellExecute = $false
    $start.CreateNoWindow = $true
    $process = [System.Diagnostics.Process]::new()
    $process.StartInfo = $start
    $startedAt = (Get-Date).ToUniversalTime()
    if (-not $process.Start()) { throw "$Label did not start." }
    $processId = $process.Id
    if (-not $process.WaitForExit($TimeoutSeconds * 1000)) {
        $knownIds = New-Object 'System.Collections.Generic.HashSet[int]'
        [void]$knownIds.Add([int]$processId)
        $emptyScans = 0
        $taskkillErrors = @()
        for ($attempt = 0; $attempt -lt 20 -and $emptyScans -lt 2; $attempt++) {
            $snapshot = @(Get-CimInstance Win32_Process -ErrorAction SilentlyContinue | Select-Object ProcessId, ParentProcessId)
            $expanded = $true
            while ($expanded) {
                $expanded = $false
                foreach ($entry in $snapshot) {
                    if ($knownIds.Contains([int]$entry.ParentProcessId) -and $knownIds.Add([int]$entry.ProcessId)) { $expanded = $true }
                }
            }
            $alive = @($knownIds | Where-Object { $null -ne (Get-Process -Id $_ -ErrorAction SilentlyContinue) })
            if ($alive.Count -eq 0) {
                $emptyScans++
            } else {
                $emptyScans = 0
                foreach ($id in $alive) {
                    $killOutput = @(& taskkill.exe /PID $id /T /F 2>&1)
                    if ($LASTEXITCODE -ne 0 -and $null -ne (Get-Process -Id $id -ErrorAction SilentlyContinue)) {
                        $taskkillErrors += "PID $id`: $($killOutput -join ' ')"
                    }
                }
            }
            Start-Sleep -Milliseconds 250
        }
        try { $process.WaitForExit(10000) | Out-Null } catch {}
        $remaining = @($knownIds | Where-Object { $null -ne (Get-Process -Id $_ -ErrorAction SilentlyContinue) })
        if ($remaining.Count -ne 0 -or $emptyScans -lt 2) {
            throw "$Label timed out and its exact process tree was not removed: $($remaining -join ', '); $($taskkillErrors -join '; ')"
        }
        throw "$Label timed out after $TimeoutSeconds seconds; its exact process tree was verified absent in two consecutive scans."
    }
    return [ordered]@{
        label = $Label
        pid = $processId
        started_at = $startedAt.ToString('o')
        ended_at = (Get-Date).ToUniversalTime().ToString('o')
        exit_code = [int]$process.ExitCode
    }
}

function Get-RegistryState {
    if (-not (Test-Path -LiteralPath $registryPath)) { return $null }
    $record = Get-ItemProperty -LiteralPath $registryPath -ErrorAction Stop
    return [ordered]@{
        display_name = [string]$record.DisplayName
        display_version = [string]$record.DisplayVersion
        display_icon = ([string]$record.DisplayIcon).Trim().Trim('"')
        install_location = ([string]$record.InstallLocation).Trim().Trim('"')
        uninstall_string = ([string]$record.UninstallString).Trim().Trim('"')
    }
}

function Write-AtomicJson {
    param([Parameter(Mandatory = $true)]$Value, [Parameter(Mandatory = $true)][string]$Path)
    if (Test-Path -LiteralPath $Path) { throw "Refusing to overwrite an existing manifest: $Path" }
    $parent = Split-Path -Parent $Path
    if (-not (Test-Path -LiteralPath $parent -PathType Container)) {
        [System.IO.Directory]::CreateDirectory($parent) | Out-Null
    }
    Assert-NoReparsePath -Path $parent -Boundary ([System.IO.Path]::GetPathRoot($parent))
    $temporary = Join-Path $parent ('.' + [System.IO.Path]::GetFileName($Path) + '.' + [Guid]::NewGuid().ToString('N') + '.tmp')
    try {
        [System.IO.File]::WriteAllText(
            $temporary,
            (($Value | ConvertTo-Json -Depth 20) + [Environment]::NewLine),
            [System.Text.UTF8Encoding]::new($false)
        )
        Move-Item -LiteralPath $temporary -Destination $Path
    } finally {
        if (Test-Path -LiteralPath $temporary -PathType Leaf) { Remove-Item -LiteralPath $temporary -Force }
    }
}

if (-not (Test-Path -LiteralPath $Installer -PathType Leaf)) { throw "Installer is missing: $Installer" }
if ($ProductName -in @('.', '..') -or $BundleId -in @('.', '..')) { throw 'ProductName and BundleId must be safe single names.' }
if (-not (Test-StrictChildPath -Candidate $installRoot -Parent $env:LOCALAPPDATA)) { throw 'Install root is not isolated below LOCALAPPDATA.' }
if (-not (Test-StrictChildPath -Candidate $dataRoot -Parent $env:APPDATA)) { throw 'Data root is not isolated below APPDATA.' }
if (-not (Test-StrictChildPath -Candidate $backupRoot -Parent $env:APPDATA)) { throw 'Backup root is not isolated below APPDATA.' }
if (-not (Test-StrictChildPath -Candidate $webViewRoot -Parent $env:LOCALAPPDATA)) { throw 'WebView root is not isolated below LOCALAPPDATA.' }
Assert-NoReparsePath -Path $Installer -Boundary ([System.IO.Path]::GetPathRoot($Installer))
Assert-NoReparsePath -Path $RepositoryRoot -Boundary ([System.IO.Path]::GetPathRoot($RepositoryRoot))
Assert-NoReparsePath -Path $installRoot -Boundary $env:LOCALAPPDATA -AllowMissing
Assert-NoReparsePath -Path $dataRoot -Boundary $env:APPDATA -AllowMissing
Assert-NoReparsePath -Path $backupRoot -Boundary $env:APPDATA -AllowMissing
Assert-NoReparsePath -Path $webViewRoot -Boundary $env:LOCALAPPDATA -AllowMissing
Assert-NoReparsePath -Path $Output -Boundary ([System.IO.Path]::GetPathRoot($Output)) -AllowMissing
foreach ($isolatedRoot in @($installRoot, $dataRoot, $backupRoot, $webViewRoot)) {
    if (Test-PathsOverlap -Left $Output -Right $isolatedRoot) { throw 'Output must not overlap an isolated install, data, backup, or WebView root.' }
}

$candidateBuildProvenance = $null
if ($Role -eq 'baseline') {
    if (-not $SourceCommit.Equals($approvedBaselineCommit, [System.StringComparison]::OrdinalIgnoreCase)) {
        throw "Baseline source commit must be $approvedBaselineCommit."
    }
    $baselineEvidence = Get-FileEvidence -Path $Installer -RelativePath ([System.IO.Path]::GetFileName($Installer))
    if ([int64]$baselineEvidence.bytes -ne 386218356 -or
        -not ([string]$baselineEvidence.sha256).Equals('1C151B1534A66927FFA5B50DE58D05EE27247B933797441A59C747510C32483C', [System.StringComparison]::OrdinalIgnoreCase)) {
        throw 'Baseline installer does not match the frozen E-02 bytes and SHA-256.'
    }
} else {
    $actualHead = (& git -C $RepositoryRoot rev-parse HEAD).Trim().ToLowerInvariant()
    if ($LASTEXITCODE -ne 0 -or $actualHead -ne $SourceCommit.ToLowerInvariant()) {
        throw "Candidate source commit does not match repository HEAD: $actualHead"
    }
    $repositoryChanges = @(Get-RepositoryChanges -Root $RepositoryRoot)
    if ($repositoryChanges.Count -ne 0) { throw 'Candidate repository must be completely clean before manifest generation.' }
    $expectedRollbackTool = [System.IO.Path]::GetFullPath((Join-Path $RepositoryRoot 'frontend\src-tauri\scripts\meetily-versioned-data.ps1'))
    if (-not $RollbackTool.Equals($expectedRollbackTool, [System.StringComparison]::OrdinalIgnoreCase)) {
        throw 'Candidate rollback tool must use the fixed repository path.'
    }
    Assert-NoReparsePath -Path $RollbackTool -Boundary $RepositoryRoot
    $candidateBuildProvenance = Read-CandidateBuildAttestation -Path $BuildAttestation
}

if (Test-Path -LiteralPath $registryPath) { throw "Isolated uninstall registry key already exists: $registryPath" }
foreach ($path in @($installRoot, $dataRoot, $backupRoot, $webViewRoot)) {
    if (Test-Path -LiteralPath $path) { throw "Isolated manifest-generation path already exists: $path" }
}
if (@(Get-Process -Name 'meetily' -ErrorAction SilentlyContinue).Count -ne 0) {
    throw 'A meetily process is already running; manifest generation will not interfere with it.'
}

$installerEvidence = Get-FileEvidence -Path $Installer -RelativePath ([System.IO.Path]::GetFileName($Installer))
$installRun = $null
$uninstallRun = $null
$manifest = $null
$primaryError = $null
$cleanupInvocationError = $null
try {
    $installRun = Invoke-NativeProcess -Executable $Installer -Arguments @('/S') -Label "$Role installer"
    if ($installRun.exit_code -ne 0) { throw "$Role installer exited with code $($installRun.exit_code)." }

    $deadline = (Get-Date).AddSeconds(30)
    do {
        $registry = Get-RegistryState
        if ($null -ne $registry -and (Test-Path -LiteralPath $installRoot -PathType Container)) { break }
        Start-Sleep -Milliseconds 250
    } while ((Get-Date) -lt $deadline)
    if ($null -eq $registry) { throw 'Installer did not create the exact isolated uninstall registry key.' }
    if ($registry.display_name -ne $ProductName) { throw "Unexpected DisplayName: $($registry.display_name)" }
    if ($registry.display_version -ne $ExpectedVersion) { throw "Unexpected DisplayVersion: $($registry.display_version)" }
    if (-not ([System.IO.Path]::GetFullPath($registry.install_location)).Equals($installRoot, [System.StringComparison]::OrdinalIgnoreCase)) {
        throw "Installer used an unexpected install location: $($registry.install_location)"
    }
    Assert-NoReparsePath -Path $installRoot -Boundary $env:LOCALAPPDATA

    $rolePaths = Get-InstalledRolePaths -ManifestRole $Role -ManifestProductName $ProductName
    $installedFiles = @()
    foreach ($fileRole in $rolePaths.Keys) {
        $relative = [string]$rolePaths[$fileRole]
        $path = [System.IO.Path]::GetFullPath((Join-Path $installRoot $relative))
        if (-not (Test-StrictChildPath -Candidate $path -Parent $installRoot)) { throw "Unsafe installed path: $relative" }
        Assert-NoReparsePath -Path $path -Boundary $installRoot
        $evidence = Get-FileEvidence -Path $path -RelativePath $relative
        $installedFiles += [ordered]@{
            role = [string]$fileRole
            relative_path = $evidence.relative_path
            bytes = $evidence.bytes
            sha256 = $evidence.sha256
        }
    }
    $expectedMain = [System.IO.Path]::GetFullPath((Join-Path $installRoot ([string]$rolePaths.main_executable)))
    if (-not ([System.IO.Path]::GetFullPath($registry.display_icon)).Equals($expectedMain, [System.StringComparison]::OrdinalIgnoreCase)) {
        throw 'DisplayIcon is not bound to the fixed main executable path.'
    }
    $expectedUninstaller = [System.IO.Path]::GetFullPath((Join-Path $installRoot 'uninstall.exe'))
    if (-not ([System.IO.Path]::GetFullPath($registry.uninstall_string)).Equals($expectedUninstaller, [System.StringComparison]::OrdinalIgnoreCase)) {
        throw 'UninstallString is not bound to the fixed uninstaller path.'
    }
    $productVersion = (Get-Item -LiteralPath $expectedMain).VersionInfo.ProductVersion
    if ([string]$productVersion -ne $ExpectedVersion) { throw "Main executable ProductVersion is $productVersion, expected $ExpectedVersion." }

    $manifest = [ordered]@{
        schema_version = 1
        stage = 'MOSS_FUNCTIONAL_INSTALL_BUILD_MANIFEST'
        status = 'PASS'
        role = $Role
        generated_at = (Get-Date).ToUniversalTime().ToString('o')
        source_commit = $SourceCommit.ToLowerInvariant()
        product_name = $ProductName
        bundle_id = $BundleId
        version = $ExpectedVersion
        installer = $installerEvidence
        installed_files = $installedFiles
        producer = Get-FileEvidence -Path ([System.IO.Path]::GetFullPath($MyInvocation.MyCommand.Path)) -RelativePath 'scripts/qa/new-install-build-manifest.ps1'
    }
    if ($Role -eq 'candidate') {
        $rollbackEvidence = Get-FileEvidence -Path $RollbackTool -RelativePath 'frontend/src-tauri/scripts/meetily-versioned-data.ps1'
        $manifest['rollback_tool'] = $rollbackEvidence
        $manifest['build_provenance'] = $candidateBuildProvenance
    }
} catch {
    $primaryError = $_
} finally {
    $uninstaller = Join-Path $installRoot 'uninstall.exe'
    if (Test-Path -LiteralPath $uninstaller -PathType Leaf) {
        try { $uninstallRun = Invoke-NativeProcess -Executable $uninstaller -Arguments @('/S') -Label "$Role uninstaller" } catch { $cleanupInvocationError = $_ }
    }
    $cleanupDeadline = (Get-Date).AddSeconds(30)
    do {
        $registryGone = -not (Test-Path -LiteralPath $registryPath)
        $installGone = -not (Test-Path -LiteralPath $installRoot)
        $dataGone = -not (Test-Path -LiteralPath $dataRoot)
        $backupGone = -not (Test-Path -LiteralPath $backupRoot)
        $webViewGone = -not (Test-Path -LiteralPath $webViewRoot)
        if ($registryGone -and $installGone -and $dataGone -and $backupGone -and $webViewGone) { break }
        Start-Sleep -Milliseconds 250
    } while ((Get-Date) -lt $cleanupDeadline)
    $cleanupErrors = @()
    if (-not $registryGone) { $cleanupErrors += 'isolated uninstall registry key remains' }
    if (-not $installGone) { $cleanupErrors += 'isolated install directory remains' }
    if (Test-Path -LiteralPath $dataRoot) { $cleanupErrors += 'isolated data directory unexpectedly exists after cleanup' }
    if (Test-Path -LiteralPath $backupRoot) { $cleanupErrors += 'isolated backup directory unexpectedly exists after cleanup' }
    if (Test-Path -LiteralPath $webViewRoot) { $cleanupErrors += 'isolated WebView directory unexpectedly exists after cleanup' }
    if ($null -ne $uninstallRun -and $uninstallRun.exit_code -ne 0) { $cleanupErrors += "uninstaller exit code is $($uninstallRun.exit_code)" }
    if ($null -ne $cleanupInvocationError) { $cleanupErrors += "uninstaller invocation failed: $($cleanupInvocationError.Exception.Message)" }
}

$failureMessages = @()
if ($null -ne $primaryError) { $failureMessages += "primary failure: $($primaryError.Exception.Message)" }
if ($cleanupErrors.Count -ne 0) { $failureMessages += "cleanup failure: $($cleanupErrors -join '; ')" }
if ($failureMessages.Count -ne 0) { throw ($failureMessages -join ' | ') }
if ($null -eq $manifest) { throw 'Manifest was not produced.' }
Write-AtomicJson -Value $manifest -Path $Output

[ordered]@{
    status = 'PASS'
    role = $Role
    source_commit = $manifest.source_commit
    installer_sha256 = $manifest.installer.sha256
    installed_file_count = @($manifest.installed_files).Count
    install_exit_code = $installRun.exit_code
    uninstall_exit_code = $uninstallRun.exit_code
    output = $Output
} | ConvertTo-Json -Depth 8

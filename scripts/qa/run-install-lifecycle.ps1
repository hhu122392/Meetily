[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)][string]$CandidateInstaller,
    [Parameter(Mandatory = $true)][string]$BaselineInstaller,
    [Parameter(Mandatory = $true)][string]$OutputRoot,
    [Parameter(Mandatory = $true)][string]$PublicOutput,
    [Parameter(Mandatory = $true)][string]$ProtectedBaselinePath,
    [Parameter(Mandatory = $true)][string]$FixtureDatabase,
    [Parameter(Mandatory = $true)][string]$FixtureManifest,
    [Parameter(Mandatory = $true)][string]$PythonPath,
    [Parameter(Mandatory = $true)][string]$CandidateBuildManifest,
    [Parameter(Mandatory = $true)][string]$BaselineBuildManifest,
    [ValidatePattern('^[A-Za-z0-9](?:[A-Za-z0-9._-]{0,126}[A-Za-z0-9])?$')][string]$ProductName = 'meetily-p6-lifecycle',
    [ValidatePattern('^[A-Za-z0-9](?:[A-Za-z0-9._-]{0,126}[A-Za-z0-9])?$')][string]$BundleId = 'com.meetily.ai.p6lifecycle',
    [string]$CandidateVersion = '0.4.2',
    [string]$BaselineVersion = '0.4.1',
    [string]$RunId = 'FIX-1B9C29A-20260902-01',
    [ValidateRange(0, 65535)][int]$CdpPort = 0,
    [ValidateRange(30, 300)][int]$OldVersionStableSeconds = 30,
    [ValidateRange(5, 300)][int]$CandidateStableSeconds = 15
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'

$scriptRoot = Split-Path -Parent $MyInvocation.MyCommand.Path
$SqliteAudit = Join-Path $scriptRoot 'sqlite-audit.py'
$MeetingCheckScript = Join-Path $scriptRoot 'cdp-lifecycle-meeting-check.mjs'
$FixtureSelectorScript = Join-Path $scriptRoot 'select-lifecycle-fixture.py'
$FixtureSeedScript = Join-Path $scriptRoot 'seed-lifecycle-preservation-fixture.py'
$CdpExitScript = Join-Path $scriptRoot 'cdp-exit-app.mjs'
$NativeArgumentScript = Join-Path $scriptRoot 'windows-native-arguments.ps1'
$VersionedDataTest = Join-Path $scriptRoot 'test-meetily-versioned-data.ps1'

$CandidateInstaller = [System.IO.Path]::GetFullPath($CandidateInstaller)
$BaselineInstaller = [System.IO.Path]::GetFullPath($BaselineInstaller)
$OutputRoot = [System.IO.Path]::GetFullPath($OutputRoot)
$PublicOutput = [System.IO.Path]::GetFullPath($PublicOutput)
$ProtectedBaselinePath = [System.IO.Path]::GetFullPath($ProtectedBaselinePath)
$FixtureDatabase = [System.IO.Path]::GetFullPath($FixtureDatabase)
$FixtureManifest = [System.IO.Path]::GetFullPath($FixtureManifest)
$PythonPath = [System.IO.Path]::GetFullPath($PythonPath)
$CandidateBuildManifest = [System.IO.Path]::GetFullPath($CandidateBuildManifest)
$BaselineBuildManifest = [System.IO.Path]::GetFullPath($BaselineBuildManifest)
$MeetingCheckScript = [System.IO.Path]::GetFullPath($MeetingCheckScript)
$FixtureSelectorScript = [System.IO.Path]::GetFullPath($FixtureSelectorScript)
$FixtureSeedScript = [System.IO.Path]::GetFullPath($FixtureSeedScript)
$CdpExitScript = [System.IO.Path]::GetFullPath($CdpExitScript)
$NativeArgumentScript = [System.IO.Path]::GetFullPath($NativeArgumentScript)
$VersionedDataTest = [System.IO.Path]::GetFullPath($VersionedDataTest)
$repoRoot = [System.IO.Path]::GetFullPath((Join-Path $scriptRoot '..\..'))
$NsisHook = [System.IO.Path]::GetFullPath((Join-Path $repoRoot 'frontend\src-tauri\scripts\nsis-installer-hooks.nsh'))
$NsisTemplate = [System.IO.Path]::GetFullPath((Join-Path $repoRoot 'frontend\src-tauri\scripts\nsis-installer-template.nsi'))
$UninstallDataGuard = [System.IO.Path]::GetFullPath((Join-Path $repoRoot 'frontend\src-tauri\scripts\meetily-uninstall-data-guard.ps1'))
$NodePath = [System.IO.Path]::GetFullPath((Get-Command node.exe -ErrorAction Stop).Source)
$WindowsPowerShellPath = [System.IO.Path]::GetFullPath((Join-Path $env:SystemRoot 'System32\WindowsPowerShell\v1.0\powershell.exe'))
. $NativeArgumentScript

$testInstallDirectory = [System.IO.Path]::GetFullPath((Join-Path $env:LOCALAPPDATA $ProductName))
$testDataDirectory = [System.IO.Path]::GetFullPath((Join-Path $env:APPDATA $BundleId))
$testWebViewDirectory = [System.IO.Path]::GetFullPath((Join-Path $env:LOCALAPPDATA $BundleId))
$testBackupRoot = $testDataDirectory + '.rollback-backups'
$testRegistryPath = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\' + $ProductName
$testArchiveRoot = Join-Path $OutputRoot 'preserved-test-data'
$logRoot = Join-Path $OutputRoot 'logs'
$privateResultPath = Join-Path $OutputRoot 'windows-install-lifecycle.private.json'
$recordingRoot = Join-Path $OutputRoot 'recordings'
$fixtureBindingPath = Join-Path $OutputRoot 'fixture-binding.private.json'
$fixtureSnapshotPath = Join-Path $OutputRoot 'fixture-snapshot.private.sqlite'
$fixtureSeedResultPath = Join-Path $OutputRoot 'fixture-seed.private.json'
$versionedDataTestRoot = Join-Path $OutputRoot 'versioned-data-tests'
$versionedDataFixtureRoot = Join-Path $OutputRoot 'versioned-data-fixtures'
$versionedDataUnitReportPath = Join-Path $versionedDataTestRoot 'R05-unit-tests.private.json'
$versionedDataFaultReportPath = Join-Path $versionedDataTestRoot 'R05-fault-injection.json'
$downgradeRefusalObservationPath = Join-Path $OutputRoot 'downgrade-refusal-observation.private.json'
$approvedBaselineSourceCommit = '7392eae159443822c80d3675ca9af388e94b2d71'

function Write-Utf8Text {
    param([Parameter(Mandatory = $true)][string]$Path, [Parameter(Mandatory = $true)][AllowEmptyString()][string]$Content)
    $parent = Split-Path -Parent $Path
    if (-not [string]::IsNullOrWhiteSpace($parent) -and -not (Test-Path -LiteralPath $parent -PathType Container)) {
        New-Item -ItemType Directory -Path $parent -Force | Out-Null
    }
    [System.IO.File]::WriteAllText($Path, $Content, [System.Text.UTF8Encoding]::new($false))
}

function Test-StrictChildPath {
    param([Parameter(Mandatory = $true)][string]$Candidate, [Parameter(Mandatory = $true)][string]$Parent)
    $candidateFull = [System.IO.Path]::GetFullPath($Candidate).TrimEnd('\', '/')
    $parentFull = [System.IO.Path]::GetFullPath($Parent).TrimEnd('\', '/')
    return $candidateFull.StartsWith($parentFull + [System.IO.Path]::DirectorySeparatorChar, [System.StringComparison]::OrdinalIgnoreCase)
}

function Test-PathsOverlap {
    param([Parameter(Mandatory = $true)][string]$Left, [Parameter(Mandatory = $true)][string]$Right)
    $leftFull = [System.IO.Path]::GetFullPath($Left).TrimEnd('\', '/')
    $rightFull = [System.IO.Path]::GetFullPath($Right).TrimEnd('\', '/')
    return $leftFull.Equals($rightFull, [System.StringComparison]::OrdinalIgnoreCase) -or
        $leftFull.StartsWith($rightFull + [System.IO.Path]::DirectorySeparatorChar, [System.StringComparison]::OrdinalIgnoreCase) -or
        $rightFull.StartsWith($leftFull + [System.IO.Path]::DirectorySeparatorChar, [System.StringComparison]::OrdinalIgnoreCase)
}

function Resolve-SafeRelativePath {
    param(
        [Parameter(Mandatory = $true)][string]$Root,
        [Parameter(Mandatory = $true)][string]$RelativePath
    )
    if ([string]::IsNullOrWhiteSpace($RelativePath) -or [System.IO.Path]::IsPathRooted($RelativePath)) {
        throw "Expected a non-empty relative path: $RelativePath"
    }
    $segments = @($RelativePath.Replace('/', '\').Split('\'))
    if (@($segments | Where-Object { $_ -in @('', '.', '..') }).Count -ne 0) {
        throw "Relative path contains an unsafe segment: $RelativePath"
    }
    $resolved = [System.IO.Path]::GetFullPath((Join-Path $Root $RelativePath))
    if (-not (Test-StrictChildPath -Candidate $resolved -Parent $Root)) {
        throw "Relative path escapes its approved root: $RelativePath"
    }
    return $resolved
}

function Assert-NoReparsePath {
    param(
        [Parameter(Mandatory = $true)][string]$Path,
        [Parameter(Mandatory = $true)][string]$ExpectedRoot,
        [switch]$AllowRoot,
        [switch]$AllowMissing
    )
    $fullPath = [System.IO.Path]::GetFullPath($Path).TrimEnd('\', '/')
    $rootFull = [System.IO.Path]::GetFullPath($ExpectedRoot).TrimEnd('\', '/')
    $inside = if ($AllowRoot) {
        $fullPath.Equals($rootFull, [System.StringComparison]::OrdinalIgnoreCase) -or
            (Test-StrictChildPath -Candidate $fullPath -Parent $rootFull)
    } else {
        Test-StrictChildPath -Candidate $fullPath -Parent $rootFull
    }
    if (-not $inside) { throw "Path is outside its approved root: $fullPath" }
    $cursor = $fullPath
    while ($true) {
        if (Test-Path -LiteralPath $cursor) {
            $item = Get-Item -LiteralPath $cursor -Force
            if (($item.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0) {
                throw "Path contains a reparse point: $cursor"
            }
        } elseif (-not $AllowMissing) {
            throw "Path component does not exist: $cursor"
        }
        if ($cursor.Equals($rootFull, [System.StringComparison]::OrdinalIgnoreCase)) { break }
        $parent = [System.IO.Path]::GetDirectoryName($cursor)
        if ([string]::IsNullOrWhiteSpace($parent) -or $parent.Equals($cursor, [System.StringComparison]::OrdinalIgnoreCase)) {
            throw "Could not reach approved path root: $rootFull"
        }
        $cursor = $parent.TrimEnd('\', '/')
    }
    return $fullPath
}

function Assert-IsolatedPathContract {
    param([Parameter(Mandatory = $true)]$ProtectedBaseline)
    if ($ProductName -in @('.', '..') -or $BundleId -in @('.', '..')) { throw 'ProductName and BundleId must be safe single directory names.' }
    if (-not (Test-StrictChildPath -Candidate $testInstallDirectory -Parent $env:LOCALAPPDATA)) { throw 'Install directory is not a strict LOCALAPPDATA child.' }
    if (-not (Test-StrictChildPath -Candidate $testWebViewDirectory -Parent $env:LOCALAPPDATA)) { throw 'WebView directory is not a strict LOCALAPPDATA child.' }
    if (-not (Test-StrictChildPath -Candidate $testDataDirectory -Parent $env:APPDATA)) { throw 'Data directory is not a strict APPDATA child.' }
    if (-not (Test-StrictChildPath -Candidate $testBackupRoot -Parent $env:APPDATA)) { throw 'Backup directory is not a strict APPDATA child.' }
    $appDataBoundary = [System.IO.Path]::GetPathRoot([System.IO.Path]::GetFullPath($env:APPDATA))
    $localAppDataBoundary = [System.IO.Path]::GetPathRoot([System.IO.Path]::GetFullPath($env:LOCALAPPDATA))
    [void](Assert-NoReparsePath -Path $env:APPDATA -ExpectedRoot $appDataBoundary -AllowRoot -AllowMissing)
    [void](Assert-NoReparsePath -Path $env:LOCALAPPDATA -ExpectedRoot $localAppDataBoundary -AllowRoot -AllowMissing)
    [void](Assert-NoReparsePath -Path $testInstallDirectory -ExpectedRoot $env:LOCALAPPDATA -AllowMissing)
    [void](Assert-NoReparsePath -Path $testWebViewDirectory -ExpectedRoot $env:LOCALAPPDATA -AllowMissing)
    [void](Assert-NoReparsePath -Path $testDataDirectory -ExpectedRoot $env:APPDATA -AllowMissing)
    [void](Assert-NoReparsePath -Path $testBackupRoot -ExpectedRoot $env:APPDATA -AllowMissing)
    [void](Assert-NoReparsePath -Path $OutputRoot -ExpectedRoot ([System.IO.Path]::GetPathRoot($OutputRoot)) -AllowRoot -AllowMissing)
    [void](Assert-NoReparsePath -Path $PublicOutput -ExpectedRoot ([System.IO.Path]::GetPathRoot($PublicOutput)) -AllowRoot -AllowMissing)
    $isolatedRoots = @($testInstallDirectory, $testWebViewDirectory, $testDataDirectory, $testBackupRoot)
    for ($leftIndex = 0; $leftIndex -lt $isolatedRoots.Count; $leftIndex++) {
        for ($rightIndex = $leftIndex + 1; $rightIndex -lt $isolatedRoots.Count; $rightIndex++) {
            if (Test-PathsOverlap -Left $isolatedRoots[$leftIndex] -Right $isolatedRoots[$rightIndex]) {
                throw "Isolated lifecycle roots overlap: $($isolatedRoots[$leftIndex]) and $($isolatedRoots[$rightIndex])"
            }
        }
    }
    $protectedPaths = @($ProtectedBaseline | ForEach-Object { [System.IO.Path]::GetFullPath([string]$_.path) })
    $protectedRoots = @(
        @($protectedPaths | ForEach-Object { Split-Path -Parent $_ }) + @(Split-Path -Parent $FixtureDatabase) |
            Select-Object -Unique
    )
    $broadOutputTargets = @(
        [System.IO.Path]::GetPathRoot($OutputRoot),
        [Environment]::GetFolderPath([Environment+SpecialFolder]::UserProfile),
        $env:APPDATA,
        $env:LOCALAPPDATA
    ) | ForEach-Object { [System.IO.Path]::GetFullPath($_).TrimEnd('\', '/') }
    if ($broadOutputTargets -contains $OutputRoot.TrimEnd('\', '/')) { throw 'OutputRoot resolves to a protected broad directory.' }
    if (Test-PathsOverlap -Left $OutputRoot -Right $PublicOutput) {
        throw 'PublicOutput must be outside the private OutputRoot so it cannot overwrite private evidence.'
    }
    foreach ($protectedPath in $protectedPaths + $protectedRoots + @($ProtectedBaselinePath, $FixtureDatabase, $FixtureManifest)) {
        if ((Test-PathsOverlap -Left $OutputRoot -Right $protectedPath) -or (Test-PathsOverlap -Left $PublicOutput -Right $protectedPath)) {
            throw 'An evidence output path overlaps protected user data or a frozen input.'
        }
    }
    $otherPaths = @(
        $OutputRoot, $PublicOutput, $ProtectedBaselinePath, $FixtureDatabase, $FixtureManifest,
        $CandidateInstaller, $BaselineInstaller, $CandidateBuildManifest, $BaselineBuildManifest,
        $SqliteAudit, $PythonPath, $MeetingCheckScript, $FixtureSelectorScript, $FixtureSeedScript,
        $CdpExitScript, $NativeArgumentScript, $VersionedDataTest, $NsisHook, $NsisTemplate, $UninstallDataGuard
    ) + $protectedPaths + $protectedRoots
    foreach ($isolatedRoot in $isolatedRoots) {
        foreach ($otherPath in $otherPaths) {
            if (Test-PathsOverlap -Left $isolatedRoot -Right $otherPath) {
                throw "Isolated lifecycle root overlaps an output, input, or protected path: $isolatedRoot"
            }
        }
    }
}

function Write-JsonFile {
    param([Parameter(Mandatory = $true)]$Value, [Parameter(Mandatory = $true)][string]$Path)
    Write-Utf8Text -Path $Path -Content (($Value | ConvertTo-Json -Depth 40) + "`n")
}

function Get-FileEvidence {
    param([Parameter(Mandatory = $true)][string]$Path)
    $exists = Test-Path -LiteralPath $Path -PathType Leaf
    $item = if ($exists) { Get-Item -LiteralPath $Path } else { $null }
    return [ordered]@{
        path = $Path
        exists = $exists
        bytes = if ($exists) { [int64]$item.Length } else { $null }
        sha256 = if ($exists) { (Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash.ToUpperInvariant() } else { $null }
    }
}

function Get-PublicFileEvidence {
    param($Evidence)
    return [ordered]@{
        exists = [bool]$Evidence.exists
        bytes = $Evidence.bytes
        sha256 = $Evidence.sha256
    }
}

function Test-FileMatchesExpectedEvidence {
    param($Actual, $Expected)
    return $Actual.exists -and
        [int64]$Actual.bytes -eq [int64]$Expected.bytes -and
        [string]$Actual.sha256 -eq ([string]$Expected.sha256).ToUpperInvariant()
}

function Read-BuildManifest {
    param(
        [Parameter(Mandatory = $true)][string]$Path,
        [Parameter(Mandatory = $true)][ValidateSet('candidate', 'baseline')][string]$ExpectedRole,
        [switch]$RequireRollbackTool
    )
    $manifest = Get-Content -LiteralPath $Path -Raw -Encoding UTF8 | ConvertFrom-Json
    if ([int]$manifest.schema_version -ne 1 -or [string]$manifest.stage -ne 'MOSS_FUNCTIONAL_INSTALL_BUILD_MANIFEST' -or
        [string]$manifest.status -ne 'PASS' -or [string]$manifest.role -ne $ExpectedRole) {
        throw "$ExpectedRole build manifest has an unexpected schema, stage, status, or role."
    }
    if ([string]$manifest.source_commit -notmatch '^[0-9a-fA-F]{40}$') {
        throw "$ExpectedRole build manifest has an invalid source commit."
    }
    if ([string]$manifest.installer.sha256 -notmatch '^[0-9a-fA-F]{64}$' -or [int64]$manifest.installer.bytes -le 0) {
        throw "$ExpectedRole build manifest has invalid installer evidence."
    }
    $requiredRolePaths = [ordered]@{
        main_executable = 'meetily.exe'
        llama_helper = 'llama-helper.exe'
        moss_helper = 'moss-helper.exe'
        ffmpeg = 'ffmpeg.exe'
        directml = 'DirectML.dll'
        webview2 = 'runtime/webview2-fixed/msedgewebview2.exe'
        uninstaller = 'uninstall.exe'
    }
    $entries = @($manifest.installed_files)
    if ($entries.Count -ne $requiredRolePaths.Count) { throw "$ExpectedRole build manifest must contain exactly the approved installed-file roles." }
    $seenRoles = @{}
    $seenPaths = @{}
    foreach ($entry in $entries) {
        $role = [string]$entry.role
        $relativePath = [string]$entry.relative_path
        if ([string]::IsNullOrWhiteSpace($role) -or $seenRoles.ContainsKey($role)) {
            throw "$ExpectedRole build manifest contains a missing or duplicate installed-file role: $role"
        }
        if (-not $requiredRolePaths.Contains($role)) {
            throw "$ExpectedRole build manifest contains an unexpected installed-file role: $role"
        }
        $normalizedRelativePath = $relativePath.Replace('\', '/')
        if (-not $normalizedRelativePath.Equals([string]$requiredRolePaths[$role], [System.StringComparison]::Ordinal)) {
            throw "$ExpectedRole build manifest role $role must use fixed relative path $($requiredRolePaths[$role])."
        }
        $resolved = Resolve-SafeRelativePath -Root $testInstallDirectory -RelativePath $relativePath
        $pathKey = $resolved.ToUpperInvariant()
        if ($seenPaths.ContainsKey($pathKey)) { throw "$ExpectedRole build manifest contains a duplicate installed path: $relativePath" }
        if ([string]$entry.sha256 -notmatch '^[0-9a-fA-F]{64}$' -or [int64]$entry.bytes -le 0) {
            throw "$ExpectedRole build manifest has invalid evidence for: $relativePath"
        }
        $seenRoles[$role] = $true
        $seenPaths[$pathKey] = $true
    }
    foreach ($requiredRole in $requiredRolePaths.Keys) {
        if (-not $seenRoles.ContainsKey($requiredRole)) { throw "$ExpectedRole build manifest is missing required role: $requiredRole" }
    }
    if ($RequireRollbackTool) {
        if ([string]$manifest.rollback_tool.relative_path -ne 'frontend/src-tauri/scripts/meetily-versioned-data.ps1' -or
            [string]$manifest.rollback_tool.sha256 -notmatch '^[0-9a-fA-F]{64}$' -or
            [int64]$manifest.rollback_tool.bytes -le 0) {
            throw 'Candidate build manifest has invalid rollback-tool evidence.'
        }
        if ([string]$manifest.producer.relative_path -ne 'scripts/qa/new-install-build-manifest.ps1') {
            throw 'Candidate build manifest has an unexpected producer path.'
        }
        $manifestProducerPath = Resolve-SafeRelativePath -Root $repoRoot -RelativePath ([string]$manifest.producer.relative_path)
        if (-not (Test-FileMatchesExpectedEvidence -Actual (Get-FileEvidence $manifestProducerPath) -Expected $manifest.producer)) {
            throw 'Candidate build manifest producer does not match the checked-out generator.'
        }
        $provenance = $manifest.build_provenance
        if ([int]$provenance.artifact_count -ne 7 -or @($provenance.commands).Count -ne 2) {
            throw 'Candidate build manifest has incomplete controlled-build provenance.'
        }
        $attestationPath = [System.IO.Path]::GetFullPath([string]$provenance.evidence.path)
        $buildLogPath = [System.IO.Path]::GetFullPath([string]$provenance.build_log.path)
        [void](Assert-NoReparsePath -Path $attestationPath -ExpectedRoot ([System.IO.Path]::GetPathRoot($attestationPath)) -AllowRoot)
        [void](Assert-NoReparsePath -Path $buildLogPath -ExpectedRoot ([System.IO.Path]::GetPathRoot($buildLogPath)) -AllowRoot)
        if (-not (Test-FileMatchesExpectedEvidence -Actual (Get-FileEvidence $attestationPath) -Expected $provenance.evidence) -or
            -not (Test-FileMatchesExpectedEvidence -Actual (Get-FileEvidence $buildLogPath) -Expected $provenance.build_log)) {
            throw 'Candidate build attestation or build log does not match the build manifest.'
        }
        $attestation = Get-Content -LiteralPath $attestationPath -Raw -Encoding UTF8 | ConvertFrom-Json
        if ([int]$attestation.schema_version -ne 1 -or [string]$attestation.stage -ne 'MOSS_FUNCTIONAL_CANDIDATE_BUILD' -or
            [string]$attestation.status -ne 'PASS' -or
            -not ([string]$attestation.source_commit).Equals([string]$manifest.source_commit, [System.StringComparison]::OrdinalIgnoreCase) -or
            -not [bool]$attestation.worktree_clean_before -or -not [bool]$attestation.worktree_clean_after) {
            throw 'Candidate build attestation does not prove one clean build from the manifest source commit.'
        }
        $attestedInstaller = @($attestation.artifacts | Where-Object { [string]$_.role -eq 'nsis_installer' })
        if ($attestedInstaller.Count -ne 1 -or [int64]$attestedInstaller[0].bytes -ne [int64]$manifest.installer.bytes -or
            -not ([string]$attestedInstaller[0].sha256).Equals([string]$manifest.installer.sha256, [System.StringComparison]::OrdinalIgnoreCase)) {
            throw 'Candidate build attestation installer does not match the installed-file manifest.'
        }
    }
    if ($ExpectedRole -eq 'baseline' -and -not ([string]$manifest.source_commit).Equals($approvedBaselineSourceCommit, [System.StringComparison]::OrdinalIgnoreCase)) {
        throw "Baseline build manifest source_commit must be the approved baseline commit $approvedBaselineSourceCommit."
    }
    if ($ExpectedRole -eq 'baseline' -and ([int64]$manifest.installer.bytes -ne 386218356 -or
        -not ([string]$manifest.installer.sha256).Equals('1C151B1534A66927FFA5B50DE58D05EE27247B933797441A59C747510C32483C', [System.StringComparison]::OrdinalIgnoreCase))) {
        throw 'Baseline build manifest installer does not match the frozen E-02 bytes and SHA-256.'
    }
    return $manifest
}

function Read-FixtureManifest {
    param([Parameter(Mandatory = $true)][string]$Path)
    $manifest = Get-Content -LiteralPath $Path -Raw -Encoding UTF8 | ConvertFrom-Json
    if ([int]$manifest.schema_version -ne 1 -or [string]$manifest.role -ne 'baseline_compatible_lifecycle_fixture') {
        throw 'Fixture manifest has an unexpected schema or role.'
    }
    $databaseName = Split-Path -Leaf $FixtureDatabase
    if ([string]$manifest.database_name -ne $databaseName) {
        throw 'Fixture manifest database_name does not match FixtureDatabase.'
    }
    $requiredRelativePaths = @($databaseName, $databaseName + '-wal')
    $entries = @($manifest.required_files)
    if ($entries.Count -ne $requiredRelativePaths.Count) {
        throw 'Fixture manifest must bind exactly the SQLite main file and WAL.'
    }
    $fixtureRoot = Split-Path -Parent $FixtureDatabase
    $seen = @{}
    $checks = @(
        foreach ($entry in $entries) {
            $relativePath = [string]$entry.relative_path
            if ($relativePath -notin $requiredRelativePaths -or $seen.ContainsKey($relativePath)) {
                throw "Fixture manifest has an unexpected or duplicate file: $relativePath"
            }
            if ([string]$entry.sha256 -notmatch '^[0-9a-fA-F]{64}$' -or [int64]$entry.bytes -le 0) {
                throw "Fixture manifest has invalid evidence for: $relativePath"
            }
            $resolved = Resolve-SafeRelativePath -Root $fixtureRoot -RelativePath $relativePath
            [void](Assert-NoReparsePath -Path $resolved -ExpectedRoot $fixtureRoot)
            $actual = Get-FileEvidence $resolved
            $matches = Test-FileMatchesExpectedEvidence -Actual $actual -Expected $entry
            if (-not $matches) { throw "Fixture input does not match its frozen manifest: $relativePath" }
            $seen[$relativePath] = $true
            [ordered]@{
                relative_path = $relativePath
                expected_bytes = [int64]$entry.bytes
                expected_sha256 = ([string]$entry.sha256).ToUpperInvariant()
                actual = $actual
                matches = $matches
            }
        }
    )
    foreach ($requiredRelativePath in $requiredRelativePaths) {
        if (-not $seen.ContainsKey($requiredRelativePath)) { throw "Fixture manifest is missing: $requiredRelativePath" }
    }
    $allowedSourceNames = @($databaseName, $databaseName + '-wal', $databaseName + '-shm')
    $actualSidecars = @(
        Get-ChildItem -LiteralPath $fixtureRoot -File -Force |
            Where-Object { $_.Name.StartsWith($databaseName + '-', [System.StringComparison]::OrdinalIgnoreCase) } |
            Sort-Object Name
    )
    $unapprovedSidecars = @($actualSidecars | Where-Object { $_.Name -notin $allowedSourceNames })
    if ($unapprovedSidecars.Count -ne 0) {
        throw "Fixture directory contains unapproved SQLite sidecars: $(@($unapprovedSidecars | ForEach-Object { $_.Name }) -join ', ')"
    }
    $sidecarPolicy = [ordered]@{
        approved_bound_files = $requiredRelativePaths
        ignored_transient_files = @($actualSidecars | Where-Object { $_.Name -eq ($databaseName + '-shm') } | ForEach-Object { $_.Name })
        rejected_unapproved_files = @()
        policy = 'main_and_wal_bound_shm_ignored_and_rebuilt'
    }
    $logical = $manifest.logical_database
    if ([int]$logical.migration_count -ne 14 -or [int64]$logical.last_migration_version -ne 20260830000000 -or
        -not [bool]$logical.all_migrations_succeeded -or [int]$logical.meeting_count -le 0 -or
        [int]$logical.transcript_count -le 0 -or [string]$logical.integrity_check -ne 'ok' -or
        [string]$logical.foreign_key_check -ne 'ok') {
        throw 'Fixture manifest does not describe an approved 14-migration compatible database.'
    }
    return [ordered]@{ manifest = $manifest; files = $checks; sidecar_policy = $sidecarPolicy; passed = @($checks | Where-Object { -not $_.matches }).Count -eq 0 }
}

function Assert-FixtureBindingMatchesManifest {
    param(
        [Parameter(Mandatory = $true)]$Binding,
        [Parameter(Mandatory = $true)]$FixtureInput
    )
    foreach ($file in @($FixtureInput.files)) {
        $property = $Binding.source_file_set.PSObject.Properties | Where-Object { $_.Name -eq [string]$file.relative_path } | Select-Object -First 1
        if ($null -eq $property -or [int64]$property.Value.bytes -ne [int64]$file.expected_bytes -or
            [string]$property.Value.sha256 -ne [string]$file.expected_sha256) {
            throw "Frozen source-set does not match the fixture manifest: $($file.relative_path)"
        }
    }
    if ([string]$Binding.sqlite_sidecar_policy -ne [string]$FixtureInput.sidecar_policy.policy -or
        @($Binding.rejected_unapproved_sidecars).Count -ne 0) {
        throw 'Frozen source-set does not enforce the approved SQLite sidecar policy.'
    }
    $logical = $FixtureInput.manifest.logical_database
    if ([string]$Binding.snapshot_integrity_check -ne [string]$logical.integrity_check -or
        [string]$Binding.snapshot_foreign_key_check -ne [string]$logical.foreign_key_check -or
        [int]$Binding.snapshot_migration_count -ne [int]$logical.migration_count -or
        [int64]$Binding.snapshot_last_migration_version -ne [int64]$logical.last_migration_version -or
        [bool]$Binding.snapshot_all_migrations_succeeded -ne [bool]$logical.all_migrations_succeeded -or
        [int]$Binding.snapshot_meeting_count -ne [int]$logical.meeting_count -or
        [int]$Binding.snapshot_transcript_count -ne [int]$logical.transcript_count) {
        throw 'Frozen fixture snapshot does not match the approved logical database facts.'
    }
}

function Assert-InstalledFilesMatchBuildManifest {
    param([Parameter(Mandatory = $true)]$BuildManifest)
    $checks = @(
        foreach ($entry in @($BuildManifest.installed_files)) {
            $path = Resolve-SafeRelativePath -Root $testInstallDirectory -RelativePath ([string]$entry.relative_path)
            $actual = Get-FileEvidence $path
            [ordered]@{
                role = [string]$entry.role
                relative_path = [string]$entry.relative_path
                expected_bytes = [int64]$entry.bytes
                expected_sha256 = ([string]$entry.sha256).ToUpperInvariant()
                actual = $actual
                matches = Test-FileMatchesExpectedEvidence -Actual $actual -Expected $entry
            }
        }
    )
    return [ordered]@{
        files = $checks
        all_match = $checks.Count -eq 7 -and @($checks | Where-Object { -not $_.matches }).Count -eq 0
    }
}

function Get-RelativePath {
    param([Parameter(Mandatory = $true)][string]$Root, [Parameter(Mandatory = $true)][string]$Path)
    $rootFull = [System.IO.Path]::GetFullPath($Root).TrimEnd('\', '/')
    $pathFull = [System.IO.Path]::GetFullPath($Path)
    if (-not $pathFull.StartsWith($rootFull + [System.IO.Path]::DirectorySeparatorChar, [System.StringComparison]::OrdinalIgnoreCase)) {
        throw "Path is outside manifest root: $pathFull"
    }
    return $pathFull.Substring($rootFull.Length + 1).Replace('\', '/')
}

function Get-SafeDirectoryInventory {
    param([Parameter(Mandatory = $true)][string]$Root)
    if (-not (Test-Path -LiteralPath $Root -PathType Container)) {
        return [ordered]@{ root = [System.IO.Path]::GetFullPath($Root); directories = @(); files = @() }
    }
    $rootFull = [System.IO.Path]::GetFullPath($Root).TrimEnd('\', '/')
    $pending = [System.Collections.Generic.Stack[string]]::new()
    $directories = [System.Collections.Generic.List[string]]::new()
    $files = [System.Collections.Generic.List[object]]::new()
    $pending.Push($rootFull)
    while ($pending.Count -gt 0) {
        $directory = $pending.Pop()
        $directoryItem = Get-Item -LiteralPath $directory -Force
        if (($directoryItem.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0) {
            throw "Directory inventory refused a reparse point: $directory"
        }
        $directories.Add($directory)
        foreach ($item in @(Get-ChildItem -LiteralPath $directory -Force)) {
            if (($item.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0) {
                throw "Directory inventory refused a reparse point: $($item.FullName)"
            }
            if ($item.PSIsContainer) {
                $pending.Push([string]$item.FullName)
            } else {
                $files.Add($item)
            }
        }
    }
    return [ordered]@{ root = $rootFull; directories = @($directories); files = @($files) }
}

function Get-DirectoryManifest {
    param([Parameter(Mandatory = $true)][string]$Root)
    $inventory = Get-SafeDirectoryInventory -Root $Root
    return @(
        foreach ($item in @($inventory.files)) {
            [ordered]@{
                relative_path = Get-RelativePath -Root $inventory.root -Path $item.FullName
                bytes = [int64]$item.Length
                sha256 = (Get-FileHash -LiteralPath $item.FullName -Algorithm SHA256).Hash.ToUpperInvariant()
            }
        }
    )
}

function Get-ManifestFingerprint {
    param([Parameter(Mandatory = $true)]$Files)
    $lines = [System.Collections.Generic.List[string]]::new()
    foreach ($file in @($Files)) {
        $lines.Add(('{0}|{1}|{2}' -f ([string]$file.relative_path), ([int64]$file.bytes), ([string]$file.sha256).ToUpperInvariant()))
    }
    $lines.Sort([System.StringComparer]::Ordinal)
    $bytes = [System.Text.Encoding]::UTF8.GetBytes([string]::Join("`n", $lines))
    $sha = [System.Security.Cryptography.SHA256]::Create()
    try { return ([System.BitConverter]::ToString($sha.ComputeHash($bytes))).Replace('-', '') } finally { $sha.Dispose() }
}

function Test-ManifestsEqual {
    param($Left, $Right)
    return @($Left).Count -eq @($Right).Count -and (Get-ManifestFingerprint $Left) -eq (Get-ManifestFingerprint $Right)
}

function Get-DirectoryTreeSnapshot {
    param([Parameter(Mandatory = $true)][string]$Root)
    $rootFull = [System.IO.Path]::GetFullPath($Root)
    if (-not (Test-Path -LiteralPath $rootFull -PathType Container)) {
        return [ordered]@{ exists = $false; directories = @(); files = @(); fingerprint_sha256 = $null }
    }
    $inventory = Get-SafeDirectoryInventory -Root $rootFull
    $directories = @(
        @($inventory.directories | ForEach-Object {
            if (([string]$_).Equals([string]$inventory.root, [System.StringComparison]::OrdinalIgnoreCase)) { '.' }
            else { Get-RelativePath -Root $inventory.root -Path ([string]$_) }
        }) | Sort-Object
    )
    $files = @(Get-DirectoryManifest -Root $rootFull | Sort-Object relative_path)
    $treeLines = [System.Collections.Generic.List[string]]::new()
    foreach ($directory in $directories) { $treeLines.Add('D|' + [string]$directory) }
    foreach ($file in $files) {
        $treeLines.Add(('F|{0}|{1}|{2}' -f ([string]$file.relative_path), ([int64]$file.bytes), ([string]$file.sha256)))
    }
    $bytes = [System.Text.Encoding]::UTF8.GetBytes([string]::Join("`n", $treeLines))
    $sha = [System.Security.Cryptography.SHA256]::Create()
    try { $fingerprint = ([System.BitConverter]::ToString($sha.ComputeHash($bytes))).Replace('-', '') } finally { $sha.Dispose() }
    return [ordered]@{ exists = $true; directories = $directories; files = $files; fingerprint_sha256 = $fingerprint }
}

function Test-DirectoryTreeSnapshotsEqual {
    param($Left, $Right)
    return [bool]$Left.exists -eq [bool]$Right.exists -and
        [string]$Left.fingerprint_sha256 -eq [string]$Right.fingerprint_sha256 -and
        @($Left.directories).Count -eq @($Right.directories).Count -and
        @($Left.files).Count -eq @($Right.files).Count
}

function Get-NonDatabaseDataManifest {
    $sqliteFamilyPattern = '^meeting_minutes\.sqlite(?:-(?:wal|shm|journal))?$'
    return @(
        Get-DirectoryManifest -Root $testDataDirectory |
            Where-Object { [string]$_.relative_path -notmatch $sqliteFamilyPattern } |
            Sort-Object relative_path
    )
}

function Compare-PreservedFileManifest {
    param(
        [Parameter(Mandatory = $true)]$Before,
        [Parameter(Mandatory = $true)]$After
    )
    $afterByPath = @{}
    foreach ($file in @($After)) { $afterByPath[[string]$file.relative_path] = $file }
    $checks = @(
        foreach ($beforeFile in @($Before)) {
            $relativePath = [string]$beforeFile.relative_path
            $afterFile = if ($afterByPath.ContainsKey($relativePath)) { $afterByPath[$relativePath] } else { $null }
            [ordered]@{
                relative_path = $relativePath
                before_bytes = [int64]$beforeFile.bytes
                after_bytes = if ($null -ne $afterFile) { [int64]$afterFile.bytes } else { $null }
                before_sha256 = [string]$beforeFile.sha256
                after_sha256 = if ($null -ne $afterFile) { [string]$afterFile.sha256 } else { $null }
                present_after = $null -ne $afterFile
                unchanged = $null -ne $afterFile -and [int64]$beforeFile.bytes -eq [int64]$afterFile.bytes -and
                    [string]$beforeFile.sha256 -eq [string]$afterFile.sha256
            }
        }
    )
    $beforePaths = @($Before | ForEach-Object { [string]$_.relative_path })
    $extraFiles = @($After | Where-Object { [string]$_.relative_path -notin $beforePaths } | Sort-Object relative_path)
    return [ordered]@{
        before_file_count = @($Before).Count
        after_file_count = @($After).Count
        file_checks = $checks
        extra_files_after = $extraFiles
        all_before_files_preserved = @($checks | Where-Object { -not $_.present_after -or -not $_.unchanged }).Count -eq 0
    }
}

function Convert-RegistryValueForEvidence {
    param($Value, [Parameter(Mandatory = $true)][string]$Kind)
    if ($null -eq $Value) { return [ordered]@{ encoding = 'null'; data = $null } }
    if ($Kind -eq 'Binary') { return [ordered]@{ encoding = 'base64'; data = [Convert]::ToBase64String([byte[]]$Value) } }
    if ($Kind -eq 'MultiString') { return [ordered]@{ encoding = 'string_array'; data = @([string[]]$Value) } }
    if ($Kind -in @('DWord', 'QWord')) {
        return [ordered]@{ encoding = 'invariant_integer'; data = [Convert]::ToString($Value, [Globalization.CultureInfo]::InvariantCulture) }
    }
    return [ordered]@{ encoding = 'string'; data = [string]$Value }
}

function Get-UninstallRegistryTreeSnapshot {
    if (-not (Test-Path -LiteralPath $testRegistryPath)) {
        return [ordered]@{ exists = $false; keys = @(); fingerprint_json = 'null' }
    }
    $pending = [System.Collections.Generic.Stack[object]]::new()
    $pending.Push([ordered]@{ path = $testRegistryPath; relative_path = '.' })
    $records = [System.Collections.Generic.List[object]]::new()
    while ($pending.Count -gt 0) {
        $pendingRecord = $pending.Pop()
        $path = [string]$pendingRecord.path
        $key = Get-Item -LiteralPath $path -ErrorAction Stop
        $values = @(
            foreach ($valueName in @($key.GetValueNames() | Sort-Object)) {
                $kind = $key.GetValueKind($valueName).ToString()
                $rawValue = $key.GetValue($valueName, $null, [Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames)
                [ordered]@{
                    name = [string]$valueName
                    type = $kind
                    value = Convert-RegistryValueForEvidence -Value $rawValue -Kind $kind
                }
            }
        )
        $relativePath = [string]$pendingRecord.relative_path
        $records.Add([ordered]@{ relative_path = $relativePath; values = $values })
        foreach ($child in @(Get-ChildItem -LiteralPath $path -ErrorAction Stop | Sort-Object PSChildName -Descending)) {
            $childRelativePath = if ($relativePath -eq '.') { [string]$child.PSChildName } else { $relativePath + '\' + [string]$child.PSChildName }
            $pending.Push([ordered]@{ path = [string]$child.PSPath; relative_path = $childRelativePath })
        }
    }
    $orderedRecords = @($records | Sort-Object relative_path)
    $json = $orderedRecords | ConvertTo-Json -Depth 20 -Compress
    return [ordered]@{ exists = $true; keys = $orderedRecords; fingerprint_json = $json }
}

function Get-ProductShortcutSnapshot {
    $desktopRoot = [Environment]::GetFolderPath([Environment+SpecialFolder]::DesktopDirectory)
    $programsRoot = [Environment]::GetFolderPath([Environment+SpecialFolder]::Programs)
    $targetName = $ProductName + '.lnk'
    $records = [System.Collections.Generic.List[object]]::new()
    foreach ($rootRecord in @(
        [ordered]@{ role = 'desktop'; path = $desktopRoot; recurse = $false },
        [ordered]@{ role = 'start_menu'; path = $programsRoot; recurse = $true }
    )) {
        if ([string]::IsNullOrWhiteSpace([string]$rootRecord.path) -or -not (Test-Path -LiteralPath $rootRecord.path -PathType Container)) { continue }
        $rootFull = [System.IO.Path]::GetFullPath([string]$rootRecord.path)
        $pending = [System.Collections.Generic.Stack[string]]::new()
        $pending.Push($rootFull)
        while ($pending.Count -gt 0) {
            $directory = $pending.Pop()
            foreach ($item in @(Get-ChildItem -LiteralPath $directory -Force -ErrorAction Stop)) {
                if ($item.PSIsContainer) {
                    if ([bool]$rootRecord.recurse -and ($item.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -eq 0) {
                        $pending.Push([string]$item.FullName)
                    }
                } elseif ($item.Name.Equals($targetName, [System.StringComparison]::OrdinalIgnoreCase)) {
                    $evidence = Get-FileEvidence -Path $item.FullName
                    $records.Add([ordered]@{
                        root_role = [string]$rootRecord.role
                        relative_path = Get-RelativePath -Root $rootFull -Path $item.FullName
                        bytes = $evidence.bytes
                        sha256 = $evidence.sha256
                    })
                }
            }
        }
    }
    return @($records | Sort-Object root_role, relative_path)
}

function Get-ProtectedSnapshot {
    param([Parameter(Mandatory = $true)]$Baseline)
    return @(
        foreach ($item in @($Baseline)) {
            $exists = Test-Path -LiteralPath $item.path -PathType Leaf
            $bytes = if ($exists) { [int64](Get-Item -LiteralPath $item.path).Length } else { $null }
            $sha256 = if ($exists) { (Get-FileHash -LiteralPath $item.path -Algorithm SHA256).Hash.ToUpperInvariant() } else { $null }
            [ordered]@{
                role = [string]$item.role
                path = [string]$item.path
                exists = $exists
                bytes = $bytes
                sha256 = $sha256
                matches_baseline = $exists -eq [bool]$item.exists -and ((-not $exists) -or ($bytes -eq [int64]$item.bytes -and $sha256 -eq [string]$item.sha256))
            }
        }
    )
}

function Get-TestRegistry {
    if (-not (Test-Path -LiteralPath $testRegistryPath)) { return $null }
    $record = Get-ItemProperty -LiteralPath $testRegistryPath -ErrorAction Stop
    if ([string]$record.DisplayName -ne $ProductName) { throw 'Exact lifecycle uninstall key has an unexpected DisplayName.' }
    $installLocation = ([string]$record.InstallLocation).Trim().Trim('"').TrimEnd('\', '/')
    if ([string]::IsNullOrWhiteSpace($installLocation) -or -not ([System.IO.Path]::GetFullPath($installLocation)).Equals($testInstallDirectory, [System.StringComparison]::OrdinalIgnoreCase)) {
        throw 'Lifecycle uninstall key points at an unexpected install directory.'
    }
    $displayIcon = ([string]$record.DisplayIcon).Trim()
    if ($displayIcon -match '^(.*),[0-9]+$') { $displayIcon = $Matches[1] }
    $displayIcon = $displayIcon.Trim('"')
    if ([string]::IsNullOrWhiteSpace($displayIcon) -or -not (Test-StrictChildPath -Candidate $displayIcon -Parent $testInstallDirectory)) {
        throw 'Lifecycle uninstall key points at an executable outside the isolated install directory.'
    }
    $uninstallString = ([string]$record.UninstallString).Trim()
    $uninstallerPath = if ($uninstallString -match '^"([^"]+)"') { $Matches[1] } elseif ($uninstallString -match '^(.+?\.exe)(?:\s|$)') { $Matches[1] } else { '' }
    if ([string]::IsNullOrWhiteSpace($uninstallerPath) -or
        -not ([System.IO.Path]::GetFullPath($uninstallerPath)).Equals((Join-Path $testInstallDirectory 'uninstall.exe'), [System.StringComparison]::OrdinalIgnoreCase)) {
        throw 'Lifecycle uninstall key points at an unexpected uninstaller.'
    }
    return [ordered]@{
        key = $ProductName
        display_name = [string]$record.DisplayName
        display_version = [string]$record.DisplayVersion
        install_location = $installLocation
        uninstall_string = $uninstallString
        display_icon = $displayIcon
        publisher = [string]$record.Publisher
    }
}

function Find-TestExecutable {
    $registry = Get-TestRegistry
    if ($null -ne $registry -and -not [string]::IsNullOrWhiteSpace($registry.display_icon)) {
        $candidate = $registry.display_icon
        if (Test-Path -LiteralPath $candidate -PathType Leaf) { return $candidate }
    }
    foreach ($name in @('meetily.exe', 'meetily-p6-lifecycle.exe')) {
        $candidate = Join-Path $testInstallDirectory $name
        if (Test-Path -LiteralPath $candidate -PathType Leaf) { return $candidate }
    }
    return $null
}

function Get-InstalledState {
    param([Parameter(Mandatory = $true)]$BuildManifest)
    $registry = Get-TestRegistry
    $mainExecutable = Find-TestExecutable
    $manifestCheck = Assert-InstalledFilesMatchBuildManifest -BuildManifest $BuildManifest
    $expectedMain = @($manifestCheck.files | Where-Object { $_.role -eq 'main_executable' })[0]
    $mainExecutableMatchesManifest = $null -ne $mainExecutable -and
        $null -ne $expectedMain -and
        ([System.IO.Path]::GetFullPath($mainExecutable)).Equals(
            [System.IO.Path]::GetFullPath([string]$expectedMain.actual.path),
            [System.StringComparison]::OrdinalIgnoreCase
        ) -and [bool]$expectedMain.matches
    $displayIconMatchesManifestMainExecutable = $null -ne $registry -and $null -ne $expectedMain -and
        ([System.IO.Path]::GetFullPath([string]$registry.display_icon)).Equals(
            [System.IO.Path]::GetFullPath([string]$expectedMain.actual.path),
            [System.StringComparison]::OrdinalIgnoreCase
        )
    $requiredFiles = @(
        $mainExecutable,
        (Join-Path $testInstallDirectory 'llama-helper.exe'),
        (Join-Path $testInstallDirectory 'moss-helper.exe'),
        (Join-Path $testInstallDirectory 'ffmpeg.exe'),
        (Join-Path $testInstallDirectory 'DirectML.dll'),
        (Join-Path $testInstallDirectory 'runtime\webview2-fixed\msedgewebview2.exe'),
        (Join-Path $testInstallDirectory 'uninstall.exe')
    )
    return [ordered]@{
        registry = $registry
        install_directory_exists = Test-Path -LiteralPath $testInstallDirectory -PathType Container
        main_executable_path = $mainExecutable
        executable = if ($null -ne $mainExecutable) { Get-FileEvidence $mainExecutable } else { [ordered]@{ path = $null; exists = $false; bytes = $null; sha256 = $null } }
        executable_product_version = if ($null -ne $mainExecutable) { (Get-Item -LiteralPath $mainExecutable).VersionInfo.ProductVersion } else { $null }
        main_executable_matches_manifest = $mainExecutableMatchesManifest
        display_icon_matches_manifest_main_executable = $displayIconMatchesManifestMainExecutable
        build_manifest_source_commit = ([string]$BuildManifest.source_commit).ToLowerInvariant()
        build_manifest_files = $manifestCheck.files
        all_required_files_match_manifest = [bool]$manifestCheck.all_match -and $mainExecutableMatchesManifest -and $displayIconMatchesManifestMainExecutable
        required_files = @(
            $requiredFiles | ForEach-Object {
                [ordered]@{
                    name = if ($null -eq $_) { 'main-executable' } else { Split-Path -Leaf $_ }
                    exists = $null -ne $_ -and (Test-Path -LiteralPath $_ -PathType Leaf)
                }
            }
        )
        all_required_files_exist = @($requiredFiles | Where-Object { $null -eq $_ -or -not (Test-Path -LiteralPath $_ -PathType Leaf) }).Count -eq 0
    }
}

function Get-ProcessTreeIds {
    param([Parameter(Mandatory = $true)][int]$RootProcessId)
    $processes = @(Get-CimInstance Win32_Process -ErrorAction SilentlyContinue | Select-Object ProcessId, ParentProcessId)
    $known = [System.Collections.Generic.HashSet[int]]::new()
    [void]$known.Add($RootProcessId)
    $changed = $true
    while ($changed) {
        $changed = $false
        foreach ($item in $processes) {
            if ($known.Contains([int]$item.ParentProcessId) -and $known.Add([int]$item.ProcessId)) { $changed = $true }
        }
    }
    return @($known)
}

function Stop-ExactProcessTree {
    param([Parameter(Mandatory = $true)][int]$RootProcessId)
    $knownTreeIds = [System.Collections.Generic.HashSet[int]]::new()
    [void]$knownTreeIds.Add($RootProcessId)
    $deadline = (Get-Date).AddSeconds(15)
    $consecutiveEmptyScans = 0
    do {
        $snapshot = @(Get-CimInstance Win32_Process -ErrorAction SilentlyContinue | Select-Object ProcessId, ParentProcessId)
        $changed = $true
        while ($changed) {
            $changed = $false
            foreach ($item in $snapshot) {
                if ($knownTreeIds.Contains([int]$item.ParentProcessId) -and $knownTreeIds.Add([int]$item.ProcessId)) { $changed = $true }
            }
        }
        $active = @($snapshot | Where-Object { $knownTreeIds.Contains([int]$_.ProcessId) } | Select-Object -ExpandProperty ProcessId)
        foreach ($processId in @($active | Sort-Object -Descending)) {
            Stop-Process -Id $processId -Force -ErrorAction SilentlyContinue
        }
        if ($active.Count -eq 0) { $consecutiveEmptyScans++ } else { $consecutiveEmptyScans = 0 }
        if ($consecutiveEmptyScans -ge 2) { return @($knownTreeIds | Sort-Object) }
        Start-Sleep -Milliseconds 250
    } while ((Get-Date) -lt $deadline)
    $remaining = @($knownTreeIds | Where-Object { $null -ne (Get-Process -Id $_ -ErrorAction SilentlyContinue) })
    throw "Exact timed-out process tree did not reach two consecutive empty scans: $($remaining -join ',')"
}

function Invoke-CapturedProcess {
    param(
        [Parameter(Mandatory = $true)][string]$FilePath,
        [string[]]$Arguments = @(),
        [Parameter(Mandatory = $true)][string]$Label,
        [ValidateRange(1, 7200)][int]$TimeoutSeconds = 900,
        [string]$WorkingDirectory = ''
    )
    if (-not (Test-Path -LiteralPath $logRoot -PathType Container)) { New-Item -ItemType Directory -Path $logRoot -Force | Out-Null }
    $stdout = Join-Path $logRoot ($Label + '.stdout.log')
    $stderr = Join-Path $logRoot ($Label + '.stderr.log')
    if ((Test-Path -LiteralPath $stdout) -or (Test-Path -LiteralPath $stderr)) { throw "Log label is already used: $Label" }
    $argumentLine = (@($Arguments | ForEach-Object { ConvertTo-NativeArgument ([string]$_) }) -join ' ')
    $parameters = @{
        FilePath = $FilePath
        ArgumentList = $argumentLine
        WindowStyle = 'Hidden'
        PassThru = $true
        RedirectStandardOutput = $stdout
        RedirectStandardError = $stderr
    }
    if (-not [string]::IsNullOrWhiteSpace($WorkingDirectory)) { $parameters.WorkingDirectory = $WorkingDirectory }
    $started = Get-Date
    $process = Start-Process @parameters
    $completed = $process.WaitForExit($TimeoutSeconds * 1000)
    if (-not $completed) {
        $stoppedTree = @(Stop-ExactProcessTree -RootProcessId $process.Id)
        throw "$Label timed out after $TimeoutSeconds seconds; stopped exact PID tree: $($stoppedTree -join ',')"
    }
    $process.WaitForExit()
    $process.Refresh()
    return [ordered]@{
        label = $Label
        file = Get-FileEvidence $FilePath
        arguments = @($Arguments)
        process_id = [int]$process.Id
        exit_code = [int]$process.ExitCode
        elapsed_seconds = [Math]::Round(((Get-Date) - $started).TotalSeconds, 3)
        stdout = Get-FileEvidence $stdout
        stderr = Get-FileEvidence $stderr
        completed_at = (Get-Date).ToString('o')
    }
}

function Invoke-TestInstaller {
    param([Parameter(Mandatory = $true)][string]$Installer, [Parameter(Mandatory = $true)][string]$Label)
    $run = Invoke-CapturedProcess -FilePath $Installer -Arguments @('/S') -Label $Label -TimeoutSeconds 1200
    $run.installer = Get-FileEvidence $Installer
    return $run
}

function Invoke-ExpectedEarlyAppExit {
    param([Parameter(Mandatory = $true)][string]$Label)
    $executable = Find-TestExecutable
    if ([string]::IsNullOrWhiteSpace($executable)) { throw "$Label main executable is missing." }
    $processesBefore = @(Get-NormalizedTestProcesses)
    $run = Invoke-CapturedProcess -FilePath $executable -Label $Label -TimeoutSeconds 90 -WorkingDirectory $testInstallDirectory
    Start-Sleep -Milliseconds 500
    $processesAfter = @(Get-NormalizedTestProcesses)
    $run.processes_before = $processesBefore
    $run.processes_after = $processesAfter
    $run.no_residual_product_processes = $processesAfter.Count -eq 0
    return $run
}

function Invoke-ConcurrentInstallerRefusal {
    param([Parameter(Mandatory = $true)]$BuildManifest)
    $stateBefore = [ordered]@{
        install = Get-DirectoryTreeSnapshot -Root $testInstallDirectory
        data = Get-DirectoryTreeSnapshot -Root $testDataDirectory
        backups = Get-DirectoryTreeSnapshot -Root $testBackupRoot
        webview = Get-DirectoryTreeSnapshot -Root $testWebViewDirectory
        registry = Get-UninstallRegistryTreeSnapshot
        shortcuts = @(Get-ProductShortcutSnapshot)
    }
    $mutexName = 'Local\MeetilyInstaller-' + $BundleId
    $faultMutexCreatedBeforeSecond = $false
    $faultMutex = $null
    try {
        $faultMutex = [System.Threading.Mutex]::new($true, $mutexName, [ref]$faultMutexCreatedBeforeSecond)
        if (-not $faultMutexCreatedBeforeSecond) { throw 'The installer mutex was already owned before the concurrency fault test.' }
        $second = Invoke-TestInstaller -Installer $CandidateInstaller -Label 'second-installer-concurrent-refusal'
    } finally {
        if ($null -ne $faultMutex) {
            if ($faultMutexCreatedBeforeSecond) { $faultMutex.ReleaseMutex() }
            $faultMutex.Dispose()
        }
    }
    Start-Sleep -Milliseconds 500
    $stateAfter = [ordered]@{
        install = Get-DirectoryTreeSnapshot -Root $testInstallDirectory
        data = Get-DirectoryTreeSnapshot -Root $testDataDirectory
        backups = Get-DirectoryTreeSnapshot -Root $testBackupRoot
        webview = Get-DirectoryTreeSnapshot -Root $testWebViewDirectory
        registry = Get-UninstallRegistryTreeSnapshot
        shortcuts = @(Get-ProductShortcutSnapshot)
    }
    $installedState = Get-InstalledState -BuildManifest $BuildManifest
    $productStateUnchanged = (Test-DirectoryTreeSnapshotsEqual $stateBefore.install $stateAfter.install) -and
        (Test-DirectoryTreeSnapshotsEqual $stateBefore.data $stateAfter.data) -and
        (Test-DirectoryTreeSnapshotsEqual $stateBefore.backups $stateAfter.backups) -and
        (Test-DirectoryTreeSnapshotsEqual $stateBefore.webview $stateAfter.webview) -and
        (Test-JsonEquivalent $stateBefore.registry $stateAfter.registry) -and
        (Test-JsonEquivalent $stateBefore.shortcuts $stateAfter.shortcuts)
    $refused = $second.exit_code -eq 1618 -and $faultMutexCreatedBeforeSecond -and $productStateUnchanged -and
        $installedState.all_required_files_match_manifest -and @(Get-ExactTestProcesses).Count -eq 0
    return [ordered]@{
        label = 'second-installer-concurrent-refusal'
        fault_mutex_name = $mutexName
        fault_mutex_owner_process_id = [int]$PID
        fault_mutex_created_before_second = $faultMutexCreatedBeforeSecond
        second_installer = $second
        expected_exit_code = 1618
        concurrent_installer_refused = $refused
        product_state_unchanged = $productStateUnchanged
        state_before = $stateBefore
        state_after = $stateAfter
        installed_state = $installedState
    }
}

function Get-ExactTestProcesses {
    return @(
        Get-CimInstance Win32_Process -ErrorAction SilentlyContinue |
            Where-Object {
                -not [string]::IsNullOrWhiteSpace([string]$_.ExecutablePath) -and
                [System.IO.Path]::GetFullPath([string]$_.ExecutablePath).StartsWith(
                    $testInstallDirectory + [System.IO.Path]::DirectorySeparatorChar,
                    [System.StringComparison]::OrdinalIgnoreCase
                )
            } |
            Select-Object ProcessId, ParentProcessId, Name, ExecutablePath, CommandLine
    )
}

function Get-NormalizedTestProcesses {
    return @(
        Get-ExactTestProcesses |
            Sort-Object ProcessId |
            ForEach-Object {
                [ordered]@{
                    process_id = [int]$_.ProcessId
                    parent_process_id = [int]$_.ParentProcessId
                    name = [string]$_.Name
                    executable_path = [string]$_.ExecutablePath
                    command_line = [string]$_.CommandLine
                }
            }
    )
}

function Get-TestCdpListenerSnapshot {
    param([ValidateRange(0, 65535)][int]$Port = 0)
    $testProcessIds = @((Get-ExactTestProcesses) | ForEach-Object { [int]$_.ProcessId })
    return @(
        Get-NetTCPConnection -State Listen -ErrorAction SilentlyContinue |
            Where-Object { ($Port -gt 0 -and [int]$_.LocalPort -eq $Port) -or ([int]$_.OwningProcess -in $testProcessIds) } |
            Sort-Object LocalAddress, LocalPort, OwningProcess |
            ForEach-Object {
                [ordered]@{
                    local_address = [string]$_.LocalAddress
                    local_port = [int]$_.LocalPort
                    owning_process = [int]$_.OwningProcess
                    state = [string]$_.State
                }
            }
    )
}

function Stop-ExactTestProcesses {
    param([ValidateRange(0, 65535)][int]$CdpPort = 0)
    $deadline = (Get-Date).AddSeconds(20)
    $consecutiveEmptyScans = 0
    do {
        $active = @(Get-ExactTestProcesses)
        $cdpOwners = @(if ($CdpPort -gt 0) { Get-TcpListenerOwnerIds -Port $CdpPort })
        if ($active.Count -eq 0 -and $cdpOwners.Count -eq 0) {
            $consecutiveEmptyScans++
            if ($consecutiveEmptyScans -ge 2) { return }
        } else {
            $consecutiveEmptyScans = 0
        }
        foreach ($item in $active) { Stop-Process -Id $item.ProcessId -Force -ErrorAction SilentlyContinue }
        Start-Sleep -Milliseconds 250
    } while ((Get-Date) -lt $deadline)
    $remaining = @(Get-ExactTestProcesses)
    $remainingCdpOwners = @(if ($CdpPort -gt 0) { Get-TcpListenerOwnerIds -Port $CdpPort })
    if ($remaining.Count -ne 0 -or $remainingCdpOwners.Count -ne 0) {
        throw "Exact lifecycle runtime did not terminate or close CDP port: processes=$(@($remaining.ProcessId) -join ','), cdp_owners=$($remainingCdpOwners -join ',')"
    }
}

function Get-FreeLoopbackTcpPort {
    $listener = [System.Net.Sockets.TcpListener]::new([System.Net.IPAddress]::Loopback, 0)
    try {
        $listener.Start()
        return [int]$listener.LocalEndpoint.Port
    } finally {
        $listener.Stop()
    }
}

function Get-TcpListenerOwnerIds {
    param([Parameter(Mandatory = $true)][int]$Port)
    return @(
        Get-NetTCPConnection -State Listen -LocalPort $Port -ErrorAction SilentlyContinue |
            Select-Object -ExpandProperty OwningProcess -Unique |
            ForEach-Object { [int]$_ }
    )
}

function Test-ProcessDescendsFrom {
    param(
        [Parameter(Mandatory = $true)][int]$ChildProcessId,
        [Parameter(Mandatory = $true)][int]$RootProcessId
    )
    if ($ChildProcessId -eq $RootProcessId) { return $true }
    $processes = @{}
    foreach ($item in @(Get-CimInstance Win32_Process -ErrorAction SilentlyContinue | Select-Object ProcessId, ParentProcessId)) {
        $processes[[int]$item.ProcessId] = [int]$item.ParentProcessId
    }
    $cursor = $ChildProcessId
    $visited = [System.Collections.Generic.HashSet[int]]::new()
    while ($processes.ContainsKey($cursor) -and $visited.Add($cursor)) {
        $cursor = [int]$processes[$cursor]
        if ($cursor -eq $RootProcessId) { return $true }
    }
    return $false
}

function Assert-CdpPortUnused {
    param([Parameter(Mandatory = $true)][int]$Port)
    $owners = @(Get-TcpListenerOwnerIds -Port $Port)
    if ($owners.Count -ne 0) { throw "CDP port $Port is already owned by PID(s): $($owners -join ',')" }
}

function Wait-CdpReady {
    param(
        [Parameter(Mandatory = $true)][int]$Port,
        [Parameter(Mandatory = $true)][int]$RootProcessId,
        [ValidateRange(1, 120)][int]$TimeoutSeconds = 30
    )
    $deadline = (Get-Date).AddSeconds($TimeoutSeconds)
    do {
        try {
            $owners = @(Get-TcpListenerOwnerIds -Port $Port)
            $ownedByApp = $owners.Count -gt 0 -and @($owners | Where-Object { -not (Test-ProcessDescendsFrom -ChildProcessId $_ -RootProcessId $RootProcessId) }).Count -eq 0
            $targets = @(Invoke-RestMethod -Uri "http://127.0.0.1:$Port/json/list" -TimeoutSec 2)
            $pages = @($targets | Where-Object {
                $_.type -eq 'page' -and
                -not [string]::IsNullOrWhiteSpace([string]$_.id) -and
                ($_.url -like 'http://tauri.localhost*' -or $_.url -like 'http://localhost:*')
            })
            if ($ownedByApp -and $pages.Count -eq 1) {
                return [ordered]@{
                    port = $Port
                    target_id = [string]$pages[0].id
                    target_url = [string]$pages[0].url
                    listener_process_ids = $owners
                    listener_owned_by_main_process_tree = $true
                }
            }
        } catch { }
        Start-Sleep -Milliseconds 250
    } while ((Get-Date) -lt $deadline)
    return $null
}

function Invoke-AppSmoke {
    param(
        [Parameter(Mandatory = $true)][string]$Label,
        [ValidateRange(5, 300)][int]$StableSeconds,
        [bool]$VerifyMeeting
    )
    $mainExecutable = Find-TestExecutable
    if ($null -eq $mainExecutable -or -not (Test-Path -LiteralPath $mainExecutable -PathType Leaf)) { throw "$Label executable is missing" }
    if (-not (Test-Path -LiteralPath $logRoot -PathType Container)) { New-Item -ItemType Directory -Path $logRoot -Force | Out-Null }
    $stdout = Join-Path $logRoot ($Label + '.app.stdout.log')
    $stderr = Join-Path $logRoot ($Label + '.app.stderr.log')
    $meetingOutput = Join-Path $OutputRoot ($Label + '.meeting.private.json')
    $meetingScreenshot = Join-Path $OutputRoot ($Label + '.meeting.private.png')
    $exitOutput = Join-Path $OutputRoot ($Label + '.exit.private.json')
    $oldBrowserArguments = [Environment]::GetEnvironmentVariable('WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS', 'Process')
    $oldCdpPort = [Environment]::GetEnvironmentVariable('CDP_PORT', 'Process')
    $oldCdpTargetId = [Environment]::GetEnvironmentVariable('CDP_TARGET_ID', 'Process')
    $started = Get-Date
    $process = $null
    $cdpReady = $false
    $cdpBinding = $null
    $meetingCheck = $null
    $gracefulExit = $null
    $earlyExitCode = $null
    $aliveForWindow = $false
    $responding = $false
    $observed = @()
    $activeCdpPort = 0
    try {
        $activeCdpPort = if ($CdpPort -eq 0) { Get-FreeLoopbackTcpPort } else { $CdpPort }
        Assert-CdpPortUnused -Port $activeCdpPort
        [Environment]::SetEnvironmentVariable('WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS', "--remote-debugging-port=$activeCdpPort", 'Process')
        [Environment]::SetEnvironmentVariable('CDP_PORT', [string]$activeCdpPort, 'Process')
        [Environment]::SetEnvironmentVariable('CDP_TARGET_ID', $null, 'Process')
        $process = Start-Process -FilePath $mainExecutable -WorkingDirectory $testInstallDirectory -WindowStyle Hidden -PassThru -RedirectStandardOutput $stdout -RedirectStandardError $stderr
        $cdpBinding = Wait-CdpReady -Port $activeCdpPort -RootProcessId $process.Id -TimeoutSeconds 30
        $cdpReady = $null -ne $cdpBinding
        if ($cdpReady) { [Environment]::SetEnvironmentVariable('CDP_TARGET_ID', [string]$cdpBinding.target_id, 'Process') }
        $process.Refresh()
        if ($process.HasExited) { $earlyExitCode = [int]$process.ExitCode }
        if ($cdpReady -and $VerifyMeeting -and -not $process.HasExited) {
            try {
                $meetingCheck = Invoke-CapturedProcess -FilePath $NodePath -Arguments @($MeetingCheckScript, $fixtureBindingPath, $meetingOutput, $meetingScreenshot) -Label ($Label + '-meeting-check') -TimeoutSeconds 60
            } catch {
                $meetingCheck = [ordered]@{ exit_code = 1; error = $_.Exception.Message }
            }
        }
        $deadline = $started.AddSeconds($StableSeconds)
        while ((Get-Date) -lt $deadline) {
            $process.Refresh()
            if ($process.HasExited) { $earlyExitCode = [int]$process.ExitCode; break }
            Start-Sleep -Milliseconds 250
        }
        $process.Refresh()
        $aliveForWindow = -not $process.HasExited
        $liveProcess = if ($aliveForWindow) { Get-Process -Id $process.Id -ErrorAction SilentlyContinue } else { $null }
        $responding = $null -ne $liveProcess -and $liveProcess.Responding
        $observed = @(Get-ExactTestProcesses)
        if ($aliveForWindow -and $cdpReady) {
            try {
                $gracefulExit = Invoke-CapturedProcess -FilePath $NodePath -Arguments @($CdpExitScript, $exitOutput) -Label ($Label + '-graceful-exit') -TimeoutSeconds 30
            } catch {
                $gracefulExit = [ordered]@{ exit_code = 1; error = $_.Exception.Message }
            }
        }
        if ($aliveForWindow) { try { Wait-Process -Id $process.Id -Timeout 15 -ErrorAction Stop } catch { } }
    } finally {
        [Environment]::SetEnvironmentVariable('WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS', $oldBrowserArguments, 'Process')
        [Environment]::SetEnvironmentVariable('CDP_PORT', $oldCdpPort, 'Process')
        [Environment]::SetEnvironmentVariable('CDP_TARGET_ID', $oldCdpTargetId, 'Process')
    }
    Start-Sleep -Seconds 5
    if ($null -ne $process) { $process.Refresh() }
    $mainExitedWithoutCleanup = $null -ne $process -and $process.HasExited
    $mainExitCode = if ($mainExitedWithoutCleanup) { [int]$process.ExitCode } else { $null }
    $residualBeforeCleanup = @(Get-ExactTestProcesses)
    $exitReport = if (Test-Path -LiteralPath $exitOutput -PathType Leaf) {
        Get-Content -LiteralPath $exitOutput -Raw -Encoding UTF8 | ConvertFrom-Json
    } else { $null }
    $exitRequestMatchesBinding = $null -ne $exitReport -and $null -ne $cdpBinding -and
        [string]$exitReport.method -eq 'plugin:process|exit' -and [int]$exitReport.code -eq 0 -and
        [string]$exitReport.cdpTargetId -eq [string]$cdpBinding.target_id
    $gracefulExitSucceeded = $null -ne $gracefulExit -and $gracefulExit.exit_code -eq 0 -and
        $exitRequestMatchesBinding -and $mainExitedWithoutCleanup -and $mainExitCode -eq 0 -and
        $residualBeforeCleanup.Count -eq 0
    Stop-ExactTestProcesses -CdpPort $activeCdpPort
    if ($residualBeforeCleanup.Count -gt 0) { Start-Sleep -Seconds 2 }
    $residualAfterCleanup = @(Get-ExactTestProcesses)
    $cdpPortClosedAfterCleanup = $activeCdpPort -gt 0 -and @(Get-TcpListenerOwnerIds -Port $activeCdpPort).Count -eq 0
    $meetingReport = if ($VerifyMeeting -and (Test-Path -LiteralPath $meetingOutput -PathType Leaf)) {
        Get-Content -LiteralPath $meetingOutput -Raw -Encoding UTF8 | ConvertFrom-Json
    } else { $null }
    $meetingPassed = (-not $VerifyMeeting) -or ($null -ne $meetingCheck -and $meetingCheck.exit_code -eq 0 -and $null -ne $meetingReport -and $meetingReport.status -eq 'PASS')
    return [ordered]@{
        label = $Label
        executable = Get-FileEvidence $mainExecutable
        process_id = if ($null -ne $process) { [int]$process.Id } else { $null }
        required_stable_seconds = $StableSeconds
        alive_for_required_window = $aliveForWindow
        responding_at_end_of_window = $responding
        early_exit_code = $earlyExitCode
        cdp_ready = $cdpReady
        cdp_binding = $cdpBinding
        meeting_check_required = $VerifyMeeting
        meeting_check_process = $meetingCheck
        meeting_check = $meetingReport
        meeting_check_passed = $meetingPassed
        meeting_output_evidence = Get-FileEvidence $meetingOutput
        meeting_screenshot_evidence = Get-FileEvidence $meetingScreenshot
        graceful_exit = $gracefulExit
        graceful_exit_report = $exitReport
        graceful_exit_request_matches_cdp_binding = $exitRequestMatchesBinding
        graceful_exit_output_evidence = Get-FileEvidence $exitOutput
        graceful_exit_succeeded_without_cleanup = $gracefulExitSucceeded
        main_process_exited_without_cleanup = $mainExitedWithoutCleanup
        main_process_exit_code = $mainExitCode
        observed_processes = $observed
        residual_before_exact_cleanup = $residualBeforeCleanup
        residual_after_exact_cleanup = $residualAfterCleanup
        cdp_port_closed_after_cleanup = $cdpPortClosedAfterCleanup
        stdout = Get-FileEvidence $stdout
        stderr = Get-FileEvidence $stderr
        elapsed_seconds = [Math]::Round(((Get-Date) - $started).TotalSeconds, 3)
        passed = $aliveForWindow -and $responding -and $cdpReady -and $meetingPassed -and $gracefulExitSucceeded -and
            $residualAfterCleanup.Count -eq 0 -and $cdpPortClosedAfterCleanup
    }
}

function Invoke-TestUninstall {
    param([Parameter(Mandatory = $true)][string]$Label)
    $uninstaller = Join-Path $testInstallDirectory 'uninstall.exe'
    if (-not (Test-Path -LiteralPath $uninstaller -PathType Leaf)) { throw "$Label uninstaller is missing" }
    $run = Invoke-CapturedProcess -FilePath $uninstaller -Arguments @('/S') -Label $Label -TimeoutSeconds 600
    $deadline = (Get-Date).AddSeconds(90)
    do {
        if (-not (Test-Path -LiteralPath $testInstallDirectory) -and $null -eq (Get-TestRegistry)) { break }
        Start-Sleep -Milliseconds 500
    } while ((Get-Date) -lt $deadline)
    $run.install_directory_removed = -not (Test-Path -LiteralPath $testInstallDirectory -PathType Container)
    $run.registry_removed = $null -eq (Get-TestRegistry)
    $run.data_directory_preserved = Test-Path -LiteralPath $testDataDirectory -PathType Container
    $run.active_processes = @(Get-ExactTestProcesses)
    return $run
}

function Invoke-DatabaseAudit {
    param([Parameter(Mandatory = $true)][string]$Label)
    $database = Join-Path $testDataDirectory 'meeting_minutes.sqlite'
    $output = Join-Path $OutputRoot ('sqlite-' + $Label + '.json')
    $run = Invoke-CapturedProcess -FilePath $PythonPath -Arguments @($SqliteAudit, $database, $output) -Label ('sqlite-' + $Label) -TimeoutSeconds 120
    if ($run.exit_code -ne 0 -or -not (Test-Path -LiteralPath $output -PathType Leaf)) { throw "SQLite audit failed at $Label" }
    $audit = Get-Content -LiteralPath $output -Raw -Encoding UTF8 | ConvertFrom-Json
    return [ordered]@{ run = $run; report = $audit; evidence = Get-FileEvidence $output }
}

function Get-AuditTable {
    param($Audit, [Parameter(Mandatory = $true)][string]$Name)
    if ($null -eq $Audit) { return $null }
    return $Audit.tables | Where-Object { $_.name -eq $Name } | Select-Object -First 1
}

function Get-AuditObject {
    param(
        $Audit,
        [Parameter(Mandatory = $true)][ValidateSet('index', 'trigger', 'view')][string]$Type,
        [Parameter(Mandatory = $true)][string]$Name
    )
    if ($null -eq $Audit) { return $null }
    return $Audit.objects | Where-Object { [string]$_.type -eq $Type -and [string]$_.name -eq $Name } | Select-Object -First 1
}

function Compare-CriticalDatabaseTables {
    param($Before, $After)
    $excludedNames = @('_sqlx_migrations')
    $requiredNonEmptyNames = @(
        'summary_manual_revisions', 'moss_transcription_runs', 'moss_candidate_segments',
        'moss_term_corrections', 'moss_speaker_bindings', 'moss_segment_overrides',
        'moss_activation_snapshots', 'moss_activation_segments'
    )
    $names = @($Before.tables | ForEach-Object { [string]$_.name } | Where-Object { $_ -notin $excludedNames } | Sort-Object -Unique)
    $comparisons = @(
        foreach ($name in $names) {
            $left = Get-AuditTable -Audit $Before -Name $name
            $right = Get-AuditTable -Audit $After -Name $name
            $schemaEqual = $null -ne $left -and $null -ne $right -and [string]$left.schema_sha256 -eq [string]$right.schema_sha256
            $schemaPolicy = 'exact'
            $schemaPolicyPassed = $schemaEqual
            if ($null -ne $left -and $null -ne $right -and
                $name -eq 'moss_candidate_segment_alignment' -and
                [string]$left.schema_sha256 -eq '8a61f71877ef30aeafbf6adfa43f9796a86888194e5216edbc37889e6f8fac77' -and
                [string]$right.schema_sha256 -eq '9912ea13b1558d18380171fafd5e63d082fa11b775251f32eca57504a3a3fbe7') {
                $schemaPolicy = 'approved_r4_to_r5_transition'
                $schemaPolicyPassed = $true
            }
            [ordered]@{
                table = $name
                before_count = if ($null -ne $left) { [int64]$left.row_count } else { $null }
                after_count = if ($null -ne $right) { [int64]$right.row_count } else { $null }
                before_schema_sha256 = if ($null -ne $left) { [string]$left.schema_sha256 } else { $null }
                after_schema_sha256 = if ($null -ne $right) { [string]$right.schema_sha256 } else { $null }
                schema_policy = $schemaPolicy
                schema_equal = $schemaEqual
                schema_policy_passed = $schemaPolicyPassed
                before_rowset_sha256 = if ($null -ne $left) { [string]$left.rowset_sha256 } else { $null }
                after_rowset_sha256 = if ($null -ne $right) { [string]$right.rowset_sha256 } else { $null }
                equal = $null -ne $left -and $null -ne $right -and $schemaPolicyPassed -and
                    [int64]$left.row_count -eq [int64]$right.row_count -and
                    [string]$left.rowset_sha256 -eq [string]$right.rowset_sha256
            }
        }
    )
    $requiredChecks = @(
        foreach ($name in $requiredNonEmptyNames) {
            $comparison = $comparisons | Where-Object { $_.table -eq $name } | Select-Object -First 1
            [ordered]@{
                table = $name
                present_before_and_after = $null -ne $comparison
                before_nonempty = $null -ne $comparison -and [int64]$comparison.before_count -gt 0
                unchanged = $null -ne $comparison -and [bool]$comparison.equal
            }
        }
    )
    $oldNonTableObjects = @(
        $Before.objects |
            Where-Object {
                [string]$_.type -in @('index', 'trigger', 'view') -and
                [string]$_.name -notlike 'sqlite_autoindex_*'
            } |
            Sort-Object type, name
    )
    $objectComparisons = @(
        foreach ($leftObject in $oldNonTableObjects) {
            $type = [string]$leftObject.type
            $name = [string]$leftObject.name
            $rightObject = Get-AuditObject -Audit $After -Type $type -Name $name
            [ordered]@{
                type = $type
                name = $name
                table_name_before = [string]$leftObject.table_name
                table_name_after = if ($null -ne $rightObject) { [string]$rightObject.table_name } else { $null }
                sql_sha256_before = [string]$leftObject.sql_sha256
                sql_sha256_after = if ($null -ne $rightObject) { [string]$rightObject.sql_sha256 } else { $null }
                equal = $null -ne $rightObject -and
                    [string]$leftObject.table_name -eq [string]$rightObject.table_name -and
                    [string]$leftObject.sql_sha256 -eq [string]$rightObject.sql_sha256
            }
        }
    )
    $allOldNonTableObjectsEqual = @($objectComparisons | Where-Object { -not $_.equal }).Count -eq 0
    $allTableRowsAndSchemasEqual = $comparisons.Count -gt 0 -and @($comparisons | Where-Object { -not $_.equal }).Count -eq 0
    return [ordered]@{
        excluded_tables = $excludedNames
        tables = $comparisons
        old_non_table_objects = $objectComparisons
        sqlite_automatic_objects_excluded = $true
        all_old_non_table_objects_equal = $allOldNonTableObjectsEqual
        required_nonempty_tables = $requiredChecks
        all_required_nonempty_and_equal = @($requiredChecks | Where-Object { -not $_.present_before_and_after -or -not $_.before_nonempty -or -not $_.unchanged }).Count -eq 0
        all_table_rows_and_schemas_equal = $allTableRowsAndSchemasEqual
        all_equal = $allTableRowsAndSchemasEqual -and $allOldNonTableObjectsEqual
    }
}

function Test-CandidateMigrationAudit {
    param([Parameter(Mandatory = $true)]$Audit)
    $requiredTables = [ordered]@{
        moss_candidate_segment_alignment = '9912ea13b1558d18380171fafd5e63d082fa11b775251f32eca57504a3a3fbe7'
        moss_run_diagnostics = 'a711327ab0154dc97860ffc82d01619d368276e871e9711864bda3c48462ac28'
        moss_audio_token_runs = '797aea33530d9375742356576251b840df066e48a6a3577599137d62310dca2b'
        moss_audio_token_source_chunks = '616076658e481f698f8922bf548e2b6e2ed8dfb1aee132967a86da668951341c'
        moss_audio_tokens = '8f9410a8957d0bc86ad7376572e9ba38275d0c63e18b3ee95615deed1a9c7172'
        moss_candidate_audio_token_boundary = '417ccaf096d9479db863114157980ba2053d0a74ee36740e8c66582efc51be7d'
        moss_machine_term_correction_source = 'aab106f9021cd05351b0eee2e2ac36631ccb7e81fc6b36daea640dfc6a83588e'
    }
    $tableChecks = @(
        foreach ($name in $requiredTables.Keys) {
            $table = Get-AuditTable -Audit $Audit -Name $name
            [ordered]@{
                table = $name
                exists = $null -ne $table
                expected_schema_sha256 = [string]$requiredTables[$name]
                actual_schema_sha256 = if ($null -ne $table) { [string]$table.schema_sha256 } else { $null }
                schema_sha256_matches = $null -ne $table -and [string]$table.schema_sha256 -eq [string]$requiredTables[$name]
            }
        }
    )
    $requiredVersions = @(20260831000000, 20260831010000)
    $migrationChecks = @(
        foreach ($version in $requiredVersions) {
            $migration = $Audit.migrations | Where-Object { [int64]$_.version -eq $version } | Select-Object -First 1
            [ordered]@{
                version = $version
                exists = $null -ne $migration
                success = $null -ne $migration -and [int]$migration.success -eq 1
            }
        }
    )
    $temporaryTable = Get-AuditTable -Audit $Audit -Name 'moss_candidate_segment_alignment_r4'
    $alignmentIndex = Get-AuditObject -Audit $Audit -Type 'index' -Name 'idx_moss_alignment_raw_segment'
    $expectedAlignmentIndexSha256 = '8c2acc79cca466b6989b8fec6fd45b65aaa9990861cb9179e015d559e8c57cab'
    return [ordered]@{
        tables = $tableChecks
        migrations = $migrationChecks
        temporary_alignment_table_absent = $null -eq $temporaryTable
        alignment_index = [ordered]@{
            exists = $null -ne $alignmentIndex
            expected_sql_sha256 = $expectedAlignmentIndexSha256
            actual_sql_sha256 = if ($null -ne $alignmentIndex) { [string]$alignmentIndex.sql_sha256 } else { $null }
            matches = $null -ne $alignmentIndex -and [string]$alignmentIndex.sql_sha256 -eq $expectedAlignmentIndexSha256
        }
        migration_count = @($Audit.migrations).Count
        last_migration_version = if (@($Audit.migrations).Count -gt 0) { [int64]@($Audit.migrations)[-1].version } else { $null }
        all_migrations_succeeded = @($Audit.migrations | Where-Object { [int]$_.success -ne 1 }).Count -eq 0
        passed = @($tableChecks | Where-Object { -not $_.exists -or -not $_.schema_sha256_matches }).Count -eq 0 -and
            @($migrationChecks | Where-Object { -not $_.exists -or -not $_.success }).Count -eq 0 -and
            @($Audit.migrations).Count -eq 16 -and [int64]@($Audit.migrations)[-1].version -eq 20260831010000 -and
            @($Audit.migrations | Where-Object { [int]$_.success -ne 1 }).Count -eq 0 -and
            $null -eq $temporaryTable -and $null -ne $alignmentIndex -and
            [string]$alignmentIndex.sql_sha256 -eq $expectedAlignmentIndexSha256
    }
}

function Invoke-VersionedDataTool {
    param(
        [ValidateSet('Verify', 'Restore')][string]$Mode,
        [Parameter(Mandatory = $true)][string]$ToolPath,
        [Parameter(Mandatory = $true)][string]$BackupDirectory,
        [Parameter(Mandatory = $true)][string]$Label
    )
    $resultPath = Join-Path $OutputRoot ($Label + '.result.private.json')
    $arguments = @(
        '-NoLogo', '-NoProfile', '-NonInteractive', '-ExecutionPolicy', 'Bypass',
        '-File', $ToolPath,
        '-Mode', $Mode,
        '-DataRoot', $testDataDirectory,
        '-BackupRoot', $testBackupRoot,
        '-BackupDirectory', $BackupDirectory,
        '-SourceVersion', $BaselineVersion,
        '-TargetVersion', $CandidateVersion,
        '-ResultPath', $resultPath
    )
    $run = Invoke-CapturedProcess -FilePath $WindowsPowerShellPath -Arguments $arguments -Label $Label -TimeoutSeconds 1200
    $record = if (Test-Path -LiteralPath $resultPath -PathType Leaf) { Get-Content -LiteralPath $resultPath -Raw -Encoding UTF8 | ConvertFrom-Json } else { $null }
    return [ordered]@{ run = $run; record = $record; evidence = Get-FileEvidence $resultPath }
}

function Invoke-VersionedDataContractSuite {
    $arguments = @(
        '-NoLogo', '-NoProfile', '-NonInteractive', '-ExecutionPolicy', 'Bypass',
        '-File', $VersionedDataTest,
        '-ToolPath', $rollbackToolSource,
        '-NsisHookPath', $NsisHook,
        '-NsisTemplatePath', $NsisTemplate,
        '-OutputRoot', $versionedDataTestRoot,
        '-FixtureRoot', $versionedDataFixtureRoot,
        '-RemoveFixtureRootAfterRun'
    )
    $run = Invoke-CapturedProcess -FilePath $WindowsPowerShellPath -Arguments $arguments -Label 'versioned-data-contract-and-fault-suite' -TimeoutSeconds 1200
    if (-not (Test-Path -LiteralPath $versionedDataUnitReportPath -PathType Leaf) -or
        -not (Test-Path -LiteralPath $versionedDataFaultReportPath -PathType Leaf)) {
        throw 'Versioned-data suite did not write both required reports.'
    }
    $unit = Get-Content -LiteralPath $versionedDataUnitReportPath -Raw -Encoding UTF8 | ConvertFrom-Json
    $fault = Get-Content -LiteralPath $versionedDataFaultReportPath -Raw -Encoding UTF8 | ConvertFrom-Json
    $getResult = {
        param([string]$Name)
        return @($unit.results | Where-Object { [string]$_.name -eq $Name }) | Select-Object -First 1
    }
    $validPreferences = & $getResult 'valid_baseline_recording_preferences_preserves_selected_directory'
    if ($null -eq $validPreferences) { $validPreferences = & $getResult 'valid_legacy_recording_preferences_preserves_selected_directory' }
    $corruptPreferences = & $getResult 'corrupt_recording_preferences_is_rejected_without_default_fallback'
    $insufficientSpace = & $getResult 'insufficient_disk_space_is_rejected_before_backup_mutation'
    $reparsePaths = & $getResult 'reparse_paths_are_rejected_without_external_mutation'
    $embeddedFixtureCleanup = & $getResult 'embedded_run_fixture_cleanup_contract'
    $requiredFaultPoints = @('BeforeOriginalMove', 'AfterOriginalMove', 'AfterReplacement', 'BeforeFinalVerification')
    $faultCases = @($fault.cases)
    $faultPointCoverage = @(
        foreach ($point in $requiredFaultPoints) {
            $matching = @($faultCases | Where-Object { [string]$_.failure_point -eq $point })
            [ordered]@{
                failure_point = $point
                observed_case_count = $matching.Count
                all_passed = $matching.Count -eq 2 -and @($matching | Where-Object { -not [bool]$_.passed }).Count -eq 0
            }
        }
    )
    $faultInjectionAllPass = $run.exit_code -eq 0 -and [string]$unit.verdict -eq 'PASS' -and
        [string]$fault.verdict -eq 'PASS' -and $faultCases.Count -eq 8 -and
        @($faultPointCoverage | Where-Object { -not $_.all_passed }).Count -eq 0
    $nestedJunction = @($reparsePaths.details | Where-Object {
        [string]$_.path_role -eq 'DataRoot nested entry' -and [bool]$_.passed -and [bool]$_.external_unchanged
    }).Count -eq 1
    return [ordered]@{
        run = $run
        unit_report = $unit
        unit_report_evidence = Get-FileEvidence $versionedDataUnitReportPath
        fault_report = $fault
        fault_report_evidence = Get-FileEvidence $versionedDataFaultReportPath
        fault_point_coverage = $faultPointCoverage
        fault_injection_all_pass = $faultInjectionAllPass
        valid_recording_preferences_preserved = $null -ne $validPreferences -and [string]$validPreferences.verdict -eq 'PASS'
        corrupt_recording_preferences_rejected = $null -ne $corruptPreferences -and [string]$corruptPreferences.verdict -eq 'PASS'
        insufficient_disk_space_rejected = $null -ne $insufficientSpace -and [string]$insufficientSpace.verdict -eq 'PASS'
        nested_junction_rejected_without_external_mutation = $nestedJunction
        embedded_fixture_root_removed = $null -ne $embeddedFixtureCleanup -and
            [string]$embeddedFixtureCleanup.verdict -eq 'PASS' -and -not (Test-Path -LiteralPath $versionedDataFixtureRoot)
    }
}

function Get-ValidatedInstallerBackup {
    param(
        [Parameter(Mandatory = $true)][string]$ResultPath,
        [Parameter(Mandatory = $true)]$ExpectedDataManifest,
        [Parameter(Mandatory = $true)][string]$Label
    )
    if (-not (Test-Path -LiteralPath $ResultPath -PathType Leaf)) { throw "$Label did not create its backup result file." }
    [void](Assert-NoReparsePath -Path $ResultPath -ExpectedRoot $testBackupRoot)
    $backupResult = Get-Content -LiteralPath $ResultPath -Raw -Encoding UTF8 | ConvertFrom-Json
    if ([string]$backupResult.status -ne 'PASS' -or
        [string]$backupResult.result.source_version -ne $BaselineVersion -or
        [string]$backupResult.result.target_version -ne $CandidateVersion) {
        throw "$Label wrote a failed or version-mismatched backup result."
    }
    $preferenceValidation = $backupResult.result.recording_preferences_validation
    if ([string]$preferenceValidation.status -ne 'VALID' -or
        [string]$preferenceValidation.layout -ne 'store-root' -or
        -not ([System.IO.Path]::GetFullPath([string]$preferenceValidation.selected_recording_root)).Equals(
            [System.IO.Path]::GetFullPath($recordingRoot), [System.StringComparison]::OrdinalIgnoreCase
        )) {
        throw "$Label did not validate and preserve the configured recording directory."
    }
    $spacePreflight = $backupResult.result.space_preflight
    if (-not [bool]$spacePreflight.passed -or [int64]$spacePreflight.available_bytes -lt [int64]$spacePreflight.required_bytes) {
        throw "$Label did not pass the real destination-volume free-space preflight."
    }
    $backupDirectory = [System.IO.Path]::GetFullPath([string]$backupResult.result.backup_directory)
    if (-not (Test-Path -LiteralPath $backupDirectory -PathType Container)) { throw "$Label backup directory is missing." }
    [void](Assert-NoReparsePath -Path $backupDirectory -ExpectedRoot $testBackupRoot)
    $backupManifestPath = [System.IO.Path]::GetFullPath((Join-Path $backupDirectory 'backup-manifest.json'))
    [void](Assert-NoReparsePath -Path $backupManifestPath -ExpectedRoot $testBackupRoot)
    $backupManifestEvidence = Get-FileEvidence $backupManifestPath
    if ([string]$backupResult.result.manifest_sha256 -ne [string]$backupManifestEvidence.sha256) {
        throw "$Label backup-result manifest SHA-256 does not match the actual manifest."
    }
    $backupManifest = Get-Content -LiteralPath $backupManifestPath -Raw -Encoding UTF8 | ConvertFrom-Json
    if ([string]$backupManifest.source_version -ne $BaselineVersion -or
        [string]$backupManifest.upgrade_target_version -ne $CandidateVersion) {
        throw "$Label backup manifest has the wrong version pair."
    }
    $trustedToolPath = [System.IO.Path]::GetFullPath((Join-Path $testBackupRoot 'tools\meetily-versioned-data.ps1'))
    [void](Assert-NoReparsePath -Path $trustedToolPath -ExpectedRoot $testBackupRoot)
    $trustedToolEvidence = Get-FileEvidence $trustedToolPath
    if (-not (Test-FileMatchesExpectedEvidence -Actual $trustedToolEvidence -Expected $candidateBuild.rollback_tool)) {
        throw "$Label trusted rollback tool does not match the approved source-tool hash."
    }
    $backupVerify = Invoke-VersionedDataTool -Mode Verify -ToolPath $trustedToolPath -BackupDirectory $backupDirectory -Label ("verify-$Label")
    if ($backupVerify.run.exit_code -ne 0 -or [string]$backupVerify.record.status -ne 'PASS') {
        throw "$Label backup did not pass independent Verify mode."
    }
    $backupMatchesData = Test-ManifestsEqual -Left $ExpectedDataManifest -Right @($backupManifest.files)
    if (-not $backupMatchesData) { throw "$Label backup payload does not match the complete pre-upgrade data manifest." }
    return [ordered]@{
        result = $backupResult
        result_evidence = Get-FileEvidence $ResultPath
        backup_directory = $backupDirectory
        manifest = $backupManifest
        manifest_evidence = $backupManifestEvidence
        verify = $backupVerify
        trusted_tool = $trustedToolEvidence
        matches_expected_data = $backupMatchesData
    }
}

function Move-TestDirectoryToEvidence {
    param([Parameter(Mandatory = $true)][string]$Source, [Parameter(Mandatory = $true)][string]$Label)
    if (-not (Test-Path -LiteralPath $Source -PathType Container)) { return $null }
    $resolvedSource = [System.IO.Path]::GetFullPath($Source)
    if ($resolvedSource -notin @($testDataDirectory, $testWebViewDirectory, $testBackupRoot)) {
        throw "Refusing to move an unexpected test directory: $resolvedSource"
    }
    $sourceInventory = Get-SafeDirectoryInventory -Root $resolvedSource
    $sourceManifest = @(Get-DirectoryManifest $resolvedSource)
    if (-not (Test-Path -LiteralPath $testArchiveRoot -PathType Container)) { New-Item -ItemType Directory -Path $testArchiveRoot -Force | Out-Null }
    $destination = [System.IO.Path]::GetFullPath((Join-Path $testArchiveRoot $Label))
    if (-not $destination.StartsWith($testArchiveRoot + [System.IO.Path]::DirectorySeparatorChar, [System.StringComparison]::OrdinalIgnoreCase)) {
        throw "Unsafe evidence destination: $destination"
    }
    if (Test-Path -LiteralPath $destination) { throw "Evidence destination already exists: $destination" }
    $staging = [System.IO.Path]::GetFullPath((Join-Path $testArchiveRoot ($Label + '.staging-' + [Guid]::NewGuid().ToString('N'))))
    if (-not (Test-StrictChildPath -Candidate $staging -Parent $testArchiveRoot)) { throw "Unsafe archive staging path: $staging" }
    New-Item -ItemType Directory -Path $staging | Out-Null
    foreach ($sourceDirectory in @($sourceInventory.directories)) {
        if ($sourceDirectory.Equals($resolvedSource, [System.StringComparison]::OrdinalIgnoreCase)) { continue }
        $relativeDirectory = Get-RelativePath -Root $resolvedSource -Path $sourceDirectory
        [void](New-Item -ItemType Directory -Path (Resolve-SafeRelativePath -Root $staging -RelativePath $relativeDirectory) -Force)
    }
    foreach ($file in @($sourceInventory.files)) {
        $relativeFile = Get-RelativePath -Root $resolvedSource -Path $file.FullName
        $targetFile = Resolve-SafeRelativePath -Root $staging -RelativePath $relativeFile
        $targetParent = Split-Path -Parent $targetFile
        if (-not (Test-Path -LiteralPath $targetParent -PathType Container)) { New-Item -ItemType Directory -Path $targetParent -Force | Out-Null }
        Copy-Item -LiteralPath $file.FullName -Destination $targetFile
    }
    $stagedManifest = @(Get-DirectoryManifest $staging)
    if (-not (Test-ManifestsEqual -Left $sourceManifest -Right $stagedManifest)) {
        throw "Archive staging copy did not match the source: $Label"
    }
    Move-Item -LiteralPath $staging -Destination $destination
    $destinationManifest = @(Get-DirectoryManifest $destination)
    $sourceManifestBeforeDelete = @(Get-DirectoryManifest $resolvedSource)
    if (-not (Test-ManifestsEqual -Left $sourceManifest -Right $destinationManifest) -or
        -not (Test-ManifestsEqual -Left $sourceManifest -Right $sourceManifestBeforeDelete)) {
        throw "Archive source changed before verified removal: $Label"
    }
    $deleteInventory = Get-SafeDirectoryInventory -Root $resolvedSource
    foreach ($file in @($deleteInventory.files)) { [System.IO.File]::Delete([string]$file.FullName) }
    foreach ($directory in @($deleteInventory.directories | Sort-Object { $_.Length } -Descending)) {
        $item = Get-Item -LiteralPath $directory -Force
        if (($item.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0) { throw "Archive removal refused a reparse point: $directory" }
        [System.IO.Directory]::Delete([string]$directory, $false)
    }
    return [ordered]@{
        source = $resolvedSource
        destination = $destination
        file_count = $sourceManifest.Count
        source_fingerprint = Get-ManifestFingerprint $sourceManifest
        destination_fingerprint = Get-ManifestFingerprint $destinationManifest
        copy_verified = Test-ManifestsEqual -Left $sourceManifest -Right $destinationManifest
        destination_exists = Test-Path -LiteralPath $destination -PathType Container
        source_removed = -not (Test-Path -LiteralPath $resolvedSource)
    }
}

function Test-ArchiveRecord {
    param($Record)
    return $null -ne $Record -and [bool]$Record.copy_verified -and [bool]$Record.destination_exists -and
        [bool]$Record.source_removed -and [string]$Record.source_fingerprint -eq [string]$Record.destination_fingerprint
}

function Test-FileHashEqual {
    param($Left, $Right)
    return $Left.exists -and $Right.exists -and [int64]$Left.bytes -eq [int64]$Right.bytes -and [string]$Left.sha256 -eq [string]$Right.sha256
}

function Test-JsonEquivalent {
    param($Left, $Right)
    if ($null -eq $Left -or $null -eq $Right) { return $null -eq $Left -and $null -eq $Right }
    return ($Left | ConvertTo-Json -Depth 20 -Compress) -eq ($Right | ConvertTo-Json -Depth 20 -Compress)
}

function Get-RecordingPreferencesEvidence {
    param([Parameter(Mandatory = $true)][string]$ExpectedSelectedRoot)
    $path = Join-Path $testDataDirectory 'recording_preferences.json'
    $file = Get-FileEvidence $path
    if (-not $file.exists) { throw 'recording_preferences.json is missing.' }
    try { $document = Get-Content -LiteralPath $path -Raw -Encoding UTF8 | ConvertFrom-Json -ErrorAction Stop } catch {
        throw "recording_preferences.json is not valid JSON: $($_.Exception.Message)"
    }
    $preferencesProperty = $document.PSObject.Properties['preferences']
    if ($null -eq $preferencesProperty -or $null -eq $preferencesProperty.Value) {
        throw 'recording_preferences.json does not contain the persisted preferences object.'
    }
    $preferences = $preferencesProperty.Value
    $selectedProperty = $preferences.PSObject.Properties['save_folder']
    if ($null -eq $selectedProperty -or $selectedProperty.Value -isnot [string] -or [string]::IsNullOrWhiteSpace([string]$selectedProperty.Value)) {
        throw 'recording_preferences.json does not contain a valid selected save folder.'
    }
    $selectedRoot = [System.IO.Path]::GetFullPath([string]$selectedProperty.Value)
    $expectedRoot = [System.IO.Path]::GetFullPath($ExpectedSelectedRoot)
    return [ordered]@{
        file = $file
        layout = 'store-root'
        selected_recording_root = $selectedRoot
        expected_selected_recording_root = $expectedRoot
        selected_recording_root_matches = $selectedRoot.Equals($expectedRoot, [System.StringComparison]::OrdinalIgnoreCase)
    }
}

$requiredFiles = @(
    $CandidateInstaller, $BaselineInstaller, $ProtectedBaselinePath, $FixtureDatabase, $FixtureManifest, $SqliteAudit,
    $PythonPath, $CandidateBuildManifest, $BaselineBuildManifest, $MeetingCheckScript,
    $FixtureSelectorScript, $FixtureSeedScript, $CdpExitScript, $NativeArgumentScript,
    $VersionedDataTest, $NsisHook, $NsisTemplate, $UninstallDataGuard, $NodePath, $WindowsPowerShellPath
)
foreach ($required in $requiredFiles) {
    if (-not (Test-Path -LiteralPath $required -PathType Leaf)) { throw "Required file is missing: $required" }
}
$qaSourceFiles = @(
    $MyInvocation.MyCommand.Path, $SqliteAudit, $MeetingCheckScript, $FixtureSelectorScript,
    $FixtureSeedScript, $CdpExitScript, $NativeArgumentScript, $VersionedDataTest,
    $NsisHook, $NsisTemplate, $UninstallDataGuard
)
foreach ($qaSourceFile in $qaSourceFiles) {
    if (-not (Test-StrictChildPath -Candidate $qaSourceFile -Parent $repoRoot)) { throw "QA source is outside the checked-out repository: $qaSourceFile" }
    [void](Assert-NoReparsePath -Path $qaSourceFile -ExpectedRoot $repoRoot)
}
$protectedBaseline = @(Get-Content -LiteralPath $ProtectedBaselinePath -Raw -Encoding UTF8 | ConvertFrom-Json)
$fixtureInput = Read-FixtureManifest -Path $FixtureManifest
Assert-IsolatedPathContract -ProtectedBaseline $protectedBaseline
if (Test-Path -LiteralPath $OutputRoot) {
    if (@(Get-ChildItem -LiteralPath $OutputRoot -Force).Count -ne 0) { throw "OutputRoot must be new or empty: $OutputRoot" }
} else {
    New-Item -ItemType Directory -Path $OutputRoot -Force | Out-Null
}
if (Test-Path -LiteralPath $PublicOutput) { throw "PublicOutput must not already exist: $PublicOutput" }
New-Item -ItemType Directory -Path $logRoot -Force | Out-Null
New-Item -ItemType Directory -Path $recordingRoot -Force | Out-Null

$protectedBefore = @(Get-ProtectedSnapshot $protectedBaseline)
if ($protectedBaseline.Count -ne 7 -or @($protectedBefore | Where-Object { -not $_.matches_baseline }).Count -ne 0) {
    throw 'Protected production user data does not match the seven-file frozen baseline.'
}
if ($null -ne (Get-TestRegistry) -or (Test-Path -LiteralPath $testInstallDirectory) -or (Test-Path -LiteralPath $testDataDirectory) -or
    (Test-Path -LiteralPath $testWebViewDirectory) -or (Test-Path -LiteralPath $testBackupRoot) -or @(Get-ExactTestProcesses).Count -ne 0) {
    throw 'Isolated lifecycle product is not clean before testing.'
}

$candidateEvidence = Get-FileEvidence $CandidateInstaller
$baselineEvidence = Get-FileEvidence $BaselineInstaller
$candidateBuild = Read-BuildManifest -Path $CandidateBuildManifest -ExpectedRole candidate -RequireRollbackTool
$baselineBuild = Read-BuildManifest -Path $BaselineBuildManifest -ExpectedRole baseline
if (-not (Test-FileMatchesExpectedEvidence -Actual $candidateEvidence -Expected $candidateBuild.installer)) {
    throw 'Candidate installer bytes or SHA-256 do not match the approved candidate build manifest.'
}
if (-not (Test-FileMatchesExpectedEvidence -Actual $baselineEvidence -Expected $baselineBuild.installer)) {
    throw 'Baseline installer bytes or SHA-256 do not match the approved baseline build manifest.'
}
$SourceCommit = ([string]$candidateBuild.source_commit).ToLowerInvariant()
$currentHead = [string](& git -C $repoRoot rev-parse HEAD 2>$null)
if ($LASTEXITCODE -ne 0 -or $currentHead.Trim().ToLowerInvariant() -ne $SourceCommit) {
    throw 'Candidate build source_commit does not match the checked-out Git HEAD.'
}
$workingTreeStatus = @(& git -C $repoRoot status --porcelain=v1 --untracked-files=all 2>$null)
if ($LASTEXITCODE -ne 0 -or $workingTreeStatus.Count -ne 0) {
    throw 'Lifecycle source checkout must have no tracked, staged, or untracked changes.'
}
$rollbackToolSource = Resolve-SafeRelativePath -Root $repoRoot -RelativePath ([string]$candidateBuild.rollback_tool.relative_path)
[void](Assert-NoReparsePath -Path $rollbackToolSource -ExpectedRoot $repoRoot)
$rollbackToolSourceEvidence = Get-FileEvidence $rollbackToolSource
if (-not (Test-FileMatchesExpectedEvidence -Actual $rollbackToolSourceEvidence -Expected $candidateBuild.rollback_tool)) {
    throw 'Checked-out rollback tool does not match the approved candidate build manifest.'
}
$fixtureBindingRun = Invoke-CapturedProcess -FilePath $PythonPath -Arguments @($FixtureSelectorScript, $FixtureDatabase, $fixtureSnapshotPath, $fixtureBindingPath) -Label 'select-protected-fixture' -TimeoutSeconds 60
if ($fixtureBindingRun.exit_code -ne 0 -or -not (Test-Path -LiteralPath $fixtureBindingPath -PathType Leaf)) {
    throw 'Could not bind the protected lifecycle meeting fixture.'
}
$fixtureBinding = Get-Content -LiteralPath $fixtureBindingPath -Raw -Encoding UTF8 | ConvertFrom-Json
if ([string]$fixtureBinding.snapshot_integrity_check -ne 'ok' -or
    [string]$fixtureBinding.snapshot_database_sha256 -ne (Get-FileHash -LiteralPath $fixtureSnapshotPath -Algorithm SHA256).Hash.ToUpperInvariant()) {
    throw 'The frozen lifecycle fixture snapshot failed integrity or SHA-256 binding.'
}
Assert-FixtureBindingMatchesManifest -Binding $fixtureBinding -FixtureInput $fixtureInput
$result = [ordered]@{
    schema_version = 2
    run_id = $RunId
    source_commit = $SourceCommit
    baseline_source_commit = ([string]$baselineBuild.source_commit).ToLowerInvariant()
    product_name = $ProductName
    bundle_id = $BundleId
    candidate_version = $CandidateVersion
    baseline_version = $BaselineVersion
    started_at = (Get-Date).ToString('o')
    candidate = $candidateEvidence
    baseline = $baselineEvidence
    candidate_build_manifest = Get-FileEvidence $CandidateBuildManifest
    baseline_build_manifest = Get-FileEvidence $BaselineBuildManifest
    rollback_tool_source = $rollbackToolSourceEvidence
    qa_source_files = @($qaSourceFiles | ForEach-Object { Get-FileEvidence $_ })
    runtimes = [ordered]@{
        python = Get-FileEvidence $PythonPath
        node = Get-FileEvidence $NodePath
        windows_powershell = Get-FileEvidence $WindowsPowerShellPath
    }
    candidate_signature = (Get-AuthenticodeSignature -LiteralPath $CandidateInstaller).Status.ToString()
    baseline_signature = (Get-AuthenticodeSignature -LiteralPath $BaselineInstaller).Status.ToString()
    fixture_binding_run = $fixtureBindingRun
    fixture_manifest = Get-FileEvidence $FixtureManifest
    fixture_input = $fixtureInput
    fixture_binding = $fixtureBinding
    fixture_binding_evidence = Get-FileEvidence $fixtureBindingPath
    fixture_snapshot_evidence = Get-FileEvidence $fixtureSnapshotPath
    protected_before = $protectedBefore
    lifecycle_labels = @(
        'fresh-install',
        'same-version-repair',
        'upgrade',
        'direct-downgrade-refused',
        'supported-rollback',
        'upgrade-after-rollback',
        'final-uninstall'
    )
    ui_follow_up = [ordered]@{
        label = 'interactive-install-ui-smoke'
        status = 'PENDING_FINAL_CANDIDATE_UI'
        must_run_after = 'final-uninstall'
        must_run_before = 'FT-26'
        scope = 'Final interactive installer flow and settings save-directory picker; not claimed by this silent producer.'
    }
    ft26_follow_up = [ordered]@{
        label = 'FT-26-reensure-candidate-and-verify-save-directory'
        status = 'PENDING_FINAL_CANDIDATE_UI'
        must_run_after = 'interactive-install-ui-smoke'
        scope = 'Reinstall the final candidate, choose a recording directory in real Windows UI, restart, and verify the persisted absolute path.'
    }
    stages = [ordered]@{}
    cleanup = [ordered]@{}
    status = 'RUNNING'
}

try {
    $versionedDataSuite = Invoke-VersionedDataContractSuite
    if (-not $versionedDataSuite.fault_injection_all_pass -or
        -not $versionedDataSuite.valid_recording_preferences_preserved -or
        -not $versionedDataSuite.corrupt_recording_preferences_rejected -or
        -not $versionedDataSuite.insufficient_disk_space_rejected -or
        -not $versionedDataSuite.nested_junction_rejected_without_external_mutation -or
        -not $versionedDataSuite.embedded_fixture_root_removed) {
        throw 'Versioned-data contract, boundary, or restore fault-injection suite failed.'
    }
    $result.stages.versioned_data_preflight = $versionedDataSuite

    $freshInstall = Invoke-TestInstaller -Installer $CandidateInstaller -Label 'fresh-install'
    $freshState = Get-InstalledState -BuildManifest $candidateBuild
    $freshBackupAbsentAfterInstall = -not (Test-Path -LiteralPath $testBackupRoot)
    $freshSmoke = Invoke-AppSmoke -Label 'fresh-candidate' -StableSeconds $CandidateStableSeconds -VerifyMeeting $false
    $freshInstallManifestBeforeRepair = @(Get-DirectoryManifest $testInstallDirectory)
    $freshDataManifestBeforeRepair = @(Get-DirectoryManifest $testDataDirectory)
    $freshBackupManifestBeforeRepair = @(Get-DirectoryManifest $testBackupRoot)
    $freshRegistryBeforeRepair = Get-TestRegistry
    $sameVersionRepair = Invoke-TestInstaller -Installer $CandidateInstaller -Label 'same-version-repair'
    $sameVersionRepairState = Get-InstalledState -BuildManifest $candidateBuild
    $sameVersionRepairInstallManifest = @(Get-DirectoryManifest $testInstallDirectory)
    $sameVersionRepairDataManifest = @(Get-DirectoryManifest $testDataDirectory)
    $sameVersionRepairBackupManifest = @(Get-DirectoryManifest $testBackupRoot)
    $sameVersionBackupAbsentAfterRepair = -not (Test-Path -LiteralPath $testBackupRoot)
    $sameVersionRepairRegistry = Get-TestRegistry
    $sameVersionRepairSmoke = Invoke-AppSmoke -Label 'same-version-candidate-after-repair' -StableSeconds $CandidateStableSeconds -VerifyMeeting $false
    $concurrentInstaller = Invoke-ConcurrentInstallerRefusal -BuildManifest $candidateBuild
    $freshUninstall = Invoke-TestUninstall -Label 'fresh-candidate-uninstall'
    $result.stages.fresh_install = [ordered]@{
        install = $freshInstall
        installed_state = $freshState
        backup_absent_after_fresh_install = $freshBackupAbsentAfterInstall
        smoke = $freshSmoke
        same_version_repair = $sameVersionRepair
        same_version_repair_state = $sameVersionRepairState
        same_version_repair_smoke = $sameVersionRepairSmoke
        same_version_install_unchanged = Test-ManifestsEqual -Left $freshInstallManifestBeforeRepair -Right $sameVersionRepairInstallManifest
        same_version_data_unchanged = Test-ManifestsEqual -Left $freshDataManifestBeforeRepair -Right $sameVersionRepairDataManifest
        same_version_backup_unchanged = Test-ManifestsEqual -Left $freshBackupManifestBeforeRepair -Right $sameVersionRepairBackupManifest
        same_version_backup_directory_absent = $sameVersionBackupAbsentAfterRepair
        same_version_registry_unchanged = Test-JsonEquivalent -Left $freshRegistryBeforeRepair -Right $sameVersionRepairRegistry
        concurrent_installer = $concurrentInstaller
        concurrent_installer_refused = $concurrentInstaller.concurrent_installer_refused
        uninstall_for_next_stage = $freshUninstall
    }
    $result.cleanup.fresh_data_archive = Move-TestDirectoryToEvidence -Source $testDataDirectory -Label 'fresh-install-appdata'
    $result.cleanup.fresh_webview_archive = Move-TestDirectoryToEvidence -Source $testWebViewDirectory -Label 'fresh-install-webview'
    $result.cleanup.fresh_backup_archive = Move-TestDirectoryToEvidence -Source $testBackupRoot -Label 'fresh-install-backups'

    $baselineInstall = Invoke-TestInstaller -Installer $BaselineInstaller -Label 'baseline-install'
    $baselineState = Get-InstalledState -BuildManifest $baselineBuild
    $baselineBackupAbsentAfterInstall = -not (Test-Path -LiteralPath $testBackupRoot)
    New-Item -ItemType Directory -Path (Join-Path $testDataDirectory 'templates') -Force | Out-Null
    Copy-Item -LiteralPath $fixtureSnapshotPath -Destination (Join-Path $testDataDirectory 'meeting_minutes.sqlite')
    Write-Utf8Text -Path (Join-Path $testDataDirectory 'lifecycle-marker.json') -Content ('{"run_id":"' + $RunId + '","must_survive":true}' + "`n")
    Write-Utf8Text -Path (Join-Path $testDataDirectory 'templates\functional-test-template.json') -Content ('{"id":"ft-install-template","name":"lifecycle-template","must_survive":true}' + "`n")
    $nestedModelMarkerPath = Join-Path $testDataDirectory 'models\lifecycle\nested\fixture-model\model-marker.json'
    Write-Utf8Text -Path $nestedModelMarkerPath -Content ('{"id":"ft-nested-model","run_id":"' + $RunId + '","must_survive":true}' + "`n")
    Write-Utf8Text -Path (Join-Path $testDataDirectory 'recording_preferences.json') -Content (([ordered]@{
        preferences = [ordered]@{
            save_folder = $recordingRoot
            auto_save = $true
            file_format = 'mp4'
            preferred_mic_device = $null
            preferred_system_device = $null
        }
    } | ConvertTo-Json -Depth 5) + "`n")
    Write-Utf8Text -Path (Join-Path $testDataDirectory 'ui-locale.json') -Content (([ordered]@{ preference = 'zh-CN'; locale = 'zh-CN' } | ConvertTo-Json) + "`n")
    $baselineMigrationSmoke = Invoke-AppSmoke -Label 'baseline-migrate-fixture' -StableSeconds 10 -VerifyMeeting $true
    if (-not $baselineMigrationSmoke.passed) { throw 'Baseline application could not migrate and open the frozen fixture.' }
    $fixtureSeedRun = Invoke-CapturedProcess -FilePath $PythonPath -Arguments @($FixtureSeedScript, (Join-Path $testDataDirectory 'meeting_minutes.sqlite'), $fixtureBindingPath, $fixtureSeedResultPath) -Label 'seed-preservation-records' -TimeoutSeconds 60
    if ($fixtureSeedRun.exit_code -ne 0 -or -not (Test-Path -LiteralPath $fixtureSeedResultPath -PathType Leaf)) {
        throw 'Could not seed non-empty manual-revision and MOSS preservation records.'
    }
    $fixtureSeedResult = Get-Content -LiteralPath $fixtureSeedResultPath -Raw -Encoding UTF8 | ConvertFrom-Json
    if ([string]$fixtureSeedResult.status -ne 'PASS' -or [string]$fixtureSeedResult.integrity_check -ne 'ok') {
        throw 'Preservation sentinel seeding did not pass integrity checks.'
    }
    $baselineSmoke = Invoke-AppSmoke -Label 'baseline-before-upgrade' -StableSeconds 10 -VerifyMeeting $true
    $databaseBeforeUpgrade = Invoke-DatabaseAudit -Label 'before-upgrade'
    $baselineDataManifest = @(Get-DirectoryManifest $testDataDirectory)
    $baselineNonDatabaseManifest = @(Get-NonDatabaseDataManifest)
    $markerBefore = Get-FileEvidence (Join-Path $testDataDirectory 'lifecycle-marker.json')
    $templateBefore = Get-FileEvidence (Join-Path $testDataDirectory 'templates\functional-test-template.json')
    $recordingPreferencesBefore = Get-FileEvidence (Join-Path $testDataDirectory 'recording_preferences.json')
    $recordingPreferencesSelectionBefore = Get-RecordingPreferencesEvidence -ExpectedSelectedRoot $recordingRoot
    $localeBefore = Get-FileEvidence (Join-Path $testDataDirectory 'ui-locale.json')
    $nestedModelMarkerBefore = Get-FileEvidence $nestedModelMarkerPath

    $upgradeInstall = Invoke-TestInstaller -Installer $CandidateInstaller -Label 'upgrade'
    $backupResultPath = Join-Path $testBackupRoot 'last-upgrade-backup-result.json'
    $firstBackup = Get-ValidatedInstallerBackup -ResultPath $backupResultPath -ExpectedDataManifest $baselineDataManifest -Label 'first-upgrade-backup'
    $backupDirectory = [string]$firstBackup.backup_directory
    $trustedToolPath = [string]$firstBackup.trusted_tool.path
    $upgradeState = Get-InstalledState -BuildManifest $candidateBuild
    $upgradeSmoke = Invoke-AppSmoke -Label 'candidate-after-upgrade' -StableSeconds $CandidateStableSeconds -VerifyMeeting $true
    $databaseAfterUpgrade = Invoke-DatabaseAudit -Label 'after-upgrade'
    $upgradeCritical = Compare-CriticalDatabaseTables -Before $databaseBeforeUpgrade.report -After $databaseAfterUpgrade.report
    $upgradeMigrationState = Test-CandidateMigrationAudit -Audit $databaseAfterUpgrade.report
    $nonDatabaseManifestAfterUpgrade = @(Get-NonDatabaseDataManifest)
    $upgradeNonDatabasePreservation = Compare-PreservedFileManifest -Before $baselineNonDatabaseManifest -After $nonDatabaseManifestAfterUpgrade
    $upgradePreservedAllNonDatabaseFiles = [bool]$upgradeNonDatabasePreservation.all_before_files_preserved
    $result.stages.upgrade = [ordered]@{
        baseline_install = $baselineInstall
        baseline_installed_state = $baselineState
        baseline_backup_absent_after_fresh_install = $baselineBackupAbsentAfterInstall
        fixture_seed_run = $fixtureSeedRun
        fixture_seed_result = $fixtureSeedResult
        fixture_seed_evidence = Get-FileEvidence $fixtureSeedResultPath
        baseline_migration_smoke = $baselineMigrationSmoke
        baseline_smoke = $baselineSmoke
        database_before = $databaseBeforeUpgrade
        baseline_data_manifest = $baselineDataManifest
        baseline_data_fingerprint = Get-ManifestFingerprint $baselineDataManifest
        non_database_manifest_before = $baselineNonDatabaseManifest
        non_database_manifest_after = $nonDatabaseManifestAfterUpgrade
        non_database_file_preservation = $upgradeNonDatabasePreservation
        all_non_database_files_preserved = $upgradePreservedAllNonDatabaseFiles
        marker_before = $markerBefore
        template_before = $templateBefore
        recording_preferences_before = $recordingPreferencesBefore
        recording_preferences_selection_before = $recordingPreferencesSelectionBefore
        locale_before = $localeBefore
        nested_model_marker_before = $nestedModelMarkerBefore
        candidate_install = $upgradeInstall
        backup = $firstBackup
        backup_result = $firstBackup.result
        backup_result_evidence = $firstBackup.result_evidence
        backup_manifest_evidence = $firstBackup.manifest_evidence
        backup_verify = $firstBackup.verify
        backup_matches_baseline = $firstBackup.matches_expected_data
        trusted_tool = $firstBackup.trusted_tool
        installed_state = $upgradeState
        candidate_smoke = $upgradeSmoke
        database_after = $databaseAfterUpgrade
        critical_database_tables = $upgradeCritical
        candidate_migration_state = $upgradeMigrationState
        marker_after = Get-FileEvidence (Join-Path $testDataDirectory 'lifecycle-marker.json')
        template_after = Get-FileEvidence (Join-Path $testDataDirectory 'templates\functional-test-template.json')
        recording_preferences_after = Get-FileEvidence (Join-Path $testDataDirectory 'recording_preferences.json')
        recording_preferences_selection_after = Get-RecordingPreferencesEvidence -ExpectedSelectedRoot $recordingRoot
        locale_after = Get-FileEvidence (Join-Path $testDataDirectory 'ui-locale.json')
        nested_model_marker_after = Get-FileEvidence $nestedModelMarkerPath
    }

    $installManifestBeforeDowngrade = @(Get-DirectoryManifest $testInstallDirectory)
    $dataManifestBeforeDowngrade = @(Get-DirectoryManifest $testDataDirectory)
    $backupManifestBeforeDowngrade = @(Get-DirectoryManifest $testBackupRoot)
    $registryBeforeDowngrade = Get-TestRegistry
    $registryTreeBeforeDowngrade = Get-UninstallRegistryTreeSnapshot
    $webViewTreeBeforeDowngrade = Get-DirectoryTreeSnapshot -Root $testWebViewDirectory
    $shortcutsBeforeDowngrade = @(Get-ProductShortcutSnapshot)
    $processesBeforeDowngrade = @(Get-NormalizedTestProcesses)
    $directDowngradeCdpPort = if ($null -ne $upgradeSmoke.cdp_binding) { [int]$upgradeSmoke.cdp_binding.port } else { 0 }
    $cdpListenersBeforeDowngrade = @(Get-TestCdpListenerSnapshot -Port $directDowngradeCdpPort)
    $stateBeforeDowngrade = Get-InstalledState -BuildManifest $candidateBuild
    $blockedDowngrade = Invoke-TestInstaller -Installer $BaselineInstaller -Label 'direct-downgrade-refused'
    $stateAfterDowngrade = Get-InstalledState -BuildManifest $candidateBuild
    $installManifestAfterDowngrade = @(Get-DirectoryManifest $testInstallDirectory)
    $dataManifestAfterDowngrade = @(Get-DirectoryManifest $testDataDirectory)
    $backupManifestAfterDowngrade = @(Get-DirectoryManifest $testBackupRoot)
    $registryAfterDowngrade = Get-TestRegistry
    $registryTreeAfterDowngrade = Get-UninstallRegistryTreeSnapshot
    $webViewTreeAfterDowngrade = Get-DirectoryTreeSnapshot -Root $testWebViewDirectory
    $shortcutsAfterDowngrade = @(Get-ProductShortcutSnapshot)
    $processesAfterDowngrade = @(Get-NormalizedTestProcesses)
    $cdpListenersAfterDowngrade = @(Get-TestCdpListenerSnapshot -Port $directDowngradeCdpPort)
    $directInstallUnchanged = Test-ManifestsEqual $installManifestBeforeDowngrade $installManifestAfterDowngrade
    $directDataUnchanged = Test-ManifestsEqual $dataManifestBeforeDowngrade $dataManifestAfterDowngrade
    $directBackupUnchanged = Test-ManifestsEqual $backupManifestBeforeDowngrade $backupManifestAfterDowngrade
    $directRegistryUnchanged = Test-JsonEquivalent -Left $registryBeforeDowngrade -Right $registryAfterDowngrade
    $directRegistryTreeUnchanged = Test-JsonEquivalent -Left $registryTreeBeforeDowngrade -Right $registryTreeAfterDowngrade
    $directRegistryTreePresent = [bool]$registryTreeBeforeDowngrade.exists -and @($registryTreeBeforeDowngrade.keys).Count -gt 0
    $directWebViewTreeUnchanged = Test-DirectoryTreeSnapshotsEqual -Left $webViewTreeBeforeDowngrade -Right $webViewTreeAfterDowngrade
    $directWebViewTreePresent = [bool]$webViewTreeBeforeDowngrade.exists -and @($webViewTreeBeforeDowngrade.files).Count -gt 0
    $directShortcutsUnchanged = Test-JsonEquivalent -Left $shortcutsBeforeDowngrade -Right $shortcutsAfterDowngrade
    $directRequiredShortcutsPresent = @($shortcutsBeforeDowngrade | Where-Object { $_.root_role -eq 'desktop' }).Count -gt 0 -and
        @($shortcutsBeforeDowngrade | Where-Object { $_.root_role -eq 'start_menu' }).Count -gt 0
    $directProcessesUnchanged = Test-JsonEquivalent -Left $processesBeforeDowngrade -Right $processesAfterDowngrade
    $directCdpListenersUnchanged = Test-JsonEquivalent -Left $cdpListenersBeforeDowngrade -Right $cdpListenersAfterDowngrade

    Write-JsonFile -Path $downgradeRefusalObservationPath -Value ([ordered]@{
        label = 'direct-downgrade-refused'
        observed_at = (Get-Date).ToString('o')
        installer = Get-PublicFileEvidence (Get-FileEvidence $BaselineInstaller)
        expected_exit_code = 3
        actual_exit_code = $blockedDowngrade.exit_code
        captured_stdout = Get-PublicFileEvidence $blockedDowngrade.stdout
        captured_stderr = Get-PublicFileEvidence $blockedDowngrade.stderr
        install_fingerprint_before = Get-ManifestFingerprint $installManifestBeforeDowngrade
        install_fingerprint_after = Get-ManifestFingerprint $installManifestAfterDowngrade
        data_fingerprint_before = Get-ManifestFingerprint $dataManifestBeforeDowngrade
        data_fingerprint_after = Get-ManifestFingerprint $dataManifestAfterDowngrade
        install_data_backup_registry_webview_shortcuts_processes_and_cdp_unchanged = $directInstallUnchanged -and
            $directDataUnchanged -and $directBackupUnchanged -and $directRegistryTreeUnchanged -and
            $directWebViewTreeUnchanged -and $directShortcutsUnchanged -and $directProcessesUnchanged -and
            $directCdpListenersUnchanged
    })
    $downgradeRefusalObservation = Get-FileEvidence $downgradeRefusalObservationPath
    $downgradeRefusalLogged = $blockedDowngrade.exit_code -eq 3 -and $downgradeRefusalObservation.exists -and
        [int64]$downgradeRefusalObservation.bytes -gt 0

    $dataManifestBeforeCandidateRemoval = @(Get-DirectoryManifest $testDataDirectory)
    $newDataTreeBeforeOldVersion = Get-DirectoryTreeSnapshot -Root $testDataDirectory
    $removeCandidate = Invoke-TestUninstall -Label 'remove-candidate-for-supported-rollback'
    $dataManifestAfterCandidateRemoval = @(Get-DirectoryManifest $testDataDirectory)
    $candidateUninstallPreservedAllData = Test-ManifestsEqual -Left $dataManifestBeforeCandidateRemoval -Right $dataManifestAfterCandidateRemoval
    $oldVersionInstall = Invoke-TestInstaller -Installer $BaselineInstaller -Label 'old-version-against-new-data-install'
    $oldVersionInstalledState = Get-InstalledState -BuildManifest $baselineBuild
    $oldVersionAgainstNewData = Invoke-ExpectedEarlyAppExit -Label 'old-version-against-new-data'
    $newDataTreeAfterOldVersion = Get-DirectoryTreeSnapshot -Root $testDataDirectory
    $oldVersionRejectsNewDataExitNonzero = $oldVersionAgainstNewData.exit_code -ne 0
    $oldVersionRejectsNewDataExitCode101 = $oldVersionAgainstNewData.exit_code -eq 101
    $newDataRecursiveManifestUnchanged = Test-DirectoryTreeSnapshotsEqual -Left $newDataTreeBeforeOldVersion -Right $newDataTreeAfterOldVersion

    $recordingPreferencePath = Join-Path $testDataDirectory 'recording_preferences.json'
    $validNewPreferenceText = Get-Content -LiteralPath $recordingPreferencePath -Raw -Encoding UTF8
    $validNewPreferenceEvidence = Get-FileEvidence $recordingPreferencePath
    Write-Utf8Text -Path $recordingPreferencePath -Content '{"preferences":{"save_folder":7,"auto_save":true,"file_format":"mp4"}}'
    $corruptDataTreeBeforeUpgrade = Get-DirectoryTreeSnapshot -Root $testDataDirectory
    $corruptPreferenceBeforeUpgrade = Get-FileEvidence $recordingPreferencePath
    $corruptPreferenceUpgrade = Invoke-TestInstaller -Installer $CandidateInstaller -Label 'corrupt-recording-preferences-upgrade-refusal'
    $stateAfterCorruptPreferenceRejection = Get-InstalledState -BuildManifest $baselineBuild
    $corruptDataTreeAfterUpgrade = Get-DirectoryTreeSnapshot -Root $testDataDirectory
    $corruptPreferenceAfterUpgrade = Get-FileEvidence $recordingPreferencePath
    $corruptRecordingPreferencesRejected = $corruptPreferenceUpgrade.exit_code -eq 24 -and
        $stateAfterCorruptPreferenceRejection.registry.display_version -eq $BaselineVersion -and
        $stateAfterCorruptPreferenceRejection.all_required_files_match_manifest -and
        (Test-DirectoryTreeSnapshotsEqual -Left $corruptDataTreeBeforeUpgrade -Right $corruptDataTreeAfterUpgrade) -and
        (Test-FileHashEqual -Left $corruptPreferenceBeforeUpgrade -Right $corruptPreferenceAfterUpgrade)
    Write-Utf8Text -Path $recordingPreferencePath -Content $validNewPreferenceText
    $newDataTreeAfterCorruptFixtureCleanup = Get-DirectoryTreeSnapshot -Root $testDataDirectory
    $corruptFixtureRestoredWithoutMutation = (Test-DirectoryTreeSnapshotsEqual -Left $newDataTreeBeforeOldVersion -Right $newDataTreeAfterCorruptFixtureCleanup) -and
        (Test-FileHashEqual -Left $validNewPreferenceEvidence -Right (Get-FileEvidence $recordingPreferencePath))
    $removeOldVersionAfterNewDataProbe = Invoke-TestUninstall -Label 'remove-old-version-after-new-data-probe'

    $restore = Invoke-VersionedDataTool -Mode Restore -ToolPath $trustedToolPath -BackupDirectory $backupDirectory -Label 'supported-rollback-data-restore'
    $restoredDataManifest = @(Get-DirectoryManifest $testDataDirectory)
    $restoredExactly = Test-ManifestsEqual $baselineDataManifest $restoredDataManifest
    $rollbackInstall = Invoke-TestInstaller -Installer $BaselineInstaller -Label 'supported-rollback'
    $rollbackState = Get-InstalledState -BuildManifest $baselineBuild
    $rollbackSmoke = Invoke-AppSmoke -Label 'baseline-after-supported-restore' -StableSeconds $OldVersionStableSeconds -VerifyMeeting $true
    $databaseAfterRollback = Invoke-DatabaseAudit -Label 'after-supported-rollback'
    $rollbackCritical = Compare-CriticalDatabaseTables -Before $databaseBeforeUpgrade.report -After $databaseAfterRollback.report
    $nonDatabaseManifestAfterRollback = @(Get-NonDatabaseDataManifest)
    $rollbackNonDatabasePreservation = Compare-PreservedFileManifest -Before $baselineNonDatabaseManifest -After $nonDatabaseManifestAfterRollback
    $rollbackPreservedAllNonDatabaseFiles = [bool]$rollbackNonDatabasePreservation.all_before_files_preserved
    $markerAfterRollback = Get-FileEvidence (Join-Path $testDataDirectory 'lifecycle-marker.json')
    $templateAfterRollback = Get-FileEvidence (Join-Path $testDataDirectory 'templates\functional-test-template.json')
    $recordingPreferencesAfterRollback = Get-FileEvidence (Join-Path $testDataDirectory 'recording_preferences.json')
    $recordingPreferencesSelectionAfterRollback = Get-RecordingPreferencesEvidence -ExpectedSelectedRoot $recordingRoot
    $localeAfterRollback = Get-FileEvidence (Join-Path $testDataDirectory 'ui-locale.json')
    $nestedModelMarkerAfterRollback = Get-FileEvidence $nestedModelMarkerPath

    $databaseImmediatelyBeforeSecondUpgrade = Invoke-DatabaseAudit -Label 'immediately-before-second-upgrade'
    $secondUpgradePreflightCritical = Compare-CriticalDatabaseTables -Before $databaseBeforeUpgrade.report -After $databaseImmediatelyBeforeSecondUpgrade.report
    $secondUpgradeDataManifest = @(Get-DirectoryManifest $testDataDirectory)
    $nonDatabaseManifestBeforeSecondUpgrade = @(Get-NonDatabaseDataManifest)
    $backupRootManifestBeforeSecondUpgrade = @(Get-DirectoryManifest $testBackupRoot)
    $upgradeAgainInstall = Invoke-TestInstaller -Installer $CandidateInstaller -Label 'upgrade-after-rollback'
    $secondBackup = Get-ValidatedInstallerBackup -ResultPath $backupResultPath -ExpectedDataManifest $secondUpgradeDataManifest -Label 'second-upgrade-backup'
    $secondBackupIsNew = -not ([System.IO.Path]::GetFullPath([string]$secondBackup.backup_directory)).Equals(
        [System.IO.Path]::GetFullPath($backupDirectory), [System.StringComparison]::OrdinalIgnoreCase
    )
    $backupRootManifestAfterSecondUpgrade = @(Get-DirectoryManifest $testBackupRoot)
    $secondUpgradeChangedBackupRoot = -not (Test-ManifestsEqual -Left $backupRootManifestBeforeSecondUpgrade -Right $backupRootManifestAfterSecondUpgrade)
    $upgradeAgainState = Get-InstalledState -BuildManifest $candidateBuild
    $upgradeAgainSmoke = Invoke-AppSmoke -Label 'candidate-after-second-upgrade' -StableSeconds $CandidateStableSeconds -VerifyMeeting $true
    $databaseAfterSecondUpgrade = Invoke-DatabaseAudit -Label 'after-second-upgrade'
    $secondUpgradeCritical = Compare-CriticalDatabaseTables -Before $databaseBeforeUpgrade.report -After $databaseAfterSecondUpgrade.report
    $secondUpgradeMigrationState = Test-CandidateMigrationAudit -Audit $databaseAfterSecondUpgrade.report
    $nonDatabaseManifestAfterSecondUpgrade = @(Get-NonDatabaseDataManifest)
    $secondUpgradeNonDatabasePreservation = Compare-PreservedFileManifest -Before $nonDatabaseManifestBeforeSecondUpgrade -After $nonDatabaseManifestAfterSecondUpgrade
    $secondUpgradePreservedAllNonDatabaseFiles = [bool]$secondUpgradeNonDatabasePreservation.all_before_files_preserved
    $nestedModelMarkerAfterSecondUpgrade = Get-FileEvidence $nestedModelMarkerPath
    $recordingPreferencesSelectionAfterSecondUpgrade = Get-RecordingPreferencesEvidence -ExpectedSelectedRoot $recordingRoot
    $result.stages.rollback = [ordered]@{
        state_before_direct_downgrade = $stateBeforeDowngrade
        blocked_direct_downgrade = $blockedDowngrade
        state_after_direct_downgrade = $stateAfterDowngrade
        direct_install_unchanged = $directInstallUnchanged
        direct_data_unchanged = $directDataUnchanged
        direct_backup_unchanged = $directBackupUnchanged
        direct_registry_unchanged = $directRegistryUnchanged
        direct_recursive_registry_tree_before = $registryTreeBeforeDowngrade
        direct_recursive_registry_tree_after = $registryTreeAfterDowngrade
        direct_recursive_registry_tree_unchanged = $directRegistryTreeUnchanged
        direct_recursive_registry_tree_present = $directRegistryTreePresent
        direct_webview_tree_before = $webViewTreeBeforeDowngrade
        direct_webview_tree_after = $webViewTreeAfterDowngrade
        direct_webview_tree_unchanged = $directWebViewTreeUnchanged
        direct_webview_tree_present = $directWebViewTreePresent
        direct_shortcuts_before = $shortcutsBeforeDowngrade
        direct_shortcuts_after = $shortcutsAfterDowngrade
        direct_shortcuts_unchanged = $directShortcutsUnchanged
        direct_required_desktop_and_start_menu_shortcuts_present = $directRequiredShortcutsPresent
        direct_processes_before = $processesBeforeDowngrade
        direct_processes_after = $processesAfterDowngrade
        direct_processes_unchanged = $directProcessesUnchanged
        direct_cdp_port = $directDowngradeCdpPort
        direct_cdp_listeners_before = $cdpListenersBeforeDowngrade
        direct_cdp_listeners_after = $cdpListenersAfterDowngrade
        direct_cdp_listeners_unchanged = $directCdpListenersUnchanged
        downgrade_refusal_logged = $downgradeRefusalLogged
        downgrade_refusal_observation = $downgradeRefusalObservation
        install_fingerprint_before = Get-ManifestFingerprint $installManifestBeforeDowngrade
        install_fingerprint_after = Get-ManifestFingerprint $installManifestAfterDowngrade
        data_fingerprint_before = Get-ManifestFingerprint $dataManifestBeforeDowngrade
        data_fingerprint_after = Get-ManifestFingerprint $dataManifestAfterDowngrade
        remove_candidate = $removeCandidate
        uninstall_data_manifest_before = $dataManifestBeforeCandidateRemoval
        uninstall_data_manifest_after = $dataManifestAfterCandidateRemoval
        uninstall_preserved_complete_data = $candidateUninstallPreservedAllData
        old_version_against_new_data_install = $oldVersionInstall
        old_version_against_new_data_state = $oldVersionInstalledState
        old_version_against_new_data = $oldVersionAgainstNewData
        old_version_rejects_new_data_exit_nonzero = $oldVersionRejectsNewDataExitNonzero
        old_version_rejects_new_data_exit_code_101 = $oldVersionRejectsNewDataExitCode101
        new_data_recursive_manifest_before = $newDataTreeBeforeOldVersion
        new_data_recursive_manifest_after = $newDataTreeAfterOldVersion
        new_data_recursive_manifest_unchanged = $newDataRecursiveManifestUnchanged
        corrupt_preference_upgrade = $corruptPreferenceUpgrade
        corrupt_recording_preferences_rejected = $corruptRecordingPreferencesRejected
        corrupt_fixture_restored_without_mutation = $corruptFixtureRestoredWithoutMutation
        state_after_corrupt_preference_rejection = $stateAfterCorruptPreferenceRejection
        remove_old_version_after_new_data_probe = $removeOldVersionAfterNewDataProbe
        restore = $restore
        restored_data_matches_baseline = $restoredExactly
        rollback_install = $rollbackInstall
        rollback_state = $rollbackState
        rollback_smoke = $rollbackSmoke
        database_after_rollback = $databaseAfterRollback
        critical_database_tables = $rollbackCritical
        non_database_manifest_before_rollback = $baselineNonDatabaseManifest
        non_database_manifest_after_rollback = $nonDatabaseManifestAfterRollback
        rollback_non_database_file_preservation = $rollbackNonDatabasePreservation
        rollback_preserved_all_non_database_files = $rollbackPreservedAllNonDatabaseFiles
        marker_after_rollback = $markerAfterRollback
        template_after_rollback = $templateAfterRollback
        recording_preferences_after_rollback = $recordingPreferencesAfterRollback
        recording_preferences_selection_after_rollback = $recordingPreferencesSelectionAfterRollback
        locale_after_rollback = $localeAfterRollback
        nested_model_marker_after_rollback = $nestedModelMarkerAfterRollback
        upgrade_again_install = $upgradeAgainInstall
        database_immediately_before_second_upgrade = $databaseImmediatelyBeforeSecondUpgrade
        second_upgrade_preflight_critical_database_tables = $secondUpgradePreflightCritical
        non_database_manifest_before_second_upgrade = $nonDatabaseManifestBeforeSecondUpgrade
        second_upgrade_backup = $secondBackup
        second_upgrade_backup_is_new = $secondBackupIsNew
        second_upgrade_changed_backup_root = $secondUpgradeChangedBackupRoot
        upgrade_again_state = $upgradeAgainState
        upgrade_again_smoke = $upgradeAgainSmoke
        database_after_second_upgrade = $databaseAfterSecondUpgrade
        second_upgrade_critical_database_tables = $secondUpgradeCritical
        second_upgrade_migration_state = $secondUpgradeMigrationState
        non_database_manifest_after_second_upgrade = $nonDatabaseManifestAfterSecondUpgrade
        second_upgrade_non_database_file_preservation = $secondUpgradeNonDatabasePreservation
        second_upgrade_preserved_all_non_database_files = $secondUpgradePreservedAllNonDatabaseFiles
        nested_model_marker_after_second_upgrade = $nestedModelMarkerAfterSecondUpgrade
        recording_preferences_selection_after_second_upgrade = $recordingPreferencesSelectionAfterSecondUpgrade
    }

    $finalUninstall = Invoke-TestUninstall -Label 'final-uninstall'
    $result.cleanup.final_uninstall = $finalUninstall
    $result.cleanup.final_data_archive = Move-TestDirectoryToEvidence -Source $testDataDirectory -Label 'final-appdata'
    $result.cleanup.final_webview_archive = Move-TestDirectoryToEvidence -Source $testWebViewDirectory -Label 'final-webview'
    $result.cleanup.final_backup_archive = Move-TestDirectoryToEvidence -Source $testBackupRoot -Label 'final-rollback-backups'
    $result.status = 'EXECUTED'
} catch {
    $result.status = 'FAIL_EXECUTION'
    $result.error = $_.Exception.Message
    $result.failed_at = (Get-Date).ToString('o')
} finally {
    $result.cleanup.step_errors = [ordered]@{}
    try { Stop-ExactTestProcesses } catch { $result.cleanup.step_errors.stop_processes = $_.Exception.Message }
    try {
        if (Test-Path -LiteralPath (Join-Path $testInstallDirectory 'uninstall.exe') -PathType Leaf) {
            $result.cleanup.emergency_uninstall = Invoke-TestUninstall -Label ('emergency-uninstall-' + (Get-Date -Format 'yyyyMMddHHmmss'))
        }
    } catch { $result.cleanup.step_errors.uninstall = $_.Exception.Message }
    try {
        if (Test-Path -LiteralPath $testDataDirectory -PathType Container) {
            $result.cleanup.emergency_data_archive = Move-TestDirectoryToEvidence -Source $testDataDirectory -Label ('emergency-appdata-' + (Get-Date -Format 'yyyyMMddHHmmss'))
        }
    } catch { $result.cleanup.step_errors.archive_data = $_.Exception.Message }
    try {
        if (Test-Path -LiteralPath $testWebViewDirectory -PathType Container) {
            $result.cleanup.emergency_webview_archive = Move-TestDirectoryToEvidence -Source $testWebViewDirectory -Label ('emergency-webview-' + (Get-Date -Format 'yyyyMMddHHmmss'))
        }
    } catch { $result.cleanup.step_errors.archive_webview = $_.Exception.Message }
    try {
        if (Test-Path -LiteralPath $testBackupRoot -PathType Container) {
            $result.cleanup.emergency_backup_archive = Move-TestDirectoryToEvidence -Source $testBackupRoot -Label ('emergency-rollback-backups-' + (Get-Date -Format 'yyyyMMddHHmmss'))
        }
    } catch { $result.cleanup.step_errors.archive_backups = $_.Exception.Message }
    try {
        $protectedAfter = @(Get-ProtectedSnapshot $protectedBaseline)
    } catch {
        $protectedAfter = @()
        $result.cleanup.step_errors.protected_snapshot = $_.Exception.Message
    }
    $result.protected_after = $protectedAfter
    try { $result.cleanup.final_registry_absent = -not (Test-Path -LiteralPath $testRegistryPath) } catch { $result.cleanup.final_registry_absent = $false; $result.cleanup.step_errors.check_registry = $_.Exception.Message }
    try { $result.cleanup.final_install_directory_absent = -not (Test-Path -LiteralPath $testInstallDirectory) } catch { $result.cleanup.final_install_directory_absent = $false; $result.cleanup.step_errors.check_install_directory = $_.Exception.Message }
    try { $result.cleanup.final_data_directory_absent = -not (Test-Path -LiteralPath $testDataDirectory) } catch { $result.cleanup.final_data_directory_absent = $false; $result.cleanup.step_errors.check_data_directory = $_.Exception.Message }
    try { $result.cleanup.final_webview_directory_absent = -not (Test-Path -LiteralPath $testWebViewDirectory) } catch { $result.cleanup.final_webview_directory_absent = $false; $result.cleanup.step_errors.check_webview_directory = $_.Exception.Message }
    try { $result.cleanup.final_backup_directory_absent = -not (Test-Path -LiteralPath $testBackupRoot) } catch { $result.cleanup.final_backup_directory_absent = $false; $result.cleanup.step_errors.check_backup_directory = $_.Exception.Message }
    try { $result.cleanup.final_process_count = @(Get-ExactTestProcesses).Count } catch { $result.cleanup.final_process_count = -1; $result.cleanup.step_errors.check_processes = $_.Exception.Message }
    try { $result.cleanup.final_product_shortcut_count = @(Get-ProductShortcutSnapshot).Count } catch { $result.cleanup.final_product_shortcut_count = -1; $result.cleanup.step_errors.check_shortcuts = $_.Exception.Message }
    $result.cleanup.protected_user_data_unchanged = $protectedAfter.Count -eq 7 -and @($protectedAfter | Where-Object { -not $_.matches_baseline }).Count -eq 0
    $result.completed_at = (Get-Date).ToString('o')
    Write-JsonFile -Value $result -Path $privateResultPath
}

$archiveRecords = @(
    foreach ($entry in $result.cleanup.GetEnumerator()) {
        if ([string]$entry.Key -like '*_archive' -and $null -ne $entry.Value) { $entry.Value }
    }
)
$allArchiveRecordsVerified = $archiveRecords.Count -eq 0 -or @($archiveRecords | Where-Object { -not (Test-ArchiveRecord $_) }).Count -eq 0
$finalArchiveOk = $result.cleanup.Contains('final_data_archive') -and (Test-ArchiveRecord $result.cleanup.final_data_archive) -and
    $result.cleanup.Contains('final_webview_archive') -and (Test-ArchiveRecord $result.cleanup.final_webview_archive) -and
    $result.cleanup.Contains('final_backup_archive') -and (Test-ArchiveRecord $result.cleanup.final_backup_archive)
$environmentCleanupOk = $result.cleanup.final_registry_absent -and $result.cleanup.final_install_directory_absent -and
    $result.cleanup.final_data_directory_absent -and $result.cleanup.final_webview_directory_absent -and
    $result.cleanup.final_backup_directory_absent -and $result.cleanup.final_process_count -eq 0 -and
    $result.cleanup.final_product_shortcut_count -eq 0 -and
    $result.cleanup.protected_user_data_unchanged
$cleanupStepErrorsAbsent = $result.cleanup.step_errors.Count -eq 0
$cleanupOk = $environmentCleanupOk -and $cleanupStepErrorsAbsent -and $allArchiveRecordsVerified -and ($result.status -ne 'EXECUTED' -or $finalArchiveOk)

$evidenceManifestPath = Join-Path $OutputRoot 'evidence-files.private.json'
$evidenceFiles = @(Get-DirectoryManifest $OutputRoot | Where-Object { $_.relative_path -ne 'evidence-files.private.json' })
$evidenceManifestDocument = [ordered]@{
    schema_version = 1
    run_id = $RunId
    source_commit = $SourceCommit
    generated_at = (Get-Date).ToString('o')
    excluded_self = 'evidence-files.private.json'
    file_count = $evidenceFiles.Count
    files_fingerprint_sha256 = Get-ManifestFingerprint $evidenceFiles
    files = $evidenceFiles
}
Write-JsonFile -Value $evidenceManifestDocument -Path $evidenceManifestPath
$evidenceManifestEvidence = Get-FileEvidence $evidenceManifestPath

if ($result.status -eq 'FAIL_EXECUTION') {
    $publicFailure = [ordered]@{
        schema_version = 2
        run_id = $RunId
        source_commit = $SourceCommit
        product_name = $ProductName
        bundle_id = $BundleId
        status = 'FAIL_EXECUTION'
        error_code = 'LIFECYCLE_EXECUTION_FAILED'
        cleanup = [ordered]@{
            status = if ($cleanupOk) { 'PASS' } else { 'FAIL' }
            isolated_registry_absent = $result.cleanup.final_registry_absent
            isolated_install_directory_absent = $result.cleanup.final_install_directory_absent
            isolated_live_data_directory_absent = $result.cleanup.final_data_directory_absent
            isolated_webview_directory_absent = $result.cleanup.final_webview_directory_absent
            isolated_backup_directory_absent = $result.cleanup.final_backup_directory_absent
            residual_process_count = $result.cleanup.final_process_count
            residual_product_shortcut_count = $result.cleanup.final_product_shortcut_count
            production_user_data_unchanged = $result.cleanup.protected_user_data_unchanged
            archive_records_verified = $allArchiveRecordsVerified
            failed_step_codes = @($result.cleanup.step_errors.Keys)
        }
        private_result = Get-PublicFileEvidence (Get-FileEvidence $privateResultPath)
        private_evidence_manifest = Get-PublicFileEvidence $evidenceManifestEvidence
        completed_at = $result.completed_at
    }
    Write-JsonFile -Value $publicFailure -Path $PublicOutput
    $publicFailure | ConvertTo-Json -Depth 15
    exit 1
}

$fresh = $result.stages.fresh_install
$upgrade = $result.stages.upgrade
$rollback = $result.stages.rollback
$preflight = $result.stages.versioned_data_preflight
$ft22 = $fresh.install.exit_code -eq 0 -and $fresh.installed_state.registry.display_version -eq $CandidateVersion -and
    $fresh.installed_state.executable_product_version -eq $CandidateVersion -and
    $fresh.installed_state.all_required_files_exist -and $fresh.installed_state.all_required_files_match_manifest -and
    $fresh.backup_absent_after_fresh_install -and $fresh.smoke.passed -and
    $fresh.same_version_repair.exit_code -eq 0 -and
    $fresh.same_version_repair_state.registry.display_version -eq $CandidateVersion -and
    $fresh.same_version_repair_state.executable_product_version -eq $CandidateVersion -and
    $fresh.same_version_repair_state.all_required_files_match_manifest -and $fresh.same_version_repair_smoke.passed -and
    $fresh.same_version_install_unchanged -and $fresh.same_version_data_unchanged -and
    $fresh.same_version_backup_unchanged -and $fresh.same_version_backup_directory_absent -and $fresh.same_version_registry_unchanged -and
    $fresh.concurrent_installer_refused -and $fresh.concurrent_installer.second_installer.exit_code -eq 1618 -and
    $fresh.concurrent_installer.product_state_unchanged
$validRecordingPreferencesPreserved = $preflight.valid_recording_preferences_preserved -and
    [string]$upgrade.backup_result.result.recording_preferences_validation.status -eq 'VALID' -and
    [bool]$upgrade.backup_result.result.space_preflight.passed -and
    [string]$rollback.second_upgrade_backup.result.result.recording_preferences_validation.status -eq 'VALID' -and
    [bool]$rollback.second_upgrade_backup.result.result.space_preflight.passed -and
    $upgrade.recording_preferences_selection_before.selected_recording_root_matches -and
    $upgrade.recording_preferences_selection_after.selected_recording_root_matches -and
    $rollback.recording_preferences_selection_after_rollback.selected_recording_root_matches -and
    $rollback.recording_preferences_selection_after_second_upgrade.selected_recording_root_matches -and
    (Test-FileHashEqual $upgrade.recording_preferences_before $upgrade.recording_preferences_after) -and
    (Test-FileHashEqual $upgrade.recording_preferences_before $rollback.recording_preferences_after_rollback)
$ft23 = $upgrade.baseline_install.exit_code -eq 0 -and $upgrade.baseline_migration_smoke.passed -and $upgrade.baseline_smoke.passed -and
    $upgrade.baseline_backup_absent_after_fresh_install -and $upgrade.baseline_installed_state.all_required_files_match_manifest -and
    $upgrade.candidate_install.exit_code -eq 0 -and $upgrade.installed_state.registry.display_version -eq $CandidateVersion -and
    $upgrade.installed_state.executable_product_version -eq $CandidateVersion -and $upgrade.installed_state.all_required_files_match_manifest -and
    $upgrade.backup_result.status -eq 'PASS' -and $upgrade.backup_result.result.source_version -eq $BaselineVersion -and
    $upgrade.backup_result.result.target_version -eq $CandidateVersion -and $upgrade.backup_verify.run.exit_code -eq 0 -and
    $upgrade.backup_verify.record.status -eq 'PASS' -and $upgrade.backup_matches_baseline -and $upgrade.candidate_smoke.passed -and
    $upgrade.database_before.report.integrity -eq 'ok' -and $upgrade.database_after.report.integrity -eq 'ok' -and
    $upgrade.critical_database_tables.all_equal -and $upgrade.critical_database_tables.all_required_nonempty_and_equal -and
    $upgrade.candidate_migration_state.passed -and $upgrade.all_non_database_files_preserved -and
    (Test-FileHashEqual $upgrade.marker_before $upgrade.marker_after) -and
    (Test-FileHashEqual $upgrade.template_before $upgrade.template_after) -and
    (Test-FileHashEqual $upgrade.recording_preferences_before $upgrade.recording_preferences_after) -and
    (Test-FileHashEqual $upgrade.locale_before $upgrade.locale_after) -and
    (Test-FileHashEqual $upgrade.nested_model_marker_before $upgrade.nested_model_marker_after) -and
    $validRecordingPreferencesPreserved
$ft24 = $preflight.fault_injection_all_pass -and $preflight.insufficient_disk_space_rejected -and
    $preflight.nested_junction_rejected_without_external_mutation -and
    $rollback.remove_candidate.exit_code -eq 0 -and $rollback.remove_candidate.install_directory_removed -and
    $rollback.remove_candidate.registry_removed -and $rollback.remove_candidate.data_directory_preserved -and
    $rollback.uninstall_preserved_complete_data -and @($rollback.remove_candidate.active_processes).Count -eq 0
$oldVersionStarts = $rollback.restore.run.exit_code -eq 0 -and $rollback.restore.record.status -eq 'PASS' -and
    $rollback.restored_data_matches_baseline -and $rollback.rollback_install.exit_code -eq 0 -and
    $rollback.rollback_state.registry.display_version -eq $BaselineVersion -and $rollback.rollback_smoke.passed -and
    $rollback.rollback_state.executable_product_version -eq $BaselineVersion -and $rollback.rollback_state.all_required_files_match_manifest -and
    $rollback.rollback_smoke.required_stable_seconds -ge 30 -and $rollback.rollback_smoke.meeting_check_passed -and
    $rollback.database_after_rollback.report.integrity -eq 'ok' -and $rollback.critical_database_tables.all_equal -and
    $rollback.critical_database_tables.all_required_nonempty_and_equal -and
    $rollback.rollback_preserved_all_non_database_files -and
    (Test-FileHashEqual $upgrade.marker_before $rollback.marker_after_rollback) -and
    (Test-FileHashEqual $upgrade.template_before $rollback.template_after_rollback) -and
    (Test-FileHashEqual $upgrade.recording_preferences_before $rollback.recording_preferences_after_rollback) -and
    (Test-FileHashEqual $upgrade.locale_before $rollback.locale_after_rollback) -and
    (Test-FileHashEqual $upgrade.nested_model_marker_before $rollback.nested_model_marker_after_rollback)
$upgradeAfterRollback = $rollback.upgrade_again_install.exit_code -eq 0 -and
    $rollback.upgrade_again_state.registry.display_version -eq $CandidateVersion -and $rollback.upgrade_again_smoke.passed -and
    $rollback.upgrade_again_state.executable_product_version -eq $CandidateVersion -and $rollback.upgrade_again_state.all_required_files_match_manifest -and
    $rollback.second_upgrade_backup.result.status -eq 'PASS' -and
    $rollback.second_upgrade_backup.verify.run.exit_code -eq 0 -and $rollback.second_upgrade_backup.verify.record.status -eq 'PASS' -and
    $rollback.second_upgrade_backup.matches_expected_data -and $rollback.second_upgrade_backup_is_new -and
    $rollback.second_upgrade_changed_backup_root -and
    $rollback.database_immediately_before_second_upgrade.report.integrity -eq 'ok' -and
    $rollback.second_upgrade_preflight_critical_database_tables.all_equal -and
    $rollback.database_after_second_upgrade.report.integrity -eq 'ok' -and
    $rollback.second_upgrade_critical_database_tables.all_equal -and
    $rollback.second_upgrade_critical_database_tables.all_required_nonempty_and_equal -and
    $rollback.second_upgrade_migration_state.passed -and $rollback.second_upgrade_preserved_all_non_database_files -and
    (Test-FileHashEqual $upgrade.nested_model_marker_before $rollback.nested_model_marker_after_second_upgrade)
$ft25 = $rollback.old_version_rejects_new_data_exit_nonzero -and $rollback.new_data_recursive_manifest_unchanged -and
    $rollback.old_version_rejects_new_data_exit_code_101 -and $rollback.downgrade_refusal_logged -and
    $rollback.corrupt_recording_preferences_rejected -and $rollback.corrupt_fixture_restored_without_mutation -and
    $preflight.corrupt_recording_preferences_rejected -and
    $rollback.old_version_against_new_data_install.exit_code -eq 0 -and
    $rollback.old_version_against_new_data_state.all_required_files_match_manifest -and
    $rollback.old_version_against_new_data.no_residual_product_processes -and
    $rollback.remove_old_version_after_new_data_probe.exit_code -eq 0 -and
    $rollback.remove_old_version_after_new_data_probe.install_directory_removed -and
    $rollback.blocked_direct_downgrade.exit_code -eq 3 -and
    $rollback.state_before_direct_downgrade.registry.display_version -eq $CandidateVersion -and
    $rollback.state_after_direct_downgrade.registry.display_version -eq $CandidateVersion -and
    $rollback.direct_install_unchanged -and $rollback.direct_data_unchanged -and
    $rollback.direct_backup_unchanged -and $rollback.direct_registry_unchanged -and
    $rollback.direct_recursive_registry_tree_unchanged -and $rollback.direct_recursive_registry_tree_present -and
    $rollback.direct_webview_tree_unchanged -and $rollback.direct_webview_tree_present -and
    $rollback.direct_shortcuts_unchanged -and $rollback.direct_required_desktop_and_start_menu_shortcuts_present -and
    $rollback.direct_processes_unchanged -and $rollback.direct_cdp_listeners_unchanged -and
    @($rollback.direct_processes_before).Count -eq 0 -and @($rollback.direct_processes_after).Count -eq 0 -and
    @($rollback.direct_cdp_listeners_before).Count -eq 0 -and @($rollback.direct_cdp_listeners_after).Count -eq 0 -and
    $oldVersionStarts -and $upgradeAfterRollback

$manualBefore = Get-AuditTable -Audit $upgrade.database_before.report -Name 'summary_manual_revisions'
$manualAfter = Get-AuditTable -Audit $rollback.database_after_rollback.report -Name 'summary_manual_revisions'
$public = [ordered]@{
    schema_version = 2
    run_id = $RunId
    source_commit = $SourceCommit
    product_name = $ProductName
    bundle_id = $BundleId
    candidate = [ordered]@{
        version = $CandidateVersion
        source_commit = ([string]$candidateBuild.source_commit).ToLowerInvariant()
        evidence = Get-PublicFileEvidence $candidateEvidence
        build_manifest = Get-PublicFileEvidence $result.candidate_build_manifest
        signature = $result.candidate_signature
    }
    baseline = [ordered]@{
        version = $BaselineVersion
        source_commit = ([string]$baselineBuild.source_commit).ToLowerInvariant()
        evidence = Get-PublicFileEvidence $baselineEvidence
        build_manifest = Get-PublicFileEvidence $result.baseline_build_manifest
        signature = $result.baseline_signature
    }
    protected_fixture = [ordered]@{
        transcript_count = [int64]$result.fixture_binding.transcript_count
        transcript_fingerprint_sha256 = [string]$result.fixture_binding.transcript_fingerprint_sha256
        source_database_sha256 = [string]$result.fixture_binding.source_database_sha256
        snapshot_database_sha256 = [string]$result.fixture_binding.snapshot_database_sha256
        binding_evidence = Get-PublicFileEvidence $result.fixture_binding_evidence
    }
    lifecycle_labels = $result.lifecycle_labels
    ui_follow_up = $result.ui_follow_up
    ft26_follow_up = $result.ft26_follow_up
    lifecycle_execution = [ordered]@{
        fresh_install = [ordered]@{ label = $fresh.install.label; exit_code = $fresh.install.exit_code }
        same_version_repair = [ordered]@{ label = $fresh.same_version_repair.label; exit_code = $fresh.same_version_repair.exit_code }
        upgrade = [ordered]@{ label = $upgrade.candidate_install.label; exit_code = $upgrade.candidate_install.exit_code }
        direct_downgrade_refused = [ordered]@{ label = $rollback.blocked_direct_downgrade.label; exit_code = $rollback.blocked_direct_downgrade.exit_code }
        supported_rollback = [ordered]@{
            label = $rollback.rollback_install.label
            data_restore_exit_code = $rollback.restore.run.exit_code
            install_exit_code = $rollback.rollback_install.exit_code
        }
        upgrade_after_rollback = [ordered]@{ label = $rollback.upgrade_again_install.label; exit_code = $rollback.upgrade_again_install.exit_code }
        final_uninstall = [ordered]@{ label = $result.cleanup.final_uninstall.label; exit_code = $result.cleanup.final_uninstall.exit_code }
    }
    cases = [ordered]@{
        FT_22 = [ordered]@{
            status = if ($ft22) { 'PASS' } else { 'FAIL' }
            install_exit_code = $fresh.install.exit_code
            registered_version = $fresh.installed_state.registry.display_version
            executable_product_version = $fresh.installed_state.executable_product_version
            required_files_present = $fresh.installed_state.all_required_files_exist
            required_file_hashes_match_build_manifest = $fresh.installed_state.all_required_files_match_manifest
            rollback_backup_absent_after_fresh_install = $fresh.backup_absent_after_fresh_install
            app_stable_and_responding = $fresh.smoke.passed
            same_version_repair_exit_code = $fresh.same_version_repair.exit_code
            same_version_repair_kept_install_data_backup_and_registry_unchanged = $fresh.same_version_install_unchanged -and
                $fresh.same_version_data_unchanged -and $fresh.same_version_backup_unchanged -and $fresh.same_version_registry_unchanged
            same_version_repair_created_no_backup_directory = $fresh.same_version_backup_directory_absent
            app_starts_after_same_version_repair = $fresh.same_version_repair_smoke.passed
            second_installer_concurrent_exit_code = $fresh.concurrent_installer.second_installer.exit_code
            concurrent_installer_refused = $fresh.concurrent_installer_refused
            product_state_unchanged_by_concurrent_refusal = $fresh.concurrent_installer.product_state_unchanged
        }
        FT_23 = [ordered]@{
            status = if ($ft23) { 'PASS' } else { 'FAIL' }
            baseline_install_exit_code = $upgrade.baseline_install.exit_code
            upgrade_exit_code = $upgrade.candidate_install.exit_code
            registered_version_after_upgrade = $upgrade.installed_state.registry.display_version
            executable_product_version_after_upgrade = $upgrade.installed_state.executable_product_version
            required_file_hashes_match_build_manifest = $upgrade.installed_state.all_required_files_match_manifest
            baseline_fresh_install_created_no_rollback_backup = $upgrade.baseline_backup_absent_after_fresh_install
            installer_backup_status = $upgrade.backup_result.status
            backup_source_version = $upgrade.backup_result.result.source_version
            backup_target_version = $upgrade.backup_result.result.target_version
            backup_manifest_sha256 = $upgrade.backup_result.result.manifest_sha256
            backup_matches_complete_pre_upgrade_data = $upgrade.backup_matches_baseline
            database_integrity_before = $upgrade.database_before.report.integrity
            database_integrity_after = $upgrade.database_after.report.integrity
            critical_database_rows_unchanged = $upgrade.critical_database_tables.all_equal
            seeded_manual_and_moss_records_nonempty_and_unchanged = $upgrade.critical_database_tables.all_required_nonempty_and_equal
            candidate_migrations_and_new_moss_tables_present = $upgrade.candidate_migration_state.passed
            all_non_database_files_preserved = $upgrade.all_non_database_files_preserved
            marker_unchanged = Test-FileHashEqual $upgrade.marker_before $upgrade.marker_after
            template_unchanged = Test-FileHashEqual $upgrade.template_before $upgrade.template_after
            settings_unchanged = (Test-FileHashEqual $upgrade.recording_preferences_before $upgrade.recording_preferences_after) -and (Test-FileHashEqual $upgrade.locale_before $upgrade.locale_after)
            nested_model_marker_unchanged = Test-FileHashEqual $upgrade.nested_model_marker_before $upgrade.nested_model_marker_after
            valid_recording_preferences_preserved = $validRecordingPreferencesPreserved
            selected_recording_root = $upgrade.recording_preferences_selection_after.selected_recording_root
            installer_recording_preference_validation = $upgrade.backup_result.result.recording_preferences_validation
            installer_real_disk_space_preflight = $upgrade.backup_result.result.space_preflight
        }
        FT_24 = [ordered]@{
            status = if ($ft24) { 'PASS' } else { 'FAIL' }
            uninstall_exit_code = $rollback.remove_candidate.exit_code
            program_files_removed = $rollback.remove_candidate.install_directory_removed
            registry_removed = $rollback.remove_candidate.registry_removed
            product_data_preserved = $rollback.remove_candidate.data_directory_preserved
            complete_data_manifest_unchanged = $rollback.uninstall_preserved_complete_data
            residual_process_count = @($rollback.remove_candidate.active_processes).Count
            fault_injection_all_pass = $preflight.fault_injection_all_pass
            restore_fault_point_coverage = $preflight.fault_point_coverage
            insufficient_disk_space_rejected = $preflight.insufficient_disk_space_rejected
            nested_junction_rejected_without_external_mutation = $preflight.nested_junction_rejected_without_external_mutation
            fault_injection_report = Get-PublicFileEvidence $preflight.fault_report_evidence
        }
        FT_25 = [ordered]@{
            status = if ($ft25) { 'PASS' } else { 'FAIL' }
            blocked_direct_downgrade_exit_code = $rollback.blocked_direct_downgrade.exit_code
            downgrade_refusal_logged = $rollback.downgrade_refusal_logged
            downgrade_refusal_observation = Get-PublicFileEvidence $rollback.downgrade_refusal_observation
            version_after_blocked_downgrade = $rollback.state_after_direct_downgrade.registry.display_version
            executable_product_version_after_blocked_downgrade = $rollback.state_after_direct_downgrade.executable_product_version
            current_install_unchanged = $rollback.direct_install_unchanged
            current_data_unchanged = $rollback.direct_data_unchanged
            rollback_backups_unchanged_by_blocked_downgrade = $rollback.direct_backup_unchanged
            exact_uninstall_registry_unchanged_by_blocked_downgrade = $rollback.direct_registry_unchanged
            recursive_uninstall_registry_values_types_and_subkeys_unchanged = $rollback.direct_recursive_registry_tree_unchanged
            recursive_uninstall_registry_snapshot_present = $rollback.direct_recursive_registry_tree_present
            webview_tree_unchanged_by_blocked_downgrade = $rollback.direct_webview_tree_unchanged
            webview_tree_snapshot_present = $rollback.direct_webview_tree_present
            desktop_and_start_menu_shortcuts_unchanged_by_blocked_downgrade = $rollback.direct_shortcuts_unchanged
            required_desktop_and_start_menu_shortcuts_present = $rollback.direct_required_desktop_and_start_menu_shortcuts_present
            product_processes_unchanged_by_blocked_downgrade = $rollback.direct_processes_unchanged
            cdp_listener_state_unchanged_by_blocked_downgrade = $rollback.direct_cdp_listeners_unchanged
            old_version_against_new_data_install_exit_code = $rollback.old_version_against_new_data_install.exit_code
            old_version_rejects_new_data_exit_nonzero = $rollback.old_version_rejects_new_data_exit_nonzero
            old_version_rejects_new_data_exit_code_101 = $rollback.old_version_rejects_new_data_exit_code_101
            new_data_recursive_manifest_unchanged = $rollback.new_data_recursive_manifest_unchanged
            corrupt_recording_preferences_rejected = $rollback.corrupt_recording_preferences_rejected -and $preflight.corrupt_recording_preferences_rejected
            corrupt_fixture_restored_without_mutation = $rollback.corrupt_fixture_restored_without_mutation
            supported_restore_exit_code = $rollback.restore.run.exit_code
            restored_data_exactly_matches_backup_source = $rollback.restored_data_matches_baseline
            old_version_starts_after_supported_restore = $oldVersionStarts
            old_version_executable_product_version = $rollback.rollback_state.executable_product_version
            old_version_stable_seconds = $rollback.rollback_smoke.required_stable_seconds
            protected_meeting_opened = $rollback.rollback_smoke.meeting_check_passed
            database_integrity_after_supported_restore = $rollback.database_after_rollback.report.integrity
            critical_database_rows_unchanged = $rollback.critical_database_tables.all_equal
            seeded_manual_and_moss_records_nonempty_and_unchanged = $rollback.critical_database_tables.all_required_nonempty_and_equal
            manual_revision_count_before = if ($null -ne $manualBefore) { [int64]$manualBefore.row_count } else { $null }
            manual_revision_count_after = if ($null -ne $manualAfter) { [int64]$manualAfter.row_count } else { $null }
            template_unchanged = Test-FileHashEqual $upgrade.template_before $rollback.template_after_rollback
            settings_unchanged = (Test-FileHashEqual $upgrade.recording_preferences_before $rollback.recording_preferences_after_rollback) -and (Test-FileHashEqual $upgrade.locale_before $rollback.locale_after_rollback)
            all_non_database_files_preserved_after_rollback = $rollback.rollback_preserved_all_non_database_files
            nested_model_marker_preserved_after_rollback = Test-FileHashEqual $upgrade.nested_model_marker_before $rollback.nested_model_marker_after_rollback
            upgrade_after_rollback_still_works = $upgradeAfterRollback
            second_upgrade_executable_product_version = $rollback.upgrade_again_state.executable_product_version
            second_upgrade_created_new_verified_backup = $rollback.second_upgrade_backup_is_new -and
                $rollback.second_upgrade_backup.matches_expected_data -and $rollback.second_upgrade_backup.verify.record.status -eq 'PASS'
            second_upgrade_migrations_and_new_moss_tables_present = $rollback.second_upgrade_migration_state.passed
            immediate_pre_second_upgrade_database_audit_passed = $rollback.database_immediately_before_second_upgrade.report.integrity -eq 'ok' -and
                $rollback.second_upgrade_preflight_critical_database_tables.all_equal
            all_non_database_files_preserved_after_second_upgrade = $rollback.second_upgrade_preserved_all_non_database_files
            nested_model_marker_preserved_after_second_upgrade = Test-FileHashEqual $upgrade.nested_model_marker_before $rollback.nested_model_marker_after_second_upgrade
        }
    }
    cleanup = [ordered]@{
        status = if ($cleanupOk) { 'PASS' } else { 'FAIL' }
        isolated_registry_absent = $result.cleanup.final_registry_absent
        isolated_install_directory_absent = $result.cleanup.final_install_directory_absent
        isolated_live_data_directory_absent = $result.cleanup.final_data_directory_absent
        isolated_webview_directory_absent = $result.cleanup.final_webview_directory_absent
        isolated_backup_directory_absent = $result.cleanup.final_backup_directory_absent
        residual_process_count = $result.cleanup.final_process_count
        residual_product_shortcut_count = $result.cleanup.final_product_shortcut_count
        production_user_data_count = $result.protected_after.Count
        production_user_data_unchanged = $result.cleanup.protected_user_data_unchanged
        archive_records_verified = $allArchiveRecordsVerified
        isolated_test_data_moved_to_private_evidence = $finalArchiveOk
        failed_step_codes = @($result.cleanup.step_errors.Keys)
    }
    private_result = Get-PublicFileEvidence (Get-FileEvidence $privateResultPath)
    private_evidence_manifest = Get-PublicFileEvidence $evidenceManifestEvidence
    functional_status = if ($ft22 -and $ft23 -and $ft24 -and $ft25 -and $cleanupOk) { 'PASS' } else { 'FAIL' }
    signature_status = [ordered]@{
        candidate = $result.candidate_signature
        baseline = $result.baseline_signature
        signed_release_claim = $result.candidate_signature -eq 'Valid'
    }
    completed_at = $result.completed_at
}
Write-JsonFile -Value $public -Path $PublicOutput
$public | ConvertTo-Json -Depth 25
if ($public.functional_status -ne 'PASS') { exit 1 }
exit 0

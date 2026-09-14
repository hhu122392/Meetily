[CmdletBinding()]
param(
    [string]$OutputRoot = (Join-Path ([System.IO.Path]::GetTempPath()) ('meetily-build-manifest-contract-' + [Guid]::NewGuid().ToString('N')))
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'

$target = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot 'new-install-build-manifest.ps1'))
$buildWrapper = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot 'build-moss-functional-candidate.ps1'))
$OutputRoot = [System.IO.Path]::GetFullPath($OutputRoot)
$results = New-Object System.Collections.Generic.List[object]
$originalAppData = $env:APPDATA
$originalLocalAppData = $env:LOCALAPPDATA

function Add-Result {
    param([Parameter(Mandatory = $true)][string]$Name, [Parameter(Mandatory = $true)][bool]$Passed, [string]$Detail = '')
    $results.Add([ordered]@{ name = $Name; status = if ($Passed) { 'PASS' } else { 'FAIL' }; detail = $Detail })
}

function Invoke-ExpectedFailure {
    param([Parameter(Mandatory = $true)][scriptblock]$Action, [Parameter(Mandatory = $true)][string]$Pattern)
    try {
        & $Action | Out-Null
        return [ordered]@{ passed = $false; detail = 'command unexpectedly succeeded' }
    } catch {
        $message = [string]$_.Exception.Message
        return [ordered]@{ passed = ($message -match $Pattern); detail = $message }
    }
}

function Get-TestFileRecord {
    param([Parameter(Mandatory = $true)][string]$Path, [Parameter(Mandatory = $true)][string]$RelativePath, [string]$Role)
    $record = [ordered]@{
        relative_path = $RelativePath.Replace('\', '/')
        bytes = [int64](Get-Item -LiteralPath $Path).Length
        sha256 = (Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash.ToUpperInvariant()
    }
    if (-not [string]::IsNullOrWhiteSpace($Role)) { $record['role'] = $Role }
    return $record
}

try {
    [System.IO.Directory]::CreateDirectory($OutputRoot) | Out-Null
    $env:APPDATA = Join-Path $OutputRoot 'appdata'
    $env:LOCALAPPDATA = Join-Path $OutputRoot 'localappdata'
    [System.IO.Directory]::CreateDirectory($env:APPDATA) | Out-Null
    [System.IO.Directory]::CreateDirectory($env:LOCALAPPDATA) | Out-Null
    $dummyInstaller = Join-Path $OutputRoot 'must-not-run.exe'
    [System.IO.File]::WriteAllBytes($dummyInstaller, [byte[]](1, 2, 3, 4))

    $parseErrors = $null
    $targetTokens = $null
    $targetAst = [System.Management.Automation.Language.Parser]::ParseFile($target, [ref]$targetTokens, [ref]$parseErrors)
    Add-Result -Name 'powershell_5_compatible_parser_accepts_script' -Passed (@($parseErrors).Count -eq 0) -Detail (@($parseErrors) -join '; ')

    $windowsPowerShell = Join-Path $env:SystemRoot 'System32\WindowsPowerShell\v1.0\powershell.exe'
    $ps5StandardOutput = Join-Path $OutputRoot 'powershell-5-default-paths.stdout.log'
    $ps5StandardError = Join-Path $OutputRoot 'powershell-5-default-paths.stderr.log'
    $ps5Arguments = @(
        '-NoProfile', '-NonInteractive', '-ExecutionPolicy', 'Bypass',
        '-File', ('"{0}"' -f $target),
        '-Role', 'baseline',
        '-Installer', ('"{0}"' -f $dummyInstaller),
        '-SourceCommit', ('0' * 40),
        '-ExpectedVersion', '0.4.1',
        '-Output', ('"{0}"' -f (Join-Path $OutputRoot 'powershell-5-default-paths.json'))
    )
    $ps5Process = Start-Process -FilePath $windowsPowerShell -ArgumentList $ps5Arguments -NoNewWindow -Wait -PassThru `
        -RedirectStandardOutput $ps5StandardOutput -RedirectStandardError $ps5StandardError
    $ps5Output = @(
        if (Test-Path -LiteralPath $ps5StandardOutput -PathType Leaf) { Get-Content -LiteralPath $ps5StandardOutput -Raw }
        if (Test-Path -LiteralPath $ps5StandardError -PathType Leaf) { Get-Content -LiteralPath $ps5StandardError -Raw }
    ) -join "`n"
    Add-Result -Name 'powershell_5_default_repository_and_rollback_paths_reach_script_body' -Passed (
        $ps5Process.ExitCode -ne 0 -and $ps5Output.Contains('Baseline source commit must be') -and
        -not $ps5Output.Contains("Cannot bind argument to parameter 'Path' because it is an empty string")
    ) -Detail ("exit={0}; output={1}" -f $ps5Process.ExitCode, $ps5Output.Trim())
    $wrapperParseErrors = $null
    $wrapperTokens = $null
    $wrapperAst = [System.Management.Automation.Language.Parser]::ParseFile($buildWrapper, [ref]$wrapperTokens, [ref]$wrapperParseErrors)
    Add-Result -Name 'controlled_build_wrapper_parses_in_windows_powershell' -Passed (@($wrapperParseErrors).Count -eq 0) -Detail (@($wrapperParseErrors) -join '; ')
    $wrapperSource = Get-Content -LiteralPath $buildWrapper -Raw
    Add-Result -Name 'controlled_build_uses_workspace_root_target_layout' -Passed (
        $wrapperSource.Contains("target\release\bundle\nsis") -and
        $wrapperSource.Contains("relative_path = 'target/release/meetily.exe'") -and
        -not $wrapperSource.Contains('frontend/src-tauri/target/release/meetily.exe')
    )
    Add-Result -Name 'controlled_build_resolves_default_repository_after_script_start' -Passed (
        $wrapperSource.Contains("[string]`$RepositoryRoot = ''") -and
        $wrapperSource.Contains("`$RepositoryRoot = Join-Path `$PSScriptRoot '..\..'") -and
        -not $wrapperSource.Contains("[string]`$RepositoryRoot = (Join-Path `$PSScriptRoot")
    )

    $wrapperFunctions = @($wrapperAst.FindAll({
        param($node)
        $node -is [System.Management.Automation.Language.FunctionDefinitionAst] -and
            $node.Name -in @('Get-NormalizedFullPath', 'Assert-NoReparseAncestors', 'Append-LogLine', 'Get-RepositoryChanges', 'Invoke-PnpmBuildStep')
    }, $true))
    foreach ($functionName in @('Get-NormalizedFullPath', 'Assert-NoReparseAncestors', 'Append-LogLine', 'Get-RepositoryChanges', 'Invoke-PnpmBuildStep')) {
        $definition = @($wrapperFunctions | Where-Object Name -eq $functionName)
        if ($definition.Count -ne 1) { throw "Could not load production function for contract test: $functionName" }
        Invoke-Expression ([string]$definition[0].Extent.Text)
    }

    $nativeFixture = Join-Path $OutputRoot 'native-stream-fixture.cmd'
    $logPath = Join-Path $OutputRoot 'native-stream-success.log'
    $pnpmPath = $nativeFixture
    [System.IO.File]::WriteAllText(
        $nativeFixture,
        "@echo off`r`necho stdout-line`r`necho stderr-line 1>&2`r`nexit /b 0`r`n",
        [System.Text.Encoding]::ASCII
    )
    $nativeSuccess = $true
    $nativeSuccessDetail = ''
    try {
        Invoke-PnpmBuildStep -Arguments @('fixture') -DisplayCommand 'fixture success'
    } catch {
        $nativeSuccess = $false
        $nativeSuccessDetail = [string]$_.Exception.Message
    }
    $nativeSuccessLog = if (Test-Path -LiteralPath $logPath -PathType Leaf) { Get-Content -LiteralPath $logPath -Raw } else { '' }
    Add-Result -Name 'controlled_build_allows_native_stderr_when_exit_code_is_zero' -Passed (
        $nativeSuccess -and $nativeSuccessLog.Contains('stdout-line') -and
        $nativeSuccessLog.Contains('stderr-line') -and $nativeSuccessLog.Contains('EXIT_CODE 0')
    ) -Detail $nativeSuccessDetail

    [System.IO.File]::WriteAllText(
        $nativeFixture,
        "@echo off`r`necho real-failure 1>&2`r`nexit /b 7`r`n",
        [System.Text.Encoding]::ASCII
    )
    $logPath = Join-Path $OutputRoot 'native-stream-failure.log'
    $nativeFailure = Invoke-ExpectedFailure -Pattern 'Build command failed with exit code 7' -Action {
        Invoke-PnpmBuildStep -Arguments @('fixture') -DisplayCommand 'fixture failure'
    }
    $nativeFailureLog = if (Test-Path -LiteralPath $logPath -PathType Leaf) { Get-Content -LiteralPath $logPath -Raw } else { '' }
    Add-Result -Name 'controlled_build_still_rejects_nonzero_native_exit_code' -Passed (
        $nativeFailure.passed -and $nativeFailureLog.Contains('real-failure') -and
        $nativeFailureLog.Contains('EXIT_CODE 7')
    ) -Detail $nativeFailure.detail

    $wrapperRepositoryRoot = Join-Path $OutputRoot 'wrapper-repository-state'
    [System.IO.Directory]::CreateDirectory($wrapperRepositoryRoot) | Out-Null
    & git -C $wrapperRepositoryRoot init -q
    & git -C $wrapperRepositoryRoot config user.email 'fixture@example.invalid'
    & git -C $wrapperRepositoryRoot config user.name 'Fixture'
    & git -C $wrapperRepositoryRoot config core.autocrlf true
    $trackedFixture = Join-Path $wrapperRepositoryRoot 'tracked.txt'
    [System.IO.File]::WriteAllText($trackedFixture, "same`n", [System.Text.UTF8Encoding]::new($false))
    & git -C $wrapperRepositoryRoot add --all
    & git -C $wrapperRepositoryRoot commit -q -m fixture
    if ($LASTEXITCODE -ne 0) { throw 'Could not prepare the repository-state fixture.' }

    [System.IO.File]::WriteAllText($trackedFixture, "same`r`n", [System.Text.UTF8Encoding]::new($false))
    $sameCanonicalContent = @(Get-RepositoryChanges -Root $wrapperRepositoryRoot)
    Add-Result -Name 'controlled_build_allows_timestamp_or_line_ending_touch_with_same_git_content' -Passed ($sameCanonicalContent.Count -eq 0) -Detail ($sameCanonicalContent -join ',')

    [System.IO.File]::WriteAllText($trackedFixture, "changed`r`n", [System.Text.UTF8Encoding]::new($false))
    $changedTrackedContent = @(Get-RepositoryChanges -Root $wrapperRepositoryRoot)
    Add-Result -Name 'controlled_build_rejects_real_tracked_content_changes' -Passed ($changedTrackedContent -contains 'tracked.txt') -Detail ($changedTrackedContent -join ',')

    & git -C $wrapperRepositoryRoot add -- tracked.txt
    if ($LASTEXITCODE -ne 0) { throw 'Could not stage the repository-state fixture.' }
    $changedStagedContent = @(Get-RepositoryChanges -Root $wrapperRepositoryRoot)
    Add-Result -Name 'controlled_build_rejects_staged_content_changes' -Passed ($changedStagedContent -contains 'tracked.txt') -Detail ($changedStagedContent -join ',')

    [System.IO.File]::WriteAllText($trackedFixture, "same`r`n", [System.Text.UTF8Encoding]::new($false))
    & git -C $wrapperRepositoryRoot add -- tracked.txt
    if ($LASTEXITCODE -ne 0) { throw 'Could not restore the staged repository-state fixture.' }
    [System.IO.File]::WriteAllText((Join-Path $wrapperRepositoryRoot 'untracked.txt'), "new`r`n", [System.Text.UTF8Encoding]::new($false))
    $newUntrackedContent = @(Get-RepositoryChanges -Root $wrapperRepositoryRoot)
    Add-Result -Name 'controlled_build_rejects_untracked_files' -Passed ($newUntrackedContent -contains 'untracked.txt') -Detail ($newUntrackedContent -join ',')

    $junctionFixture = Join-Path $OutputRoot 'pinned-junction-fixture'
    $junctionTarget = Join-Path $junctionFixture 'expected-target'
    $junctionParent = Join-Path $junctionFixture 'repository-runtime'
    $junctionPath = Join-Path $junctionParent 'webview2-fixed'
    [System.IO.Directory]::CreateDirectory($junctionTarget) | Out-Null
    [System.IO.Directory]::CreateDirectory($junctionParent) | Out-Null
    [System.IO.File]::WriteAllText((Join-Path $junctionTarget 'msedgewebview2.exe'), 'fixture', [System.Text.UTF8Encoding]::new($false))
    New-Item -ItemType Junction -Path $junctionPath -Target $junctionTarget | Out-Null
    $junctionArtifact = Join-Path $junctionPath 'msedgewebview2.exe'
    $pinnedJunctionAccepted = $true
    $pinnedJunctionDetail = ''
    try {
        Assert-NoReparseAncestors -Path $junctionArtifact -AllowedReparsePoint $junctionPath -ExpectedReparseTarget $junctionTarget
    } catch {
        $pinnedJunctionAccepted = $false
        $pinnedJunctionDetail = [string]$_.Exception.Message
    }
    Add-Result -Name 'controlled_build_allows_only_exact_pinned_webview_junction' -Passed $pinnedJunctionAccepted -Detail $pinnedJunctionDetail
    $unapprovedJunction = Invoke-ExpectedFailure -Pattern 'Path contains a reparse point' -Action {
        Assert-NoReparseAncestors -Path $junctionArtifact
    }
    Add-Result -Name 'controlled_build_rejects_unapproved_junction' -Passed $unapprovedJunction.passed -Detail $unapprovedJunction.detail
    $wrongJunctionTarget = Invoke-ExpectedFailure -Pattern 'Pinned reparse target mismatch' -Action {
        Assert-NoReparseAncestors -Path $junctionArtifact -AllowedReparsePoint $junctionPath -ExpectedReparseTarget $junctionParent
    }
    Add-Result -Name 'controlled_build_rejects_wrong_pinned_junction_target' -Passed $wrongJunctionTarget.passed -Detail $wrongJunctionTarget.detail

    $source = Get-Content -LiteralPath $target -Raw
    $rolePathFunction = @($targetAst.FindAll({
        param($node)
        $node -is [System.Management.Automation.Language.FunctionDefinitionAst] -and
            $node.Name -eq 'Get-InstalledRolePaths'
    }, $true))
    $rolePathDetail = ''
    $baselineRolePaths = $null
    $candidateRolePaths = $null
    if ($rolePathFunction.Count -eq 1) {
        try {
            Invoke-Expression ([string]$rolePathFunction[0].Extent.Text)
            $baselineRolePaths = Get-InstalledRolePaths -ManifestRole baseline -ManifestProductName 'meetily-p6-lifecycle'
            $candidateRolePaths = Get-InstalledRolePaths -ManifestRole candidate -ManifestProductName 'meetily-p6-lifecycle'
        } catch {
            $rolePathDetail = [string]$_.Exception.Message
        }
    } else {
        $rolePathDetail = "expected one Get-InstalledRolePaths function, found $($rolePathFunction.Count)"
    }
    Add-Result -Name 'baseline_accepts_frozen_product_named_main_executable' -Passed (
        $null -ne $baselineRolePaths -and [string]$baselineRolePaths.main_executable -eq 'meetily-p6-lifecycle.exe'
    ) -Detail $rolePathDetail
    Add-Result -Name 'baseline_rejects_candidate_main_executable_name' -Passed (
        $null -ne $baselineRolePaths -and [string]$baselineRolePaths.main_executable -ne 'meetily.exe'
    ) -Detail $rolePathDetail
    Add-Result -Name 'candidate_accepts_only_current_main_executable_name' -Passed (
        $null -ne $candidateRolePaths -and [string]$candidateRolePaths.main_executable -eq 'meetily.exe'
    ) -Detail $rolePathDetail
    Add-Result -Name 'candidate_rejects_legacy_product_named_main_executable' -Passed (
        $null -ne $candidateRolePaths -and [string]$candidateRolePaths.main_executable -ne 'meetily-p6-lifecycle.exe'
    ) -Detail $rolePathDetail
    $sharedInstalledPaths = [ordered]@{
        llama_helper = 'llama-helper.exe'
        moss_helper = 'moss-helper.exe'
        ffmpeg = 'ffmpeg.exe'
        directml = 'DirectML.dll'
        webview2 = 'runtime/webview2-fixed/msedgewebview2.exe'
        uninstaller = 'uninstall.exe'
    }
    $sharedInstalledPathsMatch = $null -ne $baselineRolePaths -and $null -ne $candidateRolePaths
    if ($sharedInstalledPathsMatch) {
        foreach ($entry in $sharedInstalledPaths.GetEnumerator()) {
            $sharedInstalledPathsMatch = $sharedInstalledPathsMatch -and
                [string]$baselineRolePaths[$entry.Key] -eq [string]$entry.Value -and
                [string]$candidateRolePaths[$entry.Key] -eq [string]$entry.Value
        }
    }
    Add-Result -Name 'non_main_installed_file_contract_is_role_invariant' -Passed $sharedInstalledPathsMatch -Detail $rolePathDetail
    Add-Result -Name 'display_icon_and_version_checks_use_role_specific_main_executable' -Passed (
        $source.Contains("Join-Path `$installRoot ([string]`$rolePaths.main_executable)") -and
        $source.Contains('(Get-Item -LiteralPath $expectedMain).VersionInfo.ProductVersion')
    )
    foreach ($role in @('main_executable', 'llama_helper', 'moss_helper', 'ffmpeg', 'directml', 'webview2', 'uninstaller')) {
        Add-Result -Name ("fixed_installed_role_" + $role) -Passed ($source.Contains($role))
    }
    Add-Result -Name 'candidate_requires_content_level_clean_repository' -Passed (
        $source.Contains('diff --cached --name-only --no-ext-diff --no-textconv HEAD --') -and
        $source.Contains('diff --name-only --no-ext-diff --no-textconv --') -and
        $source.Contains('ls-files --others --exclude-standard') -and
        $source.Contains('Get-RepositoryChanges -Root $RepositoryRoot') -and
        -not $source.Contains('status --porcelain=v1 --untracked-files=all')
    )
    Add-Result -Name 'manifest_refuses_overwrite' -Passed ($source.Contains('Refusing to overwrite an existing manifest'))
    Add-Result -Name 'manifest_uses_atomic_temporary_file' -Passed ($source.Contains("[Guid]::NewGuid().ToString('N') + '.tmp'"))
    Add-Result -Name 'installer_and_uninstaller_use_native_argument_encoder' -Passed ($source.Contains('ConvertTo-NativeArgument'))
    Add-Result -Name 'baseline_installer_is_bound_to_frozen_bytes_and_sha256' -Passed (
        $source.Contains('386218356') -and $source.Contains('1C151B1534A66927FFA5B50DE58D05EE27247B933797441A59C747510C32483C')
    )
    Add-Result -Name 'candidate_requires_controlled_build_attestation' -Passed (
        $source.Contains('MOSS_FUNCTIONAL_CANDIDATE_BUILD') -and $source.Contains('Read-CandidateBuildAttestation') -and
        $source.Contains('Candidate NSIS installer') -and $source.Contains('build_provenance')
    )
    Add-Result -Name 'webview_root_is_in_preflight_and_cleanup' -Passed (
        ([regex]::Matches($source, '\$webViewRoot')).Count -ge 7 -and
        $source.Contains('isolated WebView directory unexpectedly exists after cleanup')
    )
    Add-Result -Name 'primary_and_cleanup_failures_are_combined' -Passed (
        $source.Contains('primary failure:') -and $source.Contains('cleanup failure:') -and
        $source.Contains('$failureMessages -join '' | ''')
    )
    Add-Result -Name 'timeout_tree_requires_two_empty_scans' -Passed (
        $source.Contains('$emptyScans -lt 2') -and $source.Contains('$knownIds') -and
        $source.Contains('exact process tree was verified absent in two consecutive scans')
    )
    Add-Result -Name 'output_cannot_overlap_any_isolated_root' -Passed (
        $source.Contains('Output must not overlap an isolated install, data, backup, or WebView root.') -and
        $source.Contains('@($installRoot, $dataRoot, $backupRoot, $webViewRoot)')
    )

    $mockRepositoryRoot = Join-Path $OutputRoot 'candidate-clean-gate-repository'
    $mockRollback = Join-Path $mockRepositoryRoot 'frontend\src-tauri\scripts\meetily-versioned-data.ps1'
    [System.IO.Directory]::CreateDirectory((Split-Path -Parent $mockRollback)) | Out-Null
    [System.IO.File]::WriteAllText($mockRollback, "'fixture'`n", [System.Text.UTF8Encoding]::new($false))
    $global:manifestContractMockHead = 'b' * 40
    $global:manifestContractMockRepositoryState = ''
    $global:manifestContractMockGitCalls = [System.Collections.Generic.List[string]]::new()
    function git {
        $gitArguments = @($args | ForEach-Object { [string]$_ })
        $global:manifestContractMockGitCalls.Add(($gitArguments -join ' ')) | Out-Null
        $global:LASTEXITCODE = 0
        if ($gitArguments -contains 'rev-parse') { return $global:manifestContractMockHead }
        if ($gitArguments -contains 'status') {
            switch ($global:manifestContractMockRepositoryState) {
                'stat-only' { return ' M frontend/src-tauri/Cargo.toml' }
                'staged' { return 'M  frontend/src-tauri/Cargo.toml' }
                'unstaged' { return ' M frontend/src-tauri/Cargo.toml' }
                'untracked' { return '?? frontend/src-tauri/new-file.txt' }
            }
        }
        if ($gitArguments -contains 'diff') {
            if ($gitArguments -contains '--cached') {
                if ($global:manifestContractMockRepositoryState -eq 'staged') { return 'frontend/src-tauri/Cargo.toml' }
            } elseif ($global:manifestContractMockRepositoryState -eq 'unstaged') {
                return 'frontend/src-tauri/Cargo.toml'
            }
        }
        if ($gitArguments -contains 'ls-files' -and $global:manifestContractMockRepositoryState -eq 'untracked') {
            return 'frontend/src-tauri/new-file.txt'
        }
    }
    try {
        $candidateCleanCases = @(
            [ordered]@{ name = 'candidate_allows_porcelain_only_modified_state_when_content_diffs_are_empty'; state = 'stat-only'; pattern = 'requires the attestation' },
            [ordered]@{ name = 'candidate_rejects_staged_content_diff'; state = 'staged'; pattern = 'completely clean' },
            [ordered]@{ name = 'candidate_rejects_unstaged_content_diff'; state = 'unstaged'; pattern = 'completely clean' },
            [ordered]@{ name = 'candidate_rejects_untracked_file_report'; state = 'untracked'; pattern = 'completely clean' }
        )
        foreach ($case in $candidateCleanCases) {
            $global:manifestContractMockRepositoryState = [string]$case.state
            $global:manifestContractMockGitCalls = [System.Collections.Generic.List[string]]::new()
            $cleanGateResult = Invoke-ExpectedFailure -Pattern ([string]$case.pattern) -Action {
                & $target -Role candidate -Installer $dummyInstaller -SourceCommit $global:manifestContractMockHead -ExpectedVersion '0.4.2' `
                    -Output (Join-Path $OutputRoot ("candidate-clean-gate-{0}.json" -f $case.state)) `
                    -RepositoryRoot $mockRepositoryRoot -RollbackTool $mockRollback `
                    -ProductName ("meetily-manifest-{0}-fixture" -f $case.state) `
                    -BundleId ("com.meetily.ai.manifest{0}fixture" -f ($case.state -replace '-', ''))
            }
            $mockCalls = @($global:manifestContractMockGitCalls)
            $usedStagedDiff = @($mockCalls | Where-Object { $_ -match ' diff --cached --name-only ' }).Count -eq 1
            $usedUnstagedDiff = @($mockCalls | Where-Object { $_ -match ' diff --name-only ' -and $_ -notmatch ' --cached ' }).Count -eq 1
            $usedUntrackedList = @($mockCalls | Where-Object { $_ -match ' ls-files --others --exclude-standard$' }).Count -eq 1
            $usedPorcelain = @($mockCalls | Where-Object { $_ -match ' status --porcelain' }).Count -ne 0
            Add-Result -Name ([string]$case.name) -Passed (
                $cleanGateResult.passed -and $usedStagedDiff -and $usedUnstagedDiff -and $usedUntrackedList -and -not $usedPorcelain
            ) -Detail ("error={0}; calls={1}" -f $cleanGateResult.detail, ($mockCalls -join ' | '))
        }
    } finally {
        Remove-Item -LiteralPath 'Function:\git' -Force
        Remove-Variable -Name manifestContractMockHead -Scope Global -ErrorAction SilentlyContinue
        Remove-Variable -Name manifestContractMockRepositoryState -Scope Global -ErrorAction SilentlyContinue
        Remove-Variable -Name manifestContractMockGitCalls -Scope Global -ErrorAction SilentlyContinue
    }

    $manifestRepositoryFunction = @($targetAst.FindAll({
        param($node)
        $node -is [System.Management.Automation.Language.FunctionDefinitionAst] -and
            $node.Name -eq 'Get-RepositoryChanges'
    }, $true))
    $manifestRepositoryFunctionLoaded = $manifestRepositoryFunction.Count -eq 1
    $manifestRepositoryFunctionDetail = ''
    if ($manifestRepositoryFunctionLoaded) {
        try {
            Invoke-Expression ([string]$manifestRepositoryFunction[0].Extent.Text)
        } catch {
            $manifestRepositoryFunctionLoaded = $false
            $manifestRepositoryFunctionDetail = [string]$_.Exception.Message
        }
    } else {
        $manifestRepositoryFunctionDetail = "expected one Get-RepositoryChanges function, found $($manifestRepositoryFunction.Count)"
    }
    $manifestRepositoryRoot = Join-Path $OutputRoot 'manifest-repository-state'
    [System.IO.Directory]::CreateDirectory($manifestRepositoryRoot) | Out-Null
    & git -C $manifestRepositoryRoot init -q
    & git -C $manifestRepositoryRoot config user.email 'fixture@example.invalid'
    & git -C $manifestRepositoryRoot config user.name 'Fixture'
    & git -C $manifestRepositoryRoot config core.autocrlf true
    $manifestTrackedFixture = Join-Path $manifestRepositoryRoot 'tracked.txt'
    [System.IO.File]::WriteAllText($manifestTrackedFixture, "same`n", [System.Text.UTF8Encoding]::new($false))
    & git -C $manifestRepositoryRoot add --all
    & git -C $manifestRepositoryRoot commit -q -m fixture
    if ($LASTEXITCODE -ne 0) { throw 'Could not prepare the manifest repository-state fixture.' }

    [System.IO.File]::WriteAllText($manifestTrackedFixture, "same`r`n", [System.Text.UTF8Encoding]::new($false))
    $manifestSameCanonicalContent = @(if ($manifestRepositoryFunctionLoaded) { Get-RepositoryChanges -Root $manifestRepositoryRoot } else { 'function-missing' })
    Add-Result -Name 'manifest_content_check_allows_same_canonical_blob_after_line_ending_touch' -Passed (
        $manifestRepositoryFunctionLoaded -and $manifestSameCanonicalContent.Count -eq 0
    ) -Detail ($manifestRepositoryFunctionDetail + ($manifestSameCanonicalContent -join ','))

    [System.IO.File]::WriteAllText($manifestTrackedFixture, "changed`r`n", [System.Text.UTF8Encoding]::new($false))
    $manifestUnstagedContent = @(if ($manifestRepositoryFunctionLoaded) { Get-RepositoryChanges -Root $manifestRepositoryRoot })
    Add-Result -Name 'manifest_content_check_rejects_real_unstaged_change' -Passed (
        $manifestRepositoryFunctionLoaded -and $manifestUnstagedContent -contains 'tracked.txt'
    ) -Detail ($manifestRepositoryFunctionDetail + ($manifestUnstagedContent -join ','))

    & git -C $manifestRepositoryRoot add -- tracked.txt
    if ($LASTEXITCODE -ne 0) { throw 'Could not stage the manifest repository-state fixture.' }
    $manifestStagedContent = @(if ($manifestRepositoryFunctionLoaded) { Get-RepositoryChanges -Root $manifestRepositoryRoot })
    Add-Result -Name 'manifest_content_check_rejects_real_staged_change' -Passed (
        $manifestRepositoryFunctionLoaded -and $manifestStagedContent -contains 'tracked.txt'
    ) -Detail ($manifestRepositoryFunctionDetail + ($manifestStagedContent -join ','))

    [System.IO.File]::WriteAllText($manifestTrackedFixture, "same`r`n", [System.Text.UTF8Encoding]::new($false))
    & git -C $manifestRepositoryRoot add -- tracked.txt
    if ($LASTEXITCODE -ne 0) { throw 'Could not restore the manifest repository-state fixture.' }
    [System.IO.File]::WriteAllText((Join-Path $manifestRepositoryRoot 'untracked.txt'), "new`r`n", [System.Text.UTF8Encoding]::new($false))
    $manifestUntrackedContent = @(if ($manifestRepositoryFunctionLoaded) { Get-RepositoryChanges -Root $manifestRepositoryRoot })
    Add-Result -Name 'manifest_content_check_rejects_real_untracked_file' -Passed (
        $manifestRepositoryFunctionLoaded -and $manifestUntrackedContent -contains 'untracked.txt'
    ) -Detail ($manifestRepositoryFunctionDetail + ($manifestUntrackedContent -join ','))

    $wrongBaseline = Invoke-ExpectedFailure -Pattern 'Baseline source commit must be' -Action {
        & $target -Role baseline -Installer $dummyInstaller -SourceCommit ('0' * 40) -ExpectedVersion '0.4.1' -Output (Join-Path $OutputRoot 'wrong-baseline.json')
    }
    Add-Result -Name 'wrong_baseline_source_commit_is_rejected_before_install' -Passed $wrongBaseline.passed -Detail $wrongBaseline.detail

    $wrongBaselineArtifact = Invoke-ExpectedFailure -Pattern 'frozen E-02 bytes and SHA-256' -Action {
        & $target -Role baseline -Installer $dummyInstaller -SourceCommit '7392eae159443822c80d3675ca9af388e94b2d71' -ExpectedVersion '0.4.1' -Output (Join-Path $OutputRoot 'wrong-baseline-artifact.json')
    }
    Add-Result -Name 'wrong_baseline_artifact_is_rejected_before_install' -Passed $wrongBaselineArtifact.passed -Detail $wrongBaselineArtifact.detail

    $baselineBundle = 'com.meetily.ai.manifestbaselinefixture'
    [System.IO.Directory]::CreateDirectory((Join-Path $env:APPDATA $baselineBundle)) | Out-Null
    $approvedBaseline = 'D:\MeetilyData\private-evidence\moss-v3-p6-host-isolated-lifecycle-20260830\artifacts\0.4.1\meetily-p6-lifecycle_0.4.1_x64-setup.exe'
    $preexistingBaseline = if (Test-Path -LiteralPath $approvedBaseline -PathType Leaf) {
        Invoke-ExpectedFailure -Pattern 'already exists' -Action {
            & $target -Role baseline -Installer $approvedBaseline -SourceCommit '7392eae159443822c80d3675ca9af388e94b2d71' -ExpectedVersion '0.4.1' -Output (Join-Path $OutputRoot 'baseline.json') -ProductName 'meetily-manifest-baseline-fixture' -BundleId $baselineBundle
        }
    } else { [ordered]@{ passed = $false; detail = 'approved frozen baseline installer is unavailable' } }
    Add-Result -Name 'preexisting_baseline_data_is_never_deleted_or_overwritten' -Passed $preexistingBaseline.passed -Detail $preexistingBaseline.detail
    Add-Result -Name 'dummy_installer_was_not_executed_for_baseline_rejection' -Passed (-not (Test-Path -LiteralPath (Join-Path $env:LOCALAPPDATA 'meetily-manifest-baseline-fixture')))

    $repo = Join-Path $OutputRoot 'repo'
    [System.IO.Directory]::CreateDirectory((Join-Path $repo 'frontend\src-tauri\scripts')) | Out-Null
    [System.IO.File]::WriteAllText((Join-Path $repo 'frontend\src-tauri\scripts\meetily-versioned-data.ps1'), "'fixture'`n", [System.Text.UTF8Encoding]::new($false))
    & git -C $repo init -q
    & git -C $repo config user.email 'fixture@example.invalid'
    & git -C $repo config user.name 'Fixture'
    & git -C $repo add --all
    & git -C $repo commit -q -m fixture
    if ($LASTEXITCODE -ne 0) { throw 'Could not prepare the temporary Git fixture.' }
    $head = (& git -C $repo rev-parse HEAD).Trim()
    $fixedRollback = Join-Path $repo 'frontend\src-tauri\scripts\meetily-versioned-data.ps1'

    [System.IO.File]::AppendAllText($fixedRollback, "'dirty'`n", [System.Text.UTF8Encoding]::new($false))
    $dirtyCandidate = Invoke-ExpectedFailure -Pattern 'completely clean' -Action {
        & $target -Role candidate -Installer $dummyInstaller -SourceCommit $head -ExpectedVersion '0.4.2' -Output (Join-Path $OutputRoot 'dirty-candidate.json') -RepositoryRoot $repo -RollbackTool $fixedRollback -ProductName 'meetily-manifest-dirty-fixture' -BundleId 'com.meetily.ai.manifestdirtyfixture'
    }
    Add-Result -Name 'dirty_candidate_repository_is_rejected_before_install' -Passed $dirtyCandidate.passed -Detail $dirtyCandidate.detail

    $repo2 = Join-Path $OutputRoot 'repo-clean'
    [System.IO.Directory]::CreateDirectory((Join-Path $repo2 'frontend\src-tauri\scripts')) | Out-Null
    $fixedRollback2 = Join-Path $repo2 'frontend\src-tauri\scripts\meetily-versioned-data.ps1'
    [System.IO.File]::WriteAllText($fixedRollback2, "'fixture'`n", [System.Text.UTF8Encoding]::new($false))
    [System.IO.File]::WriteAllText((Join-Path $repo2 'wrong.ps1'), "'wrong'`n", [System.Text.UTF8Encoding]::new($false))
    & git -C $repo2 init -q
    & git -C $repo2 config user.email 'fixture@example.invalid'
    & git -C $repo2 config user.name 'Fixture'
    & git -C $repo2 add --all
    & git -C $repo2 commit -q -m fixture
    if ($LASTEXITCODE -ne 0) { throw 'Could not prepare the clean temporary Git fixture.' }
    $head2 = (& git -C $repo2 rev-parse HEAD).Trim()

    $wrongRollback = Invoke-ExpectedFailure -Pattern 'fixed repository path' -Action {
        & $target -Role candidate -Installer $dummyInstaller -SourceCommit $head2 -ExpectedVersion '0.4.2' -Output (Join-Path $OutputRoot 'wrong-tool.json') -RepositoryRoot $repo2 -RollbackTool (Join-Path $repo2 'wrong.ps1') -ProductName 'meetily-manifest-wrong-tool-fixture' -BundleId 'com.meetily.ai.manifestwrongtoolfixture'
    }
    Add-Result -Name 'candidate_rollback_tool_path_is_fixed' -Passed $wrongRollback.passed -Detail $wrongRollback.detail

    $candidateBundle = 'com.meetily.ai.manifestcleanfixture'
    $missingAttestation = Invoke-ExpectedFailure -Pattern 'requires the attestation' -Action {
        & $target -Role candidate -Installer $dummyInstaller -SourceCommit $head2 -ExpectedVersion '0.4.2' -Output (Join-Path $OutputRoot 'missing-attestation.json') -RepositoryRoot $repo2 -RollbackTool $fixedRollback2 -ProductName 'meetily-manifest-clean-fixture' -BundleId $candidateBundle
    }
    Add-Result -Name 'candidate_without_controlled_build_attestation_is_rejected' -Passed $missingAttestation.passed -Detail $missingAttestation.detail

    $fixtureArtifacts = [ordered]@{
        main_executable = 'target/release/meetily.exe'
        llama_helper = 'frontend/src-tauri/binaries/llama-helper-x86_64-pc-windows-msvc.exe'
        moss_helper = 'frontend/src-tauri/binaries/moss-helper-x86_64-pc-windows-msvc.exe'
        ffmpeg = 'frontend/src-tauri/binaries/ffmpeg-x86_64-pc-windows-msvc.exe'
        directml = 'frontend/src-tauri/runtime/windows-x64/nsis/DirectML.dll'
        webview2 = 'frontend/src-tauri/runtime/webview2-fixed/msedgewebview2.exe'
    }
    foreach ($relative in $fixtureArtifacts.Values) {
        $path = Join-Path $repo2 $relative
        [System.IO.Directory]::CreateDirectory((Split-Path -Parent $path)) | Out-Null
        [System.IO.File]::WriteAllText($path, "fixture:$relative`n", [System.Text.UTF8Encoding]::new($false))
    }
    $fixtureProducer = Join-Path $repo2 'scripts\qa\build-moss-functional-candidate.ps1'
    [System.IO.Directory]::CreateDirectory((Split-Path -Parent $fixtureProducer)) | Out-Null
    Copy-Item -LiteralPath $buildWrapper -Destination $fixtureProducer
    & git -C $repo2 add --all
    & git -C $repo2 commit -q -m 'build fixture'
    if ($LASTEXITCODE -ne 0) { throw 'Could not commit the controlled-build fixture.' }
    $head2 = (& git -C $repo2 rev-parse HEAD).Trim()
    $buildLog = Join-Path $OutputRoot 'fixture-build.log'
    [System.IO.File]::WriteAllText($buildLog, "fixture build log`n", [System.Text.UTF8Encoding]::new($false))
    $attestationArtifacts = @(
        foreach ($entry in $fixtureArtifacts.GetEnumerator()) {
            Get-TestFileRecord -Path (Join-Path $repo2 ([string]$entry.Value)) -RelativePath ([string]$entry.Value) -Role ([string]$entry.Key)
        }
        Get-TestFileRecord -Path $dummyInstaller -RelativePath 'fixture-candidate.exe' -Role 'nsis_installer'
    )
    $attestation = [ordered]@{
        schema_version = 1; stage = 'MOSS_FUNCTIONAL_CANDIDATE_BUILD'; status = 'PASS'
        source_commit = $head2; repository_head_before = $head2; repository_head_after = $head2
        worktree_clean_before = $true; worktree_clean_after = $true
        product_name = 'meetily-manifest-clean-fixture'; bundle_id = $candidateBundle; version = '0.4.2'
        commands = @('pnpm sidecars:prepare', 'pnpm exec tauri build --config src-tauri/tauri.lifecycle.conf.json -- --features vulkan')
        environment = [ordered]@{ LIBCLANG_PATH = 'D:\MeetilyBuildTools\clang+llvm-19.1.5-x86_64-pc-windows-msvc\bin'; VULKAN_SDK = 'D:\VulkanSDK\1.4.357.0'; CMAKE_GENERATOR = 'NMake Makefiles' }
        producer = Get-TestFileRecord -Path $fixtureProducer -RelativePath 'scripts/qa/build-moss-functional-candidate.ps1'
        build_log = (Get-TestFileRecord -Path $buildLog -RelativePath 'fixture-build.log')
        artifacts = $attestationArtifacts
    }
    $attestation.build_log['path'] = $buildLog
    $attestationPath = Join-Path $OutputRoot 'fixture-build-attestation.json'
    [System.IO.File]::WriteAllText($attestationPath, (($attestation | ConvertTo-Json -Depth 12) + "`n"), [System.Text.UTF8Encoding]::new($false))
    [System.IO.Directory]::CreateDirectory((Join-Path $env:APPDATA $candidateBundle)) | Out-Null
    $preexistingCandidate = Invoke-ExpectedFailure -Pattern 'already exists' -Action {
        & $target -Role candidate -Installer $dummyInstaller -SourceCommit $head2 -ExpectedVersion '0.4.2' -Output (Join-Path $OutputRoot 'candidate.json') -RepositoryRoot $repo2 -RollbackTool $fixedRollback2 -ProductName 'meetily-manifest-clean-fixture' -BundleId $candidateBundle -BuildAttestation $attestationPath
    }
    Add-Result -Name 'preexisting_candidate_data_is_never_deleted_or_overwritten' -Passed $preexistingCandidate.passed -Detail $preexistingCandidate.detail
    Add-Result -Name 'dummy_installer_was_not_executed_for_candidate_rejection' -Passed (-not (Test-Path -LiteralPath (Join-Path $env:LOCALAPPDATA 'meetily-manifest-clean-fixture')))

    $failed = @($results | Where-Object { $_.status -ne 'PASS' })
    $report = [ordered]@{
        schema_version = 1
        stage = 'BUILD_MANIFEST_GENERATOR_CONTRACT'
        status = if ($failed.Count -eq 0) { 'PASS' } else { 'FAIL' }
        total = $results.Count
        passed = @($results | Where-Object { $_.status -eq 'PASS' }).Count
        failed = $failed.Count
        tests = $results
        source = [ordered]@{
            relative_path = 'scripts/qa/new-install-build-manifest.ps1'
            bytes = (Get-Item -LiteralPath $target).Length
            sha256 = (Get-FileHash -LiteralPath $target -Algorithm SHA256).Hash.ToUpperInvariant()
        }
        producer = [ordered]@{
            relative_path = 'scripts/qa/test-new-install-build-manifest-contract.ps1'
            bytes = (Get-Item -LiteralPath $MyInvocation.MyCommand.Path).Length
            sha256 = (Get-FileHash -LiteralPath $MyInvocation.MyCommand.Path -Algorithm SHA256).Hash.ToUpperInvariant()
        }
        build_wrapper = [ordered]@{
            relative_path = 'scripts/qa/build-moss-functional-candidate.ps1'
            bytes = (Get-Item -LiteralPath $buildWrapper).Length
            sha256 = (Get-FileHash -LiteralPath $buildWrapper -Algorithm SHA256).Hash.ToUpperInvariant()
        }
    }
    $reportPath = Join-Path $OutputRoot 'build-manifest-generator-contract.public.json'
    [System.IO.File]::WriteAllText($reportPath, (($report | ConvertTo-Json -Depth 20) + "`n"), [System.Text.UTF8Encoding]::new($false))
    $report | ConvertTo-Json -Depth 20
    if ($failed.Count -ne 0) { exit 1 }
} finally {
    $env:APPDATA = $originalAppData
    $env:LOCALAPPDATA = $originalLocalAppData
}

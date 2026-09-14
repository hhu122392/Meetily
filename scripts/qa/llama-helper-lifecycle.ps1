[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [ValidatePattern('^FIX-[A-Z0-9]+-[0-9]{8}-[0-9]{2}$')]
    [string]$RunId,

    [string]$RepositoryRoot,

    [string]$TestExecutable
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

if ([string]::IsNullOrWhiteSpace($RepositoryRoot)) {
    $RepositoryRoot = Join-Path $PSScriptRoot '..\..'
}
$RepositoryRoot = (Resolve-Path -LiteralPath $RepositoryRoot).Path

$expectedBranch = 'codex/moss-functional-fixes-20260902'
$branch = (& git -C $RepositoryRoot branch --show-current).Trim()
if ($LASTEXITCODE -ne 0 -or $branch -ne $expectedBranch) {
    throw "Expected branch '$expectedBranch', found '$branch'."
}

$statusBefore = @(& git -C $RepositoryRoot status --porcelain=v1 --untracked-files=all)
if ($LASTEXITCODE -ne 0) {
    throw 'Unable to read Git status.'
}
if ($statusBefore.Count -ne 0) {
    throw 'The lifecycle acceptance run requires a clean worktree.'
}

$sourceCommit = (& git -C $RepositoryRoot rev-parse HEAD).Trim()
if ($LASTEXITCODE -ne 0 -or $sourceCommit -notmatch '^[0-9a-f]{40}$') {
    throw 'Unable to resolve the source commit.'
}

if ([string]::IsNullOrWhiteSpace($TestExecutable)) {
    $candidates = @(Get-ChildItem -LiteralPath (Join-Path $RepositoryRoot 'target\release\deps') `
        -Filter 'app_lib_tests-*.exe' -File | Sort-Object LastWriteTimeUtc -Descending)
    if ($candidates.Count -eq 0) {
        throw 'No release app_lib_tests executable was found.'
    }
    $TestExecutable = $candidates[0].FullName
}
$TestExecutable = (Resolve-Path -LiteralPath $TestExecutable).Path

$docsRoot = Join-Path $RepositoryRoot 'target\release\docs'
$runDirectories = @(Get-ChildItem -LiteralPath $docsRoot -Directory -Recurse |
    Where-Object {
        $_.Name -eq $RunId -and
        (Test-Path -LiteralPath (Join-Path $_.FullName 'R01-automation.public.json'))
    })
if ($runDirectories.Count -ne 1) {
    throw "Expected one public evidence directory for $RunId, found $($runDirectories.Count)."
}
$publicRoot = $runDirectories[0].FullName
$privateRoot = Join-Path 'D:\MeetilyData\private-evidence\moss-functional-fix-20260902' $RunId
New-Item -ItemType Directory -Force -Path $publicRoot | Out-Null
New-Item -ItemType Directory -Force -Path $privateRoot | Out-Null

$expectedTests = @(
    'summary::summary_engine::sidecar::tests::normal_shutdown_waits_until_process_is_gone',
    'summary::summary_engine::sidecar::tests::shutdown_timeout_kills_job_tree_and_waits_again',
    'summary::summary_engine::sidecar::tests::shutdown_is_idempotent',
    'summary::summary_engine::sidecar::tests::generation_success_releases_sidecar',
    'summary::summary_engine::sidecar::tests::generation_failure_releases_sidecar',
    'summary::summary_engine::sidecar::tests::parent_force_exit_releases_helper_and_grandchild'
)

$savedErrorActionPreference = $ErrorActionPreference
$ErrorActionPreference = 'Continue'
$testOutput = @(& $TestExecutable 'summary::summary_engine::sidecar::tests' `
    '--nocapture' '--test-threads=1' 2>&1 | ForEach-Object { $_.ToString() })
$testExitCode = $LASTEXITCODE
$ErrorActionPreference = $savedErrorActionPreference
$logPath = Join-Path $privateRoot 'R02-sidecar-tests.log'
$testOutput | Set-Content -LiteralPath $logPath -Encoding utf8

$joinedOutput = $testOutput -join "`n"
$missingTests = @()
foreach ($testName in $expectedTests) {
    $pattern = [regex]::Escape("test $testName") + '.*ok'
    if ($joinedOutput -notmatch $pattern) {
        $missingTests += $testName
    }
}

$completionMarkers = @($testOutput | Where-Object { $_ -match 'LLAMA_LIFECYCLE_DONE' })
$badMarkers = @($completionMarkers | Where-Object { $_ -notmatch 'active=0' })
$summaryPassed = $joinedOutput -match `
    'test result: ok\. 6 passed; 0 failed; 1 ignored; 0 measured;'

if ($testExitCode -ne 0 -or $missingTests.Count -ne 0 -or `
        $completionMarkers.Count -ne 6 -or $badMarkers.Count -ne 0 -or -not $summaryPassed) {
    throw "Lifecycle acceptance failed. Exit=$testExitCode Missing=$($missingTests.Count) Markers=$($completionMarkers.Count) BadMarkers=$($badMarkers.Count)."
}

$normalMarkers = @($completionMarkers | Where-Object { $_ -notmatch 'case=parent-force-exit' })
$forceMarkers = @($completionMarkers | Where-Object { $_ -match 'case=parent-force-exit' })
if ($normalMarkers.Count -ne 5 -or $forceMarkers.Count -ne 1) {
    throw 'Lifecycle marker classification is incomplete.'
}

$testHash = (Get-FileHash -LiteralPath $TestExecutable -Algorithm SHA256).Hash
$logHash = (Get-FileHash -LiteralPath $logPath -Algorithm SHA256).Hash
$fixturePath = Join-Path $RepositoryRoot 'scripts\qa\fixtures\llama-helper-lifecycle-fixture.cmd'
$fixtureHash = (Get-FileHash -LiteralPath $fixturePath -Algorithm SHA256).Hash

$normalEvidence = [ordered]@{
    schema_version = 1
    run_id = $RunId
    task = 'V-02'
    result = 'PASS'
    process_tracking = 'exact Job Object process IDs'
    fuzzy_name_termination_used = $false
    observations = $normalMarkers
    residual_active_processes = 0
}
$forceEvidence = [ordered]@{
    schema_version = 1
    run_id = $RunId
    task = 'V-02'
    result = 'PASS'
    process_tracking = 'exact parent PID plus Job Object child process IDs'
    fuzzy_name_termination_used = $false
    observations = $forceMarkers
    residual_active_processes = 0
    deadline_seconds = 5
}
$automationEvidence = [ordered]@{
    schema_version = 1
    run_id = $RunId
    task = 'V-02'
    result = 'PASS'
    source_commit = $sourceCommit
    branch = $branch
    worktree_clean_before_run = $true
    expected_test_count = 6
    passed_test_count = 6
    failed_test_count = 0
    ignored_fixture_count = 1
    exact_tests = $expectedTests
    completion_marker_count = $completionMarkers.Count
    residual_active_processes = 0
    process_tracking = 'exact PIDs obtained from the Windows Job Object'
    fuzzy_name_termination_used = $false
    test_executable = [ordered]@{
        file_name = [System.IO.Path]::GetFileName($TestExecutable)
        sha256 = $testHash
    }
    fixture = [ordered]@{
        file_name = [System.IO.Path]::GetFileName($fixturePath)
        sha256 = $fixtureHash
    }
    private_log = [ordered]@{
        file_name = [System.IO.Path]::GetFileName($logPath)
        sha256 = $logHash
    }
}

$normalPath = Join-Path $publicRoot 'R02-normal-shutdown-processes.json'
$forcePath = Join-Path $publicRoot 'R02-force-exit-processes.json'
$automationPath = Join-Path $publicRoot 'R02-automation.public.json'
$normalEvidence | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath $normalPath -Encoding utf8
$forceEvidence | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath $forcePath -Encoding utf8
$automationEvidence | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath $automationPath -Encoding utf8

$result = [ordered]@{
    result = 'PASS'
    run_id = $RunId
    source_commit = $sourceCommit
    passed = 6
    failed = 0
    residual_active_processes = 0
    public_files = @(
        [System.IO.Path]::GetFileName($normalPath),
        [System.IO.Path]::GetFileName($forcePath),
        [System.IO.Path]::GetFileName($automationPath)
    )
}
$result | ConvertTo-Json -Depth 5

[CmdletBinding()]
param(
    [string]$OutputRoot = (Join-Path (Get-Location) 'target\moss-functional-ft-performance-contract')
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'

$scriptRoot = Split-Path -Parent $MyInvocation.MyCommand.Path
$repoRoot = [System.IO.Path]::GetFullPath((Join-Path $scriptRoot '..\..'))
$evidenceLibrary = Join-Path $scriptRoot 'moss-functional-ft-evidence.ps1'
$runner = Join-Path $scriptRoot 'run-moss-functional-ft.ps1'
$cdp = Join-Path $scriptRoot 'moss-functional-ft-cdp.mjs'
$processMonitor = Join-Path $scriptRoot 'moss-functional-ft-process-monitor.ps1'
$operationLock = Join-Path $repoRoot 'frontend\src-tauri\src\storage\operation_lock.rs'
$mossReview = Join-Path $repoRoot 'frontend\src-tauri\src\moss_review.rs'
$mossProtocol = Join-Path $repoRoot 'moss-helper\src\protocol.rs'
$mossNative = Join-Path $repoRoot 'moss-helper\src\native.rs'
$mossDatabase = Join-Path $repoRoot 'frontend\src-tauri\src\database\moss.rs'
$mossMigration = Join-Path $repoRoot 'frontend\src-tauri\migrations\20260905000000_add_moss_native_decode_contract.sql'
$mossFrontendSchema = Join-Path $repoRoot 'frontend\src\features\moss\schemas.ts'

if (Test-Path -LiteralPath $OutputRoot) {
    throw "OutputRoot already exists: $OutputRoot"
}
[System.IO.Directory]::CreateDirectory($OutputRoot) | Out-Null

$tests = [System.Collections.Generic.List[object]]::new()
function Add-Test {
    param([string]$Name, [bool]$Passed, [string]$Detail)
    $tests.Add([ordered]@{ name = $Name; passed = $Passed; detail = $Detail })
    if (-not $Passed) { Write-Host "FAIL $Name :: $Detail" -ForegroundColor Red }
}

function New-Sample {
    param([double]$StartedMs, [double]$CompletedMs, [string]$Label)
    return [ordered]@{
        label = $Label
        clock = 'node_performance_now'
        started_monotonic_ms = $StartedMs
        completed_monotonic_ms = $CompletedMs
        environment = [ordered]@{
            cpu = [ordered]@{ available = $true; load_percent = 20 }
            memory = [ordered]@{ available = $true; used_percent = 40 }
            gpu = [ordered]@{ available = $true; utilization_percent = 10 }
            disk = [ordered]@{ available = $true; utilization_percent = 5 }
            power = [ordered]@{ available = $true; source = 'ac'; scheme = 'balanced' }
            model_state = 'warm'
        }
    }
}

function New-Samples {
    param([int]$Count, [double[]]$ElapsedMs)
    $items = @()
    for ($index = 0; $index -lt $Count; $index++) {
        $elapsed = $ElapsedMs[$index % $ElapsedMs.Count]
        $items += New-Sample -StartedMs ($index * 100000) -CompletedMs (($index * 100000) + $elapsed) -Label ("sample-{0}" -f ($index + 1))
    }
    return $items
}

. $evidenceLibrary

$specifications = Get-MossPerformanceGateSpecifications
$expectedSpecifications = [ordered]@{
    stop_feedback = [ordered]@{ threshold_ms = 1000; required_samples = 5 }
    page_unlock = [ordered]@{ threshold_ms = 10000; required_samples = 5 }
    enhance_feedback = [ordered]@{ threshold_ms = 1000; required_samples = 5 }
    cancel_feedback = [ordered]@{ threshold_ms = 1000; required_samples = 5 }
    summary_completion = [ordered]@{ threshold_ms = 180000; required_samples = 3 }
    whisper_enhancement_completion = [ordered]@{ threshold_ms = 300000; required_samples = 3 }
}
Add-Test 'six-exact-performance-gates' (($specifications.Keys -join '|') -eq ($expectedSpecifications.Keys -join '|')) (($specifications.Keys) -join ',')
foreach ($gate in $expectedSpecifications.Keys) {
    $actual = $specifications[$gate]
    $expected = $expectedSpecifications[$gate]
    Add-Test ("$gate-threshold-and-count") `
        ([double]$actual.threshold_ms -eq [double]$expected.threshold_ms -and [int]$actual.required_samples -eq [int]$expected.required_samples) `
        ($actual | ConvertTo-Json -Compress)
}

$fivePass = New-MossPerformanceGateSummary -GateName 'stop_feedback' -Samples (New-Samples 5 @(125, 250, 500, 750, 1000))
Add-Test 'five-sample-positive' ([bool]$fivePass.passed -and [int]$fivePass.sample_count -eq 5 -and [double]$fivePass.p50_ms -eq 500 -and [double]$fivePass.worst_ms -eq 1000) ($fivePass | ConvertTo-Json -Depth 8 -Compress)
Add-Test 'threshold-boundary-inclusive' ([bool]$fivePass.samples[-1].within_threshold) ($fivePass.samples[-1] | ConvertTo-Json -Compress)
Add-Test 'no-fake-p95' (-not $fivePass.Contains('p95_ms') -and [bool]$fivePass.p95_reported -eq $false) ($fivePass.Keys -join ',')

$overThreshold = New-Samples 5 @(100, 200, 300, 400, 1000.0000001)
$fiveFail = New-MossPerformanceGateSummary -GateName 'stop_feedback' -Samples $overThreshold
Add-Test 'unrounded-performance-over-threshold-immediate-fail' (-not [bool]$fiveFail.passed -and [int]$fiveFail.first_failed_sample -eq 5 -and [double]$fiveFail.failure_observed_ms -eq 1000 -and -not [bool]$fiveFail.samples[-1].within_threshold) ($fiveFail | ConvertTo-Json -Depth 8 -Compress)

$insufficient = New-MossPerformanceGateSummary -GateName 'summary_completion' -Samples (New-Samples 2 @(1000, 2000))
Add-Test 'insufficient-samples-fail' (-not [bool]$insufficient.passed -and [string]$insufficient.failure_reason -eq 'required_sample_count_not_met') ($insufficient | ConvertTo-Json -Depth 8 -Compress)

$nonMonotonic = New-Samples 5 @(100, 200, 300, 400, 500)
$nonMonotonic[2].completed_monotonic_ms = $nonMonotonic[2].started_monotonic_ms - 1
$nonMonotonicResult = New-MossPerformanceGateSummary -GateName 'stop_feedback' -Samples $nonMonotonic
Add-Test 'non-monotonic-sample-fails' (-not [bool]$nonMonotonicResult.passed -and [string]$nonMonotonicResult.failure_reason -eq 'invalid_monotonic_sample') ($nonMonotonicResult | ConvertTo-Json -Depth 8 -Compress)

$missingEnvironment = New-Samples 5 @(1)
$missingEnvironment[0].Remove('environment')
$missingEnvironmentResult = New-MossPerformanceGateSummary -GateName 'stop_feedback' -Samples $missingEnvironment
Add-Test 'missing-environment-fails' (-not [bool]$missingEnvironmentResult.passed -and [string]$missingEnvironmentResult.failure_reason -eq 'environment_evidence_missing') ($missingEnvironmentResult | ConvertTo-Json -Depth 8 -Compress)

$tailPass = New-MossFinalizationEvidence `
    -ExpectedMarker 'unique-tail-marker-alpha' -FinalTranscript 'opening content unique-tail-marker-alpha' `
    -ExpectedDurationSeconds 100 -LastEndSeconds 98 -ChunksInQueueAtFinalization 0 `
    -TranscriptionIsProcessingAtFinalization $false `
    -TranscriptSha256AtFinalization ('A' * 64) -TranscriptSha256AfterFiveSeconds ('A' * 64) `
    -PauseObserved $true -TranscriptChangeCountBeforeResume 1 -TranscriptChangeCountAfterResume 2
Add-Test 'tail-exact-98-percent-passes' ([bool]$tailPass.tail_marker_present_exactly_once -and [double]$tailPass.tail_coverage_ratio -eq 0.98 -and [bool]$tailPass.tail_coverage_at_least_98_percent -and [bool]$tailPass.chunks_in_queue_at_finalization_zero -and [bool]$tailPass.transcript_stable_after_five_seconds -and [bool]$tailPass.pause_observed -and [bool]$tailPass.transcript_continues_after_resume) ($tailPass | ConvertTo-Json -Compress)

$tailDuplicate = New-MossFinalizationEvidence `
    -ExpectedMarker 'unique-tail-marker' -FinalTranscript 'unique-tail-marker ... unique-tail-marker' `
    -ExpectedDurationSeconds 100 -LastEndSeconds 100 -ChunksInQueueAtFinalization 0 `
    -TranscriptionIsProcessingAtFinalization $false `
    -TranscriptSha256AtFinalization ('B' * 64) -TranscriptSha256AfterFiveSeconds ('B' * 64) `
    -PauseObserved $true -TranscriptChangeCountBeforeResume 0 -TranscriptChangeCountAfterResume 1
Add-Test 'tail-duplicate-fails' (-not [bool]$tailDuplicate.tail_marker_present_exactly_once -and [int]$tailDuplicate.tail_marker_count -eq 2) ($tailDuplicate | ConvertTo-Json -Compress)

$tailQueue = New-MossFinalizationEvidence `
    -ExpectedMarker 'unique-tail-marker' -FinalTranscript 'unique-tail-marker' `
    -ExpectedDurationSeconds 100 -LastEndSeconds 97.99999 -ChunksInQueueAtFinalization 1 `
    -TranscriptionIsProcessingAtFinalization $false `
    -TranscriptSha256AtFinalization ('C' * 64) -TranscriptSha256AfterFiveSeconds ('D' * 64) `
    -PauseObserved $false -TranscriptChangeCountBeforeResume 1 -TranscriptChangeCountAfterResume 1
Add-Test 'tail-unrounded-0.9799999-and-other-boundaries-fail' ([double]$tailQueue.tail_coverage_ratio -eq 0.98 -and -not [bool]$tailQueue.tail_coverage_at_least_98_percent -and -not [bool]$tailQueue.chunks_in_queue_at_finalization_zero -and -not [bool]$tailQueue.transcript_stable_after_five_seconds -and -not [bool]$tailQueue.pause_observed -and -not [bool]$tailQueue.transcript_continues_after_resume) ($tailQueue | ConvertTo-Json -Compress)

$tailWorkerActive = New-MossFinalizationEvidence `
    -ExpectedMarker 'unique-tail-marker' -FinalTranscript 'unique-tail-marker' `
    -ExpectedDurationSeconds 100 -LastEndSeconds 100 -ChunksInQueueAtFinalization 0 `
    -TranscriptionIsProcessingAtFinalization $true `
    -TranscriptSha256AtFinalization ('E' * 64) -TranscriptSha256AfterFiveSeconds ('E' * 64) `
    -PauseObserved $true -TranscriptChangeCountBeforeResume 1 -TranscriptChangeCountAfterResume 2
Add-Test 'worker-active-blocks-finalization' `
    (-not [bool]$tailWorkerActive.transcription_worker_idle_at_finalization -and -not [bool]$tailWorkerActive.finalization_completed) `
    ($tailWorkerActive | ConvertTo-Json -Compress)

$expectedMossPath = 'C:\candidate\moss-helper.exe'
$expectedQwenPath = 'C:\candidate\llama-helper.exe'
$roots = @(
    [ordered]@{ role = 'moss'; executable_path = $expectedMossPath; executable_sha256 = '1' * 64 },
    [ordered]@{ role = 'qwen'; executable_path = $expectedQwenPath; executable_sha256 = '2' * 64 }
)
$inventory = @(
    [ordered]@{ pid = 10; parent_pid = 1; executable_path = $expectedMossPath; executable_sha256 = '1' * 64 },
    [ordered]@{ pid = 11; parent_pid = 10; executable_path = 'C:\candidate\runtime\worker.exe'; executable_sha256 = '3' * 64 },
    [ordered]@{ pid = 12; parent_pid = 1; executable_path = 'D:\unrelated\moss-helper.exe'; executable_sha256 = '4' * 64 },
    [ordered]@{ pid = 20; parent_pid = 1; executable_path = $expectedQwenPath; executable_sha256 = '2' * 64 }
)
$tree = @(Select-MossExactProcessTrees -Inventory $inventory -Roots $roots)
Add-Test 'exact-path-process-tree' (($tree.pid -join ',') -eq '10,11,20' -and @($tree | Where-Object { $_.pid -eq 11 -and $_.root_pid -eq 10 -and $_.depth -eq 1 }).Count -eq 1) ($tree | ConvertTo-Json -Compress)
Add-Test 'same-name-different-path-not-selected' (@($tree | Where-Object { $_.pid -eq 12 }).Count -eq 0) ($tree.pid -join ',')

$lifecyclePass = New-MossHelperExitEvidence -Role 'moss' -ActionCompletedMonotonicMs 1000 -Samples @(
    [ordered]@{ monotonic_ms = 1000; processes = @($tree | Where-Object role -eq 'moss') },
    [ordered]@{ monotonic_ms = 5999.999; processes = @() },
    [ordered]@{ monotonic_ms = 6999.999; processes = @() }
)
Add-Test 'helper-exit-under-five-seconds-with-confirmation' ([bool]$lifecyclePass.passed -and [double]$lifecyclePass.zero_after_ms -eq 4999.999 -and [double]$lifecyclePass.zero_confirmation_interval_ms -eq 1000 -and [bool]$lifecyclePass.consecutive_zero_scans_confirmed -and [int]$lifecyclePass.residual_process_count -eq 0 -and @($lifecyclePass.observed_processes).Count -eq 2) ($lifecyclePass | ConvertTo-Json -Depth 8 -Compress)

$lifecycleBoundary = New-MossHelperExitEvidence -Role 'moss' -ActionCompletedMonotonicMs 1000 -Samples @(
    [ordered]@{ monotonic_ms = 1000; processes = @($tree | Where-Object role -eq 'moss') },
    [ordered]@{ monotonic_ms = 6000; processes = @() },
    [ordered]@{ monotonic_ms = 7000; processes = @() }
)
Add-Test 'helper-exit-five-second-and-one-second-confirmation-boundaries' ([bool]$lifecycleBoundary.passed -and [double]$lifecycleBoundary.zero_after_ms -eq 5000 -and [double]$lifecycleBoundary.zero_confirmation_interval_ms -eq 1000 -and [bool]$lifecycleBoundary.consecutive_zero_scans_confirmed) ($lifecycleBoundary | ConvertTo-Json -Depth 8 -Compress)

$lifecycleFail = New-MossHelperExitEvidence -Role 'moss' -ActionCompletedMonotonicMs 1000 -Samples @(
    [ordered]@{ monotonic_ms = 1000; processes = @($tree | Where-Object role -eq 'moss') },
    [ordered]@{ monotonic_ms = 6000.0000001; processes = @() },
    [ordered]@{ monotonic_ms = 7000.0000001; processes = @() }
)
Add-Test 'helper-exit-unrounded-5000.0000001ms-fails' (-not [bool]$lifecycleFail.passed -and [double]$lifecycleFail.zero_after_ms -eq 5000) ($lifecycleFail | ConvertTo-Json -Depth 8 -Compress)

$singleZero = New-MossHelperExitEvidence -Role 'moss' -ActionCompletedMonotonicMs 1000 -Samples @(
    [ordered]@{ monotonic_ms = 1000; processes = @($tree | Where-Object role -eq 'moss') },
    [ordered]@{ monotonic_ms = 2000; processes = @() }
)
Add-Test 'helper-only-one-zero-scan-fails' (-not [bool]$singleZero.passed -and -not [bool]$singleZero.consecutive_zero_scans_confirmed -and $null -eq $singleZero.second_zero_monotonic_ms) ($singleZero | ConvertTo-Json -Depth 8 -Compress)

$zerosTooClose = New-MossHelperExitEvidence -Role 'moss' -ActionCompletedMonotonicMs 1000 -Samples @(
    [ordered]@{ monotonic_ms = 1000; processes = @($tree | Where-Object role -eq 'moss') },
    [ordered]@{ monotonic_ms = 2000; processes = @() },
    [ordered]@{ monotonic_ms = 2999.9999999; processes = @() }
)
Add-Test 'helper-two-zero-scans-less-than-one-second-apart-fail' (-not [bool]$zerosTooClose.passed -and -not [bool]$zerosTooClose.consecutive_zero_scans_confirmed) ($zerosTooClose | ConvertTo-Json -Depth 8 -Compress)

$processReappears = New-MossHelperExitEvidence -Role 'moss' -ActionCompletedMonotonicMs 1000 -Samples @(
    [ordered]@{ monotonic_ms = 1000; processes = @($tree | Where-Object role -eq 'moss') },
    [ordered]@{ monotonic_ms = 2000; processes = @() },
    [ordered]@{ monotonic_ms = 2500; processes = @($tree | Where-Object role -eq 'moss') },
    [ordered]@{ monotonic_ms = 3000; processes = @() },
    [ordered]@{ monotonic_ms = 4000; processes = @() }
)
Add-Test 'helper-reappearance-between-zero-scans-fails' (-not [bool]$processReappears.passed -and [bool]$processReappears.process_reappeared_after_first_zero -and -not [bool]$processReappears.consecutive_zero_scans_confirmed) ($processReappears | ConvertTo-Json -Depth 8 -Compress)

$parentFirst = New-MossHelperExitEvidence -Role 'moss' -ActionCompletedMonotonicMs 1000 -Samples @(
    [ordered]@{ monotonic_ms = 900; processes = @($tree | Where-Object role -eq 'moss') },
    [ordered]@{ monotonic_ms = 1000; processes = @($tree | Where-Object { $_.role -eq 'moss' -and -not $_.is_root }) },
    [ordered]@{ monotonic_ms = 4000; processes = @() },
    [ordered]@{ monotonic_ms = 5000; processes = @() }
)
Add-Test 'parent-exits-before-child-is-tracked' ([bool]$parentFirst.passed -and @($parentFirst.observed_processes).Count -eq 2) ($parentFirst | ConvertTo-Json -Depth 8 -Compress)

$childResidual = New-MossHelperExitEvidence -Role 'moss' -ActionCompletedMonotonicMs 1000 -Samples @(
    [ordered]@{ monotonic_ms = 1000; processes = @($tree | Where-Object role -eq 'moss') },
    [ordered]@{ monotonic_ms = 6100; processes = @($tree | Where-Object { $_.role -eq 'moss' -and -not $_.is_root }) }
)
Add-Test 'child-residual-after-parent-fails' (-not [bool]$childResidual.passed -and [int]$childResidual.residual_process_count -eq 1) ($childResidual | ConvertTo-Json -Depth 8 -Compress)

$unresponsive = New-MossHelperExitEvidence -Role 'moss' -ActionCompletedMonotonicMs 1000 -Samples @(
    [ordered]@{ monotonic_ms = 1000; processes = @($tree | Where-Object role -eq 'moss') },
    [ordered]@{ monotonic_ms = 6100; processes = @($tree | Where-Object role -eq 'moss') }
)
Add-Test 'unresponsive-helper-fails' (-not [bool]$unresponsive.passed -and [int]$unresponsive.residual_process_count -eq 2) ($unresponsive | ConvertTo-Json -Depth 8 -Compress)

$badIdentityTree = @(Select-MossExactProcessTrees -Inventory @(
    [ordered]@{ pid = 30; parent_pid = 1; executable_path = $expectedMossPath; executable_sha256 = '9' * 64 }
) -Roots $roots)
$badIdentity = New-MossHelperExitEvidence -Role 'moss' -ActionCompletedMonotonicMs 1000 -Samples @(
    [ordered]@{ monotonic_ms = 1000; processes = $badIdentityTree },
    [ordered]@{ monotonic_ms = 1100; processes = @() },
    [ordered]@{ monotonic_ms = 2100; processes = @() }
)
Add-Test 'exact-path-wrong-sha-fails' (-not [bool]$badIdentity.passed -and -not [bool]$badIdentity.process_identity_complete) ($badIdentity | ConvertTo-Json -Depth 8 -Compress)

$exclusionPass = New-MossInferenceExclusionEvidence -Samples @(
    [ordered]@{ monotonic_ms = 0; active_roles = @('moss') },
    [ordered]@{ monotonic_ms = 5; active_roles = @() },
    [ordered]@{ monotonic_ms = 10; active_roles = @('qwen') },
    [ordered]@{ monotonic_ms = 15; active_roles = @() }
) -Attempts @(
    [ordered]@{ requested = 'qwen'; while_active = 'moss'; outcome = 'rejected'; code = 'STORAGE_OPERATION_BUSY' },
    [ordered]@{ requested = 'moss'; while_active = 'qwen'; outcome = 'queued'; code = 'MOSS_QWEN_BUSY' }
)
Add-Test 'moss-qwen-mutual-exclusion-positive' ([bool]$exclusionPass.passed -and [double]$exclusionPass.moss_and_qwen_overlap_seconds -eq 0 -and [int]$exclusionPass.explicit_attempt_count -eq 2) ($exclusionPass | ConvertTo-Json -Depth 8 -Compress)

$exclusionFail = New-MossInferenceExclusionEvidence -Samples @(
    [ordered]@{ monotonic_ms = 0; active_roles = @('moss', 'qwen') },
    [ordered]@{ monotonic_ms = 25; active_roles = @('moss', 'qwen') },
    [ordered]@{ monotonic_ms = 50; active_roles = @() }
) -Attempts @(
    [ordered]@{ requested = 'qwen'; while_active = 'moss'; outcome = 'started'; code = $null },
    [ordered]@{ requested = 'moss'; while_active = 'qwen'; outcome = 'started'; code = $null }
)
Add-Test 'moss-qwen-overlap-fails' (-not [bool]$exclusionFail.passed -and [double]$exclusionFail.moss_and_qwen_overlap_seconds -gt 0) ($exclusionFail | ConvertTo-Json -Depth 8 -Compress)

$missingDirection = New-MossInferenceExclusionEvidence -Samples @(
    [ordered]@{ monotonic_ms = 0; active_roles = @('moss') },
    [ordered]@{ monotonic_ms = 1; active_roles = @() }
) -Attempts @(
    [ordered]@{ requested = 'qwen'; while_active = 'moss'; outcome = 'rejected'; code = 'STORAGE_OPERATION_BUSY' }
)
Add-Test 'missing-mutual-exclusion-direction-fails' (-not [bool]$missingDirection.passed -and -not [bool]$missingDirection.bidirectional_attempts_explicit) ($missingDirection | ConvertTo-Json -Depth 8 -Compress)

$decodeParametersJson = '{"language":"zh","timestamps":"segment","diarize":"on"}'
$decodeContract = [ordered]@{
    languageRequested = 'zh-CN'
    languageResolved = 'zh-CN'
    decodeParametersJson = $decodeParametersJson
    decodeParametersSha256 = Get-MossTextSha256 -Value $decodeParametersJson
}
$decodePass = Test-MossNativeDecodeContract -Value $decodeContract
Add-Test 'native-decode-contract-positive' ([bool]$decodePass.passed) ($decodePass | ConvertTo-Json -Compress)

$decodeMissing = [ordered]@{
    languageRequested = 'zh-CN'
    languageResolved = 'zh-CN'
    decodeParametersJson = $decodeParametersJson
}
$decodeMissingResult = Test-MossNativeDecodeContract -Value $decodeMissing
Add-Test 'native-decode-contract-missing-field-fails' (-not [bool]$decodeMissingResult.passed -and [string]$decodeMissingResult.failure_reason -eq 'required_field_missing') ($decodeMissingResult | ConvertTo-Json -Compress)

$decodeAuto = [ordered]@{
    languageRequested = 'auto'
    languageResolved = 'zh-CN'
    decodeParametersJson = $decodeParametersJson
    decodeParametersSha256 = Get-MossTextSha256 -Value $decodeParametersJson
}
$decodeAutoResult = Test-MossNativeDecodeContract -Value $decodeAuto
Add-Test 'native-decode-contract-auto-fails' (-not [bool]$decodeAutoResult.passed -and [string]$decodeAutoResult.failure_reason -eq 'language_requested_not_explicit_zh_cn') ($decodeAutoResult | ConvertTo-Json -Compress)

$decodeNonChinese = [ordered]@{
    languageRequested = 'zh-CN'
    languageResolved = 'en-US'
    decodeParametersJson = $decodeParametersJson
    decodeParametersSha256 = Get-MossTextSha256 -Value $decodeParametersJson
}
$decodeNonChineseResult = Test-MossNativeDecodeContract -Value $decodeNonChinese
Add-Test 'native-decode-contract-non-zh-cn-fails' (-not [bool]$decodeNonChineseResult.passed -and [string]$decodeNonChineseResult.failure_reason -eq 'language_resolved_not_zh_cn') ($decodeNonChineseResult | ConvertTo-Json -Compress)

$decodeBadHash = [ordered]@{
    languageRequested = 'zh-CN'
    languageResolved = 'zh-CN'
    decodeParametersJson = $decodeParametersJson
    decodeParametersSha256 = '0' * 64
}
$decodeBadHashResult = Test-MossNativeDecodeContract -Value $decodeBadHash
Add-Test 'native-decode-contract-hash-mismatch-fails' (-not [bool]$decodeBadHashResult.passed -and [string]$decodeBadHashResult.failure_reason -eq 'decode_parameters_sha256_mismatch') ($decodeBadHashResult | ConvertTo-Json -Compress)

$runnerText = Get-Content -LiteralPath $runner -Raw -Encoding UTF8
$cdpText = Get-Content -LiteralPath $cdp -Raw -Encoding UTF8
$processMonitorText = Get-Content -LiteralPath $processMonitor -Raw -Encoding UTF8
$operationLockText = Get-Content -LiteralPath $operationLock -Raw -Encoding UTF8
$mossReviewText = Get-Content -LiteralPath $mossReview -Raw -Encoding UTF8
$mossProtocolText = Get-Content -LiteralPath $mossProtocol -Raw -Encoding UTF8
$mossNativeText = Get-Content -LiteralPath $mossNative -Raw -Encoding UTF8
$mossDatabaseText = Get-Content -LiteralPath $mossDatabase -Raw -Encoding UTF8
$mossMigrationText = Get-Content -LiteralPath $mossMigration -Raw -Encoding UTF8
$mossFrontendSchemaText = Get-Content -LiteralPath $mossFrontendSchema -Raw -Encoding UTF8
$requiredRunnerMarkers = @(
    'Get-MossPerformanceGateSpecifications',
    'New-MossPerformanceGateSummary',
    'New-MossFinalizationEvidence',
    'tail_marker_present_exactly_once',
    'tail_coverage_ratio',
    'chunks_in_queue_at_finalization',
    'pause_observed',
    'transcript_continues_after_resume',
    'moss_and_qwen_overlap_seconds',
    'residual_process_count',
    'Test-MossNativeDecodeContract'
)
$missingRunnerMarkers = @($requiredRunnerMarkers | Where-Object { -not $runnerText.Contains($_) })
Add-Test 'runner-produces-p-line-evidence' ($missingRunnerMarkers.Count -eq 0) ('missing=' + ($missingRunnerMarkers -join ','))
Add-Test 'runner-fails-closed-on-process-monitor-safety' `
    ($runnerText.Contains('termination_actions_issued') -and
        $runnerText.Contains('same_name_other_path_untouched') -and
        $runnerText.Contains('first_zero_within_five_seconds') -and
        $runnerText.Contains('consecutive_zero_scans_confirmed') -and
        $runnerText.Contains('process_reappeared_after_first_zero') -and
        $processMonitorText.Contains('firstAllRolesZeroAt') -and
        $processMonitorText.Contains('$at - $firstAllRolesZeroAt -ge 1000') -and
        $processMonitorText.Contains('processReappearedAfterFirstAllRolesZero')) `
    'formal runner must reject monitor kills, touched same-name processes, late first zero, missing confirmation, or reappearance'
Add-Test 'cdp-uses-one-monotonic-clock' ($cdpText.Contains('node_performance_now') -and $cdpText.Contains('performance.now()') -and -not $cdpText.Contains('Date.now() -')) 'six gates must use performance.now, not wall-clock subtraction'
Add-Test 'cdp-has-pause-resume-and-whisper-actions' ($cdpText.Contains('recording-pause-resume') -and $cdpText.Contains('whisper-enhance')) 'missing real product actions'
Add-Test 'cdp-moss-feedback-gates-use-visible-ui-actions' `
    ($cdpText.Contains('prepareMossUi') -and
        $cdpText.Contains('mossStartControls[0].click()') -and
        $cdpText.Contains('uiProcessingVisible') -and
        $cdpText.Contains('mossCancelControls[0].click()') -and
        $cdpText.Contains('uiCancellationVisible') -and
        $cdpText.Contains('pageUsableAfterCancellation')) `
    'MOSS enhance/cancel timing must start at the unique visible control and end at visible UI feedback'
Add-Test 'rust-moss-qwen-conflict-matrix' ($operationLockText.Contains('MossEnhancement') -and $operationLockText.Contains('moss_and_summary_block_each_other')) 'missing bidirectional product mutex test'
Add-Test 'moss-guard-held-by-background-run' ($mossReviewText.Contains('StorageOperationKind::MossEnhancement') -and $mossReviewText.Contains('_moss_operation_guard')) 'MOSS must hold the cross-model guard until the background run ends'
Add-Test 'native-decode-protocol-v3-four-field-chain' `
    ($mossProtocolText.Contains('PROTOCOL_VERSION: u16 = 3') -and
        $mossProtocolText.Contains('language_requested') -and
        $mossProtocolText.Contains('language_resolved') -and
        $mossProtocolText.Contains('decode_parameters_json') -and
        $mossProtocolText.Contains('decode_parameters_sha256')) `
    'protocol v3 must carry all four native decode proof fields'
Add-Test 'native-decoder-receives-explicit-language' `
    ($mossNativeText.Contains('run_params.language = native_language.as_ptr()') -and
        $mossNativeText.Contains('MOSS_LANGUAGE_REQUESTED') -and
        $mossNativeText.Contains('MOSS_LANGUAGE_RESOLVED')) `
    'native ggml_moss run parameters must receive the validated language pointer'
Add-Test 'database-persists-four-native-decode-fields' `
    ($mossDatabaseText.Contains('language_requested = ?, language_resolved = ?') -and
        $mossDatabaseText.Contains('decode_parameters_json = ?, decode_parameters_sha256 = ?') -and
        $mossMigrationText.Contains('ADD COLUMN language_requested') -and
        $mossMigrationText.Contains('ADD COLUMN language_resolved') -and
        $mossMigrationText.Contains('ADD COLUMN decode_parameters_json') -and
        $mossMigrationText.Contains('ADD COLUMN decode_parameters_sha256')) `
    'database record and migration must retain the four proof fields'
Add-Test 'frontend-api-keeps-four-camel-case-fields' `
    ($mossFrontendSchemaText.Contains('languageRequested') -and
        $mossFrontendSchemaText.Contains('languageResolved') -and
        $mossFrontendSchemaText.Contains('decodeParametersJson') -and
        $mossFrontendSchemaText.Contains('decodeParametersSha256')) `
    'frontend API schema must expose the camelCase proof fields'
Add-Test 'cdp-native-decode-evidence-comes-from-api' `
    ($cdpText.Contains('api_moss_get_workspace') -and
        $cdpText.Contains('nativeDecodeContract') -and
        $cdpText.Contains('source: "api_moss_get_workspace.runs"') -and
        -not $cdpText.Contains('primaryLanguage')) `
    'formal MOSS completion evidence must use persisted API fields, never browser language state'

$report = [ordered]@{
    schema_version = 1
    stage = 'MOSS_FUNCTIONAL_FT_P_LINE_CONTRACT_TEST'
    generated_at = [datetimeoffset]::UtcNow.ToString('o')
    total = $tests.Count
    passed = @($tests | Where-Object { $_.passed }).Count
    failed = @($tests | Where-Object { -not $_.passed }).Count
    tests = $tests
}
$reportPath = Join-Path $OutputRoot 'moss-functional-ft-p-line-contract-tests.json'
[System.IO.File]::WriteAllText(
    $reportPath,
    (($report | ConvertTo-Json -Depth 20) + [Environment]::NewLine),
    [System.Text.UTF8Encoding]::new($false)
)
$report | ConvertTo-Json -Depth 20
if ($report.failed -ne 0) { exit 1 }
exit 0

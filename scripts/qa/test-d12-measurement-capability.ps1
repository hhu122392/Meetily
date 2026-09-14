[CmdletBinding()]
param()

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
$script:passed = 0
$script:failed = 0

function Get-SourceText {
    param([Parameter(Mandatory)][string]$RelativePath)

    $path = Join-Path $repoRoot $RelativePath
    if (-not (Test-Path -LiteralPath $path -PathType Leaf)) {
        return ''
    }
    return [System.IO.File]::ReadAllText($path, [System.Text.Encoding]::UTF8)
}

function Assert-ContainsText {
    param(
        [Parameter(Mandatory)][string]$Name,
        [Parameter(Mandatory)][AllowEmptyString()][string]$Text,
        [Parameter(Mandatory)][string]$Needle
    )

    if ($Text.Contains($Needle, [System.StringComparison]::Ordinal)) {
        $script:passed++
        Write-Output "PASS $Name"
    }
    else {
        $script:failed++
        Write-Output "FAIL $Name -- missing literal: $Needle"
    }
}

function Assert-MatchesText {
    param(
        [Parameter(Mandatory)][string]$Name,
        [Parameter(Mandatory)][AllowEmptyString()][string]$Text,
        [Parameter(Mandatory)][string]$Pattern
    )

    if ([regex]::IsMatch($Text, $Pattern, [System.Text.RegularExpressions.RegexOptions]::Singleline)) {
        $script:passed++
        Write-Output "PASS $Name"
    }
    else {
        $script:failed++
        Write-Output "FAIL $Name -- missing regex: $Pattern"
    }
}

$measurement = Get-SourceText 'frontend/src-tauri/src/summary/measurement.rs'
$commands = Get-SourceText 'frontend/src-tauri/src/summary/commands.rs'
$lib = Get-SourceText 'frontend/src-tauri/src/lib.rs'
$processor = Get-SourceText 'frontend/src-tauri/src/summary/processor.rs'
$service = Get-SourceText 'frontend/src-tauri/src/summary/service.rs'
$helper = Get-SourceText 'llama-helper/src/main.rs'
$sidecar = Get-SourceText 'frontend/src-tauri/src/summary/summary_engine/sidecar.rs'
$frontend = Get-SourceText 'frontend/src/hooks/meeting-details/useSummaryGeneration.ts'

Assert-ContainsText 'fixed timed endpoint' $measurement 'TIMED_ENDPOINT_ID: &str = "click_to_page_display_complete"'

foreach ($stage in @(
    'wait_for_transcription',
    'prepare_input',
    'load_model',
    'chunk_summaries',
    'combine',
    'final_template',
    'translation',
    'save',
    'page_display_complete'
)) {
    Assert-ContainsText "stage $stage" $measurement "`"$stage`""
}
Assert-MatchesText 'stage monotonic bounds' $measurement 'monotonic_start_ns.*monotonic_end_ns'

foreach ($field in @(
    'input_transcript_sha256',
    'database_completed_body_sha256',
    'page_displayed_body_sha256',
    'page_matches_database'
)) {
    Assert-ContainsText "body hash $field" $measurement $field
}

foreach ($field in @(
    'helper_process_start_count',
    'job_distinct_pids',
    'model_loaded_event_count',
    'cleanup_entry_count',
    'graceful_shutdown_request_count',
    'job_confirm_zero_count'
)) {
    Assert-ContainsText "helper lifecycle $field" $measurement $field
}

foreach ($field in @(
    'cpu_usage_percent',
    'gpu_usage_percent',
    'memory_commit_bytes',
    'load_timeline_sha256'
)) {
    Assert-ContainsText "resource timeline $field" $measurement $field
}

Assert-ContainsText 'performance eligibility flag' $measurement 'eligible_for_performance'
foreach ($outcome in @('failed', 'cancelled', 'save_failed')) {
    Assert-ContainsText "excluded outcome $outcome" $measurement "`"$outcome`""
}

foreach ($command in @(
    'api_begin_summary_measurement',
    'api_record_summary_frontend_stage',
    'api_record_summary_page_completion'
)) {
    Assert-ContainsText "backend command $command" $commands "pub async fn $command"
    Assert-ContainsText "registered command $command" $lib "summary::commands::$command"
    Assert-ContainsText "frontend invocation $command" $frontend $command
}

$summaryPipeline = $commands + $processor + $service + $sidecar
foreach ($stage in @(
    'prepare_input',
    'load_model',
    'chunk_summaries',
    'combine',
    'final_template',
    'translation',
    'save'
)) {
    Assert-MatchesText "pipeline stage $stage" $summaryPipeline "(?:record_stage_start|record_stage_end|stage_guard)[^\r\n]*$stage"
}

Assert-MatchesText 'helper structured model_loaded event' $helper '"event"\s*:\s*"model_loaded"'
foreach ($event in @(
    'helper_process_start',
    'model_loaded',
    'cleanup_entry',
    'graceful_shutdown_request',
    'job_confirm_zero'
)) {
    Assert-ContainsText "sidecar event $event" $sidecar "`"$event`""
}
Assert-MatchesText 'sidecar Job PID capture' $sidecar 'job_(?:process_ids|distinct_pids)'
Assert-ContainsText 'frontend generation binding' $frontend 'generationId'
Assert-MatchesText 'frontend page completion stage' $frontend 'page_display_complete'

$total = $script:passed + $script:failed
Write-Output "D-12 measurement contract: $($script:passed) passed, $($script:failed) failed, $total total"
if ($script:failed -gt 0) {
    exit 1
}
exit 0

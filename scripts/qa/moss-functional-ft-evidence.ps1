Set-StrictMode -Version Latest

function Get-MossMapValue {
    param($Value, [Parameter(Mandatory = $true)][string]$Name)
    if ($null -eq $Value) { return $null }
    if ($Value -is [System.Collections.IDictionary]) {
        if ($Value.Contains($Name)) { return $Value[$Name] }
        return $null
    }
    $property = $Value.PSObject.Properties[$Name]
    if ($null -eq $property) { return $null }
    return $property.Value
}

function Test-MossMapKeysPresent {
    param($Value, [Parameter(Mandatory = $true)][string[]]$Names)
    foreach ($name in $Names) {
        if ($null -eq (Get-MossMapValue -Value $Value -Name $name)) { return $false }
    }
    return $true
}

function Get-MossPerformanceGateSpecifications {
    return [ordered]@{
        stop_feedback = [ordered]@{ threshold_ms = [double]1000; required_samples = 5 }
        page_unlock = [ordered]@{ threshold_ms = [double]10000; required_samples = 5 }
        enhance_feedback = [ordered]@{ threshold_ms = [double]1000; required_samples = 5 }
        cancel_feedback = [ordered]@{ threshold_ms = [double]1000; required_samples = 5 }
        summary_completion = [ordered]@{ threshold_ms = [double]180000; required_samples = 3 }
        whisper_enhancement_completion = [ordered]@{ threshold_ms = [double]300000; required_samples = 3 }
    }
}

function New-MossPerformanceGateSummary {
    param(
        [Parameter(Mandatory = $true)][ValidateSet(
            'stop_feedback', 'page_unlock', 'enhance_feedback', 'cancel_feedback',
            'summary_completion', 'whisper_enhancement_completion'
        )][string]$GateName,
        [Parameter(Mandatory = $true)]$Samples
    )

    $specification = (Get-MossPerformanceGateSpecifications)[$GateName]
    $requiredEnvironment = @('cpu', 'memory', 'gpu', 'disk', 'power', 'model_state')
    $normalized = @()
    $invalidSample = $null
    $missingEnvironment = $null
    $thresholdFailure = $null
    $index = 0
    foreach ($sample in @($Samples)) {
        $index++
        $clock = [string](Get-MossMapValue $sample 'clock')
        $started = Get-MossMapValue $sample 'started_monotonic_ms'
        $completed = Get-MossMapValue $sample 'completed_monotonic_ms'
        $environment = Get-MossMapValue $sample 'environment'
        $validClock = $clock -eq 'node_performance_now'
        $validNumbers = $null -ne $started -and $null -ne $completed -and
            [double]::TryParse([string]$started, [ref]([double]$parsedStarted = 0)) -and
            [double]::TryParse([string]$completed, [ref]([double]$parsedCompleted = 0)) -and
            [double]$completed -ge [double]$started
        $elapsedUnrounded = if ($validNumbers) { [double]$completed - [double]$started } else { $null }
        $elapsed = if ($validNumbers) { [math]::Round($elapsedUnrounded, 6) } else { $null }
        $environmentComplete = Test-MossMapKeysPresent -Value $environment -Names $requiredEnvironment
        $within = $validClock -and $validNumbers -and $environmentComplete -and
            [double]$elapsedUnrounded -le [double]$specification.threshold_ms
        $normalized += [ordered]@{
            index = $index
            label = [string](Get-MossMapValue $sample 'label')
            clock = $clock
            started_monotonic_ms = $started
            completed_monotonic_ms = $completed
            elapsed_ms = $elapsed
            threshold_ms = [double]$specification.threshold_ms
            within_threshold = $within
            environment = $environment
        }
        if (($null -eq $invalidSample) -and (-not $validClock -or -not $validNumbers)) { $invalidSample = $index }
        if (($null -eq $missingEnvironment) -and -not $environmentComplete) { $missingEnvironment = $index }
        if (($null -eq $thresholdFailure) -and $validClock -and $validNumbers -and $environmentComplete -and
            [double]$elapsedUnrounded -gt [double]$specification.threshold_ms) { $thresholdFailure = $index }
    }

    $requiredCountMet = $normalized.Count -eq [int]$specification.required_samples
    $failureReason = $null
    $firstFailedSample = $null
    if (-not $requiredCountMet) {
        $failureReason = 'required_sample_count_not_met'
    } elseif ($null -ne $invalidSample) {
        $failureReason = 'invalid_monotonic_sample'
        $firstFailedSample = $invalidSample
    } elseif ($null -ne $missingEnvironment) {
        $failureReason = 'environment_evidence_missing'
        $firstFailedSample = $missingEnvironment
    } elseif ($null -ne $thresholdFailure) {
        $failureReason = 'threshold_exceeded'
        $firstFailedSample = $thresholdFailure
    }
    $elapsedValues = @($normalized | Where-Object { $null -ne $_.elapsed_ms } | ForEach-Object { [double]$_.elapsed_ms } | Sort-Object)
    $p50 = if ($elapsedValues.Count -gt 0) { $elapsedValues[[int][math]::Floor($elapsedValues.Count / 2)] } else { $null }
    $worst = if ($elapsedValues.Count -gt 0) { $elapsedValues[-1] } else { $null }
    $failureObserved = if ($null -ne $firstFailedSample) { $normalized[[int]$firstFailedSample - 1].elapsed_ms } else { $null }
    return [ordered]@{
        gate = $GateName
        clock = 'node_performance_now'
        threshold_ms = [double]$specification.threshold_ms
        required_samples = [int]$specification.required_samples
        sample_count = $normalized.Count
        required_sample_count_met = $requiredCountMet
        samples = $normalized
        p50_ms = $p50
        worst_ms = $worst
        p95_reported = $false
        passed = $null -eq $failureReason
        failure_reason = $failureReason
        first_failed_sample = $firstFailedSample
        failure_observed_ms = $failureObserved
    }
}

function Get-MossPerformanceEnvironmentSnapshot {
    param([ValidateSet('cold', 'warm', 'not_applicable')][string]$ModelState = 'not_applicable')

    $capturedAt = [datetimeoffset]::UtcNow.ToString('o')
    $capturedMonotonic = [math]::Round(([System.Diagnostics.Stopwatch]::GetTimestamp() * 1000.0 / [System.Diagnostics.Stopwatch]::Frequency), 6)
    $cpu = [ordered]@{ available = $false; load_percent = $null; name = $null; error = $null }
    $memory = [ordered]@{ available = $false; used_percent = $null; total_bytes = $null; available_bytes = $null; error = $null }
    $gpu = [ordered]@{ available = $false; utilization_percent = $null; error = $null }
    $disk = [ordered]@{ available = $false; utilization_percent = $null; error = $null }
    $power = [ordered]@{ available = $false; source = $null; scheme = $null; error = $null }

    try {
        $processors = @(Get-CimInstance Win32_Processor -ErrorAction Stop)
        $loads = @($processors | Where-Object { $null -ne $_.LoadPercentage } | ForEach-Object { [double]$_.LoadPercentage })
        $cpu.available = $processors.Count -gt 0 -and $loads.Count -gt 0
        $cpu.load_percent = if ($loads.Count -gt 0) { [math]::Round((($loads | Measure-Object -Average).Average), 3) } else { $null }
        $cpu.name = (($processors | ForEach-Object { [string]$_.Name }) -join '; ')
    } catch { $cpu.error = $_.Exception.Message }

    try {
        $os = Get-CimInstance Win32_OperatingSystem -ErrorAction Stop
        $total = [int64]$os.TotalVisibleMemorySize * 1024
        $available = [int64]$os.FreePhysicalMemory * 1024
        $memory.available = $total -gt 0
        $memory.total_bytes = $total
        $memory.available_bytes = $available
        $memory.used_percent = if ($total -gt 0) { [math]::Round((($total - $available) * 100.0 / $total), 3) } else { $null }
    } catch { $memory.error = $_.Exception.Message }

    try {
        $engines = @(Get-CimInstance Win32_PerfFormattedData_GPUPerformanceCounters_GPUEngine -ErrorAction Stop |
            Where-Object { [double]$_.UtilizationPercentage -gt 0 })
        $gpu.available = $true
        $gpu.utilization_percent = if ($engines.Count -gt 0) {
            [math]::Round([math]::Min(100.0, [double](($engines | Measure-Object UtilizationPercentage -Sum).Sum)), 3)
        } else { [double]0 }
    } catch { $gpu.error = $_.Exception.Message }

    try {
        $physicalDisk = Get-CimInstance Win32_PerfFormattedData_PerfDisk_PhysicalDisk -Filter "Name='_Total'" -ErrorAction Stop
        $disk.available = $null -ne $physicalDisk
        $disk.utilization_percent = if ($null -ne $physicalDisk) { [math]::Round([double]$physicalDisk.PercentDiskTime, 3) } else { $null }
    } catch { $disk.error = $_.Exception.Message }

    try {
        $schemeOutput = (& powercfg.exe /getactivescheme 2>&1 | Out-String).Trim()
        if ($LASTEXITCODE -ne 0) { throw "powercfg exited with code $LASTEXITCODE" }
        $battery = @(Get-CimInstance Win32_Battery -ErrorAction SilentlyContinue)
        $source = if ($battery.Count -eq 0) { 'ac_no_battery' } elseif (@($battery | Where-Object { [int]$_.BatteryStatus -in @(1, 4, 5) }).Count -gt 0) { 'battery' } else { 'ac' }
        $power.available = $true
        $power.source = $source
        $power.scheme = $schemeOutput
    } catch { $power.error = $_.Exception.Message }

    return [ordered]@{
        captured_at = $capturedAt
        captured_monotonic_ms = $capturedMonotonic
        cpu = $cpu
        memory = $memory
        gpu = $gpu
        disk = $disk
        power = $power
        model_state = $ModelState
    }
}

function Get-MossOrdinalSubstringCount {
    param([Parameter(Mandatory = $true)][string]$Text, [Parameter(Mandatory = $true)][string]$Needle)
    if ([string]::IsNullOrWhiteSpace($Needle)) { throw 'Tail marker must not be empty.' }
    $count = 0
    $offset = 0
    while ($offset -le $Text.Length - $Needle.Length) {
        $next = $Text.IndexOf($Needle, $offset, [System.StringComparison]::Ordinal)
        if ($next -lt 0) { break }
        $count++
        $offset = $next + $Needle.Length
    }
    return $count
}

function New-MossFinalizationEvidence {
    param(
        [Parameter(Mandatory = $true)][string]$ExpectedMarker,
        [Parameter(Mandatory = $true)][string]$FinalTranscript,
        [Parameter(Mandatory = $true)][double]$ExpectedDurationSeconds,
        [Parameter(Mandatory = $true)][double]$LastEndSeconds,
        [Parameter(Mandatory = $true)][int]$ChunksInQueueAtFinalization,
        [Parameter(Mandatory = $true)][bool]$TranscriptionIsProcessingAtFinalization,
        [Parameter(Mandatory = $true)][string]$TranscriptSha256AtFinalization,
        [Parameter(Mandatory = $true)][string]$TranscriptSha256AfterFiveSeconds,
        [Parameter(Mandatory = $true)][bool]$PauseObserved,
        [Parameter(Mandatory = $true)][int]$TranscriptChangeCountBeforeResume,
        [Parameter(Mandatory = $true)][int]$TranscriptChangeCountAfterResume
    )
    if ($ExpectedDurationSeconds -le 0) { throw 'Expected duration must be positive.' }
    $markerCount = Get-MossOrdinalSubstringCount -Text $FinalTranscript -Needle $ExpectedMarker
    $coverageUnrounded = $LastEndSeconds / $ExpectedDurationSeconds
    $coverage = [math]::Round($coverageUnrounded, 6)
    $hashesValid = $TranscriptSha256AtFinalization -match '^[0-9A-Fa-f]{64}$' -and
        $TranscriptSha256AfterFiveSeconds -match '^[0-9A-Fa-f]{64}$'
    $stable = $hashesValid -and $TranscriptSha256AtFinalization.Equals(
        $TranscriptSha256AfterFiveSeconds,
        [System.StringComparison]::OrdinalIgnoreCase
    )
    $queueZero = $ChunksInQueueAtFinalization -eq 0
    $workerIdle = -not $TranscriptionIsProcessingAtFinalization
    return [ordered]@{
        tail_marker_sha256 = Get-MossTextSha256 -Value $ExpectedMarker
        tail_marker_count = $markerCount
        tail_marker_present_exactly_once = $markerCount -eq 1
        tail_coverage_ratio = $coverage
        tail_coverage_at_least_98_percent = $coverageUnrounded -ge 0.98
        chunks_in_queue_at_finalization = $ChunksInQueueAtFinalization
        chunks_in_queue_at_finalization_zero = $queueZero
        transcription_is_processing_at_finalization = $TranscriptionIsProcessingAtFinalization
        transcription_worker_idle_at_finalization = $workerIdle
        finalization_completed = $queueZero -and $workerIdle
        transcript_sha256_at_finalization = $TranscriptSha256AtFinalization.ToUpperInvariant()
        transcript_sha256_after_five_seconds = $TranscriptSha256AfterFiveSeconds.ToUpperInvariant()
        transcript_stable_after_five_seconds = $stable
        pause_observed = $PauseObserved
        transcript_change_count_before_resume = $TranscriptChangeCountBeforeResume
        transcript_change_count_after_resume = $TranscriptChangeCountAfterResume
        transcript_continues_after_resume = $PauseObserved -and $TranscriptChangeCountAfterResume -gt $TranscriptChangeCountBeforeResume
    }
}

function Get-MossTextSha256 {
    param([Parameter(Mandatory = $true)][string]$Value)
    $algorithm = [System.Security.Cryptography.SHA256]::Create()
    try {
        return ([BitConverter]::ToString($algorithm.ComputeHash([System.Text.Encoding]::UTF8.GetBytes($Value)))).Replace('-', '')
    } finally {
        $algorithm.Dispose()
    }
}

function Test-MossNativeDecodeContract {
    param([Parameter(Mandatory = $true)]$Value)

    $requiredFields = @(
        'languageRequested',
        'languageResolved',
        'decodeParametersJson',
        'decodeParametersSha256'
    )
    $missingFields = @($requiredFields | Where-Object {
        $fieldValue = Get-MossMapValue -Value $Value -Name $_
        $null -eq $fieldValue -or [string]::IsNullOrWhiteSpace([string]$fieldValue)
    })
    $failureReason = $null
    $parsedParameters = $null
    $recomputedSha256 = $null
    if ($missingFields.Count -gt 0) {
        $failureReason = 'required_field_missing'
    } elseif ([string](Get-MossMapValue $Value 'languageRequested') -ne 'zh-CN') {
        $failureReason = 'language_requested_not_explicit_zh_cn'
    } elseif ([string](Get-MossMapValue $Value 'languageResolved') -ne 'zh-CN') {
        $failureReason = 'language_resolved_not_zh_cn'
    } else {
        $decodeParametersJson = [string](Get-MossMapValue $Value 'decodeParametersJson')
        try {
            $parsedParameters = $decodeParametersJson | ConvertFrom-Json -ErrorAction Stop
        } catch {
            $failureReason = 'decode_parameters_json_invalid'
        }
        if ($null -eq $failureReason) {
            $parameterNames = @($parsedParameters.PSObject.Properties.Name)
            $expectedNames = @('language', 'timestamps', 'diarize')
            if (($parameterNames -join '|') -ne ($expectedNames -join '|') -or
                [string]$parsedParameters.language -ne 'zh' -or
                [string]$parsedParameters.timestamps -ne 'segment' -or
                [string]$parsedParameters.diarize -ne 'on') {
                $failureReason = 'decode_parameters_not_frozen_zh_cn'
            }
        }
        $recomputedSha256 = Get-MossTextSha256 -Value $decodeParametersJson
        if ($null -eq $failureReason -and -not $recomputedSha256.Equals(
            [string](Get-MossMapValue $Value 'decodeParametersSha256'),
            [System.StringComparison]::OrdinalIgnoreCase
        )) {
            $failureReason = 'decode_parameters_sha256_mismatch'
        }
    }
    return [ordered]@{
        passed = $null -eq $failureReason
        failure_reason = $failureReason
        missing_fields = $missingFields
        language_requested = Get-MossMapValue $Value 'languageRequested'
        language_resolved = Get-MossMapValue $Value 'languageResolved'
        decode_parameters_json = Get-MossMapValue $Value 'decodeParametersJson'
        decode_parameters_sha256 = Get-MossMapValue $Value 'decodeParametersSha256'
        recomputed_decode_parameters_sha256 = $recomputedSha256
    }
}

function Get-MossNormalizedPathForComparison {
    param([Parameter(Mandatory = $true)][string]$Path)
    return ([System.IO.Path]::GetFullPath($Path)).TrimEnd('\', '/').ToLowerInvariant()
}

function Select-MossExactProcessTrees {
    param(
        [Parameter(Mandatory = $true)]$Inventory,
        [Parameter(Mandatory = $true)]$Roots
    )
    $inventoryItems = @($Inventory)
    $result = @()
    foreach ($root in @($Roots)) {
        $role = [string](Get-MossMapValue $root 'role')
        $expectedPath = [string](Get-MossMapValue $root 'executable_path')
        $expectedSha = [string](Get-MossMapValue $root 'executable_sha256')
        if ([string]::IsNullOrWhiteSpace($role) -or [string]::IsNullOrWhiteSpace($expectedPath) -or $expectedSha -notmatch '^[0-9A-Fa-f]{64}$') {
            throw 'Every exact process root requires role, executable_path and executable_sha256.'
        }
        $normalizedExpectedPath = Get-MossNormalizedPathForComparison $expectedPath
        $rootProcesses = @($inventoryItems | Where-Object {
            $pathValue = [string](Get-MossMapValue $_ 'executable_path')
            -not [string]::IsNullOrWhiteSpace($pathValue) -and
                (Get-MossNormalizedPathForComparison $pathValue) -eq $normalizedExpectedPath
        })
        foreach ($rootProcess in $rootProcesses) {
            $rootPid = [int](Get-MossMapValue $rootProcess 'pid')
            $queue = [System.Collections.Generic.Queue[object]]::new()
            $queue.Enqueue([ordered]@{ process = $rootProcess; depth = 0 })
            $visited = [System.Collections.Generic.HashSet[int]]::new()
            while ($queue.Count -gt 0) {
                $entry = $queue.Dequeue()
                $process = $entry.process
                $pidValue = [int](Get-MossMapValue $process 'pid')
                if (-not $visited.Add($pidValue)) { continue }
                $processSha = [string](Get-MossMapValue $process 'executable_sha256')
                $result += [ordered]@{
                    role = $role
                    root_role = $role
                    root_pid = $rootPid
                    pid = $pidValue
                    parent_pid = [int](Get-MossMapValue $process 'parent_pid')
                    depth = [int]$entry.depth
                    is_root = [int]$entry.depth -eq 0
                    executable_path = [string](Get-MossMapValue $process 'executable_path')
                    executable_sha256 = $processSha
                    root_identity_valid = ([int]$entry.depth -ne 0) -or $processSha.Equals($expectedSha, [System.StringComparison]::OrdinalIgnoreCase)
                }
                foreach ($child in @($inventoryItems | Where-Object { [int](Get-MossMapValue $_ 'parent_pid') -eq $pidValue })) {
                    $queue.Enqueue([ordered]@{ process = $child; depth = [int]$entry.depth + 1 })
                }
            }
        }
    }
    return @($result | Sort-Object @{ Expression = { [string]$_.root_role } }, @{ Expression = { [int]$_.root_pid } }, @{ Expression = { [int]$_.depth } }, @{ Expression = { [int]$_.pid } })
}

function New-MossHelperExitEvidence {
    param(
        [Parameter(Mandatory = $true)][ValidateSet('moss', 'qwen')][string]$Role,
        [Parameter(Mandatory = $true)][double]$ActionCompletedMonotonicMs,
        [Parameter(Mandatory = $true)]$Samples
    )
    [array]$orderedSamples = @($Samples | Sort-Object { [double](Get-MossMapValue $_ 'monotonic_ms') })
    [array]$observed = @()
    [array]$normalizedSamples = @()
    $firstZeroAt = $null
    $secondZeroAt = $null
    $processReappearedAfterFirstZero = $false
    foreach ($sample in $orderedSamples) {
        $at = [double](Get-MossMapValue $sample 'monotonic_ms')
        [array]$processes = @((Get-MossMapValue $sample 'processes') | Where-Object { [string](Get-MossMapValue $_ 'role') -eq $Role })
        $normalizedSamples += [ordered]@{ monotonic_ms = $at; process_count = $processes.Count; processes = $processes }
        $observed += $processes
        if ($at -lt $ActionCompletedMonotonicMs) { continue }
        if ($processes.Count -eq 0) {
            if ($null -eq $firstZeroAt) {
                $firstZeroAt = $at
            } elseif ($null -eq $secondZeroAt -and -not $processReappearedAfterFirstZero -and
                $at - $firstZeroAt -ge 1000) {
                $secondZeroAt = $at
            }
        } elseif ($null -ne $firstZeroAt) {
            $processReappearedAfterFirstZero = $true
        }
    }
    [array]$unique = @($observed | Sort-Object `
        @{ Expression = { [int](Get-MossMapValue $_ 'pid') } }, `
        @{ Expression = { [string](Get-MossMapValue $_ 'executable_path') } } -Unique)
    $identityComplete = $unique.Count -gt 0 -and @($unique | Where-Object {
        $null -eq (Get-MossMapValue $_ 'parent_pid') -or
        [string](Get-MossMapValue $_ 'executable_sha256') -notmatch '^[0-9A-Fa-f]{64}$' -or
        -not [bool](Get-MossMapValue $_ 'root_identity_valid')
    }).Count -eq 0
    [array]$lastProcesses = @()
    if ($normalizedSamples.Count -gt 0) {
        [array]$lastProcesses = @($normalizedSamples[-1].processes)
    }
    $zeroAfterUnrounded = if ($null -ne $firstZeroAt) { [double]$firstZeroAt - $ActionCompletedMonotonicMs } else { $null }
    $zeroAfter = if ($null -ne $zeroAfterUnrounded) { [math]::Round($zeroAfterUnrounded, 6) } else { $null }
    $confirmationIntervalUnrounded = if ($null -ne $secondZeroAt) { [double]$secondZeroAt - [double]$firstZeroAt } else { $null }
    $confirmationInterval = if ($null -ne $confirmationIntervalUnrounded) { [math]::Round($confirmationIntervalUnrounded, 6) } else { $null }
    $firstZeroWithinThreshold = $null -ne $zeroAfterUnrounded -and
        [double]$zeroAfterUnrounded -ge 0 -and [double]$zeroAfterUnrounded -le 5000
    $consecutiveZeroScansConfirmed = $null -ne $confirmationIntervalUnrounded -and
        [double]$confirmationIntervalUnrounded -ge 1000 -and -not $processReappearedAfterFirstZero
    return [ordered]@{
        role = $Role
        threshold_ms = [double]5000
        action_completed_monotonic_ms = $ActionCompletedMonotonicMs
        zero_monotonic_ms = $firstZeroAt
        first_zero_monotonic_ms = $firstZeroAt
        second_zero_monotonic_ms = $secondZeroAt
        zero_after_ms = $zeroAfter
        zero_confirmation_interval_ms = $confirmationInterval
        first_zero_within_five_seconds = $firstZeroWithinThreshold
        process_reappeared_after_first_zero = $processReappearedAfterFirstZero
        consecutive_zero_scans_confirmed = $consecutiveZeroScansConfirmed
        observed_processes = $unique
        process_identity_complete = $identityComplete
        samples = $normalizedSamples
        residual_process_count = $lastProcesses.Count
        passed = $identityComplete -and $firstZeroWithinThreshold -and
            $consecutiveZeroScansConfirmed -and $lastProcesses.Count -eq 0
    }
}

function New-MossInferenceExclusionEvidence {
    param(
        [Parameter(Mandatory = $true)]$Samples,
        [Parameter(Mandatory = $true)]$Attempts
    )
    $orderedSamples = @($Samples | Sort-Object { [double](Get-MossMapValue $_ 'monotonic_ms') })
    $overlapMilliseconds = [double]0
    $overlapDetected = $false
    for ($index = 0; $index -lt $orderedSamples.Count; $index++) {
        $roles = @((Get-MossMapValue $orderedSamples[$index] 'active_roles') | ForEach-Object { [string]$_ })
        $both = $roles -contains 'moss' -and $roles -contains 'qwen'
        if ($both) {
            $overlapDetected = $true
            if ($index + 1 -lt $orderedSamples.Count) {
                $current = [double](Get-MossMapValue $orderedSamples[$index] 'monotonic_ms')
                $next = [double](Get-MossMapValue $orderedSamples[$index + 1] 'monotonic_ms')
                if ($next -ge $current) { $overlapMilliseconds += $next - $current }
            }
        }
    }
    $attemptItems = @($Attempts)
    $requiredDirections = @(
        [ordered]@{ requested = 'qwen'; while_active = 'moss' },
        [ordered]@{ requested = 'moss'; while_active = 'qwen' }
    )
    $directionsValid = $true
    foreach ($direction in $requiredDirections) {
        $matches = @($attemptItems | Where-Object {
            [string](Get-MossMapValue $_ 'requested') -eq $direction.requested -and
            [string](Get-MossMapValue $_ 'while_active') -eq $direction.while_active -and
            [string](Get-MossMapValue $_ 'outcome') -in @('rejected', 'queued') -and
            -not [string]::IsNullOrWhiteSpace([string](Get-MossMapValue $_ 'code'))
        })
        if ($matches.Count -ne 1) { $directionsValid = $false }
    }
    return [ordered]@{
        samples = $orderedSamples
        attempts = $attemptItems
        explicit_attempt_count = @($attemptItems | Where-Object { [string](Get-MossMapValue $_ 'outcome') -in @('rejected', 'queued') }).Count
        bidirectional_attempts_explicit = $directionsValid
        overlap_detected = $overlapDetected
        moss_and_qwen_overlap_seconds = [math]::Round(($overlapMilliseconds / 1000.0), 6)
        passed = -not $overlapDetected -and $directionsValid
    }
}

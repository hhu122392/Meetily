[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string]$EvidenceRoot,

    [string]$OutputPath,

    [string]$RepositoryRoot = (Get-Location).Path
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$root = [System.IO.Path]::GetFullPath($EvidenceRoot)
if (-not (Test-Path -LiteralPath $root -PathType Container)) {
    throw "Evidence root does not exist: $root"
}
if ([string]::IsNullOrWhiteSpace($OutputPath)) {
    $OutputPath = Join-Path $root 'evidence-validation.json'
}
$fullOutput = [System.IO.Path]::GetFullPath($OutputPath)
$repoRoot = [System.IO.Path]::GetFullPath($RepositoryRoot)
$failures = [System.Collections.Generic.List[string]]::new()
$checks = [System.Collections.Generic.List[object]]::new()

function Add-Check {
    param([string]$Name, [bool]$Passed, [string]$Detail)
    $checks.Add([pscustomobject]@{ name = $Name; passed = $Passed; detail = $Detail })
    if (-not $Passed) { $failures.Add("${Name}: ${Detail}") }
}

function Get-Sha256([string]$Path) {
    (Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash
}

$required = @(
    'fixture-generator-output.json',
    'fixture-registration-addendum.json',
    'fixed-passphrase.wav',
    'truth.txt',
    'windows-audio-endpoints.json',
    'windows-audio-endpoints-after.json',
    'pnp-audio-endpoints.json',
    'machine-context.json',
    'driver-mute-fact.json',
    'driver-mute-fact.sha256.txt'
)
foreach ($relative in $required) {
    Add-Check "required:$relative" (Test-Path -LiteralPath (Join-Path $root $relative) -PathType Leaf) 'required file exists'
}

$fixture = Get-Content -LiteralPath (Join-Path $root 'fixture-generator-output.json') -Raw -Encoding utf8 | ConvertFrom-Json
$registration = Get-Content -LiteralPath (Join-Path $root 'fixture-registration-addendum.json') -Raw -Encoding utf8 | ConvertFrom-Json
$audioHash = Get-Sha256 (Join-Path $root 'fixed-passphrase.wav')
$truthHash = Get-Sha256 (Join-Path $root 'truth.txt')
Add-Check 'fixture:audio-sha256' ($audioHash -ceq $fixture.audio_sha256) "$audioHash"
Add-Check 'fixture:truth-sha256' ($truthHash -ceq $fixture.truth_file_sha256) "$truthHash"
Add-Check 'fixture:duration' ([double]$fixture.duration_seconds -ge 20 -and [double]$fixture.duration_seconds -le 30) "$($fixture.duration_seconds) seconds"
Add-Check 'fixture:format' ($fixture.sample_rate_hz -eq 16000 -and $fixture.channels -eq 1 -and $fixture.bits_per_sample -eq 16) '16000Hz mono PCM16'
Add-Check 'registration:original-file-sha256' ((Get-Sha256 (Join-Path $root $registration.original_registration_file)) -ceq $registration.original_registration_sha256) $registration.original_registration_sha256
Add-Check 'registration:audio-sha256' ($audioHash -ceq $registration.audio_sha256) $audioHash
Add-Check 'registration:truth-sha256' ($truthHash -ceq $registration.truth_file_sha256) $truthHash
foreach ($source in @(
    @{ Name = 'player'; Path = $registration.player_source; Hash = $registration.player_source_sha256 },
    @{ Name = 'fixture-generator'; Path = $registration.fixture_generator_source; Hash = $registration.fixture_generator_sha256 },
    @{ Name = 'diagnostic-recognizer'; Path = $registration.diagnostic_recognizer_source; Hash = $registration.diagnostic_recognizer_sha256 }
)) {
    $sourcePath = Join-Path $repoRoot $source.Path
    $exists = Test-Path -LiteralPath $sourcePath -PathType Leaf
    Add-Check "registration:$($source.Name)-exists" $exists $source.Path
    if ($exists) {
        $actual = Get-Sha256 $sourcePath
        Add-Check "registration:$($source.Name)-sha256" ($actual -ceq $source.Hash) $actual
    }
}

$scenarioResults = @(
    'scenarios/D10A-01/result.json',
    'scenarios/D10A-01-attempt-01-failed/result.json',
    'scenarios/D10A-02/result.json',
    'scenarios/D10A-03/result.json',
    'scenarios/D10A-04/result.json',
    'scenarios/D10A-05/result.json',
    'scenarios/D10A-06/result.json',
    'scenarios/D10A-07-unmuted/result.json',
    'scenarios/D10A-07-muted/result.json'
)
$verifiedPcmFiles = 0
foreach ($relative in $scenarioResults) {
    $path = Join-Path $root $relative
    Add-Check "scenario-result:$relative" (Test-Path -LiteralPath $path -PathType Leaf) 'scenario result exists'
    if (-not (Test-Path -LiteralPath $path -PathType Leaf)) { continue }
    $payload = Get-Content -LiteralPath $path -Raw -Encoding utf8 | ConvertFrom-Json
    $hasRoutes = $payload.PSObject.Properties.Name -contains 'routes'
    if ($hasRoutes -and $null -ne $payload.routes) {
        foreach ($route in $payload.routes) {
            $preMixFile = if ($route.PSObject.Properties.Name -contains 'pre_mix_pcm_file') {
                [string]$route.pre_mix_pcm_file
            } else {
                ''
            }
            if (-not [string]::IsNullOrWhiteSpace($preMixFile)) {
                $pcmPath = Join-Path (Split-Path -Parent $path) $preMixFile
                $exists = Test-Path -LiteralPath $pcmPath -PathType Leaf
                Add-Check "pre-mix-file:${relative}:$($route.route)" $exists 'referenced PCM file exists'
                if ($exists) {
                    $actual = Get-Sha256 $pcmPath
                    Add-Check "pre-mix-sha:${relative}:$($route.route)" ($actual -ceq $route.pre_mix_pcm_sha256) $actual
                    $verifiedPcmFiles++
                }
            }
            $engineFile = if ($route.PSObject.Properties.Name -contains 'engine_before_pcm_file') {
                [string]$route.engine_before_pcm_file
            } else {
                ''
            }
            if (-not [string]::IsNullOrWhiteSpace($engineFile)) {
                $pcmPath = Join-Path (Split-Path -Parent $path) $engineFile
                $exists = Test-Path -LiteralPath $pcmPath -PathType Leaf
                Add-Check "engine-file:${relative}:$($route.route)" $exists 'referenced PCM file exists'
                if ($exists) {
                    $actual = Get-Sha256 $pcmPath
                    Add-Check "engine-sha:${relative}:$($route.route)" ($actual -ceq $route.engine_before_pcm_sha256) $actual
                    $verifiedPcmFiles++
                }
            }
        }
    } elseif (($payload.PSObject.Properties.Name -contains 'engine_before_pcm_file') -and
        -not [string]::IsNullOrWhiteSpace([string]$payload.engine_before_pcm_file)) {
        $pcmPath = Join-Path (Split-Path -Parent $path) $payload.engine_before_pcm_file
        $exists = Test-Path -LiteralPath $pcmPath -PathType Leaf
        Add-Check "decode-engine-file:$relative" $exists 'referenced PCM file exists'
        if ($exists) {
            $actual = Get-Sha256 $pcmPath
            Add-Check "decode-engine-sha:$relative" ($actual -ceq $payload.engine_before_pcm_sha256) $actual
            $verifiedPcmFiles++
        }
    }
}

$decodeUnmuted = Get-Content -LiteralPath (Join-Path $root 'scenarios/D10A-07-unmuted/result.json') -Raw -Encoding utf8 | ConvertFrom-Json
$decodeMuted = Get-Content -LiteralPath (Join-Path $root 'scenarios/D10A-07-muted/result.json') -Raw -Encoding utf8 | ConvertFrom-Json
Add-Check 'D10A-07:source-pcm-equal' ($decodeUnmuted.source_pcm_sha256 -ceq $decodeMuted.source_pcm_sha256) $decodeUnmuted.source_pcm_sha256
Add-Check 'D10A-07:engine-pcm-equal' ($decodeUnmuted.engine_before_pcm_sha256 -ceq $decodeMuted.engine_before_pcm_sha256) $decodeUnmuted.engine_before_pcm_sha256
Add-Check 'D10A-07:no-microphone' ($decodeUnmuted.microphone_stream_started_count -eq 0 -and $decodeMuted.microphone_stream_started_count -eq 0) 'both runs report zero microphone streams'

$switchFact = Get-Content -LiteralPath (Join-Path $root 'scenarios/D10A-05/result.json') -Raw -Encoding utf8 | ConvertFrom-Json
Add-Check 'D10A-05:source-result-sha256' ((Get-Sha256 (Join-Path $root $switchFact.source_result)) -ceq $switchFact.source_result_sha256) $switchFact.source_result_sha256
Add-Check 'D10A-05:endpoint-snapshot-sha256' ((Get-Sha256 (Join-Path $root $switchFact.endpoint_snapshot)) -ceq $switchFact.endpoint_snapshot_sha256) $switchFact.endpoint_snapshot_sha256

$muteFactPath = Join-Path $root 'driver-mute-fact.json'
$muteFact = Get-Content -LiteralPath $muteFactPath -Raw -Encoding utf8 | ConvertFrom-Json
$sidecarLine = (Get-Content -LiteralPath (Join-Path $root 'driver-mute-fact.sha256.txt') -Encoding utf8 | Select-Object -First 1)
$sidecarHash = ($sidecarLine -split '\s+')[0]
$actualMuteFactHash = Get-Sha256 $muteFactPath
Add-Check 'D10A-06:fact-sidecar' ($actualMuteFactHash -ceq $sidecarHash) $actualMuteFactHash
Add-Check 'D10A-06:frozen-branch' ($muteFact.driver_mute_behavior -in @('audible_frames', 'silent_frames', 'no_callback')) $muteFact.driver_mute_behavior
Add-Check 'D10A-06:scenario-result-sha256' ((Get-Sha256 (Join-Path $root 'scenarios/D10A-06/result.json')) -ceq $muteFact.scenario_result_sha256) $muteFact.scenario_result_sha256

$executables = @(Get-ChildItem -LiteralPath $root -File -Recurse | Where-Object { $_.Extension -in @('.exe', '.dll') })
Add-Check 'isolation:no-binary-in-evidence' ($executables.Count -eq 0) "$($executables.Count) executable files"

$logOutcomes = @()
foreach ($log in Get-ChildItem -LiteralPath (Join-Path $root 'logs') -File | Sort-Object Name) {
    $exitLine = Select-String -LiteralPath $log.FullName -Pattern '^EXIT_CODE=' | Select-Object -First 1
    $commandLine = Select-String -LiteralPath $log.FullName -Pattern '^COMMAND=' | Select-Object -First 1
    $logOutcomes += [pscustomobject]@{
        log = $log.Name
        command = if ($null -eq $commandLine) { $null } else { $commandLine.Line.Substring(8) }
        exit_code = if ($null -eq $exitLine) { $null } else { [int]$exitLine.Line.Substring(10) }
        sha256 = Get-Sha256 $log.FullName
    }
}
Add-Check 'logs:exit-code-present' ((@($logOutcomes | Where-Object { $null -eq $_.exit_code })).Count -eq 0) "$($logOutcomes.Count) logs checked"

$payload = [pscustomobject]@{
    schema_version = 1
    validated_at_utc = [DateTime]::UtcNow.ToString('o')
    evidence_root = $root
    status = if ($failures.Count -eq 0) { 'PASS' } else { 'FAIL' }
    check_count = $checks.Count
    passed_check_count = @($checks | Where-Object passed).Count
    failed_check_count = $failures.Count
    verified_pcm_file_count = $verifiedPcmFiles
    log_count = $logOutcomes.Count
    nonzero_exit_log_count = @($logOutcomes | Where-Object { $_.exit_code -ne 0 }).Count
    checks = $checks
    failures = $failures
    command_outcomes = $logOutcomes
}
$utf8NoBom = [System.Text.UTF8Encoding]::new($false)
[System.IO.File]::WriteAllText($fullOutput, ($payload | ConvertTo-Json -Depth 10), $utf8NoBom)
$payload | ConvertTo-Json -Depth 5
if ($failures.Count -ne 0) { exit 1 }

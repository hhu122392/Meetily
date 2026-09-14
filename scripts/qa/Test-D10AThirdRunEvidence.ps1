[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string]$EvidenceRoot,

    [string]$OutputPath
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$root = [IO.Path]::GetFullPath($EvidenceRoot)
if ([string]::IsNullOrWhiteSpace($OutputPath)) {
    $OutputPath = Join-Path $root 'evidence-validation.json'
}
$checks = [Collections.Generic.List[object]]::new()
$failures = [Collections.Generic.List[string]]::new()

function Add-Check {
    param([string]$Name, [bool]$Passed, [string]$Detail)
    $checks.Add([pscustomobject]@{ name = $Name; passed = $Passed; detail = $Detail })
    if (-not $Passed) { $failures.Add("$($Name): $Detail") }
}

function Read-Json([string]$RelativePath) {
    Get-Content -LiteralPath (Join-Path $root $RelativePath) -Raw -Encoding utf8 | ConvertFrom-Json
}

$required = @(
    'machine-context.json',
    'fixture-registration.json',
    'fixed-passphrase.wav',
    'truth.txt',
    'windows-audio-endpoints-before.json',
    'windows-audio-endpoints-ready.json',
    'windows-audio-endpoints-after-tests-before-restore.json',
    'windows-audio-endpoints-after-restore.json',
    'pnp-audio-endpoints-before.json',
    'pnp-audio-endpoints-ready.json',
    'pnp-audio-endpoints-after-restore.json',
    'microphone-restore-system-interface-attempt.json',
    'driver-mute-confirmation.json',
    'scenario-assessment.json'
)
foreach ($relative in $required) {
    Add-Check "required:$relative" (Test-Path -LiteralPath (Join-Path $root $relative) -PathType Leaf) 'required file exists'
}

$fixture = Read-Json 'fixture-registration.json'
$audioHash = (Get-FileHash -LiteralPath (Join-Path $root 'fixed-passphrase.wav') -Algorithm SHA256).Hash
$truthHash = (Get-FileHash -LiteralPath (Join-Path $root 'truth.txt') -Algorithm SHA256).Hash
Add-Check 'fixture:audio-sha256' ($audioHash -ceq $fixture.audio_sha256 -and $audioHash -ceq 'BD8E75FA557B6C64A7E5E327E8B7CAB2D80EA41841C7905AF6991BB8A25A901E') $audioHash
Add-Check 'fixture:truth-sha256' ($truthHash -ceq $fixture.truth_sha256 -and $truthHash -ceq '1AD21C289893DF0AC5DD98B25A23AAB0CC48E7CD3A7BBF2EE924F1676E93FAFA') $truthHash

$before = Read-Json 'windows-audio-endpoints-before.json'
$ready = Read-Json 'windows-audio-endpoints-ready.json'
$afterTests = Read-Json 'windows-audio-endpoints-after-tests-before-restore.json'
$afterRestore = Read-Json 'windows-audio-endpoints-after-restore.json'
$beforeActiveCapture = @($before.endpoints | Where-Object { $_.flow -eq 'capture' -and $_.state -eq 'active' })
$readyActiveCapture = @($ready.endpoints | Where-Object { $_.flow -eq 'capture' -and $_.state -eq 'active' })
$readyFreeClip = @($ready.endpoints | Where-Object { $_.flow -eq 'render' -and $_.state -eq 'active' -and $_.friendly_name -match 'HUAWEI FreeClip' })
$afterTestsActiveCapture = @($afterTests.endpoints | Where-Object { $_.flow -eq 'capture' -and $_.state -eq 'active' })
$restoredTargets = @($afterRestore.endpoints | Where-Object {
    $_.flow -eq 'capture' -and $_.state -eq 'active' -and $_.friendly_name -match '英特尔|网易虚拟|HUAWEI FreeClip'
})
$activeStereoMix = @($afterRestore.endpoints | Where-Object {
    $_.flow -eq 'capture' -and $_.state -eq 'active' -and $_.friendly_name -match '立体声混音'
})
Add-Check 'precondition:before-freeclip-microphone-active' ($beforeActiveCapture.Count -eq 1 -and $beforeActiveCapture[0].friendly_name -match 'HUAWEI FreeClip') "$($beforeActiveCapture.Count) active capture endpoints"
Add-Check 'precondition:ready-no-active-microphone' ($readyActiveCapture.Count -eq 0) "$($readyActiveCapture.Count) active capture endpoints"
Add-Check 'precondition:ready-freeclip-render-active' ($readyFreeClip.Count -eq 1) "$($readyFreeClip.Count) active FreeClip render endpoints"
Add-Check 'post-test:no-active-microphone' ($afterTestsActiveCapture.Count -eq 0) "$($afterTestsActiveCapture.Count) active capture endpoints"
Add-Check 'restore:three-targets-active' ($restoredTargets.Count -eq 3) "$($restoredTargets.Count) restored targets"
Add-Check 'restore:stereo-mix-not-enabled' ($activeStereoMix.Count -eq 0) "$($activeStereoMix.Count) active stereo-mix endpoints"

$expectedEndpoints = @{
    'D10A-02' = '{0.0.0.00000000}.{dd978360-d5dc-4dbc-8917-7843a07fe531}'
    'D10A-04' = '{0.0.0.00000000}.{d3ec8873-a0d1-4b53-9dd4-ae64b2a221a4}'
    'D10A-06' = '{0.0.0.00000000}.{d3ec8873-a0d1-4b53-9dd4-ae64b2a221a4}'
}
foreach ($scenario in @('D10A-02', 'D10A-04', 'D10A-06')) {
    $resultPath = "scenarios/$scenario/result.json"
    $recognitionPath = "scenarios/$scenario/windows-sapi-recognition.json"
    Add-Check "$($scenario):result-exists" (Test-Path -LiteralPath (Join-Path $root $resultPath) -PathType Leaf) $resultPath
    Add-Check "$($scenario):recognition-exists" (Test-Path -LiteralPath (Join-Path $root $recognitionPath) -PathType Leaf) $recognitionPath
    $result = Read-Json $resultPath
    $route = $result.routes | Where-Object route -eq 'system'
    $recognition = Read-Json $recognitionPath
    Add-Check "$($scenario):microphone-not-started" ($result.microphone_stream_started_count -eq 0) "count=$($result.microphone_stream_started_count)"
    Add-Check "$($scenario):endpoint" ($route.endpoint_id -ceq $expectedEndpoints[$scenario]) $route.endpoint_id
    Add-Check "$($scenario):nonzero-system-frames" ($route.stream_started -and $route.callback_frames -gt 0 -and $route.rms -gt 0 -and $route.peak -gt 0) "callbacks=$($route.callback_count), frames=$($route.callback_frames), rms=$($route.rms), peak=$($route.peak)"
    foreach ($field in @(
        @{ Name = 'pre-mix'; File = $route.pre_mix_pcm_file; Hash = $route.pre_mix_pcm_sha256 },
        @{ Name = 'engine'; File = $route.engine_before_pcm_file; Hash = $route.engine_before_pcm_sha256 }
    )) {
        $pcmPath = Join-Path (Join-Path $root "scenarios/$scenario") $field.File
        $exists = Test-Path -LiteralPath $pcmPath -PathType Leaf
        Add-Check "$($scenario):$($field.Name)-exists" $exists $field.File
        if ($exists) {
            $actualHash = (Get-FileHash -LiteralPath $pcmPath -Algorithm SHA256).Hash
            Add-Check "$($scenario):$($field.Name)-sha256" ($actualHash -ceq $field.Hash) $actualHash
        }
    }
    Add-Check "$($scenario):recognition-source-sha256" ($recognition.source_pcm_sha256 -ceq $route.engine_before_pcm_sha256) $recognition.source_pcm_sha256
    Add-Check "$($scenario):strict-mismatch-recorded" (-not $recognition.strict_normalized_match) $recognition.normalized_recognized_text
}

$mute = Read-Json 'driver-mute-confirmation.json'
Add-Check 'D10A-06:frozen-branch-match' ($mute.branch_matches_prior -and $mute.current_driver_mute_behavior -eq 'audible_frames') $mute.current_driver_mute_behavior
Add-Check 'D10A-06:branch-confirmation' ($mute.branch_confirmation_pass) "first_callback_delay_ns=$($mute.first_callback_delay_ns)"
Add-Check 'D10A-06:mute-restored' ($mute.render_mute_target -and $mute.render_mute_observed_target -and $mute.render_mute_before -eq $mute.render_mute_after_restore) "before=$($mute.render_mute_before), after=$($mute.render_mute_after_restore)"

$restoreAttempt = Read-Json 'microphone-restore-system-interface-attempt.json'
Add-Check 'restore:system-interface-failure-recorded' ($restoreAttempt.enabled_count -eq 0 -and $restoreAttempt.exit_code -eq 5) "enabled=$($restoreAttempt.enabled_count), exit=$($restoreAttempt.exit_code)"

$expectedLogExit = @{
    '01-enumerate-before.log' = 0
    '02-enumerate-ready.log' = 0
    '03-D10A-02-capture.log' = 0
    '04-D10A-02-recognition.log' = 0
    '05-D10A-04-capture.log' = 0
    '06-D10A-04-recognition.log' = 0
    '07-D10A-06-capture.log' = 0
    '08-D10A-06-recognition.log' = 0
    '09-enumerate-after-tests-before-restore.log' = 0
    '10-restore-microphones-system-interface.log' = 5
    '11-enumerate-after-restore.log' = 0
}
foreach ($entry in $expectedLogExit.GetEnumerator()) {
    $path = Join-Path $root "logs/$($entry.Key)"
    $line = Select-String -LiteralPath $path -Pattern '^EXIT_CODE=' | Select-Object -Last 1
    $actual = if ($null -eq $line) { $null } else { [int]$line.Line.Substring(10) }
    Add-Check "log-exit:$($entry.Key)" ($actual -eq $entry.Value) "expected=$($entry.Value), actual=$actual"
}

$binaries = @(Get-ChildItem -LiteralPath $root -File -Recurse | Where-Object Extension -in @('.exe', '.dll'))
Add-Check 'isolation:no-binaries' ($binaries.Count -eq 0) "$($binaries.Count) binaries"

$payload = [pscustomobject]@{
    schema_version = 1
    validated_at_utc = [DateTime]::UtcNow.ToString('o')
    evidence_root = $root
    status = if ($failures.Count -eq 0) { 'PASS' } else { 'FAIL' }
    meaning = 'PASS validates evidence integrity and recorded outcomes only; scenario acceptance remains BLOCKED because strict recognition and other conditions are unmet'
    check_count = $checks.Count
    passed_check_count = @($checks | Where-Object passed).Count
    failed_check_count = $failures.Count
    checks = $checks
    failures = $failures
}
[IO.File]::WriteAllText([IO.Path]::GetFullPath($OutputPath), ($payload | ConvertTo-Json -Depth 8), [Text.UTF8Encoding]::new($false))
$payload | ConvertTo-Json -Depth 8
if ($failures.Count -ne 0) { exit 1 }

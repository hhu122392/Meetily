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
$output = [IO.Path]::GetFullPath($OutputPath)
$checks = [Collections.Generic.List[object]]::new()
$failures = [Collections.Generic.List[string]]::new()

function Add-Check {
    param([string]$Name, [bool]$Passed, [string]$Detail)
    $checks.Add([pscustomobject]@{ name = $Name; passed = $Passed; detail = $Detail })
    if (-not $Passed) { $failures.Add("$($Name): $Detail") }
}

$required = @(
    'machine-context.json',
    'fixture-registration.json',
    'fixed-passphrase.wav',
    'truth.txt',
    'windows-audio-endpoints-before.json',
    'pnp-audio-endpoints-before.json',
    'pnp-microphones-during-disable-attempt.json',
    'pnp-microphones-after-restore.json',
    'microphone-disable-attempt.json',
    'logs/01-enumerate-before.log',
    'logs/02-disable-microphones-and-D10A-02.log'
)
foreach ($relative in $required) {
    Add-Check "required:$relative" (Test-Path -LiteralPath (Join-Path $root $relative) -PathType Leaf) 'required file exists'
}

$fixture = Get-Content -LiteralPath (Join-Path $root 'fixture-registration.json') -Raw -Encoding utf8 | ConvertFrom-Json
$audioHash = (Get-FileHash -LiteralPath (Join-Path $root 'fixed-passphrase.wav') -Algorithm SHA256).Hash
$truthHash = (Get-FileHash -LiteralPath (Join-Path $root 'truth.txt') -Algorithm SHA256).Hash
Add-Check 'fixture:audio-sha256' ($audioHash -ceq $fixture.audio_sha256 -and $audioHash -ceq 'BD8E75FA557B6C64A7E5E327E8B7CAB2D80EA41841C7905AF6991BB8A25A901E') $audioHash
Add-Check 'fixture:truth-sha256' ($truthHash -ceq $fixture.truth_sha256 -and $truthHash -ceq '1AD21C289893DF0AC5DD98B25A23AAB0CC48E7CD3A7BBF2EE924F1676E93FAFA') $truthHash
Add-Check 'fixture:format' ($fixture.duration_seconds -eq 24 -and $fixture.sample_rate_hz -eq 16000 -and $fixture.channels -eq 1 -and $fixture.sample_format -eq 'pcm_s16le') '24 seconds, 16000Hz, mono PCM16'

$endpoints = Get-Content -LiteralPath (Join-Path $root 'windows-audio-endpoints-before.json') -Raw -Encoding utf8 | ConvertFrom-Json
$activeBluetooth = @($endpoints.endpoints | Where-Object {
    $_.state -eq 'active' -and ($_.friendly_name -match 'HUAWEI|FreeClip|Bluetooth|蓝牙')
})
Add-Check 'hardware:no-active-bluetooth-endpoint' ($activeBluetooth.Count -eq 0) "$($activeBluetooth.Count) active Bluetooth endpoints"

$attempt = Get-Content -LiteralPath (Join-Path $root 'microphone-disable-attempt.json') -Raw -Encoding utf8 | ConvertFrom-Json
Add-Check 'microphone:non-admin' (-not $attempt.is_administrator) "is_administrator=$($attempt.is_administrator)"
Add-Check 'microphone:two-targets' (@($attempt.targets).Count -eq 2) "$(@($attempt.targets).Count) targets"
Add-Check 'microphone:disable-failed-truthfully' (
    @($attempt.attempts | Where-Object disable_succeeded).Count -eq 0 -and
    $attempt.disabled_endpoint_count -eq 0
) 'zero endpoints disabled'
Add-Check 'D10A-02:capture-skipped' (-not $attempt.capture_executed -and $attempt.final_exit_code -eq 5) "capture_executed=$($attempt.capture_executed), exit=$($attempt.final_exit_code)"

$after = @(Get-Content -LiteralPath (Join-Path $root 'pnp-microphones-after-restore.json') -Raw -Encoding utf8 | ConvertFrom-Json)
Add-Check 'microphone:post-attempt-state-preserved' (
    $after.Count -eq 2 -and @($after | Where-Object Status -ne 'OK').Count -eq 0
) "$($after.Count) endpoints; $(@($after | Where-Object Status -eq 'OK').Count) OK"

$expectedExitCodes = @{
    '01-enumerate-before.log' = 0
    '02-disable-microphones-and-D10A-02.log' = 5
}
foreach ($entry in $expectedExitCodes.GetEnumerator()) {
    $line = Select-String -LiteralPath (Join-Path $root "logs/$($entry.Key)") -Pattern '^EXIT_CODE=' | Select-Object -Last 1
    $actual = if ($null -eq $line) { $null } else { [int]($line.Line.Substring(10)) }
    Add-Check "log-exit:$($entry.Key)" ($actual -eq $entry.Value) "expected=$($entry.Value), actual=$actual"
}

$binaries = @(Get-ChildItem -LiteralPath $root -File -Recurse | Where-Object Extension -in @('.exe', '.dll'))
Add-Check 'isolation:no-binaries' ($binaries.Count -eq 0) "$($binaries.Count) binaries"

$payload = [pscustomobject]@{
    schema_version = 1
    validated_at_utc = [DateTime]::UtcNow.ToString('o')
    evidence_root = $root
    status = if ($failures.Count -eq 0) { 'PASS' } else { 'FAIL' }
    meaning = 'PASS validates evidence integrity only; D10A-02 and D10A-04 remain BLOCKED'
    check_count = $checks.Count
    passed_check_count = @($checks | Where-Object passed).Count
    failed_check_count = $failures.Count
    checks = $checks
    failures = $failures
}
[IO.File]::WriteAllText($output, ($payload | ConvertTo-Json -Depth 8), [Text.UTF8Encoding]::new($false))
$payload | ConvertTo-Json -Depth 8
if ($failures.Count -ne 0) { exit 1 }

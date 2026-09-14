param(
    [Parameter(Mandatory = $true)][string]$EvidenceDirectory
)

$ErrorActionPreference = 'Stop'
$env:CDP_PORT = '9233'
$workspace = 'D:\桌面\meetlily'
$wavPath = 'D:\桌面\meetlily\target\release\docs\方案\证据\RUN-20260825-124500-RC26-LIVE-ACCURACY\golden-phase-1.wav'
$node = (Get-Command node -ErrorAction Stop).Source
$started = $false
$stopped = $false
[System.IO.Directory]::CreateDirectory($EvidenceDirectory) | Out-Null

function Invoke-NodeEvidence {
    param(
        [Parameter(Mandatory = $true)][string[]]$Arguments,
        [Parameter(Mandatory = $true)][string]$OutputPath
    )
    $result = & $node @Arguments 2>&1
    $exitCode = $LASTEXITCODE
    $text = ($result | Out-String).Trim()
    [System.IO.File]::WriteAllText($OutputPath, $text + [Environment]::NewLine, [System.Text.UTF8Encoding]::new($false))
    if ($exitCode -ne 0) {
        throw "Node command failed with exit code $exitCode. See $OutputPath"
    }
    $text
}

try {
    $null = Invoke-NodeEvidence -Arguments @(
        'scripts/qa/cdp-navigate.mjs',
        'http://tauri.localhost/'
    ) -OutputPath (Join-Path $EvidenceDirectory '01-navigate-home.json')
    Start-Sleep -Milliseconds 750

    $startPath = Join-Path $EvidenceDirectory '02-context-recording-start.json'
    $startText = & $node 'scripts/qa/cdp-stage-e-context-recording-start.mjs' $startPath 2>&1 | Out-String
    if ($LASTEXITCODE -ne 0) {
        throw "Context recording start failed. See $startPath"
    }
    $start = Get-Content -LiteralPath $startPath -Raw | ConvertFrom-Json
    if (-not $start.verdict.backendStarted) {
        throw 'The context-aware recording did not become active.'
    }
    $started = $true

    $monitorOutput = Join-Path $EvidenceDirectory '03-live-monitor.json'
    $monitor = Start-Process -FilePath $node -ArgumentList @(
        'scripts/qa/cdp-live-transcription-monitor.mjs',
        $monitorOutput,
        '75000',
        '200'
    ) -WorkingDirectory $workspace -WindowStyle Hidden -RedirectStandardOutput (Join-Path $EvidenceDirectory '03-live-monitor.stdout.txt') -RedirectStandardError (Join-Path $EvidenceDirectory '03-live-monitor.stderr.txt') -PassThru

    Start-Sleep -Seconds 1
    $player = [System.Media.SoundPlayer]::new($wavPath)
    $playbackStarted = Get-Date
    $player.PlaySync()
    $playbackFinished = Get-Date
    Start-Sleep -Seconds 5

    $stopText = Invoke-NodeEvidence -Arguments @(
        'scripts/qa/cdp-stop-recording-ui.mjs'
    ) -OutputPath (Join-Path $EvidenceDirectory '04-recording-stop.json')
    $stop = $stopText | ConvertFrom-Json
    if (-not $stop.verdict.backendStopped) {
        throw 'The recording backend did not return to idle.'
    }
    $stopped = $true

    if (-not $monitor.WaitForExit(30000)) {
        throw 'The live transcription monitor did not finish in time.'
    }
    if ($monitor.ExitCode -ne 0) {
        throw "The live transcription monitor failed with exit code $($monitor.ExitCode)."
    }

    $clock = [ordered]@{
        capturedAt = [datetimeoffset]::Now.ToUniversalTime().ToString('o')
        wavPath = $wavPath
        wavSha256 = (Get-FileHash -LiteralPath $wavPath -Algorithm SHA256).Hash
        playbackStartedAt = $playbackStarted.ToUniversalTime().ToString('o')
        playbackFinishedAt = $playbackFinished.ToUniversalTime().ToString('o')
        playbackDurationSeconds = [math]::Round(($playbackFinished - $playbackStarted).TotalSeconds, 3)
        meetingFolder = $start.folder
        meetingName = $start.meetingName
        stoppedAt = $stop.clickedAt
    }
    $json = $clock | ConvertTo-Json -Depth 8
    [System.IO.File]::WriteAllText((Join-Path $EvidenceDirectory '05-clock.json'), $json + [Environment]::NewLine, [System.Text.UTF8Encoding]::new($false))
    $json
}
finally {
    if ($started -and -not $stopped) {
        try {
            $emergency = & $node 'scripts/qa/cdp-stop-recording-ui.mjs' 2>&1 | Out-String
            [System.IO.File]::WriteAllText((Join-Path $EvidenceDirectory '98-emergency-stop.json'), $emergency, [System.Text.UTF8Encoding]::new($false))
        }
        catch {
            [System.IO.File]::WriteAllText((Join-Path $EvidenceDirectory '98-emergency-stop-error.txt'), $_.Exception.Message, [System.Text.UTF8Encoding]::new($false))
        }
    }
}

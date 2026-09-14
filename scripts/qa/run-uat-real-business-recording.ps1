param(
    [Parameter(Mandatory = $true)][string]$EvidenceDirectory,
    [Parameter(Mandatory = $true)][string]$SourceAudioPath,
    [int]$StartSeconds = 480,
    [int]$DurationSeconds = 720,
    [int]$ExpectedTemplateVersion = 4
)

$ErrorActionPreference = 'Stop'
$env:CDP_PORT = '9233'
$workspace = 'D:\桌面\meetlily'
$node = (Get-Command node -ErrorAction Stop).Source
$started = $false
$stopped = $false
$microphoneWasMuted = $null
$player = $null
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
    if (-not (Test-Path -LiteralPath $SourceAudioPath -PathType Leaf)) {
        throw "Source audio was not found: $SourceAudioPath"
    }
    $muteBefore = & "$workspace\scripts\qa\Set-DefaultCaptureMute.ps1" -Action get | ConvertFrom-Json
    $microphoneWasMuted = [bool]$muteBefore.After
    $mute = & "$workspace\scripts\qa\Set-DefaultCaptureMute.ps1" -Action mute
    [System.IO.File]::WriteAllText((Join-Path $EvidenceDirectory '01-microphone-mute.json'), $mute + [Environment]::NewLine, [System.Text.UTF8Encoding]::new($false))

    $null = Invoke-NodeEvidence -Arguments @(
        'scripts/qa/cdp-navigate.mjs',
        'http://tauri.localhost/'
    ) -OutputPath (Join-Path $EvidenceDirectory '02-navigate-home.json')
    Start-Sleep -Milliseconds 750

    $startPath = Join-Path $EvidenceDirectory '03-context-recording-start.json'
    $startText = & $node 'scripts/qa/cdp-uat-real-business-start.mjs' $startPath 'license_station_weekly' 'UAT真实业务会议-牌照站周会' $ExpectedTemplateVersion 2>&1 | Out-String
    if ($LASTEXITCODE -ne 0) {
        throw "Context recording start failed. See $startPath"
    }
    $start = Get-Content -LiteralPath $startPath -Raw -Encoding UTF8 | ConvertFrom-Json
    if (-not $start.verdict.backendStarted) {
        throw 'The context-aware recording did not become active.'
    }
    $started = $true

    $monitorOutput = Join-Path $EvidenceDirectory '04-live-monitor.json'
    $monitorDurationMs = ($DurationSeconds + 45) * 1000
    $monitor = Start-Process -FilePath $node -ArgumentList @(
        'scripts/qa/cdp-live-transcription-monitor.mjs',
        $monitorOutput,
        $monitorDurationMs.ToString(),
        '500'
    ) -WorkingDirectory $workspace -WindowStyle Hidden -RedirectStandardOutput (Join-Path $EvidenceDirectory '04-live-monitor.stdout.txt') -RedirectStandardError (Join-Path $EvidenceDirectory '04-live-monitor.stderr.txt') -PassThru

    Start-Sleep -Seconds 1
    if ([System.IO.Path]::GetExtension($SourceAudioPath) -ne '.wav') {
        throw 'The deterministic UAT playback path requires a WAV source.'
    }
    $player = [System.Media.SoundPlayer]::new($SourceAudioPath)
    $player.Load()
    $playbackStarted = Get-Date
    $player.PlaySync()
    $playbackFinished = Get-Date
    $playbackElapsedSeconds = ($playbackFinished - $playbackStarted).TotalSeconds
    if ($playbackElapsedSeconds -lt ($DurationSeconds - 1)) {
        throw "WAV playback ended before the required duration: $playbackElapsedSeconds seconds"
    }
    $playbackFinishedPosition = $StartSeconds + $playbackElapsedSeconds
    Start-Sleep -Seconds 8

    $stopText = Invoke-NodeEvidence -Arguments @(
        'scripts/qa/cdp-stop-recording-ui.mjs'
    ) -OutputPath (Join-Path $EvidenceDirectory '05-recording-stop.json')
    $stop = $stopText | ConvertFrom-Json
    if (-not $stop.verdict.backendStopped) {
        throw 'The recording backend did not return to idle.'
    }
    $stopped = $true

    if (-not $monitor.WaitForExit(60000)) {
        throw 'The live transcription monitor did not finish in time.'
    }
    if ($monitor.ExitCode -ne 0) {
        throw "The live transcription monitor failed with exit code $($monitor.ExitCode)."
    }

    $clock = [ordered]@{
        capturedAt = [datetimeoffset]::Now.ToUniversalTime().ToString('o')
        sourceAudioPath = $SourceAudioPath
        sourceAudioSha256 = (Get-FileHash -LiteralPath $SourceAudioPath -Algorithm SHA256).Hash
        sourceStartSeconds = $StartSeconds
        requestedDurationSeconds = $DurationSeconds
        playbackStartedAt = $playbackStarted.ToUniversalTime().ToString('o')
        playbackFinishedAt = $playbackFinished.ToUniversalTime().ToString('o')
        playbackElapsedSeconds = [math]::Round($playbackElapsedSeconds, 3)
        playbackFinishedPositionSeconds = [math]::Round($playbackFinishedPosition, 3)
        meetingFolder = $start.folder
        meetingName = $start.meetingName
        stoppedAt = $stop.clickedAt
        microphoneMutedDuringReplay = $true
        player = 'System.Media.SoundPlayer'
        sourceIsPreviouslyCapturedRealBusinessMeeting = $true
    }
    $json = $clock | ConvertTo-Json -Depth 8
    [System.IO.File]::WriteAllText((Join-Path $EvidenceDirectory '06-clock.json'), $json + [Environment]::NewLine, [System.Text.UTF8Encoding]::new($false))
    $json
}
finally {
    if ($null -ne $player) {
        try { $player.Stop() } catch { }
        try { $player.Dispose() } catch { }
    }
    if ($started -and -not $stopped) {
        try {
            $emergency = & $node 'scripts/qa/cdp-stop-recording-ui.mjs' 2>&1 | Out-String
            [System.IO.File]::WriteAllText((Join-Path $EvidenceDirectory '98-emergency-stop.json'), $emergency, [System.Text.UTF8Encoding]::new($false))
        }
        catch {
            [System.IO.File]::WriteAllText((Join-Path $EvidenceDirectory '98-emergency-stop-error.txt'), $_.Exception.Message, [System.Text.UTF8Encoding]::new($false))
        }
    }
    if ($null -ne $microphoneWasMuted -and -not $microphoneWasMuted) {
        try {
            $restore = & "$workspace\scripts\qa\Set-DefaultCaptureMute.ps1" -Action unmute
            [System.IO.File]::WriteAllText((Join-Path $EvidenceDirectory '99-microphone-restore.json'), $restore + [Environment]::NewLine, [System.Text.UTF8Encoding]::new($false))
        }
        catch { }
    }
}

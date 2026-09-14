param(
    [Parameter(Mandatory = $true)][string]$EvidenceDirectory,
    [Parameter(Mandatory = $true)][string]$MeetingDirectory,
    [string]$AppDataDirectory = 'C:\Users\liuxin\AppData\Roaming\com.meetily.ai'
)

$ErrorActionPreference = 'Stop'

$running = @(Get-Process -Name meetily -ErrorAction SilentlyContinue)
if ($running.Count -ne 0) {
    throw 'Meetily must be fully stopped before the SQLite backup is created.'
}

$backupDirectory = Join-Path $EvidenceDirectory 'pre-retranscription-backup'
if (Test-Path -LiteralPath $backupDirectory) {
    throw "Backup directory already exists: $backupDirectory"
}

[System.IO.Directory]::CreateDirectory($backupDirectory) | Out-Null
$meetingBackupDirectory = Join-Path $backupDirectory 'meeting'
[System.IO.Directory]::CreateDirectory($meetingBackupDirectory) | Out-Null

$meetingFiles = @('metadata.json', 'transcripts.json')
foreach ($name in $meetingFiles) {
    $source = Join-Path $MeetingDirectory $name
    if (-not (Test-Path -LiteralPath $source -PathType Leaf)) {
        throw "Required meeting file is missing: $source"
    }
    Copy-Item -LiteralPath $source -Destination (Join-Path $meetingBackupDirectory $name)
}

$snapshotSource = Join-Path $MeetingDirectory 'summary-template-snapshots'
if (Test-Path -LiteralPath $snapshotSource -PathType Container) {
    Copy-Item -LiteralPath $snapshotSource -Destination $meetingBackupDirectory -Recurse
}

$databaseFiles = @(
    'meeting_minutes.sqlite',
    'meeting_minutes.sqlite-wal',
    'meeting_minutes.sqlite-shm'
)
$databaseBackupDirectory = Join-Path $backupDirectory 'database'
[System.IO.Directory]::CreateDirectory($databaseBackupDirectory) | Out-Null
foreach ($name in $databaseFiles) {
    $source = Join-Path $AppDataDirectory $name
    if (Test-Path -LiteralPath $source -PathType Leaf) {
        Copy-Item -LiteralPath $source -Destination (Join-Path $databaseBackupDirectory $name)
    }
}

function Get-FileEvidence {
    param([Parameter(Mandatory = $true)][string]$Path)
    $item = Get-Item -LiteralPath $Path
    [ordered]@{
        path = $item.FullName
        length = $item.Length
        lastWriteTime = $item.LastWriteTime.ToString('o')
        sha256 = (Get-FileHash -LiteralPath $item.FullName -Algorithm SHA256).Hash
    }
}

$audioPath = Join-Path $MeetingDirectory 'audio.mp4'
if (-not (Test-Path -LiteralPath $audioPath -PathType Leaf)) {
    throw "Required meeting audio is missing: $audioPath"
}

$sourceEvidence = [ordered]@{
    audio = Get-FileEvidence -Path $audioPath
    metadata = Get-FileEvidence -Path (Join-Path $MeetingDirectory 'metadata.json')
    transcripts = Get-FileEvidence -Path (Join-Path $MeetingDirectory 'transcripts.json')
    snapshots = @(Get-ChildItem -LiteralPath $snapshotSource -File -ErrorAction SilentlyContinue | ForEach-Object {
        Get-FileEvidence -Path $_.FullName
    })
    database = @(Get-ChildItem -LiteralPath $databaseBackupDirectory -File | ForEach-Object {
        Get-FileEvidence -Path $_.FullName
    })
}

$evidence = [ordered]@{
    capturedAt = [datetimeoffset]::Now.ToUniversalTime().ToString('o')
    meetilyProcessCount = $running.Count
    meetingDirectory = $MeetingDirectory
    backupDirectory = $backupDirectory
    source = $sourceEvidence
    checks = [ordered]@{
        appStopped = $running.Count -eq 0
        metadataBackedUp = Test-Path -LiteralPath (Join-Path $meetingBackupDirectory 'metadata.json')
        transcriptsBackedUp = Test-Path -LiteralPath (Join-Path $meetingBackupDirectory 'transcripts.json')
        databaseBackedUp = Test-Path -LiteralPath (Join-Path $databaseBackupDirectory 'meeting_minutes.sqlite')
        audioPresentAndHashed = $sourceEvidence.audio.sha256.Length -eq 64
    }
}

$json = $evidence | ConvertTo-Json -Depth 12
$outputPath = Join-Path $EvidenceDirectory '07-pre-retranscription-backup.json'
[System.IO.File]::WriteAllText($outputPath, $json + [Environment]::NewLine, [System.Text.UTF8Encoding]::new($false))
$json

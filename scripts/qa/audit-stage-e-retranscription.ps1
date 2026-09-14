param(
    [Parameter(Mandatory = $true)][string]$EvidenceDirectory,
    [Parameter(Mandatory = $true)][string]$MeetingDirectory,
    [string]$OutputName = '13-post-retranscription-integrity.json'
)

$ErrorActionPreference = 'Stop'

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

function Get-GenerationIds {
    param([Parameter(Mandatory = $true)]$History)
    @($History.result | ForEach-Object { $_.generationId } | Sort-Object)
}

$baselinePath = Join-Path $EvidenceDirectory '07-pre-retranscription-backup.json'
$uiResultPath = Join-Path $EvidenceDirectory '11-vulkan-real-51min-retranscription.json'
$historyBeforePath = Join-Path $EvidenceDirectory '06-real-generation-history-before.json'
$historyAfterPath = Join-Path $EvidenceDirectory '12-generation-history-after-retranscription.json'

$baseline = Get-Content -LiteralPath $baselinePath -Raw | ConvertFrom-Json
$uiResult = Get-Content -LiteralPath $uiResultPath -Raw | ConvertFrom-Json
$historyBefore = Get-Content -LiteralPath $historyBeforePath -Raw | ConvertFrom-Json
$historyAfter = Get-Content -LiteralPath $historyAfterPath -Raw | ConvertFrom-Json

$audio = Get-FileEvidence -Path (Join-Path $MeetingDirectory 'audio.mp4')
$metadata = Get-FileEvidence -Path (Join-Path $MeetingDirectory 'metadata.json')
$transcripts = Get-FileEvidence -Path (Join-Path $MeetingDirectory 'transcripts.json')
$metadataData = Get-Content -LiteralPath $metadata.path -Raw | ConvertFrom-Json
$transcriptData = Get-Content -LiteralPath $transcripts.path -Raw | ConvertFrom-Json

$snapshotFiles = @(Get-ChildItem -LiteralPath (Join-Path $MeetingDirectory 'summary-template-snapshots') -File -ErrorAction SilentlyContinue)
$snapshotEvidence = @($snapshotFiles | ForEach-Object { Get-FileEvidence -Path $_.FullName })
$baselineSnapshotHashes = @($baseline.source.snapshots | ForEach-Object { $_.sha256 } | Sort-Object)
$currentSnapshotHashes = @($snapshotEvidence | ForEach-Object { $_.sha256 } | Sort-Object)
$historyIdsBefore = @(Get-GenerationIds -History $historyBefore)
$historyIdsAfter = @(Get-GenerationIds -History $historyAfter)

$checks = [ordered]@{
    uiCompleted = $uiResult.complete -eq $true
    uiHasNoFailure = $uiResult.errorVisible -eq $false
    chineseLanguageSelected = $uiResult.selectedLanguage -eq '中文'
    expectedWhisperModelSelected = $uiResult.selectedModel -like '*large-v3-turbo-q5_0*'
    transcriptCountMatchesUi = [int]$transcriptData.total_segments -eq @($uiResult.meeting.transcripts).Count
    transcriptCountIs137 = [int]$transcriptData.total_segments -eq 137
    audioHashUnchanged = $audio.sha256 -eq $baseline.source.audio.sha256
    metadataHashChanged = $metadata.sha256 -ne $baseline.source.metadata.sha256
    transcriptHashChanged = $transcripts.sha256 -ne $baseline.source.transcripts.sha256
    retranscribedAtPersisted = -not [string]::IsNullOrWhiteSpace([string]$metadataData.retranscribed_at)
    legacyMeetingContextStillAbsent = $null -eq $metadataData.meeting_context
    existingTemplateBindingPreserved =
        $metadataData.summary_template.template_id -eq 'license_station_weekly' -and
        [int]$metadataData.summary_template.template_version -eq 1
    snapshotHashesUnchanged = ($baselineSnapshotHashes -join '|') -eq ($currentSnapshotHashes -join '|')
    generationHistoryIdsUnchanged = ($historyIdsBefore -join '|') -eq ($historyIdsAfter -join '|')
    noActiveGenerationAfterRetranscription = @($historyAfter.result | Where-Object { $_.isActiveGeneration }).Count -eq 0
}

$evidence = [ordered]@{
    capturedAt = [datetimeoffset]::Now.ToUniversalTime().ToString('o')
    meetingDirectory = $MeetingDirectory
    uiResult = [ordered]@{
        elapsedMs = $uiResult.elapsedMs
        selectedLanguage = $uiResult.selectedLanguage
        selectedModel = $uiResult.selectedModel
        complete = $uiResult.complete
        errorVisible = $uiResult.errorVisible
        transcriptCount = @($uiResult.meeting.transcripts).Count
    }
    files = [ordered]@{
        audio = $audio
        metadata = $metadata
        transcripts = $transcripts
        snapshots = $snapshotEvidence
    }
    before = [ordered]@{
        audioSha256 = $baseline.source.audio.sha256
        metadataSha256 = $baseline.source.metadata.sha256
        transcriptsSha256 = $baseline.source.transcripts.sha256
        generationIds = $historyIdsBefore
    }
    after = [ordered]@{
        metadata = $metadataData
        totalSegments = [int]$transcriptData.total_segments
        generationIds = $historyIdsAfter
    }
    checks = $checks
    verdict = if (@($checks.Values | Where-Object { $_ -ne $true }).Count -eq 0) { 'PASS' } else { 'FAIL' }
}

$json = $evidence | ConvertTo-Json -Depth 12
$outputPath = Join-Path $EvidenceDirectory $OutputName
[System.IO.File]::WriteAllText($outputPath, $json + [Environment]::NewLine, [System.Text.UTF8Encoding]::new($false))
$json

if ($evidence.verdict -ne 'PASS') {
    exit 1
}

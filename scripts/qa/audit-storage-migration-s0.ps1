[CmdletBinding()]
param(
    [Parameter()]
    [string]$SourceAppData = "C:\Users\liuxin\AppData\Roaming\com.meetily.ai",

    [Parameter()]
    [string]$TargetRoot = "E:\MeetilyData",

    [Parameter()]
    [string]$EvidenceDir = "D:\桌面\meetlily\target\release\docs\方案\Meetily统一存储迁移验收证据-20260828",

    [Parameter()]
    [string]$PythonPath = "C:\Users\liuxin\.cache\codex-runtimes\codex-primary-runtime\dependencies\python\python.exe",

    [Parameter()]
    [double]$MinimumFreeGiB = 20
)

$ErrorActionPreference = "Stop"
$startedAtUtc = [DateTime]::UtcNow
$scriptExitCode = 1
$errors = [System.Collections.Generic.List[string]]::new()
$checks = [System.Collections.Generic.List[object]]::new()
$probePath = $null

function Add-Check {
    param(
        [Parameter(Mandatory)] [string]$Name,
        [Parameter(Mandatory)] [bool]$Passed,
        [Parameter(Mandatory)] [string]$Evidence
    )

    $script:checks.Add([ordered]@{
        name = $Name
        passed = $Passed
        evidence = $Evidence
    })

    if (-not $Passed) {
        $script:errors.Add("${Name}: ${Evidence}")
    }
}

function Write-JsonFile {
    param(
        [Parameter(Mandatory)] [string]$Path,
        [Parameter(Mandatory)] $Value
    )

    $json = $Value | ConvertTo-Json -Depth 12
    [System.IO.File]::WriteAllText(
        $Path,
        $json + [Environment]::NewLine,
        [System.Text.UTF8Encoding]::new($false)
    )
}

function Get-ModelIdentity {
    param([Parameter(Mandatory)] [System.IO.FileInfo]$File)

    $relative = [System.IO.Path]::GetRelativePath($modelsRoot, $File.FullName)
    if ($relative.StartsWith("summary\", [System.StringComparison]::OrdinalIgnoreCase)) {
        return [ordered]@{ engine = "summary"; modelName = [System.IO.Path]::GetFileNameWithoutExtension($File.Name) }
    }
    if ($relative.StartsWith("parakeet\", [System.StringComparison]::OrdinalIgnoreCase)) {
        return [ordered]@{ engine = "parakeet"; modelName = "parakeet-tdt-0.6b-v3-int8" }
    }

    $name = switch ($File.Name) {
        "ggml-base-q5_1.bin" { "Whisper Base Q5_1" }
        "ggml-small-q5_1.bin" { "Whisper Small Q5_1" }
        "ggml-large-v3-turbo-q5_0.bin" { "Whisper Large V3 Turbo Q5_0" }
        default { [System.IO.Path]::GetFileNameWithoutExtension($File.Name) }
    }
    return [ordered]@{ engine = "whisper"; modelName = $name }
}

function Get-ModelManifest {
    param([Parameter(Mandatory)] [string]$Root)

    $files = @(Get-ChildItem -LiteralPath $Root -Recurse -File | Sort-Object FullName)
    $items = foreach ($file in $files) {
        $identity = Get-ModelIdentity -File $file
        $hash = Get-FileHash -LiteralPath $file.FullName -Algorithm SHA256
        [ordered]@{
            relativePath = [System.IO.Path]::GetRelativePath($Root, $file.FullName)
            bytes = [int64]$file.Length
            sha256 = $hash.Hash.ToUpperInvariant()
            lastWriteTimeUtc = $file.LastWriteTimeUtc.ToString("o")
            engine = $identity.engine
            modelName = $identity.modelName
            status = "available"
        }
    }

    return @($items)
}

try {
    $sourceRoot = [System.IO.Path]::GetFullPath($SourceAppData)
    $targetFullPath = [System.IO.Path]::GetFullPath($TargetRoot)
    $evidenceFullPath = [System.IO.Path]::GetFullPath($EvidenceDir)
    $modelsRoot = Join-Path $sourceRoot "models"
    $backupRoot = Join-Path $evidenceFullPath "S0-backup"

    [System.IO.Directory]::CreateDirectory($evidenceFullPath) | Out-Null
    [System.IO.Directory]::CreateDirectory($backupRoot) | Out-Null

    Add-Check -Name "source_app_data_exists" -Passed (Test-Path -LiteralPath $sourceRoot -PathType Container) -Evidence $sourceRoot
    Add-Check -Name "source_models_exists" -Passed (Test-Path -LiteralPath $modelsRoot -PathType Container) -Evidence $modelsRoot

    $blockedProcesses = @(
        Get-CimInstance Win32_Process | Where-Object {
            $name = [string]$_.Name
            $path = [string]$_.ExecutablePath
            $commandLine = [string]$_.CommandLine
            $name -ieq "meetily.exe" -or
            $name -like "llama-helper*.exe" -or
            $name -like "moss-helper*.exe" -or
            ($name -ieq "ffmpeg.exe" -and $path -match "(?i)meetily") -or
            ($name -match "(?i)^python(?:w)?\.exe$" -and $commandLine -match "(?i)MOSS-Transcribe-Diarize|moss-poc")
        } | Select-Object ProcessId, Name, ExecutablePath, CommandLine
    )
    $blockedProcessEvidence = if ($blockedProcesses.Count -eq 0) {
        "No Meetily, llama-helper, MOSS helper, or Meetily ffmpeg process is running."
    }
    else {
        $blockedProcesses | ConvertTo-Json -Depth 4 -Compress
    }
    Add-Check -Name "meetily_and_model_helpers_stopped" -Passed ($blockedProcesses.Count -eq 0) -Evidence $blockedProcessEvidence

    $targetDrive = [System.IO.Path]::GetPathRoot($targetFullPath).TrimEnd('\').TrimEnd(':')
    $logicalDisk = Get-CimInstance Win32_LogicalDisk -Filter "DeviceID='$($targetDrive):'"
    Add-Check -Name "target_drive_present" -Passed ($null -ne $logicalDisk) -Evidence "$($targetDrive):"
    if ($null -ne $logicalDisk) {
        Add-Check -Name "target_drive_fixed" -Passed ($logicalDisk.DriveType -eq 3) -Evidence "DriveType=$($logicalDisk.DriveType) (3=fixed)"
        Add-Check -Name "target_drive_ntfs" -Passed ($logicalDisk.FileSystem -ieq "NTFS") -Evidence "FileSystem=$($logicalDisk.FileSystem)"
        $freeGiB = [Math]::Round(([double]$logicalDisk.FreeSpace / 1GB), 3)
        Add-Check -Name "target_free_space" -Passed ($freeGiB -ge $MinimumFreeGiB) -Evidence "FreeGiB=$freeGiB; requiredGiB=$MinimumFreeGiB"

        $probeName = ".meetily-s0-write-probe-$([Guid]::NewGuid().ToString('N')).tmp"
        $probePath = Join-Path "$($targetDrive):\" $probeName
        $probeContent = "meetily-storage-s0-$([Guid]::NewGuid().ToString('N'))"
        [System.IO.File]::WriteAllText($probePath, $probeContent, [System.Text.UTF8Encoding]::new($false))
        $probeReadBack = [System.IO.File]::ReadAllText($probePath, [System.Text.Encoding]::UTF8)
        Add-Check -Name "target_write_read_test" -Passed ($probeReadBack -ceq $probeContent) -Evidence "Exact temporary probe write/read succeeded on $($targetDrive):"
        [System.IO.File]::Delete($probePath)
        Add-Check -Name "target_probe_cleanup" -Passed (-not (Test-Path -LiteralPath $probePath)) -Evidence $probePath
        $probePath = $null
    }

    if ($errors.Count -gt 0) {
        throw "S0 preflight gate failed before source hashing."
    }

    $manifestBefore = Get-ModelManifest -Root $modelsRoot
    # Get-ModelManifest returns ordered dictionaries. Measure-Object does not
    # reliably read dictionary keys as properties on every PowerShell version,
    # which can silently produce a zero total even though every file entry has
    # the correct byte count. Project the key explicitly before summing.
    $sourceBytes = [int64](($manifestBefore | ForEach-Object { [int64]$_['bytes'] } | Measure-Object -Sum).Sum)
    $actualSourceBytes = [int64]((Get-ChildItem -LiteralPath $modelsRoot -Recurse -File | Measure-Object -Property Length -Sum).Sum)
    Add-Check -Name "source_model_file_count" -Passed ($manifestBefore.Count -eq 9) -Evidence "count=$($manifestBefore.Count); expected=9"
    Add-Check -Name "source_model_hash_count" -Passed (@($manifestBefore | Where-Object { $_.sha256 -match '^[0-9A-F]{64}$' }).Count -eq 9) -Evidence "validSha256Count=$(@($manifestBefore | Where-Object { $_.sha256 -match '^[0-9A-F]{64}$' }).Count)"
    Add-Check -Name "source_model_total_bytes" -Passed ($sourceBytes -gt 0 -and $sourceBytes -eq $actualSourceBytes) -Evidence "manifestBytes=$sourceBytes; actualBytes=$actualSourceBytes"

    $manifestDocument = [ordered]@{
        schemaVersion = 1
        generatedAtUtc = [DateTime]::UtcNow.ToString("o")
        sourceRoot = $modelsRoot
        fileCount = $manifestBefore.Count
        totalBytes = $sourceBytes
        files = $manifestBefore
    }
    Write-JsonFile -Path (Join-Path $evidenceFullPath "source-model-manifest.json") -Value $manifestDocument
    Write-JsonFile -Path (Join-Path $evidenceFullPath "S0-source-inventory.json") -Value $manifestDocument

    $configNames = @(
        "storage-preferences.v1.json",
        "recording_preferences.json",
        "preferences.json",
        "summary-template-preferences.v1.json",
        "ui-locale.json"
    )
    $configInventory = foreach ($name in $configNames) {
        $source = Join-Path $sourceRoot $name
        if (Test-Path -LiteralPath $source -PathType Leaf) {
            $destination = Join-Path $backupRoot $name
            Copy-Item -LiteralPath $source -Destination $destination -Force
            [ordered]@{
                name = $name
                exists = $true
                bytes = [int64](Get-Item -LiteralPath $source).Length
                sha256 = (Get-FileHash -LiteralPath $source -Algorithm SHA256).Hash.ToUpperInvariant()
                backupPath = $destination
                backupSha256 = (Get-FileHash -LiteralPath $destination -Algorithm SHA256).Hash.ToUpperInvariant()
            }
        }
        else {
            [ordered]@{
                name = $name
                exists = $false
                bytes = 0
                sha256 = $null
                backupPath = $null
                backupSha256 = $null
            }
        }
    }

    $databasePath = Join-Path $sourceRoot "meeting_minutes.sqlite"
    Add-Check -Name "database_exists" -Passed (Test-Path -LiteralPath $databasePath -PathType Leaf) -Evidence $databasePath
    Add-Check -Name "python_for_consistent_database_backup" -Passed (Test-Path -LiteralPath $PythonPath -PathType Leaf) -Evidence $PythonPath
    if ($errors.Count -gt 0) {
        throw "S0 database backup prerequisites failed."
    }

    $databaseBackupPath = Join-Path $backupRoot "meeting_minutes.sqlite"
    $modelStatePath = Join-Path $evidenceFullPath "S0-current-model-state.json"
    $pythonCode = @'
import json
import sqlite3
import sys

source_path, backup_path, state_path = sys.argv[1:4]
source = sqlite3.connect("file:" + source_path.replace("\\", "/") + "?mode=ro", uri=True)
integrity = [row[0] for row in source.execute("PRAGMA integrity_check")]
destination = sqlite3.connect(backup_path)
source.backup(destination)
destination_integrity = [row[0] for row in destination.execute("PRAGMA integrity_check")]
summary = source.execute('SELECT provider, model, "whisperModel" FROM settings LIMIT 1').fetchone()
transcription = source.execute('SELECT provider, model FROM transcript_settings LIMIT 1').fetchone()
state = {
    "schemaVersion": 1,
    "sourceIntegrityCheck": integrity,
    "backupIntegrityCheck": destination_integrity,
    "summaryModelConfig": None if summary is None else {
        "provider": summary[0],
        "model": summary[1],
        "legacyWhisperModelField": summary[2],
    },
    "transcriptionModelConfig": None if transcription is None else {
        "provider": transcription[0],
        "model": transcription[1],
    },
}
with open(state_path, "w", encoding="utf-8", newline="\n") as output:
    json.dump(state, output, ensure_ascii=False, indent=2)
    output.write("\n")
destination.close()
source.close()
'@
    & $PythonPath -c $pythonCode $databasePath $databaseBackupPath $modelStatePath
    if ($LASTEXITCODE -ne 0) {
        throw "Python database backup returned exit code $LASTEXITCODE."
    }

    $databaseSidecars = foreach ($suffix in @("-wal", "-shm")) {
        $source = "$databasePath$suffix"
        if (Test-Path -LiteralPath $source -PathType Leaf) {
            $destination = "$databaseBackupPath$suffix"
            Copy-Item -LiteralPath $source -Destination $destination -Force
            [ordered]@{
                suffix = $suffix
                exists = $true
                bytes = [int64](Get-Item -LiteralPath $source).Length
                sha256 = (Get-FileHash -LiteralPath $source -Algorithm SHA256).Hash.ToUpperInvariant()
                backupPath = $destination
            }
        }
        else {
            [ordered]@{ suffix = $suffix; exists = $false; bytes = 0; sha256 = $null; backupPath = $null }
        }
    }

    $databaseState = Get-Content -LiteralPath $modelStatePath -Raw | ConvertFrom-Json
    Add-Check -Name "source_database_integrity" -Passed (@($databaseState.sourceIntegrityCheck).Count -eq 1 -and $databaseState.sourceIntegrityCheck[0] -eq "ok") -Evidence ($databaseState.sourceIntegrityCheck -join ",")
    Add-Check -Name "backup_database_integrity" -Passed (@($databaseState.backupIntegrityCheck).Count -eq 1 -and $databaseState.backupIntegrityCheck[0] -eq "ok") -Evidence ($databaseState.backupIntegrityCheck -join ",")
    Add-Check -Name "database_backup_hash_present" -Passed ((Get-FileHash -LiteralPath $databaseBackupPath -Algorithm SHA256).Hash -match '^[0-9A-Fa-f]{64}$') -Evidence $databaseBackupPath

    $manifestAfter = Get-ModelManifest -Root $modelsRoot
    $beforeCompact = $manifestBefore | ForEach-Object { "$($_.relativePath)|$($_.bytes)|$($_.sha256)|$($_.lastWriteTimeUtc)" }
    $afterCompact = $manifestAfter | ForEach-Object { "$($_.relativePath)|$($_.bytes)|$($_.sha256)|$($_.lastWriteTimeUtc)" }
    $sourceUnchanged = ($beforeCompact.Count -eq $afterCompact.Count) -and -not (Compare-Object -ReferenceObject $beforeCompact -DifferenceObject $afterCompact)
    Add-Check -Name "source_models_unchanged" -Passed $sourceUnchanged -Evidence "Compared path, bytes, SHA-256, and UTC last-write time before and after S0."

    $configBackupsMatch = @($configInventory | Where-Object { $_.exists -and $_.sha256 -ne $_.backupSha256 }).Count -eq 0
    Add-Check -Name "configuration_backups_match" -Passed $configBackupsMatch -Evidence "All present configuration copies match their source SHA-256."

    $allChecksPassed = @($checks | Where-Object { -not $_.passed }).Count -eq 0
    $scriptExitCode = if ($allChecksPassed) { 0 } else { 1 }

    $preflight = [ordered]@{
        schemaVersion = 1
        stage = "S0"
        status = if ($allChecksPassed) { "PASS" } else { "FAIL" }
        startedAtUtc = $startedAtUtc.ToString("o")
        finishedAtUtc = [DateTime]::UtcNow.ToString("o")
        command = "pwsh -NoProfile -File scripts/qa/audit-storage-migration-s0.ps1"
        exitCode = $scriptExitCode
        sourceAppData = $sourceRoot
        sourceModels = $modelsRoot
        targetRootPlanned = $targetFullPath
        targetRootCreated = (Test-Path -LiteralPath $targetFullPath)
        minimumFreeGiB = $MinimumFreeGiB
        sourceModelFileCount = $manifestBefore.Count
        sourceModelTotalBytes = $sourceBytes
        blockedProcesses = $blockedProcesses
        configurationInventory = $configInventory
        database = [ordered]@{
            sourcePath = $databasePath
            sourceBytes = [int64](Get-Item -LiteralPath $databasePath).Length
            sourceSha256 = (Get-FileHash -LiteralPath $databasePath -Algorithm SHA256).Hash.ToUpperInvariant()
            consistentBackupPath = $databaseBackupPath
            consistentBackupBytes = [int64](Get-Item -LiteralPath $databaseBackupPath).Length
            consistentBackupSha256 = (Get-FileHash -LiteralPath $databaseBackupPath -Algorithm SHA256).Hash.ToUpperInvariant()
            sidecars = @($databaseSidecars)
        }
        checks = @($checks)
        errors = @($errors)
    }
    Write-JsonFile -Path (Join-Path $evidenceFullPath "S0-preflight.json") -Value $preflight
}
catch {
    $errors.Add($_.Exception.Message)
    try {
        if ($null -ne $probePath -and (Test-Path -LiteralPath $probePath -PathType Leaf)) {
            [System.IO.File]::Delete($probePath)
        }

        if (-not (Test-Path -LiteralPath $EvidenceDir -PathType Container)) {
            [System.IO.Directory]::CreateDirectory([System.IO.Path]::GetFullPath($EvidenceDir)) | Out-Null
        }
        $failure = [ordered]@{
            schemaVersion = 1
            stage = "S0"
            status = "FAIL"
            startedAtUtc = $startedAtUtc.ToString("o")
            finishedAtUtc = [DateTime]::UtcNow.ToString("o")
            command = "pwsh -NoProfile -File scripts/qa/audit-storage-migration-s0.ps1"
            exitCode = 1
            sourceAppData = $SourceAppData
            targetRootPlanned = $TargetRoot
            checks = @($checks)
            errors = @($errors)
        }
        Write-JsonFile -Path (Join-Path ([System.IO.Path]::GetFullPath($EvidenceDir)) "S0-preflight.json") -Value $failure
    }
    catch {
        Write-Error "Unable to persist S0 failure evidence: $($_.Exception.Message)"
    }
    $scriptExitCode = 1
}

if ($scriptExitCode -eq 0) {
    Write-Output "S0 PASS: source inventory, hashes, disk gate, configuration backup, and database backup completed."
}
else {
    Write-Error "S0 FAIL: $($errors -join '; ')"
}

exit $scriptExitCode

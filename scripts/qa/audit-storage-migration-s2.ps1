[CmdletBinding()]
param(
    [Parameter()]
    [string]$RepositoryRoot = "D:\桌面\meetlily",

    [Parameter()]
    [string]$SourceAppData = "C:\Users\liuxin\AppData\Roaming\com.meetily.ai",

    [Parameter()]
    [string]$TargetRoot = "E:\MeetilyData",

    [Parameter()]
    [string]$EvidenceDir = "D:\桌面\meetlily\target\release\docs\方案\Meetily统一存储迁移验收证据-20260828"
)

$ErrorActionPreference = "Stop"
$startedAtUtc = [DateTime]::UtcNow
$checks = [System.Collections.Generic.List[object]]::new()
$errors = [System.Collections.Generic.List[string]]::new()
$currentManifest = [System.Collections.Generic.List[object]]::new()

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

    $json = $Value | ConvertTo-Json -Depth 18
    [System.IO.File]::WriteAllText(
        $Path,
        $json + [Environment]::NewLine,
        [System.Text.UTF8Encoding]::new($false)
    )
}

function Test-Log {
    param(
        [Parameter(Mandatory)] [string]$Name,
        [Parameter(Mandatory)] [string]$Path,
        [Parameter(Mandatory)] [string[]]$RequiredPatterns
    )

    if (-not (Test-Path -LiteralPath $Path -PathType Leaf)) {
        Add-Check -Name $Name -Passed $false -Evidence "missing: $Path"
        return
    }
    $text = [System.IO.File]::ReadAllText($Path)
    $missing = @($RequiredPatterns | Where-Object { $text -notmatch $_ })
    Add-Check `
        -Name $Name `
        -Passed ($missing.Count -eq 0) `
        -Evidence $(if ($missing.Count -eq 0) {
            "all required result markers present in $([System.IO.Path]::GetFileName($Path))"
        } else {
            "missing result markers: $($missing -join ', ')"
        })
}

$repositoryFullPath = [System.IO.Path]::GetFullPath($RepositoryRoot)
$sourceRoot = [System.IO.Path]::GetFullPath($SourceAppData)
$sourceModels = Join-Path $sourceRoot "models"
$targetFullPath = [System.IO.Path]::GetFullPath($TargetRoot)
$preferencesPath = Join-Path $sourceRoot "storage-preferences.v1.json"
$evidenceFullPath = [System.IO.Path]::GetFullPath($EvidenceDir)
$manifestPath = Join-Path $evidenceFullPath "source-model-manifest.json"
$migrationSourcePath = Join-Path $repositoryFullPath "frontend\src-tauri\src\storage\migration.rs"
$storageModulePath = Join-Path $repositoryFullPath "frontend\src-tauri\src\storage\mod.rs"
$libPath = Join-Path $repositoryFullPath "frontend\src-tauri\src\lib.rs"

try {
    Add-Check -Name "repository_exists" -Passed (Test-Path -LiteralPath $repositoryFullPath -PathType Container) -Evidence $repositoryFullPath
    Add-Check -Name "source_models_exist" -Passed (Test-Path -LiteralPath $sourceModels -PathType Container) -Evidence $sourceModels
    Add-Check -Name "s0_source_manifest_exists" -Passed (Test-Path -LiteralPath $manifestPath -PathType Leaf) -Evidence $manifestPath
    Add-Check -Name "migration_source_exists" -Passed (Test-Path -LiteralPath $migrationSourcePath -PathType Leaf) -Evidence $migrationSourcePath
    if ($errors.Count -gt 0) {
        throw "S2 audit prerequisites are missing."
    }

    $blockedProcesses = @(
        Get-CimInstance Win32_Process | Where-Object {
            $name = [string]$_.Name
            $path = [string]$_.ExecutablePath
            $commandLine = [string]$_.CommandLine
            $name -ieq "meetily.exe" -or
            $name -like "llama-helper*.exe" -or
            $name -like "moss-helper*.exe" -or
            ($name -ieq "ffmpeg.exe" -and $path -match "(?i)meetily") -or
            ($commandLine -match "(?i)MeetilyData|com\.meetily\.ai\\models")
        } | ForEach-Object {
            [ordered]@{
                processId = $_.ProcessId
                name = $_.Name
                executablePath = $_.ExecutablePath
            }
        }
    )
    Add-Check -Name "application_and_model_helpers_stopped" -Passed ($blockedProcesses.Count -eq 0) -Evidence "blockedProcessCount=$($blockedProcesses.Count)"
    Add-Check -Name "formal_target_not_created" -Passed (-not (Test-Path -LiteralPath $targetFullPath)) -Evidence $targetFullPath
    Add-Check -Name "formal_storage_preferences_not_created" -Passed (-not (Test-Path -LiteralPath $preferencesPath)) -Evidence $preferencesPath

    $manifest = Get-Content -LiteralPath $manifestPath -Raw | ConvertFrom-Json
    $manifestByRelativePath = @{}
    foreach ($entry in @($manifest.files)) {
        $manifestByRelativePath[[string]$entry.relativePath] = $entry
    }
    $sourceFiles = @(Get-ChildItem -LiteralPath $sourceModels -Recurse -File | Sort-Object FullName)
    $hashMismatches = [System.Collections.Generic.List[string]]::new()
    foreach ($file in $sourceFiles) {
        $relativePath = [System.IO.Path]::GetRelativePath($sourceModels, $file.FullName)
        $entry = $manifestByRelativePath[$relativePath]
        $hash = (Get-FileHash -LiteralPath $file.FullName -Algorithm SHA256).Hash.ToUpperInvariant()
        $currentManifest.Add([ordered]@{
            relativePath = $relativePath
            bytes = [int64]$file.Length
            sha256 = $hash
        })
        if ($null -eq $entry) {
            $hashMismatches.Add("missing manifest entry: $relativePath")
        }
        elseif ([int64]$entry.bytes -ne [int64]$file.Length -or [string]$entry.sha256 -cne $hash) {
            $hashMismatches.Add("changed: $relativePath")
        }
    }
    foreach ($entry in @($manifest.files)) {
        if (-not (Test-Path -LiteralPath (Join-Path $sourceModels ([string]$entry.relativePath)) -PathType Leaf)) {
            $hashMismatches.Add("missing source file: $($entry.relativePath)")
        }
    }
    $sourceBytes = [int64](($currentManifest | ForEach-Object { [int64]$_['bytes'] } | Measure-Object -Sum).Sum)
    Add-Check -Name "formal_source_model_count" -Passed ($sourceFiles.Count -eq 9 -and [int]$manifest.fileCount -eq 9) -Evidence "current=$($sourceFiles.Count); baseline=$($manifest.fileCount)"
    Add-Check -Name "formal_source_model_bytes" -Passed ($sourceBytes -eq [int64]$manifest.totalBytes) -Evidence "current=$sourceBytes; baseline=$($manifest.totalBytes)"
    Add-Check -Name "formal_source_models_match_s0_sha256" -Passed ($hashMismatches.Count -eq 0) -Evidence $(if ($hashMismatches.Count -eq 0) { "all 9 paths, byte counts, and SHA-256 values match S0" } else { $hashMismatches -join "; " })

    $testEvidenceNames = @(
        "S2-T01-normal-copy.json",
        "S2-T02-repeat.json",
        "S2-T03-interruption-10.json",
        "S2-T04-interruption-90.json",
        "S2-T05-write-failure.json",
        "S2-T06-space-failure.json",
        "S2-T07-existing-target-corruption.json",
        "S2-T08-partial-corruption.json",
        "S2-T09-state-corruption.json",
        "S2-T10-source-change.json",
        "S2-T11-ready-target-corruption.json",
        "S2-T12-preferences-sentinel.json",
        "S2-T13-unicode-long-path.json",
        "S2-T14-source-integrity.json"
    )
    $testDocuments = [System.Collections.Generic.List[object]]::new()
    foreach ($name in $testEvidenceNames) {
        $path = Join-Path $evidenceFullPath $name
        if (-not (Test-Path -LiteralPath $path -PathType Leaf)) {
            Add-Check -Name "test_evidence_$name" -Passed $false -Evidence "missing: $path"
            continue
        }
        $document = Get-Content -LiteralPath $path -Raw | ConvertFrom-Json
        $testDocuments.Add($document)
        $passed = (
            $document.status -eq "PASS" -and
            [int]$document.exitCode -eq 0 -and
            [bool]$document.temporaryInputOnly -and
            -not [bool]$document.formalPathsTouched -and
            @($document.errors).Count -eq 0
        )
        Add-Check -Name "test_evidence_$($document.testId)" -Passed $passed -Evidence "status=$($document.status); exitCode=$($document.exitCode); temporaryOnly=$($document.temporaryInputOnly); errors=$(@($document.errors).Count)"
    }
    $testIds = @($testDocuments | ForEach-Object { [string]$_.testId } | Sort-Object -Unique)
    $expectedIds = @(1..14 | ForEach-Object { "S2-T$($_.ToString('00'))" })
    Add-Check -Name "all_fourteen_unique_s2_tests_have_evidence" -Passed ($testIds.Count -eq 14 -and (Compare-Object $testIds $expectedIds).Count -eq 0) -Evidence "testIds=$($testIds -join ',')"

    $t01 = $testDocuments | Where-Object testId -eq "S2-T01"
    $t02 = $testDocuments | Where-Object testId -eq "S2-T02"
    $t03 = $testDocuments | Where-Object testId -eq "S2-T03"
    $t04 = $testDocuments | Where-Object testId -eq "S2-T04"
    $t05 = $testDocuments | Where-Object testId -eq "S2-T05"
    $t06 = $testDocuments | Where-Object testId -eq "S2-T06"
    $t07 = $testDocuments | Where-Object testId -eq "S2-T07"
    $t08 = $testDocuments | Where-Object testId -eq "S2-T08"
    $t09 = $testDocuments | Where-Object testId -eq "S2-T09"
    $t10 = $testDocuments | Where-Object testId -eq "S2-T10"
    $t11 = $testDocuments | Where-Object testId -eq "S2-T11"
    $t12 = $testDocuments | Where-Object testId -eq "S2-T12"
    $t13 = $testDocuments | Where-Object testId -eq "S2-T13"
    $t14 = $testDocuments | Where-Object testId -eq "S2-T14"

    Add-Check -Name "t01_normal_copy_ready_and_hash_equal" -Passed ($t01.details.phase -eq "ready_to_switch" -and [bool]$t01.details.targetMatchesSourceSha256) -Evidence ($t01.details | ConvertTo-Json -Compress)
    Add-Check -Name "t02_repeat_writes_zero_bytes" -Passed ([int64]$t02.details.bytesWrittenOnRepeat -eq 0 -and [bool]$t02.details.targetUnchanged) -Evidence ($t02.details | ConvertTo-Json -Compress)
    Add-Check -Name "t03_ten_percent_resume_uses_partial" -Passed ([int64]$t03.details.resumedFromBytes -gt 0 -and [int64]$t03.details.bytesWrittenAfterResume -lt [int64]$t03.details.totalBytes -and $t03.details.finalPhase -eq "ready_to_switch") -Evidence ($t03.details | ConvertTo-Json -Compress)
    Add-Check -Name "t04_ninety_percent_resume_does_not_recopy" -Passed ([int64]$t04.details.bytesWrittenAfterResume -eq [int64]$t04.details.remainingBytes -and [int64]$t04.details.skippedCompletedFiles -gt 0 -and $t04.details.finalPhase -eq "ready_to_switch") -Evidence ($t04.details | ConvertTo-Json -Compress)
    Add-Check -Name "t05_write_failure_retains_and_recovers" -Passed ($t05.details.failureCode -eq "target_write_failed" -and [bool]$t05.details.partialRetained -and $t05.details.recoveredPhase -eq "ready_to_switch") -Evidence ($t05.details | ConvertTo-Json -Compress)
    Add-Check -Name "t06_space_failure_precedes_target_creation" -Passed ([bool]$t06.details.preflightRejected -and -not [bool]$t06.details.migrationControlCreated -and -not [bool]$t06.details.formalTargetFilesCreated) -Evidence ($t06.details | ConvertTo-Json -Compress)
    Add-Check -Name "t07_corrupt_target_quarantined" -Passed ([bool]$t07.details.quarantineExists -and [bool]$t07.details.sourceUnchanged -and $t07.details.retryPhase -eq "ready_to_switch") -Evidence ($t07.details | ConvertTo-Json -Compress)
    Add-Check -Name "t08_corrupt_partial_never_appended" -Passed (-not [bool]$t08.details.continuedAppendingToCorruptPartial -and $t08.details.retryPhase -eq "ready_to_switch") -Evidence ($t08.details | ConvertTo-Json -Compress)
    Add-Check -Name "t09_state_and_manifest_corruption_rejected" -Passed ([bool]$t09.details.invalidJsonRejected -and [bool]$t09.details.stateHashMismatchRejected -and [bool]$t09.details.unknownSchemaRejected -and [bool]$t09.details.falseReadyStateRejected -and [bool]$t09.details.manifestHashMismatchRejected -and -not [bool]$t09.details.targetFilesCreated) -Evidence ($t09.details | ConvertTo-Json -Compress)
    Add-Check -Name "t10_changed_source_cannot_be_published" -Passed ([bool]$t10.details.sourceSha256Changed -and [bool]$t10.details.migrationFailed -and -not [bool]$t10.details.publishedChangedTarget -and $t10.details.failureCode -eq "target_hash_mismatch") -Evidence ($t10.details | ConvertTo-Json -Compress)
    Add-Check -Name "t11_ready_target_mutation_blocks_switch" -Passed ([bool]$t11.details.oneByteMutationDetected -and $t11.details.finalPhase -eq "failed" -and -not [bool]$t11.details.switched -and -not [bool]$t11.details.runtimeValidated) -Evidence ($t11.details | ConvertTo-Json -Compress)
    Add-Check -Name "t12_preferences_byte_and_hash_sentinel_unchanged" -Passed ([bool]$t12.details.byteForByteUnchanged -and $t12.details.sha256Before -eq $t12.details.sha256After -and -not [bool]$t12.details.migrationCompletedWritten) -Evidence ($t12.details | ConvertTo-Json -Compress)
    Add-Check -Name "t13_unicode_space_long_path_resumes" -Passed ([int64]$t13.details.resumedFromBytes -gt 0 -and $t13.details.finalPhase -eq "ready_to_switch" -and [bool]$t13.details.targetMatchesSourceSha256) -Evidence ($t13.details | ConvertTo-Json -Compress)
    Add-Check -Name "t14_source_integrity_after_interrupt_resume_repeat" -Passed ([int]$t14.details.sourceFileCountBefore -eq [int]$t14.details.sourceFileCountAfter -and [bool]$t14.details.pathsBytesAndSha256Unchanged -and [int64]$t14.details.bytesWrittenOnRepeat -eq 0) -Evidence ($t14.details | ConvertTo-Json -Compress)

    foreach ($aggregateName in @(
        "S2-interruption-resume.json",
        "S2-hash-corruption.json",
        "S2-state-corruption.json",
        "S2-space-and-write-failures.json",
        "S2-source-integrity.json"
    )) {
        $aggregatePath = Join-Path $evidenceFullPath $aggregateName
        if (-not (Test-Path -LiteralPath $aggregatePath -PathType Leaf)) {
            Add-Check -Name "aggregate_$aggregateName" -Passed $false -Evidence "missing: $aggregatePath"
            continue
        }
        $aggregate = Get-Content -LiteralPath $aggregatePath -Raw | ConvertFrom-Json
        Add-Check -Name "aggregate_$aggregateName" -Passed ($aggregate.status -eq "PASS" -and [int]$aggregate.exitCode -eq 0 -and @($aggregate.errors).Count -eq 0 -and -not [bool]$aggregate.formalTargetExistsAfter -and -not [bool]$aggregate.formalPreferencesExistAfter) -Evidence "status=$($aggregate.status); cases=$(@($aggregate.cases).Count); errors=$(@($aggregate.errors).Count)"
    }

    Test-Log -Name "s2_targeted_migration_tests" -Path (Join-Path $evidenceFullPath "S2-migration-tests.txt") -RequiredPatterns @(
        'test result: ok\. 14 passed; 0 failed; 0 ignored',
        'COMMAND_EXIT_CODE:\s*0'
    )
    Test-Log -Name "s2_full_rust_regression" -Path (Join-Path $evidenceFullPath "S2-regression-tests.txt") -RequiredPatterns @(
        'test result: ok\. 477 passed; 0 failed; 8 ignored',
        'COMMAND_EXIT_CODE:\s*0'
    )
    Test-Log -Name "s2_rust_format" -Path (Join-Path $evidenceFullPath "S2-cargo-fmt.txt") -RequiredPatterns @('COMMAND_EXIT_CODE:\s*0')
    Test-Log -Name "s2_frontend_typecheck" -Path (Join-Path $evidenceFullPath "S2-frontend-typecheck.txt") -RequiredPatterns @('COMMAND_EXIT_CODE:\s*0')
    Test-Log -Name "s2_frontend_node_tests" -Path (Join-Path $evidenceFullPath "S2-frontend-tests.txt") -RequiredPatterns @(
        '(?m)^# pass 138\r?$',
        '(?m)^# fail 0\r?$',
        'COMMAND_EXIT_CODE:\s*0'
    )

    $migrationText = [System.IO.File]::ReadAllText($migrationSourcePath)
    $productionText = $migrationText.Split("#[cfg(test)]", 2)[0]
    $storageModuleText = [System.IO.File]::ReadAllText($storageModulePath)
    $libText = [System.IO.File]::ReadAllText($libPath)
    $requiredMarkers = @(
        'pub struct MigrationEngine',
        'manifest_sha256',
        'state_sha256',
        '.partial',
        'sha256_prefix',
        'ReadyToSwitch',
        'validate_target_candidate',
        'quarantine_path',
        'write_json_atomic'
    )
    $missingMarkers = @($requiredMarkers | Where-Object { -not $productionText.Contains($_) })
    Add-Check -Name "migration_engine_required_code_markers" -Passed ($missingMarkers.Count -eq 0) -Evidence $(if ($missingMarkers.Count -eq 0) { "all required copy, resume, hash, state, and quarantine markers present" } else { "missing: $($missingMarkers -join ', ')" })
    Add-Check -Name "migration_module_is_compiled" -Passed ($storageModuleText -match '(?m)^pub mod migration;\s*$') -Evidence "frontend/src-tauri/src/storage/mod.rs declares migration"
    Add-Check -Name "migration_has_no_tauri_command" -Passed ($productionText -notmatch '#\[tauri::command\]') -Evidence "no migration method is decorated as a Tauri command"
    Add-Check -Name "migration_not_registered_in_invoke_handler" -Passed ($libText -notmatch 'storage::migration::|MigrationEngine|prepare_storage_migration|run_storage_migration') -Evidence "current release invoke handler contains no S2 migration execution entry"
    Add-Check -Name "s2_does_not_save_storage_preferences" -Passed ($productionText -notmatch 'preferences::save_atomic|save_atomic\s*\(\s*&?preferences') -Evidence "migration engine only loads and snapshots preferences"
    Add-Check -Name "s2_contains_no_formal_c_or_e_path" -Passed ($productionText -notmatch '(?i)E:\\MeetilyData|C:\\Users\\liuxin|com\.meetily\.ai') -Evidence "no machine-specific formal path is embedded in migration.rs"
    Add-Check -Name "s2_has_no_recursive_delete" -Passed ($productionText -notmatch 'remove_dir_all|remove_file\s*\([^\)]*models|remove_dir\s*\([^\)]*models') -Evidence "no recursive delete and no model-path delete call in production migration code"
    $removeCalls = @([regex]::Matches($productionText, 'fs::remove_(?:file|dir)\s*\([^;]+;') | ForEach-Object { $_.Value.Trim() })
    $unexpectedRemovals = @($removeCalls | Where-Object {
        $_ -notmatch 'remove_file\(self\.manifest_path\(\)\)' -and
        $_ -notmatch 'remove_dir\(&control_root\)' -and
        $_ -notmatch 'remove_file\(&temporary_path\)'
    })
    Add-Check -Name "all_delete_calls_are_exact_control_or_temp_cleanup" -Passed ($removeCalls.Count -eq 4 -and $unexpectedRemovals.Count -eq 0) -Evidence "removeCalls=$($removeCalls.Count); expectedExactCleanupCalls=4; unexpected=$($unexpectedRemovals -join ' | ')"

    $gitOutput = @(& git -C $repositoryFullPath diff --check 2>&1)
    $gitCode = $LASTEXITCODE
    Add-Check -Name "git_diff_check" -Passed ($gitCode -eq 0) -Evidence "exitCode=$gitCode; outputLines=$($gitOutput.Count)"
}
catch {
    $errors.Add($_.Exception.Message)
}

$allChecksPassed = $errors.Count -eq 0 -and @($checks | Where-Object { -not $_.passed }).Count -eq 0
$result = [ordered]@{
    schemaVersion = 1
    stage = "S2"
    status = if ($allChecksPassed) { "PASS" } else { "FAIL" }
    startedAtUtc = $startedAtUtc.ToString("o")
    finishedAtUtc = [DateTime]::UtcNow.ToString("o")
    command = "pwsh -NoProfile -File scripts/qa/audit-storage-migration-s2.ps1"
    exitCode = if ($allChecksPassed) { 0 } else { 1 }
    temporaryInputOnly = $true
    sourceAppData = $sourceRoot
    targetRootPlanned = $targetFullPath
    targetRootCreated = Test-Path -LiteralPath $targetFullPath
    storagePreferencesCreated = Test-Path -LiteralPath $preferencesPath
    sourceModelFileCount = $currentManifest.Count
    sourceModelTotalBytes = [int64](($currentManifest | ForEach-Object { [int64]$_['bytes'] } | Measure-Object -Sum).Sum)
    sourceModels = @($currentManifest)
    blockedProcesses = @($blockedProcesses)
    expectedNonBlockingWarnings = @(
        "Existing unrelated Rust compiler warnings are preserved in command logs.",
        "blocknote-markdown.test.ts requires Bun, which is unavailable; the established 138-test Node-compatible suite excludes only that file."
    )
    checks = @($checks)
    failedCheckCount = @($checks | Where-Object { -not $_.passed }).Count
    errors = @($errors)
}

[System.IO.Directory]::CreateDirectory($evidenceFullPath) | Out-Null
$resultPath = Join-Path $evidenceFullPath "S2-audit.json"
Write-JsonFile -Path $resultPath -Value $result

if (-not $allChecksPassed) {
    Write-Error "S2 audit failed. See $resultPath"
    exit 1
}

Write-Output "S2 PASS: 14 migration tests, full regressions, static safety gates, and formal source integrity all passed."
exit 0

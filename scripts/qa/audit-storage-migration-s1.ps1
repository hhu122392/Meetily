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
$forbiddenHits = [System.Collections.Generic.List[string]]::new()

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
    $evidenceText = if ($missing.Count -eq 0) {
        "all required result markers present in $([System.IO.Path]::GetFileName($Path))"
    }
    else {
        "missing result markers: $($missing -join ', ')"
    }
    Add-Check -Name $Name -Passed ($missing.Count -eq 0) -Evidence $evidenceText
}

$repositoryFullPath = [System.IO.Path]::GetFullPath($RepositoryRoot)
$sourceRoot = [System.IO.Path]::GetFullPath($SourceAppData)
$sourceModels = Join-Path $sourceRoot "models"
$targetFullPath = [System.IO.Path]::GetFullPath($TargetRoot)
$evidenceFullPath = [System.IO.Path]::GetFullPath($EvidenceDir)
$preferencesPath = Join-Path $sourceRoot "storage-preferences.v1.json"
$s0Path = Join-Path $evidenceFullPath "S0-preflight.json"
$manifestPath = Join-Path $evidenceFullPath "source-model-manifest.json"

try {
    Add-Check -Name "repository_exists" -Passed (Test-Path -LiteralPath $repositoryFullPath -PathType Container) -Evidence $repositoryFullPath
    Add-Check -Name "source_models_exist" -Passed (Test-Path -LiteralPath $sourceModels -PathType Container) -Evidence $sourceModels
    Add-Check -Name "s0_evidence_exists" -Passed (Test-Path -LiteralPath $s0Path -PathType Leaf) -Evidence $s0Path
    Add-Check -Name "source_manifest_exists" -Passed (Test-Path -LiteralPath $manifestPath -PathType Leaf) -Evidence $manifestPath

    if ($errors.Count -gt 0) {
        throw "Required S0 input is missing."
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
            [ordered]@{ processId = $_.ProcessId; name = $_.Name; executablePath = $_.ExecutablePath }
        }
    )
    Add-Check -Name "application_and_model_helpers_stopped" -Passed ($blockedProcesses.Count -eq 0) -Evidence "blockedProcessCount=$($blockedProcesses.Count)"

    Add-Check -Name "formal_storage_preferences_not_created" -Passed (-not (Test-Path -LiteralPath $preferencesPath)) -Evidence $preferencesPath
    Add-Check -Name "formal_target_not_created" -Passed (-not (Test-Path -LiteralPath $targetFullPath)) -Evidence $targetFullPath

    $leftoverProbes = @(
        Get-ChildItem -LiteralPath ([System.IO.Path]::GetPathRoot($targetFullPath)) -File -Filter ".meetily-storage-write-probe-*.tmp" -ErrorAction SilentlyContinue
    )
    Add-Check -Name "target_write_probe_cleaned" -Passed ($leftoverProbes.Count -eq 0) -Evidence "leftoverProbeCount=$($leftoverProbes.Count)"

    $s0 = Get-Content -LiteralPath $s0Path -Raw | ConvertFrom-Json
    Add-Check -Name "s0_preflight_passed" -Passed ($s0.status -eq "PASS" -and [int]$s0.exitCode -eq 0 -and @($s0.errors).Count -eq 0) -Evidence "status=$($s0.status); exitCode=$($s0.exitCode); errors=$(@($s0.errors).Count)"

    $manifest = Get-Content -LiteralPath $manifestPath -Raw | ConvertFrom-Json
    $sourceFiles = @(Get-ChildItem -LiteralPath $sourceModels -Recurse -File | Sort-Object FullName)
    $sourceBytes = [int64](($sourceFiles | Measure-Object -Property Length -Sum).Sum)
    Add-Check -Name "source_manifest_count_and_bytes" -Passed (
        [int]$manifest.fileCount -eq $sourceFiles.Count -and
        [int64]$manifest.totalBytes -eq $sourceBytes -and
        [int64]$s0.sourceModelTotalBytes -eq $sourceBytes
    ) -Evidence "files=$($sourceFiles.Count); bytes=$sourceBytes; manifestFiles=$($manifest.fileCount); manifestBytes=$($manifest.totalBytes); s0Bytes=$($s0.sourceModelTotalBytes)"

    $manifestByRelativePath = @{}
    foreach ($entry in @($manifest.files)) {
        $manifestByRelativePath[[string]$entry.relativePath] = $entry
    }

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
            continue
        }
        if ([int64]$entry.bytes -ne [int64]$file.Length -or [string]$entry.sha256 -cne $hash) {
            $hashMismatches.Add("changed: $relativePath")
        }
    }
    foreach ($entry in @($manifest.files)) {
        $candidate = Join-Path $sourceModels ([string]$entry.relativePath)
        if (-not (Test-Path -LiteralPath $candidate -PathType Leaf)) {
            $hashMismatches.Add("source file missing: $($entry.relativePath)")
        }
    }
    $hashEvidence = if ($hashMismatches.Count -eq 0) {
        "all $($sourceFiles.Count) files match path, bytes, and SHA-256"
    }
    else {
        $hashMismatches -join "; "
    }
    Add-Check -Name "source_models_still_match_s0_sha256" -Passed ($hashMismatches.Count -eq 0) -Evidence $hashEvidence

    $rustRoot = Join-Path $repositoryFullPath "frontend\src-tauri\src"
    $compiledRustFiles = @(
        Get-ChildItem -LiteralPath $rustRoot -Recurse -File -Filter "*.rs" | Where-Object {
            $_.Name -ne "lib_old_complex.rs" -and $_.FullName -notlike "*\storage\*"
        }
    )
    $forbiddenPatterns = @(
        '\.join\(\s*"models"\s*\)',
        'WhisperEngine::new\(\)',
        'ParakeetEngine::new\(\)',
        'ModelManager::new\(\)',
        'new_with_models_dir\(\s*None\s*\)'
    )
    foreach ($file in $compiledRustFiles) {
        foreach ($hit in @(Select-String -LiteralPath $file.FullName -Pattern $forbiddenPatterns)) {
            $forbiddenHits.Add("$($hit.Path):$($hit.LineNumber):$($hit.Line.Trim())")
        }
    }
    $forbiddenEvidence = if ($forbiddenHits.Count -eq 0) {
        "no forbidden model path construction or fallback constructor found"
    }
    else {
        $forbiddenHits -join "; "
    }
    Add-Check -Name "compiled_business_code_has_no_model_path_fallback" -Passed ($forbiddenHits.Count -eq 0) -Evidence $forbiddenEvidence

    $libText = [System.IO.File]::ReadAllText((Join-Path $rustRoot "lib.rs"))
    $integrationMarkers = @(
        'storage::resolve_layout_for_app',
        'StorageLayoutState::new',
        'ParallelProcessorState::new',
        'storage::commands::get_storage_status',
        'storage::commands::validate_storage_target'
    )
    $missingIntegrationMarkers = @($integrationMarkers | Where-Object { -not $libText.Contains($_) })
    $integrationPassed = (
        $missingIntegrationMarkers.Count -eq 0 -and
        $libText.IndexOf('storage::resolve_layout_for_app') -lt $libText.IndexOf('whisper_engine::commands::set_models_directory') -and
        $libText.IndexOf('storage::resolve_layout_for_app') -lt $libText.IndexOf('parakeet_engine::commands::set_models_directory')
    )
    $integrationEvidence = if ($missingIntegrationMarkers.Count -eq 0) {
        "required integration markers present and resolver appears first"
    }
    else {
        "missing: $($missingIntegrationMarkers -join ', ')"
    }
    Add-Check -Name "storage_layout_is_initialized_before_model_commands" -Passed $integrationPassed -Evidence $integrationEvidence
    Add-Check -Name "archived_lib_is_not_compiled" -Passed ($libText -notmatch '(?m)^\s*mod\s+lib_old_complex\s*;') -Evidence "lib_old_complex.rs is excluded from the compiled crate module tree"

    $gitOutput = @(& git -C $repositoryFullPath diff --check 2>&1)
    $gitCode = $LASTEXITCODE
    Add-Check -Name "git_diff_check" -Passed ($gitCode -eq 0) -Evidence "exitCode=$gitCode; outputLines=$($gitOutput.Count)"

    Test-Log -Name "storage_path_tests" -Path (Join-Path $evidenceFullPath "S1-path-resolution-tests.txt") -RequiredPatterns @(
        'test result: ok\. 15 passed; 0 failed; 2 ignored',
        'COMMAND_EXIT_CODE:\s*0'
    )
    Test-Log -Name "real_legacy_model_discovery" -Path (Join-Path $evidenceFullPath "S1-legacy-real-model-discovery.txt") -RequiredPatterns @(
        'test result: ok\. 1 passed; 0 failed',
        'COMMAND_EXIT_CODE:\s*0'
    )
    Test-Log -Name "real_target_validation" -Path (Join-Path $evidenceFullPath "S1-real-target-validation.txt") -RequiredPatterns @(
        'test result: ok\. 1 passed; 0 failed',
        'COMMAND_EXIT_CODE:\s*0'
    )
    Test-Log -Name "full_rust_regression" -Path (Join-Path $evidenceFullPath "S1-regression-tests.txt") -RequiredPatterns @(
        'test result: ok\. 463 passed; 0 failed; 8 ignored',
        'COMMAND_EXIT_CODE:\s*0'
    )
    Test-Log -Name "rust_format" -Path (Join-Path $evidenceFullPath "S1-cargo-fmt.txt") -RequiredPatterns @(
        'COMMAND_EXIT_CODE:\s*0'
    )
    Test-Log -Name "frontend_typecheck" -Path (Join-Path $evidenceFullPath "S1-frontend-typecheck.txt") -RequiredPatterns @(
        'COMMAND_EXIT_CODE:\s*0'
    )
    Test-Log -Name "frontend_node_compatible_tests" -Path (Join-Path $evidenceFullPath "S1-frontend-tests.txt") -RequiredPatterns @(
        '(?m)^# pass 138\r?$',
        '(?m)^# fail 0\r?$',
        'COMMAND_EXIT_CODE:\s*0'
    )
}
catch {
    $errors.Add($_.Exception.Message)
}

$allChecksPassed = $errors.Count -eq 0 -and @($checks | Where-Object { -not $_.passed }).Count -eq 0
$result = [ordered]@{
    schemaVersion = 1
    stage = "S1"
    status = if ($allChecksPassed) { "PASS" } else { "FAIL" }
    startedAtUtc = $startedAtUtc.ToString("o")
    finishedAtUtc = [DateTime]::UtcNow.ToString("o")
    command = "pwsh -NoProfile -File scripts/qa/audit-storage-migration-s1.ps1"
    exitCode = if ($allChecksPassed) { 0 } else { 1 }
    sourceAppData = $sourceRoot
    targetRootPlanned = $targetFullPath
    targetRootCreated = (Test-Path -LiteralPath $targetFullPath)
    storagePreferencesCreated = (Test-Path -LiteralPath $preferencesPath)
    sourceModelFileCount = $currentManifest.Count
    sourceModelTotalBytes = [int64](($currentManifest | ForEach-Object { [int64]$_['bytes'] } | Measure-Object -Sum).Sum)
    sourceModels = @($currentManifest)
    blockedProcesses = @($blockedProcesses)
    forbiddenCodeHits = @($forbiddenHits)
    knownNonBlockingTestEnvironmentLimitation = [ordered]@{
        file = "frontend/tests/lib/blocknote-markdown.test.ts"
        reason = "This unrelated test imports bun:test, but Bun is not installed in the current release build environment. Its failed mixed-runtime run is preserved separately and is not counted as a product pass."
        preservedEvidence = (Join-Path $evidenceFullPath "S1-frontend-tests-mixed-runtime-environment-failure.txt")
    }
    checks = @($checks)
    errors = @($errors)
}

[System.IO.Directory]::CreateDirectory($evidenceFullPath) | Out-Null
$resultPath = Join-Path $evidenceFullPath "S1-audit.json"
Write-JsonFile -Path $resultPath -Value $result

if (-not $allChecksPassed) {
    Write-Error "S1 audit failed. See $resultPath"
    exit 1
}

Write-Output "S1 PASS: storage routing, compatibility, target validation, source integrity, and regression evidence passed."
exit 0

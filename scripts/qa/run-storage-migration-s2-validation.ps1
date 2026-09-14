[CmdletBinding()]
param(
    [Parameter()]
    [string]$RepositoryRoot = "D:\桌面\meetlily",

    [Parameter()]
    [string]$SourceAppData = "C:\Users\liuxin\AppData\Roaming\com.meetily.ai",

    [Parameter()]
    [string]$TargetRoot = "E:\MeetilyData",

    [Parameter()]
    [string]$EvidenceDir = "D:\桌面\meetlily\target\release\docs\方案\Meetily统一存储迁移验收证据-20260828",

    [Parameter()]
    [string]$CargoTargetDir = "D:\MeetilyPhase5A4Cargo"
)

$ErrorActionPreference = "Stop"

function Write-Utf8File {
    param(
        [Parameter(Mandatory)] [string]$Path,
        [Parameter(Mandatory)] [string]$Content
    )

    [System.IO.File]::WriteAllText(
        $Path,
        $Content,
        [System.Text.UTF8Encoding]::new($false)
    )
}

function Write-JsonFile {
    param(
        [Parameter(Mandatory)] [string]$Path,
        [Parameter(Mandatory)] $Value
    )

    Write-Utf8File -Path $Path -Content (($Value | ConvertTo-Json -Depth 16) + [Environment]::NewLine)
}

function Invoke-LoggedNative {
    param(
        [Parameter(Mandatory)] [string]$CommandLabel,
        [Parameter(Mandatory)] [string]$Executable,
        [Parameter(Mandatory)] [string[]]$Arguments,
        [Parameter(Mandatory)] [string]$WorkingDirectory,
        [Parameter(Mandatory)] [string]$LogPath,
        [Parameter()] [string[]]$Notes = @()
    )

    $startedAtUtc = [DateTime]::UtcNow
    Push-Location -LiteralPath $WorkingDirectory
    try {
        $output = @(& $Executable @Arguments 2>&1 | ForEach-Object { $_.ToString() })
        $exitCode = $LASTEXITCODE
    }
    finally {
        Pop-Location
    }
    $finishedAtUtc = [DateTime]::UtcNow
    $lines = [System.Collections.Generic.List[string]]::new()
    $lines.Add("COMMAND: $CommandLabel")
    foreach ($note in $Notes) {
        $lines.Add($note)
    }
    $lines.Add("STARTED_AT_UTC: $($startedAtUtc.ToString('o'))")
    foreach ($line in $output) {
        $lines.Add($line)
    }
    $lines.Add("FINISHED_AT_UTC: $($finishedAtUtc.ToString('o'))")
    $lines.Add("COMMAND_EXIT_CODE: $exitCode")
    Write-Utf8File -Path $LogPath -Content (($lines -join [Environment]::NewLine) + [Environment]::NewLine)

    if ($exitCode -ne 0) {
        throw "$CommandLabel failed with exit code $exitCode. See $LogPath"
    }
}

function Merge-TestEvidence {
    param(
        [Parameter(Mandatory)] [string]$OutputName,
        [Parameter(Mandatory)] [string[]]$InputNames,
        [Parameter(Mandatory)] [string]$Purpose,
        [Parameter(Mandatory)] [bool]$FormalTargetBefore,
        [Parameter(Mandatory)] [bool]$FormalPreferencesBefore,
        [Parameter(Mandatory)] [bool]$FormalTargetAfter,
        [Parameter(Mandatory)] [bool]$FormalPreferencesAfter
    )

    $cases = foreach ($name in $InputNames) {
        $path = Join-Path $evidenceFullPath $name
        if (-not (Test-Path -LiteralPath $path -PathType Leaf)) {
            throw "Missing S2 test evidence: $path"
        }
        Get-Content -LiteralPath $path -Raw | ConvertFrom-Json
    }
    $invalid = @($cases | Where-Object {
        $_.status -ne "PASS" -or
        [int]$_.exitCode -ne 0 -or
        -not [bool]$_.temporaryInputOnly -or
        [bool]$_.formalPathsTouched -or
        @($_.errors).Count -ne 0
    })
    $result = [ordered]@{
        schemaVersion = 1
        stage = "S2"
        purpose = $Purpose
        status = if ($invalid.Count -eq 0) { "PASS" } else { "FAIL" }
        startedAtUtc = [DateTime]::UtcNow.ToString("o")
        finishedAtUtc = [DateTime]::UtcNow.ToString("o")
        command = "pwsh -NoProfile -File scripts/qa/run-storage-migration-s2-validation.ps1"
        exitCode = if ($invalid.Count -eq 0) { 0 } else { 1 }
        temporaryInputOnly = $true
        formalTargetExistedBefore = $FormalTargetBefore
        formalPreferencesExistedBefore = $FormalPreferencesBefore
        formalTargetExistsAfter = $FormalTargetAfter
        formalPreferencesExistAfter = $FormalPreferencesAfter
        sourceChanged = $false
        errors = @($invalid | ForEach-Object { "invalid evidence: $($_.testId)" })
        cases = @($cases)
    }
    Write-JsonFile -Path (Join-Path $evidenceFullPath $OutputName) -Value $result
    if ($invalid.Count -ne 0) {
        throw "One or more component tests for $OutputName did not pass."
    }
}

$repositoryFullPath = [System.IO.Path]::GetFullPath($RepositoryRoot)
$frontendRoot = Join-Path $repositoryFullPath "frontend"
$sourceRoot = [System.IO.Path]::GetFullPath($SourceAppData)
$targetFullPath = [System.IO.Path]::GetFullPath($TargetRoot)
$preferencesPath = Join-Path $sourceRoot "storage-preferences.v1.json"
$evidenceFullPath = [System.IO.Path]::GetFullPath($EvidenceDir)
[System.IO.Directory]::CreateDirectory($evidenceFullPath) | Out-Null

$formalTargetBefore = Test-Path -LiteralPath $targetFullPath
$formalPreferencesBefore = Test-Path -LiteralPath $preferencesPath
if ($formalTargetBefore -or $formalPreferencesBefore) {
    throw "S2 cannot run because a formal target or formal storage preferences already exists."
}

$env:LIBCLANG_PATH = "D:\桌面\meetlily\.tools\libclang\clang\native"
$env:CMAKE_GENERATOR = "NMake Makefiles"
$env:CMAKE_MAKE_PROGRAM = "C:\BuildTools\VC\Tools\MSVC\14.44.35207\bin\Hostx64\x64\nmake.exe"
$env:CARGO_TARGET_DIR = [System.IO.Path]::GetFullPath($CargoTargetDir)
$env:MEETILY_S2_EVIDENCE_DIR = $evidenceFullPath

Invoke-LoggedNative `
    -CommandLabel "cargo test -p meetily --test app_lib_tests storage::migration::tests::s2_ -- --test-threads=1" `
    -Executable "cargo" `
    -Arguments @("test", "-p", "meetily", "--test", "app_lib_tests", "storage::migration::tests::s2_", "--", "--test-threads=1") `
    -WorkingDirectory $repositoryFullPath `
    -LogPath (Join-Path $evidenceFullPath "S2-migration-tests.txt")

Invoke-LoggedNative `
    -CommandLabel "cargo test -p meetily --test app_lib_tests -- --test-threads=4" `
    -Executable "cargo" `
    -Arguments @("test", "-p", "meetily", "--test", "app_lib_tests", "--", "--test-threads=4") `
    -WorkingDirectory $repositoryFullPath `
    -LogPath (Join-Path $evidenceFullPath "S2-regression-tests.txt")

Invoke-LoggedNative `
    -CommandLabel "cargo fmt --all -- --check" `
    -Executable "cargo" `
    -Arguments @("fmt", "--all", "--", "--check") `
    -WorkingDirectory $repositoryFullPath `
    -LogPath (Join-Path $evidenceFullPath "S2-cargo-fmt.txt")

Invoke-LoggedNative `
    -CommandLabel "pnpm exec tsc --noEmit --pretty false" `
    -Executable "pnpm" `
    -Arguments @("exec", "tsc", "--noEmit", "--pretty", "false") `
    -WorkingDirectory $frontendRoot `
    -LogPath (Join-Path $evidenceFullPath "S2-frontend-typecheck.txt")

$frontendTests = @(
    Get-ChildItem -LiteralPath (Join-Path $frontendRoot "tests") -Recurse -File |
        Where-Object {
            ($_.Name -like "*.test.ts" -or $_.Name -like "*.test.tsx") -and
            $_.Name -ne "blocknote-markdown.test.ts"
        } |
        Sort-Object FullName |
        ForEach-Object { [System.IO.Path]::GetRelativePath($frontendRoot, $_.FullName) }
)
if ($frontendTests.Count -eq 0) {
    throw "No Node-compatible frontend tests were found."
}
$frontendTestArguments = @("exec", "tsx", "--test") + $frontendTests
Invoke-LoggedNative `
    -CommandLabel "pnpm exec tsx --test <all Node-compatible *.test.ts and *.test.tsx>" `
    -Executable "pnpm" `
    -Arguments $frontendTestArguments `
    -WorkingDirectory $frontendRoot `
    -LogPath (Join-Path $evidenceFullPath "S2-frontend-tests.txt") `
    -Notes @(
        "TEST_FILE_COUNT: $($frontendTests.Count)",
        "RUNTIME_NOTE: blocknote-markdown.test.ts imports bun:test; Bun is not installed, so the established Node-compatible suite excludes only that file."
    )

$formalTargetAfter = Test-Path -LiteralPath $targetFullPath
$formalPreferencesAfter = Test-Path -LiteralPath $preferencesPath
if ($formalTargetAfter -or $formalPreferencesAfter) {
    throw "S2 validation changed a formal target or formal storage preferences path."
}

Merge-TestEvidence `
    -OutputName "S2-interruption-resume.json" `
    -InputNames @("S2-T03-interruption-10.json", "S2-T04-interruption-90.json") `
    -Purpose "10% interruption and 90% abnormal-exit resume" `
    -FormalTargetBefore $formalTargetBefore `
    -FormalPreferencesBefore $formalPreferencesBefore `
    -FormalTargetAfter $formalTargetAfter `
    -FormalPreferencesAfter $formalPreferencesAfter

Merge-TestEvidence `
    -OutputName "S2-hash-corruption.json" `
    -InputNames @(
        "S2-T07-existing-target-corruption.json",
        "S2-T08-partial-corruption.json",
        "S2-T10-source-change.json",
        "S2-T11-ready-target-corruption.json"
    ) `
    -Purpose "existing target, partial, source, and ready-target SHA-256 corruption gates" `
    -FormalTargetBefore $formalTargetBefore `
    -FormalPreferencesBefore $formalPreferencesBefore `
    -FormalTargetAfter $formalTargetAfter `
    -FormalPreferencesAfter $formalPreferencesAfter

Merge-TestEvidence `
    -OutputName "S2-state-corruption.json" `
    -InputNames @("S2-T09-state-corruption.json") `
    -Purpose "invalid JSON, payload hash, schema, false-ready, and manifest hash rejection" `
    -FormalTargetBefore $formalTargetBefore `
    -FormalPreferencesBefore $formalPreferencesBefore `
    -FormalTargetAfter $formalTargetAfter `
    -FormalPreferencesAfter $formalPreferencesAfter

Merge-TestEvidence `
    -OutputName "S2-space-and-write-failures.json" `
    -InputNames @("S2-T05-write-failure.json", "S2-T06-space-failure.json") `
    -Purpose "recoverable target write failure and pre-copy insufficient-space rejection" `
    -FormalTargetBefore $formalTargetBefore `
    -FormalPreferencesBefore $formalPreferencesBefore `
    -FormalTargetAfter $formalTargetAfter `
    -FormalPreferencesAfter $formalPreferencesAfter

Merge-TestEvidence `
    -OutputName "S2-source-integrity.json" `
    -InputNames @(
        "S2-T01-normal-copy.json",
        "S2-T02-repeat.json",
        "S2-T03-interruption-10.json",
        "S2-T04-interruption-90.json",
        "S2-T05-write-failure.json",
        "S2-T07-existing-target-corruption.json",
        "S2-T08-partial-corruption.json",
        "S2-T11-ready-target-corruption.json",
        "S2-T12-preferences-sentinel.json",
        "S2-T13-unicode-long-path.json",
        "S2-T14-source-integrity.json"
    ) `
    -Purpose "temporary source path, byte-count, and SHA-256 preservation" `
    -FormalTargetBefore $formalTargetBefore `
    -FormalPreferencesBefore $formalPreferencesBefore `
    -FormalTargetAfter $formalTargetAfter `
    -FormalPreferencesAfter $formalPreferencesAfter

Write-Output "S2 validation commands passed; evidence is in $evidenceFullPath"

[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)][ValidateSet('Run', 'Cleanup')][string]$Mode,
    [Parameter(Mandatory = $true)][ValidateSet(
        'ft01-03-live-and-persistence',
        'ft04-15-moss-chain',
        'ft16-21-fault-chain',
        'ft22-25-install-lifecycle',
        'ft26-data-drive-placement',
        'ft27-long-audio',
        'ft28-business-chain'
    )][string]$ProducerKey,
    [Parameter(Mandatory = $true)][string]$AcceptanceConfig
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'

$scriptRoot = Split-Path -Parent $MyInvocation.MyCommand.Path
$repoRoot = [System.IO.Path]::GetFullPath((Join-Path $scriptRoot '..\..'))
$cdpScript = Join-Path $scriptRoot 'moss-functional-ft-cdp.mjs'
$evidenceLibrary = Join-Path $scriptRoot 'moss-functional-ft-evidence.ps1'
$processMonitorScript = Join-Path $scriptRoot 'moss-functional-ft-process-monitor.ps1'
$playbackScript = Join-Path $scriptRoot 'play-wav-for-moss-functional-ft.ps1'
$nativeArgumentsScript = Join-Path $scriptRoot 'windows-native-arguments.ps1'
$lifecycleScript = Join-Path $scriptRoot 'run-install-lifecycle.ps1'
$qualityGateScript = Join-Path $scriptRoot 'moss_functional_fix_quality_gate.py'
$producerInputScript = Join-Path $scriptRoot 'new-moss-functional-ft-producer-input.ps1'
$fixedSandboxLauncher = Join-Path $scriptRoot 'invoke-moss-functional-ft-sandbox.ps1'
$obsoleteRunId = 'FT-1B9C29A-20260902-01'
$obsoleteCandidate = '27EB95ED81A6E6DA5C37B33B36639B7BA7D592018DF6065531C6A8221D16381F'
$approvedProductName = 'meetily-p6-lifecycle'
$approvedBundleId = 'com.meetily.ai.p6lifecycle'
$approvedBaselineCommit = '7392eae159443822c80d3675ca9af388e94b2d71'
$isolatedOwnerMarkerName = '.moss-functional-ft-owner.private.json'
$requiredInstalledRoles = [ordered]@{
    main_executable = 'meetily.exe'
    llama_helper = 'llama-helper.exe'
    moss_helper = 'moss-helper.exe'
    ffmpeg = 'ffmpeg.exe'
    directml = 'DirectML.dll'
    webview2 = 'runtime/webview2-fixed/msedgewebview2.exe'
    uninstaller = 'uninstall.exe'
}
$modelContracts = [ordered]@{
    moss = [ordered]@{
        filename = 'MOSS-Transcribe-Diarize-Q8_0.gguf'; relative_path = 'models/moss/MOSS-Transcribe-Diarize-Q8_0.gguf'
        bytes = [int64]986899616; sha256 = '64EC654DC6FFCFDFE180422DFFCE1D33422B0C30959B7EDFD131BAD77EE35039'
    }
    whisper = [ordered]@{
        filename = 'ggml-large-v3-turbo-q5_0.bin'; relative_path = 'models/ggml-large-v3-turbo-q5_0.bin'
        bytes = [int64]574041195; sha256 = '394221709CD5AD1F40C46E6031CA61BCE88931E6E088C188294C6D5A55FFA7E2'
    }
    qwen_2b = [ordered]@{
        filename = 'Qwen3.5-2B-Q4_K_M.gguf'; relative_path = 'models/summary/Qwen3.5-2B-Q4_K_M.gguf'
        bytes = [int64]1280835840; sha256 = 'AAF42C8B7C3CAB2BF3D69C355048D4A0EE9973D48F16C731C0520EE914699223'
    }
}
$runtimeContractBytes = [int64]125
$runtimeContractSha256 = 'C266622EE16A69C458EBA57FD2B2F2114B9C5F2E34292E9603F1CE46C7A1DC73'
$q00Roles = @(
    'bindings', 'window_audio', 'window_manifest', 'moss_raw', 'whisper_same_window', 'corrected',
    'human_verbatim', 'speaker_truth', 'positive_truth', 'negative_truth', 'activation_evidence',
    'summary_evidence', 'public_report', 'private_report'
)
$faultCaseCheckKeys = [ordered]@{
    'FT-16' = @('network_probe_before', 'network_block_effective', 'moss_completed_offline', 'summary_completed_offline', 'network_probe_after', 'product_state_bound', 'residual_helpers_zero')
    'FT-17' = @('summary_helper_observed_before_forced_exit', 'candidate_forced_exit', 'database_reopened', 'interrupted_status_explicit', 'summary_rerun_completed', 'transcript_unchanged', 'residual_helpers_zero')
    'FT-18' = @('network_block_effective', 'protected_file_set_nonempty', 'protected_hashes_unchanged', 'transcript_unchanged', 'summary_unchanged', 'app_remained_running', 'residual_helpers_zero')
    'FT-19' = @('model_corruption_verified', 'moss_failure_explicit', 'app_remained_running', 'protected_hashes_unchanged', 'model_restored_exact', 'error_redacted', 'residual_helpers_zero')
    'FT-20' = @('summary_model_missing_verified', 'summary_failure_explicit', 'transcript_unchanged', 'old_summary_unchanged', 'no_new_completed_summary', 'model_restored_exact', 'residual_helpers_zero')
    'FT-21' = @('moss_helper_observed', 'moss_completed', 'moss_helper_zero_before_summary', 'llama_helper_observed', 'helper_overlap_count_zero', 'summary_completed', 'residual_helpers_zero')
}
$groupCases = [ordered]@{
    'ft01-03-live-and-persistence' = @('FT-01', 'FT-02', 'FT-03')
    'ft04-15-moss-chain' = @('FT-04', 'FT-05', 'FT-06', 'FT-07', 'FT-08', 'FT-09', 'FT-10', 'FT-11', 'FT-12', 'FT-13', 'FT-14', 'FT-15')
    'ft16-21-fault-chain' = @('FT-16', 'FT-17', 'FT-18', 'FT-19', 'FT-20', 'FT-21')
    'ft22-25-install-lifecycle' = @('FT-22', 'FT-23', 'FT-24', 'FT-25')
    'ft26-data-drive-placement' = @('FT-26')
    'ft27-long-audio' = @('FT-27')
    'ft28-business-chain' = @('FT-28')
}

. $nativeArgumentsScript
. $evidenceLibrary

$performanceGateSpecifications = Get-MossPerformanceGateSpecifications

function Get-NormalizedFullPath {
    param([Parameter(Mandatory = $true)][string]$Path)
    $full = [System.IO.Path]::GetFullPath($Path)
    $root = [System.IO.Path]::GetPathRoot($full)
    if ($full.Equals($root, [System.StringComparison]::OrdinalIgnoreCase)) { return $root }
    return $full.TrimEnd('\', '/')
}

function Test-StrictChildPath {
    param([Parameter(Mandatory = $true)][string]$Candidate, [Parameter(Mandatory = $true)][string]$Parent)
    $candidateFull = Get-NormalizedFullPath $Candidate
    $parentFull = Get-NormalizedFullPath $Parent
    return $candidateFull.StartsWith(
        $parentFull.TrimEnd('\', '/') + [System.IO.Path]::DirectorySeparatorChar,
        [System.StringComparison]::OrdinalIgnoreCase
    )
}

function Test-PathsOverlap {
    param([Parameter(Mandatory = $true)][string]$Left, [Parameter(Mandatory = $true)][string]$Right)
    $leftFull = Get-NormalizedFullPath $Left
    $rightFull = Get-NormalizedFullPath $Right
    return $leftFull.Equals($rightFull, [System.StringComparison]::OrdinalIgnoreCase) -or
        (Test-StrictChildPath -Candidate $leftFull -Parent $rightFull) -or
        (Test-StrictChildPath -Candidate $rightFull -Parent $leftFull)
}

function Assert-NoReparsePath {
    param(
        [Parameter(Mandatory = $true)][string]$Path,
        [Parameter(Mandatory = $true)][string]$Boundary,
        [switch]$AllowMissingLeaf
    )
    $full = Get-NormalizedFullPath $Path
    $root = Get-NormalizedFullPath $Boundary
    if (-not $full.Equals($root, [System.StringComparison]::OrdinalIgnoreCase) -and
        -not (Test-StrictChildPath -Candidate $full -Parent $root)) {
        throw "Path is outside its approved boundary: $full"
    }
    $cursor = $full
    $first = $true
    while ($true) {
        if (Test-Path -LiteralPath $cursor) {
            $item = Get-Item -LiteralPath $cursor -Force
            if (($item.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0) {
                throw "Path contains a reparse point: $cursor"
            }
        } elseif (-not ($first -and $AllowMissingLeaf)) {
            throw "Path component is missing: $cursor"
        }
        if ($cursor.Equals($root, [System.StringComparison]::OrdinalIgnoreCase)) { break }
        $parent = [System.IO.Path]::GetDirectoryName($cursor)
        if ([string]::IsNullOrWhiteSpace($parent) -or $parent.Equals($cursor, [System.StringComparison]::OrdinalIgnoreCase)) {
            throw "Could not reach approved boundary: $root"
        }
        $cursor = Get-NormalizedFullPath $parent
        $first = $false
    }
    return $full
}

function Resolve-SafeRelativePath {
    param([Parameter(Mandatory = $true)][string]$Root, [Parameter(Mandatory = $true)][string]$RelativePath)
    if ([string]::IsNullOrWhiteSpace($RelativePath) -or [System.IO.Path]::IsPathRooted($RelativePath)) {
        throw "Evidence path must be relative: $RelativePath"
    }
    $segments = @($RelativePath.Replace('/', '\').Split('\'))
    if (@($segments | Where-Object { $_ -in @('', '.', '..') }).Count -ne 0) {
        throw "Evidence path contains an unsafe segment: $RelativePath"
    }
    $resolved = Get-NormalizedFullPath (Join-Path $Root $RelativePath)
    if (-not (Test-StrictChildPath -Candidate $resolved -Parent $Root)) {
        throw "Evidence path escapes its root: $RelativePath"
    }
    return $resolved
}

function Get-FileRecord {
    param([Parameter(Mandatory = $true)][string]$Path)
    $full = Get-NormalizedFullPath $Path
    Assert-NoReparsePath -Path $full -Boundary ([System.IO.Path]::GetPathRoot($full)) | Out-Null
    if (-not (Test-Path -LiteralPath $full -PathType Leaf)) { throw "Required file is missing: $full" }
    $before = Get-Item -LiteralPath $full -Force
    $sha = (Get-FileHash -LiteralPath $full -Algorithm SHA256).Hash.ToUpperInvariant()
    $after = Get-Item -LiteralPath $full -Force
    if ([int64]$before.Length -ne [int64]$after.Length -or $before.LastWriteTimeUtc.Ticks -ne $after.LastWriteTimeUtc.Ticks) {
        throw "File changed while being hashed: $full"
    }
    return [ordered]@{ path = $full; bytes = [int64]$after.Length; sha256 = $sha }
}

function Assert-BoundFile {
    param([Parameter(Mandatory = $true)]$Record, [Parameter(Mandatory = $true)][string]$Label)
    if ($null -eq $Record -or [string]::IsNullOrWhiteSpace([string]$Record.path)) { throw "$Label path is missing." }
    $actual = Get-FileRecord ([string]$Record.path)
    if ([int64]$Record.bytes -ne [int64]$actual.bytes -or
        -not ([string]$Record.sha256).Equals([string]$actual.sha256, [System.StringComparison]::OrdinalIgnoreCase)) {
        throw "$Label bytes or SHA-256 do not match the real file."
    }
    return $actual
}

function Assert-ExactProperties {
    param($Object, [Parameter(Mandatory = $true)][string[]]$Names, [Parameter(Mandatory = $true)][string]$Label)
    if ($null -eq $Object -or $Object -is [System.Array]) { throw "$Label must be an object." }
    $actual = @($Object.PSObject.Properties.Name | Sort-Object)
    $expected = @($Names | Sort-Object)
    if (($actual -join '|') -ne ($expected -join '|')) {
        throw "$Label fields are not exact. actual=$($actual -join ',') expected=$($expected -join ',')"
    }
}

function Assert-NonemptyUniqueStrings {
    param($Value, [Parameter(Mandatory = $true)][string]$Label)
    $items = @($Value)
    if ($items.Count -eq 0) { throw "$Label must not be empty." }
    $strings = @()
    foreach ($item in $items) {
        $text = [string]$item
        if ([string]::IsNullOrWhiteSpace($text) -or $text -match '[\r\n\x00]') { throw "$Label contains an invalid value." }
        $strings += $text
    }
    if (@($strings | Sort-Object -Unique).Count -ne $strings.Count) { throw "$Label contains duplicates." }
    return $strings
}

function Assert-CdpPort {
    param($Value, [Parameter(Mandatory = $true)][string]$Label)
    $port = [int]$Value
    if ($port -lt 1024 -or $port -gt 65535) { throw "$Label must be between 1024 and 65535." }
    return $port
}

function Assert-SafeRelativeFileName {
    param([Parameter(Mandatory = $true)][string]$RelativePath, [Parameter(Mandatory = $true)][string]$Label)
    if ([System.IO.Path]::IsPathRooted($RelativePath)) { throw "$Label path must be relative." }
    $segments = @($RelativePath.Replace('/', '\').Split('\'))
    if (@($segments | Where-Object { $_ -in @('', '.', '..') }).Count -ne 0) { throw "$Label contains an unsafe path." }
    return ($segments -join '/')
}

function Assert-BoundRuntimePackage {
    param($Runtime)
    Assert-ExactProperties $Runtime @('root', 'contract', 'files') 'MOSS runtime binding'
    $root = Get-NormalizedFullPath ([string]$Runtime.root)
    Assert-NoReparsePath -Path $root -Boundary ([System.IO.Path]::GetPathRoot($root)) | Out-Null
    if (-not (Test-Path -LiteralPath $root -PathType Container)) { throw 'MOSS runtime root is missing.' }
    $contract = Assert-BoundFile $Runtime.contract 'MOSS runtime contract'
    if (-not $contract.path.Equals((Get-NormalizedFullPath (Join-Path $root 'contract.json')), [System.StringComparison]::OrdinalIgnoreCase) -or
        [int64]$contract.bytes -ne $runtimeContractBytes -or
        -not $contract.sha256.Equals($runtimeContractSha256, [System.StringComparison]::OrdinalIgnoreCase)) {
        throw 'MOSS runtime contract is not the approved v0.2.2 package.'
    }
    $declared = @($Runtime.files)
    if ($declared.Count -lt 4) { throw 'MOSS runtime package is incomplete.' }
    $seen = @{}
    foreach ($row in $declared) {
        Assert-ExactProperties $row @('relative_path', 'bytes', 'sha256') 'MOSS runtime file record'
        $relative = Assert-SafeRelativeFileName ([string]$row.relative_path) 'MOSS runtime file'
        if ($seen.ContainsKey($relative)) { throw "Duplicate MOSS runtime file: $relative" }
        $actual = Get-FileRecord (Join-Path $root $relative.Replace('/', '\'))
        if ([int64]$row.bytes -ne [int64]$actual.bytes -or
            -not ([string]$row.sha256).Equals($actual.sha256, [System.StringComparison]::OrdinalIgnoreCase)) {
            throw "MOSS runtime file binding is stale: $relative"
        }
        $seen[$relative] = $actual
    }
    $actualFiles = @(Get-ChildItem -LiteralPath $root -File -Recurse -Force)
    if ($actualFiles.Count -ne $declared.Count) { throw 'MOSS runtime binding does not cover the exact directory.' }
    return [ordered]@{ root = $root; contract = $contract; files = $declared }
}

function Assert-PrivateGroupDirectory {
    param([Parameter(Mandatory = $true)][string]$Path, [Parameter(Mandatory = $true)][string]$PrivateRoot, [string]$Label)
    $full = Get-NormalizedFullPath $Path
    if (-not (Test-StrictChildPath -Candidate $full -Parent $PrivateRoot)) { throw "$Label must be below the private evidence root." }
    if (-not (Test-Path -LiteralPath $full -PathType Container)) { throw "$Label directory is missing." }
    Assert-NoReparsePath -Path $full -Boundary $PrivateRoot | Out-Null
    return $full
}

function Read-JsonObject {
    param([Parameter(Mandatory = $true)][string]$Path, [Parameter(Mandatory = $true)][string]$Label)
    $record = Get-FileRecord $Path
    try { $value = Get-Content -LiteralPath $record.path -Raw -Encoding UTF8 | ConvertFrom-Json }
    catch { throw "$Label is not valid JSON: $($_.Exception.Message)" }
    if ($null -eq $value -or $value -is [System.Array]) { throw "$Label must be a JSON object." }
    return [ordered]@{ value = $value; file = $record }
}

function Write-JsonExclusive {
    param([Parameter(Mandatory = $true)]$Value, [Parameter(Mandatory = $true)][string]$Path)
    $full = Get-NormalizedFullPath $Path
    if (Test-Path -LiteralPath $full) { throw "Refusing to overwrite existing evidence: $full" }
    $parent = Split-Path -Parent $full
    if (-not (Test-Path -LiteralPath $parent -PathType Container)) {
        [System.IO.Directory]::CreateDirectory($parent) | Out-Null
    }
    Assert-NoReparsePath -Path $parent -Boundary ([System.IO.Path]::GetPathRoot($parent)) | Out-Null
    $json = ($Value | ConvertTo-Json -Depth 80) + [Environment]::NewLine
    $bytes = [System.Text.UTF8Encoding]::new($false).GetBytes($json)
    $stream = [System.IO.File]::Open($full, [System.IO.FileMode]::CreateNew, [System.IO.FileAccess]::Write, [System.IO.FileShare]::None)
    try { $stream.Write($bytes, 0, $bytes.Length); $stream.Flush($true) } finally { $stream.Dispose() }
}

function Write-JsonAtomic {
    param([Parameter(Mandatory = $true)]$Value, [Parameter(Mandatory = $true)][string]$Path)
    $full = Get-NormalizedFullPath $Path
    $parent = Split-Path -Parent $full
    if (-not (Test-Path -LiteralPath $parent -PathType Container)) { [System.IO.Directory]::CreateDirectory($parent) | Out-Null }
    Assert-NoReparsePath -Path $parent -Boundary ([System.IO.Path]::GetPathRoot($parent)) | Out-Null
    $temporary = Join-Path $parent ('.' + [System.IO.Path]::GetFileName($full) + '.' + [Guid]::NewGuid().ToString('N') + '.tmp')
    [System.IO.File]::WriteAllText($temporary, (($Value | ConvertTo-Json -Depth 80) + [Environment]::NewLine), [System.Text.UTF8Encoding]::new($false))
    Move-Item -LiteralPath $temporary -Destination $full -Force
}

function Get-PropertyValue {
    param($Object, [Parameter(Mandatory = $true)][string[]]$Names)
    if ($null -eq $Object) { return $null }
    foreach ($name in $Names) {
        $property = $Object.PSObject.Properties[$name]
        if ($null -ne $property) { return $property.Value }
    }
    return $null
}

function Test-AllTrue {
    param([Parameter(Mandatory = $true)]$Checks)
    $items = @($Checks.PSObject.Properties | ForEach-Object { [bool]$_.Value })
    return $items.Count -gt 0 -and @($items | Where-Object { -not $_ }).Count -eq 0
}

function Get-TextSha256 {
    param([Parameter(Mandatory = $true)][string]$Value)
    $algorithm = [System.Security.Cryptography.SHA256]::Create()
    try { return ([BitConverter]::ToString($algorithm.ComputeHash([System.Text.Encoding]::UTF8.GetBytes($Value)))).Replace('-', '') }
    finally { $algorithm.Dispose() }
}

function Get-HostFirewallSnapshot {
    $profiles = @(
        Get-NetFirewallProfile -ErrorAction Stop | Sort-Object Name | Select-Object `
            Name, Enabled, DefaultInboundAction, DefaultOutboundAction, AllowInboundRules, AllowLocalFirewallRules,
            AllowLocalIPsecRules, AllowUnicastResponseToMulticast, NotifyOnListen, LogAllowed, LogBlocked
    )
    $rules = @(
        Get-NetFirewallRule -PolicyStore ActiveStore -ErrorAction Stop | Sort-Object Name | Select-Object `
            Name, DisplayName, Enabled, Direction, Action, Profile, PolicyStoreSourceType
    )
    $payload = [ordered]@{ profiles = $profiles; rules = $rules }
    $json = $payload | ConvertTo-Json -Depth 12 -Compress
    return [ordered]@{
        captured_at = [datetimeoffset]::UtcNow.ToString('o')
        profile_count = $profiles.Count
        rule_count = $rules.Count
        sha256 = Get-TextSha256 $json
        payload = $payload
    }
}

function Invoke-NativeCapture {
    param(
        [Parameter(Mandatory = $true)][string]$Executable,
        [string[]]$Arguments = @(),
        [Parameter(Mandatory = $true)][string]$Label,
        [int]$TimeoutSeconds = 600,
        [switch]$AllowFailure,
        [hashtable]$Environment = @{}
    )
    $record = Get-FileRecord $Executable
    $start = [System.Diagnostics.ProcessStartInfo]::new()
    $start.FileName = $record.path
    $start.Arguments = (($Arguments | ForEach-Object { ConvertTo-NativeArgument -Value ([string]$_) }) -join ' ')
    $start.WorkingDirectory = $repoRoot
    $start.UseShellExecute = $false
    $start.CreateNoWindow = $true
    $start.RedirectStandardOutput = $true
    $start.RedirectStandardError = $true
    foreach ($entry in $Environment.GetEnumerator()) { $start.EnvironmentVariables[[string]$entry.Key] = [string]$entry.Value }
    $process = [System.Diagnostics.Process]::new()
    $process.StartInfo = $start
    $startedAt = [datetimeoffset]::UtcNow
    if (-not $process.Start()) { throw "$Label did not start." }
    $processId = $process.Id
    $stdoutTask = $process.StandardOutput.ReadToEndAsync()
    $stderrTask = $process.StandardError.ReadToEndAsync()
    if (-not $process.WaitForExit($TimeoutSeconds * 1000)) {
        & taskkill.exe /PID $processId /T /F 2>$null | Out-Null
        try { $process.WaitForExit(10000) | Out-Null } catch {}
        throw "$Label timed out after $TimeoutSeconds seconds."
    }
    $stdout = $stdoutTask.GetAwaiter().GetResult()
    $stderr = $stderrTask.GetAwaiter().GetResult()
    $result = [ordered]@{
        label = $Label
        executable = $record
        arguments = @($Arguments)
        pid = $processId
        started_at = $startedAt.ToString('o')
        completed_at = [datetimeoffset]::UtcNow.ToString('o')
        exit_code = [int]$process.ExitCode
        stdout = $stdout
        stderr = $stderr
    }
    if (-not $AllowFailure -and $result.exit_code -ne 0) { throw "$Label exited with code $($result.exit_code)." }
    return $result
}

function Assert-ExactScenarioMap {
    param($Document)
    $allIds = 1..28 | ForEach-Object { 'FT-{0:D2}' -f $_ }
    $scenarios = @($Document.scenarios)
    if ($scenarios.Count -ne 28) { throw 'Acceptance config must contain exactly FT-01 through FT-28.' }
    $actualIds = @($scenarios | ForEach-Object { [string]$_.id })
    if (($actualIds -join '|') -ne ($allIds -join '|')) { throw 'Acceptance scenario IDs are missing, duplicated, or out of order.' }
    foreach ($entry in $groupCases.GetEnumerator()) {
        $actual = @($scenarios | Where-Object { [string]$_.run_once_key -eq [string]$entry.Key } | ForEach-Object { [string]$_.id })
        if (($actual -join '|') -ne (@($entry.Value) -join '|')) { throw "Acceptance group mapping is wrong for $($entry.Key)." }
    }
}

function Read-ProducerContext {
    $configPath = Get-NormalizedFullPath $AcceptanceConfig
    Assert-NoReparsePath -Path $configPath -Boundary ([System.IO.Path]::GetPathRoot($configPath)) | Out-Null
    if (Test-StrictChildPath -Candidate $configPath -Parent $repoRoot) { throw 'Materialized acceptance.json must be outside the repository.' }
    $configRead = Read-JsonObject -Path $configPath -Label 'acceptance config'
    $config = $configRead.value
    if ([int]$config.schema_version -ne 1 -or [string]$config.stage -ne 'MOSS_FUNCTIONAL_FIX_ACCEPTANCE_CONFIG' -or [bool]$config.template_only) {
        throw 'Acceptance config schema/stage is invalid or still a template.'
    }
    $serialized = $config | ConvertTo-Json -Depth 100 -Compress
    if ($serialized -match '__[A-Z][A-Z0-9_]*__|\bTODO\b|\bTBD\b') {
        throw 'Acceptance config contains a placeholder or obsolete worktree marker.'
    }
    $runId = [string]$config.run_id
    if ($runId -eq $obsoleteRunId -or $runId -notmatch '^[A-Za-z0-9][A-Za-z0-9._-]{0,127}$') { throw 'Acceptance run_id is obsolete or unsafe.' }
    $sourceCommit = ([string]$config.source_commit).ToLowerInvariant()
    if ($sourceCommit -notmatch '^[0-9a-f]{40}$') { throw 'Acceptance source_commit is invalid.' }
    $head = (& git -C $repoRoot rev-parse --verify HEAD 2>$null).Trim().ToLowerInvariant()
    if ($LASTEXITCODE -ne 0 -or $head -ne $sourceCommit) { throw 'Acceptance source_commit does not match repository HEAD.' }
    $candidate = Assert-BoundFile -Record $config.candidate -Label 'candidate installer'
    if ($candidate.sha256 -eq $obsoleteCandidate) { throw 'Obsolete candidate installer is forbidden.' }
    $buildManifestRecord = Assert-BoundFile -Record $config.build_manifest -Label 'candidate build manifest'
    $buildRead = Read-JsonObject -Path $buildManifestRecord.path -Label 'candidate build manifest'
    $build = $buildRead.value
    if ([string]$build.role -ne 'candidate' -or ([string]$build.source_commit).ToLowerInvariant() -ne $sourceCommit -or
        [string]$build.product_name -ne $approvedProductName -or [string]$build.bundle_id -ne $approvedBundleId -or
        -not ([string]$build.installer.sha256).Equals($candidate.sha256, [System.StringComparison]::OrdinalIgnoreCase)) {
        throw 'Candidate build manifest is not bound to this source, candidate, or isolated identity.'
    }
    $installed = @($build.installed_files)
    if ($installed.Count -ne $requiredInstalledRoles.Count) { throw 'Candidate build manifest must contain exactly seven installed roles.' }
    foreach ($role in $requiredInstalledRoles.Keys) {
        $matches = @($installed | Where-Object { [string]$_.role -eq $role })
        if ($matches.Count -ne 1 -or [string]$matches[0].relative_path -ne [string]$requiredInstalledRoles[$role] -or
            [string]$matches[0].sha256 -notmatch '^[0-9A-Fa-f]{64}$') {
            throw "Candidate build manifest role/path is invalid: $role"
        }
    }
    Assert-ExactScenarioMap $config
    $publicRoot = Get-NormalizedFullPath ([string]$config.evidence_roots.public)
    $privateRoot = Get-NormalizedFullPath ([string]$config.evidence_roots.private)
    foreach ($root in @($publicRoot, $privateRoot)) {
        Assert-NoReparsePath -Path $root -Boundary ([System.IO.Path]::GetPathRoot($root)) | Out-Null
        if (-not (Test-Path -LiteralPath $root -PathType Container)) { throw "Evidence root is missing: $root" }
    }
    if (Test-PathsOverlap -Left $publicRoot -Right $privateRoot) { throw 'Public and private evidence roots must be separate and non-nested.' }
    foreach ($scenario in @($config.scenarios)) {
        if (([string]$scenario.source_commit).ToLowerInvariant() -ne $sourceCommit -or
            -not ([string]$scenario.candidate_sha256).Equals($candidate.sha256, [System.StringComparison]::OrdinalIgnoreCase)) {
            throw "Scenario $($scenario.id) is bound to another source or candidate."
        }
        foreach ($input in @($scenario.inputs)) { Assert-BoundFile -Record $input -Label "$($scenario.id) input" | Out-Null }
        foreach ($kind in @('public', 'private')) {
            $root = if ($kind -eq 'public') { $publicRoot } else { $privateRoot }
            foreach ($relative in @($scenario.evidence.$kind)) {
                Resolve-SafeRelativePath -Root $root -RelativePath ([string]$relative) | Out-Null
            }
        }
    }
    $sidecarPath = [System.IO.Path]::ChangeExtension($configPath, '.producer.private.json')
    if (-not (Test-Path -LiteralPath $sidecarPath -PathType Leaf)) { throw 'Producer private input is missing.' }
    $sidecarRead = Read-JsonObject -Path $sidecarPath -Label 'producer private input'
    $sidecar = $sidecarRead.value
    Assert-ExactProperties $sidecar @(
        'schema_version', 'stage', 'created_at', 'run_id', 'source_commit', 'candidate_sha256',
        'build_manifest_sha256', 'acceptance_sha256', 'product_name', 'bundle_id', 'version',
        'producer', 'values', 'python', 'models', 'moss_runtime', 'groups'
    ) 'producer private input'
    if ([int]$sidecar.schema_version -ne 1 -or [string]$sidecar.stage -ne 'MOSS_FUNCTIONAL_FT_PRODUCER_INPUT') {
        throw 'Producer private input schema/stage is invalid.'
    }
    if ([string]$sidecar.run_id -ne $runId -or ([string]$sidecar.source_commit).ToLowerInvariant() -ne $sourceCommit -or
        -not ([string]$sidecar.candidate_sha256).Equals($candidate.sha256, [System.StringComparison]::OrdinalIgnoreCase) -or
        -not ([string]$sidecar.build_manifest_sha256).Equals($buildManifestRecord.sha256, [System.StringComparison]::OrdinalIgnoreCase) -or
        -not ([string]$sidecar.acceptance_sha256).Equals($configRead.file.sha256, [System.StringComparison]::OrdinalIgnoreCase)) {
        throw 'Producer private input is bound to another acceptance run.'
    }
    if ([string]$sidecar.product_name -ne $approvedProductName -or [string]$sidecar.bundle_id -ne $approvedBundleId -or
        [string]$sidecar.version -ne [string]$build.version) { throw 'Producer private input identity does not match the build manifest.' }
    $producer = Assert-BoundFile $sidecar.producer 'producer input generator'
    if (-not $producer.path.Equals((Get-NormalizedFullPath $producerInputScript), [System.StringComparison]::OrdinalIgnoreCase)) {
        throw 'Producer private input was not made by the fixed repository generator.'
    }
    Assert-BoundFile $sidecar.values 'producer values' | Out-Null
    Assert-BoundFile $sidecar.python 'Python runtime' | Out-Null
    Assert-ExactProperties $sidecar.groups @($groupCases.Keys) 'producer private input groups'
    $groupInput = $sidecar.groups.PSObject.Properties[$ProducerKey]
    if ($null -eq $groupInput -or $null -eq $groupInput.Value) { throw "Producer private input lacks group $ProducerKey." }
    $stateRoot = Join-Path $privateRoot '_producer'
    if (-not (Test-Path -LiteralPath $stateRoot -PathType Container)) { [System.IO.Directory]::CreateDirectory($stateRoot) | Out-Null }
    Assert-NoReparsePath -Path $stateRoot -Boundary $privateRoot | Out-Null
    return [ordered]@{
        config_path = $configPath
        config_record = $configRead.file
        config = $config
        run_id = $runId
        source_commit = $sourceCommit
        candidate = $candidate
        build_manifest_record = $buildManifestRecord
        build_manifest = $build
        public_root = $publicRoot
        private_root = $privateRoot
        sidecar_path = $sidecarPath
        sidecar_record = $sidecarRead.file
        sidecar = $sidecar
        group_input = $groupInput.Value
        state_root = $stateRoot
        state_path = Join-Path $stateRoot ($ProducerKey + '.state.private.json')
        lease_path = Join-Path $stateRoot ($ProducerKey + '.run.private.json')
    }
}

function Assert-ProducerRunInputs {
    param($Context)
    $sidecar = $Context.sidecar
    Assert-ExactProperties $sidecar.models @('moss', 'whisper', 'qwen_2b') 'producer model bindings'
    foreach ($role in $modelContracts.Keys) {
        $record = Assert-BoundFile $sidecar.models.$role "formal $role model"
        $contract = $modelContracts[$role]
        if (-not [System.IO.Path]::GetFileName($record.path).Equals([string]$contract.filename, [System.StringComparison]::OrdinalIgnoreCase) -or
            [int64]$record.bytes -ne [int64]$contract.bytes -or
            -not $record.sha256.Equals([string]$contract.sha256, [System.StringComparison]::OrdinalIgnoreCase)) {
            throw "Formal model binding is not exact: $role"
        }
    }
    Assert-BoundRuntimePackage $sidecar.moss_runtime | Out-Null

    $groups = $sidecar.groups
    $g01 = $groups.'ft01-03-live-and-persistence'
    $g04 = $groups.'ft04-15-moss-chain'
    $g16 = $groups.'ft16-21-fault-chain'
    $g22 = $groups.'ft22-25-install-lifecycle'
    $g26 = $groups.'ft26-data-drive-placement'
    $g27 = $groups.'ft27-long-audio'
    $g28 = $groups.'ft28-business-chain'
    Assert-ExactProperties $g01 @('cdp_port', 'recording_root', 'whisper_model', 'qwen_model', 'short_duration_seconds') 'FT-01 producer input'
    Assert-ExactProperties $g04 @('cdp_port', 'positive_terms', 'negative_terms') 'FT-04 producer input'
    Assert-ExactProperties $g16 @('sandbox_launcher', 'sandbox_exchange_root') 'FT-16 producer input'
    Assert-ExactProperties $g22 @('baseline_build_manifest', 'protected_baseline_manifest', 'fixture_database') 'FT-22 producer input'
    Assert-ExactProperties $g26 @('cdp_port', 'selected_recording_root', 'default_recording_root', 'invalid_recording_root', 'short_duration_seconds') 'FT-26 producer input'
    Assert-ExactProperties $g27 @('cdp_port', 'expected_duration_seconds') 'FT-27 producer input'
    Assert-ExactProperties $g28 @('cdp_port', 'expected_duration_seconds', 'positive_terms', 'negative_terms', 'q00') 'FT-28 producer input'

    $ports = @(
        Assert-CdpPort $g01.cdp_port 'FT-01 cdp_port'
        Assert-CdpPort $g04.cdp_port 'FT-04 cdp_port'
        Assert-CdpPort $g26.cdp_port 'FT-26 cdp_port'
        Assert-CdpPort $g27.cdp_port 'FT-27 cdp_port'
        Assert-CdpPort $g28.cdp_port 'FT-28 cdp_port'
    )
    if (@($ports | Sort-Object -Unique).Count -ne $ports.Count) { throw 'Every app-running FT group must use a distinct CDP port.' }
    if ([string]$g01.whisper_model -ne 'large-v3-turbo-q5_0' -or [string]$g01.qwen_model -ne 'qwen3.5:2b') {
        throw 'FT-01 model selections do not match the bound model files.'
    }
    if ([double]$g01.short_duration_seconds -le 0 -or [double]$g26.short_duration_seconds -le 0) { throw 'Short input durations must be positive.' }
    if ([math]::Abs([double]$g27.expected_duration_seconds - 3096.62) -gt 0.000001) { throw 'FT-27 duration must be exactly 3096.62 seconds.' }
    if ([math]::Abs([double]$g28.expected_duration_seconds - 737.728) -gt 0.000001) { throw 'FT-28 duration must be exactly 737.728 seconds.' }
    Assert-NonemptyUniqueStrings $g04.positive_terms 'FT-04 positive terms' | Out-Null
    Assert-NonemptyUniqueStrings $g04.negative_terms 'FT-04 negative terms' | Out-Null
    Assert-NonemptyUniqueStrings $g28.positive_terms 'FT-28 positive terms' | Out-Null
    Assert-NonemptyUniqueStrings $g28.negative_terms 'FT-28 negative terms' | Out-Null

    Assert-PrivateGroupDirectory ([string]$g01.recording_root) $Context.private_root 'FT-01 recording root' | Out-Null
    Assert-PrivateGroupDirectory ([string]$g16.sandbox_exchange_root) $Context.private_root 'FT-16 sandbox exchange root' | Out-Null
    Assert-PrivateGroupDirectory ([string]$g26.selected_recording_root) $Context.private_root 'FT-26 selected root' | Out-Null
    Assert-PrivateGroupDirectory ([string]$g26.default_recording_root) $Context.private_root 'FT-26 default root' | Out-Null
    $invalidRoot = Get-NormalizedFullPath ([string]$g26.invalid_recording_root)
    if (-not (Test-StrictChildPath -Candidate $invalidRoot -Parent $Context.private_root) -or (Test-Path -LiteralPath $invalidRoot)) {
        throw 'FT-26 invalid recording root must be a missing private-evidence child.'
    }
    Assert-NoReparsePath -Path (Split-Path -Parent $invalidRoot) -Boundary $Context.private_root | Out-Null

    $launcher = Assert-BoundFile $g16.sandbox_launcher 'fixed Windows Sandbox launcher'
    if (-not $launcher.path.Equals((Get-NormalizedFullPath $fixedSandboxLauncher), [System.StringComparison]::OrdinalIgnoreCase)) {
        throw 'FT-16 through FT-21 must use the fixed repository Sandbox launcher.'
    }
    foreach ($name in @('baseline_build_manifest', 'protected_baseline_manifest', 'fixture_database')) {
        Assert-BoundFile $g22.$name "FT-22 $name" | Out-Null
    }
    Assert-ExactProperties $g28.q00 $q00Roles 'FT-28 Q00 bindings'
    foreach ($role in $q00Roles) { Assert-BoundFile $g28.q00.$role "FT-28 Q00 $role" | Out-Null }
}

function New-InitialState {
    param($Context)
    $installRoot = Get-NormalizedFullPath (Join-Path $env:LOCALAPPDATA $approvedProductName)
    $dataRoot = Get-NormalizedFullPath (Join-Path $env:APPDATA $approvedBundleId)
    $webviewRoot = Get-NormalizedFullPath (Join-Path $env:LOCALAPPDATA $approvedBundleId)
    $backupRoot = $dataRoot + '.rollback-backups'
    foreach ($item in @($installRoot, $webviewRoot)) {
        if (-not (Test-StrictChildPath -Candidate $item -Parent $env:LOCALAPPDATA)) { throw 'Isolated LOCALAPPDATA path contract failed.' }
    }
    foreach ($item in @($dataRoot, $backupRoot)) {
        if (-not (Test-StrictChildPath -Candidate $item -Parent $env:APPDATA)) { throw 'Isolated APPDATA path contract failed.' }
    }
    return [ordered]@{
        schema_version = 1
        stage = 'MOSS_FUNCTIONAL_FT_STATE'
        run_id = $Context.run_id
        source_commit = $Context.source_commit
        candidate_sha256 = $Context.candidate.sha256
        build_manifest_sha256 = $Context.build_manifest_record.sha256
        producer_key = $ProducerKey
        created_at = [datetimeoffset]::UtcNow.ToString('o')
        install_root = $installRoot
        data_root = $dataRoot
        webview_root = $webviewRoot
        backup_root = $backupRoot
        registry_path = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\' + $approvedProductName
        app_pid = $null
        app_started_at = $null
        cdp_port = $null
        cdp_target_id = $null
        recorded_processes = @()
        isolated_roots_owned = $false
        run_completed = $false
        run_passed = $false
    }
}

function Read-State {
    param($Context)
    $read = Read-JsonObject -Path $Context.state_path -Label 'producer state'
    $state = $read.value
    if ([string]$state.stage -ne 'MOSS_FUNCTIONAL_FT_STATE' -or [string]$state.run_id -ne $Context.run_id -or
        ([string]$state.source_commit).ToLowerInvariant() -ne $Context.source_commit -or
        -not ([string]$state.candidate_sha256).Equals($Context.candidate.sha256, [System.StringComparison]::OrdinalIgnoreCase) -or
        [string]$state.producer_key -ne $ProducerKey) { throw 'Producer state belongs to another run or group.' }
    return $state
}

function Save-State {
    param($Context, $State)
    Write-JsonAtomic -Value $State -Path $Context.state_path
}

function Assert-NoFormalEvidenceExists {
    param($Context)
    foreach ($caseId in @($groupCases[$ProducerKey])) {
        foreach ($pair in @(
            @{ root = $Context.public_root; relative = "$caseId/result.public.json" },
            @{ root = $Context.private_root; relative = "$caseId/result.private.json" }
        )) {
            $path = Resolve-SafeRelativePath -Root $pair.root -RelativePath $pair.relative
            if (Test-Path -LiteralPath $path) { throw "Formal evidence already exists; use a new run_id: $($pair.relative)" }
        }
    }
    foreach ($pair in @(
        @{ root = $Context.public_root; relative = "cleanup/$ProducerKey.public.json" },
        @{ root = $Context.private_root; relative = "cleanup/$ProducerKey.private.json" }
    )) {
        $path = Resolve-SafeRelativePath -Root $pair.root -RelativePath $pair.relative
        if (Test-Path -LiteralPath $path) { throw "Cleanup evidence already exists; use a new run_id: $($pair.relative)" }
    }
    if (Test-Path -LiteralPath $Context.lease_path) { throw 'This producer group already has a run lease.' }
}

function Get-InstalledFilePath {
    param($State, [Parameter(Mandatory = $true)][string]$Role)
    return Get-NormalizedFullPath (Join-Path ([string]$State.install_root) ([string]$requiredInstalledRoles[$Role]))
}

function Get-InstalledRoleSha256 {
    param($Context, [Parameter(Mandatory = $true)][string]$Role)
    $matches = @($Context.build_manifest.installed_files | Where-Object { [string]$_.role -eq $Role })
    if ($matches.Count -ne 1 -or [string]$matches[0].sha256 -notmatch '^[0-9A-Fa-f]{64}$') {
        throw "Installed role hash is not uniquely bound: $Role"
    }
    return ([string]$matches[0].sha256).ToUpperInvariant()
}

function Assert-InstalledCandidate {
    param($Context, $State)
    if (-not (Test-Path -LiteralPath ([string]$State.registry_path))) { throw 'Exact isolated uninstall registry key is missing.' }
    $registry = Get-ItemProperty -LiteralPath ([string]$State.registry_path)
    if ([string]$registry.DisplayVersion -ne [string]$Context.build_manifest.version -or
        -not (Get-NormalizedFullPath ([string]$registry.InstallLocation)).Equals([string]$State.install_root, [System.StringComparison]::OrdinalIgnoreCase)) {
        throw 'Installed candidate registry identity/version/path is wrong.'
    }
    foreach ($role in $requiredInstalledRoles.Keys) {
        $expected = @($Context.build_manifest.installed_files | Where-Object { [string]$_.role -eq $role })[0]
        $actual = Get-FileRecord (Get-InstalledFilePath -State $State -Role $role)
        if ([int64]$actual.bytes -ne [int64]$expected.bytes -or
            -not ([string]$actual.sha256).Equals([string]$expected.sha256, [System.StringComparison]::OrdinalIgnoreCase)) {
            throw "Installed candidate file does not match build manifest: $role"
        }
    }
}

function Get-IsolatedOwnerMarkerPath {
    param([Parameter(Mandatory = $true)][string]$Root)
    return Join-Path (Get-NormalizedFullPath $Root) $isolatedOwnerMarkerName
}

function Assert-OwnedIsolatedRoot {
    param(
        $Context,
        [Parameter(Mandatory = $true)][string]$Root,
        [Parameter(Mandatory = $true)][ValidateSet('data', 'webview', 'rollback-backups')][string]$Label,
        [Parameter(Mandatory = $true)][string]$Boundary
    )
    $full = Get-NormalizedFullPath $Root
    if (-not (Test-Path -LiteralPath $full -PathType Container)) { throw "Owned isolated $Label root is missing: $full" }
    Assert-NoReparsePath -Path $full -Boundary $Boundary | Out-Null
    $marker = Get-IsolatedOwnerMarkerPath $full
    if (-not (Test-Path -LiteralPath $marker -PathType Leaf)) { throw "Owned isolated $Label marker is missing: $marker" }
    $owner = (Read-JsonObject -Path $marker -Label "isolated $Label owner").value
    Assert-ExactProperties $owner @('schema_version', 'stage', 'root_label', 'run_id', 'source_commit', 'candidate_sha256', 'created_at') "isolated $Label owner"
    if ([int]$owner.schema_version -ne 1 -or [string]$owner.stage -ne 'MOSS_FUNCTIONAL_FT_ISOLATED_ROOT_OWNER' -or
        [string]$owner.root_label -ne $Label -or [string]$owner.run_id -ne $Context.run_id -or
        ([string]$owner.source_commit).ToLowerInvariant() -ne $Context.source_commit -or
        -not ([string]$owner.candidate_sha256).Equals($Context.candidate.sha256, [System.StringComparison]::OrdinalIgnoreCase)) {
        throw "Isolated $Label root is owned by another run."
    }
    return [ordered]@{ label = $Label; root = $full; marker = Get-FileRecord $marker }
}

function Ensure-OwnedIsolatedRoot {
    param(
        $Context,
        [Parameter(Mandatory = $true)][string]$Root,
        [Parameter(Mandatory = $true)][ValidateSet('data', 'webview', 'rollback-backups')][string]$Label,
        [Parameter(Mandatory = $true)][string]$Boundary
    )
    $full = Get-NormalizedFullPath $Root
    if (-not (Test-Path -LiteralPath $full -PathType Container)) { [System.IO.Directory]::CreateDirectory($full) | Out-Null }
    Assert-NoReparsePath -Path $full -Boundary $Boundary | Out-Null
    $marker = Get-IsolatedOwnerMarkerPath $full
    if (-not (Test-Path -LiteralPath $marker)) {
        Write-JsonExclusive -Path $marker -Value ([ordered]@{
            schema_version = 1
            stage = 'MOSS_FUNCTIONAL_FT_ISOLATED_ROOT_OWNER'
            root_label = $Label
            run_id = $Context.run_id
            source_commit = $Context.source_commit
            candidate_sha256 = $Context.candidate.sha256
            created_at = [datetimeoffset]::UtcNow.ToString('o')
        })
    }
    return Assert-OwnedIsolatedRoot -Context $Context -Root $full -Label $Label -Boundary $Boundary
}

function Ensure-CandidateInstalled {
    param($Context, $State)
    $freshInstall = -not (Test-Path -LiteralPath ([string]$State.registry_path))
    if ($freshInstall) {
        foreach ($path in @([string]$State.install_root, [string]$State.data_root, [string]$State.webview_root, [string]$State.backup_root)) {
            if (Test-Path -LiteralPath $path) { throw "Unowned isolated path exists before install: $path" }
        }
        $run = Invoke-NativeCapture -Executable $Context.candidate.path -Arguments @('/S') -Label 'candidate silent install' -TimeoutSeconds 1200
        if ($run.exit_code -ne 0) { throw 'Candidate silent install failed.' }
        $deadline = (Get-Date).AddSeconds(30)
        while ((Get-Date) -lt $deadline -and -not (Test-Path -LiteralPath ([string]$State.registry_path))) { Start-Sleep -Milliseconds 250 }
    }
    Assert-InstalledCandidate -Context $Context -State $State
    foreach ($entry in @(
        @{ root = [string]$State.data_root; label = 'data'; boundary = $env:APPDATA },
        @{ root = [string]$State.webview_root; label = 'webview'; boundary = $env:LOCALAPPDATA },
        @{ root = [string]$State.backup_root; label = 'rollback-backups'; boundary = $env:APPDATA }
    )) {
        if ($freshInstall) {
            Ensure-OwnedIsolatedRoot -Context $Context -Root $entry.root -Label $entry.label -Boundary $entry.boundary | Out-Null
        } else {
            Assert-OwnedIsolatedRoot -Context $Context -Root $entry.root -Label $entry.label -Boundary $entry.boundary | Out-Null
        }
    }
    $State.isolated_roots_owned = $true
    Save-State -Context $Context -State $State
    Ensure-BoundRuntimeAssets -Context $Context -State $State
}

function Copy-BoundFileToDataRoot {
    param(
        $Record,
        [Parameter(Mandatory = $true)][string]$Destination,
        [Parameter(Mandatory = $true)][string]$DataRoot,
        [Parameter(Mandatory = $true)][string]$Label
    )
    $source = Assert-BoundFile $Record $Label
    $target = Get-NormalizedFullPath $Destination
    if (-not (Test-StrictChildPath -Candidate $target -Parent $DataRoot)) { throw "$Label destination escapes the isolated data root." }
    if (Test-Path -LiteralPath $target) {
        $actual = Get-FileRecord $target
        if ([int64]$actual.bytes -ne [int64]$source.bytes -or
            -not $actual.sha256.Equals($source.sha256, [System.StringComparison]::OrdinalIgnoreCase)) {
            throw "$Label destination exists with different bytes."
        }
        return $actual
    }
    $parent = Split-Path -Parent $target
    if (-not (Test-Path -LiteralPath $parent -PathType Container)) { [System.IO.Directory]::CreateDirectory($parent) | Out-Null }
    Assert-NoReparsePath -Path $parent -Boundary $DataRoot | Out-Null
    $reader = [System.IO.File]::Open($source.path, [System.IO.FileMode]::Open, [System.IO.FileAccess]::Read, [System.IO.FileShare]::Read)
    try {
        $writer = [System.IO.File]::Open($target, [System.IO.FileMode]::CreateNew, [System.IO.FileAccess]::Write, [System.IO.FileShare]::None)
        try { $reader.CopyTo($writer, 4MB); $writer.Flush($true) } finally { $writer.Dispose() }
    } finally { $reader.Dispose() }
    $copied = Get-FileRecord $target
    if ([int64]$copied.bytes -ne [int64]$source.bytes -or
        -not $copied.sha256.Equals($source.sha256, [System.StringComparison]::OrdinalIgnoreCase)) {
        throw "$Label copy does not match its bound source."
    }
    return $copied
}

function Ensure-BoundRuntimeAssets {
    param($Context, $State)
    $dataRoot = Get-NormalizedFullPath ([string]$State.data_root)
    Assert-NoReparsePath -Path $dataRoot -Boundary $env:APPDATA | Out-Null
    foreach ($role in $modelContracts.Keys) {
        $contract = $modelContracts[$role]
        $source = $Context.sidecar.models.PSObject.Properties[$role].Value
        $destination = Join-Path $dataRoot ([string]$contract.relative_path).Replace('/', '\')
        $copied = Copy-BoundFileToDataRoot $source $destination $dataRoot "formal $role model"
        if ([int64]$copied.bytes -ne [int64]$contract.bytes -or
            -not $copied.sha256.Equals([string]$contract.sha256, [System.StringComparison]::OrdinalIgnoreCase)) {
            throw "Seeded model does not match the formal contract: $role"
        }
    }
    $runtime = Assert-BoundRuntimePackage $Context.sidecar.moss_runtime
    $runtimeDestination = Join-Path $dataRoot 'runtime\moss'
    foreach ($row in @($runtime.files)) {
        $relative = Assert-SafeRelativeFileName ([string]$row.relative_path) 'MOSS runtime file'
        $record = [ordered]@{
            path = Join-Path $runtime.root $relative.Replace('/', '\')
            bytes = [int64]$row.bytes
            sha256 = [string]$row.sha256
        }
        $target = Join-Path $runtimeDestination $relative.Replace('/', '\')
        Copy-BoundFileToDataRoot $record $target $dataRoot "MOSS runtime $relative" | Out-Null
    }
    $seededFiles = @(Get-ChildItem -LiteralPath $runtimeDestination -File -Recurse -Force)
    if ($seededFiles.Count -ne @($runtime.files).Count) { throw 'Seeded MOSS runtime directory contains an unbound file.' }
}

function Get-FreeCdpPort {
    param($Context)
    $configured = [int]$Context.group_input.cdp_port
    if ($configured -lt 1024 -or $configured -gt 65535) { throw 'Group cdp_port must be between 1024 and 65535.' }
    $listeners = @(Get-NetTCPConnection -State Listen -LocalPort $configured -ErrorAction SilentlyContinue)
    if ($listeners.Count -ne 0) { throw "Configured CDP port is already listening: $configured" }
    return $configured
}

function Start-ExactCandidateApp {
    param($Context, $State)
    $main = Get-InstalledFilePath -State $State -Role 'main_executable'
    if ($null -ne $State.app_pid) {
        $existing = Get-CimInstance Win32_Process -Filter ("ProcessId=" + [int]$State.app_pid) -ErrorAction SilentlyContinue
        if ($null -ne $existing -and $existing.ExecutablePath -and (Get-NormalizedFullPath ([string]$existing.ExecutablePath)).Equals($main, [System.StringComparison]::OrdinalIgnoreCase)) {
            return $State
        }
    }
    $port = Get-FreeCdpPort $Context
    $start = [System.Diagnostics.ProcessStartInfo]::new()
    $start.FileName = $main
    $start.WorkingDirectory = Split-Path -Parent $main
    $start.UseShellExecute = $false
    $start.EnvironmentVariables['WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS'] = "--remote-debugging-port=$port --remote-allow-origins=http://127.0.0.1:$port"
    $process = [System.Diagnostics.Process]::new()
    $process.StartInfo = $start
    $started = [datetimeoffset]::UtcNow
    if (-not $process.Start()) { throw 'Candidate app did not start.' }
    $State.app_pid = [int]$process.Id
    $State.app_started_at = $started.ToString('o')
    $State.cdp_port = $port
    $State.cdp_target_id = $null
    $State.recorded_processes = @([ordered]@{ pid = [int]$process.Id; executable_path = $main; captured_at = $started.ToString('o') })
    Save-State -Context $Context -State $State
    $deadline = (Get-Date).AddSeconds(120)
    $target = $null
    while ((Get-Date) -lt $deadline) {
        $alive = Get-CimInstance Win32_Process -Filter ("ProcessId=" + [int]$State.app_pid) -ErrorAction SilentlyContinue
        if ($null -eq $alive) { throw 'Candidate app exited before CDP became ready.' }
        try {
            $targets = @(Invoke-RestMethod -Uri ("http://127.0.0.1:$port/json/list") -TimeoutSec 2)
            $pages = @($targets | Where-Object { $_.type -eq 'page' -and ($_.url.StartsWith('http://tauri.localhost') -or $_.url.StartsWith('http://localhost:')) })
            if ($pages.Count -eq 1) { $target = $pages[0]; break }
        } catch {}
        Start-Sleep -Milliseconds 500
    }
    if ($null -eq $target) { throw 'Exact candidate WebView2 target did not become ready.' }
    $State.cdp_target_id = [string]$target.id
    Save-State -Context $Context -State $State
    Start-Sleep -Seconds 3
    return $State
}

function Invoke-CdpAction {
    param(
        $Context, $State,
        [Parameter(Mandatory = $true)][string]$Action,
        [Parameter(Mandatory = $true)][string]$Label,
        $Request = $null,
        [int]$TimeoutSeconds = 900,
        [ValidateSet('cold', 'warm', 'not_applicable')][string]$ModelState = 'not_applicable'
    )
    if ($null -eq $State.cdp_port -or [string]::IsNullOrWhiteSpace([string]$State.cdp_target_id)) { throw 'Exact CDP binding is absent from state.' }
    $rawRoot = Join-Path $Context.state_root ($ProducerKey + '.raw')
    if (-not (Test-Path -LiteralPath $rawRoot -PathType Container)) { [System.IO.Directory]::CreateDirectory($rawRoot) | Out-Null }
    Assert-NoReparsePath -Path $rawRoot -Boundary $Context.private_root | Out-Null
    $safeLabel = $Label -replace '[^A-Za-z0-9._-]', '_'
    $output = Join-Path $rawRoot ($safeLabel + '.json')
    $requestPath = Join-Path $rawRoot ($safeLabel + '.request.private.json')
    if (Test-Path -LiteralPath $output) { throw "CDP raw output already exists: $output" }
    if ($null -eq $Request) { $Request = [ordered]@{} }
    Write-JsonExclusive -Value $Request -Path $requestPath
    $environment = Get-MossPerformanceEnvironmentSnapshot -ModelState $ModelState
    $monitor = $null
    $monitorStdoutTask = $null
    $monitorStderrTask = $null
    $monitorOutput = $null
    $monitorStop = $null
    $monitoredRoles = switch ($Action) {
        'moss-cancel' { @('moss') }
        'moss-complete' { @('moss') }
        'summary-generate' { @('qwen') }
        'inference-exclusion' { @('moss', 'qwen') }
        default { @() }
    }
    if ($monitoredRoles.Count -gt 0) {
        $monitorOutput = Join-Path $rawRoot ($safeLabel + '.process-monitor.private.json')
        $monitorStop = Join-Path $rawRoot ($safeLabel + '.process-monitor-stop.private.json')
        $roots = @(
            [ordered]@{ role = 'moss'; executable_path = Get-InstalledFilePath $State 'moss_helper'; executable_sha256 = Get-InstalledRoleSha256 $Context 'moss_helper' },
            [ordered]@{ role = 'qwen'; executable_path = Get-InstalledFilePath $State 'llama_helper'; executable_sha256 = Get-InstalledRoleSha256 $Context 'llama_helper' }
        )
        $monitorStart = [System.Diagnostics.ProcessStartInfo]::new()
        $monitorStart.FileName = (Get-Command powershell.exe -ErrorAction Stop).Source
        $monitorArguments = @(
            '-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', $processMonitorScript,
            '-RootsJson', ($roots | ConvertTo-Json -Depth 5 -Compress),
            '-ExpectedRolesJson', ($monitoredRoles | ConvertTo-Json -Compress),
            '-StopSignal', $monitorStop, '-Output', $monitorOutput
        )
        $monitorStart.Arguments = ($monitorArguments | ForEach-Object { ConvertTo-NativeArgument -Value ([string]$_) }) -join ' '
        $monitorStart.UseShellExecute = $false
        $monitorStart.CreateNoWindow = $true
        $monitorStart.RedirectStandardOutput = $true
        $monitorStart.RedirectStandardError = $true
        $monitor = [System.Diagnostics.Process]::new()
        $monitor.StartInfo = $monitorStart
        if (-not $monitor.Start()) { throw "Process monitor did not start for $Label." }
        $monitorStdoutTask = $monitor.StandardOutput.ReadToEndAsync()
        $monitorStderrTask = $monitor.StandardError.ReadToEndAsync()
        Start-Sleep -Milliseconds 250
    }
    $node = (Get-Command node.exe -ErrorAction Stop).Source
    try {
        $run = Invoke-NativeCapture -Executable $node -Arguments @($cdpScript, $Action, $Context.state_path, $output, $requestPath) `
            -Label ("CDP " + $Label) -TimeoutSeconds $TimeoutSeconds -Environment @{
                CDP_PORT = [string]$State.cdp_port
                CDP_TARGET_ID = [string]$State.cdp_target_id
            }
    } finally {
        if ($null -ne $monitor) {
            $actionCompletedMonotonicMs = [System.Diagnostics.Stopwatch]::GetTimestamp() * 1000.0 / [System.Diagnostics.Stopwatch]::Frequency
            if (Test-Path -LiteralPath $output -PathType Leaf) {
                try {
                    $completedOutput = (Read-JsonObject -Path $output -Label "CDP $Label terminal output").value
                    $reportedCompleted = Get-PropertyValue $completedOutput @('action_completed_system_monotonic_ms')
                    if ($null -ne $reportedCompleted -and [double]$reportedCompleted -gt 0 -and
                        [double]$reportedCompleted -le $actionCompletedMonotonicMs) {
                        $actionCompletedMonotonicMs = [double]$reportedCompleted
                    }
                } catch {}
            }
            Write-JsonExclusive -Value ([ordered]@{ action = $Action; action_completed_monotonic_ms = $actionCompletedMonotonicMs }) -Path $monitorStop
            if (-not $monitor.WaitForExit(15000)) {
                throw "Process monitor did not finish after $Label."
            }
            $monitorStdout = $monitorStdoutTask.GetAwaiter().GetResult()
            $monitorStderr = $monitorStderrTask.GetAwaiter().GetResult()
            if ($monitor.ExitCode -ne 0) {
                throw "Process monitor failed for $Label with code $($monitor.ExitCode): $monitorStderr $monitorStdout"
            }
        }
    }
    $value = (Read-JsonObject -Path $output -Label "CDP $Label output").value
    $processEvidence = if ($null -ne $monitorOutput) {
        [ordered]@{ output = Get-FileRecord $monitorOutput; value = (Read-JsonObject $monitorOutput "process monitor $Label").value }
    } else { $null }
    return [ordered]@{ run = $run; output = Get-FileRecord $output; value = $value;
        performance_environment = $environment; process_monitor = $processEvidence }
}

function Stop-ExactCandidateApp {
    param($Context, $State)
    $records = @()
    if ($null -ne $State.app_pid) {
        $pidValue = [int]$State.app_pid
        $record = Get-CimInstance Win32_Process -Filter ("ProcessId=$pidValue") -ErrorAction SilentlyContinue
        if ($null -ne $record) {
            $main = Get-InstalledFilePath -State $State -Role 'main_executable'
            if (-not $record.ExecutablePath -or -not (Get-NormalizedFullPath ([string]$record.ExecutablePath)).Equals($main, [System.StringComparison]::OrdinalIgnoreCase)) {
                throw 'Stored app PID now belongs to another executable; refusing to terminate it.'
            }
            if ($null -ne $State.cdp_port -and -not [string]::IsNullOrWhiteSpace([string]$State.cdp_target_id)) {
                $exitOutput = Join-Path $Context.state_root ($ProducerKey + '.exit.private.json')
                if (-not (Test-Path -LiteralPath $exitOutput)) {
                    $node = (Get-Command node.exe -ErrorAction Stop).Source
                    $records += Invoke-NativeCapture -Executable $node -Arguments @((Join-Path $scriptRoot 'cdp-exit-app.mjs'), $exitOutput) `
                        -Label 'exact CDP app exit' -TimeoutSeconds 20 -AllowFailure -Environment @{
                            CDP_PORT = [string]$State.cdp_port; CDP_TARGET_ID = [string]$State.cdp_target_id
                        }
                }
            }
            $deadline = (Get-Date).AddSeconds(10)
            while ((Get-Date) -lt $deadline -and $null -ne (Get-CimInstance Win32_Process -Filter ("ProcessId=$pidValue") -ErrorAction SilentlyContinue)) {
                Start-Sleep -Milliseconds 250
            }
            if ($null -ne (Get-CimInstance Win32_Process -Filter ("ProcessId=$pidValue") -ErrorAction SilentlyContinue)) {
                $killOutput = & taskkill.exe /PID $pidValue /T /F 2>&1 | Out-String
                $records += [ordered]@{ method = 'taskkill-exact-pid-tree'; pid = $pidValue; output = $killOutput; exit_code = $LASTEXITCODE }
            }
        }
    }
    $State.app_pid = $null
    $State.app_started_at = $null
    $State.cdp_target_id = $null
    Save-State -Context $Context -State $State
    return @($records)
}

function Get-ExactProductProcesses {
    param($State)
    $allowed = @($requiredInstalledRoles.Keys | Where-Object { $_ -in @('main_executable', 'llama_helper', 'moss_helper') } | ForEach-Object {
        (Get-InstalledFilePath -State $State -Role $_).ToLowerInvariant()
    })
    return @(Get-CimInstance Win32_Process -ErrorAction SilentlyContinue | Where-Object {
        $_.ExecutablePath -and $allowed -contains (Get-NormalizedFullPath ([string]$_.ExecutablePath)).ToLowerInvariant()
    } | ForEach-Object {
        [ordered]@{ pid = [int]$_.ProcessId; parent_pid = [int]$_.ParentProcessId; executable_path = [string]$_.ExecutablePath }
    })
}

function Get-DirectoryManifest {
    param([Parameter(Mandatory = $true)][string]$Root)
    $full = Get-NormalizedFullPath $Root
    if (-not (Test-Path -LiteralPath $full -PathType Container)) { return @() }
    Assert-NoReparsePath -Path $full -Boundary ([System.IO.Path]::GetPathRoot($full)) | Out-Null
    return @(
        Get-ChildItem -LiteralPath $full -File -Recurse -Force | Sort-Object FullName | ForEach-Object {
            Assert-NoReparsePath -Path $_.FullName -Boundary $full | Out-Null
            [ordered]@{
                relative_path = $_.FullName.Substring($full.Length).TrimStart('\').Replace('\', '/')
                bytes = [int64]$_.Length
                sha256 = (Get-FileHash -LiteralPath $_.FullName -Algorithm SHA256).Hash.ToUpperInvariant()
            }
        }
    )
}

function ConvertTo-MossPerformanceSample {
    param(
        [Parameter(Mandatory = $true)]$ActionResult,
        [Parameter(Mandatory = $true)][string]$GateName,
        [Parameter(Mandatory = $true)][string]$Label
    )
    $performance = Get-PropertyValue -Object $ActionResult.value -Names @('performance')
    $event = if ($null -ne $performance) { Get-PropertyValue -Object $performance -Names @($GateName) } else { $null }
    if ($null -eq $event) { throw "CDP action $Label did not produce $GateName timing evidence." }
    return [ordered]@{
        label = $Label
        clock = [string](Get-PropertyValue $event @('clock'))
        started_monotonic_ms = Get-PropertyValue $event @('started_monotonic_ms')
        completed_monotonic_ms = Get-PropertyValue $event @('completed_monotonic_ms')
        environment = $ActionResult.performance_environment
    }
}

function Assert-MossPerformanceSample {
    param(
        [Parameter(Mandatory = $true)]$ActionResult,
        [Parameter(Mandatory = $true)][string]$GateName,
        [Parameter(Mandatory = $true)][string]$Label
    )
    $sample = ConvertTo-MossPerformanceSample $ActionResult $GateName $Label
    $started = $sample.started_monotonic_ms
    $completed = $sample.completed_monotonic_ms
    $threshold = [double]$performanceGateSpecifications[$GateName].threshold_ms
    if ([string]$sample.clock -ne 'node_performance_now' -or $null -eq $started -or $null -eq $completed -or
        [double]$completed -lt [double]$started) {
        throw "$GateName sample $Label is missing valid monotonic timing evidence."
    }
    $elapsed = [double]$completed - [double]$started
    if ($elapsed -gt $threshold) {
        throw "$GateName sample $Label exceeded the hard threshold immediately: $elapsed ms > $threshold ms."
    }
    return $sample
}

function New-MossGateSummaryFromActions {
    param(
        [Parameter(Mandatory = $true)][string]$GateName,
        [Parameter(Mandatory = $true)]$Actions
    )
    $samples = @()
    $index = 0
    foreach ($actionResult in @($Actions)) {
        $index++
        $samples += Assert-MossPerformanceSample $actionResult $GateName ("$GateName-$index")
    }
    $summary = New-MossPerformanceGateSummary -GateName $GateName -Samples $samples
    if (-not [bool]$summary.passed) {
        throw "$GateName performance statistics failed: $($summary.failure_reason)"
    }
    return $summary
}

function Assert-MossNativeDecodeEvidence {
    param([Parameter(Mandatory = $true)]$ActionResult)
    $reported = Get-PropertyValue -Object $ActionResult.value -Names @('native_decode_contract')
    if ($null -eq $reported) { throw 'CDP moss-complete omitted native decode evidence.' }
    $apiFields = Get-PropertyValue -Object $reported -Names @('api_fields')
    $validated = Test-MossNativeDecodeContract -Value $apiFields
    if (-not [bool]$validated.passed -or -not [bool](Get-PropertyValue $reported @('passed'))) {
        throw "MOSS native decode evidence failed: $($validated.failure_reason)"
    }
    if ([string](Get-PropertyValue $reported @('language_requested')) -ne [string]$validated.language_requested -or
        [string](Get-PropertyValue $reported @('language_resolved')) -ne [string]$validated.language_resolved -or
        [string](Get-PropertyValue $reported @('decode_parameters_json')) -ne [string]$validated.decode_parameters_json -or
        -not [string](Get-PropertyValue $reported @('decode_parameters_sha256')).Equals(
            [string]$validated.decode_parameters_sha256,
            [System.StringComparison]::OrdinalIgnoreCase
        )) {
        throw 'CDP snake_case native decode evidence differs from the camelCase API record.'
    }
    return $validated
}

function Get-MossProcessRoleEvidence {
    param([Parameter(Mandatory = $true)]$ActionResult, [Parameter(Mandatory = $true)][string]$Role)
    if ($null -eq $ActionResult.process_monitor) { throw "Process monitor evidence is absent for $Role." }
    $roleEvidence = Get-PropertyValue $ActionResult.process_monitor.value @('role_evidence', 'roleEvidence')
    $evidence = Get-PropertyValue $roleEvidence @($Role)
    if ($null -eq $evidence) { throw "Process monitor did not produce $Role lifecycle evidence." }
    return $evidence
}

function Assert-MossProcessRoleExited {
    param([Parameter(Mandatory = $true)]$ActionResult, [Parameter(Mandatory = $true)][string]$Role)
    $monitor = $ActionResult.process_monitor.value
    if ([bool](Get-PropertyValue $monitor @('timed_out_waiting_for_zero'))) {
        throw 'The helper monitor timed out before two confirmed empty scans.'
    }
    if ([int](Get-PropertyValue $monitor @('termination_actions_issued')) -ne 0) {
        throw 'The read-only helper monitor reported a process termination action.'
    }
    $otherPathProcesses = @(Get-PropertyValue $monitor @('same_name_other_path_processes'))
    if ($otherPathProcesses.Count -gt 0 -and
        -not [bool](Get-PropertyValue $monitor @('same_name_other_path_untouched'))) {
        throw 'An observed same-name process from another path did not remain untouched.'
    }
    $evidence = Get-MossProcessRoleEvidence $ActionResult $Role
    $actionCompleted = [double](Get-PropertyValue $evidence @('action_completed_monotonic_ms'))
    $firstZero = [double](Get-PropertyValue $evidence @('first_zero_monotonic_ms'))
    $secondZero = [double](Get-PropertyValue $evidence @('second_zero_monotonic_ms'))
    $rawFirstZeroAfter = $firstZero - $actionCompleted
    $rawConfirmationInterval = $secondZero - $firstZero
    if (-not [bool]$evidence.passed -or [int]$evidence.residual_process_count -ne 0 -or
        -not [bool](Get-PropertyValue $evidence @('first_zero_within_five_seconds')) -or
        -not [bool](Get-PropertyValue $evidence @('consecutive_zero_scans_confirmed')) -or
        [bool](Get-PropertyValue $evidence @('process_reappeared_after_first_zero')) -or
        $rawFirstZeroAfter -lt 0 -or $rawFirstZeroAfter -gt 5000 -or
        $rawConfirmationInterval -lt 1000) {
        throw "$Role exact helper tree did not reach zero within five seconds and remain absent for the one-second confirmation scan."
    }
    return $evidence
}

function Get-MossRecursiveTextValues {
    param($Value, [switch]$ExplicitMarkerOnly)
    $result = @()
    if ($null -eq $Value) { return $result }
    if ($Value -is [string]) {
        if (-not $ExplicitMarkerOnly -and -not [string]::IsNullOrWhiteSpace($Value)) { return @([string]$Value) }
        return $result
    }
    if ($Value -is [System.ValueType]) { return $result }
    if ($Value -is [System.Collections.IDictionary]) {
        foreach ($key in $Value.Keys) {
            $name = [string]$key
            $child = $Value[$key]
            if ($name -match '(?i)(tail.*marker|marker.*tail)' -and $child -is [string] -and -not [string]::IsNullOrWhiteSpace([string]$child)) {
                $result += [string]$child
            } elseif (-not $ExplicitMarkerOnly -and $name -match '(?i)^(text|content|transcript|reference|expected_text)$') {
                $result += Get-MossRecursiveTextValues $child
            } else {
                $result += Get-MossRecursiveTextValues $child -ExplicitMarkerOnly:$ExplicitMarkerOnly
            }
        }
        return @($result)
    }
    if ($Value -is [System.Collections.IEnumerable]) {
        foreach ($item in $Value) { $result += Get-MossRecursiveTextValues $item -ExplicitMarkerOnly:$ExplicitMarkerOnly }
        return @($result)
    }
    foreach ($property in @($Value.PSObject.Properties)) {
        $name = [string]$property.Name
        if ($name -match '(?i)(tail.*marker|marker.*tail)' -and $property.Value -is [string] -and
            -not [string]::IsNullOrWhiteSpace([string]$property.Value)) {
            $result += [string]$property.Value
        } elseif (-not $ExplicitMarkerOnly -and $name -match '(?i)^(text|content|transcript|reference|expected_text)$') {
            $result += Get-MossRecursiveTextValues $property.Value
        } else {
            $result += Get-MossRecursiveTextValues $property.Value -ExplicitMarkerOnly:$ExplicitMarkerOnly
        }
    }
    return @($result)
}

function Get-MossTailMarkerFromBoundReference {
    param([Parameter(Mandatory = $true)][string]$Path)
    $raw = Get-Content -LiteralPath $Path -Raw -Encoding UTF8
    $texts = @()
    if ([System.IO.Path]::GetExtension($Path).Equals('.json', [System.StringComparison]::OrdinalIgnoreCase)) {
        $document = $raw | ConvertFrom-Json
        $texts = @(Get-MossRecursiveTextValues $document -ExplicitMarkerOnly)
        if ($texts.Count -eq 0) { $texts = @(Get-MossRecursiveTextValues $document) }
    } else {
        $texts = @($raw)
    }
    $joined = (@($texts | ForEach-Object { [string]$_ }) -join "`n").Trim()
    if ([string]::IsNullOrWhiteSpace($joined)) { throw "Bound reference has no tail text: $Path" }
    $nonemptyLines = @($joined -split '\r?\n' | ForEach-Object { $_.Trim() } | Where-Object { $_ })
    $tail = if ($nonemptyLines.Count -gt 0) { [string]$nonemptyLines[-1] } else { $joined }
    $marker = $tail
    if ($tail.Length -gt 24) {
        for ($length = 24; $length -le [math]::Min(96, $tail.Length); $length += 8) {
            $candidate = $tail.Substring($tail.Length - $length)
            if ((Get-MossOrdinalSubstringCount -Text $joined -Needle $candidate) -eq 1) { $marker = $candidate; break }
        }
    }
    if ((Get-MossOrdinalSubstringCount -Text $joined -Needle $marker) -ne 1) {
        throw "Could not derive one unique tail marker from the bound reference: $Path"
    }
    return $marker
}

function New-MossRecordingFinalizationEvidence {
    param(
        [Parameter(Mandatory = $true)]$Recording,
        [Parameter(Mandatory = $true)]$AfterFiveSeconds,
        [Parameter(Mandatory = $true)][string]$ReferencePath,
        [Parameter(Mandatory = $true)][double]$ExpectedDurationSeconds,
        [bool]$RequirePause = $true
    )
    $pause = if ($null -ne $Recording.pause_resume) { $Recording.pause_resume.value } else { $null }
    $pauseObserved = $RequirePause -and $null -ne $pause -and [bool]$pause.verdict.pauseObserved
    $beforeResume = if ($null -ne $pause) { [int]$pause.transcript_change_count_before_resume } else { 0 }
    $afterResume = if ($null -ne $pause) { [int]$pause.transcript_change_count_after_resume } else { 0 }
    return New-MossFinalizationEvidence `
        -ExpectedMarker (Get-MossTailMarkerFromBoundReference $ReferencePath) `
        -FinalTranscript ([string]$Recording.finalize.value.final_transcript) `
        -ExpectedDurationSeconds $ExpectedDurationSeconds `
        -LastEndSeconds ([double]$Recording.finalize.value.time_audit.lastEnd) `
        -ChunksInQueueAtFinalization ([int]$Recording.finalize.value.chunks_in_queue_at_finalization) `
        -TranscriptionIsProcessingAtFinalization ([bool]$Recording.finalize.value.transcription_status_at_finalization.is_processing) `
        -TranscriptSha256AtFinalization ([string]$Recording.finalize.value.transcript_sha256) `
        -TranscriptSha256AfterFiveSeconds ([string]$AfterFiveSeconds.value.transcript_sha256) `
        -PauseObserved $pauseObserved `
        -TranscriptChangeCountBeforeResume $beforeResume `
        -TranscriptChangeCountAfterResume $afterResume
}

function Invoke-RecordedAudioMeeting {
    param(
        $Context, $State,
        [Parameter(Mandatory = $true)][string]$AudioPath,
        [Parameter(Mandatory = $true)][double]$ExpectedDurationSeconds,
        [Parameter(Mandatory = $true)][string]$Label,
        [Parameter(Mandatory = $true)][string]$TitlePrefix
    )
    $audio = Get-FileRecord $AudioPath
    if ([System.IO.Path]::GetExtension($audio.path) -ne '.wav') { throw 'Deterministic product recording requires a WAV input.' }
    $start = Invoke-CdpAction -Context $Context -State $State -Action 'recording-start' -Label ($Label + '-start')
    if (-not [bool]$start.value.verdict.backendStarted) { throw 'Product recording did not start.' }
    $monitorRequest = [ordered]@{ duration_ms = [int64](($ExpectedDurationSeconds + 120) * 1000); poll_ms = 500 }
    $rawRoot = Join-Path $Context.state_root ($ProducerKey + '.raw')
    $monitorRequestPath = Join-Path $rawRoot ($Label + '-monitor.request.private.json')
    $monitorOutput = Join-Path $rawRoot ($Label + '-monitor.json')
    Write-JsonExclusive -Value $monitorRequest -Path $monitorRequestPath
    $node = (Get-Command node.exe -ErrorAction Stop).Source
    $monitorStart = [System.Diagnostics.ProcessStartInfo]::new()
    $monitorStart.FileName = $node
    $monitorStart.Arguments = (@($cdpScript, 'recording-monitor', $Context.state_path, $monitorOutput, $monitorRequestPath) | ForEach-Object {
        ConvertTo-NativeArgument -Value ([string]$_)
    }) -join ' '
    $monitorStart.UseShellExecute = $false
    $monitorStart.CreateNoWindow = $true
    $monitorStart.RedirectStandardOutput = $true
    $monitorStart.RedirectStandardError = $true
    $monitorStart.EnvironmentVariables['CDP_PORT'] = [string]$State.cdp_port
    $monitorStart.EnvironmentVariables['CDP_TARGET_ID'] = [string]$State.cdp_target_id
    $monitor = [System.Diagnostics.Process]::new()
    $monitor.StartInfo = $monitorStart
    if (-not $monitor.Start()) { throw 'Live monitor did not start.' }
    $monitorPid = $monitor.Id
    $stdoutTask = $monitor.StandardOutput.ReadToEndAsync()
    $stderrTask = $monitor.StandardError.ReadToEndAsync()
    Start-Sleep -Seconds 1
    $playbackStartInfo = [System.Diagnostics.ProcessStartInfo]::new()
    $playbackStartInfo.FileName = (Get-Command powershell.exe -ErrorAction Stop).Source
    $playbackStartInfo.Arguments = (@('-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', $playbackScript, '-AudioPath', $audio.path) | ForEach-Object {
        ConvertTo-NativeArgument -Value ([string]$_)
    }) -join ' '
    $playbackStartInfo.UseShellExecute = $false
    $playbackStartInfo.CreateNoWindow = $true
    $playbackStartInfo.RedirectStandardOutput = $true
    $playbackStartInfo.RedirectStandardError = $true
    $playback = [System.Diagnostics.Process]::new()
    $playback.StartInfo = $playbackStartInfo
    $playbackStarted = [datetimeoffset]::UtcNow
    if (-not $playback.Start()) { throw 'Bound WAV playback process did not start.' }
    $playbackStdoutTask = $playback.StandardOutput.ReadToEndAsync()
    $playbackStderrTask = $playback.StandardError.ReadToEndAsync()
    Start-Sleep -Seconds 2
    $pauseResume = Invoke-CdpAction -Context $Context -State $State -Action 'recording-pause-resume' `
        -Label ($Label + '-pause-resume') -Request ([ordered]@{ pause_ms = 1500; continuation_timeout_ms = 30000 }) -TimeoutSeconds 60
    if (-not (Test-AllTrue $pauseResume.value.verdict)) { throw 'Pause/resume or post-resume transcript continuation failed.' }
    $playbackTimeoutMilliseconds = [int][math]::Ceiling(($ExpectedDurationSeconds + 60) * 1000)
    if (-not $playback.WaitForExit($playbackTimeoutMilliseconds)) {
        throw 'Bound WAV playback did not finish within the duration-bound timeout.'
    }
    $playbackStdout = $playbackStdoutTask.GetAwaiter().GetResult()
    $playbackStderr = $playbackStderrTask.GetAwaiter().GetResult()
    if ($playback.ExitCode -ne 0) { throw "Bound WAV playback failed with code $($playback.ExitCode): $playbackStderr" }
    $playbackCompleted = [datetimeoffset]::UtcNow
    $elapsed = ($playbackCompleted - $playbackStarted).TotalSeconds
    if ($elapsed -lt ($ExpectedDurationSeconds - 1.5)) { throw "WAV playback ended too early: $elapsed seconds." }
    Start-Sleep -Seconds 6
    $stop = Invoke-CdpAction -Context $Context -State $State -Action 'recording-stop' -Label ($Label + '-stop') -TimeoutSeconds 120
    Assert-MossPerformanceSample $stop 'stop_feedback' ($Label + '-stop-feedback') | Out-Null
    Assert-MossPerformanceSample $stop 'page_unlock' ($Label + '-page-unlock') | Out-Null
    if (-not (Test-AllTrue $stop.value.verdict)) { throw 'Product recording stop/finalization hard gates failed.' }
    if (-not $monitor.WaitForExit(150000)) {
        & taskkill.exe /PID $monitorPid /T /F 2>$null | Out-Null
        throw 'Live monitor did not finish after recording stopped.'
    }
    $monitorStdout = $stdoutTask.GetAwaiter().GetResult()
    $monitorStderr = $stderrTask.GetAwaiter().GetResult()
    if ($monitor.ExitCode -ne 0) { throw "Live monitor failed with code $($monitor.ExitCode): $monitorStderr" }
    $monitorValue = (Read-JsonObject -Path $monitorOutput -Label 'live monitor output').value
    $finalize = Invoke-CdpAction -Context $Context -State (Read-State $Context) -Action 'meeting-finalize' -Label ($Label + '-finalize') `
        -Request ([ordered]@{ title_prefix = $TitlePrefix; expected_duration_seconds = $ExpectedDurationSeconds; timeout_ms = 240000 }) -TimeoutSeconds 300
    return [ordered]@{
        audio = $audio
        playback_started_at = $playbackStarted.ToString('o')
        playback_completed_at = $playbackCompleted.ToString('o')
        playback_elapsed_seconds = $elapsed
        monitor_pid = $monitorPid
        monitor_stdout = $monitorStdout
        monitor_stderr = $monitorStderr
        playback_stdout = $playbackStdout
        playback_stderr = $playbackStderr
        start = $start
        pause_resume = $pauseResume
        stop = $stop
        monitor = [ordered]@{ output = Get-FileRecord $monitorOutput; value = $monitorValue }
        finalize = $finalize
    }
}

function Invoke-StopPerformanceRehearsals {
    param(
        $Context, $State,
        [Parameter(Mandatory = $true)][string]$AudioPath,
        [ValidateRange(1, 4)][int]$Count = 4
    )
    $audio = Get-FileRecord $AudioPath
    $results = @()
    for ($index = 1; $index -le $Count; $index++) {
        $label = "stop-rehearsal-$index"
        $currentState = Read-State $Context
        $start = Invoke-CdpAction $Context $currentState 'recording-start' ($label + '-start') ([ordered]@{}) 60
        if (-not [bool]$start.value.verdict.backendStarted) { throw "$label did not start recording." }
        $player = [System.Media.SoundPlayer]::new($audio.path)
        try {
            $player.Load()
            $player.Play()
            Start-Sleep -Seconds 3
        } finally {
            try { $player.Stop() } catch {}
            $player.Dispose()
        }
        $stop = Invoke-CdpAction $Context (Read-State $Context) 'recording-stop' ($label + '-stop') ([ordered]@{}) 60
        Assert-MossPerformanceSample $stop 'stop_feedback' ($label + '-feedback') | Out-Null
        Assert-MossPerformanceSample $stop 'page_unlock' ($label + '-unlock') | Out-Null
        if (-not (Test-AllTrue $stop.value.verdict)) { throw "$label stop hard checks failed." }
        $results += $stop
        Start-Sleep -Seconds 1
    }
    return @($results)
}

function New-CaseResult {
    param($Context, [string]$CaseId, [string]$Verdict, $Checks, $Metrics, $Private)
    if ($Verdict -eq 'PASS' -and -not (Test-AllTrue $Checks)) { throw "$CaseId cannot PASS while a required check is false." }
    return [ordered]@{
        public = [ordered]@{
            schema_version = 1
            stage = 'MOSS_FUNCTIONAL_FT_RESULT'
            id = $CaseId
            producer_key = $ProducerKey
            verdict = $Verdict
            run_id = $Context.run_id
            source_commit = $Context.source_commit
            candidate_sha256 = $Context.candidate.sha256
            build_manifest_sha256 = $Context.build_manifest_record.sha256
            executed_at = [datetimeoffset]::UtcNow.ToString('o')
            checks = $Checks
            metrics = $Metrics
        }
        private = [ordered]@{
            schema_version = 1
            stage = 'MOSS_FUNCTIONAL_FT_RESULT_PRIVATE'
            id = $CaseId
            producer_key = $ProducerKey
            verdict = $Verdict
            run_id = $Context.run_id
            source_commit = $Context.source_commit
            candidate = $Context.candidate
            build_manifest = $Context.build_manifest_record
            acceptance = $Context.config_record
            producer_input = $Context.sidecar_record
            checks = $Checks
            metrics = $Metrics
            raw = $Private
        }
    }
}

function Write-CaseResults {
    param($Context, [Parameter(Mandatory = $true)]$Results)
    foreach ($caseId in @($groupCases[$ProducerKey])) {
        if (-not $Results.Contains($caseId)) { throw "Producer omitted required result $caseId." }
        $result = $Results[$caseId]
        Write-JsonExclusive -Value $result.private -Path (Resolve-SafeRelativePath -Root $Context.private_root -RelativePath "$caseId/result.private.json")
        $privateRecord = Get-FileRecord (Resolve-SafeRelativePath -Root $Context.private_root -RelativePath "$caseId/result.private.json")
        $result.public['private_result_bytes'] = $privateRecord.bytes
        $result.public['private_result_sha256'] = $privateRecord.sha256
        Write-JsonExclusive -Value $result.public -Path (Resolve-SafeRelativePath -Root $Context.public_root -RelativePath "$caseId/result.public.json")
    }
}

function Get-GroupScenarioInputs {
    param($Context)
    $firstId = @($groupCases[$ProducerKey])[0]
    $scenario = @($Context.config.scenarios | Where-Object { [string]$_.id -eq $firstId })[0]
    return @($scenario.inputs | ForEach-Object { Assert-BoundFile -Record $_ -Label "$firstId input" })
}

function Get-SummarySource {
    param($SummaryResult)
    $binding = $SummaryResult.value.sourceBinding
    $source = Get-PropertyValue -Object $binding -Names @('transcriptSource', 'transcript_source', 'sourceKind', 'source_kind')
    return ([string]$source).ToLowerInvariant()
}

function Invoke-LivePersistenceGroup {
    param($Context, $State)
    Ensure-CandidateInstalled $Context $State
    $State = Start-ExactCandidateApp $Context $State
    $inputs = Get-GroupScenarioInputs $Context
    if ($inputs.Count -ne 2) { throw 'FT-01 group requires short WAV and reference inputs.' }
    $recordingRoot = Get-NormalizedFullPath ([string]$Context.group_input.recording_root)
    if (-not (Test-StrictChildPath -Candidate $recordingRoot -Parent $Context.private_root)) { throw 'Short recording root must be below private evidence root.' }
    if (-not (Test-Path -LiteralPath $recordingRoot -PathType Container)) { [System.IO.Directory]::CreateDirectory($recordingRoot) | Out-Null }
    $bootstrap = Invoke-CdpAction $Context $State 'bootstrap' 'bootstrap' ([ordered]@{
        whisper_model = [string]$Context.group_input.whisper_model
        qwen_model = [string]$Context.group_input.qwen_model
        recording_root = $recordingRoot
    }) 300
    if (-not (Test-AllTrue $bootstrap.value.verdict)) { throw 'Candidate bootstrap/model readiness failed.' }
    $duration = [double]$Context.group_input.short_duration_seconds
    if ($duration -le 1 -or $duration -gt 600) { throw 'short_duration_seconds is outside the formal range.' }
    $stopRehearsals = @(Invoke-StopPerformanceRehearsals $Context $State $inputs[0].path 4)
    $record = Invoke-RecordedAudioMeeting $Context $State $inputs[0].path $duration 'short' ('MOSS-FT-SHORT-' + $Context.run_id)
    $State = Read-State $Context
    $snapshot1 = Invoke-CdpAction $Context $State 'meeting-snapshot' 'convergence-1' ([ordered]@{ expected_duration_seconds = $duration }) 120
    Start-Sleep -Seconds 5
    $snapshot2 = Invoke-CdpAction $Context $State 'meeting-snapshot' 'convergence-2' ([ordered]@{ expected_duration_seconds = $duration }) 120
    $finalization = New-MossRecordingFinalizationEvidence $record $snapshot2 $inputs[1].path $duration $true
    $stopActions = @($stopRehearsals) + @($record.stop)
    $stopFeedbackPerformance = New-MossGateSummaryFromActions 'stop_feedback' $stopActions
    $pageUnlockPerformance = New-MossGateSummaryFromActions 'page_unlock' $stopActions
    $whisperEnhancements = @()
    for ($index = 1; $index -le 3; $index++) {
        $modelState = if ($index -eq 1) { 'cold' } else { 'warm' }
        $whisper = Invoke-CdpAction $Context (Read-State $Context) 'whisper-enhance' ("whisper-enhance-$index") `
            ([ordered]@{ timeout_ms = 300000 }) 360 -ModelState $modelState
        Assert-MossPerformanceSample $whisper 'whisper_enhancement_completion' ("whisper-enhance-completion-$index") | Out-Null
        if (-not (Test-AllTrue $whisper.value.verdict)) { throw "Whisper enhancement $index hard checks failed." }
        $whisperEnhancements += $whisper
    }
    $whisperPerformance = New-MossGateSummaryFromActions 'whisper_enhancement_completion' $whisperEnhancements
    $manual = Invoke-CdpAction $Context $State 'manual-edit' 'manual-edit' ([ordered]@{
        title = 'MOSS-FT-MANUAL-' + $Context.run_id
        transcript_marker = ' [verified-manual-edit-' + $Context.run_id + ']'
    }) 180
    if (-not (Test-AllTrue $manual.value.verdict)) { throw 'Manual title/transcript UI edit failed.' }
    $State = Read-State $Context
    $summary1 = Invoke-CdpAction $Context $State 'summary-generate' 'summary-first' ([ordered]@{ timeout_ms = 180000 }) 240 -ModelState 'cold'
    if (-not (Test-AllTrue $summary1.value.verdict)) { throw 'First real Qwen summary failed.' }
    Assert-MossPerformanceSample $summary1 'summary_completion' 'summary-first' | Out-Null
    Assert-MossProcessRoleExited $summary1 'qwen' | Out-Null
    $State = Read-State $Context
    $summary2 = Invoke-CdpAction $Context $State 'summary-generate' 'summary-second' ([ordered]@{ timeout_ms = 180000 }) 240 -ModelState 'warm'
    if (-not (Test-AllTrue $summary2.value.verdict)) { throw 'Second real Qwen summary failed.' }
    Assert-MossPerformanceSample $summary2 'summary_completion' 'summary-second' | Out-Null
    Assert-MossProcessRoleExited $summary2 'qwen' | Out-Null
    $State = Read-State $Context
    $summary3 = Invoke-CdpAction $Context $State 'summary-generate' 'summary-third' ([ordered]@{ timeout_ms = 180000 }) 240 -ModelState 'warm'
    if (-not (Test-AllTrue $summary3.value.verdict)) { throw 'Third real Qwen summary failed.' }
    Assert-MossPerformanceSample $summary3 'summary_completion' 'summary-third' | Out-Null
    Assert-MossProcessRoleExited $summary3 'qwen' | Out-Null
    $summaryPerformance = New-MossGateSummaryFromActions 'summary_completion' @($summary1, $summary2, $summary3)
    $State = Read-State $Context
    $State.manual_edit_updated_at = Get-PropertyValue -Object $summary3.value.meetingAfter -Names @('updated_at', 'updatedAt')
    Save-State $Context $State
    Stop-ExactCandidateApp $Context $State | Out-Null
    $State = Start-ExactCandidateApp $Context (Read-State $Context)
    $persistence = Invoke-CdpAction $Context $State 'persistence-verify' 'restart-persistence' ([ordered]@{}) 180
    $monitor = $record.monitor.value
    $time = $record.finalize.value.time_audit
    $ft01Checks = [ordered]@{
        real_candidate_bootstrap = Test-AllTrue $bootstrap.value.verdict
        fixed_audio_played_complete = [double]$record.playback_elapsed_seconds -ge ($duration - 1.5)
        multiple_nonempty_live_changes = [int]$monitor.metrics.nonempty_change_count -ge 2
        final_transcript_nonempty = [int]$record.finalize.value.transcript_character_count -gt 0
        timestamps_parseable = [bool]$time.finite
        timestamps_legal = [bool]$time.legal
        timestamps_monotonic = [bool]$time.monotonic
    }
    $ft02Checks = [ordered]@{
        backend_stopped = [bool]$record.stop.value.verdict.backendStopped
        finalization_created_meeting = [int]$record.finalize.value.transcript_character_count -gt 0
        transcript_stable_after_five_seconds = [string]$snapshot1.value.transcript_sha256 -eq [string]$snapshot2.value.transcript_sha256
        segment_count_stable = [int]$snapshot1.value.time_audit.count -eq [int]$snapshot2.value.time_audit.count
        state_idle = -not [bool]$record.stop.value.after.is_recording -and -not [bool]$record.stop.value.after.is_active
        tail_marker_present_exactly_once = [bool]$finalization.tail_marker_present_exactly_once
        tail_coverage_ratio_at_least_98_percent = [bool]$finalization.tail_coverage_at_least_98_percent
        chunks_in_queue_at_finalization_zero = [bool]$finalization.chunks_in_queue_at_finalization_zero
        transcription_worker_idle_at_finalization = [bool]$finalization.transcription_worker_idle_at_finalization
        finalization_completed = [bool]$finalization.finalization_completed
        pause_observed = [bool]$finalization.pause_observed
        transcript_continues_after_resume = [bool]$finalization.transcript_continues_after_resume
        stop_feedback_five_samples_pass = [bool]$stopFeedbackPerformance.passed
        page_unlock_five_samples_pass = [bool]$pageUnlockPerformance.passed
        whisper_enhancement_three_samples_pass = [bool]$whisperPerformance.passed
    }
    $ft03Checks = [ordered]@{
        title_edited_through_ui = [bool]$manual.value.verdict.titleSaved
        transcript_edited_through_ui = [bool]$manual.value.verdict.transcriptSavedExactly
        first_qwen_summary_completed = [bool]$summary1.value.verdict.completed
        second_qwen_summary_completed = [bool]$summary2.value.verdict.completed
        third_qwen_summary_completed = [bool]$summary3.value.verdict.completed
        summary_three_samples_pass = [bool]$summaryPerformance.passed
        title_exact_after_restart = [bool]$persistence.value.title_exact
        transcript_exact_after_restart = [bool]$persistence.value.transcript_exact
        updated_at_exact_after_restart = [bool]$persistence.value.updated_at_exact
        three_completed_summaries_persist = [int]$persistence.value.completed_summary_count -ge 3
    }
    $results = [ordered]@{}
    $results['FT-01'] = New-CaseResult $Context 'FT-01' $(if (Test-AllTrue $ft01Checks) { 'PASS' } else { 'FAIL' }) $ft01Checks ([ordered]@{
        live_change_count = [int]$monitor.metrics.change_count; nonempty_live_change_count = [int]$monitor.metrics.nonempty_change_count
        final_segment_count = [int]$time.count; playback_elapsed_seconds = [double]$record.playback_elapsed_seconds
        short_input_sha256 = $inputs[0].sha256; short_reference_sha256 = $inputs[1].sha256
    }) ([ordered]@{ bootstrap = $bootstrap; recording = $record; snapshot = $snapshot1; stop_rehearsals = $stopRehearsals })
    $results['FT-02'] = New-CaseResult $Context 'FT-02' $(if (Test-AllTrue $ft02Checks) { 'PASS' } else { 'FAIL' }) $ft02Checks ([ordered]@{
        convergence_wait_seconds = 5; stable_segment_count = [int]$snapshot2.value.time_audit.count
        tail_coverage_ratio = [double]$finalization.tail_coverage_ratio
        chunks_in_queue_at_finalization = [int]$finalization.chunks_in_queue_at_finalization
        stop_feedback_p50_ms = [double]$stopFeedbackPerformance.p50_ms; stop_feedback_worst_ms = [double]$stopFeedbackPerformance.worst_ms
        page_unlock_p50_ms = [double]$pageUnlockPerformance.p50_ms; page_unlock_worst_ms = [double]$pageUnlockPerformance.worst_ms
        whisper_enhancement_p50_ms = [double]$whisperPerformance.p50_ms; whisper_enhancement_worst_ms = [double]$whisperPerformance.worst_ms
    }) ([ordered]@{ stop = $record.stop; finalize = $record.finalize; snapshot_1 = $snapshot1; snapshot_2 = $snapshot2;
        finalization = $finalization; stop_feedback_performance = $stopFeedbackPerformance;
        page_unlock_performance = $pageUnlockPerformance; whisper_enhancement_performance = $whisperPerformance;
        whisper_enhancements = $whisperEnhancements })
    $results['FT-03'] = New-CaseResult $Context 'FT-03' $(if (Test-AllTrue $ft03Checks) { 'PASS' } else { 'FAIL' }) $ft03Checks ([ordered]@{
        completed_summary_count = [int]$persistence.value.completed_summary_count
        summary_completion_p50_ms = [double]$summaryPerformance.p50_ms
        summary_completion_worst_ms = [double]$summaryPerformance.worst_ms
        edited_transcript_sha256 = [string]$persistence.value.transcript_sha256
    }) ([ordered]@{ manual = $manual; summary_1 = $summary1; summary_2 = $summary2; summary_3 = $summary3;
        summary_completion_performance = $summaryPerformance; persistence = $persistence })
    return $results
}

function Invoke-MossChainGroup {
    param($Context, $State)
    Ensure-CandidateInstalled $Context $State
    $priorPath = Join-Path $Context.state_root 'ft01-03-live-and-persistence.state.private.json'
    $prior = (Read-JsonObject $priorPath 'FT-01 group state').value
    if ([string]$prior.run_id -ne $Context.run_id -or [string]::IsNullOrWhiteSpace([string]$prior.meeting_id)) { throw 'FT-01 meeting state is unavailable.' }
    $State.meeting_id = [string]$prior.meeting_id
    Save-State $Context $State
    $State = Start-ExactCandidateApp $Context $State
    $cancelActions = @()
    $cancelHelperExits = @()
    for ($index = 1; $index -le 5; $index++) {
        $modelState = if ($index -eq 1) { 'cold' } else { 'warm' }
        $cancelAction = Invoke-CdpAction $Context (Read-State $Context) 'moss-cancel' ("moss-cancel-$index") `
            ([ordered]@{ cancel_after_ms = 1500; timeout_ms = 180000 }) 240 -ModelState $modelState
        Assert-MossPerformanceSample $cancelAction 'enhance_feedback' ("moss-enhance-feedback-$index") | Out-Null
        Assert-MossPerformanceSample $cancelAction 'cancel_feedback' ("moss-cancel-feedback-$index") | Out-Null
        if (-not (Test-AllTrue $cancelAction.value.verdict)) { throw "MOSS cancel trial $index failed." }
        $cancelHelperExits += Assert-MossProcessRoleExited $cancelAction 'moss'
        $cancelActions += $cancelAction
    }
    $enhanceFeedbackPerformance = New-MossGateSummaryFromActions 'enhance_feedback' $cancelActions
    $cancelFeedbackPerformance = New-MossGateSummaryFromActions 'cancel_feedback' $cancelActions
    $cancel = $cancelActions[0]
    $complete = Invoke-CdpAction $Context (Read-State $Context) 'moss-complete' 'moss-complete' ([ordered]@{ timeout_ms = 7200000 }) 7500 -ModelState 'warm'
    Assert-MossPerformanceSample $complete 'enhance_feedback' 'moss-complete-enhance-feedback' | Out-Null
    $nativeDecodeEvidence = Assert-MossNativeDecodeEvidence $complete
    $completeHelperExit = Assert-MossProcessRoleExited $complete 'moss'
    if (-not (Test-AllTrue $complete.value.verdict)) { throw 'MOSS completion/progress hard checks failed.' }
    $review = Invoke-CdpAction $Context (Read-State $Context) 'moss-review-strict' 'moss-review' ([ordered]@{
        positive_terms = @($Context.group_input.positive_terms)
        negative_terms = @($Context.group_input.negative_terms)
    }) 300
    if (-not (Test-AllTrue $review.value.verdict)) { throw 'Strict MOSS review hard checks failed.' }
    $State = Read-State $Context
    Stop-ExactCandidateApp $Context $State | Out-Null
    $State = Start-ExactCandidateApp $Context (Read-State $Context)
    $persist = Invoke-CdpAction $Context $State 'moss-workspace-verify' 'moss-restart-persistence' ([ordered]@{}) 180
    if (-not (Test-AllTrue $persist.value.verdict)) { throw 'MOSS review state did not persist across restart.' }
    $activate = Invoke-CdpAction $Context (Read-State $Context) 'moss-activate' 'moss-activate' ([ordered]@{}) 180
    $summaryMoss = Invoke-CdpAction $Context (Read-State $Context) 'summary-generate' 'moss-summary' ([ordered]@{ timeout_ms = 180000 }) 240 -ModelState 'warm'
    Assert-MossPerformanceSample $summaryMoss 'summary_completion' 'moss-summary' | Out-Null
    $summaryMossExit = Assert-MossProcessRoleExited $summaryMoss 'qwen'
    $rollback = Invoke-CdpAction $Context (Read-State $Context) 'moss-rollback' 'moss-rollback' ([ordered]@{}) 180
    $summaryWhisper = Invoke-CdpAction $Context (Read-State $Context) 'summary-generate' 'whisper-summary' ([ordered]@{ timeout_ms = 180000 }) 240 -ModelState 'warm'
    Assert-MossPerformanceSample $summaryWhisper 'summary_completion' 'whisper-summary' | Out-Null
    $summaryWhisperExit = Assert-MossProcessRoleExited $summaryWhisper 'qwen'
    $reactivate = Invoke-CdpAction $Context (Read-State $Context) 'moss-reactivate' 'moss-reactivate' ([ordered]@{}) 180
    $summaryReactivated = Invoke-CdpAction $Context (Read-State $Context) 'summary-generate' 'moss-reactivated-summary' ([ordered]@{ timeout_ms = 180000 }) 240 -ModelState 'warm'
    Assert-MossPerformanceSample $summaryReactivated 'summary_completion' 'moss-reactivated-summary' | Out-Null
    $summaryReactivatedExit = Assert-MossProcessRoleExited $summaryReactivated 'qwen'
    $inferenceExclusionAction = Invoke-CdpAction $Context (Read-State $Context) 'inference-exclusion' 'moss-qwen-exclusion' `
        ([ordered]@{ timeout_ms = 180000 }) 240 -ModelState 'warm'
    $inferenceMossExit = Assert-MossProcessRoleExited $inferenceExclusionAction 'moss'
    $inferenceQwenExit = Assert-MossProcessRoleExited $inferenceExclusionAction 'qwen'
    $inferenceExclusion = New-MossInferenceExclusionEvidence `
        -Samples @($inferenceExclusionAction.value.operationSamples) -Attempts @($inferenceExclusionAction.value.attempts)
    if (-not [bool]$inferenceExclusion.passed -or -not (Test-AllTrue $inferenceExclusionAction.value.verdict)) {
        throw 'MOSS/Qwen bidirectional mutual exclusion or zero-overlap gate failed.'
    }
    Start-Sleep -Seconds 3
    $helpers = @(Get-ExactProductProcesses (Read-State $Context) | Where-Object { $_.executable_path -match '(?i)(moss-helper|llama-helper)\.exe$' })
    $ftChecks = [ordered]@{}
    $ftChecks['FT-04'] = [ordered]@{
        completed = [bool]$complete.value.verdict.completed
        progress_monotonic = [bool]$complete.value.verdict.progressMonotonic
        progress_samples_present = [bool]$complete.value.verdict.progressSamplesPresent
        candidate_stored = [bool]$complete.value.verdict.candidateStored
        enhance_feedback_five_samples_pass = [bool]$enhanceFeedbackPerformance.passed
        complete_helper_tree_zero_within_five_seconds = [bool]$completeHelperExit.passed
        native_decode_contract_pass = [bool]$nativeDecodeEvidence.passed
    }
    $ftChecks['FT-05'] = [ordered]@{
        cancelled = [bool]$cancel.value.verdict.cancelled
        cancelled_run_has_no_candidate = [bool]$cancel.value.verdict.noCandidate
        five_cancel_trials_passed = @($cancelActions | Where-Object { Test-AllTrue $_.value.verdict }).Count -eq 5
        cancel_feedback_five_samples_pass = [bool]$cancelFeedbackPerformance.passed
        cancelled_helper_trees_zero_within_five_seconds = @($cancelHelperExits | Where-Object { [bool]$_.passed }).Count -eq 5
    }
    $ftChecks['FT-06'] = [ordered]@{
        candidate_not_auto_activated = [bool]$complete.value.verdict.currentTranscriptNotAutoReplaced
        manual_candidate_edit_persisted = [bool]$review.value.verdict.manualEditPersisted
    }
    $ftChecks['FT-07'] = [ordered]@{
        positive_truth_nonempty = @($Context.group_input.positive_terms).Count -gt 0
        correction_records_present = [int]$review.value.strict.corrections_count -gt 0
        all_positive_terms_traceable = [bool]$review.value.verdict.positiveTermsTraceable
    }
    $ftChecks['FT-08'] = [ordered]@{
        negative_truth_nonempty = @($Context.group_input.negative_terms).Count -gt 0
        negative_insertions_zero = [bool]$review.value.verdict.negativeInsertionsZero
    }
    $ftChecks['FT-09'] = [ordered]@{
        participants_present = [int]$review.value.strict.participants_count -ge 2
        speaker_labels_present = [int]$review.value.strict.speaker_label_count -gt 0
        binding_applied = [bool]$review.value.verdict.speakerBindingApplied
        segment_override_wins = [bool]$review.value.verdict.segmentOverrideWins
        correction_round_trip = [bool]$review.value.verdict.correctionRoundTrip
    }
    $ftChecks['FT-10'] = [ordered]@{
        revision_exact = [bool]$persist.value.verdict.revisionExact
        edit_exact = [bool]$persist.value.verdict.editedTextExact
        binding_exact = [bool]$persist.value.verdict.bindingExact
        override_exact = [bool]$persist.value.verdict.overrideExact
        correction_applied = [bool]$persist.value.verdict.correctionApplied
    }
    $ftChecks['FT-11'] = [ordered]@{
        candidate_activated = [bool]$activate.value.verdict.activated
        active_run_matches = [bool]$activate.value.verdict.activeRunMatches
        activation_id_present = [bool]$activate.value.verdict.activationIdPresent
    }
    $ftChecks['FT-12'] = [ordered]@{
        real_qwen_summary_completed = [bool]$summaryMoss.value.verdict.completed
        source_binding_present = [bool]$summaryMoss.value.verdict.sourceBindingPresent
        source_is_moss = (Get-SummarySource $summaryMoss) -eq 'moss'
        helper_process_count_zero = $helpers.Count -eq 0
        moss_qwen_bidirectional_exclusion = [bool]$inferenceExclusion.bidirectional_attempts_explicit
        moss_and_qwen_overlap_seconds_zero = [double]$inferenceExclusion.moss_and_qwen_overlap_seconds -eq 0
        summary_helper_tree_zero_within_five_seconds = [bool]$summaryMossExit.passed
    }
    $ftChecks['FT-13'] = [ordered]@{
        stale_revision_rejected = [bool]$review.value.verdict.staleRevisionRejected
        exact_conflict_code = [string]$review.value.conflict.value.code -eq 'MOSS_CANDIDATE_CONFLICT'
    }
    $ftChecks['FT-14'] = [ordered]@{
        active_run_cleared = [bool]$rollback.value.verdict.noActiveRun
        candidate_inactive = [bool]$rollback.value.verdict.candidateInactive
        whisper_summary_completed = [bool]$summaryWhisper.value.verdict.completed
        summary_source_is_whisper = (Get-SummarySource $summaryWhisper) -eq 'whisper'
    }
    $ftChecks['FT-15'] = [ordered]@{
        same_candidate_reactivated = [bool]$reactivate.value.verdict.activated
        active_run_matches = [bool]$reactivate.value.verdict.activeRunMatches
        summary_completed = [bool]$summaryReactivated.value.verdict.completed
        summary_source_is_moss = (Get-SummarySource $summaryReactivated) -eq 'moss'
        helper_process_count_zero = $helpers.Count -eq 0
        all_summary_helper_trees_zero_within_five_seconds = [bool]$summaryWhisperExit.passed -and [bool]$summaryReactivatedExit.passed
        exclusion_probe_helper_trees_zero_within_five_seconds = [bool]$inferenceMossExit.passed -and [bool]$inferenceQwenExit.passed
    }
    $results = [ordered]@{}
    foreach ($caseId in @($groupCases[$ProducerKey])) {
        $checks = $ftChecks[$caseId]
        $results[$caseId] = New-CaseResult $Context $caseId $(if (Test-AllTrue $checks) { 'PASS' } else { 'FAIL' }) $checks `
            ([ordered]@{ moss_run_id_sha256 = if ($caseId -eq 'FT-04') { (Get-FileHash $complete.output.path -Algorithm SHA256).Hash } else { $null }; helper_process_count = $helpers.Count
                enhance_feedback_p50_ms = if ($caseId -eq 'FT-04') { [double]$enhanceFeedbackPerformance.p50_ms } else { $null }
                cancel_feedback_p50_ms = if ($caseId -eq 'FT-05') { [double]$cancelFeedbackPerformance.p50_ms } else { $null }
                moss_and_qwen_overlap_seconds = if ($caseId -eq 'FT-12') { [double]$inferenceExclusion.moss_and_qwen_overlap_seconds } else { $null }
                residual_process_count = $helpers.Count }) `
            ([ordered]@{ cancel = $cancel; cancel_trials = $cancelActions; cancel_helper_exits = $cancelHelperExits;
                enhance_feedback_performance = $enhanceFeedbackPerformance; cancel_feedback_performance = $cancelFeedbackPerformance;
                complete = $complete; native_decode_contract = $nativeDecodeEvidence;
                complete_helper_exit = $completeHelperExit; review = $review; persistence = $persist; activate = $activate;
                summary_moss = $summaryMoss; rollback = $rollback; summary_whisper = $summaryWhisper;
                reactivate = $reactivate; summary_reactivated = $summaryReactivated;
                summary_helper_exits = @($summaryMossExit, $summaryWhisperExit, $summaryReactivatedExit);
                inference_exclusion_action = $inferenceExclusionAction; inference_exclusion = $inferenceExclusion;
                helpers = $helpers })
    }
    return $results
}

function Invoke-InstallLifecycleGroup {
    param($Context, $State)
    $inputs = Get-GroupScenarioInputs $Context
    if ($inputs.Count -ne 2) { throw 'FT-22 group requires baseline installer and fixture manifest.' }
    $baselineBuild = Assert-BoundFile $Context.group_input.baseline_build_manifest 'baseline build manifest'
    $baselineDocument = (Read-JsonObject $baselineBuild.path 'baseline build manifest').value
    if ([string]$baselineDocument.role -ne 'baseline' -or ([string]$baselineDocument.source_commit).ToLowerInvariant() -ne $approvedBaselineCommit -or
        -not ([string]$baselineDocument.installer.sha256).Equals($inputs[0].sha256, [System.StringComparison]::OrdinalIgnoreCase)) {
        throw 'Baseline build manifest is not bound to the approved baseline installer/commit.'
    }
    $protectedBaseline = Assert-BoundFile $Context.group_input.protected_baseline_manifest 'protected baseline manifest'
    $fixtureDatabase = Assert-BoundFile $Context.group_input.fixture_database 'lifecycle fixture database'
    $python = Assert-BoundFile $Context.sidecar.python 'Python runtime'
    $privateLifecycleRoot = Join-Path $Context.state_root ($ProducerKey + '.lifecycle.private')
    $publicLifecycle = Join-Path $Context.state_root ($ProducerKey + '.lifecycle.public.json')
    if ((Test-Path -LiteralPath $privateLifecycleRoot) -or (Test-Path -LiteralPath $publicLifecycle)) { throw 'Lifecycle raw output already exists.' }
    [System.IO.Directory]::CreateDirectory($privateLifecycleRoot) | Out-Null
    $arguments = @(
        '-NoProfile', '-NonInteractive', '-ExecutionPolicy', 'Bypass', '-File', $lifecycleScript,
        '-CandidateInstaller', $Context.candidate.path,
        '-BaselineInstaller', $inputs[0].path,
        '-OutputRoot', $privateLifecycleRoot,
        '-PublicOutput', $publicLifecycle,
        '-ProtectedBaselinePath', $protectedBaseline.path,
        '-FixtureDatabase', $fixtureDatabase.path,
        '-FixtureManifest', $inputs[1].path,
        '-PythonPath', $python.path,
        '-CandidateBuildManifest', $Context.build_manifest_record.path,
        '-BaselineBuildManifest', $baselineBuild.path,
        '-ProductName', $approvedProductName,
        '-BundleId', $approvedBundleId,
        '-CandidateVersion', [string]$Context.build_manifest.version,
        '-BaselineVersion', [string]$baselineDocument.version,
        '-RunId', $Context.run_id,
        '-OldVersionStableSeconds', '30',
        '-CandidateStableSeconds', '15'
    )
    $windowsPowerShell = Join-Path $env:SystemRoot 'System32\WindowsPowerShell\v1.0\powershell.exe'
    $run = Invoke-NativeCapture $windowsPowerShell $arguments 'single hardened install lifecycle' 10800 -AllowFailure
    $public = (Read-JsonObject $publicLifecycle 'lifecycle public result').value
    if ([string]$public.run_id -ne $Context.run_id -or ([string]$public.source_commit).ToLowerInvariant() -ne $Context.source_commit -or
        -not ([string]$public.candidate.evidence.sha256).Equals($Context.candidate.sha256, [System.StringComparison]::OrdinalIgnoreCase) -or
        -not ([string]$public.candidate.build_manifest.sha256).Equals($Context.build_manifest_record.sha256, [System.StringComparison]::OrdinalIgnoreCase)) {
        throw 'Lifecycle report is not bound to this acceptance candidate and manifest.'
    }
    $results = [ordered]@{}
    foreach ($number in 22..25) {
        $caseId = 'FT-{0:D2}' -f $number
        $property = 'FT_{0:D2}' -f $number
        $case = $public.cases.PSObject.Properties[$property].Value
        $checks = [ordered]@{
            lifecycle_process_exit_zero = [int]$run.exit_code -eq 0
            lifecycle_case_pass = [string]$case.status -eq 'PASS'
            lifecycle_functional_pass = [string]$public.functional_status -eq 'PASS'
            lifecycle_cleanup_pass = [string]$public.cleanup.status -eq 'PASS'
            lifecycle_residual_processes_zero = [int]$public.cleanup.residual_process_count -eq 0
            candidate_manifest_bound = [string]$public.candidate.build_manifest.sha256 -eq $Context.build_manifest_record.sha256
            baseline_manifest_bound = [string]$public.baseline.build_manifest.sha256 -eq $baselineBuild.sha256
        }
        $metrics = [ordered]@{ lifecycle_schema_version = [int]$public.schema_version; lifecycle_case = $case; lifecycle_public_sha256 = (Get-FileRecord $publicLifecycle).sha256 }
        $results[$caseId] = New-CaseResult $Context $caseId $(if (Test-AllTrue $checks) { 'PASS' } else { 'FAIL' }) $checks $metrics `
            ([ordered]@{ invocation = $run; lifecycle_public = $public; lifecycle_private_root = $privateLifecycleRoot })
    }
    return $results
}

function Invoke-DataPlacementGroup {
    param($Context, $State)
    Ensure-CandidateInstalled $Context $State
    $State = Start-ExactCandidateApp $Context $State
    $inputs = Get-GroupScenarioInputs $Context
    $selected = Get-NormalizedFullPath ([string]$Context.group_input.selected_recording_root)
    $default = Get-NormalizedFullPath ([string]$Context.group_input.default_recording_root)
    $invalid = Get-NormalizedFullPath ([string]$Context.group_input.invalid_recording_root)
    foreach ($root in @($selected, $default)) {
        if (-not (Test-StrictChildPath $root $Context.private_root)) { throw 'FT-26 selected/default roots must be under private evidence root.' }
        if (-not (Test-Path -LiteralPath $root -PathType Container)) { [System.IO.Directory]::CreateDirectory($root) | Out-Null }
    }
    $defaultBefore = @(Get-DirectoryManifest $default)
    $set = Invoke-CdpAction $Context $State 'recording-preferences-set' 'data-drive-set' ([ordered]@{ recording_root = $selected }) 120
    if (-not [bool]$set.value.applied) { throw 'Selected recording root was not saved.' }
    Stop-ExactCandidateApp $Context (Read-State $Context) | Out-Null
    $State = Start-ExactCandidateApp $Context (Read-State $Context)
    $duration = [double]$Context.group_input.short_duration_seconds
    $record = Invoke-RecordedAudioMeeting $Context $State $inputs[0].path $duration 'data-drive' ('MOSS-FT-DATA-' + $Context.run_id)
    $selectedAfter = @(Get-DirectoryManifest $selected)
    $defaultAfter = @(Get-DirectoryManifest $default)
    $invalidBeforeParent = Split-Path -Parent $invalid
    $invalidManifestBefore = @(Get-DirectoryManifest $invalidBeforeParent)
    $invalidAttempt = Invoke-CdpAction $Context (Read-State $Context) 'recording-preferences-invalid' 'data-drive-invalid' ([ordered]@{ recording_root = $invalid }) 120
    $invalidManifestAfter = @(Get-DirectoryManifest $invalidBeforeParent)
    $checks = [ordered]@{
        selected_root_saved = [bool]$set.value.applied
        selected_root_survived_restart = (Get-NormalizedFullPath ([string]$record.start.value.folder)).StartsWith($selected + '\', [System.StringComparison]::OrdinalIgnoreCase)
        selected_root_contains_recording = $selectedAfter.Count -gt 0
        default_root_manifest_unchanged = ($defaultBefore | ConvertTo-Json -Depth 8 -Compress) -eq ($defaultAfter | ConvertTo-Json -Depth 8 -Compress)
        invalid_root_explicitly_rejected = [bool]$invalidAttempt.value.verdict.explicitlyRejected
        preference_unchanged_after_invalid = [bool]$invalidAttempt.value.verdict.preferenceUnchanged
        no_invalid_half_product = ($invalidManifestBefore | ConvertTo-Json -Depth 8 -Compress) -eq ($invalidManifestAfter | ConvertTo-Json -Depth 8 -Compress)
    }
    $result = New-CaseResult $Context 'FT-26' $(if (Test-AllTrue $checks) { 'PASS' } else { 'FAIL' }) $checks ([ordered]@{
        selected_file_count = $selectedAfter.Count; default_file_count = $defaultAfter.Count
        input_sha256 = $inputs[0].sha256
    }) ([ordered]@{ set = $set; record = $record; invalid = $invalidAttempt; selected_manifest = $selectedAfter; default_manifest = $defaultAfter })
    return [ordered]@{ 'FT-26' = $result }
}

function Invoke-LongAudioGroup {
    param($Context, $State)
    Ensure-CandidateInstalled $Context $State
    $State = Start-ExactCandidateApp $Context $State
    $inputs = Get-GroupScenarioInputs $Context
    if ($inputs.Count -ne 2) { throw 'FT-27 requires long WAV and chunk manifest.' }
    $duration = [double]$Context.group_input.expected_duration_seconds
    if ([math]::Abs($duration - 3096.62) -gt 0.001) { throw 'FT-27 duration must be exactly 3096.62 seconds.' }
    $record = Invoke-RecordedAudioMeeting $Context $State $inputs[0].path $duration 'long-3096' ('MOSS-FT-LONG-' + $Context.run_id)
    Start-Sleep -Seconds 5
    $stableSnapshot = Invoke-CdpAction $Context (Read-State $Context) 'meeting-snapshot' 'long-stable-after-five-seconds' `
        ([ordered]@{ expected_duration_seconds = $duration }) 120
    $finalization = New-MossRecordingFinalizationEvidence $record $stableSnapshot $inputs[1].path $duration $true
    $complete = Invoke-CdpAction $Context (Read-State $Context) 'moss-complete' 'long-moss-complete' ([ordered]@{ timeout_ms = 10800000; expected_duration_seconds = $duration }) 11000
    Assert-MossPerformanceSample $complete 'enhance_feedback' 'long-moss-enhance-feedback' | Out-Null
    $nativeDecodeEvidence = Assert-MossNativeDecodeEvidence $complete
    $mossHelperExit = Assert-MossProcessRoleExited $complete 'moss'
    Start-Sleep -Seconds 3
    $helpers = @(Get-ExactProductProcesses (Read-State $Context) | Where-Object { $_.executable_path -match '(?i)(moss-helper|llama-helper)\.exe$' })
    $audit = $complete.value.candidate_time_audit
    $tailDifference = if ($null -ne $audit.lastEnd) { [math]::Abs($duration - [double]$audit.lastEnd) } else { [double]::PositiveInfinity }
    $checks = [ordered]@{
        full_audio_played_once = [double]$record.playback_elapsed_seconds -ge ($duration - 1.5)
        moss_completed = [bool]$complete.value.verdict.completed
        all_timestamps_parseable = [bool]$audit.finite
        all_timestamps_legal = [bool]$audit.legal
        all_timestamps_monotonic = [bool]$audit.monotonic
        candidate_segments_nonempty = [int]$audit.count -gt 0
        tail_difference_at_most_half_second = $tailDifference -le 0.5
        tail_marker_present_exactly_once = [bool]$finalization.tail_marker_present_exactly_once
        tail_coverage_ratio_at_least_98_percent = [bool]$finalization.tail_coverage_at_least_98_percent
        chunks_in_queue_at_finalization_zero = [bool]$finalization.chunks_in_queue_at_finalization_zero
        transcription_worker_idle_at_finalization = [bool]$finalization.transcription_worker_idle_at_finalization
        finalization_completed = [bool]$finalization.finalization_completed
        transcript_stable_after_five_seconds = [bool]$finalization.transcript_stable_after_five_seconds
        pause_observed = [bool]$finalization.pause_observed
        transcript_continues_after_resume = [bool]$finalization.transcript_continues_after_resume
        moss_helper_tree_zero_within_five_seconds = [bool]$mossHelperExit.passed
        native_decode_contract_pass = [bool]$nativeDecodeEvidence.passed
        helper_process_count_zero = $helpers.Count -eq 0
    }
    $result = New-CaseResult $Context 'FT-27' $(if (Test-AllTrue $checks) { 'PASS' } else { 'FAIL' }) $checks ([ordered]@{
        expected_duration_seconds = $duration; playback_elapsed_seconds = [double]$record.playback_elapsed_seconds
        candidate_segment_count = [int]$audit.count; last_end_seconds = $audit.lastEnd; tail_difference_seconds = $tailDifference
        tail_coverage_ratio = [double]$finalization.tail_coverage_ratio
        chunks_in_queue_at_finalization = [int]$finalization.chunks_in_queue_at_finalization
        long_input_sha256 = $inputs[0].sha256; chunk_manifest_sha256 = $inputs[1].sha256; helper_process_count = $helpers.Count
    }) ([ordered]@{ recording = $record; stable_snapshot = $stableSnapshot; finalization = $finalization;
        moss = $complete; native_decode_contract = $nativeDecodeEvidence;
        moss_helper_exit = $mossHelperExit; helpers = $helpers })
    return [ordered]@{ 'FT-27' = $result }
}

function Invoke-QualityVerify {
    param($Context, $Q00)
    $python = Assert-BoundFile $Context.sidecar.python 'Python runtime'
    $roleArguments = [ordered]@{
        bindings = '--bindings'; window_audio = '--window-audio'; window_manifest = '--window-manifest'
        moss_raw = '--moss-raw'; whisper_same_window = '--whisper-same-window'; corrected = '--corrected'
        human_verbatim = '--human-verbatim'; speaker_truth = '--speaker-truth'; positive_truth = '--positive-truth'
        negative_truth = '--negative-truth'; activation_evidence = '--activation-evidence'; summary_evidence = '--summary-evidence'
        public_report = '--public-report'; private_report = '--private-report'
    }
    $arguments = @($qualityGateScript, 'verify', '--repo', $repoRoot, '--source-commit', $Context.source_commit)
    $bound = [ordered]@{}
    foreach ($role in $roleArguments.Keys) {
        $property = $Q00.PSObject.Properties[$role]
        if ($null -eq $property) { throw "Q00 input lacks $role." }
        $record = Assert-BoundFile $property.Value "Q00 $role"
        $bound[$role] = $record
        $arguments += @([string]$roleArguments[$role], $record.path)
    }
    $run = Invoke-NativeCapture $python.path $arguments 'Q00 independent verify' 1800 -AllowFailure
    $public = (Read-JsonObject $bound.public_report.path 'Q00 public report').value
    if ([string]$public.source_commit -ne $Context.source_commit) { throw 'Q00 report source_commit does not match the acceptance source.' }
    $bindingsDocument = (Read-JsonObject $bound.bindings.path 'Q00 bindings').value
    $allowedCandidateProducerHashes = @(
        @($Context.build_manifest.installed_files | Where-Object { [string]$_.role -in @('main_executable', 'moss_helper') } | ForEach-Object {
            ([string]$_.sha256).ToUpperInvariant()
        })
    )
    $currentRoles = @('moss_raw', 'whisper_same_window', 'corrected', 'activation_evidence', 'summary_evidence')
    $candidateProducerChecks = [ordered]@{}
    foreach ($role in $currentRoles) {
        $artifactProperty = $bindingsDocument.artifacts.PSObject.Properties[$role]
        $producerSha = if ($null -ne $artifactProperty) { ([string]$artifactProperty.Value.producer.sha256).ToUpperInvariant() } else { '' }
        $candidateProducerChecks[$role] = $allowedCandidateProducerHashes -contains $producerSha
    }
    return [ordered]@{
        run = $run
        public = $public
        bound = $bound
        candidate_producer_checks = $candidateProducerChecks
        all_current_artifacts_from_candidate = Test-AllTrue $candidateProducerChecks
    }
}

function Invoke-BusinessGroup {
    param($Context, $State)
    Ensure-CandidateInstalled $Context $State
    $State = Start-ExactCandidateApp $Context $State
    $inputs = Get-GroupScenarioInputs $Context
    if ($inputs.Count -ne 4) { throw 'FT-28 requires business WAV, reference, positive truth and negative truth.' }
    $duration = [double]$Context.group_input.expected_duration_seconds
    if ([math]::Abs($duration - 737.728) -gt 0.001) { throw 'FT-28 duration must be exactly 737.728 seconds.' }
    $record = Invoke-RecordedAudioMeeting $Context $State $inputs[0].path $duration 'business-737' ('MOSS-FT-BUSINESS-' + $Context.run_id)
    Start-Sleep -Seconds 5
    $stableSnapshot = Invoke-CdpAction $Context (Read-State $Context) 'meeting-snapshot' 'business-stable-after-five-seconds' `
        ([ordered]@{ expected_duration_seconds = $duration }) 120
    $finalization = New-MossRecordingFinalizationEvidence $record $stableSnapshot $inputs[1].path $duration $true
    $complete = Invoke-CdpAction $Context (Read-State $Context) 'moss-complete' 'business-moss-complete' ([ordered]@{ timeout_ms = 7200000; expected_duration_seconds = $duration }) 7500
    Assert-MossPerformanceSample $complete 'enhance_feedback' 'business-moss-enhance-feedback' | Out-Null
    $nativeDecodeEvidence = Assert-MossNativeDecodeEvidence $complete
    $mossHelperExit = Assert-MossProcessRoleExited $complete 'moss'
    $review = Invoke-CdpAction $Context (Read-State $Context) 'moss-review-strict' 'business-review' ([ordered]@{
        positive_terms = @($Context.group_input.positive_terms); negative_terms = @($Context.group_input.negative_terms)
    }) 300
    $activate = Invoke-CdpAction $Context (Read-State $Context) 'moss-activate' 'business-activate' ([ordered]@{}) 180
    $summary = Invoke-CdpAction $Context (Read-State $Context) 'summary-generate' 'business-summary' ([ordered]@{ timeout_ms = 180000 }) 240 -ModelState 'warm'
    Assert-MossPerformanceSample $summary 'summary_completion' 'business-summary' | Out-Null
    $summaryHelperExit = Assert-MossProcessRoleExited $summary 'qwen'
    $quality = Invoke-QualityVerify $Context $Context.group_input.q00
    $q00WindowManifest = (Read-JsonObject $quality.bound.window_manifest.path 'Q00 window manifest').value
    $q00SourceBinding = Get-PropertyValue -Object $q00WindowManifest -Names @('source_audio', 'source_wav', 'source')
    $q00SourceSha = Get-PropertyValue -Object $q00SourceBinding -Names @('sha256')
    $q00InputsMatchFt28 = [string]$q00SourceSha -eq [string]$inputs[0].sha256 -and
        [string]$quality.bound.human_verbatim.sha256 -eq [string]$inputs[1].sha256 -and
        [string]$quality.bound.positive_truth.sha256 -eq [string]$inputs[2].sha256 -and
        [string]$quality.bound.negative_truth.sha256 -eq [string]$inputs[3].sha256
    $q00WindowDuration = [double]$quality.public.scope.duration_seconds
    Start-Sleep -Seconds 3
    $helpers = @(Get-ExactProductProcesses (Read-State $Context) | Where-Object { $_.executable_path -match '(?i)(moss-helper|llama-helper)\.exe$' })
    $metrics = $quality.public.metrics
    $checks = [ordered]@{
        full_business_audio_played = [double]$record.playback_elapsed_seconds -ge ($duration - 1.5)
        product_moss_completed = [bool]$complete.value.verdict.completed
        strict_corrections_and_speaker_override_completed = Test-AllTrue $review.value.verdict
        candidate_activated = [bool]$activate.value.verdict.activated
        real_qwen_summary_completed = [bool]$summary.value.verdict.completed
        summary_source_is_active_moss = (Get-SummarySource $summary) -eq 'moss'
        q00_verify_exit_zero = [int]$quality.run.exit_code -eq 0
        q00_status_pass = [string]$quality.public.status -eq 'PASS'
        q00_truth_and_source_match_ft28_inputs = $q00InputsMatchFt28
        q00_window_duration_is_226_440_seconds = [math]::Abs($q00WindowDuration - 226.440) -le 0.000001
        q00_current_artifacts_bound_to_candidate_files = [bool]$quality.all_current_artifacts_from_candidate
        moss_cer_at_most_20_percent = [double]$metrics.moss_raw_cer -le 0.20
        moss_not_worse_than_whisper = [double]$metrics.moss_raw_cer -le [double]$metrics.whisper_same_window_cer
        corrected_cer_at_most_15_percent = [double]$metrics.corrected_cer -le 0.15
        positive_term_accuracy_at_least_95_percent = [double]$metrics.positive_term_accuracy -ge 0.95
        negative_term_insertions_zero = [int]$metrics.negative_term_insertions -eq 0
        moss_rtf_at_most_one = [double]$metrics.moss_rtf -le 1.0
        manual_speaker_coverage_error_zero = [int]$metrics.manual_single_segment_coverage_error_count -eq 0
        tail_marker_present_exactly_once = [bool]$finalization.tail_marker_present_exactly_once
        tail_coverage_ratio_at_least_98_percent = [bool]$finalization.tail_coverage_at_least_98_percent
        chunks_in_queue_at_finalization_zero = [bool]$finalization.chunks_in_queue_at_finalization_zero
        transcription_worker_idle_at_finalization = [bool]$finalization.transcription_worker_idle_at_finalization
        finalization_completed = [bool]$finalization.finalization_completed
        transcript_stable_after_five_seconds = [bool]$finalization.transcript_stable_after_five_seconds
        pause_observed = [bool]$finalization.pause_observed
        transcript_continues_after_resume = [bool]$finalization.transcript_continues_after_resume
        moss_helper_tree_zero_within_five_seconds = [bool]$mossHelperExit.passed
        summary_helper_tree_zero_within_five_seconds = [bool]$summaryHelperExit.passed
        helper_process_count_zero = $helpers.Count -eq 0
        native_decode_contract_pass = [bool]$nativeDecodeEvidence.passed
    }
    $publicMetrics = [ordered]@{
        business_audio_duration_seconds = $duration; playback_elapsed_seconds = [double]$record.playback_elapsed_seconds
        q00_window_duration_seconds = $q00WindowDuration
        q00_moss_raw_cer = [double]$metrics.moss_raw_cer; q00_whisper_same_window_cer = [double]$metrics.whisper_same_window_cer
        q00_corrected_cer = [double]$metrics.corrected_cer; q00_positive_term_accuracy = [double]$metrics.positive_term_accuracy
        q00_negative_term_insertions = [int]$metrics.negative_term_insertions; q00_moss_rtf = [double]$metrics.moss_rtf
        q00_manual_single_segment_coverage_error_count = [int]$metrics.manual_single_segment_coverage_error_count
        tail_coverage_ratio = [double]$finalization.tail_coverage_ratio
        chunks_in_queue_at_finalization = [int]$finalization.chunks_in_queue_at_finalization
        business_input_sha256 = $inputs[0].sha256; q00_human_verbatim_sha256 = $inputs[1].sha256
        positive_truth_sha256 = $inputs[2].sha256; negative_truth_sha256 = $inputs[3].sha256
        q00_public_report_sha256 = $quality.bound.public_report.sha256; helper_process_count = $helpers.Count
    }
    $result = New-CaseResult $Context 'FT-28' $(if (Test-AllTrue $checks) { 'PASS' } else { 'FAIL' }) $checks $publicMetrics `
        ([ordered]@{ recording = $record; stable_snapshot = $stableSnapshot; finalization = $finalization;
            moss = $complete; native_decode_contract = $nativeDecodeEvidence;
            moss_helper_exit = $mossHelperExit; review = $review; activate = $activate;
            summary = $summary; summary_helper_exit = $summaryHelperExit; quality = $quality; helpers = $helpers })
    return [ordered]@{ 'FT-28' = $result }
}

function Invoke-FaultChainGroup {
    param($Context, $State)
    $sandboxExe = Join-Path $env:SystemRoot 'System32\WindowsSandbox.exe'
    if (-not (Test-Path -LiteralPath $sandboxExe -PathType Leaf)) { throw 'Windows Sandbox is not installed; FT-16 through FT-21 cannot run on the host.' }
    $launcher = Assert-BoundFile $Context.group_input.sandbox_launcher 'sandbox launcher'
    if (-not $launcher.path.Equals((Get-NormalizedFullPath $fixedSandboxLauncher), [System.StringComparison]::OrdinalIgnoreCase)) {
        throw 'Fault chain must use the fixed repository Windows Sandbox launcher.'
    }
    $exchange = Get-NormalizedFullPath ([string]$Context.group_input.sandbox_exchange_root)
    if (-not (Test-StrictChildPath $exchange $Context.private_root)) { throw 'Sandbox exchange root must be below the private evidence root.' }
    if (-not (Test-Path -LiteralPath $exchange -PathType Container)) { [System.IO.Directory]::CreateDirectory($exchange) | Out-Null }
    Assert-NoReparsePath $exchange $Context.private_root | Out-Null
    $resultPath = Join-Path $exchange 'fault-chain-result.private.json'
    if (Test-Path -LiteralPath $resultPath) { throw 'Sandbox fault result already exists.' }
    $firewallBefore = Get-HostFirewallSnapshot
    $windowsPowerShell = Join-Path $env:SystemRoot 'System32\WindowsPowerShell\v1.0\powershell.exe'
    $run = Invoke-NativeCapture $windowsPowerShell @(
        '-NoLogo', '-NoProfile', '-NonInteractive', '-ExecutionPolicy', 'Bypass', '-File', $launcher.path,
        '-AcceptanceConfig', $Context.config_path,
        '-ProducerInput', $Context.sidecar_path,
        '-Output', $resultPath,
        '-RunId', $Context.run_id,
        '-SourceCommit', $Context.source_commit,
        '-CandidateSha256', $Context.candidate.sha256
    ) 'Windows Sandbox fault-chain launcher' 10800 -AllowFailure
    $firewallAfter = Get-HostFirewallSnapshot
    $hostFirewallUntouched = [string]$firewallBefore.sha256 -eq [string]$firewallAfter.sha256 -and
        [int]$firewallBefore.profile_count -eq [int]$firewallAfter.profile_count -and
        [int]$firewallBefore.rule_count -eq [int]$firewallAfter.rule_count
    if (-not (Test-Path -LiteralPath $resultPath -PathType Leaf)) { throw 'Windows Sandbox did not produce the bound fault result.' }
    $sandbox = (Read-JsonObject $resultPath 'sandbox fault result').value
    if ([string]$sandbox.stage -ne 'MOSS_FUNCTIONAL_FT_SANDBOX_FAULT_RESULT' -or
        [string]$sandbox.run_id -ne $Context.run_id -or ([string]$sandbox.source_commit).ToLowerInvariant() -ne $Context.source_commit -or
        -not ([string]$sandbox.candidate_sha256).Equals($Context.candidate.sha256, [System.StringComparison]::OrdinalIgnoreCase) -or
        [bool]$sandbox.ran_on_host -or -not [bool]$sandbox.windows_sandbox -or -not [bool]$sandbox.is_admin) {
        throw 'Sandbox fault result identity/environment binding is invalid.'
    }
    Assert-ExactProperties $sandbox.cases @($faultCaseCheckKeys.Keys) 'Sandbox fault cases'
    if ([string]$sandbox.cleanup.status -ne 'PASS' -or @($sandbox.cleanup.errors).Count -ne 0 -or @($sandbox.cleanup.final_processes).Count -ne 0 -or
        -not [bool]$sandbox.cleanup.firewall_rules_removed -or @($sandbox.errors).Count -ne 0) {
        throw 'Sandbox worker cleanup or top-level execution failed.'
    }
    $worker = Assert-BoundFile ([ordered]@{
        path = Join-Path $scriptRoot 'moss-functional-ft-sandbox-worker.ps1'
        bytes = $sandbox.environment.worker.bytes
        sha256 = $sandbox.environment.worker.sha256
    }) 'fixed Sandbox worker'
    foreach ($role in $requiredInstalledRoles.Keys) {
        $expected = @($Context.build_manifest.installed_files | Where-Object { [string]$_.role -eq $role })[0]
        $actual = $sandbox.installed_candidate.PSObject.Properties[$role]
        if ($null -eq $actual -or [int64]$actual.Value.bytes -ne [int64]$expected.bytes -or
            -not ([string]$actual.Value.sha256).Equals([string]$expected.sha256, [System.StringComparison]::OrdinalIgnoreCase)) {
            throw "Sandbox installed candidate is not build-manifest bound: $role"
        }
    }
    $results = [ordered]@{}
    foreach ($number in 16..21) {
        $caseId = 'FT-{0:D2}' -f $number
        $property = $sandbox.cases.PSObject.Properties[$caseId]
        if ($null -eq $property) { throw "Sandbox result omitted $caseId." }
        $case = $property.Value
        $caseChecks = $case.checks
        Assert-ExactProperties $caseChecks @($faultCaseCheckKeys[$caseId]) "$caseId Sandbox checks"
        $mossResidual = [int]$case.metrics.residual_moss_helper_count
        $llamaResidual = [int]$case.metrics.residual_llama_helper_count
        $checks = [ordered]@{
            sandbox_launcher_exit_zero = [int]$run.exit_code -eq 0
            windows_sandbox_proven = [bool]$sandbox.windows_sandbox -and -not [bool]$sandbox.ran_on_host
            sandbox_admin_proven = [bool]$sandbox.is_admin
            fixed_worker_hash_bound = $worker.sha256 -eq [string]$sandbox.environment.worker.sha256
            host_firewall_untouched = $hostFirewallUntouched
            sandbox_cleanup_pass = [string]$sandbox.cleanup.status -eq 'PASS'
            exact_case_facts_all_true = Test-AllTrue $caseChecks
            residual_moss_helpers_zero = $mossResidual -eq 0
            residual_llama_helpers_zero = $llamaResidual -eq 0
        }
        $results[$caseId] = New-CaseResult $Context $caseId $(if (Test-AllTrue $checks) { 'PASS' } else { 'FAIL' }) $checks `
            ([ordered]@{
                sandbox_result_sha256 = (Get-FileRecord $resultPath).sha256
                host_firewall_before_sha256 = $firewallBefore.sha256
                host_firewall_after_sha256 = $firewallAfter.sha256
                residual_moss_helper_count = $mossResidual
                residual_llama_helper_count = $llamaResidual
            }) `
            ([ordered]@{ launcher = $run; sandbox = $sandbox; case = $case; host_firewall_before = $firewallBefore; host_firewall_after = $firewallAfter })
    }
    return $results
}

function Invoke-GroupRun {
    param($Context, $State)
    switch ($ProducerKey) {
        'ft01-03-live-and-persistence' { return Invoke-LivePersistenceGroup $Context $State }
        'ft04-15-moss-chain' { return Invoke-MossChainGroup $Context $State }
        'ft16-21-fault-chain' { return Invoke-FaultChainGroup $Context $State }
        'ft22-25-install-lifecycle' { return Invoke-InstallLifecycleGroup $Context $State }
        'ft26-data-drive-placement' { return Invoke-DataPlacementGroup $Context $State }
        'ft27-long-audio' { return Invoke-LongAudioGroup $Context $State }
        'ft28-business-chain' { return Invoke-BusinessGroup $Context $State }
        default { throw "Unsupported producer key: $ProducerKey" }
    }
}

function Write-FailureResults {
    param($Context, [Parameter(Mandatory = $true)][string]$ErrorCode, [Parameter(Mandatory = $true)][string]$PrivateError)
    $results = [ordered]@{}
    foreach ($caseId in @($groupCases[$ProducerKey])) {
        $checks = [ordered]@{ producer_completed = $false; evidence_complete = $false; no_hard_gate_failed = $false }
        $results[$caseId] = New-CaseResult $Context $caseId 'FAIL' $checks ([ordered]@{ error_code = $ErrorCode }) `
            ([ordered]@{ error_code = $ErrorCode; error = $PrivateError; failed_at = [datetimeoffset]::UtcNow.ToString('o') })
    }
    Write-CaseResults $Context $results
}

function Invoke-ExactUninstallAndArchive {
    param($Context, $State)
    $record = [ordered]@{ uninstall = $null; archives = @(); errors = @() }
    $ownedRoots = @(
        @{ label = 'data'; path = [string]$State.data_root; boundary = $env:APPDATA },
        @{ label = 'webview'; path = [string]$State.webview_root; boundary = $env:LOCALAPPDATA },
        @{ label = 'rollback-backups'; path = [string]$State.backup_root; boundary = $env:APPDATA }
    )
    foreach ($entry in $ownedRoots) {
        try { Assert-OwnedIsolatedRoot -Context $Context -Root $entry.path -Label $entry.label -Boundary $entry.boundary | Out-Null }
        catch { $record.errors += $_.Exception.Message }
    }
    if ($record.errors.Count -ne 0) { return $record }
    if (Test-Path -LiteralPath ([string]$State.registry_path)) {
        try {
            Assert-InstalledCandidate -Context $Context -State $State
            $uninstaller = Get-InstalledFilePath $State 'uninstaller'
            $record.uninstall = Invoke-NativeCapture $uninstaller @('/S') 'exact isolated uninstall' 1200 -AllowFailure
            if ([int]$record.uninstall.exit_code -ne 0) { $record.errors += "Exact isolated uninstall exited with code $($record.uninstall.exit_code)." }
            $deadline = (Get-Date).AddSeconds(30)
            while ((Get-Date) -lt $deadline -and (
                (Test-Path -LiteralPath ([string]$State.registry_path)) -or
                (Test-Path -LiteralPath ([string]$State.install_root))
            )) { Start-Sleep -Milliseconds 250 }
            if (Test-Path -LiteralPath ([string]$State.registry_path)) { $record.errors += 'Exact isolated uninstall left its registry key.' }
            if (Test-Path -LiteralPath ([string]$State.install_root)) { $record.errors += 'Exact isolated uninstall left its install root.' }
        } catch { $record.errors += $_.Exception.Message }
    }
    $archiveRoot = Join-Path $Context.state_root ($ProducerKey + '.cleanup-archive.private')
    if (-not (Test-Path -LiteralPath $archiveRoot -PathType Container)) { [System.IO.Directory]::CreateDirectory($archiveRoot) | Out-Null }
    foreach ($entry in $ownedRoots) {
        if (Test-Path -LiteralPath $entry.path -PathType Container) {
            try {
                Assert-OwnedIsolatedRoot -Context $Context -Root $entry.path -Label $entry.label -Boundary $entry.boundary | Out-Null
                $destination = Join-Path $archiveRoot $entry.label
                if (Test-Path -LiteralPath $destination) { throw "Cleanup archive destination exists: $destination" }
                Move-Item -LiteralPath $entry.path -Destination $destination
                $record.archives += [ordered]@{ label = $entry.label; file_count = @(Get-DirectoryManifest $destination).Count }
            } catch { $record.errors += $_.Exception.Message }
        }
    }
    return $record
}

function Invoke-GroupCleanup {
    param($Context, $State)
    $actions = @()
    $errors = @()
    try { $actions += @(Stop-ExactCandidateApp $Context $State) } catch { $errors += $_.Exception.Message }
    $State = Read-State $Context
    $hostAppGroups = @(
        'ft01-03-live-and-persistence', 'ft04-15-moss-chain', 'ft26-data-drive-placement',
        'ft27-long-audio', 'ft28-business-chain'
    )
    $failedOwnedHostRun = $ProducerKey -in $hostAppGroups -and -not [bool]$State.run_passed -and [bool]$State.isolated_roots_owned
    if ($ProducerKey -in @('ft16-21-fault-chain', 'ft28-business-chain') -or $failedOwnedHostRun) {
        try {
            $uninstallAction = Invoke-ExactUninstallAndArchive $Context $State
            $actions += $uninstallAction
            $errors += @($uninstallAction.errors)
        } catch { $errors += $_.Exception.Message }
    } elseif ($ProducerKey -in $hostAppGroups -and -not [bool]$State.run_passed) {
        $footprintExists = (Test-Path -LiteralPath ([string]$State.registry_path)) -or
            (Test-Path -LiteralPath ([string]$State.install_root)) -or (Test-Path -LiteralPath ([string]$State.data_root)) -or
            (Test-Path -LiteralPath ([string]$State.webview_root)) -or (Test-Path -LiteralPath ([string]$State.backup_root))
        if ($footprintExists) { $errors += 'Failed host run left an unowned or partially owned isolated footprint; cleanup refused to remove it.' }
    }
    $scans = @()
    for ($attempt = 1; $attempt -le 2; $attempt++) {
        $processes = @(Get-ExactProductProcesses $State)
        $listeners = if ($null -ne $State.cdp_port) { @(Get-NetTCPConnection -LocalPort ([int]$State.cdp_port) -State Listen -ErrorAction SilentlyContinue) } else { @() }
        $scans += [ordered]@{ attempt = $attempt; process_count = $processes.Count; listener_count = $listeners.Count; processes = $processes }
        if ($attempt -eq 1) { Start-Sleep -Seconds 2 }
    }
    $residualProcesses = [int]$scans[-1].process_count
    $residualListeners = [int]$scans[-1].listener_count
    $status = if ($errors.Count -eq 0 -and @($scans | Where-Object { $_.process_count -ne 0 -or $_.listener_count -ne 0 }).Count -eq 0) { 'PASS' } else { 'FAIL' }
    $private = [ordered]@{
        schema_version = 1; stage = 'MOSS_FUNCTIONAL_FT_CLEANUP_PRIVATE'; producer_key = $ProducerKey
        run_id = $Context.run_id; source_commit = $Context.source_commit; candidate_sha256 = $Context.candidate.sha256
        completed_at = [datetimeoffset]::UtcNow.ToString('o'); status = $status; actions = $actions; scans = $scans; errors = $errors
    }
    $privatePath = Resolve-SafeRelativePath $Context.private_root "cleanup/$ProducerKey.private.json"
    Write-JsonExclusive $private $privatePath
    $privateRecord = Get-FileRecord $privatePath
    $public = [ordered]@{
        schema_version = 1; stage = 'MOSS_FUNCTIONAL_FT_CLEANUP'; producer_key = $ProducerKey
        run_id = $Context.run_id; source_commit = $Context.source_commit; candidate_sha256 = $Context.candidate.sha256
        status = $status; residual_process_count = $residualProcesses; residual_cdp_listener_count = $residualListeners
        consecutive_zero_scans = @($scans | Where-Object { $_.process_count -eq 0 -and $_.listener_count -eq 0 }).Count
        private_result_bytes = $privateRecord.bytes; private_result_sha256 = $privateRecord.sha256
        completed_at = [datetimeoffset]::UtcNow.ToString('o')
    }
    Write-JsonExclusive $public (Resolve-SafeRelativePath $Context.public_root "cleanup/$ProducerKey.public.json")
    if ($status -ne 'PASS') { throw 'Exact group cleanup did not reach two consecutive zero-process/closed-port scans.' }
}

function Invoke-NoStateCleanup {
    param($Context)
    $transientState = New-InitialState $Context
    $scans = @()
    for ($attempt = 1; $attempt -le 2; $attempt++) {
        $processes = @(Get-ExactProductProcesses $transientState)
        $scans += [ordered]@{ attempt = $attempt; process_count = $processes.Count; listener_count = 0; processes = $processes }
        if ($attempt -eq 1) { Start-Sleep -Seconds 2 }
    }
    $status = if (@($scans | Where-Object { $_.process_count -ne 0 }).Count -eq 0) { 'PASS' } else { 'FAIL' }
    $private = [ordered]@{
        schema_version = 1; stage = 'MOSS_FUNCTIONAL_FT_CLEANUP_PRIVATE'; producer_key = $ProducerKey
        run_id = $Context.run_id; source_commit = $Context.source_commit; candidate_sha256 = $Context.candidate.sha256
        completed_at = [datetimeoffset]::UtcNow.ToString('o'); status = $status
        producer_state_existed = $false; actions = @(); scans = $scans; errors = @()
    }
    $privatePath = Resolve-SafeRelativePath $Context.private_root "cleanup/$ProducerKey.private.json"
    Write-JsonExclusive $private $privatePath
    $privateRecord = Get-FileRecord $privatePath
    $public = [ordered]@{
        schema_version = 1; stage = 'MOSS_FUNCTIONAL_FT_CLEANUP'; producer_key = $ProducerKey
        run_id = $Context.run_id; source_commit = $Context.source_commit; candidate_sha256 = $Context.candidate.sha256
        status = $status; residual_process_count = [int]$scans[-1].process_count; residual_cdp_listener_count = 0
        consecutive_zero_scans = @($scans | Where-Object { $_.process_count -eq 0 }).Count
        producer_state_existed = $false
        private_result_bytes = $privateRecord.bytes; private_result_sha256 = $privateRecord.sha256
        completed_at = [datetimeoffset]::UtcNow.ToString('o')
    }
    Write-JsonExclusive $public (Resolve-SafeRelativePath $Context.public_root "cleanup/$ProducerKey.public.json")
    if ($status -ne 'PASS') { throw 'Skipped group had residual exact product processes; no unowned PID was terminated.' }
}

$context = $null
try {
    $context = Read-ProducerContext
    if ($Mode -eq 'Run') {
        Assert-NoFormalEvidenceExists $context
        Assert-ProducerRunInputs $context
        $state = New-InitialState $context
        Write-JsonExclusive $state $context.state_path
        Write-JsonExclusive ([ordered]@{
            schema_version = 1; stage = 'MOSS_FUNCTIONAL_FT_RUN_LEASE'; run_id = $context.run_id
            source_commit = $context.source_commit; candidate_sha256 = $context.candidate.sha256
            producer_key = $ProducerKey; started_at = [datetimeoffset]::UtcNow.ToString('o')
        }) $context.lease_path
        try {
            $results = Invoke-GroupRun $context $state
            Write-CaseResults $context $results
            $state = Read-State $context
            $runPassed = @($results.Values | Where-Object { [string]$_.public.verdict -ne 'PASS' }).Count -eq 0
            $state.run_completed = $true
            $state.run_passed = $runPassed
            $state.completed_at = [datetimeoffset]::UtcNow.ToString('o')
            Save-State $context $state
            if (-not $runPassed) { exit 1 }
            exit 0
        } catch {
            try { Write-FailureResults $context 'PRODUCER_HARD_GATE_FAILED' $_.Exception.ToString() } catch {}
            [Console]::Error.WriteLine($_.Exception.ToString())
            exit 1
        }
    } else {
        if (Test-Path -LiteralPath $context.state_path -PathType Leaf) {
            $state = Read-State $context
            Invoke-GroupCleanup $context $state
        } else {
            Invoke-NoStateCleanup $context
        }
        exit 0
    }
} catch {
    [Console]::Error.WriteLine($_.Exception.ToString())
    exit 2
}

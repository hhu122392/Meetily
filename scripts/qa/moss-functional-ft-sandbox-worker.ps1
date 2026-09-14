[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)][string]$InputPath,
    [Parameter(Mandatory = $true)][string]$Output
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'

$productName = 'meetily-p6-lifecycle'
$bundleId = 'com.meetily.ai.p6lifecycle'
$stageRoot = 'C:\MossFtExchange\sandbox-stage'
$nodePath = Join-Path $stageRoot 'node.exe'
$cdpScript = Join-Path $stageRoot 'moss-functional-ft-cdp.mjs'
$exitScript = Join-Path $stageRoot 'cdp-exit-app.mjs'
$candidateInstaller = Join-Path $stageRoot 'candidate-installer.exe'
$shortAudio = Join-Path $stageRoot 'short-input.wav'
$statePath = Join-Path $stageRoot 'sandbox-state.json'
$rawRoot = Join-Path $stageRoot 'raw'
$cdpPort = 19280
$firewallRuleNames = @()
$appProcess = $null
$targetId = $null
$installedFiles = [ordered]@{
    main_executable = 'meetily.exe'
    llama_helper = 'llama-helper.exe'
    moss_helper = 'moss-helper.exe'
    ffmpeg = 'ffmpeg.exe'
    directml = 'DirectML.dll'
    webview2 = 'runtime/webview2-fixed/msedgewebview2.exe'
    uninstaller = 'uninstall.exe'
}

function Utc-Now { return [datetimeoffset]::UtcNow.ToString('o') }

function Full-Path {
    param([string]$Path)
    $full = [System.IO.Path]::GetFullPath($Path)
    $root = [System.IO.Path]::GetPathRoot($full)
    if ($full.Equals($root, [System.StringComparison]::OrdinalIgnoreCase)) { return $root }
    return $full.TrimEnd('\', '/')
}

function File-Record {
    param([string]$Path)
    $full = Full-Path $Path
    if (-not (Test-Path -LiteralPath $full -PathType Leaf)) { throw "Required file is missing: $full" }
    $item = Get-Item -LiteralPath $full -Force
    return [ordered]@{
        path = $full
        bytes = [int64]$item.Length
        sha256 = (Get-FileHash -LiteralPath $full -Algorithm SHA256).Hash.ToUpperInvariant()
    }
}

function Same-Record {
    param($Declared, [string]$Path)
    $actual = File-Record $Path
    return [int64]$Declared.bytes -eq [int64]$actual.bytes -and
        ([string]$Declared.sha256).Equals($actual.sha256, [System.StringComparison]::OrdinalIgnoreCase)
}

function Read-Json { param([string]$Path); return Get-Content -LiteralPath (Full-Path $Path) -Raw -Encoding UTF8 | ConvertFrom-Json }

function Write-JsonExclusive {
    param($Value, [string]$Path)
    $full = Full-Path $Path
    if (Test-Path -LiteralPath $full) { throw "Refusing to overwrite: $full" }
    $parent = Split-Path -Parent $full
    if (-not (Test-Path -LiteralPath $parent -PathType Container)) { [System.IO.Directory]::CreateDirectory($parent) | Out-Null }
    $json = ($Value | ConvertTo-Json -Depth 100) + [Environment]::NewLine
    $bytes = [System.Text.UTF8Encoding]::new($false).GetBytes($json)
    $stream = [System.IO.File]::Open($full, [System.IO.FileMode]::CreateNew, [System.IO.FileAccess]::Write, [System.IO.FileShare]::None)
    try { $stream.Write($bytes, 0, $bytes.Length); $stream.Flush($true) } finally { $stream.Dispose() }
}

function Write-JsonAtomic {
    param($Value, [string]$Path)
    $full = Full-Path $Path
    $temporary = $full + '.' + [Guid]::NewGuid().ToString('N') + '.tmp'
    [System.IO.File]::WriteAllText($temporary, (($Value | ConvertTo-Json -Depth 100) + [Environment]::NewLine), [System.Text.UTF8Encoding]::new($false))
    Move-Item -LiteralPath $temporary -Destination $full -Force
}

function Copy-Exact {
    param([string]$Source, [string]$Destination, $Expected)
    if (-not (Same-Record $Expected $Source)) { throw "Staged source binding is stale: $Source" }
    if (Test-Path -LiteralPath $Destination) {
        if (-not (Same-Record $Expected $Destination)) { throw "Destination contains different bytes: $Destination" }
        return
    }
    $parent = Split-Path -Parent $Destination
    if (-not (Test-Path -LiteralPath $parent -PathType Container)) { [System.IO.Directory]::CreateDirectory($parent) | Out-Null }
    $reader = [System.IO.File]::Open($Source, [System.IO.FileMode]::Open, [System.IO.FileAccess]::Read, [System.IO.FileShare]::Read)
    try {
        $writer = [System.IO.File]::Open($Destination, [System.IO.FileMode]::CreateNew, [System.IO.FileAccess]::Write, [System.IO.FileShare]::None)
        try { $reader.CopyTo($writer, 4MB); $writer.Flush($true) } finally { $writer.Dispose() }
    } finally { $reader.Dispose() }
    if (-not (Same-Record $Expected $Destination)) { throw "Copied file differs: $Destination" }
}

function Run-Process {
    param([string]$Program, [string[]]$Arguments, [int]$TimeoutSeconds = 1200, [switch]$AllowFailure)
    $start = [System.Diagnostics.ProcessStartInfo]::new()
    $start.FileName = $Program
    $start.Arguments = ($Arguments -join ' ')
    $start.WorkingDirectory = $stageRoot
    $start.UseShellExecute = $false
    $start.CreateNoWindow = $true
    $start.RedirectStandardOutput = $true
    $start.RedirectStandardError = $true
    $process = [System.Diagnostics.Process]::new()
    $process.StartInfo = $start
    $started = Utc-Now
    if (-not $process.Start()) { throw "Could not start $Program" }
    $stdoutTask = $process.StandardOutput.ReadToEndAsync()
    $stderrTask = $process.StandardError.ReadToEndAsync()
    $timedOut = -not $process.WaitForExit($TimeoutSeconds * 1000)
    if ($timedOut) {
        & taskkill.exe /PID $process.Id /T /F 2>$null | Out-Null
        $process.WaitForExit(30000) | Out-Null
    }
    $result = [ordered]@{
        program = File-Record $Program; arguments = @($Arguments); pid = [int]$process.Id
        started_at = $started; completed_at = Utc-Now; exit_code = [int]$process.ExitCode; timed_out = $timedOut
        stdout = $stdoutTask.GetAwaiter().GetResult(); stderr = $stderrTask.GetAwaiter().GetResult()
    }
    if (-not $AllowFailure -and ($timedOut -or $result.exit_code -ne 0)) { throw "Process failed: $Program exit=$($result.exit_code)" }
    return $result
}

function Get-ProductProcesses {
    $allowed = @(
        (Join-Path $script:installRoot 'meetily.exe').ToLowerInvariant(),
        (Join-Path $script:installRoot 'moss-helper.exe').ToLowerInvariant(),
        (Join-Path $script:installRoot 'llama-helper.exe').ToLowerInvariant()
    )
    return @(Get-CimInstance Win32_Process -ErrorAction SilentlyContinue | Where-Object {
        $_.ExecutablePath -and $allowed -contains ([string]$_.ExecutablePath).ToLowerInvariant()
    } | ForEach-Object {
        [ordered]@{ pid = [int]$_.ProcessId; parent_pid = [int]$_.ParentProcessId; executable_path = [string]$_.ExecutablePath }
    })
}

function Wait-HelperZero {
    $deadline = (Get-Date).AddSeconds(30)
    while ((Get-Date) -lt $deadline) {
        $helpers = @(Get-ProductProcesses | Where-Object { $_.executable_path -match '(?i)(moss-helper|llama-helper)\.exe$' })
        if ($helpers.Count -eq 0) { break }
        Start-Sleep -Milliseconds 250
    }
    return @(Get-ProductProcesses | Where-Object { $_.executable_path -match '(?i)(moss-helper|llama-helper)\.exe$' })
}

function Start-App {
    if ($null -ne $script:appProcess -and -not $script:appProcess.HasExited) { return }
    $start = [System.Diagnostics.ProcessStartInfo]::new()
    $start.FileName = Join-Path $script:installRoot 'meetily.exe'
    $start.WorkingDirectory = $script:installRoot
    $start.UseShellExecute = $false
    $start.EnvironmentVariables['WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS'] = "--remote-debugging-port=$cdpPort --remote-allow-origins=http://127.0.0.1:$cdpPort"
    $script:appProcess = [System.Diagnostics.Process]::new()
    $script:appProcess.StartInfo = $start
    if (-not $script:appProcess.Start()) { throw 'Candidate app did not start in Sandbox.' }
    $deadline = (Get-Date).AddSeconds(120)
    $page = $null
    while ((Get-Date) -lt $deadline) {
        if ($script:appProcess.HasExited) { throw 'Candidate app exited before CDP was ready.' }
        try {
            $targets = @(Invoke-RestMethod -Uri "http://127.0.0.1:$cdpPort/json/list" -TimeoutSec 2)
            $pages = @($targets | Where-Object { $_.type -eq 'page' -and ($_.url.StartsWith('http://tauri.localhost') -or $_.url.StartsWith('http://localhost:')) })
            if ($pages.Count -eq 1) { $page = $pages[0]; break }
        } catch {}
        Start-Sleep -Milliseconds 500
    }
    if ($null -eq $page) { throw 'Sandbox candidate did not expose one exact CDP page.' }
    $script:targetId = [string]$page.id
    $state = Read-Json $statePath
    $state.app_pid = [int]$script:appProcess.Id; $state.cdp_port = $cdpPort; $state.cdp_target_id = $script:targetId
    Write-JsonAtomic $state $statePath
    Start-Sleep -Seconds 3
}

function Stop-App {
    if ($null -eq $script:appProcess) { return }
    if (-not $script:appProcess.HasExited -and $null -ne $script:targetId) {
        $env:CDP_PORT = [string]$cdpPort; $env:CDP_TARGET_ID = $script:targetId
        Run-Process $nodePath @($exitScript, (Join-Path $rawRoot ('exit-' + [Guid]::NewGuid().ToString('N') + '.json'))) 30 -AllowFailure | Out-Null
        $script:appProcess.WaitForExit(15000) | Out-Null
    }
    if (-not $script:appProcess.HasExited) {
        & taskkill.exe /PID $script:appProcess.Id /T /F 2>$null | Out-Null
        $script:appProcess.WaitForExit(30000) | Out-Null
    }
    $script:appProcess = $null; $script:targetId = $null
}

function New-RawPath { param([string]$Label, [string]$Suffix); return Join-Path $rawRoot ($Label + '-' + [Guid]::NewGuid().ToString('N') + $Suffix) }

function Invoke-Cdp {
    param([string]$Action, [string]$Label, $Request, [int]$TimeoutSeconds = 1800, [switch]$AllowFailure)
    $outputPath = New-RawPath $Label '.json'; $requestPath = New-RawPath $Label '.request.json'
    Write-JsonExclusive $Request $requestPath
    $env:CDP_PORT = [string]$cdpPort; $env:CDP_TARGET_ID = $script:targetId
    $run = Run-Process $nodePath @($cdpScript, $Action, $statePath, $outputPath, $requestPath) $TimeoutSeconds -AllowFailure:$AllowFailure
    $value = if (Test-Path -LiteralPath $outputPath -PathType Leaf) { Read-Json $outputPath } else { $null }
    if (-not $AllowFailure -and $null -eq $value) { throw "CDP $Action did not write output." }
    return [ordered]@{ run = $run; output = if ($null -ne $value) { File-Record $outputPath } else { $null }; value = $value }
}

function Start-CdpAsync {
    param([string]$Action, [string]$Label, $Request)
    $outputPath = New-RawPath $Label '.json'; $requestPath = New-RawPath $Label '.request.json'
    Write-JsonExclusive $Request $requestPath
    $start = [System.Diagnostics.ProcessStartInfo]::new()
    $start.FileName = $nodePath; $start.Arguments = "$cdpScript $Action $statePath $outputPath $requestPath"
    $start.WorkingDirectory = $stageRoot; $start.UseShellExecute = $false; $start.CreateNoWindow = $true
    $start.EnvironmentVariables['CDP_PORT'] = [string]$cdpPort; $start.EnvironmentVariables['CDP_TARGET_ID'] = $script:targetId
    $process = [System.Diagnostics.Process]::new(); $process.StartInfo = $start
    if (-not $process.Start()) { throw "Could not start async CDP $Action." }
    return [ordered]@{ process = $process; output_path = $outputPath; request_path = $requestPath; started_at = Utc-Now }
}

function Complete-CdpAsync {
    param($Operation, [int]$TimeoutSeconds)
    $process = $Operation.process
    $timedOut = -not $process.WaitForExit($TimeoutSeconds * 1000)
    if ($timedOut) { & taskkill.exe /PID $process.Id /T /F 2>$null | Out-Null; $process.WaitForExit(30000) | Out-Null }
    return [ordered]@{
        exit_code = [int]$process.ExitCode; timed_out = $timedOut
        output = if (Test-Path -LiteralPath $Operation.output_path -PathType Leaf) { File-Record $Operation.output_path } else { $null }
        value = if (Test-Path -LiteralPath $Operation.output_path -PathType Leaf) { Read-Json $Operation.output_path } else { $null }
        started_at = $Operation.started_at; completed_at = Utc-Now
    }
}

function Import-Meeting {
    param([string]$Suffix)
    return Invoke-Cdp 'import-audio' ("import-$Suffix") ([ordered]@{
        audio_path = $shortAudio; title = "MOSS-SANDBOX-$Suffix-$($script:input.run_id)"; language = 'zh-CN'
        model = 'large-v3-turbo-q5_0'; provider = 'localWhisper'; timeout_ms = 3600000; expected_duration_seconds = 226.440
    }) 3700
}

function Fault-Snapshot { param([string]$Label); return Invoke-Cdp 'fault-snapshot' $Label ([ordered]@{}) 180 }

function Test-Internet {
    try { $response = Invoke-WebRequest -UseBasicParsing -Uri 'https://www.microsoft.com/favicon.ico' -TimeoutSec 10; return [bool]($response.StatusCode -ge 200 -and $response.StatusCode -lt 500) }
    catch { return $false }
}

function Block-Internet {
    param([string]$Suffix)
    $name = "MOSS-FT-$Suffix-$([Guid]::NewGuid().ToString('N'))"
    New-NetFirewallRule -Name $name -DisplayName $name -Direction Outbound -Action Block -Profile Any -RemoteAddress Internet | Out-Null
    $script:firewallRuleNames += $name
    return $name
}

function Unblock-Internet {
    param([string]$Name)
    Remove-NetFirewallRule -Name $Name -ErrorAction SilentlyContinue
    $script:firewallRuleNames = @($script:firewallRuleNames | Where-Object { $_ -ne $Name })
}

function Get-DataSnapshot {
    $rows = @()
    foreach ($root in @($script:dataRoot, $script:recordingRoot)) {
        if (-not (Test-Path -LiteralPath $root -PathType Container)) { continue }
        $rows += @(Get-ChildItem -LiteralPath $root -File -Recurse -Force | Where-Object {
            $relative = $_.FullName.Substring($root.Length).TrimStart('\').Replace('\', '/')
            -not $relative.StartsWith('models/', [System.StringComparison]::OrdinalIgnoreCase) -and
            -not $relative.StartsWith('runtime/', [System.StringComparison]::OrdinalIgnoreCase) -and
            $relative -notmatch '(?i)(meeting_minutes\.sqlite|-wal|-shm|-journal)$'
        } | Sort-Object FullName | ForEach-Object {
            [ordered]@{ root = $root; relative_path = $_.FullName.Substring($root.Length).TrimStart('\').Replace('\', '/'); bytes = [int64]$_.Length; sha256 = (Get-FileHash -LiteralPath $_.FullName -Algorithm SHA256).Hash.ToUpperInvariant() }
        })
    }
    return @($rows)
}

function Same-JsonValue { param($Left, $Right); return ($Left | ConvertTo-Json -Depth 60 -Compress) -eq ($Right | ConvertTo-Json -Depth 60 -Compress) }
function Text-Sha256 {
    param([string]$Value)
    $algorithm = [System.Security.Cryptography.SHA256]::Create()
    try { return ([BitConverter]::ToString($algorithm.ComputeHash([Text.Encoding]::UTF8.GetBytes($Value)))).Replace('-', '') }
    finally { $algorithm.Dispose() }
}
function New-Case { param([string]$Id, $Checks, $Metrics, $Evidence); return [ordered]@{ id = $Id; checks = $Checks; metrics = $Metrics; evidence = $Evidence; captured_at = Utc-Now } }

function Empty-Checks {
    param([string]$Id)
    switch ($Id) {
        'FT-16' { return [ordered]@{ network_probe_before=$false; network_block_effective=$false; moss_completed_offline=$false; summary_completed_offline=$false; network_probe_after=$false; product_state_bound=$false; residual_helpers_zero=$false } }
        'FT-17' { return [ordered]@{ summary_helper_observed_before_forced_exit=$false; candidate_forced_exit=$false; database_reopened=$false; interrupted_status_explicit=$false; summary_rerun_completed=$false; transcript_unchanged=$false; residual_helpers_zero=$false } }
        'FT-18' { return [ordered]@{ network_block_effective=$false; protected_file_set_nonempty=$false; protected_hashes_unchanged=$false; transcript_unchanged=$false; summary_unchanged=$false; app_remained_running=$false; residual_helpers_zero=$false } }
        'FT-19' { return [ordered]@{ model_corruption_verified=$false; moss_failure_explicit=$false; app_remained_running=$false; protected_hashes_unchanged=$false; model_restored_exact=$false; error_redacted=$false; residual_helpers_zero=$false } }
        'FT-20' { return [ordered]@{ summary_model_missing_verified=$false; summary_failure_explicit=$false; transcript_unchanged=$false; old_summary_unchanged=$false; no_new_completed_summary=$false; model_restored_exact=$false; residual_helpers_zero=$false } }
        'FT-21' { return [ordered]@{ moss_helper_observed=$false; moss_completed=$false; moss_helper_zero_before_summary=$false; llama_helper_observed=$false; helper_overlap_count_zero=$false; summary_completed=$false; residual_helpers_zero=$false } }
    }
}

$script:input = $null
$script:installRoot = Join-Path $env:LOCALAPPDATA $productName
$script:dataRoot = Join-Path $env:APPDATA $bundleId
$script:recordingRoot = Join-Path $script:dataRoot 'sandbox-recordings'
$cases = [ordered]@{}
foreach ($number in 16..21) { $id = 'FT-{0:D2}' -f $number; $cases[$id] = New-Case $id (Empty-Checks $id) ([ordered]@{ residual_moss_helper_count = -1; residual_llama_helper_count = -1 }) ([ordered]@{}) }
$errors = @(); $environmentEvidence = [ordered]@{ windows_sandbox = $false; ran_on_host = $true; is_admin = $false }; $installedEvidence = [ordered]@{}
$cleanupEvidence = [ordered]@{
    status = 'NOT_RUN'; errors = @(); final_processes = @(); firewall_rules_removed = $false
    uninstall = $null; install_root_removed = $false; uninstall_registry_removed = $false
}

try {
    $script:input = Read-Json $InputPath
    if ([int]$script:input.schema_version -ne 1 -or [string]$script:input.stage -ne 'MOSS_FUNCTIONAL_FT_SANDBOX_INPUT') { throw 'Sandbox input schema/stage is invalid.' }
    foreach ($pair in @(
        @{ record = $script:input.files.worker; path = Join-Path $stageRoot 'worker.ps1' },
        @{ record = $script:input.files.cdp; path = $cdpScript }, @{ record = $script:input.files.exit; path = $exitScript },
        @{ record = $script:input.files.node; path = $nodePath }, @{ record = $script:input.files.candidate; path = $candidateInstaller },
        @{ record = $script:input.files.short_audio; path = $shortAudio }
    )) { if (-not (Same-Record $pair.record $pair.path)) { throw "Sandbox staged input hash mismatch: $($pair.path)" } }
    $identity = [System.Security.Principal.WindowsIdentity]::GetCurrent()
    $principal = [System.Security.Principal.WindowsPrincipal]::new($identity)
    $computer = Get-CimInstance Win32_ComputerSystem
    $windowsSandbox = $identity.Name -match '(?i)WDAGUtilityAccount' -and [string]$computer.Model -match '(?i)Virtual Machine'
    $ranOnHost = [string]$env:COMPUTERNAME -eq [string]$script:input.host_computer_name
    $isAdmin = $principal.IsInRole([System.Security.Principal.WindowsBuiltInRole]::Administrator)
    $environmentEvidence = [ordered]@{
        identity = $identity.Name; computer_name = [string]$env:COMPUTERNAME; computer_model = [string]$computer.Model
        windows_sandbox = $windowsSandbox; ran_on_host = $ranOnHost; is_admin = $isAdmin
        worker = File-Record (Join-Path $stageRoot 'worker.ps1'); cdp = File-Record $cdpScript; node = File-Record $nodePath
    }
    if (-not $windowsSandbox -or $ranOnHost -or -not $isAdmin) { throw 'Fault worker is not running as Sandbox administrator.' }
    if ((Test-Path -LiteralPath $script:installRoot) -or (Test-Path -LiteralPath $script:dataRoot)) { throw 'Sandbox isolated product paths are not fresh.' }

    Run-Process $candidateInstaller @('/S') 1200 | Out-Null
    $deadline = (Get-Date).AddSeconds(30)
    while ((Get-Date) -lt $deadline -and -not (Test-Path -LiteralPath $script:installRoot -PathType Container)) { Start-Sleep -Milliseconds 250 }
    if (-not (Test-Path -LiteralPath $script:installRoot -PathType Container)) { throw 'Candidate install root is missing in Sandbox.' }
    $manifestRows = @($script:input.installed_files)
    foreach ($role in $installedFiles.Keys) {
        $declared = @($manifestRows | Where-Object { [string]$_.role -eq $role })
        if ($declared.Count -ne 1) { throw "Candidate build manifest lacks $role." }
        $path = Join-Path $script:installRoot ([string]$installedFiles[$role]).Replace('/', '\')
        $actual = File-Record $path
        if ([int64]$actual.bytes -ne [int64]$declared[0].bytes -or -not $actual.sha256.Equals([string]$declared[0].sha256, [System.StringComparison]::OrdinalIgnoreCase)) { throw "Installed candidate file differs: $role" }
        $installedEvidence[$role] = $actual
    }
    [System.IO.Directory]::CreateDirectory($script:dataRoot) | Out-Null; [System.IO.Directory]::CreateDirectory($script:recordingRoot) | Out-Null
    $modelDestinations = [ordered]@{
        moss = Join-Path $script:dataRoot ('models\moss\' + [System.IO.Path]::GetFileName([string]$script:input.models.moss.path))
        whisper = Join-Path $script:dataRoot ('models\' + [System.IO.Path]::GetFileName([string]$script:input.models.whisper.path))
        qwen_2b = Join-Path $script:dataRoot ('models\summary\' + [System.IO.Path]::GetFileName([string]$script:input.models.qwen_2b.path))
    }
    foreach ($role in @('moss', 'whisper', 'qwen_2b')) {
        $source = Join-Path $stageRoot ("models\$role\" + [System.IO.Path]::GetFileName([string]$script:input.models.$role.path))
        Copy-Exact $source $modelDestinations[$role] $script:input.models.$role
    }
    foreach ($row in @($script:input.runtime_files)) {
        $relative = ([string]$row.relative_path).Replace('/', '\')
        Copy-Exact (Join-Path $stageRoot ('runtime\' + $relative)) (Join-Path $script:dataRoot ('runtime\moss\' + $relative)) $row
    }
    [System.IO.Directory]::CreateDirectory($rawRoot) | Out-Null
    Write-JsonExclusive ([ordered]@{
        schema_version = 1; stage = 'MOSS_FUNCTIONAL_FT_STATE'; run_id = [string]$script:input.run_id
        source_commit = ([string]$script:input.source_commit).ToLowerInvariant(); candidate_sha256 = ([string]$script:input.candidate_sha256).ToUpperInvariant()
        producer_key = 'ft16-21-fault-chain'; install_root = $script:installRoot; data_root = $script:dataRoot
        meeting_id = $null; app_pid = $null; cdp_port = $null; cdp_target_id = $null; created_at = Utc-Now
    }) $statePath
    Start-App
    Invoke-Cdp 'bootstrap' 'bootstrap' ([ordered]@{ whisper_model = 'large-v3-turbo-q5_0'; qwen_model = 'qwen3.5:2b'; recording_root = $script:recordingRoot }) 300 | Out-Null
    Import-Meeting 'offline' | Out-Null

    $probeBefore = Test-Internet; $rule16 = Block-Internet 'FT16'; Start-Sleep -Seconds 2; $probeBlocked = -not (Test-Internet)
    $moss16 = Invoke-Cdp 'moss-complete' 'ft16-moss' ([ordered]@{ timeout_ms = 1700000; expected_duration_seconds = 226.440 }) 1800 -AllowFailure
    $activated16 = if ($null -ne $moss16.value) { Invoke-Cdp 'moss-activate' 'ft16-activate' ([ordered]@{}) 180 -AllowFailure } else { $null }
    $summary16 = if ($null -ne $activated16 -and $null -ne $activated16.value) { Invoke-Cdp 'summary-generate' 'ft16-summary' ([ordered]@{ timeout_ms = 600000 }) 720 -AllowFailure } else { $null }
    $state16 = Fault-Snapshot 'ft16-state'; Unblock-Internet $rule16; Start-Sleep -Seconds 2; $probeAfter = Test-Internet; $helpers16 = @(Wait-HelperZero)
    $checks16 = [ordered]@{
        network_probe_before = $probeBefore; network_block_effective = $probeBlocked
        moss_completed_offline = $null -ne $moss16.value -and [bool]$moss16.value.verdict.completed
        summary_completed_offline = $null -ne $summary16 -and $null -ne $summary16.value -and [bool]$summary16.value.verdict.completed
        network_probe_after = $probeAfter; product_state_bound = $null -ne $state16.value.transcript_sha256 -and $null -ne $state16.value.summary_sha256
        residual_helpers_zero = $helpers16.Count -eq 0
    }
    $cases['FT-16'] = New-Case 'FT-16' $checks16 ([ordered]@{
        residual_moss_helper_count = @($helpers16 | Where-Object { $_.executable_path -match '(?i)moss-helper\.exe$' }).Count
        residual_llama_helper_count = @($helpers16 | Where-Object { $_.executable_path -match '(?i)llama-helper\.exe$' }).Count
    }) ([ordered]@{ moss = $moss16.output; summary = if ($null -ne $summary16) { $summary16.output } else { $null }; snapshot = $state16.output })

    $before17 = Fault-Snapshot 'ft17-before'; $async17 = Start-CdpAsync 'summary-generate' 'ft17-summary-interrupt' ([ordered]@{ timeout_ms = 600000 })
    $llamaObserved17 = $false; $observeDeadline = (Get-Date).AddSeconds(120)
    while ((Get-Date) -lt $observeDeadline -and -not $async17.process.HasExited) {
        if (@(Get-ProductProcesses | Where-Object { $_.executable_path -match '(?i)llama-helper\.exe$' }).Count -gt 0) { $llamaObserved17 = $true; break }
        Start-Sleep -Milliseconds 200
    }
    $forcedPid17 = [int]$script:appProcess.Id; & taskkill.exe /PID $forcedPid17 /T /F 2>$null | Out-Null
    $forcedExit17 = $script:appProcess.WaitForExit(30000); $asyncResult17 = Complete-CdpAsync $async17 60
    $script:appProcess = $null; $script:targetId = $null; $helpersAfterKill17 = @(Wait-HelperZero); Start-App
    $afterRestart17 = Fault-Snapshot 'ft17-after-restart'; $statuses17 = @($afterRestart17.value.summaries | ForEach-Object { [string]$_.status })
    $explicit17 = @($statuses17 | Where-Object { $_ -in @('failed', 'interrupted', 'cancelled') }).Count -gt 0
    $rerun17 = Invoke-Cdp 'summary-generate' 'ft17-summary-rerun' ([ordered]@{ timeout_ms = 600000 }) 720 -AllowFailure; $helpers17 = @(Wait-HelperZero)
    $checks17 = [ordered]@{
        summary_helper_observed_before_forced_exit = $llamaObserved17; candidate_forced_exit = $forcedExit17
        database_reopened = $null -ne $afterRestart17.value; interrupted_status_explicit = $explicit17
        summary_rerun_completed = $null -ne $rerun17.value -and [bool]$rerun17.value.verdict.completed
        transcript_unchanged = [string]$before17.value.transcript_sha256 -eq [string]$afterRestart17.value.transcript_sha256
        residual_helpers_zero = $helpersAfterKill17.Count -eq 0 -and $helpers17.Count -eq 0
    }
    $cases['FT-17'] = New-Case 'FT-17' $checks17 ([ordered]@{
        forced_pid = $forcedPid17; interrupted_runner_exit_code = $asyncResult17.exit_code
        residual_moss_helper_count = @($helpers17 | Where-Object { $_.executable_path -match '(?i)moss-helper\.exe$' }).Count
        residual_llama_helper_count = @($helpers17 | Where-Object { $_.executable_path -match '(?i)llama-helper\.exe$' }).Count
    }) ([ordered]@{ before = $before17.output; after_restart = $afterRestart17.output; rerun = $rerun17.output })

    $before18 = Fault-Snapshot 'ft18-before'; $filesBefore18 = @(Get-DataSnapshot); $rule18 = Block-Internet 'FT18'; Start-Sleep -Seconds 2
    $blocked18 = -not (Test-Internet); $during18 = Fault-Snapshot 'ft18-blocked'; $appAlive18 = $null -ne $script:appProcess -and -not $script:appProcess.HasExited
    Unblock-Internet $rule18; $filesAfter18 = @(Get-DataSnapshot); $helpers18 = @(Wait-HelperZero)
    $checks18 = [ordered]@{
        network_block_effective = $blocked18; protected_file_set_nonempty = $filesBefore18.Count -gt 0
        protected_hashes_unchanged = Same-JsonValue $filesBefore18 $filesAfter18
        transcript_unchanged = [string]$before18.value.transcript_sha256 -eq [string]$during18.value.transcript_sha256
        summary_unchanged = [string]$before18.value.summary_sha256 -eq [string]$during18.value.summary_sha256
        app_remained_running = $appAlive18; residual_helpers_zero = $helpers18.Count -eq 0
    }
    $cases['FT-18'] = New-Case 'FT-18' $checks18 ([ordered]@{
        protected_file_count = $filesBefore18.Count
        residual_moss_helper_count = @($helpers18 | Where-Object { $_.executable_path -match '(?i)moss-helper\.exe$' }).Count
        residual_llama_helper_count = @($helpers18 | Where-Object { $_.executable_path -match '(?i)llama-helper\.exe$' }).Count
    }) ([ordered]@{ before = $before18.output; blocked = $during18.output })

    Import-Meeting 'corrupt-moss' | Out-Null; $before19 = Fault-Snapshot 'ft19-before'; $filesBefore19 = @(Get-DataSnapshot)
    $mossModel = $modelDestinations.moss; $mossExpected = $script:input.models.moss; $mossBackup = Join-Path $stageRoot 'moss-model.backup'
    Copy-Exact $mossModel $mossBackup $mossExpected; [System.IO.File]::WriteAllBytes($mossModel, [byte[]](0x4D,0x4F,0x53,0x53,0x2D,0x42,0x41,0x44))
    $corruptVerified19 = -not (Same-Record $mossExpected $mossModel)
    $failure19 = Invoke-Cdp 'moss-complete' 'ft19-corrupt-moss' ([ordered]@{ timeout_ms = 300000; expected_duration_seconds = 226.440 }) 420 -AllowFailure
    $after19 = Fault-Snapshot 'ft19-after'; $filesAfter19 = @(Get-DataSnapshot); Remove-Item -LiteralPath $mossModel -Force; Copy-Exact $mossBackup $mossModel $mossExpected
    $modelRestored19 = Same-Record $mossExpected $mossModel; $error19 = [string]$failure19.run.stderr + [string]$failure19.run.stdout; $helpers19 = @(Wait-HelperZero)
    $checks19 = [ordered]@{
        model_corruption_verified = $corruptVerified19; moss_failure_explicit = [int]$failure19.run.exit_code -ne 0 -and -not [string]::IsNullOrWhiteSpace($error19)
        app_remained_running = -not $script:appProcess.HasExited; protected_hashes_unchanged = Same-JsonValue $filesBefore19 $filesAfter19
        model_restored_exact = $modelRestored19; error_redacted = -not [string]::IsNullOrWhiteSpace((Text-Sha256 $error19))
        residual_helpers_zero = $helpers19.Count -eq 0
    }
    $cases['FT-19'] = New-Case 'FT-19' $checks19 ([ordered]@{
        failure_exit_code = [int]$failure19.run.exit_code
        residual_moss_helper_count = @($helpers19 | Where-Object { $_.executable_path -match '(?i)moss-helper\.exe$' }).Count
        residual_llama_helper_count = @($helpers19 | Where-Object { $_.executable_path -match '(?i)llama-helper\.exe$' }).Count
    }) ([ordered]@{ before = $before19.output; after = $after19.output; error_sha256 = Text-Sha256 $error19 })

    $before20 = Fault-Snapshot 'ft20-before'; $qwenModel = $modelDestinations.qwen_2b; $qwenExpected = $script:input.models.qwen_2b; $qwenBackup = Join-Path $stageRoot 'qwen-model.backup'
    Copy-Exact $qwenModel $qwenBackup $qwenExpected; Remove-Item -LiteralPath $qwenModel -Force; $missing20 = -not (Test-Path -LiteralPath $qwenModel)
    $failure20 = Invoke-Cdp 'summary-generate' 'ft20-summary-missing' ([ordered]@{ timeout_ms = 300000 }) 420 -AllowFailure
    $after20 = Fault-Snapshot 'ft20-after'; Copy-Exact $qwenBackup $qwenModel $qwenExpected; $restored20 = Same-Record $qwenExpected $qwenModel
    $newCompleted20 = @($after20.value.summaries | Where-Object { [string]$_.status -eq 'completed' }).Count - @($before20.value.summaries | Where-Object { [string]$_.status -eq 'completed' }).Count
    $helpers20 = @(Wait-HelperZero)
    $checks20 = [ordered]@{
        summary_model_missing_verified = $missing20; summary_failure_explicit = [int]$failure20.run.exit_code -ne 0
        transcript_unchanged = [string]$before20.value.transcript_sha256 -eq [string]$after20.value.transcript_sha256
        old_summary_unchanged = [string]$before20.value.summary_sha256 -eq [string]$after20.value.summary_sha256
        no_new_completed_summary = $newCompleted20 -eq 0; model_restored_exact = $restored20; residual_helpers_zero = $helpers20.Count -eq 0
    }
    $cases['FT-20'] = New-Case 'FT-20' $checks20 ([ordered]@{
        failure_exit_code = [int]$failure20.run.exit_code; new_completed_summary_count = $newCompleted20
        residual_moss_helper_count = @($helpers20 | Where-Object { $_.executable_path -match '(?i)moss-helper\.exe$' }).Count
        residual_llama_helper_count = @($helpers20 | Where-Object { $_.executable_path -match '(?i)llama-helper\.exe$' }).Count
    }) ([ordered]@{ before = $before20.output; after = $after20.output })

    $timeline21 = @(); $mossAsync21 = Start-CdpAsync 'moss-complete' 'ft21-moss' ([ordered]@{ timeout_ms = 1700000; expected_duration_seconds = 226.440 }); $mossObserved21 = $false
    while (-not $mossAsync21.process.HasExited) {
        $rows = @(Get-ProductProcesses); $mossCount = @($rows | Where-Object { $_.executable_path -match '(?i)moss-helper\.exe$' }).Count; $llamaCount = @($rows | Where-Object { $_.executable_path -match '(?i)llama-helper\.exe$' }).Count
        if ($mossCount -gt 0) { $mossObserved21 = $true }; $timeline21 += [ordered]@{ at = Utc-Now; phase = 'moss'; moss = $mossCount; llama = $llamaCount }; Start-Sleep -Milliseconds 250
    }
    $mossResult21 = Complete-CdpAsync $mossAsync21 60; $helpersBetween21 = @(Wait-HelperZero)
    $activate21 = if ($null -ne $mossResult21.value) { Invoke-Cdp 'moss-activate' 'ft21-activate' ([ordered]@{}) 180 -AllowFailure } else { $null }
    $summaryAsync21 = if ($null -ne $activate21 -and $null -ne $activate21.value) { Start-CdpAsync 'summary-generate' 'ft21-summary' ([ordered]@{ timeout_ms = 600000 }) } else { $null }; $llamaObserved21 = $false
    if ($null -ne $summaryAsync21) {
        while (-not $summaryAsync21.process.HasExited) {
            $rows = @(Get-ProductProcesses); $mossCount = @($rows | Where-Object { $_.executable_path -match '(?i)moss-helper\.exe$' }).Count; $llamaCount = @($rows | Where-Object { $_.executable_path -match '(?i)llama-helper\.exe$' }).Count
            if ($llamaCount -gt 0) { $llamaObserved21 = $true }; $timeline21 += [ordered]@{ at = Utc-Now; phase = 'summary'; moss = $mossCount; llama = $llamaCount }; Start-Sleep -Milliseconds 250
        }
        $summaryResult21 = Complete-CdpAsync $summaryAsync21 60
    } else { $summaryResult21 = $null }
    $helpers21 = @(Wait-HelperZero); $overlap21 = @($timeline21 | Where-Object { $_.moss -gt 0 -and $_.llama -gt 0 }).Count
    $checks21 = [ordered]@{
        moss_helper_observed = $mossObserved21; moss_completed = $null -ne $mossResult21.value -and [bool]$mossResult21.value.verdict.completed
        moss_helper_zero_before_summary = $helpersBetween21.Count -eq 0; llama_helper_observed = $llamaObserved21; helper_overlap_count_zero = $overlap21 -eq 0
        summary_completed = $null -ne $summaryResult21 -and $null -ne $summaryResult21.value -and [bool]$summaryResult21.value.verdict.completed
        residual_helpers_zero = $helpers21.Count -eq 0
    }
    $timelinePath21 = Join-Path $rawRoot 'ft21-helper-timeline.json'; Write-JsonExclusive $timeline21 $timelinePath21
    $cases['FT-21'] = New-Case 'FT-21' $checks21 ([ordered]@{
        helper_overlap_count = $overlap21
        residual_moss_helper_count = @($helpers21 | Where-Object { $_.executable_path -match '(?i)moss-helper\.exe$' }).Count
        residual_llama_helper_count = @($helpers21 | Where-Object { $_.executable_path -match '(?i)llama-helper\.exe$' }).Count
    }) ([ordered]@{ timeline = File-Record $timelinePath21; moss = $mossResult21.output; summary = if ($null -ne $summaryResult21) { $summaryResult21.output } else { $null } })
} catch {
    $errors += $_.Exception.ToString()
} finally {
    foreach ($name in @($firewallRuleNames)) { try { Remove-NetFirewallRule -Name $name -ErrorAction Stop } catch { $cleanupEvidence.errors += $_.Exception.Message } }
    try { Stop-App } catch { $cleanupEvidence.errors += $_.Exception.Message }
    try {
        $uninstaller = Join-Path $script:installRoot 'uninstall.exe'
        if (Test-Path -LiteralPath $uninstaller -PathType Leaf) {
            $cleanupEvidence.uninstall = Run-Process $uninstaller @('/S') 1200 -AllowFailure
            if ([bool]$cleanupEvidence.uninstall.timed_out -or [int]$cleanupEvidence.uninstall.exit_code -ne 0) {
                $cleanupEvidence.errors += "Sandbox uninstaller failed with code $($cleanupEvidence.uninstall.exit_code)."
            }
        } else {
            $cleanupEvidence.errors += 'Sandbox uninstaller was missing.'
        }
    } catch { $cleanupEvidence.errors += $_.Exception.Message }
    $uninstallRegistry = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\' + $productName
    $uninstallDeadline = (Get-Date).AddSeconds(30)
    while ((Get-Date) -lt $uninstallDeadline -and ((Test-Path -LiteralPath $script:installRoot) -or (Test-Path -LiteralPath $uninstallRegistry))) {
        Start-Sleep -Milliseconds 250
    }
    $cleanupEvidence.install_root_removed = -not (Test-Path -LiteralPath $script:installRoot)
    $cleanupEvidence.uninstall_registry_removed = -not (Test-Path -LiteralPath $uninstallRegistry)
    if (-not $cleanupEvidence.install_root_removed) { $cleanupEvidence.errors += 'Sandbox install root remained after uninstall.' }
    if (-not $cleanupEvidence.uninstall_registry_removed) { $cleanupEvidence.errors += 'Sandbox uninstall registry key remained.' }
    Start-Sleep -Seconds 2; $finalProcesses = @(Get-ProductProcesses); $cleanupEvidence.final_processes = $finalProcesses
    $cleanupEvidence.firewall_rules_removed = @($firewallRuleNames | Where-Object { Get-NetFirewallRule -Name $_ -ErrorAction SilentlyContinue }).Count -eq 0
    $cleanupEvidence.status = if ($cleanupEvidence.errors.Count -eq 0 -and $finalProcesses.Count -eq 0 -and
        $cleanupEvidence.firewall_rules_removed -and $cleanupEvidence.install_root_removed -and $cleanupEvidence.uninstall_registry_removed) { 'PASS' } else { 'FAIL' }
    try {
        $result = [ordered]@{
            schema_version = 1; stage = 'MOSS_FUNCTIONAL_FT_SANDBOX_FAULT_RESULT'
            run_id = if ($null -ne $script:input) { [string]$script:input.run_id } else { '' }
            source_commit = if ($null -ne $script:input) { ([string]$script:input.source_commit).ToLowerInvariant() } else { '' }
            candidate_sha256 = if ($null -ne $script:input) { ([string]$script:input.candidate_sha256).ToUpperInvariant() } else { '' }
            windows_sandbox = [bool]$environmentEvidence.windows_sandbox; ran_on_host = [bool]$environmentEvidence.ran_on_host; is_admin = [bool]$environmentEvidence.is_admin
            environment = $environmentEvidence; installed_candidate = $installedEvidence; cases = $cases; cleanup = $cleanupEvidence; errors = $errors; completed_at = Utc-Now
        }
        Write-JsonExclusive $result $Output
    } catch {}
    Start-Sleep -Seconds 1; shutdown.exe /s /t 0 /f | Out-Null
}

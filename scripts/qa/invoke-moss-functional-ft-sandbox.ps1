[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)][string]$AcceptanceConfig,
    [Parameter(Mandatory = $true)][string]$ProducerInput,
    [Parameter(Mandatory = $true)][string]$Output,
    [Parameter(Mandatory = $true)][string]$RunId,
    [Parameter(Mandatory = $true)][string]$SourceCommit,
    [Parameter(Mandatory = $true)][string]$CandidateSha256
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'

$scriptRoot = Split-Path -Parent $MyInvocation.MyCommand.Path
$workerSource = Join-Path $scriptRoot 'moss-functional-ft-sandbox-worker.ps1'
$cdpSource = Join-Path $scriptRoot 'moss-functional-ft-cdp.mjs'
$exitSource = Join-Path $scriptRoot 'cdp-exit-app.mjs'
$nativeArgumentsScript = Join-Path $scriptRoot 'windows-native-arguments.ps1'
$sandboxExe = Join-Path $env:SystemRoot 'System32\WindowsSandbox.exe'

. $nativeArgumentsScript

function Full-Path {
    param([string]$Path)
    $full = [System.IO.Path]::GetFullPath($Path)
    $root = [System.IO.Path]::GetPathRoot($full)
    if ($full.Equals($root, [System.StringComparison]::OrdinalIgnoreCase)) { return $root }
    return $full.TrimEnd('\', '/')
}

function Strict-Child {
    param([string]$Child, [string]$Parent)
    $childFull = Full-Path $Child
    $parentFull = Full-Path $Parent
    return $childFull.StartsWith($parentFull.TrimEnd('\', '/') + '\', [System.StringComparison]::OrdinalIgnoreCase)
}

function Assert-NoReparse {
    param([string]$Path, [string]$Boundary)
    $cursor = Full-Path $Path
    $root = Full-Path $Boundary
    if (-not $cursor.Equals($root, [System.StringComparison]::OrdinalIgnoreCase) -and -not (Strict-Child $cursor $root)) {
        throw "Path escapes boundary: $cursor"
    }
    while ($true) {
        $item = Get-Item -LiteralPath $cursor -Force
        if (($item.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0) { throw "Reparse point is forbidden: $cursor" }
        if ($cursor.Equals($root, [System.StringComparison]::OrdinalIgnoreCase)) { break }
        $cursor = Full-Path ([System.IO.Path]::GetDirectoryName($cursor))
    }
}

function File-Record {
    param([string]$Path)
    $full = Full-Path $Path
    if (-not (Test-Path -LiteralPath $full -PathType Leaf)) { throw "Required file is missing: $full" }
    Assert-NoReparse $full ([System.IO.Path]::GetPathRoot($full))
    $item = Get-Item -LiteralPath $full -Force
    return [ordered]@{
        path = $full
        bytes = [int64]$item.Length
        sha256 = (Get-FileHash -LiteralPath $full -Algorithm SHA256).Hash.ToUpperInvariant()
    }
}

function Assert-Record {
    param($Declared, [string]$Label)
    $actual = File-Record ([string]$Declared.path)
    if ([int64]$Declared.bytes -ne [int64]$actual.bytes -or
        -not ([string]$Declared.sha256).Equals($actual.sha256, [System.StringComparison]::OrdinalIgnoreCase)) {
        throw "$Label binding is stale."
    }
    return $actual
}

function Read-Json {
    param([string]$Path, [string]$Label)
    try { $value = Get-Content -LiteralPath (Full-Path $Path) -Raw -Encoding UTF8 | ConvertFrom-Json }
    catch { throw "$Label is invalid JSON: $($_.Exception.Message)" }
    if ($null -eq $value -or $value -is [System.Array]) { throw "$Label must be an object." }
    return $value
}

function Copy-Exclusive {
    param([string]$Source, [string]$Destination, [string]$Label)
    $sourceRecord = File-Record $Source
    $target = Full-Path $Destination
    if (Test-Path -LiteralPath $target) { throw "Refusing to overwrite staged ${Label}: $target" }
    $parent = Split-Path -Parent $target
    if (-not (Test-Path -LiteralPath $parent -PathType Container)) { [System.IO.Directory]::CreateDirectory($parent) | Out-Null }
    $reader = [System.IO.File]::Open($sourceRecord.path, [System.IO.FileMode]::Open, [System.IO.FileAccess]::Read, [System.IO.FileShare]::Read)
    try {
        $writer = [System.IO.File]::Open($target, [System.IO.FileMode]::CreateNew, [System.IO.FileAccess]::Write, [System.IO.FileShare]::None)
        try { $reader.CopyTo($writer, 4MB); $writer.Flush($true) } finally { $writer.Dispose() }
    } finally { $reader.Dispose() }
    $targetRecord = File-Record $target
    if ($targetRecord.bytes -ne $sourceRecord.bytes -or $targetRecord.sha256 -ne $sourceRecord.sha256) {
        throw "Staged $Label differs from its source."
    }
    return $targetRecord
}

function Write-JsonExclusive {
    param($Value, [string]$Path)
    $json = ($Value | ConvertTo-Json -Depth 100) + [Environment]::NewLine
    $bytes = [System.Text.UTF8Encoding]::new($false).GetBytes($json)
    $stream = [System.IO.File]::Open((Full-Path $Path), [System.IO.FileMode]::CreateNew, [System.IO.FileAccess]::Write, [System.IO.FileShare]::None)
    try { $stream.Write($bytes, 0, $bytes.Length); $stream.Flush($true) } finally { $stream.Dispose() }
}

if (-not (Test-Path -LiteralPath $sandboxExe -PathType Leaf)) { throw 'Windows Sandbox executable is unavailable.' }
$configPath = Full-Path $AcceptanceConfig
$sidecarPath = Full-Path $ProducerInput
$outputPath = Full-Path $Output
$config = Read-Json $configPath 'acceptance config'
$sidecar = Read-Json $sidecarPath 'producer private input'
$configRecord = File-Record $configPath
$candidate = Assert-Record $config.candidate 'candidate installer'
$buildManifestRecord = Assert-Record $config.build_manifest 'candidate build manifest'
$buildManifest = Read-Json $buildManifestRecord.path 'candidate build manifest'
if ([string]$config.run_id -ne $RunId -or [string]$sidecar.run_id -ne $RunId -or
    ([string]$config.source_commit).ToLowerInvariant() -ne $SourceCommit.ToLowerInvariant() -or
    ([string]$sidecar.source_commit).ToLowerInvariant() -ne $SourceCommit.ToLowerInvariant() -or
    -not $candidate.sha256.Equals($CandidateSha256, [System.StringComparison]::OrdinalIgnoreCase) -or
    -not ([string]$sidecar.candidate_sha256).Equals($CandidateSha256, [System.StringComparison]::OrdinalIgnoreCase) -or
    -not ([string]$sidecar.acceptance_sha256).Equals($configRecord.sha256, [System.StringComparison]::OrdinalIgnoreCase)) {
    throw 'Sandbox launcher inputs are not bound to one acceptance run.'
}
$group = $sidecar.groups.'ft16-21-fault-chain'
$exchange = Full-Path ([string]$group.sandbox_exchange_root)
if (-not (Test-Path -LiteralPath $exchange -PathType Container)) { throw 'Sandbox exchange root is missing.' }
Assert-NoReparse $exchange ([System.IO.Path]::GetPathRoot($exchange))
if (-not (Strict-Child $outputPath $exchange)) { throw 'Sandbox output must be below the bound exchange root.' }
if (Test-Path -LiteralPath $outputPath) { throw 'Sandbox output already exists.' }

$worker = File-Record $workerSource
$cdp = File-Record $cdpSource
$exit = File-Record $exitSource
$node = File-Record ((Get-Command node.exe -ErrorAction Stop).Source)
$shortScenario = @($config.scenarios | Where-Object { [string]$_.id -eq 'FT-16' })
if ($shortScenario.Count -ne 1 -or @($shortScenario[0].inputs).Count -ne 2) { throw 'FT-16 inputs are not exact.' }
$shortAudio = Assert-Record $shortScenario[0].inputs[0] 'FT-16 short audio'
$shortReference = Assert-Record $shortScenario[0].inputs[1] 'FT-16 short reference'

$stage = Join-Path $exchange 'sandbox-stage'
if (Test-Path -LiteralPath $stage) { throw 'Sandbox stage already exists.' }
[System.IO.Directory]::CreateDirectory($stage) | Out-Null
Assert-NoReparse $stage $exchange
$staged = [ordered]@{
    worker = Copy-Exclusive $worker.path (Join-Path $stage 'worker.ps1') 'worker'
    cdp = Copy-Exclusive $cdp.path (Join-Path $stage 'moss-functional-ft-cdp.mjs') 'CDP script'
    exit = Copy-Exclusive $exit.path (Join-Path $stage 'cdp-exit-app.mjs') 'CDP exit script'
    node = Copy-Exclusive $node.path (Join-Path $stage 'node.exe') 'Node runtime'
    candidate = Copy-Exclusive $candidate.path (Join-Path $stage 'candidate-installer.exe') 'candidate installer'
    build_manifest = Copy-Exclusive $buildManifestRecord.path (Join-Path $stage 'candidate-build-manifest.json') 'candidate build manifest'
    short_audio = Copy-Exclusive $shortAudio.path (Join-Path $stage 'short-input.wav') 'short audio'
    short_reference = Copy-Exclusive $shortReference.path (Join-Path $stage 'short-reference.tsv') 'short reference'
}
$stagedModels = [ordered]@{}
foreach ($role in @('moss', 'whisper', 'qwen_2b')) {
    $record = Assert-Record $sidecar.models.PSObject.Properties[$role].Value "formal $role model"
    $stagedModels[$role] = Copy-Exclusive $record.path (Join-Path $stage ("models\$role\" + [System.IO.Path]::GetFileName($record.path))) "$role model"
}
$runtimeRoot = Full-Path ([string]$sidecar.moss_runtime.root)
$runtimeStage = Join-Path $stage 'runtime'
[System.IO.Directory]::CreateDirectory($runtimeStage) | Out-Null
$stagedRuntime = @()
foreach ($row in @($sidecar.moss_runtime.files)) {
    $relative = ([string]$row.relative_path).Replace('/', '\')
    if ([System.IO.Path]::IsPathRooted($relative) -or @($relative.Split('\') | Where-Object { $_ -in @('', '.', '..') }).Count -ne 0) {
        throw 'MOSS runtime contains an unsafe relative path.'
    }
    $source = Join-Path $runtimeRoot $relative
    $actual = Assert-Record ([ordered]@{ path = $source; bytes = $row.bytes; sha256 = $row.sha256 }) "runtime $relative"
    $copied = Copy-Exclusive $actual.path (Join-Path $runtimeStage $relative) "runtime $relative"
    $stagedRuntime += [ordered]@{ relative_path = $relative.Replace('\', '/'); bytes = $copied.bytes; sha256 = $copied.sha256 }
}

$sandboxInputPath = Join-Path $stage 'sandbox-input.json'
$sandboxInput = [ordered]@{
    schema_version = 1
    stage = 'MOSS_FUNCTIONAL_FT_SANDBOX_INPUT'
    run_id = $RunId
    source_commit = $SourceCommit.ToLowerInvariant()
    candidate_sha256 = $CandidateSha256.ToUpperInvariant()
    product_name = [string]$sidecar.product_name
    bundle_id = [string]$sidecar.bundle_id
    version = [string]$sidecar.version
    files = $staged
    models = $stagedModels
    runtime_files = $stagedRuntime
    installed_files = @($buildManifest.installed_files)
    host_computer_name = [string]$env:COMPUTERNAME
    worker_source = $worker
    cdp_source = $cdp
    node_source = $node
}
Write-JsonExclusive $sandboxInput $sandboxInputPath

$escapedExchange = [System.Security.SecurityElement]::Escape($exchange)
$command = 'powershell.exe -NoLogo -NoProfile -NonInteractive -ExecutionPolicy Bypass -File C:\MossFtExchange\sandbox-stage\worker.ps1 -InputPath C:\MossFtExchange\sandbox-stage\sandbox-input.json -Output C:\MossFtExchange\fault-chain-result.private.json'
$escapedCommand = [System.Security.SecurityElement]::Escape($command)
$wsbPath = Join-Path $exchange 'fault-chain.wsb'
$wsb = @"
<Configuration>
  <VGpu>Disable</VGpu>
  <Networking>Enable</Networking>
  <AudioInput>Disable</AudioInput>
  <VideoInput>Disable</VideoInput>
  <ProtectedClient>Enable</ProtectedClient>
  <PrinterRedirection>Disable</PrinterRedirection>
  <ClipboardRedirection>Disable</ClipboardRedirection>
  <MappedFolders>
    <MappedFolder>
      <HostFolder>$escapedExchange</HostFolder>
      <SandboxFolder>C:\MossFtExchange</SandboxFolder>
      <ReadOnly>false</ReadOnly>
    </MappedFolder>
  </MappedFolders>
  <LogonCommand><Command>$escapedCommand</Command></LogonCommand>
</Configuration>
"@
[System.IO.File]::WriteAllText($wsbPath, $wsb, [System.Text.UTF8Encoding]::new($false))
$wsbRecord = File-Record $wsbPath

$process = Start-Process -FilePath $sandboxExe -ArgumentList (ConvertTo-NativeArgument -Value $wsbPath) -PassThru -Wait
if ($process.ExitCode -ne 0) { throw "Windows Sandbox exited with code $($process.ExitCode)." }
if (-not (Test-Path -LiteralPath $outputPath -PathType Leaf)) { throw 'Windows Sandbox closed without writing the exact result.' }
$result = Read-Json $outputPath 'Sandbox worker result'
if ([string]$result.stage -ne 'MOSS_FUNCTIONAL_FT_SANDBOX_FAULT_RESULT' -or [string]$result.run_id -ne $RunId -or
    ([string]$result.source_commit).ToLowerInvariant() -ne $SourceCommit.ToLowerInvariant() -or
    -not ([string]$result.candidate_sha256).Equals($CandidateSha256, [System.StringComparison]::OrdinalIgnoreCase)) {
    throw 'Sandbox worker result identity is wrong.'
}
[ordered]@{
    status = 'PASS'
    stage = 'MOSS_FUNCTIONAL_FT_SANDBOX_LAUNCH'
    run_id = $RunId
    worker = $worker
    wsb = $wsbRecord
    result = File-Record $outputPath
    windows_sandbox_exit_code = [int]$process.ExitCode
} | ConvertTo-Json -Depth 8

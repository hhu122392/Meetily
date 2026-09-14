[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)][string]$AcceptanceConfig,
    [Parameter(Mandatory = $true)][string]$Values,
    [Parameter(Mandatory = $true)][string]$Output
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'

$scriptRoot = Split-Path -Parent $MyInvocation.MyCommand.Path
$repoRoot = [System.IO.Path]::GetFullPath((Join-Path $scriptRoot '..\..'))
$fixedSandboxLauncher = Join-Path $scriptRoot 'invoke-moss-functional-ft-sandbox.ps1'
$approvedProductName = 'meetily-p6-lifecycle'
$approvedBundleId = 'com.meetily.ai.p6lifecycle'
$requiredGroups = @(
    'ft01-03-live-and-persistence',
    'ft04-15-moss-chain',
    'ft16-21-fault-chain',
    'ft22-25-install-lifecycle',
    'ft26-data-drive-placement',
    'ft27-long-audio',
    'ft28-business-chain'
)
$modelContracts = [ordered]@{
    moss = [ordered]@{
        filename = 'MOSS-Transcribe-Diarize-Q8_0.gguf'
        bytes = [int64]986899616
        sha256 = '64EC654DC6FFCFDFE180422DFFCE1D33422B0C30959B7EDFD131BAD77EE35039'
    }
    whisper = [ordered]@{
        filename = 'ggml-large-v3-turbo-q5_0.bin'
        bytes = [int64]574041195
        sha256 = '394221709CD5AD1F40C46E6031CA61BCE88931E6E088C188294C6D5A55FFA7E2'
    }
    qwen_2b = [ordered]@{
        filename = 'Qwen3.5-2B-Q4_K_M.gguf'
        bytes = [int64]1280835840
        sha256 = 'AAF42C8B7C3CAB2BF3D69C355048D4A0EE9973D48F16C731C0520EE914699223'
    }
}
$runtimeContract = [ordered]@{
    bytes = [int64]125
    sha256 = 'C266622EE16A69C458EBA57FD2B2F2114B9C5F2E34292E9603F1CE46C7A1DC73'
}
$q00Roles = @(
    'bindings', 'window_audio', 'window_manifest', 'moss_raw', 'whisper_same_window',
    'corrected', 'human_verbatim', 'speaker_truth', 'positive_truth', 'negative_truth',
    'activation_evidence', 'summary_evidence', 'public_report', 'private_report'
)

function Get-NormalizedFullPath {
    param([Parameter(Mandatory = $true)][string]$Path)
    $full = [System.IO.Path]::GetFullPath($Path)
    $root = [System.IO.Path]::GetPathRoot($full)
    if ($full.Equals($root, [System.StringComparison]::OrdinalIgnoreCase)) { return $root }
    return $full.TrimEnd('\', '/')
}

function Test-StrictChildPath {
    param([Parameter(Mandatory = $true)][string]$Candidate, [Parameter(Mandatory = $true)][string]$Parent)
    $child = Get-NormalizedFullPath $Candidate
    $root = Get-NormalizedFullPath $Parent
    return $child.StartsWith(
        $root.TrimEnd('\', '/') + [System.IO.Path]::DirectorySeparatorChar,
        [System.StringComparison]::OrdinalIgnoreCase
    )
}

function Assert-NoReparseComponents {
    param(
        [Parameter(Mandatory = $true)][string]$Path,
        [Parameter(Mandatory = $true)][string]$Boundary,
        [switch]$AllowMissing
    )
    $full = Get-NormalizedFullPath $Path
    $root = Get-NormalizedFullPath $Boundary
    if (-not $full.Equals($root, [System.StringComparison]::OrdinalIgnoreCase) -and
        -not (Test-StrictChildPath -Candidate $full -Parent $root)) {
        throw "Path is outside its approved boundary: $full"
    }
    $cursor = $full
    while ($true) {
        if (Test-Path -LiteralPath $cursor) {
            $item = Get-Item -LiteralPath $cursor -Force
            if (($item.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0) {
                throw "Path contains a reparse point: $cursor"
            }
        } elseif (-not $AllowMissing) {
            throw "Path is missing: $cursor"
        }
        if ($cursor.Equals($root, [System.StringComparison]::OrdinalIgnoreCase)) { break }
        $parent = [System.IO.Path]::GetDirectoryName($cursor)
        if ([string]::IsNullOrWhiteSpace($parent) -or $parent.Equals($cursor, [System.StringComparison]::OrdinalIgnoreCase)) {
            throw "Could not reach approved boundary: $root"
        }
        $cursor = Get-NormalizedFullPath $parent
    }
    return $full
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

function Read-JsonObject {
    param([Parameter(Mandatory = $true)][string]$Path, [Parameter(Mandatory = $true)][string]$Label)
    $full = Get-NormalizedFullPath $Path
    if (-not (Test-Path -LiteralPath $full -PathType Leaf)) { throw "$Label is missing: $full" }
    try { $value = Get-Content -LiteralPath $full -Raw -Encoding UTF8 | ConvertFrom-Json }
    catch { throw "$Label is not valid JSON: $($_.Exception.Message)" }
    if ($null -eq $value -or $value -is [System.Array]) { throw "$Label must be a JSON object." }
    return $value
}

function Get-FileRecord {
    param([Parameter(Mandatory = $true)][string]$Path)
    $full = Get-NormalizedFullPath $Path
    Assert-NoReparseComponents -Path $full -Boundary ([System.IO.Path]::GetPathRoot($full)) | Out-Null
    if (-not (Test-Path -LiteralPath $full -PathType Leaf)) { throw "Required file is missing: $full" }
    $before = Get-Item -LiteralPath $full -Force
    $hash = (Get-FileHash -LiteralPath $full -Algorithm SHA256).Hash.ToUpperInvariant()
    $after = Get-Item -LiteralPath $full -Force
    if ([int64]$before.Length -ne [int64]$after.Length -or $before.LastWriteTimeUtc.Ticks -ne $after.LastWriteTimeUtc.Ticks) {
        throw "File changed while being hashed: $full"
    }
    return [ordered]@{ path = $full; bytes = [int64]$after.Length; sha256 = $hash }
}

function Assert-RecordMatches {
    param($Declared, [Parameter(Mandatory = $true)][string]$Label)
    Assert-ExactProperties $Declared @('path', 'bytes', 'sha256') $Label
    $actual = Get-FileRecord ([string]$Declared.path)
    if ([int64]$Declared.bytes -ne [int64]$actual.bytes -or
        -not ([string]$Declared.sha256).Equals([string]$actual.sha256, [System.StringComparison]::OrdinalIgnoreCase)) {
        throw "$Label does not match the real file."
    }
    return $actual
}

function Get-DirectoryFileRecords {
    param([Parameter(Mandatory = $true)][string]$Root)
    $full = Get-NormalizedFullPath $Root
    Assert-NoReparseComponents -Path $full -Boundary ([System.IO.Path]::GetPathRoot($full)) | Out-Null
    if (-not (Test-Path -LiteralPath $full -PathType Container)) { throw "Required directory is missing: $full" }
    return @(
        Get-ChildItem -LiteralPath $full -Recurse -Force | Sort-Object FullName | ForEach-Object {
            Assert-NoReparseComponents -Path $_.FullName -Boundary $full | Out-Null
            if (-not $_.PSIsContainer) {
                [ordered]@{
                    relative_path = $_.FullName.Substring($full.Length).TrimStart('\').Replace('\', '/')
                    bytes = [int64]$_.Length
                    sha256 = (Get-FileHash -LiteralPath $_.FullName -Algorithm SHA256).Hash.ToUpperInvariant()
                }
            }
        }
    )
}

function Assert-ModelFile {
    param([Parameter(Mandatory = $true)][string]$Role, [Parameter(Mandatory = $true)][string]$Path)
    $record = Get-FileRecord $Path
    $contract = $modelContracts[$Role]
    if (-not [System.IO.Path]::GetFileName($record.path).Equals([string]$contract.filename, [System.StringComparison]::OrdinalIgnoreCase) -or
        [int64]$record.bytes -ne [int64]$contract.bytes -or
        -not ([string]$record.sha256).Equals([string]$contract.sha256, [System.StringComparison]::OrdinalIgnoreCase)) {
        throw "Model $Role is not the approved formal file."
    }
    return $record
}

function Get-NonemptyStrings {
    param($Value, [Parameter(Mandatory = $true)][string]$Label)
    $items = @($Value)
    if ($items.Count -eq 0) { throw "$Label must not be empty." }
    $result = @()
    foreach ($item in $items) {
        $text = [string]$item
        if ([string]::IsNullOrWhiteSpace($text) -or $text -match '[\r\n\x00]') { throw "$Label contains an invalid value." }
        $result += $text
    }
    if (@($result | Sort-Object -Unique).Count -ne $result.Count) { throw "$Label contains duplicates." }
    return $result
}

function Get-CdpPort {
    param($Value, [Parameter(Mandatory = $true)][string]$Label)
    $port = [int]$Value
    if ($port -lt 1024 -or $port -gt 65535) { throw "$Label must be between 1024 and 65535." }
    return $port
}

function Get-PrivateDirectory {
    param(
        [Parameter(Mandatory = $true)][string]$Path,
        [Parameter(Mandatory = $true)][string]$PrivateRoot,
        [switch]$MustRemainMissing
    )
    $full = Get-NormalizedFullPath $Path
    if (-not (Test-StrictChildPath -Candidate $full -Parent $PrivateRoot)) { throw "Private producer path escapes the private evidence root: $full" }
    if ($MustRemainMissing) {
        if (Test-Path -LiteralPath $full) { throw "Invalid-path fixture must remain missing: $full" }
        $parent = Split-Path -Parent $full
        if (-not (Test-Path -LiteralPath $parent -PathType Container)) { [System.IO.Directory]::CreateDirectory($parent) | Out-Null }
        Assert-NoReparseComponents -Path $parent -Boundary $PrivateRoot | Out-Null
    } else {
        if (-not (Test-Path -LiteralPath $full -PathType Container)) { [System.IO.Directory]::CreateDirectory($full) | Out-Null }
        Assert-NoReparseComponents -Path $full -Boundary $PrivateRoot | Out-Null
    }
    return $full
}

function Write-JsonExclusive {
    param([Parameter(Mandatory = $true)]$Value, [Parameter(Mandatory = $true)][string]$Path)
    $full = Get-NormalizedFullPath $Path
    if (Test-Path -LiteralPath $full) { throw "Refusing to overwrite producer input: $full" }
    $parent = Split-Path -Parent $full
    if (-not (Test-Path -LiteralPath $parent -PathType Container)) { throw "Producer input parent must already exist: $parent" }
    Assert-NoReparseComponents -Path $parent -Boundary ([System.IO.Path]::GetPathRoot($parent)) | Out-Null
    $json = ($Value | ConvertTo-Json -Depth 100) + [Environment]::NewLine
    $bytes = [System.Text.UTF8Encoding]::new($false).GetBytes($json)
    $stream = [System.IO.File]::Open($full, [System.IO.FileMode]::CreateNew, [System.IO.FileAccess]::Write, [System.IO.FileShare]::None)
    try { $stream.Write($bytes, 0, $bytes.Length); $stream.Flush($true) } finally { $stream.Dispose() }
}

$configPath = Get-NormalizedFullPath $AcceptanceConfig
$valuesPath = Get-NormalizedFullPath $Values
$outputPath = Get-NormalizedFullPath $Output
$expectedOutput = [System.IO.Path]::ChangeExtension($configPath, '.producer.private.json')
if (-not $outputPath.Equals($expectedOutput, [System.StringComparison]::OrdinalIgnoreCase)) {
    throw "Output must be the acceptance-bound sidecar path: $expectedOutput"
}
if (Test-StrictChildPath -Candidate $configPath -Parent $repoRoot -or Test-StrictChildPath -Candidate $outputPath -Parent $repoRoot) {
    throw 'Materialized acceptance and producer input must remain outside the repository.'
}
if (Test-Path -LiteralPath $outputPath) { throw "Refusing to overwrite producer input: $outputPath" }

$acceptance = Read-JsonObject $configPath 'acceptance config'
$valuesDocument = Read-JsonObject $valuesPath 'producer values'
Assert-ExactProperties $valuesDocument @('schema_version', 'stage', 'python_path', 'models', 'moss_runtime_root', 'groups') 'producer values'
if ([int]$valuesDocument.schema_version -ne 1 -or [string]$valuesDocument.stage -ne 'MOSS_FUNCTIONAL_FT_PRODUCER_VALUES') {
    throw 'Producer values schema/stage is invalid.'
}
if ([int]$acceptance.schema_version -ne 1 -or [string]$acceptance.stage -ne 'MOSS_FUNCTIONAL_FIX_ACCEPTANCE_CONFIG' -or [bool]$acceptance.template_only) {
    throw 'Acceptance config is not a materialized formal config.'
}
$head = (& git -C $repoRoot rev-parse --verify HEAD 2>$null).Trim().ToLowerInvariant()
if ($LASTEXITCODE -ne 0 -or $head -ne ([string]$acceptance.source_commit).ToLowerInvariant()) {
    throw 'Acceptance source_commit does not match repository HEAD.'
}
$candidate = Assert-RecordMatches $acceptance.candidate 'acceptance candidate'
$buildManifestRecord = Assert-RecordMatches $acceptance.build_manifest 'acceptance build manifest'
$build = Read-JsonObject $buildManifestRecord.path 'candidate build manifest'
if ([string]$build.role -ne 'candidate' -or ([string]$build.source_commit).ToLowerInvariant() -ne $head -or
    [string]$build.product_name -ne $approvedProductName -or [string]$build.bundle_id -ne $approvedBundleId -or
    -not ([string]$build.installer.sha256).Equals($candidate.sha256, [System.StringComparison]::OrdinalIgnoreCase)) {
    throw 'Candidate build manifest identity does not match acceptance.'
}
$publicRoot = Get-NormalizedFullPath ([string]$acceptance.evidence_roots.public)
$privateRoot = Get-NormalizedFullPath ([string]$acceptance.evidence_roots.private)
foreach ($root in @($publicRoot, $privateRoot)) {
    Assert-NoReparseComponents -Path $root -Boundary ([System.IO.Path]::GetPathRoot($root)) | Out-Null
    if (-not (Test-Path -LiteralPath $root -PathType Container)) { throw "Evidence root is missing: $root" }
}
if ($publicRoot.Equals($privateRoot, [System.StringComparison]::OrdinalIgnoreCase) -or
    (Test-StrictChildPath -Candidate $publicRoot -Parent $privateRoot) -or
    (Test-StrictChildPath -Candidate $privateRoot -Parent $publicRoot)) {
    throw 'Public and private evidence roots must be separate and non-nested.'
}

Assert-ExactProperties $valuesDocument.models @('moss', 'whisper', 'qwen_2b') 'producer model values'
$models = [ordered]@{
    moss = Assert-ModelFile 'moss' ([string]$valuesDocument.models.moss)
    whisper = Assert-ModelFile 'whisper' ([string]$valuesDocument.models.whisper)
    qwen_2b = Assert-ModelFile 'qwen_2b' ([string]$valuesDocument.models.qwen_2b)
}
$runtimeRoot = Get-NormalizedFullPath ([string]$valuesDocument.moss_runtime_root)
$runtimeFiles = @(Get-DirectoryFileRecords $runtimeRoot)
if ($runtimeFiles.Count -lt 4) { throw 'MOSS runtime package is incomplete.' }
$contractRows = @($runtimeFiles | Where-Object { [string]$_.relative_path -eq 'contract.json' })
if ($contractRows.Count -ne 1 -or [int64]$contractRows[0].bytes -ne $runtimeContract.bytes -or
    -not ([string]$contractRows[0].sha256).Equals($runtimeContract.sha256, [System.StringComparison]::OrdinalIgnoreCase)) {
    throw 'MOSS runtime contract is not the approved v0.2.2 package.'
}
$runtime = [ordered]@{
    root = $runtimeRoot
    contract = Get-FileRecord (Join-Path $runtimeRoot 'contract.json')
    files = $runtimeFiles
}

Assert-ExactProperties $valuesDocument.groups $requiredGroups 'producer value groups'
$g01 = $valuesDocument.groups.'ft01-03-live-and-persistence'
$g04 = $valuesDocument.groups.'ft04-15-moss-chain'
$g16 = $valuesDocument.groups.'ft16-21-fault-chain'
$g22 = $valuesDocument.groups.'ft22-25-install-lifecycle'
$g26 = $valuesDocument.groups.'ft26-data-drive-placement'
$g27 = $valuesDocument.groups.'ft27-long-audio'
$g28 = $valuesDocument.groups.'ft28-business-chain'
Assert-ExactProperties $g01 @('cdp_port', 'recording_root', 'whisper_model', 'qwen_model', 'short_duration_seconds') 'FT-01 values'
Assert-ExactProperties $g04 @('cdp_port', 'positive_terms', 'negative_terms') 'FT-04 values'
Assert-ExactProperties $g16 @('sandbox_exchange_root') 'FT-16 values'
Assert-ExactProperties $g22 @('baseline_build_manifest', 'protected_baseline_manifest', 'fixture_database') 'FT-22 values'
Assert-ExactProperties $g26 @('cdp_port', 'selected_recording_root', 'default_recording_root', 'invalid_recording_root', 'short_duration_seconds') 'FT-26 values'
Assert-ExactProperties $g27 @('cdp_port', 'expected_duration_seconds') 'FT-27 values'
Assert-ExactProperties $g28 @('cdp_port', 'expected_duration_seconds', 'positive_terms', 'negative_terms', 'q00') 'FT-28 values'
Assert-ExactProperties $g28.q00 $q00Roles 'FT-28 Q00 values'

$ports = @(
    Get-CdpPort $g01.cdp_port 'FT-01 cdp_port'
    Get-CdpPort $g04.cdp_port 'FT-04 cdp_port'
    Get-CdpPort $g26.cdp_port 'FT-26 cdp_port'
    Get-CdpPort $g27.cdp_port 'FT-27 cdp_port'
    Get-CdpPort $g28.cdp_port 'FT-28 cdp_port'
)
if (@($ports | Sort-Object -Unique).Count -ne $ports.Count) { throw 'Every app-running FT group must use a distinct CDP port.' }
if ([double]$g01.short_duration_seconds -le 0 -or [double]$g26.short_duration_seconds -le 0) { throw 'Short input durations must be positive.' }
if ([math]::Abs([double]$g27.expected_duration_seconds - 3096.62) -gt 0.000001) { throw 'FT-27 duration must be exactly 3096.62.' }
if ([math]::Abs([double]$g28.expected_duration_seconds - 737.728) -gt 0.000001) { throw 'FT-28 duration must be exactly 737.728.' }
if ([string]$g01.whisper_model -ne 'large-v3-turbo-q5_0' -or [string]$g01.qwen_model -ne 'qwen3.5:2b') {
    throw 'FT-01 model selections do not match the bound formal models.'
}

$q00 = [ordered]@{}
foreach ($role in $q00Roles) { $q00[$role] = Get-FileRecord ([string]$g28.q00.$role) }
$groups = [ordered]@{
    'ft01-03-live-and-persistence' = [ordered]@{
        cdp_port = $ports[0]
        recording_root = Get-PrivateDirectory ([string]$g01.recording_root) $privateRoot
        whisper_model = [string]$g01.whisper_model
        qwen_model = [string]$g01.qwen_model
        short_duration_seconds = [double]$g01.short_duration_seconds
    }
    'ft04-15-moss-chain' = [ordered]@{
        cdp_port = $ports[1]
        positive_terms = @(Get-NonemptyStrings $g04.positive_terms 'FT-04 positive terms')
        negative_terms = @(Get-NonemptyStrings $g04.negative_terms 'FT-04 negative terms')
    }
    'ft16-21-fault-chain' = [ordered]@{
        sandbox_launcher = Get-FileRecord $fixedSandboxLauncher
        sandbox_exchange_root = Get-PrivateDirectory ([string]$g16.sandbox_exchange_root) $privateRoot
    }
    'ft22-25-install-lifecycle' = [ordered]@{
        baseline_build_manifest = Get-FileRecord ([string]$g22.baseline_build_manifest)
        protected_baseline_manifest = Get-FileRecord ([string]$g22.protected_baseline_manifest)
        fixture_database = Get-FileRecord ([string]$g22.fixture_database)
    }
    'ft26-data-drive-placement' = [ordered]@{
        cdp_port = $ports[2]
        selected_recording_root = Get-PrivateDirectory ([string]$g26.selected_recording_root) $privateRoot
        default_recording_root = Get-PrivateDirectory ([string]$g26.default_recording_root) $privateRoot
        invalid_recording_root = Get-PrivateDirectory ([string]$g26.invalid_recording_root) $privateRoot -MustRemainMissing
        short_duration_seconds = [double]$g26.short_duration_seconds
    }
    'ft27-long-audio' = [ordered]@{
        cdp_port = $ports[3]
        expected_duration_seconds = [double]$g27.expected_duration_seconds
    }
    'ft28-business-chain' = [ordered]@{
        cdp_port = $ports[4]
        expected_duration_seconds = [double]$g28.expected_duration_seconds
        positive_terms = @(Get-NonemptyStrings $g28.positive_terms 'FT-28 positive terms')
        negative_terms = @(Get-NonemptyStrings $g28.negative_terms 'FT-28 negative terms')
        q00 = $q00
    }
}

$sidecar = [ordered]@{
    schema_version = 1
    stage = 'MOSS_FUNCTIONAL_FT_PRODUCER_INPUT'
    created_at = [datetimeoffset]::UtcNow.ToString('o')
    run_id = [string]$acceptance.run_id
    source_commit = $head
    candidate_sha256 = $candidate.sha256
    build_manifest_sha256 = $buildManifestRecord.sha256
    acceptance_sha256 = (Get-FileRecord $configPath).sha256
    product_name = $approvedProductName
    bundle_id = $approvedBundleId
    version = [string]$build.version
    producer = Get-FileRecord $MyInvocation.MyCommand.Path
    values = Get-FileRecord $valuesPath
    python = Get-FileRecord ([string]$valuesDocument.python_path)
    models = $models
    moss_runtime = $runtime
    groups = $groups
}
Write-JsonExclusive -Value $sidecar -Path $outputPath
$result = Get-FileRecord $outputPath
[ordered]@{
    status = 'PASS'
    stage = 'MOSS_FUNCTIONAL_FT_PRODUCER_INPUT_CREATED'
    run_id = $sidecar.run_id
    source_commit = $sidecar.source_commit
    output = $result
    model_roles = @($models.Keys)
    runtime_file_count = $runtimeFiles.Count
    group_count = $requiredGroups.Count
} | ConvertTo-Json -Depth 8

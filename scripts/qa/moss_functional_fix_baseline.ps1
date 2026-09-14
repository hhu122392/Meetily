param(
    [string]$SourceRoot = 'D:\MeetilyBuildScratch\meetily-moss-functional-fixes-20260902',
    [string]$BaseCommit = '1b9c29a656d267a857d9f5d982d38a3f62f1df4f',
    [string]$ExpectedBranch = 'codex/moss-functional-fixes-20260902',
    [string]$RunId = 'FIX-1B9C29A-20260902-01',
    [string]$PublicOutputRoot = 'D:\MeetilyBuildScratch\meetily-moss-functional-fixes-20260902\target\release\docs\方案\MOSS功能修复验证结果-20260902\FIX-1B9C29A-20260902-01',
    [string]$PrivateOutputRoot = 'D:\MeetilyData\private-evidence\moss-functional-fix-20260902\FIX-1B9C29A-20260902-01'
)

$ErrorActionPreference = 'Stop'

function Write-Utf8JsonExclusiveAtomic {
    param(
        [Parameter(Mandatory = $true)]$Value,
        [Parameter(Mandatory = $true)][string]$Path
    )
    $fullPath = [System.IO.Path]::GetFullPath($Path)
    if (Test-Path -LiteralPath $fullPath) { throw "Refusing to overwrite frozen baseline evidence: $fullPath" }
    $parent = Split-Path -Parent $fullPath
    [System.IO.Directory]::CreateDirectory($parent) | Out-Null
    $temporary = Join-Path $parent ('.' + [System.IO.Path]::GetFileName($fullPath) + '.' + [Guid]::NewGuid().ToString('N') + '.tmp')
    try {
        [System.IO.File]::WriteAllText(
            $temporary,
            (($Value | ConvertTo-Json -Depth 20) + [Environment]::NewLine),
            [System.Text.UTF8Encoding]::new($false)
        )
        Move-Item -LiteralPath $temporary -Destination $fullPath
    } finally {
        if (Test-Path -LiteralPath $temporary -PathType Leaf) { Remove-Item -LiteralPath $temporary -Force }
    }
}

function Get-FileEvidence {
    param(
        [Parameter(Mandatory = $true)][string]$Role,
        [Parameter(Mandatory = $true)][string]$Path,
        [string]$ExpectedSha256
    )
    $exists = Test-Path -LiteralPath $Path -PathType Leaf
    $item = if ($exists) { Get-Item -LiteralPath $Path } else { $null }
    $sha256 = if ($exists) { (Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash } else { $null }
    [ordered]@{
        role = $Role
        path = if ($exists) { $item.FullName } else { [System.IO.Path]::GetFullPath($Path) }
        exists = $exists
        bytes = if ($exists) { [int64]$item.Length } else { $null }
        last_write_time = if ($exists) { $item.LastWriteTime.ToString('o') } else { $null }
        sha256 = $sha256
        expected_sha256 = if ([string]::IsNullOrWhiteSpace($ExpectedSha256)) { $null } else { $ExpectedSha256 }
        matches_expected = if ([string]::IsNullOrWhiteSpace($ExpectedSha256)) { $null } else { $exists -and $sha256 -eq $ExpectedSha256 }
    }
}

function Get-ProductProcesses {
    $names = @('meetily', 'meetily-p6-lifecycle', 'moss-helper', 'moss_helper', 'llama-helper', 'llama_helper', 'ffmpeg')
    @(
        Get-Process -ErrorAction SilentlyContinue |
            Where-Object { $_.ProcessName -in $names } |
            ForEach-Object {
                [ordered]@{
                    name = $_.ProcessName
                    id = $_.Id
                    started_at = try { $_.StartTime.ToString('o') } catch { $null }
                }
            }
    )
}

$inputs = @(
    Get-FileEvidence -Role 'moss_model' -Path 'D:\MeetilyData\staging\moss-p1\MOSS-Transcribe-Diarize-Q8_0.gguf' -ExpectedSha256 '64EC654DC6FFCFDFE180422DFFCE1D33422B0C30959B7EDFD131BAD77EE35039'
    Get-FileEvidence -Role 'moss_model_license' -Path 'D:\MeetilyData\staging\moss-p1\LICENSE-MOSS-Transcribe-Diarize-Apache-2.0.txt' -ExpectedSha256 'C71D239DF91726FC519C6EB72D318EC65820627232B2F796219E87DCF35D0AB4'
    Get-FileEvidence -Role 'whisper_model' -Path 'C:\Users\liuxin\AppData\Roaming\com.meetily.ai\models\ggml-large-v3-turbo-q5_0.bin' -ExpectedSha256 '394221709CD5AD1F40C46E6031CA61BCE88931E6E088C188294C6D5A55FFA7E2'
    Get-FileEvidence -Role 'qwen_2b_summary_model' -Path 'C:\Users\liuxin\AppData\Roaming\com.meetily.ai\models\summary\Qwen3.5-2B-Q4_K_M.gguf' -ExpectedSha256 'AAF42C8B7C3CAB2BF3D69C355048D4A0EE9973D48F16C731C0520EE914699223'
    Get-FileEvidence -Role 'business_audio_737s' -Path 'D:\桌面\meetlily\target\release\docs\方案\证据\MOSS-S8-M00-R3-20260828-110430\source-context-frozen-real-business.wav' -ExpectedSha256 '82829A5D2011109F69C6526C6E87AFD767B18F9AFDFFF87B93260AF5073382BB'
    Get-FileEvidence -Role 'business_audio_737s_chunk_manifest' -Path 'D:\MeetilyData\staging\moss-p1\inputs\business_737s-chunks-82829A5D2011109F-D1E777FE1E4F4435\manifest.json' -ExpectedSha256 '513A827ACBB4F5E72E89E409573C094E830FA57B1372C230A428AA865150608F'
    Get-FileEvidence -Role 'long_audio_3096s' -Path 'D:\MeetilyData\staging\moss-p1\prepared-freeze\long_3096s.wav' -ExpectedSha256 '2C293CAE418ACC6FA8CB5A0620B966FDF0C1E5629D730ABB5A249D1DEC850FE6'
    Get-FileEvidence -Role 'long_audio_3096s_chunk_manifest' -Path 'D:\MeetilyData\staging\moss-p1\inputs\long_3096s-chunks-FE6BAB06F9B64DD4-5364B7CE6F7A5FA8\manifest.json' -ExpectedSha256 '2B77448B160221BDCC5C320B59F61D1503521EC9EC702F5FF2DFEBC31274B546'
    Get-FileEvidence -Role 'moss_runtime_contract' -Path 'D:\MeetilyData\staging\v0\runtime\transcribe-native-windows-x86_64-cpu-vulkan\contract.json' -ExpectedSha256 'C266622EE16A69C458EBA57FD2B2F2114B9C5F2E34292E9603F1CE46C7A1DC73'
    Get-FileEvidence -Role 'fixture_ffmpeg' -Path 'D:\桌面\meetlily\target\release\ffmpeg.exe' -ExpectedSha256 '5AF82A0D4FE2B9EAE211B967332EA97EDFC51C6B328CA35B827E73EAC560DC0D'
    Get-FileEvidence -Role 'rollback_baseline_0_4_1' -Path 'D:\MeetilyData\private-evidence\moss-v3-p6-host-isolated-lifecycle-20260830\artifacts\0.4.1\meetily-p6-lifecycle_0.4.1_x64-setup.exe' -ExpectedSha256 '1C151B1534A66927FFA5B50DE58D05EE27247B933797441A59C747510C32483C'
)

$userDataRoot = 'C:\Users\liuxin\AppData\Roaming\com.meetily.ai'
$protectedNames = @(
    'analytics.json',
    'meeting_minutes.sqlite',
    'onboarding-status.json',
    'preferences.json',
    'recording_preferences.json',
    'summary-template-preferences.v1.json',
    'ui-locale.json'
)
$protected = @(
    $protectedNames | ForEach-Object {
        Get-FileEvidence -Role ('protected_user_data_' + $_) -Path (Join-Path $userDataRoot $_)
    }
)

$publicPath = Join-Path $PublicOutputRoot 'baseline.public.json'
$privatePath = Join-Path $PrivateOutputRoot 'baseline.private.json'
$protectedPath = Join-Path $PrivateOutputRoot 'protected-user-data.before.json'
foreach ($path in @($publicPath, $privatePath, $protectedPath)) {
    if (Test-Path -LiteralPath $path) { throw "Frozen baseline output already exists: $path" }
}

$currentHead = (& git -C $SourceRoot rev-parse HEAD).Trim()
$currentBranch = (& git -C $SourceRoot branch --show-current).Trim()
$worktreeStatus = @(& git -C $SourceRoot status --porcelain=v1 --untracked-files=all)
& git -C $SourceRoot merge-base --is-ancestor $BaseCommit $currentHead
$baseIsAncestor = $LASTEXITCODE -eq 0
$processes = @(Get-ProductProcesses)
$disk = Get-PSDrive -Name D
$os = Get-CimInstance Win32_OperatingSystem
$computer = Get-CimInstance Win32_ComputerSystem
$cpu = Get-CimInstance Win32_Processor | Select-Object -First 1
$gpu = Get-CimInstance Win32_VideoController | Select-Object -First 1

$gateErrors = @()
if (-not $baseIsAncestor) { $gateErrors += 'repair branch is not descended from the frozen base commit' }
if ($currentBranch -ne $ExpectedBranch) { $gateErrors += "unexpected branch: $currentBranch" }
if ($worktreeStatus.Count -ne 0) { $gateErrors += 'repair worktree is not completely clean' }
foreach ($input in $inputs) {
    if (-not $input.exists) { $gateErrors += "missing input: $($input.role)" }
    if ($input.matches_expected -eq $false) { $gateErrors += "input hash mismatch: $($input.role)" }
}
if (@($protected | Where-Object { -not $_.exists }).Count -ne 0) { $gateErrors += 'one or more protected user-data files are missing' }
if ($protected.Count -ne 7) { $gateErrors += "protected user-data count is $($protected.Count), expected 7" }
if ($processes.Count -ne 0) { $gateErrors += 'product or helper processes are already running' }
if ([int64]$disk.Free -lt 35GB) { $gateErrors += 'D drive free space is below 35 GiB' }

$generatedAt = (Get-Date).ToString('o')
$private = [ordered]@{
    schema_version = 1
    run_id = $RunId
    stage = 'FUNCTIONAL_FIX_BASELINE'
    generated_at = $generatedAt
    status = if ($gateErrors.Count -eq 0) { 'PASS' } else { 'BLOCKED' }
    gate_errors = $gateErrors
    source = [ordered]@{
        root = [System.IO.Path]::GetFullPath($SourceRoot)
        frozen_base_commit = $BaseCommit
        current_head = $currentHead
        branch = $currentBranch
        base_is_ancestor = $baseIsAncestor
        worktree_status = $worktreeStatus
    }
    host = [ordered]@{
        os = $os.Caption
        os_version = $os.Version
        os_build = $os.BuildNumber
        hypervisor_present = [bool]$computer.HypervisorPresent
        cpu = $cpu.Name
        gpu = $gpu.Name
        gpu_driver = $gpu.DriverVersion
        total_physical_memory_bytes = [int64]$computer.TotalPhysicalMemory
        d_drive_free_bytes = [int64]$disk.Free
        d_drive_used_bytes = [int64]$disk.Used
    }
    running_product_processes = $processes
    inputs = $inputs
    protected_user_data = $protected
}

$public = [ordered]@{
    schema_version = 1
    run_id = $RunId
    stage = 'FUNCTIONAL_FIX_BASELINE_PUBLIC'
    generated_at = $generatedAt
    status = $private.status
    gate_errors = $gateErrors
    source = [ordered]@{
        frozen_base_commit = $BaseCommit
        current_head = $currentHead
        branch = $currentBranch
        base_is_ancestor = $baseIsAncestor
        worktree_clean = $worktreeStatus.Count -eq 0
    }
    host = $private.host
    running_product_process_count = $processes.Count
    inputs = @(
        $inputs | ForEach-Object {
            [ordered]@{
                role = $_.role
                exists = $_.exists
                bytes = $_.bytes
                sha256 = $_.sha256
                matches_expected = $_.matches_expected
            }
        }
    )
    protected_user_data_file_count = $protected.Count
}

Write-Utf8JsonExclusiveAtomic -Value $public -Path $publicPath
Write-Utf8JsonExclusiveAtomic -Value $private -Path $privatePath
Write-Utf8JsonExclusiveAtomic -Value $protected -Path $protectedPath

[ordered]@{
    status = $private.status
    run_id = $RunId
    source_head = $currentHead
    input_count = $inputs.Count
    protected_user_data_file_count = $protected.Count
    running_product_process_count = $processes.Count
    gate_errors = $gateErrors
    public_output = $publicPath
    private_output = $privatePath
} | ConvertTo-Json -Depth 8

if ($gateErrors.Count -ne 0) { exit 2 }

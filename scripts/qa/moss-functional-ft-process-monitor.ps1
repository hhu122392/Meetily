[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)][string]$RootsJson,
    [Parameter(Mandatory = $true)][string]$ExpectedRolesJson,
    [Parameter(Mandatory = $true)][string]$StopSignal,
    [Parameter(Mandatory = $true)][string]$Output,
    [ValidateRange(20, 1000)][int]$PollMilliseconds = 100
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'

$scriptRoot = Split-Path -Parent $MyInvocation.MyCommand.Path
. (Join-Path $scriptRoot 'moss-functional-ft-evidence.ps1')

if (Test-Path -LiteralPath $Output) { throw "Refusing to overwrite process monitor output: $Output" }
$roots = @(ConvertFrom-Json -InputObject $RootsJson)
$expectedRoles = @(ConvertFrom-Json -InputObject $ExpectedRolesJson | ForEach-Object { [string]$_ })
if ($roots.Count -ne 2 -or $expectedRoles.Count -eq 0) { throw 'Process monitor roots or expected roles are invalid.' }

$hashCache = @{}
function Get-CachedFileSha256 {
    param([Parameter(Mandatory = $true)][string]$Path)
    $key = (Get-MossNormalizedPathForComparison $Path)
    if (-not $hashCache.ContainsKey($key)) {
        try { $hashCache[$key] = (Get-FileHash -LiteralPath $Path -Algorithm SHA256 -ErrorAction Stop).Hash.ToUpperInvariant() }
        catch { $hashCache[$key] = $null }
    }
    return $hashCache[$key]
}

function Get-MonotonicMilliseconds {
    return [System.Diagnostics.Stopwatch]::GetTimestamp() * 1000.0 / [System.Diagnostics.Stopwatch]::Frequency
}

function Get-ProcessInventory {
    $items = @()
    $query = 'SELECT ProcessId,ParentProcessId,ExecutablePath,Name,CreationDate FROM Win32_Process'
    foreach ($process in @(Get-CimInstance -Query $query -ErrorAction SilentlyContinue)) {
        if ([string]::IsNullOrWhiteSpace([string]$process.ExecutablePath)) { continue }
        $items += [ordered]@{
            pid = [int]$process.ProcessId
            parent_pid = [int]$process.ParentProcessId
            executable_path = [string]$process.ExecutablePath
            executable_sha256 = Get-CachedFileSha256 -Path ([string]$process.ExecutablePath)
            name = [string]$process.Name
            creation_date = if ($null -ne $process.CreationDate) { ([datetime]$process.CreationDate).ToUniversalTime().ToString('o') } else { $null }
        }
    }
    return @($items)
}

$samples = @()
$otherPathProcesses = @{}
$trackedProcesses = @{}
$lastFingerprint = $null
$lastRecordedAt = [double]0
$stopDocument = $null
$stopObservedAt = $null
$timedOut = $false
$firstAllRolesZeroAt = $null
$processReappearedAfterFirstAllRolesZero = $false

while ($true) {
    $at = Get-MonotonicMilliseconds
    [array]$inventory = @(Get-ProcessInventory)
    [array]$tree = @(Select-MossExactProcessTrees -Inventory $inventory -Roots $roots)
    $currentIdentityKeys = @{}
    foreach ($selected in $tree) {
        $inventoryMatch = @($inventory | Where-Object { [int]$_.pid -eq [int]$selected.pid }) | Select-Object -First 1
        $creationDate = if ($null -ne $inventoryMatch) { [string]$inventoryMatch.creation_date } else { $null }
        $selected['creation_date'] = $creationDate
        $selected['root_exited_before_descendant'] = $false
        $identityKey = '{0}|{1}|{2}' -f $selected.pid, $creationDate, (Get-MossNormalizedPathForComparison ([string]$selected.executable_path))
        $trackedProcesses[$identityKey] = $selected
        $currentIdentityKeys[$identityKey] = $true
    }
    foreach ($identityKey in @($trackedProcesses.Keys)) {
        if ($currentIdentityKeys.ContainsKey($identityKey)) { continue }
        $tracked = $trackedProcesses[$identityKey]
        $stillAlive = @($inventory | Where-Object {
            [int]$_.pid -eq [int]$tracked.pid -and
            [string]$_.creation_date -eq [string]$tracked.creation_date -and
            (Get-MossNormalizedPathForComparison ([string]$_.executable_path)) -eq
                (Get-MossNormalizedPathForComparison ([string]$tracked.executable_path))
        })
        if ($stillAlive.Count -eq 1) {
            $residual = [ordered]@{}
            foreach ($key in $tracked.Keys) { $residual[$key] = $tracked[$key] }
            $residual['root_exited_before_descendant'] = $true
            $tree += $residual
            $currentIdentityKeys[$identityKey] = $true
        }
    }
    [array]$tree = @($tree | Sort-Object @{ Expression = { [string]$_.role } }, @{ Expression = { [int]$_.root_pid } }, @{ Expression = { [int]$_.depth } }, @{ Expression = { [int]$_.pid } })

    foreach ($root in $roots) {
        $expectedPath = Get-MossNormalizedPathForComparison ([string]$root.executable_path)
        $expectedName = [System.IO.Path]::GetFileName([string]$root.executable_path)
        foreach ($candidate in @($inventory | Where-Object { [string]$_.name -ieq $expectedName })) {
            if ((Get-MossNormalizedPathForComparison ([string]$candidate.executable_path)) -ne $expectedPath) {
                $key = '{0}|{1}|{2}' -f $candidate.pid, $candidate.creation_date, $candidate.executable_path
                $otherPathProcesses[$key] = $candidate
            }
        }
    }

    $fingerprint = (@($tree | ForEach-Object { '{0}:{1}:{2}:{3}' -f $_.role, $_.pid, $_.parent_pid, $_.executable_sha256 }) -join '|')
    $mustRecord = $fingerprint -ne $lastFingerprint -or $samples.Count -eq 0
    if ($null -ne $stopObservedAt -and $at - $lastRecordedAt -ge 250) { $mustRecord = $true }
    if ($mustRecord) {
        $samples += [ordered]@{ monotonic_ms = $at; processes = $tree }
        $lastFingerprint = $fingerprint
        $lastRecordedAt = $at
    }

    if ($null -eq $stopDocument -and (Test-Path -LiteralPath $StopSignal -PathType Leaf)) {
        $stopDocument = Get-Content -LiteralPath $StopSignal -Raw -Encoding UTF8 | ConvertFrom-Json
        $stopObservedAt = $at
        # Record a post-action point even when the process set reached zero earlier.
        $samples += [ordered]@{ monotonic_ms = $at; processes = $tree }
        $lastRecordedAt = $at
    }

    if ($null -ne $stopDocument) {
        $actionCompleted = [double]$stopDocument.action_completed_monotonic_ms
        $expectedActive = @($tree | Where-Object { [string]$_.role -in $expectedRoles })
        if ($at -ge $actionCompleted) {
            if ($expectedActive.Count -eq 0) {
                if ($null -eq $firstAllRolesZeroAt) {
                    $firstAllRolesZeroAt = $at
                } elseif (-not $processReappearedAfterFirstAllRolesZero -and
                    $at - $firstAllRolesZeroAt -ge 1000) {
                    break
                }
            } elseif ($null -ne $firstAllRolesZeroAt) {
                $processReappearedAfterFirstAllRolesZero = $true
            }
        }
        # The first empty scan still has a strict five-second gate. This wider
        # monitor deadline only leaves room for the mandatory one-second
        # confirmation scan and polling jitter; it does not relax that gate.
        if ($at - $actionCompleted -gt 7000) { $timedOut = $true; break }
    }
    Start-Sleep -Milliseconds $PollMilliseconds
}

$roleEvidence = [ordered]@{}
foreach ($role in $expectedRoles) {
    $roleEvidence[$role] = New-MossHelperExitEvidence -Role $role `
        -ActionCompletedMonotonicMs ([double]$stopDocument.action_completed_monotonic_ms) -Samples $samples
}
$otherPathFinal = @()
foreach ($observed in @($otherPathProcesses.Values)) {
    $match = @($inventory | Where-Object {
        [int]$_.pid -eq [int]$observed.pid -and [string]$_.creation_date -eq [string]$observed.creation_date -and
        (Get-MossNormalizedPathForComparison ([string]$_.executable_path)) -eq
            (Get-MossNormalizedPathForComparison ([string]$observed.executable_path))
    })
    if ($match.Count -eq 1) { $otherPathFinal += $match[0] }
}
$report = [ordered]@{
    schema_version = 1
    stage = 'MOSS_FUNCTIONAL_FT_PROCESS_MONITOR'
    clock = 'windows_query_performance_counter'
    action = [string]$stopDocument.action
    action_completed_monotonic_ms = [double]$stopDocument.action_completed_monotonic_ms
    poll_milliseconds = $PollMilliseconds
    timed_out_waiting_for_zero = $timedOut
    first_all_roles_zero_monotonic_ms = $firstAllRolesZeroAt
    process_reappeared_after_first_all_roles_zero = $processReappearedAfterFirstAllRolesZero
    roots = $roots
    expected_roles = $expectedRoles
    role_evidence = $roleEvidence
    samples = $samples
    same_name_other_path_processes = @($otherPathProcesses.Values)
    same_name_other_path_processes_at_end = $otherPathFinal
    same_name_other_path_untouched = if ($otherPathProcesses.Count -gt 0) { $otherPathFinal.Count -eq $otherPathProcesses.Count } else { $null }
    termination_actions_issued = 0
}

$json = ($report | ConvertTo-Json -Depth 30) + [Environment]::NewLine
$bytes = [System.Text.UTF8Encoding]::new($false).GetBytes($json)
$stream = [System.IO.File]::Open($Output, [System.IO.FileMode]::CreateNew, [System.IO.FileAccess]::Write, [System.IO.FileShare]::None)
try { $stream.Write($bytes, 0, $bytes.Length); $stream.Flush($true) } finally { $stream.Dispose() }

param(
    [Parameter(Mandatory = $true)][string]$BaselineInstaller,
    [Parameter(Mandatory = $true)][string]$ArchivedUpgradedData,
    [Parameter(Mandatory = $true)][string]$OutputRoot,
    [Parameter(Mandatory = $true)][string]$ProtectedBaselinePath,
    [string]$ExpectedBaselineSha256 = '',
    [string]$ProductName = 'meetily-p6-lifecycle',
    [string]$BundleId = 'com.meetily.ai.p6lifecycle',
    [int]$ObserveSeconds = 30
)

$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'

$installDirectory = [System.IO.Path]::GetFullPath((Join-Path $env:LOCALAPPDATA $ProductName))
$dataDirectory = [System.IO.Path]::GetFullPath((Join-Path $env:APPDATA $BundleId))
$webViewDirectory = [System.IO.Path]::GetFullPath((Join-Path $env:LOCALAPPDATA $BundleId))
$uninstallRoot = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\*'
$resultPath = Join-Path $OutputRoot 'R05-diagnostic.private.json'
$stdoutPath = Join-Path $OutputRoot 'R05-stdout.log'
$stderrPath = Join-Path $OutputRoot 'R05-stderr.log'
$backtracePath = Join-Path $OutputRoot 'R05-backtrace.log'
$reproductionPath = Join-Path $OutputRoot 'R05-reproduction.log'
$eventsPath = Join-Path $OutputRoot 'R05-windows-events.json'
$preservedDataPath = Join-Path $OutputRoot 'preserved-live-appdata'
$preservedWebViewPath = Join-Path $OutputRoot 'preserved-live-webview'

function Write-Utf8File {
    param([Parameter(Mandatory = $true)][string]$Path, [Parameter(Mandatory = $true)][string]$Content)
    $parent = Split-Path -Parent $Path
    if (-not (Test-Path -LiteralPath $parent -PathType Container)) {
        New-Item -ItemType Directory -Path $parent -Force | Out-Null
    }
    [System.IO.File]::WriteAllText($Path, $Content, [System.Text.UTF8Encoding]::new($false))
}

function Write-JsonFile {
    param([Parameter(Mandatory = $true)][string]$Path, [Parameter(Mandatory = $true)]$Value)
    Write-Utf8File -Path $Path -Content (($Value | ConvertTo-Json -Depth 30) + "`n")
}

function Get-FileEvidence {
    param([Parameter(Mandatory = $true)][string]$Path)
    $exists = Test-Path -LiteralPath $Path -PathType Leaf
    return [ordered]@{
        path = $Path
        exists = $exists
        bytes = if ($exists) { [int64](Get-Item -LiteralPath $Path).Length } else { $null }
        sha256 = if ($exists) { (Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash.ToUpperInvariant() } else { $null }
    }
}

function Get-DirectoryManifest {
    param([Parameter(Mandatory = $true)][string]$Root)
    if (-not (Test-Path -LiteralPath $Root -PathType Container)) { return @() }
    return @(
        Get-ChildItem -LiteralPath $Root -Recurse -File -Force |
            Sort-Object FullName |
            ForEach-Object {
                [ordered]@{
                    relative_path = $_.FullName.Substring($Root.Length).TrimStart('\')
                    bytes = [int64]$_.Length
                    sha256 = (Get-FileHash -LiteralPath $_.FullName -Algorithm SHA256).Hash.ToUpperInvariant()
                }
            }
    )
}

function Get-ProtectedSnapshot {
    param([Parameter(Mandatory = $true)]$Baseline)
    return @(
        foreach ($item in $Baseline) {
            $exists = Test-Path -LiteralPath $item.path -PathType Leaf
            $bytes = if ($exists) { [int64](Get-Item -LiteralPath $item.path).Length } else { $null }
            $sha256 = if ($exists) { (Get-FileHash -LiteralPath $item.path -Algorithm SHA256).Hash.ToUpperInvariant() } else { $null }
            [ordered]@{
                role = [string]$item.role
                path = [string]$item.path
                exists = $exists
                bytes = $bytes
                sha256 = $sha256
                matches_baseline = ($exists -eq [bool]$item.exists) -and ((-not $exists) -or ($bytes -eq [int64]$item.bytes -and $sha256 -eq [string]$item.sha256))
            }
        }
    )
}

function Get-TestRegistry {
    $record = Get-ItemProperty -Path $uninstallRoot -ErrorAction SilentlyContinue |
        Where-Object { $_.DisplayName -eq $ProductName } |
        Select-Object -First 1
    if ($null -eq $record) { return $null }
    return [ordered]@{
        key = [string]$record.PSChildName
        display_name = [string]$record.DisplayName
        display_version = [string]$record.DisplayVersion
        install_location = [string]$record.InstallLocation
        uninstall_string = [string]$record.UninstallString
        display_icon = [string]$record.DisplayIcon
    }
}

function Get-ExactProcesses {
    return @(
        Get-CimInstance Win32_Process -ErrorAction SilentlyContinue |
            Where-Object {
                $_.ExecutablePath -and
                [System.IO.Path]::GetFullPath([string]$_.ExecutablePath).StartsWith(
                    $installDirectory + [System.IO.Path]::DirectorySeparatorChar,
                    [System.StringComparison]::OrdinalIgnoreCase
                )
            } |
            Select-Object ProcessId, ParentProcessId, Name, ExecutablePath, CommandLine
    )
}

function Assert-IsolatedStateAbsent {
    $state = [ordered]@{
        install_directory_exists = Test-Path -LiteralPath $installDirectory
        data_directory_exists = Test-Path -LiteralPath $dataDirectory
        webview_directory_exists = Test-Path -LiteralPath $webViewDirectory
        registry_exists = $null -ne (Get-TestRegistry)
        process_count = @(Get-ExactProcesses).Count
    }
    if ($state.install_directory_exists -or $state.data_directory_exists -or $state.webview_directory_exists -or $state.registry_exists -or $state.process_count -ne 0) {
        throw "Isolated product identity is not clean: $($state | ConvertTo-Json -Compress)"
    }
    return $state
}

function Invoke-SilentExecutable {
    param(
        [Parameter(Mandatory = $true)][string]$FilePath,
        [Parameter(Mandatory = $true)][string]$Label,
        [string[]]$Arguments = @('/S'),
        [int]$TimeoutSeconds = 900
    )
    $started = Get-Date
    $process = Start-Process -FilePath $FilePath -ArgumentList $Arguments -WindowStyle Hidden -PassThru
    if (-not $process.WaitForExit($TimeoutSeconds * 1000)) {
        Stop-Process -Id $process.Id -Force -ErrorAction SilentlyContinue
        throw "$Label timed out after $TimeoutSeconds seconds"
    }
    $process.Refresh()
    return [ordered]@{
        label = $Label
        path = $FilePath
        process_id = [int]$process.Id
        exit_code = [int]$process.ExitCode
        started_at = $started.ToString('o')
        completed_at = (Get-Date).ToString('o')
        elapsed_seconds = [Math]::Round(((Get-Date) - $started).TotalSeconds, 3)
    }
}

function Move-IsolatedDirectoryToEvidence {
    param([Parameter(Mandatory = $true)][string]$Source, [Parameter(Mandatory = $true)][string]$Destination)
    if (-not (Test-Path -LiteralPath $Source -PathType Container)) { return $null }
    $resolvedSource = [System.IO.Path]::GetFullPath($Source)
    $resolvedDestination = [System.IO.Path]::GetFullPath($Destination)
    if ($resolvedSource -notin @($dataDirectory, $webViewDirectory)) {
        throw "Refusing to move unexpected directory: $resolvedSource"
    }
    if (-not $resolvedDestination.StartsWith($OutputRoot + [System.IO.Path]::DirectorySeparatorChar, [System.StringComparison]::OrdinalIgnoreCase)) {
        throw "Refusing unsafe evidence destination: $resolvedDestination"
    }
    if (Test-Path -LiteralPath $resolvedDestination) {
        throw "Evidence destination already exists: $resolvedDestination"
    }
    Move-Item -LiteralPath $resolvedSource -Destination $resolvedDestination
    return $resolvedDestination
}

foreach ($required in @($BaselineInstaller, $ProtectedBaselinePath)) {
    if (-not (Test-Path -LiteralPath $required -PathType Leaf)) { throw "Required file is missing: $required" }
}
if (-not (Test-Path -LiteralPath $ArchivedUpgradedData -PathType Container)) {
    throw "Archived upgraded data is missing: $ArchivedUpgradedData"
}
if ($ObserveSeconds -lt 5 -or $ObserveSeconds -gt 120) { throw 'ObserveSeconds must be between 5 and 120.' }

$OutputRoot = [System.IO.Path]::GetFullPath($OutputRoot)
if (Test-Path -LiteralPath $OutputRoot) {
    if (@(Get-ChildItem -LiteralPath $OutputRoot -Force).Count -gt 0) { throw "OutputRoot must be new or empty: $OutputRoot" }
} else {
    New-Item -ItemType Directory -Path $OutputRoot -Force | Out-Null
}

$protectedBaseline = Get-Content -LiteralPath $ProtectedBaselinePath -Raw -Encoding UTF8 | ConvertFrom-Json
$baselineEvidence = Get-FileEvidence $BaselineInstaller
if (-not [string]::IsNullOrWhiteSpace($ExpectedBaselineSha256) -and $baselineEvidence.sha256 -ne $ExpectedBaselineSha256.ToUpperInvariant()) {
    throw "Baseline installer SHA-256 mismatch: $($baselineEvidence.sha256)"
}
$protectedBefore = Get-ProtectedSnapshot $protectedBaseline
if (@($protectedBefore | Where-Object { -not $_.matches_baseline }).Count -ne 0) {
    throw 'Protected user data does not match the frozen baseline before the diagnostic run.'
}

$result = [ordered]@{
    schema_version = 1
    purpose = 'Diagnose isolated 0.4.1 startup exit code 101 after a 0.4.2-upgraded data set'
    started_at = (Get-Date).ToString('o')
    product_name = $ProductName
    bundle_id = $BundleId
    install_directory = $installDirectory
    data_directory = $dataDirectory
    webview_directory = $webViewDirectory
    baseline_installer = $baselineEvidence
    archived_upgraded_data = $ArchivedUpgradedData
    archived_upgraded_data_manifest_before = Get-DirectoryManifest $ArchivedUpgradedData
    protected_before = $protectedBefore
    clean_state_before = $null
    install = $null
    launch = $null
    cleanup = [ordered]@{}
    status = 'RUNNING'
}

$previousBacktrace = $env:RUST_BACKTRACE
$previousLog = $env:RUST_LOG
$eventStart = Get-Date
$appProcess = $null

try {
    $result.clean_state_before = Assert-IsolatedStateAbsent
    $result.install = Invoke-SilentExecutable -FilePath $BaselineInstaller -Label 'install-baseline-0.4.1'
    if ($result.install.exit_code -ne 0) { throw "Baseline installer failed with exit code $($result.install.exit_code)" }

    Copy-Item -LiteralPath $ArchivedUpgradedData -Destination $dataDirectory -Recurse
    $copiedManifestBeforeLaunch = Get-DirectoryManifest $dataDirectory
    $mainExecutable = Join-Path $installDirectory ($ProductName + '.exe')
    if (-not (Test-Path -LiteralPath $mainExecutable -PathType Leaf)) {
        $mainExecutable = Join-Path $installDirectory 'meetily.exe'
    }
    if (-not (Test-Path -LiteralPath $mainExecutable -PathType Leaf)) { throw 'Installed main executable is missing.' }

    $env:RUST_BACKTRACE = 'full'
    $env:RUST_LOG = 'trace'
    $launchStarted = Get-Date
    $appProcess = Start-Process -FilePath $mainExecutable `
        -WorkingDirectory $installDirectory `
        -WindowStyle Hidden `
        -RedirectStandardOutput $stdoutPath `
        -RedirectStandardError $stderrPath `
        -PassThru

    $commandSnapshot = $null
    $moduleSnapshot = @()
    $processSnapshots = @()
    $deadline = $launchStarted.AddSeconds($ObserveSeconds)
    do {
        Start-Sleep -Milliseconds 200
        $appProcess.Refresh()
        if ($null -eq $commandSnapshot) {
            $commandSnapshot = Get-CimInstance Win32_Process -Filter "ProcessId=$($appProcess.Id)" -ErrorAction SilentlyContinue |
                Select-Object ProcessId, ParentProcessId, ExecutablePath, CommandLine
        }
        if ($moduleSnapshot.Count -eq 0 -and -not $appProcess.HasExited) {
            try {
                $moduleSnapshot = @(
                    (Get-Process -Id $appProcess.Id -ErrorAction Stop).Modules |
                        Select-Object ModuleName, FileName, FileVersionInfo
                )
            } catch {
                $moduleSnapshot = @([ordered]@{ capture_error = $_.Exception.Message })
            }
        }
        if ($processSnapshots.Count -lt 10) {
            $processSnapshots += [ordered]@{
                captured_at = (Get-Date).ToString('o')
                processes = @(Get-ExactProcesses)
            }
        }
    } while (-not $appProcess.HasExited -and (Get-Date) -lt $deadline)

    $appProcess.Refresh()
    $aliveAfterObservation = -not $appProcess.HasExited
    $exitCode = if ($appProcess.HasExited) { [int]$appProcess.ExitCode } else { $null }
    if ($aliveAfterObservation) {
        Stop-Process -Id $appProcess.Id -Force -ErrorAction SilentlyContinue
        Wait-Process -Id $appProcess.Id -Timeout 15 -ErrorAction SilentlyContinue
    }
    Start-Sleep -Milliseconds 500
    $stderrText = if (Test-Path -LiteralPath $stderrPath) { Get-Content -LiteralPath $stderrPath -Raw -Encoding UTF8 } else { '' }
    $stdoutText = if (Test-Path -LiteralPath $stdoutPath) { Get-Content -LiteralPath $stdoutPath -Raw -Encoding UTF8 } else { '' }
    Write-Utf8File -Path $backtracePath -Content $stderrText

    $result.launch = [ordered]@{
        executable = Get-FileEvidence $mainExecutable
        process_id = [int]$appProcess.Id
        working_directory = $installDirectory
        command = $commandSnapshot
        environment_whitelist = [ordered]@{
            RUST_BACKTRACE = 'full'
            RUST_LOG = 'trace'
            APPDATA = $env:APPDATA
            LOCALAPPDATA = $env:LOCALAPPDATA
            PATH_entry_count = @($env:PATH -split ';' | Where-Object { $_ }).Count
        }
        started_at = $launchStarted.ToString('o')
        observed_until = (Get-Date).ToString('o')
        alive_after_observation = $aliveAfterObservation
        exit_code = $exitCode
        process_snapshots = $processSnapshots
        loaded_modules = $moduleSnapshot
        copied_data_manifest_before_launch = $copiedManifestBeforeLaunch
        data_manifest_after_launch = Get-DirectoryManifest $dataDirectory
        stdout = Get-FileEvidence $stdoutPath
        stderr = Get-FileEvidence $stderrPath
        backtrace = Get-FileEvidence $backtracePath
        stage_evidence = [ordered]@{
            database_file_present = Test-Path -LiteralPath (Join-Path $dataDirectory 'meeting_minutes.sqlite') -PathType Leaf
            database_path_logged = $stderrText -match 'Tauri DB path:'
            database_opened = $stderrText -match 'Database opened successfully'
            database_initialized = $stderrText -match 'Database initialized successfully'
            migration_error = $stderrText -match '(?i)migration .*missing|VersionMissing'
            panic = $stderrText -match '(?i)panicked at|panic'
            window_or_tray_started = $stderrText -match '(?i)window|tray'
            webview_process_seen = @($processSnapshots.processes | Where-Object { $_.Name -match '(?i)msedgewebview2' }).Count -gt 0
            stderr_last_lines = @($stderrText -split "`r?`n" | Where-Object { $_ } | Select-Object -Last 20)
            stdout_last_lines = @($stdoutText -split "`r?`n" | Where-Object { $_ } | Select-Object -Last 20)
        }
    }

    $eventEnd = (Get-Date).AddSeconds(2)
    Start-Sleep -Seconds 2
    $events = @(
        Get-WinEvent -FilterHashtable @{ LogName = 'Application'; StartTime = $eventStart; EndTime = $eventEnd } -ErrorAction SilentlyContinue |
            Where-Object {
                $_.Level -in @(1, 2, 3) -or
                $_.ProviderName -in @('Application Error', 'Windows Error Reporting', '.NET Runtime', 'SideBySide') -or
                $_.Message -match [regex]::Escape($ProductName)
            } |
            Select-Object TimeCreated, Id, LevelDisplayName, ProviderName, Message
    )
    Write-JsonFile -Path $eventsPath -Value $events

    $reproductionLines = @(
        "Started: $($launchStarted.ToString('o'))"
        "Executable: $mainExecutable"
        "Working directory: $installDirectory"
        'Environment: RUST_BACKTRACE=full; RUST_LOG=trace'
        "Observed seconds: $ObserveSeconds"
        "Alive after observation: $aliveAfterObservation"
        "Exit code: $exitCode"
        "Database path logged: $($result.launch.stage_evidence.database_path_logged)"
        "Database opened: $($result.launch.stage_evidence.database_opened)"
        "Database initialized: $($result.launch.stage_evidence.database_initialized)"
        "Migration error: $($result.launch.stage_evidence.migration_error)"
        "Panic: $($result.launch.stage_evidence.panic)"
        'Last stderr lines:'
        ($result.launch.stage_evidence.stderr_last_lines -join "`n")
    )
    Write-Utf8File -Path $reproductionPath -Content (($reproductionLines -join "`n") + "`n")
    $result.status = 'CAPTURED'
} catch {
    $result.status = 'FAILED_TO_CAPTURE'
    $result.error = $_.Exception.Message
    $result.failed_at = (Get-Date).ToString('o')
} finally {
    $env:RUST_BACKTRACE = $previousBacktrace
    $env:RUST_LOG = $previousLog
    foreach ($process in @(Get-ExactProcesses)) {
        Stop-Process -Id $process.ProcessId -Force -ErrorAction SilentlyContinue
    }
    Start-Sleep -Milliseconds 500
    try {
        $uninstaller = Join-Path $installDirectory 'uninstall.exe'
        if (Test-Path -LiteralPath $uninstaller -PathType Leaf) {
            $result.cleanup.uninstall = Invoke-SilentExecutable -FilePath $uninstaller -Label 'cleanup-uninstall-baseline-0.4.1' -TimeoutSeconds 300
            $uninstallDeadline = (Get-Date).AddSeconds(90)
            do {
                $installStillExists = Test-Path -LiteralPath $installDirectory
                $registryStillExists = $null -ne (Get-TestRegistry)
                if (-not $installStillExists -and -not $registryStillExists) { break }
                Start-Sleep -Milliseconds 500
            } while ((Get-Date) -lt $uninstallDeadline)
        }
    } catch {
        $result.cleanup.uninstall_error = $_.Exception.Message
    }
    try { $result.cleanup.preserved_data = Move-IsolatedDirectoryToEvidence -Source $dataDirectory -Destination $preservedDataPath } catch { $result.cleanup.data_move_error = $_.Exception.Message }
    try { $result.cleanup.preserved_webview = Move-IsolatedDirectoryToEvidence -Source $webViewDirectory -Destination $preservedWebViewPath } catch { $result.cleanup.webview_move_error = $_.Exception.Message }
    $result.cleanup.registry_absent = $null -eq (Get-TestRegistry)
    $result.cleanup.install_directory_absent = -not (Test-Path -LiteralPath $installDirectory)
    $result.cleanup.data_directory_absent = -not (Test-Path -LiteralPath $dataDirectory)
    $result.cleanup.webview_directory_absent = -not (Test-Path -LiteralPath $webViewDirectory)
    $result.cleanup.process_count = @(Get-ExactProcesses).Count
    $result.cleanup.protected_after = Get-ProtectedSnapshot $protectedBaseline
    $result.cleanup.protected_unchanged = @($result.cleanup.protected_after | Where-Object { -not $_.matches_baseline }).Count -eq 0
    $result.completed_at = (Get-Date).ToString('o')
    Write-JsonFile -Path $resultPath -Value $result
}

if ($result.status -ne 'CAPTURED') { exit 1 }
if (-not $result.cleanup.registry_absent -or -not $result.cleanup.install_directory_absent -or -not $result.cleanup.data_directory_absent -or -not $result.cleanup.webview_directory_absent -or $result.cleanup.process_count -ne 0 -or -not $result.cleanup.protected_unchanged) { exit 2 }
exit 0

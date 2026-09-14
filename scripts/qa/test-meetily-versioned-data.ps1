[CmdletBinding()]
param(
    [string]$ToolPath = '',
    [string]$NsisHookPath = '',
    [string]$NsisTemplatePath = '',
    [string[]]$TauriConfigPaths = @(),
    [string]$FixtureRoot = '',
    [Parameter(Mandatory = $true)][string]$OutputRoot,
    [switch]$RemoveFixtureRootAfterRun
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'

$scriptRoot = Split-Path -Parent $MyInvocation.MyCommand.Path
if ([string]::IsNullOrWhiteSpace($ToolPath)) { $ToolPath = Join-Path $scriptRoot '..\..\frontend\src-tauri\scripts\meetily-versioned-data.ps1' }
if ([string]::IsNullOrWhiteSpace($NsisHookPath)) { $NsisHookPath = Join-Path $scriptRoot '..\..\frontend\src-tauri\scripts\nsis-installer-hooks.nsh' }
if ([string]::IsNullOrWhiteSpace($NsisTemplatePath)) { $NsisTemplatePath = Join-Path $scriptRoot '..\..\frontend\src-tauri\scripts\nsis-installer-template.nsi' }
if ($TauriConfigPaths.Count -eq 0) {
    $TauriConfigPaths = @(
        (Join-Path $scriptRoot '..\..\frontend\src-tauri\tauri.conf.json'),
        (Join-Path $scriptRoot '..\..\frontend\src-tauri\tauri.phase5.conf.json')
    )
}
$ToolPath = [System.IO.Path]::GetFullPath($ToolPath)
$NsisHookPath = [System.IO.Path]::GetFullPath($NsisHookPath)
$NsisTemplatePath = [System.IO.Path]::GetFullPath($NsisTemplatePath)
$TauriConfigPaths = @($TauriConfigPaths | ForEach-Object { [System.IO.Path]::GetFullPath($_) })
$OutputRoot = [System.IO.Path]::GetFullPath($OutputRoot)
if ([string]::IsNullOrWhiteSpace($FixtureRoot)) {
    $FixtureRoot = Join-Path ([System.IO.Path]::GetTempPath()) ('meetily-r05-' + [guid]::NewGuid().ToString('N').Substring(0, 12))
}
$FixtureRoot = [System.IO.Path]::GetFullPath($FixtureRoot)
foreach ($required in @($ToolPath, $NsisHookPath, $NsisTemplatePath) + $TauriConfigPaths) {
    if (-not (Test-Path -LiteralPath $required -PathType Leaf)) { throw "Required source file is missing: $required" }
}
if (Test-Path -LiteralPath $OutputRoot) {
    if (@(Get-ChildItem -LiteralPath $OutputRoot -Force).Count -gt 0) { throw "OutputRoot must be new or empty: $OutputRoot" }
} else {
    New-Item -ItemType Directory -Path $OutputRoot -Force | Out-Null
}
if (Test-Path -LiteralPath $FixtureRoot) {
    if (@(Get-ChildItem -LiteralPath $FixtureRoot -Force).Count -gt 0) { throw "FixtureRoot must be new or empty: $FixtureRoot" }
} else {
    New-Item -ItemType Directory -Path $FixtureRoot -Force | Out-Null
}

. $ToolPath

function Write-TestFile {
    param([Parameter(Mandatory = $true)][string]$Path, [Parameter(Mandatory = $true)][AllowEmptyString()][string]$Content)
    Write-MeetilyUtf8File -Path $Path -Content $Content
}

function New-TestJunction {
    param(
        [Parameter(Mandatory = $true)][string]$Path,
        [Parameter(Mandatory = $true)][string]$Target
    )
    $parent = Split-Path -Parent $Path
    if (-not (Test-Path -LiteralPath $parent -PathType Container)) {
        New-Item -ItemType Directory -Path $parent -Force | Out-Null
    }
    New-Item -ItemType Junction -Path $Path -Target $Target | Out-Null
    $item = Get-Item -LiteralPath $Path -Force
    if (($item.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -eq 0) {
        throw "Test fixture is not a real reparse point: $Path"
    }
}

function New-CaseLayout {
    param([Parameter(Mandatory = $true)][string]$Name)
    $root = Join-Path $FixtureRoot $Name
    if (Test-Path -LiteralPath $root) { throw "Test case directory already exists: $root" }
    New-Item -ItemType Directory -Path $root -Force | Out-Null
    return [ordered]@{
        root = $root
        data = Join-Path $root 'com.meetily.test'
        backups = Join-Path $root 'com.meetily.test.rollback-backups'
        install = Join-Path $root 'meetily-install'
    }
}

function Set-OldDataFixture {
    param([Parameter(Mandatory = $true)][string]$DataRoot, [string]$Marker = 'old')
    $recordingRoot = [System.IO.Path]::GetFullPath((Join-Path (Split-Path -Parent $DataRoot) ('recordings-' + $Marker)))
    foreach ($entry in @(
        @{ relative = 'meeting_minutes.sqlite'; content = "$Marker-database-14-migrations" },
        @{ relative = 'settings.json'; content = "{`"version`":`"$Marker`",`"autoSave`":true}" },
        @{ relative = 'recording_preferences.json'; content = (([ordered]@{ preferences = [ordered]@{
            save_folder = $recordingRoot
            auto_save = $true
            file_format = 'mp4'
            preferred_mic_device = $null
            preferred_system_device = $null
        } } | ConvertTo-Json -Depth 5) + "`n") },
        @{ relative = 'templates\custom.json'; content = "{`"id`":`"template-$Marker`"}" },
        @{ relative = 'models\model.marker'; content = "$Marker-model" }
    )) {
        Write-TestFile -Path (Join-Path $DataRoot $entry.relative) -Content $entry.content
    }
}

function Set-CurrentDataFixture {
    param([Parameter(Mandatory = $true)][string]$DataRoot, [string]$Marker = 'current')
    $recordingRoot = [System.IO.Path]::GetFullPath((Join-Path (Split-Path -Parent $DataRoot) ('recordings-' + $Marker)))
    foreach ($entry in @(
        @{ relative = 'meeting_minutes.sqlite'; content = "$Marker-database-16-migrations" },
        @{ relative = 'settings.json'; content = "{`"version`":`"$Marker`",`"autoSave`":false}" },
        @{ relative = 'recording_preferences.json'; content = (([ordered]@{ preferences = [ordered]@{
            save_folder = $recordingRoot
            auto_save = $false
            file_format = 'mp4'
            preferred_mic_device = $null
            preferred_system_device = $null
        } } | ConvertTo-Json -Depth 5) + "`n") },
        @{ relative = 'templates\custom.json'; content = "{`"id`":`"template-$Marker`"}" },
        @{ relative = 'models\model.marker'; content = "$Marker-model" },
        @{ relative = 'new-version-only.json'; content = "{`"migration`":16}" }
    )) {
        Write-TestFile -Path (Join-Path $DataRoot $entry.relative) -Content $entry.content
    }
}

function Get-DataFingerprint {
    param([Parameter(Mandatory = $true)][string]$DataRoot)
    return Get-MeetilyManifestFingerprint (Get-MeetilyDirectoryManifest $DataRoot)
}

function Invoke-ExpectedFailure {
    param([Parameter(Mandatory = $true)][scriptblock]$Action)
    try {
        & $Action | Out-Null
        return [ordered]@{ failed = $false; message = 'Operation unexpectedly succeeded.' }
    } catch {
        return [ordered]@{ failed = $true; message = $_.Exception.Message }
    }
}

function Get-RestoreScratchPaths {
    param([Parameter(Mandatory = $true)][string]$DataRoot)
    $parent = Split-Path -Parent $DataRoot
    $leaf = Split-Path -Leaf $DataRoot
    return @(
        Get-ChildItem -LiteralPath $parent -Force -ErrorAction SilentlyContinue |
            Where-Object {
                $_.Name -like ".$leaf.restore-staging-*" -or
                $_.Name -like ".$leaf.restore-original-*" -or
                $_.Name -like '.rs-*' -or
                $_.Name -like '.ro-*'
            } |
            Select-Object -ExpandProperty FullName
    )
}

function Get-DifferentFilesystemRoot {
    param([Parameter(Mandatory = $true)][string]$Path)
    $sourceVolumeRoot = [System.IO.Path]::GetPathRoot([System.IO.Path]::GetFullPath($Path))
    foreach ($drive in @(Get-PSDrive -PSProvider FileSystem | Sort-Object Name)) {
        if ([string]::IsNullOrWhiteSpace([string]$drive.Root)) { continue }
        try {
            $candidateVolumeRoot = [System.IO.Path]::GetPathRoot([System.IO.Path]::GetFullPath([string]$drive.Root))
            if (-not [string]::IsNullOrWhiteSpace($candidateVolumeRoot) -and
                -not $candidateVolumeRoot.Equals($sourceVolumeRoot, [System.StringComparison]::OrdinalIgnoreCase) -and
                (Test-Path -LiteralPath $candidateVolumeRoot -PathType Container)) {
                return $candidateVolumeRoot
            }
        } catch { }
    }
    throw "The cross-volume test requires a second accessible filesystem volume; source volume is $sourceVolumeRoot."
}

$started = Get-Date
$results = @()
$faultDetails = @()
$canonicalManifestPath = $null

function Add-TestResult {
    param(
        [Parameter(Mandatory = $true)][string]$Name,
        [Parameter(Mandatory = $true)][bool]$Passed,
        [Parameter(Mandatory = $true)]$Details
    )
    $script:results += [ordered]@{
        name = $Name
        verdict = if ($Passed) { 'PASS' } else { 'FAIL' }
        details = $Details
    }
}

try {
    $case = New-CaseLayout 'direct-downgrade-rejected'
    Set-OldDataFixture $case.data
    New-Item -ItemType Directory -Path $case.install -Force | Out-Null
    Write-TestFile -Path (Join-Path $case.install 'meetily.exe') -Content 'current-binary'
    $dataBefore = Get-DataFingerprint $case.data
    $installBefore = Get-DataFingerprint $case.install
    $hook = Get-Content -LiteralPath $NsisHookPath -Raw -Encoding UTF8
    $template = Get-Content -LiteralPath $NsisTemplatePath -Raw -Encoding UTF8
    $configs = @($TauriConfigPaths | ForEach-Object {
        $config = Get-Content -LiteralPath $_ -Raw -Encoding UTF8 | ConvertFrom-Json
        [ordered]@{
            path = $_
            sha256 = (Get-FileHash -LiteralPath $_ -Algorithm SHA256).Hash.ToUpperInvariant()
            configured_template = [string]$config.bundle.windows.nsis.template
            valid = [string]$config.bundle.windows.nsis.template -eq 'scripts/nsis-installer-template.nsi'
        }
    })
    $hookContract = $hook -match 'SemverCompare\s+"\$\{VERSION\}"\s+\$R8' -and
        $hook -match '\$R9\s*=\s*-1' -and
        $hook -match 'SetErrorLevel\s+3' -and
        $hook -match '(?m)^\s*Quit\s*$' -and
        $hook -match 'MEETILY_INSTALLER_SOURCE_VERSION' -and
        $hook -match 'MEETILY_INSTALLER_TARGET_VERSION' -and
        $hook -match 'MEETILY_INSTALLER_DATA_ROOT' -and
        $hook -match 'nsExec::ExecToLog\s+/TIMEOUT=1200000' -and
        $hook -match 'meetily-versioned-data\.ps1"\s+-Mode Backup' -and
        $hook.IndexOf('meetily-versioned-data.ps1', [System.StringComparison]::Ordinal) -lt $hook.IndexOf('DirectML.dll', [System.StringComparison]::Ordinal)
    $templateContract = $template -match 'Tauri CLI 2\.11\.1' -and
        $template -match '(?m)^Var MeetilyInstalledVersion\s*$' -and
        $template -match 'ReadRegStr \$MeetilyInstalledVersion SHCTX "\$\{UNINSTKEY\}" "DisplayVersion"' -and
        $template -match '(?s)Function PageReinstall.*?SemverCompare "\$\{VERSION\}" \$R0.*?\$R0 = -1.*?SetErrorLevel 3.*?Quit' -and
        $template -match '(?s)Section EarlyChecks.*?\$MeetilyInstalledVersion != "".*?SemverCompare "\$\{VERSION\}" \$MeetilyInstalledVersion.*?\$R0 = -1.*?SetErrorLevel 3.*?Quit.*?SectionEnd' -and
        $template -match '(?s)\$R0 = 1.*?\$WixMode != 1.*?Abort' -and
        $template -match '(?s)Function \.onInit.*?ReadRegStr \$MeetilyInstalledVersion'
    $configsValid = @($configs | Where-Object { -not $_.valid }).Count -eq 0
    $contract = $hookContract -and $templateContract -and $configsValid
    $dataAfter = Get-DataFingerprint $case.data
    $installAfter = Get-DataFingerprint $case.install
    Add-TestResult -Name 'direct_downgrade_is_rejected_without_mutation' -Passed ($contract -and $dataBefore -eq $dataAfter -and $installBefore -eq $installAfter) -Details ([ordered]@{
        scope = 'source contract; real installer transition is verified by the lifecycle integration test'
        hook_sha256 = (Get-FileHash -LiteralPath $NsisHookPath -Algorithm SHA256).Hash.ToUpperInvariant()
        template_sha256 = (Get-FileHash -LiteralPath $NsisTemplatePath -Algorithm SHA256).Hash.ToUpperInvariant()
        hook_contract_valid = $hookContract
        template_contract_valid = $templateContract
        configs = $configs
        data_fingerprint_before = $dataBefore
        data_fingerprint_after = $dataAfter
        install_fingerprint_before = $installBefore
        install_fingerprint_after = $installAfter
    })
} catch {
    Add-TestResult -Name 'direct_downgrade_is_rejected_without_mutation' -Passed $false -Details ([ordered]@{ error = $_.Exception.Message })
}

try {
    $case = New-CaseLayout 'installer-environment-cli'
    Set-OldDataFixture $case.data -Marker 'installer-cli'
    $resultPath = Join-Path $case.backups 'last-upgrade-backup-result.json'
    $before = Get-DataFingerprint $case.data
    $names = @('MEETILY_INSTALLER_DATA_ROOT', 'MEETILY_INSTALLER_SOURCE_VERSION', 'MEETILY_INSTALLER_TARGET_VERSION')
    $savedEnvironment = @{}
    foreach ($name in $names) { $savedEnvironment[$name] = [Environment]::GetEnvironmentVariable($name, 'Process') }
    try {
        $env:MEETILY_INSTALLER_DATA_ROOT = $case.data
        $env:MEETILY_INSTALLER_SOURCE_VERSION = '0.4.1'
        $env:MEETILY_INSTALLER_TARGET_VERSION = '0.4.2'
        $windowsPowerShell = Join-Path $env:SystemRoot 'System32\WindowsPowerShell\v1.0\powershell.exe'
        $cliOutput = (& $windowsPowerShell -NoLogo -NoProfile -NonInteractive -ExecutionPolicy Bypass -File $ToolPath -Mode Backup 2>&1 | Out-String).Trim()
        $cliExitCode = [int]$LASTEXITCODE
    } finally {
        foreach ($name in $names) {
            [Environment]::SetEnvironmentVariable($name, $savedEnvironment[$name], 'Process')
        }
    }
    $after = Get-DataFingerprint $case.data
    $record = if (Test-Path -LiteralPath $resultPath -PathType Leaf) {
        Get-Content -LiteralPath $resultPath -Raw -Encoding UTF8 | ConvertFrom-Json
    } else { $null }
    $backupDirectory = if ($null -ne $record) { [string]$record.result.backup_directory } else { '' }
    $manifestPath = if ([string]::IsNullOrWhiteSpace($backupDirectory)) { '' } else { Join-Path $backupDirectory 'backup-manifest.json' }
    $trustedTool = Join-Path $case.backups 'tools\meetily-versioned-data.ps1'
    $trustedToolHashFile = $trustedTool + '.sha256'
    $passed = $cliExitCode -eq 0 -and $null -ne $record -and $record.status -eq 'PASS' -and
        $record.result.source_version -eq '0.4.1' -and $record.result.target_version -eq '0.4.2' -and
        (Test-Path -LiteralPath $manifestPath -PathType Leaf) -and
        (Test-Path -LiteralPath $trustedTool -PathType Leaf) -and
        (Test-Path -LiteralPath $trustedToolHashFile -PathType Leaf) -and
        $before -eq $after
    Add-TestResult -Name 'installer_backup_cli_uses_environment_and_writes_result' -Passed $passed -Details ([ordered]@{
        windows_powershell = $windowsPowerShell
        exit_code = $cliExitCode
        output = $cliOutput
        result_path = $resultPath
        result_status = if ($null -ne $record) { [string]$record.status } else { $null }
        backup_directory = $backupDirectory
        manifest_exists = -not [string]::IsNullOrWhiteSpace($manifestPath) -and (Test-Path -LiteralPath $manifestPath -PathType Leaf)
        trusted_tool_exists = Test-Path -LiteralPath $trustedTool -PathType Leaf
        trusted_tool_hash_matches_source = (Test-Path -LiteralPath $trustedTool -PathType Leaf) -and
            (Get-FileHash -LiteralPath $trustedTool -Algorithm SHA256).Hash -eq (Get-FileHash -LiteralPath $ToolPath -Algorithm SHA256).Hash
        source_data_unchanged = $before -eq $after
    })
} catch {
    Add-TestResult -Name 'installer_backup_cli_uses_environment_and_writes_result' -Passed $false -Details ([ordered]@{ error = $_.Exception.Message })
}

function Remove-TestReparsePoints {
    param([Parameter(Mandatory = $true)][string]$Root)
    $removed = [System.Collections.Generic.List[string]]::new()
    $pending = [System.Collections.Generic.Queue[string]]::new()
    $pending.Enqueue([System.IO.Path]::GetFullPath($Root))
    while ($pending.Count -gt 0) {
        $directory = $pending.Dequeue()
        foreach ($ioEntry in [System.IO.Directory]::EnumerateFileSystemEntries((ConvertTo-MeetilyIoPath $directory))) {
            $entry = ConvertFrom-MeetilyIoPath $ioEntry
            $state = Get-MeetilyPathStateUnchecked $entry
            if (($state.attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0) {
                if (($state.attributes -band [System.IO.FileAttributes]::Directory) -ne 0) {
                    [System.IO.Directory]::Delete((ConvertTo-MeetilyIoPath $entry), $false)
                } else {
                    [System.IO.File]::Delete((ConvertTo-MeetilyIoPath $entry))
                }
                $removed.Add($entry)
            } elseif (($state.attributes -band [System.IO.FileAttributes]::Directory) -ne 0) {
                $pending.Enqueue($entry)
            }
        }
    }
    return @($removed)
}

function Get-TestReparsePoints {
    param([Parameter(Mandatory = $true)][string]$Root)
    $found = [System.Collections.Generic.List[string]]::new()
    $pending = [System.Collections.Generic.Queue[string]]::new()
    $pending.Enqueue([System.IO.Path]::GetFullPath($Root))
    while ($pending.Count -gt 0) {
        $directory = $pending.Dequeue()
        foreach ($ioEntry in [System.IO.Directory]::EnumerateFileSystemEntries((ConvertTo-MeetilyIoPath $directory))) {
            $entry = ConvertFrom-MeetilyIoPath $ioEntry
            $state = Get-MeetilyPathStateUnchecked $entry
            if (($state.attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0) {
                $found.Add($entry)
            } elseif (($state.attributes -band [System.IO.FileAttributes]::Directory) -ne 0) {
                $pending.Enqueue($entry)
            }
        }
    }
    return @($found)
}

function Remove-TestFixtureTree {
    param([Parameter(Mandatory = $true)][string]$Root)
    $rootFull = Assert-MeetilyNoReparsePathChain -Path $Root -Label 'fixture cleanup root'
    if (-not (Test-MeetilyDirectoryExists -Path $rootFull -Label 'fixture cleanup root')) { return }
    Assert-MeetilyNoReparsePoints $rootFull
    $entries = @(Get-MeetilyDirectoryEntries $rootFull)
    foreach ($file in @($entries | Where-Object { -not $_.is_directory })) {
        [System.IO.File]::Delete((ConvertTo-MeetilyIoPath ([string]$file.full_path)))
    }
    foreach ($directory in @($entries | Where-Object { $_.is_directory } | Sort-Object { ([string]$_.full_path).Length } -Descending)) {
        [System.IO.Directory]::Delete((ConvertTo-MeetilyIoPath ([string]$directory.full_path)), $false)
    }
    [System.IO.Directory]::Delete((ConvertTo-MeetilyIoPath $rootFull), $false)
}

try {
    $case = New-CaseLayout 'baseline-recording-preferences'
    Set-OldDataFixture $case.data -Marker 'baseline-pref'
    $selectedRoot = [System.IO.Path]::GetFullPath((Join-Path $case.root 'selected-recordings'))
    $preferencePath = Join-Path $case.data 'recording_preferences.json'
    $document = [ordered]@{ preferences = [ordered]@{
        save_folder = $selectedRoot
        auto_save = $true
        file_format = 'mp4'
        preferred_mic_device = $null
        preferred_system_device = $null
    } }
    Write-TestFile -Path $preferencePath -Content (($document | ConvertTo-Json -Depth 5) + "`n")
    $preferenceBefore = Get-MeetilyFileMetadata $preferencePath
    $backup = New-MeetilyVersionedBackup -DataRoot $case.data -BackupRoot $case.backups -SourceVersion '0.4.1' -TargetVersion '0.4.2'
    Set-CurrentDataFixture $case.data -Marker 'baseline-pref-current'
    $restore = Restore-MeetilyVersionedBackup -DataRoot $case.data -BackupRoot $case.backups -BackupDirectory $backup.backup_directory -SourceVersion '0.4.1' -TargetVersion '0.4.2'
    $preferenceAfter = Get-MeetilyFileMetadata $preferencePath
    $restoredDocument = Read-MeetilyUtf8File $preferencePath | ConvertFrom-Json
    $passed = $backup.status -eq 'PASS' -and $restore.status -eq 'PASS' -and
        [string]$backup.recording_preferences_validation.status -eq 'VALID' -and
        [string]$backup.recording_preferences_validation.layout -eq 'store-root' -and
        [string]$restoredDocument.preferences.save_folder -eq $selectedRoot -and
        [int64]$preferenceBefore.bytes -eq [int64]$preferenceAfter.bytes -and
        [string]$preferenceBefore.sha256 -eq [string]$preferenceAfter.sha256
    Add-TestResult -Name 'valid_baseline_recording_preferences_preserves_selected_directory' -Passed $passed -Details ([ordered]@{
        selected_recording_root = $selectedRoot
        backup_validation = $backup.recording_preferences_validation
        backup_status = $backup.status
        restore_status = $restore.status
        preference_bytes_unchanged = [int64]$preferenceBefore.bytes -eq [int64]$preferenceAfter.bytes
        preference_sha256_unchanged = [string]$preferenceBefore.sha256 -eq [string]$preferenceAfter.sha256
        restored_selected_root = [string]$restoredDocument.preferences.save_folder
    })
} catch {
    Add-TestResult -Name 'valid_baseline_recording_preferences_preserves_selected_directory' -Passed $false -Details ([ordered]@{ error = $_.Exception.Message })
}

try {
    $case = New-CaseLayout 'corrupt-recording-preferences'
    Set-OldDataFixture $case.data -Marker 'corrupt-pref'
    $preferencePath = Join-Path $case.data 'recording_preferences.json'
    Write-TestFile -Path $preferencePath -Content '{"preferences":{"save_folder":7,"auto_save":true,"file_format":"mp4"}}'
    $dataBefore = Get-DataFingerprint $case.data
    $defaultRoot = Join-Path $case.root 'default-recordings-must-stay-absent'
    $failure = Invoke-ExpectedFailure {
        New-MeetilyVersionedBackup -DataRoot $case.data -BackupRoot $case.backups -SourceVersion '0.4.1' -TargetVersion '0.4.2'
    }
    $dataAfter = Get-DataFingerprint $case.data
    $passed = $failure.failed -and $failure.message -match 'recording_preferences\.json' -and
        $dataBefore -eq $dataAfter -and -not (Test-Path -LiteralPath $case.backups) -and
        -not (Test-Path -LiteralPath $defaultRoot)
    Add-TestResult -Name 'corrupt_recording_preferences_is_rejected_without_default_fallback' -Passed $passed -Details ([ordered]@{
        rejection = $failure
        source_data_unchanged = $dataBefore -eq $dataAfter
        backup_root_was_not_created = -not (Test-Path -LiteralPath $case.backups)
        default_recording_root_was_not_created = -not (Test-Path -LiteralPath $defaultRoot)
    })
} catch {
    Add-TestResult -Name 'corrupt_recording_preferences_is_rejected_without_default_fallback' -Passed $false -Details ([ordered]@{ error = $_.Exception.Message })
}

try {
    $case = New-CaseLayout 'insufficient-backup-space'
    Set-OldDataFixture $case.data -Marker 'space'
    $dataBefore = Get-DataFingerprint $case.data
    $failure = Invoke-ExpectedFailure {
        New-MeetilyVersionedBackup -DataRoot $case.data -BackupRoot $case.backups -SourceVersion '0.4.1' -TargetVersion '0.4.2' -AvailableBytesOverride 0
    }
    $dataAfter = Get-DataFingerprint $case.data
    $passed = $failure.failed -and $failure.message -match 'insufficient.*space|free space' -and
        $dataBefore -eq $dataAfter -and -not (Test-Path -LiteralPath $case.backups)
    Add-TestResult -Name 'insufficient_disk_space_is_rejected_before_backup_mutation' -Passed $passed -Details ([ordered]@{
        injected_available_bytes = 0
        rejection = $failure
        source_data_unchanged = $dataBefore -eq $dataAfter
        backup_root_was_not_created = -not (Test-Path -LiteralPath $case.backups)
    })
} catch {
    Add-TestResult -Name 'insufficient_disk_space_is_rejected_before_backup_mutation' -Passed $false -Details ([ordered]@{ error = $_.Exception.Message })
}

try {
    $sourceCase = New-CaseLayout 'wrong-source-version'
    Set-OldDataFixture $sourceCase.data
    $sourceBackup = New-MeetilyVersionedBackup -DataRoot $sourceCase.data -BackupRoot $sourceCase.backups -SourceVersion '0.4.1' -TargetVersion '0.4.2'
    Set-CurrentDataFixture $sourceCase.data
    $sourceLiveBefore = Get-DataFingerprint $sourceCase.data
    $wrongSource = Invoke-ExpectedFailure {
        Restore-MeetilyVersionedBackup -DataRoot $sourceCase.data -BackupRoot $sourceCase.backups -BackupDirectory $sourceBackup.backup_directory -SourceVersion '0.4.0' -TargetVersion '0.4.2'
    }
    $sourceLiveAfter = Get-DataFingerprint $sourceCase.data

    $targetCase = New-CaseLayout 'wrong-target-version'
    Set-OldDataFixture $targetCase.data
    $targetBackup = New-MeetilyVersionedBackup -DataRoot $targetCase.data -BackupRoot $targetCase.backups -SourceVersion '0.4.1' -TargetVersion '0.4.2'
    Set-CurrentDataFixture $targetCase.data
    $targetLiveBefore = Get-DataFingerprint $targetCase.data
    $wrongTarget = Invoke-ExpectedFailure {
        Restore-MeetilyVersionedBackup -DataRoot $targetCase.data -BackupRoot $targetCase.backups -BackupDirectory $targetBackup.backup_directory -SourceVersion '0.4.1' -TargetVersion '0.4.3'
    }
    $targetLiveAfter = Get-DataFingerprint $targetCase.data
    $passed = $wrongSource.failed -and $wrongTarget.failed -and $sourceLiveBefore -eq $sourceLiveAfter -and $targetLiveBefore -eq $targetLiveAfter
    Add-TestResult -Name 'backup_manifest_rejects_wrong_source_or_target_version' -Passed $passed -Details ([ordered]@{
        wrong_source = $wrongSource
        wrong_target = $wrongTarget
        source_live_unchanged = $sourceLiveBefore -eq $sourceLiveAfter
        target_live_unchanged = $targetLiveBefore -eq $targetLiveAfter
    })
} catch {
    Add-TestResult -Name 'backup_manifest_rejects_wrong_source_or_target_version' -Passed $false -Details ([ordered]@{ error = $_.Exception.Message })
}

try {
    $case = New-CaseLayout 'hash-mismatch'
    Set-OldDataFixture $case.data
    $backup = New-MeetilyVersionedBackup -DataRoot $case.data -BackupRoot $case.backups -SourceVersion '0.4.1' -TargetVersion '0.4.2'
    Set-CurrentDataFixture $case.data
    $liveBefore = Get-DataFingerprint $case.data
    Write-TestFile -Path (Join-Path $backup.backup_directory 'payload\settings.json') -Content 'tampered-backup'
    $failure = Invoke-ExpectedFailure {
        Restore-MeetilyVersionedBackup -DataRoot $case.data -BackupRoot $case.backups -BackupDirectory $backup.backup_directory -SourceVersion '0.4.1' -TargetVersion '0.4.2'
    }
    $liveAfter = Get-DataFingerprint $case.data
    Add-TestResult -Name 'backup_restore_rejects_hash_mismatch' -Passed ($failure.failed -and $liveBefore -eq $liveAfter) -Details ([ordered]@{
        rejection = $failure
        live_fingerprint_before = $liveBefore
        live_fingerprint_after = $liveAfter
    })
} catch {
    Add-TestResult -Name 'backup_restore_rejects_hash_mismatch' -Passed $false -Details ([ordered]@{ error = $_.Exception.Message })
}

try {
    $case = New-CaseLayout 'manifest-presence-must-be-boolean'
    Set-OldDataFixture $case.data -Marker 'boolean-manifest'
    $backup = New-MeetilyVersionedBackup -DataRoot $case.data -BackupRoot $case.backups -SourceVersion '0.4.1' -TargetVersion '0.4.2'
    $liveBefore = Get-DataFingerprint $case.data
    $manifest = Read-MeetilyUtf8File $backup.manifest_path | ConvertFrom-Json
    $manifest.data_root_was_present = 'true'
    Write-MeetilyJsonFile -Path $backup.manifest_path -Value $manifest
    $tamperedManifest = Read-MeetilyUtf8File $backup.manifest_path | ConvertFrom-Json
    $failure = Invoke-ExpectedFailure {
        Test-MeetilyVersionedBackup -DataRoot $case.data -BackupRoot $case.backups -BackupDirectory $backup.backup_directory -SourceVersion '0.4.1' -TargetVersion '0.4.2'
    }
    $liveAfter = Get-DataFingerprint $case.data
    $passed = $failure.failed -and $failure.message -match 'data_root_was_present must be a JSON boolean' -and
        $tamperedManifest.data_root_was_present -is [string] -and $liveBefore -eq $liveAfter
    Add-TestResult -Name 'backup_manifest_requires_boolean_data_root_presence' -Passed $passed -Details ([ordered]@{
        rejection = $failure
        tampered_value_type = $tamperedManifest.data_root_was_present.GetType().FullName
        live_data_unchanged = $liveBefore -eq $liveAfter
    })
} catch {
    Add-TestResult -Name 'backup_manifest_requires_boolean_data_root_presence' -Passed $false -Details ([ordered]@{ error = $_.Exception.Message })
}

try {
    $case = New-CaseLayout 'cross-volume-roots'
    Set-CurrentDataFixture $case.data -Marker 'cross-volume-current'
    $otherVolumeRoot = Get-DifferentFilesystemRoot -Path $case.data
    $crossVolumeBackupRoot = Join-Path $otherVolumeRoot ('meetily-r05-cross-volume-' + [Guid]::NewGuid().ToString('N'))
    if (Test-Path -LiteralPath $crossVolumeBackupRoot) { throw "Generated cross-volume path already exists: $crossVolumeBackupRoot" }
    $liveBefore = Get-DataFingerprint $case.data
    $backupFailure = Invoke-ExpectedFailure {
        New-MeetilyVersionedBackup -DataRoot $case.data -BackupRoot $crossVolumeBackupRoot -SourceVersion '0.4.1' -TargetVersion '0.4.2'
    }
    $restoreFailure = Invoke-ExpectedFailure {
        Restore-MeetilyVersionedBackup -DataRoot $case.data -BackupRoot $crossVolumeBackupRoot -BackupDirectory (Join-Path $crossVolumeBackupRoot 'b-not-created') -SourceVersion '0.4.1' -TargetVersion '0.4.2'
    }
    $liveAfter = Get-DataFingerprint $case.data
    $scratch = @(Get-RestoreScratchPaths $case.data)
    $passed = $backupFailure.failed -and $restoreFailure.failed -and
        $backupFailure.message -match 'same filesystem volume' -and $restoreFailure.message -match 'same filesystem volume' -and
        $liveBefore -eq $liveAfter -and -not (Test-Path -LiteralPath $crossVolumeBackupRoot) -and $scratch.Count -eq 0
    Add-TestResult -Name 'cross_volume_roots_are_rejected_before_mutation' -Passed $passed -Details ([ordered]@{
        data_volume_root = [System.IO.Path]::GetPathRoot([System.IO.Path]::GetFullPath($case.data))
        backup_volume_root = $otherVolumeRoot
        backup_rejection = $backupFailure
        restore_rejection = $restoreFailure
        live_data_unchanged = $liveBefore -eq $liveAfter
        backup_root_was_not_created = -not (Test-Path -LiteralPath $crossVolumeBackupRoot)
        scratch_paths_after = $scratch
    })
} catch {
    Add-TestResult -Name 'cross_volume_roots_are_rejected_before_mutation' -Passed $false -Details ([ordered]@{ error = $_.Exception.Message })
}

try {
    $subcases = @()

    $case = New-CaseLayout 'reparse-data-child'
    Set-OldDataFixture $case.data
    $outside = Join-Path $case.root 'outside-data-target'
    Write-TestFile -Path (Join-Path $outside 'do-not-touch.txt') -Content 'outside-data-sentinel'
    New-TestJunction -Path (Join-Path $case.data 'linked-outside') -Target $outside
    $outsideBefore = Get-DataFingerprint $outside
    $failure = Invoke-ExpectedFailure {
        New-MeetilyVersionedBackup -DataRoot $case.data -BackupRoot $case.backups -SourceVersion '0.4.1' -TargetVersion '0.4.2'
    }
    $outsideAfter = Get-DataFingerprint $outside
    $passed = $failure.failed -and $failure.message -match 'reparse point|junction|symbolic link' -and
        $outsideBefore -eq $outsideAfter -and -not (Test-Path -LiteralPath $case.backups)
    $subcases += [ordered]@{
        path_role = 'DataRoot nested entry'
        rejected = $failure.failed
        error = $failure.message
        external_before = $outsideBefore
        external_after = $outsideAfter
        external_unchanged = $outsideBefore -eq $outsideAfter
        backup_root_was_not_created = -not (Test-Path -LiteralPath $case.backups)
        passed = $passed
    }

    $case = New-CaseLayout 'reparse-backup-root'
    Set-OldDataFixture $case.data
    $outside = Join-Path $case.root 'outside-backup-root-target'
    Write-TestFile -Path (Join-Path $outside 'do-not-touch.txt') -Content 'outside-backup-root-sentinel'
    New-TestJunction -Path $case.backups -Target $outside
    $outsideBefore = Get-DataFingerprint $outside
    $sourceBefore = Get-DataFingerprint $case.data
    $failure = Invoke-ExpectedFailure {
        New-MeetilyVersionedBackup -DataRoot $case.data -BackupRoot $case.backups -SourceVersion '0.4.1' -TargetVersion '0.4.2'
    }
    $outsideAfter = Get-DataFingerprint $outside
    $sourceAfter = Get-DataFingerprint $case.data
    $passed = $failure.failed -and $failure.message -match 'reparse point|junction|symbolic link' -and
        $outsideBefore -eq $outsideAfter -and $sourceBefore -eq $sourceAfter
    $subcases += [ordered]@{
        path_role = 'BackupRoot'
        rejected = $failure.failed
        error = $failure.message
        external_before = $outsideBefore
        external_after = $outsideAfter
        external_unchanged = $outsideBefore -eq $outsideAfter
        source_data_unchanged = $sourceBefore -eq $sourceAfter
        passed = $passed
    }

    $case = New-CaseLayout 'reparse-backup-tools'
    Set-OldDataFixture $case.data
    New-Item -ItemType Directory -Path $case.backups -Force | Out-Null
    $outside = Join-Path $case.root 'outside-tools-target'
    Write-TestFile -Path (Join-Path $outside 'do-not-touch.txt') -Content 'outside-tools-sentinel'
    New-TestJunction -Path (Join-Path $case.backups 'tools') -Target $outside
    $outsideBefore = Get-DataFingerprint $outside
    $failure = Invoke-ExpectedFailure {
        New-MeetilyVersionedBackup -DataRoot $case.data -BackupRoot $case.backups -SourceVersion '0.4.1' -TargetVersion '0.4.2'
    }
    $outsideAfter = Get-DataFingerprint $outside
    $publishedBackups = @(Get-ChildItem -LiteralPath $case.backups -Directory -Force | Where-Object { $_.Name -like 'b-*' })
    $passed = $failure.failed -and $failure.message -match 'reparse point|junction|symbolic link' -and
        $outsideBefore -eq $outsideAfter -and $publishedBackups.Count -eq 0
    $subcases += [ordered]@{
        path_role = 'tools directory'
        rejected = $failure.failed
        error = $failure.message
        external_before = $outsideBefore
        external_after = $outsideAfter
        external_unchanged = $outsideBefore -eq $outsideAfter
        published_backup_count = $publishedBackups.Count
        passed = $passed
    }

    $case = New-CaseLayout 'reparse-failed-backups'
    Set-OldDataFixture $case.data
    New-Item -ItemType Directory -Path $case.backups -Force | Out-Null
    $outside = Join-Path $case.root 'outside-failed-backups-target'
    Write-TestFile -Path (Join-Path $outside 'do-not-touch.txt') -Content 'outside-failed-backups-sentinel'
    New-TestJunction -Path (Join-Path $case.backups 'failed-backups') -Target $outside
    $outsideBefore = Get-DataFingerprint $outside
    $failure = Invoke-ExpectedFailure {
        New-MeetilyVersionedBackup -DataRoot $case.data -BackupRoot $case.backups -SourceVersion '0.4.1' -TargetVersion '0.4.2'
    }
    $outsideAfter = Get-DataFingerprint $outside
    $publishedBackups = @(Get-ChildItem -LiteralPath $case.backups -Directory -Force | Where-Object { $_.Name -like 'b-*' })
    $passed = $failure.failed -and $failure.message -match 'reparse point|junction|symbolic link' -and
        $outsideBefore -eq $outsideAfter -and $publishedBackups.Count -eq 0
    $subcases += [ordered]@{
        path_role = 'failed-backups artifacts root'
        rejected = $failure.failed
        error = $failure.message
        external_before = $outsideBefore
        external_after = $outsideAfter
        external_unchanged = $outsideBefore -eq $outsideAfter
        published_backup_count = $publishedBackups.Count
        passed = $passed
    }

    $case = New-CaseLayout 'reparse-staging-chain'
    Set-OldDataFixture $case.data
    $outside = Join-Path $case.root 'outside-staging-target'
    Write-TestFile -Path (Join-Path $outside 'do-not-touch.txt') -Content 'outside-staging-sentinel'
    $stagingLink = Join-Path $case.root 'staging-parent-link'
    New-TestJunction -Path $stagingLink -Target $outside
    $outsideBefore = Get-DataFingerprint $outside
    $failure = Invoke-ExpectedFailure {
        Copy-MeetilyDirectoryContents -Source $case.data -Destination (Join-Path $stagingLink '.rs-known-test-id')
    }
    $outsideAfter = Get-DataFingerprint $outside
    $passed = $failure.failed -and $failure.message -match 'reparse point|junction|symbolic link' -and $outsideBefore -eq $outsideAfter
    $subcases += [ordered]@{
        path_role = 'staging complete ancestor chain'
        rejected = $failure.failed
        error = $failure.message
        external_before = $outsideBefore
        external_after = $outsideAfter
        external_unchanged = $outsideBefore -eq $outsideAfter
        passed = $passed
    }

    $case = New-CaseLayout 'reparse-backup-directory'
    Set-OldDataFixture $case.data
    $backup = New-MeetilyVersionedBackup -DataRoot $case.data -BackupRoot $case.backups -SourceVersion '0.4.1' -TargetVersion '0.4.2'
    $outside = Join-Path $case.root 'outside-backup-directory-target'
    Move-MeetilyDirectory -Source $backup.backup_directory -Destination $outside -Label 'test backup relocation' | Out-Null
    New-TestJunction -Path $backup.backup_directory -Target $outside
    $outsideBefore = Get-DataFingerprint $outside
    $failure = Invoke-ExpectedFailure {
        Test-MeetilyVersionedBackup -DataRoot $case.data -BackupRoot $case.backups -BackupDirectory $backup.backup_directory -SourceVersion '0.4.1' -TargetVersion '0.4.2'
    }
    $outsideAfter = Get-DataFingerprint $outside
    $passed = $failure.failed -and $failure.message -match 'reparse point|junction|symbolic link' -and $outsideBefore -eq $outsideAfter
    $subcases += [ordered]@{
        path_role = 'BackupDirectory'
        rejected = $failure.failed
        error = $failure.message
        external_before = $outsideBefore
        external_after = $outsideAfter
        external_unchanged = $outsideBefore -eq $outsideAfter
        passed = $passed
    }

    $case = New-CaseLayout 'reparse-payload'
    Set-OldDataFixture $case.data
    $backup = New-MeetilyVersionedBackup -DataRoot $case.data -BackupRoot $case.backups -SourceVersion '0.4.1' -TargetVersion '0.4.2'
    $payload = Join-Path $backup.backup_directory 'payload'
    $outside = Join-Path $case.root 'outside-payload-target'
    Move-MeetilyDirectory -Source $payload -Destination $outside -Label 'test payload relocation' | Out-Null
    New-TestJunction -Path $payload -Target $outside
    $outsideBefore = Get-DataFingerprint $outside
    $failure = Invoke-ExpectedFailure {
        Test-MeetilyVersionedBackup -DataRoot $case.data -BackupRoot $case.backups -BackupDirectory $backup.backup_directory -SourceVersion '0.4.1' -TargetVersion '0.4.2'
    }
    $outsideAfter = Get-DataFingerprint $outside
    $passed = $failure.failed -and $failure.message -match 'reparse point|junction|symbolic link' -and $outsideBefore -eq $outsideAfter
    $subcases += [ordered]@{
        path_role = 'payload directory'
        rejected = $failure.failed
        error = $failure.message
        external_before = $outsideBefore
        external_after = $outsideAfter
        external_unchanged = $outsideBefore -eq $outsideAfter
        passed = $passed
    }

    foreach ($role in @('recovery', 'failed-restores')) {
        $case = New-CaseLayout ('reparse-' + $role)
        Set-OldDataFixture $case.data
        $backup = New-MeetilyVersionedBackup -DataRoot $case.data -BackupRoot $case.backups -SourceVersion '0.4.1' -TargetVersion '0.4.2'
        Set-CurrentDataFixture $case.data -Marker ('current-' + $role)
        $liveBefore = Get-DataFingerprint $case.data
        $outside = Join-Path $case.root ('outside-' + $role + '-target')
        Write-TestFile -Path (Join-Path $outside 'do-not-touch.txt') -Content ('outside-' + $role + '-sentinel')
        New-TestJunction -Path (Join-Path $case.backups $role) -Target $outside
        $outsideBefore = Get-DataFingerprint $outside
        $failure = Invoke-ExpectedFailure {
            Restore-MeetilyVersionedBackup -DataRoot $case.data -BackupRoot $case.backups -BackupDirectory $backup.backup_directory -SourceVersion '0.4.1' -TargetVersion '0.4.2'
        }
        $outsideAfter = Get-DataFingerprint $outside
        $liveAfter = Get-DataFingerprint $case.data
        $passed = $failure.failed -and $failure.message -match 'reparse point|junction|symbolic link' -and
            $outsideBefore -eq $outsideAfter -and $liveBefore -eq $liveAfter -and @(Get-RestoreScratchPaths $case.data).Count -eq 0
        $subcases += [ordered]@{
            path_role = $role
            rejected = $failure.failed
            error = $failure.message
            external_before = $outsideBefore
            external_after = $outsideAfter
            external_unchanged = $outsideBefore -eq $outsideAfter
            live_data_unchanged = $liveBefore -eq $liveAfter
            scratch_path_count = @(Get-RestoreScratchPaths $case.data).Count
            passed = $passed
        }
    }

    $case = New-CaseLayout 'reparse-result-path'
    Set-OldDataFixture $case.data
    $outside = Join-Path $case.root 'outside-result-target'
    Write-TestFile -Path (Join-Path $outside 'do-not-touch.txt') -Content 'outside-result-sentinel'
    $resultLink = Join-Path $case.root 'result-link'
    New-TestJunction -Path $resultLink -Target $outside
    $outsideBefore = Get-DataFingerprint $outside
    $windowsPowerShell = Join-Path $env:SystemRoot 'System32\WindowsPowerShell\v1.0\powershell.exe'
    $savedErrorActionPreference = $ErrorActionPreference
    try {
        # Windows PowerShell turns native stderr into non-terminating ErrorRecord
        # objects. Keep those in the captured output instead of aborting this
        # aggregate test before LASTEXITCODE can be checked.
        $ErrorActionPreference = 'Continue'
        $cliOutput = (& $windowsPowerShell -NoLogo -NoProfile -NonInteractive -ExecutionPolicy Bypass -File $ToolPath `
            -Mode Backup -DataRoot $case.data -BackupRoot $case.backups -SourceVersion '0.4.1' -TargetVersion '0.4.2' `
            -ResultPath (Join-Path $resultLink 'result.json') 2>&1 | Out-String).Trim()
        $cliExitCode = [int]$LASTEXITCODE
    } finally {
        $ErrorActionPreference = $savedErrorActionPreference
    }
    $outsideAfter = Get-DataFingerprint $outside
    $passed = $cliExitCode -ne 0 -and $cliOutput -match 'reparse point|junction|symbolic link' -and
        $outsideBefore -eq $outsideAfter -and -not (Test-Path -LiteralPath $case.backups)
    $subcases += [ordered]@{
        path_role = 'result path'
        rejected = $cliExitCode -ne 0
        exit_code = $cliExitCode
        output = $cliOutput
        external_before = $outsideBefore
        external_after = $outsideAfter
        external_unchanged = $outsideBefore -eq $outsideAfter
        backup_root_was_not_created = -not (Test-Path -LiteralPath $case.backups)
        passed = $passed
    }

    Add-TestResult -Name 'reparse_paths_are_rejected_without_external_mutation' -Passed (@($subcases | Where-Object { -not $_.passed }).Count -eq 0) -Details $subcases
} catch {
    Add-TestResult -Name 'reparse_paths_are_rejected_without_external_mutation' -Passed $false -Details ([ordered]@{ error = $_.Exception.Message })
}

try {
    $case = New-CaseLayout 'long-path-roundtrip'
    Set-OldDataFixture $case.data -Marker 'long-old'
    $segmentOne = 'segment-a-' + ('a' * 108)
    $segmentTwo = 'segment-b-' + ('b' * 108)
    $fileName = 'transcript-' + ('c' * 36) + '.json'
    $longRelativePath = Join-Path (Join-Path $segmentOne $segmentTwo) $fileName
    $longSourcePath = Join-Path $case.data $longRelativePath
    Write-TestFile -Path $longSourcePath -Content '{"state":"old","unicode":"会议记录"}'
    $sourceBefore = Get-MeetilyDirectoryManifest $case.data
    $sourceFingerprint = Get-MeetilyManifestFingerprint $sourceBefore
    $longEntry = @($sourceBefore | Where-Object { $_.relative_path -eq $longRelativePath.Replace('\', '/') })

    $backup = New-MeetilyVersionedBackup -DataRoot $case.data -BackupRoot $case.backups -SourceVersion '0.4.1' -TargetVersion '0.4.2'
    $verified = Test-MeetilyVersionedBackup -DataRoot $case.data -BackupRoot $case.backups -BackupDirectory $backup.backup_directory -SourceVersion '0.4.1' -TargetVersion '0.4.2'
    Write-TestFile -Path $longSourcePath -Content '{"state":"current","unicode":"升级后数据"}'
    Write-TestFile -Path (Join-Path $case.data 'new-version-only.json') -Content '{"migration":16}'
    $currentBefore = Get-MeetilyDirectoryManifest $case.data
    $restore = Restore-MeetilyVersionedBackup -DataRoot $case.data -BackupRoot $case.backups -BackupDirectory $backup.backup_directory -SourceVersion '0.4.1' -TargetVersion '0.4.2'
    $restored = Get-MeetilyDirectoryManifest $case.data
    $recoveryFiles = Get-MeetilyDirectoryManifest $restore.previous_data_recovery_directory
    $manifest = Read-MeetilyUtf8File $backup.manifest_path | ConvertFrom-Json
    $payloadLongPath = Join-Path (Join-Path $backup.backup_directory 'payload') $longRelativePath
    $backupDirectoryLeafLength = (Split-Path -Leaf $backup.backup_directory).Length
    $recoveryDirectoryLeafLength = (Split-Path -Leaf $restore.previous_data_recovery_directory).Length
    $passed = $longRelativePath.Length -gt 120 -and $longSourcePath.Length -ge 260 -and $payloadLongPath.Length -ge 260 -and
        $backupDirectoryLeafLength -le 32 -and $recoveryDirectoryLeafLength -le 16 -and
        $longEntry.Count -eq 1 -and $backup.status -eq 'PASS' -and $verified.status -eq 'PASS' -and $restore.status -eq 'PASS' -and
        (Test-MeetilyFileManifestsEqual -Left @($manifest.files) -Right $restored) -and
        (Test-MeetilyFileManifestsEqual -Left $currentBefore -Right $recoveryFiles) -and
        $sourceFingerprint -eq (Get-MeetilyManifestFingerprint $restored) -and @(Get-RestoreScratchPaths $case.data).Count -eq 0
    Add-TestResult -Name 'long_paths_round_trip_through_backup_verify_restore' -Passed $passed -Details ([ordered]@{
        windows_powershell = $PSVersionTable.PSVersion.ToString()
        relative_path_length = $longRelativePath.Length
        source_full_path_length = $longSourcePath.Length
        payload_full_path_length = $payloadLongPath.Length
        final_backup_leaf_length = $backupDirectoryLeafLength
        recovery_leaf_length = $recoveryDirectoryLeafLength
        manifest_long_entry_count = $longEntry.Count
        backup_status = $backup.status
        verify_status = $verified.status
        restore_status = $restore.status
        restored_matches_manifest = Test-MeetilyFileManifestsEqual -Left @($manifest.files) -Right $restored
        replaced_data_preserved = Test-MeetilyFileManifestsEqual -Left $currentBefore -Right $recoveryFiles
        source_fingerprint = $sourceFingerprint
        restored_fingerprint = Get-MeetilyManifestFingerprint $restored
        scratch_path_count = @(Get-RestoreScratchPaths $case.data).Count
    })
} catch {
    Add-TestResult -Name 'long_paths_round_trip_through_backup_verify_restore' -Passed $false -Details ([ordered]@{ error = $_.Exception.Message })
}

try {
    $case = New-CaseLayout 'atomic-supported-restore'
    Set-OldDataFixture $case.data
    $backup = New-MeetilyVersionedBackup -DataRoot $case.data -BackupRoot $case.backups -SourceVersion '0.4.1' -TargetVersion '0.4.2'
    $verifiedBefore = Test-MeetilyVersionedBackup -DataRoot $case.data -BackupRoot $case.backups -BackupDirectory $backup.backup_directory -SourceVersion '0.4.1' -TargetVersion '0.4.2'
    Set-CurrentDataFixture $case.data
    $currentBefore = Get-MeetilyDirectoryManifest $case.data
    $restore = Restore-MeetilyVersionedBackup -DataRoot $case.data -BackupRoot $case.backups -BackupDirectory $backup.backup_directory -SourceVersion '0.4.1' -TargetVersion '0.4.2'
    $manifest = Get-Content -LiteralPath $backup.manifest_path -Raw -Encoding UTF8 | ConvertFrom-Json
    $liveAfter = Get-MeetilyDirectoryManifest $case.data
    $recoveryFiles = Get-MeetilyDirectoryManifest $restore.previous_data_recovery_directory
    $scratch = @(Get-RestoreScratchPaths $case.data)
    $passed = (Test-MeetilyFileManifestsEqual -Left @($manifest.files) -Right $liveAfter) -and
        (Test-MeetilyFileManifestsEqual -Left $currentBefore -Right $recoveryFiles) -and
        $scratch.Count -eq 0
    Add-TestResult -Name 'supported_restore_is_atomic' -Passed $passed -Details ([ordered]@{
        verify = $verifiedBefore
        restore = $restore
        restored_matches_backup = Test-MeetilyFileManifestsEqual -Left @($manifest.files) -Right $liveAfter
        replaced_data_preserved = Test-MeetilyFileManifestsEqual -Left $currentBefore -Right $recoveryFiles
        scratch_paths_after = $scratch
    })
    $canonicalManifestPath = $backup.manifest_path
} catch {
    Add-TestResult -Name 'supported_restore_is_atomic' -Passed $false -Details ([ordered]@{ error = $_.Exception.Message })
}

try {
    $subcases = @()
    foreach ($failurePoint in @('BeforeOriginalMove', 'AfterOriginalMove', 'AfterReplacement', 'BeforeFinalVerification')) {
        $case = New-CaseLayout ('interrupted-' + $failurePoint.ToLowerInvariant())
        Set-OldDataFixture $case.data -Marker ('old-' + $failurePoint.ToLowerInvariant())
        $backup = New-MeetilyVersionedBackup -DataRoot $case.data -BackupRoot $case.backups -SourceVersion '0.4.1' -TargetVersion '0.4.2'
        Set-CurrentDataFixture $case.data -Marker ('current-' + $failurePoint.ToLowerInvariant())
        $liveBefore = Get-DataFingerprint $case.data
        $failure = Invoke-ExpectedFailure {
            Restore-MeetilyVersionedBackup -DataRoot $case.data -BackupRoot $case.backups -BackupDirectory $backup.backup_directory -SourceVersion '0.4.1' -TargetVersion '0.4.2' -FailurePoint $failurePoint
        }
        $liveAfter = Get-DataFingerprint $case.data
        $scratch = @(Get-RestoreScratchPaths $case.data)
        $failedArtifacts = @(Get-ChildItem -LiteralPath (Join-Path $case.backups 'failed-restores') -Directory -ErrorAction SilentlyContinue)
        $expectedFailureObserved = $failure.failed -and $failure.message -match [regex]::Escape("Injected restore failure at $failurePoint")
        $subcases += [ordered]@{
            failure_point = $failurePoint
            rejected = $failure.failed
            expected_failure_observed = $expectedFailureObserved
            error = $failure.message
            original_data_recovered = $liveBefore -eq $liveAfter
            scratch_paths_after = $scratch
            preserved_failed_artifact_count = $failedArtifacts.Count
            passed = $expectedFailureObserved -and $liveBefore -eq $liveAfter -and $scratch.Count -eq 0 -and $failedArtifacts.Count -ge 1
        }
    }
    $faultDetails = $subcases
    Add-TestResult -Name 'interrupted_restore_recovers_original_data' -Passed (@($subcases | Where-Object { -not $_.passed }).Count -eq 0) -Details $subcases
} catch {
    Add-TestResult -Name 'interrupted_restore_recovers_original_data' -Passed $false -Details ([ordered]@{ error = $_.Exception.Message })
}

try {
    $case = New-CaseLayout 'absent-data-root-success'
    $backup = New-MeetilyVersionedBackup -DataRoot $case.data -BackupRoot $case.backups -SourceVersion '0.4.1' -TargetVersion '0.4.2'
    $manifest = Read-MeetilyUtf8File $backup.manifest_path | ConvertFrom-Json
    $verified = Test-MeetilyVersionedBackup -DataRoot $case.data -BackupRoot $case.backups -BackupDirectory $backup.backup_directory -SourceVersion '0.4.1' -TargetVersion '0.4.2'
    Set-CurrentDataFixture $case.data -Marker 'absent-success-current'
    $currentBefore = Get-MeetilyDirectoryManifest $case.data
    $restore = Restore-MeetilyVersionedBackup -DataRoot $case.data -BackupRoot $case.backups -BackupDirectory $backup.backup_directory -SourceVersion '0.4.1' -TargetVersion '0.4.2'
    $recoveryFiles = Get-MeetilyDirectoryManifest $restore.previous_data_recovery_directory
    $scratch = @(Get-RestoreScratchPaths $case.data)
    $passed = $manifest.data_root_was_present -is [bool] -and -not [bool]$manifest.data_root_was_present -and
        $verified.status -eq 'PASS' -and -not [bool]$verified.data_root_was_present -and
        $restore.status -eq 'PASS' -and -not [bool]$restore.data_root_was_present -and
        -not [bool]$restore.restored_data_root_present -and -not (Test-Path -LiteralPath $case.data) -and
        (Test-MeetilyFileManifestsEqual -Left $currentBefore -Right $recoveryFiles) -and $scratch.Count -eq 0
    Add-TestResult -Name 'absent_data_root_restore_keeps_data_root_absent' -Passed $passed -Details ([ordered]@{
        manifest_data_root_was_present = $manifest.data_root_was_present
        verify = $verified
        restore = $restore
        restored_data_root_exists = Test-Path -LiteralPath $case.data
        replaced_data_preserved = Test-MeetilyFileManifestsEqual -Left $currentBefore -Right $recoveryFiles
        scratch_paths_after = $scratch
    })
} catch {
    Add-TestResult -Name 'absent_data_root_restore_keeps_data_root_absent' -Passed $false -Details ([ordered]@{ error = $_.Exception.Message })
}

try {
    $subcases = @()
    foreach ($failurePoint in @('BeforeOriginalMove', 'AfterOriginalMove', 'AfterReplacement', 'BeforeFinalVerification')) {
        $case = New-CaseLayout ('interrupted-absent-' + $failurePoint.ToLowerInvariant())
        $backup = New-MeetilyVersionedBackup -DataRoot $case.data -BackupRoot $case.backups -SourceVersion '0.4.1' -TargetVersion '0.4.2'
        $manifest = Read-MeetilyUtf8File $backup.manifest_path | ConvertFrom-Json
        Set-CurrentDataFixture $case.data -Marker ('current-absent-' + $failurePoint.ToLowerInvariant())
        $liveBefore = Get-DataFingerprint $case.data
        $failure = Invoke-ExpectedFailure {
            Restore-MeetilyVersionedBackup -DataRoot $case.data -BackupRoot $case.backups -BackupDirectory $backup.backup_directory -SourceVersion '0.4.1' -TargetVersion '0.4.2' -FailurePoint $failurePoint
        }
        $liveAfter = Get-DataFingerprint $case.data
        $scratch = @(Get-RestoreScratchPaths $case.data)
        $failedArtifacts = @(Get-ChildItem -LiteralPath (Join-Path $case.backups 'failed-restores') -Directory -ErrorAction SilentlyContinue)
        $expectedFailureObserved = $failure.failed -and $failure.message -match [regex]::Escape("Injected restore failure at $failurePoint")
        $passed = $manifest.data_root_was_present -is [bool] -and -not [bool]$manifest.data_root_was_present -and
            $expectedFailureObserved -and (Test-Path -LiteralPath $case.data -PathType Container) -and
            $liveBefore -eq $liveAfter -and $scratch.Count -eq 0
        $record = [ordered]@{
            source_data_root_was_present = $false
            failure_point = $failurePoint
            rejected = $failure.failed
            expected_failure_observed = $expectedFailureObserved
            error = $failure.message
            original_data_recovered = $liveBefore -eq $liveAfter
            data_root_exists_after_recovery = Test-Path -LiteralPath $case.data -PathType Container
            scratch_paths_after = $scratch
            preserved_failed_artifact_count = $failedArtifacts.Count
            passed = $passed
        }
        $subcases += $record
        $faultDetails += $record
    }
    Add-TestResult -Name 'interrupted_absent_data_root_restore_recovers_original_data' -Passed (@($subcases | Where-Object { -not $_.passed }).Count -eq 0) -Details $subcases
} catch {
    Add-TestResult -Name 'interrupted_absent_data_root_restore_recovers_original_data' -Passed $false -Details ([ordered]@{ error = $_.Exception.Message })
}

try {
    $removedFixtureReparsePoints = @(Remove-TestReparsePoints -Root $FixtureRoot)
    $remainingFixtureReparsePoints = @(Get-TestReparsePoints -Root $FixtureRoot)
    Add-TestResult -Name 'fixture_reparse_points_are_removed_after_safety_tests' `
        -Passed ($removedFixtureReparsePoints.Count -gt 0 -and $remainingFixtureReparsePoints.Count -eq 0) `
        -Details ([ordered]@{
            removed_count = $removedFixtureReparsePoints.Count
            removed_paths = $removedFixtureReparsePoints
            remaining_count = $remainingFixtureReparsePoints.Count
        })
} catch {
    Add-TestResult -Name 'fixture_reparse_points_are_removed_after_safety_tests' -Passed $false -Details ([ordered]@{ error = $_.Exception.Message })
}

try {
    if ($RemoveFixtureRootAfterRun -and $null -ne $canonicalManifestPath -and
        (Test-Path -LiteralPath $canonicalManifestPath -PathType Leaf)) {
        Copy-Item -LiteralPath $canonicalManifestPath -Destination (Join-Path $OutputRoot 'R05-backup-manifest.json')
    }
    if ($RemoveFixtureRootAfterRun) { Remove-TestFixtureTree -Root $FixtureRoot }
    $fixtureRootRemoved = -not (Test-Path -LiteralPath $FixtureRoot)
    Add-TestResult -Name 'embedded_run_fixture_cleanup_contract' `
        -Passed ((-not $RemoveFixtureRootAfterRun) -or $fixtureRootRemoved) `
        -Details ([ordered]@{
            cleanup_requested = [bool]$RemoveFixtureRootAfterRun
            fixture_root_removed = $fixtureRootRemoved
        })
} catch {
    Add-TestResult -Name 'embedded_run_fixture_cleanup_contract' -Passed $false -Details ([ordered]@{ error = $_.Exception.Message })
}

$completed = Get-Date
$failed = @($results | Where-Object { $_.verdict -ne 'PASS' })
$summary = [ordered]@{
    schema_version = 1
    suite = 'meetily-versioned-data'
    fixture_root = $FixtureRoot
    started_at = $started.ToString('o')
    completed_at = $completed.ToString('o')
    producer = [ordered]@{
        path = [System.IO.Path]::GetFullPath($MyInvocation.MyCommand.Path)
        bytes = [int64](Get-Item -LiteralPath $MyInvocation.MyCommand.Path).Length
        sha256 = (Get-FileHash -LiteralPath $MyInvocation.MyCommand.Path -Algorithm SHA256).Hash.ToUpperInvariant()
    }
    invocation = [ordered]@{
        interpreter = 'Windows PowerShell'
        interpreter_version = $PSVersionTable.PSVersion.ToString()
        parameters = [ordered]@{
            output_root = $OutputRoot
            fixture_root = $FixtureRoot
            tool_path = $ToolPath
            remove_fixture_root_after_run = [bool]$RemoveFixtureRootAfterRun
        }
    }
    tool = [ordered]@{
        path = $ToolPath
        bytes = [int64](Get-Item -LiteralPath $ToolPath).Length
        sha256 = (Get-FileHash -LiteralPath $ToolPath -Algorithm SHA256).Hash.ToUpperInvariant()
    }
    nsis_hook = [ordered]@{
        path = $NsisHookPath
        bytes = [int64](Get-Item -LiteralPath $NsisHookPath).Length
        sha256 = (Get-FileHash -LiteralPath $NsisHookPath -Algorithm SHA256).Hash.ToUpperInvariant()
    }
    nsis_template = [ordered]@{
        path = $NsisTemplatePath
        bytes = [int64](Get-Item -LiteralPath $NsisTemplatePath).Length
        sha256 = (Get-FileHash -LiteralPath $NsisTemplatePath -Algorithm SHA256).Hash.ToUpperInvariant()
    }
    total = $results.Count
    passed = $results.Count - $failed.Count
    failed = $failed.Count
    verdict = if ($failed.Count -eq 0) { 'PASS' } else { 'FAIL' }
    results = $results
}

Write-MeetilyJsonFile -Path (Join-Path $OutputRoot 'R05-unit-tests.private.json') -Value $summary
$faultResultNames = @('interrupted_restore_recovers_original_data', 'interrupted_absent_data_root_restore_recovers_original_data')
$faultResults = @($results | Where-Object { $_.name -in $faultResultNames })
Write-MeetilyJsonFile -Path (Join-Path $OutputRoot 'R05-fault-injection.json') -Value ([ordered]@{
    schema_version = 1
    producer = $summary.producer
    invocation = $summary.invocation
    tests = $faultResultNames
    required_failure_points = @('BeforeOriginalMove', 'AfterOriginalMove', 'AfterReplacement', 'BeforeFinalVerification')
    expected_case_count = 8
    observed_case_count = @($faultDetails).Count
    verdict = if ($faultResults.Count -eq $faultResultNames.Count -and @($faultResults | Where-Object { $_.verdict -ne 'PASS' }).Count -eq 0 -and
        @($faultDetails).Count -eq 8 -and @($faultDetails | Where-Object { -not $_.passed }).Count -eq 0) { 'PASS' } else { 'FAIL' }
    cases = $faultDetails
})
if (-not (Test-Path -LiteralPath (Join-Path $OutputRoot 'R05-backup-manifest.json')) -and
    $null -ne $canonicalManifestPath -and (Test-Path -LiteralPath $canonicalManifestPath -PathType Leaf)) {
    Copy-Item -LiteralPath $canonicalManifestPath -Destination (Join-Path $OutputRoot 'R05-backup-manifest.json')
}
$logLines = @(
    "Suite: meetily-versioned-data"
    "Started: $($started.ToString('o'))"
    "Completed: $($completed.ToString('o'))"
    "Total: $($summary.total)"
    "Passed: $($summary.passed)"
    "Failed: $($summary.failed)"
    "Verdict: $($summary.verdict)"
) + @($results | ForEach-Object { "$($_.name): $($_.verdict)" })
Write-MeetilyUtf8File -Path (Join-Path $OutputRoot 'R05-unit-tests.log') -Content (($logLines -join "`n") + "`n")

$summary | ConvertTo-Json -Depth 30
if ($failed.Count -ne 0) { exit 1 }
exit 0

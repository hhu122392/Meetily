[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)][string]$OutputRoot
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$scriptRoot = Split-Path -Parent $MyInvocation.MyCommand.Path
$repoRoot = [System.IO.Path]::GetFullPath((Join-Path $scriptRoot '..\..'))
$OutputRoot = [System.IO.Path]::GetFullPath($OutputRoot)
if (Test-Path -LiteralPath $OutputRoot) {
    if (@(Get-ChildItem -LiteralPath $OutputRoot -Force).Count -ne 0) { throw "OutputRoot must be empty: $OutputRoot" }
} else {
    New-Item -ItemType Directory -Path $OutputRoot | Out-Null
}

$runnerPath = Join-Path $scriptRoot 'run-install-lifecycle.ps1'
$runner = Get-Content -LiteralPath $runnerPath -Raw -Encoding UTF8
$runnerTokens = $null
$runnerParseErrors = $null
[void][System.Management.Automation.Language.Parser]::ParseFile($runnerPath, [ref]$runnerTokens, [ref]$runnerParseErrors)
$meeting = Get-Content -LiteralPath (Join-Path $scriptRoot 'cdp-lifecycle-meeting-check.mjs') -Raw -Encoding UTF8
$exitScript = Get-Content -LiteralPath (Join-Path $scriptRoot 'cdp-exit-app.mjs') -Raw -Encoding UTF8
$selector = Get-Content -LiteralPath (Join-Path $scriptRoot 'select-lifecycle-fixture.py') -Raw -Encoding UTF8
$seed = Get-Content -LiteralPath (Join-Path $scriptRoot 'seed-lifecycle-preservation-fixture.py') -Raw -Encoding UTF8
$migrationTest = Get-Content -LiteralPath (Join-Path $scriptRoot 'test-moss-r5-migration.py') -Raw -Encoding UTF8
$sqliteAudit = Get-Content -LiteralPath (Join-Path $scriptRoot 'sqlite-audit.py') -Raw -Encoding UTF8
$argumentEncoder = Get-Content -LiteralPath (Join-Path $scriptRoot 'windows-native-arguments.ps1') -Raw -Encoding UTF8
$hook = Get-Content -LiteralPath (Join-Path $repoRoot 'frontend\src-tauri\scripts\nsis-installer-hooks.nsh') -Raw -Encoding UTF8
$template = Get-Content -LiteralPath (Join-Path $repoRoot 'frontend\src-tauri\scripts\nsis-installer-template.nsi') -Raw -Encoding UTF8
$uninstallGuardPath = Join-Path $repoRoot 'frontend\src-tauri\scripts\meetily-uninstall-data-guard.ps1'
$uninstallGuard = Get-Content -LiteralPath $uninstallGuardPath -Raw -Encoding UTF8
$uninstallGuardTokens = $null
$uninstallGuardParseErrors = $null
[void][System.Management.Automation.Language.Parser]::ParseFile($uninstallGuardPath, [ref]$uninstallGuardTokens, [ref]$uninstallGuardParseErrors)
$tauriConfigText = Get-Content -LiteralPath (Join-Path $repoRoot 'frontend\src-tauri\tauri.conf.json') -Raw -Encoding UTF8
$tauriConfig = $tauriConfigText | ConvertFrom-Json
$lifecycleTauriConfigText = Get-Content -LiteralPath (Join-Path $repoRoot 'frontend\src-tauri\tauri.lifecycle.conf.json') -Raw -Encoding UTF8
$lifecycleTauriConfig = $lifecycleTauriConfigText | ConvertFrom-Json

$checks = [System.Collections.Generic.List[object]]::new()
function Add-Check {
    param([Parameter(Mandatory = $true)][string]$Name, [Parameter(Mandatory = $true)][bool]$Passed)
    $checks.Add([ordered]@{ name = $Name; verdict = if ($Passed) { 'PASS' } else { 'FAIL' } })
}

function Test-WindowsBundleConfiguration {
    param([Parameter(Mandatory = $true)]$Configuration)
    return $null -ne $Configuration.bundle.windows.PSObject.Properties['allowDowngrades'] -and
        -not [bool]$Configuration.bundle.windows.allowDowngrades -and
        [string]$Configuration.bundle.windows.webviewInstallMode.type -eq 'fixedRuntime' -and
        [string]$Configuration.bundle.windows.webviewInstallMode.path -eq 'runtime/webview2-fixed' -and
        [string]$Configuration.bundle.windows.nsis.installMode -eq 'currentUser' -and
        [string]$Configuration.bundle.windows.nsis.template -eq 'scripts/nsis-installer-template.nsi' -and
        [string]$Configuration.bundle.windows.nsis.installerHooks -eq 'scripts/nsis-installer-hooks.nsh'
}

Add-Check 'runner_parses_without_windows_powershell_errors' (@($runnerParseErrors).Count -eq 0)

Add-Check 'build_manifest_and_git_head_are_hard_bound' (
    $runner -match '\[Parameter\(Mandatory = \$true\)\]\[string\]\$CandidateBuildManifest' -and
    $runner -match '\[Parameter\(Mandatory = \$true\)\]\[string\]\$BaselineBuildManifest' -and
    $runner -match 'Test-FileMatchesExpectedEvidence -Actual \$candidateEvidence -Expected \$candidateBuild\.installer' -and
    $runner -match 'Test-FileMatchesExpectedEvidence -Actual \$baselineEvidence -Expected \$baselineBuild\.installer' -and
    $runner -match 'git -C \$repoRoot rev-parse HEAD' -and
    $runner -match 'git -C \$repoRoot status --porcelain=v1 --untracked-files=all' -and
    $runner -notmatch '\[string\]\$SourceCommit\s*,' -and
    $runner -match 'MOSS_FUNCTIONAL_CANDIDATE_BUILD' -and
    $runner -match 'MOSS_FUNCTIONAL_INSTALL_BUILD_MANIFEST' -and
    $runner -match 'build_provenance' -and $runner -match 'attestedInstaller'
)
Add-Check 'qa_scripts_are_fixed_to_clean_checkout_and_hashed' (
    $runner.Contains('$SqliteAudit = Join-Path $scriptRoot ''sqlite-audit.py''') -and
    $runner -match '\$qaSourceFiles' -and $runner -match 'QA source is outside the checked-out repository' -and
    $runner -match 'qa_source_files = @\(' -and $runner -match 'python = Get-FileEvidence \$PythonPath' -and
    $runner -notmatch '\[string\]\$SqliteAudit\s*,'
)
Add-Check 'installed_critical_files_require_bytes_and_sha256' (
    $runner -match "main_executable = 'meetily\.exe'" -and
    $runner -match "llama_helper = 'llama-helper\.exe'" -and
    $runner -match "moss_helper = 'moss-helper\.exe'" -and
    $runner -match "ffmpeg = 'ffmpeg\.exe'" -and
    $runner -match "directml = 'DirectML\.dll'" -and
    $runner -match "webview2 = 'runtime/webview2-fixed/msedgewebview2\.exe'" -and
    $runner -match "uninstaller = 'uninstall\.exe'" -and
    $runner -match 'installed_files\)\s*\r?\n\s*if \(\$entries\.Count -ne \$requiredRolePaths\.Count\)' -and
    $runner -match 'all_required_files_match_manifest' -and
    ([regex]::Matches($runner, '\.all_required_files_match_manifest')).Count -ge 8
)
Add-Check 'baseline_commit_and_display_icon_are_bound_to_manifest' (
    $runner -match "approvedBaselineSourceCommit = '7392eae159443822c80d3675ca9af388e94b2d71'" -and
    $runner -match "ExpectedRole -eq 'baseline'" -and
    $runner -match 'display_icon_matches_manifest_main_executable' -and
    $runner -match '\$registry\.display_icon' -and
    $runner -match '\$expectedMain\.actual\.path'
)
Add-Check 'release_windows_bundle_configuration_is_fixed' (
    (Test-WindowsBundleConfiguration -Configuration $tauriConfig) -and
    (Test-WindowsBundleConfiguration -Configuration $lifecycleTauriConfig) -and
    [string]$lifecycleTauriConfig.productName -eq 'meetily-p6-lifecycle' -and
    [string]$lifecycleTauriConfig.identifier -eq 'com.meetily.ai.p6lifecycle' -and
    @($tauriConfig.bundle.targets | Where-Object { $_ -eq 'msi' }).Count -eq 0 -and
    @($tauriConfig.bundle.targets | Where-Object { $_ -eq 'nsis' }).Count -eq 1 -and
    $null -eq $tauriConfig.bundle.windows.PSObject.Properties['wix'] -and
    @($lifecycleTauriConfig.bundle.targets).Count -eq 1 -and [string]@($lifecycleTauriConfig.bundle.targets)[0] -eq 'nsis'
)
Add-Check 'exact_uninstall_registry_is_validated' (
    $runner.Contains('$testRegistryPath = ''HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\'' + $ProductName') -and
    $runner -match 'unexpected DisplayName' -and $runner -match 'unexpected install directory' -and
    $runner -match 'unexpected uninstaller' -and $runner -match 'outside the isolated install directory'
)
Add-Check 'path_contract_runs_before_output_creation' (
    $runner.IndexOf('Assert-IsolatedPathContract -ProtectedBaseline', [System.StringComparison]::Ordinal) -lt
        $runner.IndexOf('New-Item -ItemType Directory -Path $OutputRoot', [System.StringComparison]::Ordinal) -and
    $runner -match 'PublicOutput must be outside the private OutputRoot' -and
    $runner -match '\$protectedRoots' -and $runner -match 'FixtureDatabase' -and
    $runner -match 'Assert-NoReparsePath -Path \$OutputRoot.*-AllowMissing' -and
    $runner -match 'Assert-NoReparsePath -Path \$PublicOutput.*-AllowMissing' -and
    $runner -match 'Assert-NoReparsePath -Path \$testDataDirectory.*-AllowMissing' -and
    $runner -match 'Assert-NoReparsePath -Path \$testWebViewDirectory.*-AllowMissing'
)
Add-Check 'reparse_safe_manifest_and_verified_archive' (
    $runner -match 'Get-SafeDirectoryInventory' -and
    $runner -notmatch 'Get-ChildItem -LiteralPath \$Root -Recurse' -and
    $runner -match 'Archive staging copy did not match the source' -and
    $runner -match 'source_fingerprint' -and $runner -match 'destination_fingerprint' -and
    $runner -match '\[System\.IO\.Directory\]::Delete\(\[string\]\$directory, \$false\)'
)
Add-Check 'native_windows_argv_encoding_handles_backslashes_and_quotes' (
    $runner -match '\. \$NativeArgumentScript' -and
    $argumentEncoder -match '\$backslashCount \* 2' -and
    $argumentEncoder -match '\$builder\.Append' -and
    $argumentEncoder -match '\$Value\.Length -eq 0'
)
Add-Check 'timed_out_process_tree_is_stopped' (
    $runner -match 'Get-ProcessTreeIds' -and $runner -match 'Stop-ExactProcessTree' -and
    $runner -match 'stopped exact PID tree' -and $runner -match '\$knownTreeIds' -and
    $runner -match '\$consecutiveEmptyScans -ge 2' -and
    $runner -match 'cdp_port_closed_after_cleanup' -and
    $runner -match 'Stop-ExactTestProcesses -CdpPort \$activeCdpPort'
)
Add-Check 'cdp_is_dynamic_unused_and_owned_by_app_tree' (
    $runner -match '\[int\]\$CdpPort = 0' -and $runner -match 'Get-FreeLoopbackTcpPort' -and
    $runner -match 'Assert-CdpPortUnused' -and $runner -match 'Test-ProcessDescendsFrom' -and
    $runner -match 'listener_owned_by_main_process_tree = \$true' -and
    $runner -match "SetEnvironmentVariable\('CDP_TARGET_ID'"
)
Add-Check 'cdp_clients_bind_exact_target_and_have_timeouts' (
    $meeting -match 'target\.id === cdpTargetId' -and $exitScript -match 'target\.id === cdpTargetId' -and
    $meeting -match 'AbortSignal\.timeout\(5000\)' -and $exitScript -match 'AbortSignal\.timeout\(5000\)' -and
    $meeting -match 'CDP .* timed out' -and $exitScript -match 'CDP .* timed out'
)
Add-Check 'meeting_check_binds_exact_content_and_retries_navigation' (
    $meeting -match 'meeting\?\.id === binding\.meeting_id' -and
    $meeting -match 'details\.heading === binding\.meeting_title' -and
    $meeting -match 'anchorTranscriptVisible' -and $meeting -match 'transcriptFingerprint' -and
    $meeting -match 'transcript\.text \?\? transcript\.transcript' -and
    $meeting -match 'while \(Date\.now\(\) < detailDeadline\)' -and $meeting -match 'catch \(error\)'
)
Add-Check 'graceful_exit_requires_bound_request_and_zero_main_exit' (
    $runner -match 'graceful_exit_request_matches_cdp_binding' -and
    $runner -match '\$mainExitCode -eq 0' -and
    $runner.Contains('[string]$exitReport.method -eq ''plugin:process|exit''') -and
    $runner -match '\$exitReport\.cdpTargetId -eq \[string\]\$cdpBinding\.target_id' -and
    $runner -match 'residualBeforeCleanup\.Count -eq 0'
)
Add-Check 'fixture_snapshot_includes_stable_sqlite_file_set' (
    $selector -match 'source_before != source_after' -and $selector -match 'source\.backup\(destination\)' -and
    $selector -match 'PRAGMA integrity_check' -and $selector -match 'PRAGMA foreign_key_check' -and
    $selector -match 'snapshot_migration_count' -and $selector -match 'source_sidecars_present' -and
    $runner -match '\$fixtureSnapshotPath' -and $runner -match 'snapshot_database_sha256' -and
    $selector -match 'main_and_wal_bound_shm_ignored_and_rebuilt' -and
    $selector -match 'Unapproved SQLite sidecar' -and $selector -notmatch 'source_files = \[database\].*journal'
)
Add-Check 'fixture_main_and_wal_are_bound_to_a_14_migration_manifest' (
    $runner -match '\[Parameter\(Mandatory = \$true\)\]\[string\]\$FixtureManifest' -and
    $runner -match 'Read-FixtureManifest' -and $runner -match "databaseName \+ '-wal'" -and
    $runner -match 'Fixture input does not match its frozen manifest' -and
    $runner -match 'snapshot_last_migration_version' -and $runner -match '20260830000000' -and
    $runner -match 'Assert-FixtureBindingMatchesManifest'
)
Add-Check 'sqlite_audit_is_transactional_and_checks_foreign_keys' (
    $sqliteAudit -match 'connection\.execute\("BEGIN"\)' -and
    $sqliteAudit -match 'PRAGMA foreign_key_check' -and
    $sqliteAudit -match 'foreign_key_violation_count' -and
    $sqliteAudit -match 'report\["foreign_key_check"\] == "ok"'
)
Add-Check 'manual_revision_and_moss_records_are_nonempty' (
    $seed -match 'summary_manual_revisions' -and $seed -match 'moss_transcription_runs' -and
    $seed -match 'moss_activation_segments' -and $seed -match 'any\(value < 1 for value in counts\.values\(\)\)' -and
    $runner -match 'all_required_nonempty_and_equal'
)
Add-Check 'database_schema_and_rows_are_both_compared' (
    $runner -match 'before_schema_sha256' -and $runner -match 'after_schema_sha256' -and
    $runner -match 'schema_equal' -and $runner -match '\[string\]\$left\.schema_sha256 -eq \[string\]\$right\.schema_sha256' -and
    $runner -match '\$schemaPolicy = ''exact''' -and
    $runner -match '\$schemaPolicyPassed = \$schemaEqual' -and
    $runner -match "name -eq 'moss_candidate_segment_alignment'" -and
    $runner -match '8a61f71877ef30aeafbf6adfa43f9796a86888194e5216edbc37889e6f8fac77' -and
    $runner -match '9912ea13b1558d18380171fafd5e63d082fa11b775251f32eca57504a3a3fbe7' -and
    $runner -match "schemaPolicy = 'approved_r4_to_r5_transition'" -and
    ([regex]::Matches($runner, 'approved_r4_to_r5_transition')).Count -eq 1
)
Add-Check 'all_old_indexes_triggers_and_views_are_compared_exactly' (
    $sqliteAudit -match 'sqlite_master' -and $sqliteAudit -match '"objects"' -and
    $runner -match 'Get-AuditObject' -and
    $runner -match "type -in @\('index', 'trigger', 'view'\)" -and
    $runner -match "name -notlike 'sqlite_autoindex_\*'" -and
    $runner -match 'table_name_before' -and $runner -match 'sql_sha256_before' -and
    $runner -match 'all_old_non_table_objects_equal' -and
    $runner -match '\$allTableRowsAndSchemasEqual -and \$allOldNonTableObjectsEqual'
)
Add-Check 'candidate_migration_schema_is_exactly_allowlisted' (
    $runner -match 'moss_audio_token_runs.*797aea33530d9375742356576251b840df066e48a6a3577599137d62310dca2b' -and
    $runner -match 'schema_sha256_matches' -and $runner -match 'migration_count' -and
    $runner -match '@\(\$Audit\.migrations\)\.Count -eq 16' -and
    $runner -match '20260831010000' -and
    $runner -match 'Get-AuditTable -Audit \$Audit -Name ''moss_candidate_segment_alignment_r4''' -and
    $runner -match 'temporary_alignment_table_absent' -and
    $runner -match '8c2acc79cca466b6989b8fec6fd45b65aaa9990861cb9179e015d559e8c57cab' -and
    $runner -match 'alignment_index'
)
Add-Check 'nonempty_r4_to_r5_migration_has_a_standalone_gate' (
    $migrationTest -match 'seed_nonempty_r4_state' -and
    $migrationTest -match 'alignment_rows_are_preserved_exactly' -and
    $migrationTest -match 'run_diagnostics_schema_and_rows_are_unchanged' -and
    $migrationTest -match 'foreign_key_check_is_empty' -and
    $migrationTest -match 'EXPECTED_SCHEMA_SHA256' -and
    $migrationTest -notmatch 'TO_BE_RECORDED'
)
Add-Check 'fresh_and_same_version_installs_do_not_backup' (
    $runner -match 'backup_absent_after_fresh_install' -and
    $runner -match 'same_version_backup_unchanged' -and
    $runner -match 'same_version_backup_directory_absent' -and
    $runner -match '\$sameVersionBackupAbsentAfterRepair = -not \(Test-Path -LiteralPath \$testBackupRoot\)' -and
    $runner -match 'baseline_backup_absent_after_fresh_install'
)
Add-Check 'blocked_downgrade_proves_all_state_unchanged' (
    $runner -match 'direct_install_unchanged' -and $runner -match 'direct_data_unchanged' -and
    $runner -match 'direct_backup_unchanged' -and $runner -match 'direct_registry_unchanged' -and
    $runner -match 'direct_recursive_registry_tree_unchanged' -and
    $runner -match 'direct_recursive_registry_tree_present' -and
    $runner -match 'direct_webview_tree_unchanged' -and $runner -match 'direct_webview_tree_present' -and
    $runner -match 'direct_shortcuts_unchanged' -and
    $runner -match 'direct_required_desktop_and_start_menu_shortcuts_present' -and
    $runner -match 'direct_processes_unchanged' -and $runner -match 'direct_cdp_listeners_unchanged' -and
    $runner -match 'Get-UninstallRegistryTreeSnapshot' -and $runner -match 'Get-DirectoryTreeSnapshot' -and
    $runner -match 'GetValueNames\(\)' -and $runner -match 'GetValueKind' -and
    $runner -match 'relative_path = \$relativePath' -and
    $runner -match 'Get-ProductShortcutSnapshot' -and $runner -match 'Get-TestCdpListenerSnapshot'
)
Add-Check 'uninstall_and_second_upgrade_have_complete_proofs' (
    $runner -match 'uninstall_preserved_complete_data' -and
    $runner -match 'database_immediately_before_second_upgrade' -and
    $runner.IndexOf("Invoke-DatabaseAudit -Label 'immediately-before-second-upgrade'", [System.StringComparison]::Ordinal) -lt
        $runner.IndexOf("Invoke-TestInstaller -Installer `$CandidateInstaller -Label 'upgrade-after-rollback'", [System.StringComparison]::Ordinal) -and
    $runner -match 'second_upgrade_backup_is_new' -and
    $runner -match 'second_upgrade_changed_backup_root' -and
    $runner -match 'second_upgrade_backup\.verify\.record\.status'
)
Add-Check 'all_non_database_files_and_nested_model_marker_are_preserved' (
    $runner -match "models\\lifecycle\\nested\\fixture-model\\model-marker\.json" -and
    $runner -match 'Get-NonDatabaseDataManifest' -and $runner -match 'Compare-PreservedFileManifest' -and
    $runner -match 'file_checks' -and $runner -match 'all_before_files_preserved' -and
    $runner -match 'all_non_database_files_preserved' -and
    $runner -match 'rollback_preserved_all_non_database_files' -and
    $runner -match 'second_upgrade_preserved_all_non_database_files' -and
    $runner -match 'nested_model_marker_after_rollback' -and
    $runner -match 'nested_model_marker_after_second_upgrade' -and
    $runner -match 'non_database_manifest_before_second_upgrade' -and
    $runner -match 'non_database_manifest_after_second_upgrade'
)
Add-Check 'backup_result_path_and_trusted_tool_are_independently_validated' (
    $runner -match 'Assert-NoReparsePath -Path \$backupDirectory -ExpectedRoot \$testBackupRoot' -and
    $runner -match 'backup-result manifest SHA-256 does not match' -and
    $runner -match 'trusted rollback tool does not match the approved source-tool hash'
)
Add-Check 'private_evidence_tree_is_hashed_and_public_errors_are_redacted' (
    $runner -match "evidence-files\.private\.json" -and $runner -match 'private_evidence_manifest' -and
    $runner -match "error_code = 'LIFECYCLE_EXECUTION_FAILED'" -and
    $runner -notmatch 'error = \$result\.error' -and
    $runner -match 'isolated_test_data_moved_to_private_evidence = \$finalArchiveOk'
)
Add-Check 'cleanup_actions_and_post_cleanup_checks_are_individually_captured' (
    $runner -match 'step_errors\.stop_processes' -and $runner -match 'step_errors\.uninstall' -and
    $runner -match 'step_errors\.archive_data' -and $runner -match 'step_errors\.archive_webview' -and
    $runner -match 'step_errors\.archive_backups' -and $runner -match 'step_errors\.protected_snapshot' -and
    $runner -match 'step_errors\.check_registry' -and $runner -match 'step_errors\.check_install_directory' -and
    $runner -match 'step_errors\.check_data_directory' -and $runner -match 'step_errors\.check_webview_directory' -and
    $runner -match 'step_errors\.check_backup_directory' -and $runner -match 'step_errors\.check_processes' -and
    $runner -match 'step_errors\.check_shortcuts' -and $runner -match 'final_product_shortcut_count -eq 0' -and
    $runner -match 'failed_step_codes = @\(\$result\.cleanup\.step_errors\.Keys\)'
)
Add-Check 'nsis_downgrade_and_backup_timeout_run_before_file_copy' (
    $template -match '(?s)Section EarlyChecks.*?SemverCompare "\$\{VERSION\}" \$MeetilyInstalledVersion.*?SetErrorLevel 3.*?Quit.*?Section WebView2' -and
    $hook -match 'nsExec::ExecToLog /TIMEOUT=1200000' -and
    $hook.IndexOf('nsExec::ExecToLog', [System.StringComparison]::Ordinal) -lt $hook.IndexOf('DirectML.dll', [System.StringComparison]::Ordinal)
)
Add-Check 'nsis_rejects_a_second_concurrent_installer_before_any_page_or_write' (
    $template -match '(?m)^Var MeetilyInstallerMutexHandle\s*$' -and
    $template -match '(?s)Function \.onInit.*?CreateMutexW.*?MeetilyInstaller-\$\{BUNDLEID\}.*?\$R1 = 183.*?SetErrorLevel 1618.*?Quit' -and
    $template -match 'CloseHandle\(p r[0-9]+\)' -and
    $runner -match 'Mutex\]::new' -and $runner -match 'fault_mutex_created_before_second' -and
    $runner -notmatch 'Start-Process -FilePath \$CandidateInstaller -WindowStyle Hidden' -and
    $runner -match 'second-installer-concurrent-refusal' -and
    $runner -match 'concurrent_installer_refused' -and $runner -match 'product_state_unchanged' -and
    $runner -match '1618'
)
Add-Check 'versioned_data_preflights_space_preferences_and_four_restore_interruptions' (
    $runner -match 'test-meetily-versioned-data\.ps1' -and
    $runner -match 'R05-fault-injection\.json' -and
    $runner -match 'fault_injection_all_pass' -and
    $runner -match 'BeforeFinalVerification' -and
    $runner -match 'insufficient_disk_space_rejected' -and
    $runner -match 'corrupt_recording_preferences_rejected' -and
    $runner -match 'valid_recording_preferences_preserved' -and
    $runner -match '\$ft24.*fault_injection_all_pass'
)
Add-Check 'ft25_runs_old_version_against_new_data_and_requires_exit_101_without_mutation' (
    $runner -match 'old-version-against-new-data' -and
    $runner -match 'old_version_rejects_new_data_exit_nonzero' -and
    $runner -match 'old_version_rejects_new_data_exit_code_101' -and
    $runner -match 'new_data_recursive_manifest_unchanged' -and
    $runner -match '\$ft25.*old_version_rejects_new_data_exit_nonzero' -and
    $runner -match '\$ft25.*new_data_recursive_manifest_unchanged'
)
Add-Check 'machine_lifecycle_has_one_fixed_silent_chain_and_leaves_ui_smoke_pending' (
    $runner -match 'fresh-install' -and $runner -match 'same-version-repair' -and
    $runner -match 'upgrade' -and $runner -match 'direct-downgrade-refused' -and
    $runner -match 'supported-rollback' -and $runner -match 'upgrade-after-rollback' -and
    $runner -match 'final-uninstall' -and
    $runner -match "label = 'interactive-install-ui-smoke'" -and
    $runner -match "status = 'PENDING_FINAL_CANDIDATE_UI'" -and
    $runner -match "label = 'FT-26-reensure-candidate-and-verify-save-directory'" -and
    $runner -notmatch "label = 'interactive-install-ui-smoke'.{0,200}status = 'PASS'"
)
Add-Check 'uninstall_lists_and_validates_every_recursive_delete_target' (
    @($uninstallGuardParseErrors).Count -eq 0 -and
    $hook -match 'meetily-uninstall-data-guard\.ps1' -and
    $hook -match 'AppDataRoot "\$APPDATA"' -and $hook -match 'LocalAppDataRoot "\$LOCALAPPDATA"' -and
    $hook -match '\$MeetilyDeleteDataPath' -and $hook -match '\$MeetilyDeleteWebViewPath' -and
    $template -match 'RmDir /r "\$MeetilyDeleteDataPath"' -and
    $template -match 'RmDir /r "\$MeetilyDeleteWebViewPath"' -and
    $template -notmatch 'RmDir /r "\$APPDATA\\\$\{BUNDLEID\}"' -and
    $template -notmatch 'RmDir /r "\$LOCALAPPDATA\\\$\{BUNDLEID\}"' -and
    $uninstallGuard -match 'Assert-ExactDirectChild' -and
    $uninstallGuard -match 'Assert-NoReparseAncestor' -and
    $uninstallGuard -match 'Assert-NoReparseTree' -and
    $uninstallGuard -match 'FileAttributes\]::ReparsePoint'
)
$guardAppData = Join-Path $OutputRoot 'guard-appdata'
$guardLocalAppData = Join-Path $OutputRoot 'guard-localappdata'
[System.IO.Directory]::CreateDirectory($guardAppData) | Out-Null
[System.IO.Directory]::CreateDirectory($guardLocalAppData) | Out-Null
$guardBundleId = 'com.meetily.ai.guardfixture'
try {
    $guardResult = (& $uninstallGuardPath -AppDataRoot $guardAppData -LocalAppDataRoot $guardLocalAppData -BundleId $guardBundleId | ConvertFrom-Json)
    $guardSafeMissingPassed = [string]$guardResult.status -eq 'PASS' -and
        [string]$guardResult.data_target -eq [System.IO.Path]::GetFullPath((Join-Path $guardAppData $guardBundleId)) -and
        [string]$guardResult.webview_target -eq [System.IO.Path]::GetFullPath((Join-Path $guardLocalAppData $guardBundleId))
} catch {
    $guardSafeMissingPassed = $false
}
Add-Check 'uninstall_guard_accepts_only_the_two_exact_safe_missing_targets' $guardSafeMissingPassed

$junctionTarget = Join-Path $OutputRoot 'guard-junction-target'
[System.IO.Directory]::CreateDirectory($junctionTarget) | Out-Null
$junctionPath = Join-Path $guardAppData $guardBundleId
$junctionCreated = $false
try {
    New-Item -ItemType Junction -Path $junctionPath -Target $junctionTarget -ErrorAction Stop | Out-Null
    $junctionCreated = $true
    try {
        & $uninstallGuardPath -AppDataRoot $guardAppData -LocalAppDataRoot $guardLocalAppData -BundleId $guardBundleId | Out-Null
        $guardReparseRejected = $false
    } catch {
        $guardReparseRejected = [string]$_.Exception.Message -match 'reparse point'
    }
} catch {
    $guardReparseRejected = $false
}
Add-Check 'uninstall_guard_rejects_a_reparse_delete_target' ($junctionCreated -and $guardReparseRejected)
if ($junctionCreated -and (Test-Path -LiteralPath $junctionPath)) {
    [System.IO.Directory]::Delete($junctionPath, $false)
}

$nestedGuardDataTarget = Join-Path $guardAppData $guardBundleId
$nestedGuardOutside = Join-Path $OutputRoot 'guard-nested-junction-outside'
$nestedGuardJunction = Join-Path $nestedGuardDataTarget 'nested\redirect'
$nestedGuardRejected = $false
$nestedGuardOutsideUnchanged = $false
$nestedTargetIsPlainDirectory = $false
$nestedChildIsReparsePoint = $false
try {
    if (-not (Test-Path -LiteralPath $nestedGuardDataTarget -PathType Container)) {
        [System.IO.Directory]::CreateDirectory($nestedGuardDataTarget) | Out-Null
    }
    [System.IO.Directory]::CreateDirectory($nestedGuardOutside) | Out-Null
    $nestedSentinel = Join-Path $nestedGuardOutside 'do-not-touch.txt'
    [System.IO.File]::WriteAllText($nestedSentinel, 'nested-junction-sentinel', [System.Text.UTF8Encoding]::new($false))
    $nestedSentinelBefore = (Get-FileHash -LiteralPath $nestedSentinel -Algorithm SHA256).Hash
    New-Item -ItemType Junction -Path $nestedGuardJunction -Target $nestedGuardOutside -ErrorAction Stop | Out-Null
    $nestedTargetIsPlainDirectory = ((Get-Item -LiteralPath $nestedGuardDataTarget -Force).Attributes -band [System.IO.FileAttributes]::ReparsePoint) -eq 0
    $nestedChildIsReparsePoint = ((Get-Item -LiteralPath $nestedGuardJunction -Force).Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0
    try {
        & $uninstallGuardPath -AppDataRoot $guardAppData -LocalAppDataRoot $guardLocalAppData -BundleId $guardBundleId | Out-Null
    } catch {
        $nestedGuardRejected = [string]$_.Exception.Message -match 'reparse point'
    }
    $nestedGuardOutsideUnchanged = (Get-FileHash -LiteralPath $nestedSentinel -Algorithm SHA256).Hash -eq $nestedSentinelBefore
} catch {
    $nestedGuardRejected = $false
}
Add-Check 'uninstall_guard_rejects_a_nested_junction_without_external_mutation' (
    $nestedTargetIsPlainDirectory -and $nestedChildIsReparsePoint -and $nestedGuardRejected -and $nestedGuardOutsideUnchanged
)

try {
    & $uninstallGuardPath -AppDataRoot $guardAppData -LocalAppDataRoot $guardLocalAppData -BundleId '..' | Out-Null
    $guardBroadNameRejected = $false
} catch {
    $guardBroadNameRejected = $true
}
Add-Check 'uninstall_guard_rejects_a_broad_bundle_name' $guardBroadNameRejected

$failed = @($checks | Where-Object { $_.verdict -ne 'PASS' })
$sourceFiles = @(
    'test-install-lifecycle-contract.ps1',
    'run-install-lifecycle.ps1', 'cdp-lifecycle-meeting-check.mjs', 'cdp-exit-app.mjs',
    'select-lifecycle-fixture.py', 'seed-lifecycle-preservation-fixture.py', 'sqlite-audit.py',
    'test-moss-r5-migration.py',
    'windows-native-arguments.ps1', '..\..\frontend\src-tauri\scripts\nsis-installer-hooks.nsh',
    '..\..\frontend\src-tauri\scripts\nsis-installer-template.nsi',
    '..\..\frontend\src-tauri\scripts\meetily-uninstall-data-guard.ps1',
    '..\..\frontend\src-tauri\tauri.conf.json',
    '..\..\frontend\src-tauri\tauri.lifecycle.conf.json'
)
$result = [ordered]@{
    schema_version = 1
    suite = 'install-lifecycle-contract'
    generated_at = (Get-Date).ToString('o')
    total = $checks.Count
    passed = $checks.Count - $failed.Count
    failed = $failed.Count
    verdict = if ($failed.Count -eq 0) { 'PASS' } else { 'FAIL' }
    checks = $checks
    source_files = @(
        foreach ($relativePath in $sourceFiles) {
            $path = [System.IO.Path]::GetFullPath((Join-Path $scriptRoot $relativePath))
            [ordered]@{
                relative_path = $path.Substring($repoRoot.Length + 1).Replace('\', '/')
                bytes = [int64](Get-Item -LiteralPath $path).Length
                sha256 = (Get-FileHash -LiteralPath $path -Algorithm SHA256).Hash.ToUpperInvariant()
            }
        }
    )
}
$resultPath = Join-Path $OutputRoot 'install-lifecycle-contract.private.json'
[System.IO.File]::WriteAllText($resultPath, (($result | ConvertTo-Json -Depth 12) + "`n"), [System.Text.UTF8Encoding]::new($false))
$result | ConvertTo-Json -Depth 12
if ($failed.Count -ne 0) { exit 1 }

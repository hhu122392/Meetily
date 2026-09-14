[CmdletBinding()]
param(
    [string]$OutputRoot = (Join-Path 'D:\MeetilyBuildScratch' ('moss-functional-ft-contract-' + (Get-Date -Format 'yyyyMMdd-HHmmssfff')))
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'

$scriptRoot = Split-Path -Parent $MyInvocation.MyCommand.Path
$repoRoot = [System.IO.Path]::GetFullPath((Join-Path $scriptRoot '..\..'))
$runner = Join-Path $scriptRoot 'run-moss-functional-ft.ps1'
$cdp = Join-Path $scriptRoot 'moss-functional-ft-cdp.mjs'
$producerInputGenerator = Join-Path $scriptRoot 'new-moss-functional-ft-producer-input.ps1'
$sandboxLauncher = Join-Path $scriptRoot 'invoke-moss-functional-ft-sandbox.ps1'
$sandboxWorker = Join-Path $scriptRoot 'moss-functional-ft-sandbox-worker.ps1'
$windowsPowerShell = Join-Path $env:SystemRoot 'System32\WindowsPowerShell\v1.0\powershell.exe'
$OutputRoot = [System.IO.Path]::GetFullPath($OutputRoot)
if (Test-Path -LiteralPath $OutputRoot) { throw "OutputRoot already exists: $OutputRoot" }
[System.IO.Directory]::CreateDirectory($OutputRoot) | Out-Null

$tests = [System.Collections.Generic.List[object]]::new()
function Add-Test {
    param([string]$Name, [bool]$Passed, [string]$Detail)
    $tests.Add([ordered]@{ name = $Name; passed = $Passed; detail = $Detail })
    if (-not $Passed) { Write-Host "FAIL $Name :: $Detail" -ForegroundColor Red }
}

function Write-Utf8 {
    param([string]$Path, [string]$Content)
    $parent = Split-Path -Parent $Path
    if (-not (Test-Path -LiteralPath $parent -PathType Container)) { [System.IO.Directory]::CreateDirectory($parent) | Out-Null }
    [System.IO.File]::WriteAllText($Path, $Content, [System.Text.UTF8Encoding]::new($false))
}

function Write-Json {
    param([string]$Path, $Value)
    Write-Utf8 $Path (($Value | ConvertTo-Json -Depth 60) + [Environment]::NewLine)
}

function File-Record {
    param([string]$Path)
    $item = Get-Item -LiteralPath $Path
    [ordered]@{ path = $item.FullName; bytes = [int64]$item.Length; sha256 = (Get-FileHash -LiteralPath $item.FullName -Algorithm SHA256).Hash }
}

$runnerText = Get-Content -LiteralPath $runner -Raw -Encoding UTF8
$cdpText = Get-Content -LiteralPath $cdp -Raw -Encoding UTF8
$generatorText = Get-Content -LiteralPath $producerInputGenerator -Raw -Encoding UTF8
$launcherText = Get-Content -LiteralPath $sandboxLauncher -Raw -Encoding UTF8
$workerText = Get-Content -LiteralPath $sandboxWorker -Raw -Encoding UTF8
$tokens = $null
$parseErrors = $null
[System.Management.Automation.Language.Parser]::ParseFile($runner, [ref]$tokens, [ref]$parseErrors) | Out-Null
Add-Test 'powershell-5-parser' ($parseErrors.Count -eq 0) (($parseErrors | ForEach-Object { $_.Message }) -join '; ')
$supportParseErrors = @()
foreach ($supportScript in @($producerInputGenerator, $sandboxLauncher, $sandboxWorker)) {
    $supportTokens = $null; $errors = $null
    [System.Management.Automation.Language.Parser]::ParseFile($supportScript, [ref]$supportTokens, [ref]$errors) | Out-Null
    $supportParseErrors += @($errors | ForEach-Object { "$(Split-Path -Leaf $supportScript):$($_.Extent.StartLineNumber):$($_.Message)" })
}
Add-Test 'support-powershell-5-parser' ($supportParseErrors.Count -eq 0) ($supportParseErrors -join '; ')
$nodeCheck = & node.exe --check $cdp 2>&1 | Out-String
Add-Test 'node-syntax' ($LASTEXITCODE -eq 0) $nodeCheck.Trim()

$parameterNames = @('Mode', 'ProducerKey', 'AcceptanceConfig')
$ast = [System.Management.Automation.Language.Parser]::ParseFile($runner, [ref]$tokens, [ref]$parseErrors)
$actualParameters = @($ast.ParamBlock.Parameters | ForEach-Object { $_.Name.VariablePath.UserPath })
Add-Test 'exact-public-parameter-contract' (($actualParameters -join '|') -eq ($parameterNames -join '|')) ($actualParameters -join ',')
$generatorTokens = $null; $generatorErrors = $null
$generatorAst = [System.Management.Automation.Language.Parser]::ParseFile($producerInputGenerator, [ref]$generatorTokens, [ref]$generatorErrors)
$generatorParameters = @($generatorAst.ParamBlock.Parameters | ForEach-Object { $_.Name.VariablePath.UserPath })
Add-Test 'exact-sidecar-generator-parameter-contract' (($generatorParameters -join '|') -eq 'AcceptanceConfig|Values|Output') ($generatorParameters -join ',')

$expectedMap = [ordered]@{
    'ft01-03-live-and-persistence' = 1..3
    'ft04-15-moss-chain' = 4..15
    'ft16-21-fault-chain' = 16..21
    'ft22-25-install-lifecycle' = 22..25
    'ft26-data-drive-placement' = @(26)
    'ft27-long-audio' = @(27)
    'ft28-business-chain' = @(28)
}
foreach ($entry in $expectedMap.GetEnumerator()) {
    $keyPresent = $runnerText.Contains("'$($entry.Key)'")
    $idsPresent = @($entry.Value | Where-Object { -not $runnerText.Contains("'FT-{0:D2}'" -f $_) }).Count -eq 0
    Add-Test ("mapping-" + $entry.Key) ($keyPresent -and $idsPresent) "key=$keyPresent ids=$idsPresent"
}
Add-Test 'seven-groups-only' (([regex]::Matches($runnerText, "(?m)^\s*'ft(?:0[1-9]|1[0-9]|2[0-9])[^']*'\s*=\s*@\(")).Count -eq 7) 'group map must have seven entries'
$oldDriverMarker = 'MOSS' + [char]0x529F + [char]0x80FD + [char]0x6D4B + [char]0x8BD5 + [char]0x8BA1 + [char]0x5212 + '-20260902'
Add-Test 'no-obsolete-untracked-driver-path' (-not ($runnerText + $cdpText).Contains($oldDriverMarker)) 'must not depend on the old untracked driver directory'
Add-Test 'no-hardcoded-old-worktree' (-not ($runnerText + $cdpText).Contains('meetily-moss-ft-1b9c29a-20260902')) 'must not depend on the old worktree'
$mainCheckoutMarker = 'D:\' + [char]0x684C + [char]0x9762 + '\meetlily'
Add-Test 'no-private-desktop-path' (-not ($runnerText + $cdpText).Contains($mainCheckoutMarker)) 'must not hard-code the main checkout'
Add-Test 'no-vague-stop-process' (-not [regex]::IsMatch($runnerText, '(?i)Stop-Process\s+-(?:Name|InputObject)|Get-Process\s+-Name')) 'termination must be exact PID/path scoped'
Add-Test 'no-host-firewall-mutation' (-not [regex]::IsMatch($runnerText, '(?i)(New|Set|Remove)-NetFirewallRule')) 'host runner must not change the firewall'
Add-Test 'exact-cdp-target-required' ($cdpText.Contains('CDP_TARGET_ID') -and $cdpText.Contains('target.id === cdpTargetId')) 'CDP target must be process-bound'
Add-Test 'exclusive-cdp-output' ($cdpText.Contains('flag: "wx"')) 'raw CDP evidence must not overwrite'
Add-Test 'strict-correction-presence' ($cdpText.Contains('MOSS corrections are absent; strict correction tests cannot pass')) 'missing corrections must fail'
Add-Test 'strict-participant-presence' ($cdpText.Contains('At least two real participants are required')) 'missing participants must fail'
Add-Test 'no-null-speaker-default-pass' (-not $cdpText.Contains('binding === null ||')) 'missing speaker evidence must not pass'
Add-Test 'no-null-correction-default-pass' (-not $cdpText.Contains('correctionToggle === null ||')) 'missing correction evidence must not pass'
$missingPublicFields = @(@('verdict', 'run_id', 'source_commit', 'candidate_sha256', 'build_manifest_sha256', 'metrics', 'checks') | Where-Object { -not $runnerText.Contains($_) })
Add-Test 'public-binding-fields' ($missingPublicFields.Count -eq 0) ('missing=' + ($missingPublicFields -join ','))
Add-Test 'cleanup-two-zero-scans' ($runnerText.Contains('for ($attempt = 1; $attempt -le 2; $attempt++)') -and $runnerText.Contains('consecutive_zero_scans')) 'cleanup must prove two zero scans'
Add-Test 'lifecycle-one-call-site' (([regex]::Matches($runnerText, 'Invoke-NativeCapture \$windowsPowerShell \$arguments ''single hardened install lifecycle''')).Count -eq 1) 'lifecycle runner must be invoked once per group'
Add-Test 'long-tail-hard-gate' ($runnerText.Contains('tail_difference_at_most_half_second') -and $runnerText.Contains('$tailDifference -le 0.5')) 'FT-27 tail gate missing'
$missingBusinessGates = @(@('moss_cer_at_most_20_percent','moss_not_worse_than_whisper','corrected_cer_at_most_15_percent','positive_term_accuracy_at_least_95_percent','negative_term_insertions_zero','moss_rtf_at_most_one','manual_speaker_coverage_error_zero') | Where-Object { -not $runnerText.Contains($_) })
Add-Test 'business-quality-hard-gates' ($missingBusinessGates.Count -eq 0) ('missing=' + ($missingBusinessGates -join ','))
Add-Test 'sandbox-required' ($runnerText.Contains('WindowsSandbox.exe') -and $runnerText.Contains('windows_sandbox_proven') -and $runnerText.Contains('host_firewall_untouched')) 'fault chain must be sandbox-bound'
Add-Test 'fixed-sidecar-generator' ($generatorText.Contains('[System.IO.FileMode]::CreateNew') -and $generatorText.Contains('MOSS_FUNCTIONAL_FT_PRODUCER_INPUT') -and $runnerText.Contains('Producer private input was not made by the fixed repository generator')) 'private sidecar must be exclusively generated and hash-bound'
Add-Test 'models-and-runtime-seeded' ($runnerText.Contains('Ensure-BoundRuntimeAssets') -and $runnerText.Contains('MOSS-Transcribe-Diarize-Q8_0.gguf') -and $runnerText.Contains('Qwen3.5-2B-Q4_K_M.gguf') -and $runnerText.Contains('ggml-large-v3-turbo-q5_0.bin')) 'formal model/runtime seeding is missing'
Add-Test 'fixed-sandbox-worker' ($launcherText.Contains('WindowsSandbox.exe') -and $launcherText.Contains('moss-functional-ft-sandbox-worker.ps1') -and $runnerText.Contains('fixed_worker_hash_bound')) 'fault chain must use the fixed worker'
Add-Test 'sandbox-case-specific-facts' ($workerText.Contains('network_block_effective') -and $workerText.Contains('interrupted_status_explicit') -and $workerText.Contains('protected_hashes_unchanged') -and $workerText.Contains('model_corruption_verified') -and $workerText.Contains('summary_model_missing_verified') -and $workerText.Contains('helper_overlap_count_zero') -and -not $runnerText.Contains('case_verdict_pass')) 'Sandbox cases must expose exact facts instead of a copied verdict'
Add-Test 'host-firewall-readonly-proof' ($runnerText.Contains('Get-HostFirewallSnapshot') -and -not [regex]::IsMatch($runnerText, '(?i)(New|Set|Remove)-NetFirewallRule')) 'host must only snapshot firewall state'
Add-Test 'all-isolated-roots-owned-before-archive' ($runnerText.Contains("label = 'data'") -and $runnerText.Contains("label = 'webview'") -and $runnerText.Contains("label = 'rollback-backups'") -and $runnerText.Contains('Ensure-OwnedIsolatedRoot') -and $runnerText.Contains('Assert-OwnedIsolatedRoot')) 'data, WebView and rollback backup roots must be marked and have exact run ownership before archive'
Add-Test 'existing-install-cannot-be-claimed' ($runnerText.Contains('$freshInstall = -not') -and $runnerText.Contains('if ($freshInstall)') -and $runnerText.Contains('Assert-OwnedIsolatedRoot -Context $Context -Root $entry.root')) 'an existing isolated install must already carry this run owner marker'
Add-Test 'uninstall-failure-blocks-cleanup-pass' ($runnerText.Contains('Exact isolated uninstall exited with code') -and $runnerText.Contains('Exact isolated uninstall left its registry key') -and $runnerText.Contains('$errors += @($uninstallAction.errors)')) 'uninstall failures must reach the group cleanup verdict'
Add-Test 'failed-host-group-cleans-only-owned-run' ($runnerText.Contains('$failedOwnedHostRun') -and $runnerText.Contains('isolated_roots_owned') -and $runnerText.Contains('cleanup refused to remove it')) 'failed host groups must uninstall owned test data and refuse an unowned partial footprint'
Add-Test 'sandbox-path-is-native-quoted' ($launcherText.Contains('ConvertTo-NativeArgument -Value $wsbPath')) 'Windows Sandbox config paths with spaces must be quoted as one native argument'
Add-Test 'sandbox-uninstall-is-verified' ($workerText.Contains('Sandbox uninstaller failed with code') -and $workerText.Contains('install_root_removed') -and $workerText.Contains('uninstall_registry_removed')) 'Sandbox cleanup must fail when exact uninstall leaves files, registry identity or a nonzero exit code'

$head = (& git -C $repoRoot rev-parse HEAD).Trim().ToLowerInvariant()
$fixtureRoot = Join-Path $OutputRoot 'fixture'
$publicRoot = Join-Path $fixtureRoot 'public'
$privateRoot = Join-Path $fixtureRoot 'private'
[System.IO.Directory]::CreateDirectory($publicRoot) | Out-Null
[System.IO.Directory]::CreateDirectory($privateRoot) | Out-Null
$candidatePath = Join-Path $fixtureRoot 'candidate.exe'
Write-Utf8 $candidatePath 'not-an-installer-contract-fixture'
$candidate = File-Record $candidatePath
$inputPaths = 1..10 | ForEach-Object {
    $path = Join-Path $fixtureRoot ("input-$_.bin")
    Write-Utf8 $path ("bound-input-$_")
    $path
}
$installed = @(
    @{ role = 'main_executable'; relative_path = 'meetily.exe' },
    @{ role = 'llama_helper'; relative_path = 'llama-helper.exe' },
    @{ role = 'moss_helper'; relative_path = 'moss-helper.exe' },
    @{ role = 'ffmpeg'; relative_path = 'ffmpeg.exe' },
    @{ role = 'directml'; relative_path = 'DirectML.dll' },
    @{ role = 'webview2'; relative_path = 'runtime/webview2-fixed/msedgewebview2.exe' },
    @{ role = 'uninstaller'; relative_path = 'uninstall.exe' }
) | ForEach-Object { [ordered]@{ role = $_.role; relative_path = $_.relative_path; bytes = 1; sha256 = 'A' * 64 } }
$buildPath = Join-Path $fixtureRoot 'build-manifest.json'
$build = [ordered]@{
    schema_version = 1; role = 'candidate'; source_commit = $head; product_name = 'meetily-p6-lifecycle'
    bundle_id = 'com.meetily.ai.p6lifecycle'; version = '0.4.2'; installer = $candidate; installed_files = $installed
}
Write-Json $buildPath $build
$buildRecord = File-Record $buildPath
$valuesPath = Join-Path $fixtureRoot 'producer-values.json'
Write-Json $valuesPath ([ordered]@{ schema_version = 1; stage = 'CONTRACT_VALUES' })
$pythonRecord = File-Record $windowsPowerShell
$dummyModelRecord = File-Record $inputPaths[0]

$runMap = @{}
1..3 | ForEach-Object { $runMap['FT-{0:D2}' -f $_] = 'ft01-03-live-and-persistence' }
4..15 | ForEach-Object { $runMap['FT-{0:D2}' -f $_] = 'ft04-15-moss-chain' }
16..21 | ForEach-Object { $runMap['FT-{0:D2}' -f $_] = 'ft16-21-fault-chain' }
22..25 | ForEach-Object { $runMap['FT-{0:D2}' -f $_] = 'ft22-25-install-lifecycle' }
$runMap['FT-26'] = 'ft26-data-drive-placement'; $runMap['FT-27'] = 'ft27-long-audio'; $runMap['FT-28'] = 'ft28-business-chain'

function New-Config {
    param([string]$Path, [scriptblock]$Mutate)
    $freshCandidate = File-Record $candidatePath
    $freshBuildRecord = File-Record $buildPath
    $scenarios = @(
        foreach ($number in 1..28) {
            $id = 'FT-{0:D2}' -f $number
            $inputIndex = if ($number -eq 27) { 5 } elseif ($number -eq 28) { 6 } elseif ($number -ge 22 -and $number -le 25) { 2 } else { 0 }
            [ordered]@{
                id = $id; name = "contract-$id"; run_once_key = $runMap[$id]; source_commit = $head; candidate_sha256 = $freshCandidate.sha256
                inputs = @((File-Record $inputPaths[$inputIndex]))
                evidence = [ordered]@{
                    public = @("$id/result.public.json", "cleanup/$($runMap[$id]).public.json")
                    private = @("$id/result.private.json", "cleanup/$($runMap[$id]).private.json")
                }
            }
        }
    )
    $document = [ordered]@{
        schema_version = 1; stage = 'MOSS_FUNCTIONAL_FIX_ACCEPTANCE_CONFIG'; template_only = $false
        run_id = 'FIX-CONTRACT-20260902-01'; source_commit = $head; candidate = $freshCandidate; build_manifest = $freshBuildRecord
        evidence_roots = [ordered]@{ public = $publicRoot; private = $privateRoot }
        scenarios = $scenarios
    }
    if ($null -ne $Mutate) { & $Mutate $document }
    Write-Json $Path $document
    return $document
}

function Write-Sidecar {
    param([string]$ConfigPath, $Document, [scriptblock]$Mutate)
    $configRecord = File-Record $ConfigPath
    $groups = [ordered]@{}
    foreach ($key in $expectedMap.Keys) { $groups[$key] = [ordered]@{} }
    $sidecar = [ordered]@{
        schema_version = 1; stage = 'MOSS_FUNCTIONAL_FT_PRODUCER_INPUT'; run_id = $Document.run_id
        source_commit = $Document.source_commit; candidate_sha256 = (File-Record $candidatePath).sha256; build_manifest_sha256 = (File-Record $buildPath).sha256
        acceptance_sha256 = $configRecord.sha256; product_name = 'meetily-p6-lifecycle'; bundle_id = 'com.meetily.ai.p6lifecycle'
        version = '0.4.2'; created_at = [datetimeoffset]::UtcNow.ToString('o')
        producer = File-Record $producerInputGenerator; values = File-Record $valuesPath; python = $pythonRecord
        models = [ordered]@{ moss = $dummyModelRecord; whisper = $dummyModelRecord; qwen_2b = $dummyModelRecord }
        moss_runtime = [ordered]@{ root = $fixtureRoot; contract = $dummyModelRecord; files = @() }
        groups = $groups
    }
    if ($null -ne $Mutate) { & $Mutate $sidecar }
    $sidecarPath = [System.IO.Path]::ChangeExtension($ConfigPath, '.producer.private.json')
    Write-Json $sidecarPath $sidecar
    return $sidecarPath
}

function Invoke-Reject {
    param([string]$Name, [string]$ConfigPath, [string]$ExpectedText)
    $previousPreference = $ErrorActionPreference
    $ErrorActionPreference = 'Continue'
    try {
        $output = & $windowsPowerShell -NoProfile -NonInteractive -ExecutionPolicy Bypass -File $runner `
            -Mode Run -ProducerKey ft01-03-live-and-persistence -AcceptanceConfig $ConfigPath 2>&1 | Out-String
        $code = $LASTEXITCODE
    } finally {
        $ErrorActionPreference = $previousPreference
    }
    $compactOutput = [regex]::Replace($output, '\s+', ' ')
    $containsExpected = $compactOutput.IndexOf($ExpectedText, [System.StringComparison]::OrdinalIgnoreCase) -ge 0
    Add-Test $Name ($code -ne 0 -and $containsExpected) ("exit=$code output=" + $compactOutput.Trim())
}

$missingSidecarPath = Join-Path $fixtureRoot 'missing-sidecar.json'
$missingSidecar = New-Config $missingSidecarPath $null
Invoke-Reject 'reject-missing-sidecar' $missingSidecarPath 'producer private input'

$oldRunPath = Join-Path $fixtureRoot 'old-run.json'
$oldRun = New-Config $oldRunPath { param($d) $d.run_id = 'FT-1B9C29A-20260902-01' }
Invoke-Reject 'reject-old-run-id' $oldRunPath 'run_id is obsolete or unsafe'

$placeholderPath = Join-Path $fixtureRoot 'placeholder.json'
$placeholder = New-Config $placeholderPath { param($d) $d.scenarios[0].name = '__TODO_VALUE__' }
Invoke-Reject 'reject-placeholder' $placeholderPath 'placeholder or obsolete'

$wrongCommitPath = Join-Path $fixtureRoot 'wrong-commit.json'
$wrongCommit = New-Config $wrongCommitPath { param($d) $d.source_commit = '0' * 40 }
Invoke-Reject 'reject-wrong-source-commit' $wrongCommitPath 'does not match repository HEAD'

$wrongCandidatePath = Join-Path $fixtureRoot 'wrong-candidate.json'
$wrongCandidate = New-Config $wrongCandidatePath { param($d) $d.candidate.sha256 = 'B' * 64 }
Invoke-Reject 'reject-wrong-candidate-hash' $wrongCandidatePath 'bytes or SHA-256 do not match'

$wrongManifestPath = Join-Path $fixtureRoot 'wrong-manifest-hash.json'
$wrongManifest = New-Config $wrongManifestPath { param($d) $d.build_manifest.sha256 = 'C' * 64 }
Invoke-Reject 'reject-wrong-build-manifest-hash' $wrongManifestPath 'bytes or SHA-256 do not match'

$wrongInputPath = Join-Path $fixtureRoot 'wrong-input.json'
$wrongInput = New-Config $wrongInputPath { param($d) $d.scenarios[0].inputs[0].sha256 = 'D' * 64 }
Invoke-Reject 'reject-wrong-input-hash' $wrongInputPath 'FT-01 input bytes or SHA-256'

$escapePath = Join-Path $fixtureRoot 'escape.json'
$escape = New-Config $escapePath { param($d) $d.scenarios[0].evidence.public[0] = '../escaped.json' }
Invoke-Reject 'reject-evidence-path-escape' $escapePath 'unsafe segment'

$nestedPath = Join-Path $fixtureRoot 'nested-roots.json'
$nested = New-Config $nestedPath { param($d) $d.evidence_roots.private = Join-Path $d.evidence_roots.public 'nested'; [System.IO.Directory]::CreateDirectory($d.evidence_roots.private) | Out-Null }
Invoke-Reject 'reject-nested-evidence-roots' $nestedPath 'separate and non-nested'

$badMapPath = Join-Path $fixtureRoot 'bad-map.json'
$badMap = New-Config $badMapPath { param($d) $d.scenarios[0].run_once_key = 'ft04-15-moss-chain' }
Invoke-Reject 'reject-wrong-ft-group-map' $badMapPath 'group mapping is wrong'

$validPath = Join-Path $fixtureRoot 'valid-preflight.json'
$valid = New-Config $validPath $null
$validSidecar = Write-Sidecar $validPath $valid $null
$existingEvidence = Join-Path $publicRoot 'FT-01\result.public.json'
Write-Utf8 $existingEvidence '{}'
Invoke-Reject 'reject-existing-formal-evidence' $validPath 'Formal evidence already exists'

$badBindingPath = Join-Path $fixtureRoot 'bad-sidecar-binding.json'
$badBinding = New-Config $badBindingPath $null
$badBindingSidecar = Write-Sidecar $badBindingPath $badBinding { param($s) $s.acceptance_sha256 = 'E' * 64 }
Invoke-Reject 'reject-sidecar-binding' $badBindingPath 'bound to another acceptance'

$missingGroupPath = Join-Path $fixtureRoot 'missing-sidecar-group.json'
$missingGroup = New-Config $missingGroupPath $null
$missingGroupSidecar = Write-Sidecar $missingGroupPath $missingGroup { param($s) [void]$s.groups.Remove('ft01-03-live-and-persistence') }
Invoke-Reject 'reject-missing-sidecar-group' $missingGroupPath 'fields are not exact'

$cleanupPublicRoot = Join-Path $fixtureRoot 'cleanup-public'
$cleanupPrivateRoot = Join-Path $fixtureRoot 'cleanup-private'
[System.IO.Directory]::CreateDirectory($cleanupPublicRoot) | Out-Null
[System.IO.Directory]::CreateDirectory($cleanupPrivateRoot) | Out-Null
$cleanupConfigPath = Join-Path $fixtureRoot 'cleanup-without-run.json'
$cleanupConfig = New-Config $cleanupConfigPath {
    param($d)
    $d.evidence_roots.public = $cleanupPublicRoot
    $d.evidence_roots.private = $cleanupPrivateRoot
}
$cleanupSidecar = Write-Sidecar $cleanupConfigPath $cleanupConfig $null
$previousPreference = $ErrorActionPreference
$ErrorActionPreference = 'Continue'
try {
    $cleanupOutput = & $windowsPowerShell -NoProfile -NonInteractive -ExecutionPolicy Bypass -File $runner `
        -Mode Cleanup -ProducerKey ft16-21-fault-chain -AcceptanceConfig $cleanupConfigPath 2>&1 | Out-String
    $cleanupExit = $LASTEXITCODE
} finally {
    $ErrorActionPreference = $previousPreference
}
$cleanupPublicPath = Join-Path $cleanupPublicRoot 'cleanup\ft16-21-fault-chain.public.json'
$cleanupDocument = if (Test-Path -LiteralPath $cleanupPublicPath -PathType Leaf) {
    Get-Content -LiteralPath $cleanupPublicPath -Raw -Encoding UTF8 | ConvertFrom-Json
} else { $null }
Add-Test 'cleanup-skipped-group-without-state' ($cleanupExit -eq 0 -and $null -ne $cleanupDocument -and
    [string]$cleanupDocument.status -eq 'PASS' -and [int]$cleanupDocument.residual_process_count -eq 0 -and
    [bool]$cleanupDocument.producer_state_existed -eq $false) ("exit=$cleanupExit output=" + $cleanupOutput.Trim())

$report = [ordered]@{
    schema_version = 1
    stage = 'MOSS_FUNCTIONAL_FT_CONTRACT_TEST'
    generated_at = [datetimeoffset]::UtcNow.ToString('o')
    runner_sha256 = (Get-FileHash -LiteralPath $runner -Algorithm SHA256).Hash
    cdp_sha256 = (Get-FileHash -LiteralPath $cdp -Algorithm SHA256).Hash
    producer_input_generator_sha256 = (Get-FileHash -LiteralPath $producerInputGenerator -Algorithm SHA256).Hash
    sandbox_launcher_sha256 = (Get-FileHash -LiteralPath $sandboxLauncher -Algorithm SHA256).Hash
    sandbox_worker_sha256 = (Get-FileHash -LiteralPath $sandboxWorker -Algorithm SHA256).Hash
    total = $tests.Count
    passed = @($tests | Where-Object { $_.passed }).Count
    failed = @($tests | Where-Object { -not $_.passed }).Count
    tests = $tests
}
$reportPath = Join-Path $OutputRoot 'moss-functional-ft-contract-tests.json'
Write-Json $reportPath $report
$report | ConvertTo-Json -Depth 12
if ($report.failed -ne 0) { exit 1 }
exit 0

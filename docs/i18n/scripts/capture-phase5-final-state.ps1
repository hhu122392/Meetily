[CmdletBinding()]
param()

$ErrorActionPreference = 'Stop'
$repoRoot = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..\..\..'))
$formalExe = Join-Path $repoRoot 'target\release\meetily.exe'
$formalInstaller = Join-Path $repoRoot 'target\release\bundle\nsis\meetily_0.4.0_x64-setup.exe'
$releaseGatePath = Join-Path $repoRoot 'docs\i18n\audit\phase-5-release\release-gate.json'
$deliveryIntegrityPath = Join-Path $repoRoot 'target\release\docs\i18n\audit\phase-5-release\delivery-integrity.json'
$outputPath = Join-Path $repoRoot 'docs\i18n\audit\phase-5-release\final-state.json'

$formalExeHash = (Get-FileHash -LiteralPath $formalExe -Algorithm SHA256).Hash
$formalInstallerHash = (Get-FileHash -LiteralPath $formalInstaller -Algorithm SHA256).Hash
$releaseGate = Get-Content -LiteralPath $releaseGatePath -Raw | ConvertFrom-Json
$deliveryIntegrity = Get-Content -LiteralPath $deliveryIntegrityPath -Raw | ConvertFrom-Json
$phase5Processes = @(
  Get-CimInstance Win32_Process -Filter "Name='meetily.exe'" |
    Where-Object {
      $_.ExecutablePath -like '*target-phase5-release*' -or
      $_.ExecutablePath -like '*Meetily Phase 5 Audit*'
    }
)
$phase5Registry = @(
  Get-ItemProperty -Path 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\*' -ErrorAction SilentlyContinue |
    Where-Object { $_.DisplayName -eq 'Meetily Phase 5 Audit' }
)

$assertions = [ordered]@{
  formalExecutableFrozen = ($formalExeHash -eq '1873D2CCA9B06EEBE8B2D42DC8530B6888BCB91C4267BBD6E6724F6A78257823')
  formalInstallerFrozen = ($formalInstallerHash -eq 'C6A5BD12F947AEDECAC890C33A48FDEA94E4C11C29FCA03EEF753C25AEE30434')
  noPhase5Process = ($phase5Processes.Count -eq 0)
  noPhase5RegistryEntry = ($phase5Registry.Count -eq 0)
  noPhase5InstallDirectory = (-not (Test-Path -LiteralPath (Join-Path $env:LOCALAPPDATA 'Meetily Phase 5 Audit')))
  noPhase5AppData = (-not (Test-Path -LiteralPath (Join-Path $env:APPDATA 'com.meetily.ai.phase5audit')))
  deliveryIntegrityPassed = [bool]$deliveryIntegrity.passed
  technicalSuitePassed = [bool]$releaseGate.technicalSuitePassed
  formalReleaseCorrectlyNotAuthorized = (-not [bool]$releaseGate.formalReleaseAuthorized)
}

$report = [ordered]@{
  schemaVersion = 1
  generatedAt = (Get-Date).ToUniversalTime().ToString('o')
  formalExecutableSha256 = $formalExeHash
  formalInstallerSha256 = $formalInstallerHash
  releaseGateVerdict = $releaseGate.verdict
  formalReleaseAuthorized = [bool]$releaseGate.formalReleaseAuthorized
  assertions = $assertions
  passed = (-not ($assertions.Values -contains $false))
}

[System.IO.File]::WriteAllText(
  $outputPath,
  ($report | ConvertTo-Json -Depth 6),
  [System.Text.UTF8Encoding]::new($false)
)
$report | ConvertTo-Json -Depth 6
if (-not $report.passed) {
  exit 1
}

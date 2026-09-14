[CmdletBinding()]
param([string]$TargetDirectory = '')
$ErrorActionPreference = 'Stop'
$releaseRoot = Split-Path -Parent $PSScriptRoot
if (-not [Environment]::Is64BitOperatingSystem) { throw 'Windows x64 is required' }
if (-not $TargetDirectory) { $TargetDirectory = Join-Path $releaseRoot 'target' }
$env:CARGO_TARGET_DIR = [IO.Path]::GetFullPath($TargetDirectory)
$env:CL = '/utf-8'
$env:CFLAGS = '/utf-8'
$env:CXXFLAGS = '/utf-8'
$env:CMAKE_POLICY_VERSION_MINIMUM = '3.5'
Push-Location (Join-Path $releaseRoot 'frontend')
try {
  & corepack pnpm install --frozen-lockfile
  if ($LASTEXITCODE) { throw 'Frontend dependency installation failed' }
  & (Join-Path $PSScriptRoot 'prepare-tauri-sidecars.ps1')
  if ($LASTEXITCODE) { throw 'Sidecar preparation failed' }
  & node node_modules/@tauri-apps/cli/tauri.js build --bundles nsis --ci
  if ($LASTEXITCODE) { throw 'Windows installer build failed' }
  Write-Host "Unsigned community installer: $env:CARGO_TARGET_DIR\release\bundle\nsis"
} finally { Pop-Location }

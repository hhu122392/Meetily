[CmdletBinding()]
param(
    [string]$TargetDirectory = "",
    [string]$CargoExecutable = ""
)

$ErrorActionPreference = "Stop"
$workspace = Split-Path -Parent $PSScriptRoot
& (Join-Path $PSScriptRoot "prepare-webview2-fixed.ps1")
if ([string]::IsNullOrWhiteSpace($TargetDirectory)) {
    $TargetDirectory = Join-Path $workspace "target\tauri-sidecars"
}
if ([string]::IsNullOrWhiteSpace($CargoExecutable)) {
    $CargoExecutable = (Get-Command cargo -ErrorAction Stop).Source
}
$TargetDirectory = [System.IO.Path]::GetFullPath($TargetDirectory)
$binaries = Join-Path $workspace "frontend\src-tauri\binaries"
$triple = "x86_64-pc-windows-msvc"

$vswhere = Join-Path ${env:ProgramFiles(x86)} "Microsoft Visual Studio\Installer\vswhere.exe"
$visualStudio = $null
if (Test-Path -LiteralPath $vswhere -PathType Leaf) {
    $visualStudio = & $vswhere -latest -products * `
        -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath
    if ($LASTEXITCODE -ne 0) {
        throw "Visual Studio toolchain discovery failed"
    }
}
$visualStudio = $visualStudio | Select-Object -First 1
if (-not [string]::IsNullOrWhiteSpace($visualStudio)) {
    $devCommand = Join-Path $visualStudio "Common7\Tools\VsDevCmd.bat"
    if (-not (Test-Path -LiteralPath $devCommand -PathType Leaf)) {
        throw "Visual Studio developer environment is incomplete"
    }
    $environmentLines = & "$env:SystemRoot\System32\cmd.exe" /d /s /c `
        "`"$devCommand`" -no_logo -arch=x64 -host_arch=x64 >nul && set"
    if ($LASTEXITCODE -ne 0) {
        throw "Visual Studio developer environment setup failed"
    }
    foreach ($line in $environmentLines) {
        $separator = $line.IndexOf("=")
        if ($separator -gt 0) {
            [Environment]::SetEnvironmentVariable(
                $line.Substring(0, $separator),
                $line.Substring($separator + 1),
                "Process"
            )
        }
    }
} else {
    $standaloneTools = "C:\BuildTools"
    $versionFile = Join-Path $standaloneTools "VC\Auxiliary\Build\Microsoft.VCToolsVersion.default.txt"
    if (-not (Test-Path -LiteralPath $versionFile -PathType Leaf)) {
        throw "Visual Studio 2022 C++ Build Tools are required to build release sidecars"
    }
    $toolsVersion = (Get-Content -LiteralPath $versionFile -Raw).Trim()
    $msvc = Join-Path $standaloneTools "VC\Tools\MSVC\$toolsVersion"
    $windowsKits = "C:\Program Files (x86)\Windows Kits\10"
    $sdkVersion = Get-ChildItem -LiteralPath (Join-Path $windowsKits "Lib") -Directory |
        Where-Object { Test-Path -LiteralPath (Join-Path $_.FullName "um\x64\kernel32.lib") } |
        Sort-Object Name -Descending |
        Select-Object -First 1 -ExpandProperty Name
    if ([string]::IsNullOrWhiteSpace($sdkVersion) -or
        -not (Test-Path -LiteralPath (Join-Path $msvc "bin\Hostx64\x64\cl.exe") -PathType Leaf)) {
        throw "A complete MSVC and Windows SDK toolchain is required to build release sidecars"
    }
    $env:PATH = "$(Join-Path $msvc 'bin\Hostx64\x64');$(Join-Path $windowsKits "bin\$sdkVersion\x64");$env:PATH"
    $env:INCLUDE = @(
        (Join-Path $msvc "include"),
        (Join-Path $windowsKits "Include\$sdkVersion\ucrt"),
        (Join-Path $windowsKits "Include\$sdkVersion\shared"),
        (Join-Path $windowsKits "Include\$sdkVersion\um"),
        (Join-Path $windowsKits "Include\$sdkVersion\winrt"),
        (Join-Path $windowsKits "Include\$sdkVersion\cppwinrt")
    ) -join ";"
    $env:LIB = @(
        (Join-Path $msvc "lib\x64"),
        (Join-Path $windowsKits "Lib\$sdkVersion\ucrt\x64"),
        (Join-Path $windowsKits "Lib\$sdkVersion\um\x64")
    ) -join ";"
    $env:CMAKE_GENERATOR = "NMake Makefiles"
}

if ([string]::IsNullOrWhiteSpace($env:LIBCLANG_PATH)) {
    $bundledLibclang = Join-Path $env:LOCALAPPDATA `
        "MeetilyBuildTools\llvm-x64-bindgen\Contents\VC\Tools\Llvm\x64\bin"
    if (-not (Test-Path -LiteralPath (Join-Path $bundledLibclang "libclang.dll") -PathType Leaf)) {
        throw "libclang.dll is required to build the release llama helper"
    }
    $env:LIBCLANG_PATH = $bundledLibclang
}

& $CargoExecutable build --locked --release --target $triple --target-dir $TargetDirectory `
    --package llama-helper --package moss-helper
if ($LASTEXITCODE -ne 0) {
    throw "release helper build failed with exit code $LASTEXITCODE"
}

New-Item -ItemType Directory -Force -Path $binaries | Out-Null
foreach ($name in @("llama-helper", "moss-helper")) {
    $source = Join-Path $TargetDirectory "$triple\release\$name.exe"
    $destination = Join-Path $binaries "$name-$triple.exe"
    if (-not (Test-Path -LiteralPath $source -PathType Leaf)) {
        throw "built helper is missing: $name"
    }
    $bytes = [System.IO.File]::ReadAllBytes($source)
    if ($bytes.Length -lt 2 -or $bytes[0] -ne 0x4D -or $bytes[1] -ne 0x5A) {
        throw "built helper is not a Windows PE executable: $name"
    }
    $temporary = "$destination.$PID.tmp"
    [System.IO.File]::WriteAllBytes($temporary, $bytes)
    Move-Item -LiteralPath $temporary -Destination $destination -Force
}

Write-Host "Prepared release Tauri sidecars for $triple."

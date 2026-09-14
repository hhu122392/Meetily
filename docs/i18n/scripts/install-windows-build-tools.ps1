#Requires -Version 5.1
#Requires -RunAsAdministrator

[CmdletBinding()]
param(
    [string]$InstallPath = "D:\MeetilyBuildTools\VisualStudio2022",
    [string]$SharedPath = "D:\MeetilyBuildTools\VisualStudioShared"
)

$ErrorActionPreference = "Stop"

$vswhere = "C:\Program Files (x86)\Microsoft Visual Studio\Installer\vswhere.exe"
$bootstrapperUri = "https://aka.ms/vs/17/release/vs_BuildTools.exe"
$bootstrapperPath = Join-Path $env:TEMP "meetily-vs_BuildTools.exe"
$vcComponent = "Microsoft.VisualStudio.Component.VC.Tools.x86.x64"
$clangComponentGroup = "Microsoft.VisualStudio.ComponentGroup.NativeDesktop.Llvm.Clang"

function Assert-MicrosoftSignature {
    param([Parameter(Mandatory)][string]$Path)

    $signature = Get-AuthenticodeSignature -LiteralPath $Path
    if ($signature.Status -ne "Valid" -or $signature.SignerCertificate.Subject -notmatch "Microsoft") {
        throw "Microsoft signature validation failed for ${Path}: $($signature.Status) / $($signature.SignerCertificate.Subject)"
    }

    Write-Host "Verified Microsoft signature: $($signature.SignerCertificate.Subject)"
}

$existingInstallation = $null
if (Test-Path -LiteralPath $vswhere) {
    $existingInstallation = & $vswhere `
        -all `
        -products Microsoft.VisualStudio.Product.BuildTools `
        -property installationPath | Select-Object -First 1
}

if ($existingInstallation) {
    $setup = "C:\Program Files (x86)\Microsoft Visual Studio\Installer\setup.exe"
    if (!(Test-Path -LiteralPath $setup)) {
        throw "Visual Studio Installer setup.exe was not found."
    }

    Assert-MicrosoftSignature -Path $setup
    Write-Host "Repairing the existing Build Tools instance at $existingInstallation ..."
    Write-Host "Adding the required Visual C++ x86/x64 and x64 Clang components."

    $arguments = @(
        "modify",
        "--quiet",
        "--norestart",
        "--nocache",
        "--installPath", $existingInstallation,
        "--add", $vcComponent,
        "--add", $clangComponentGroup
    )

    $installer = Start-Process `
        -FilePath $setup `
        -ArgumentList $arguments `
        -Wait `
        -PassThru `
        -WindowStyle Hidden
} else {
    Write-Host "Downloading the Microsoft Visual Studio 2022 Build Tools bootstrapper..."
    Invoke-WebRequest -Uri $bootstrapperUri -OutFile $bootstrapperPath
    Assert-MicrosoftSignature -Path $bootstrapperPath
    Write-Host "Installing the C++ workload to $InstallPath ..."

    $arguments = @(
        "--quiet",
        "--wait",
        "--norestart",
        "--nocache",
        "--installPath", $InstallPath,
        "--path", "shared=$SharedPath",
        "--add", "Microsoft.VisualStudio.Workload.VCTools",
        "--add", $clangComponentGroup,
        "--includeRecommended"
    )

    $installer = Start-Process `
        -FilePath $bootstrapperPath `
        -ArgumentList $arguments `
        -Wait `
        -PassThru `
        -WindowStyle Hidden
}

if ($installer.ExitCode -notin @(0, 3010)) {
    throw "Visual Studio Build Tools installation or repair failed with exit code $($installer.ExitCode)."
}

$installation = & $vswhere `
    -all `
    -latest `
    -products Microsoft.VisualStudio.Product.BuildTools `
    -requires $vcComponent `
    -property installationPath

if (!$installation) {
    throw "The Visual C++ x86/x64 component could not be verified."
}

Write-Host "Visual C++ Build Tools verified at: $installation"
$x64LibClang = Join-Path $installation "VC\Tools\Llvm\x64\bin\libclang.dll"
if (!(Test-Path -LiteralPath $x64LibClang)) {
    throw "The x64 libclang.dll could not be verified at $x64LibClang."
}

Write-Host "x64 libclang verified at: $x64LibClang"
if ($installer.ExitCode -eq 3010) {
    Write-Warning "Installation succeeded and Windows requested a reboot before building."
}

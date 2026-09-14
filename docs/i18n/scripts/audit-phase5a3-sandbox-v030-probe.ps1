[CmdletBinding()]
param()

$ErrorActionPreference = 'Stop'
$mappedInstaller = 'C:\MeetilyInput\meetily_0.3.0_x64-setup.exe'
$installer = 'C:\MeetilyAudit\meetily_0.3.0_x64-setup.exe'
$reportPath = 'C:\MeetilyOutput\official-v030-probe.json'
$expectedHash = '900C10A4EA05D991A06AB670886DE45DE08904903298A862A94FDC66F25420D9'
$productName = 'meetily'
$bundleId = 'com.meetily.ai'
$launchedProcess = $null
$uninstaller = $null

function Write-ProbeStage {
  param([Parameter(Mandatory = $true)][string]$Stage)
  New-Item -ItemType Directory -Path (Split-Path -Parent $reportPath) -Force | Out-Null
  [System.IO.File]::WriteAllText(
    'C:\MeetilyOutput\probe-stage.txt',
    "$((Get-Date).ToUniversalTime().ToString('o'))`t$Stage",
    [System.Text.UTF8Encoding]::new($false))
}

function Get-MeetilyRegistryEntries {
  @(
    'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\*',
    'HKLM:\Software\Microsoft\Windows\CurrentVersion\Uninstall\*',
    'HKLM:\Software\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall\*'
  ) | ForEach-Object {
    Get-ItemProperty -Path $_ -ErrorAction SilentlyContinue
  } | Where-Object { $_.DisplayName -match '(?i)meetily' }
}

function Get-MeetilyRegistryEntry {
  Get-MeetilyRegistryEntries | Select-Object -First 1
}

function Get-ArtifactRecord {
  param([Parameter(Mandatory = $true)][string]$Path)
  $item = Get-Item -LiteralPath $Path
  [ordered]@{
    path = $item.FullName
    bytes = $item.Length
    sha256 = (Get-FileHash -LiteralPath $item.FullName -Algorithm SHA256).Hash
    productName = $item.VersionInfo.ProductName
    productVersion = $item.VersionInfo.ProductVersion
    fileVersion = $item.VersionInfo.FileVersion
    signatureStatus = (Get-AuthenticodeSignature -LiteralPath $item.FullName).Status.ToString()
  }
}

$result = [ordered]@{
  schemaVersion = 1
  phase = '15.11 / Stage 5A-3'
  generatedAt = (Get-Date).ToUniversalTime().ToString('o')
  scope = 'Disposable Windows Sandbox identity and launch probe for the official Meetily v0.3.0 NSIS asset'
  hostDataRead = $false
  sandboxNetworking = 'Enabled only for the legacy WebView2 online bootstrapper; no host user-data folders are mapped'
  officialTag = 'v0.3.0'
  officialTagCommit = '91b0c0985932d0797e249033601afa14f22ee3d3'
  artifacts = [ordered]@{}
  observations = [ordered]@{}
  assertions = [ordered]@{}
  error = $null
  passed = $false
}

try {
  Write-ProbeStage -Stage 'preflight'
  if (-not (Test-Path -LiteralPath $mappedInstaller -PathType Leaf)) {
    throw "Official installer is missing: $mappedInstaller"
  }

  $result.artifacts.mappedInstaller = Get-ArtifactRecord -Path $mappedInstaller
  New-Item -ItemType Directory -Path (Split-Path -Parent $installer) -Force | Out-Null
  Copy-Item -LiteralPath $mappedInstaller -Destination $installer
  $result.artifacts.installer = Get-ArtifactRecord -Path $installer
  $result.assertions.officialDigestMatches = (
    $result.artifacts.mappedInstaller.sha256 -eq $expectedHash -and
    $result.artifacts.installer.sha256 -eq $expectedHash)
  $result.assertions.officialInstallerAuthenticodeValid = (
    $result.artifacts.mappedInstaller.signatureStatus -eq 'Valid' -and
    $result.artifacts.installer.signatureStatus -eq 'Valid')
  $result.assertions.cleanRegistryBeforeInstall = ($null -eq (Get-MeetilyRegistryEntry))
  if ($result.assertions.Values -contains $false) {
    throw 'Official v0.3.0 preflight failed.'
  }

  $result.observations.webView2BeforeInstall = @(
    @(
      'HKLM:\Software\Microsoft\EdgeUpdate\Clients\*',
      'HKLM:\Software\WOW6432Node\Microsoft\EdgeUpdate\Clients\*'
    ) | ForEach-Object {
      Get-ItemProperty -Path $_ -ErrorAction SilentlyContinue
    } | Where-Object { $_.name -match '(?i)webview' } | ForEach-Object {
      [ordered]@{
        name = $_.name
        version = $_.pv
        registryPath = $_.PSPath
      }
    })
  Write-ProbeStage -Stage 'installer-started'
  $installProcess = Start-Process -FilePath $installer -ArgumentList @('/S') -PassThru -WindowStyle Hidden
  $installCompleted = $installProcess.WaitForExit(120000)
  if (-not $installCompleted) {
    Stop-Process -Id $installProcess.Id -Force -ErrorAction SilentlyContinue
    $result.observations.installTimeout = [ordered]@{
      timeoutSeconds = 120
      installerProcessId = $installProcess.Id
      webView2BeforeInstall = $result.observations.webView2BeforeInstall
    }
    throw 'Official v0.3.0 installer exceeded the 120-second Sandbox timeout.'
  }
  Write-ProbeStage -Stage 'installer-exited'
  Start-Sleep -Seconds 3
  $entry = Get-MeetilyRegistryEntry
  if ($null -eq $entry) {
    $result.observations.installDiscovery = [ordered]@{
      exitCode = $installProcess.ExitCode
      matchingRegistryEntries = @(
        Get-MeetilyRegistryEntries | ForEach-Object {
          [ordered]@{
            displayName = $_.DisplayName
            displayVersion = $_.DisplayVersion
            installLocation = $_.InstallLocation
            registryPath = $_.PSPath
          }
        })
      candidateDirectories = @(
        (Join-Path $env:LOCALAPPDATA 'meetily'),
        (Join-Path $env:ProgramFiles 'meetily'),
        (Join-Path ${env:ProgramFiles(x86)} 'meetily')
      ) | ForEach-Object {
        [ordered]@{
          path = $_
          exists = (Test-Path -LiteralPath $_)
        }
      }
    }
    throw 'Official v0.3.0 did not create the expected uninstall entry.'
  }

  $installDir = [System.IO.Path]::GetFullPath($entry.InstallLocation.Trim('"'))
  $installedExe = Join-Path $installDir 'meetily.exe'
  $uninstaller = [System.IO.Path]::GetFullPath($entry.UninstallString.Trim('"'))
  $result.artifacts.installedExecutable = Get-ArtifactRecord -Path $installedExe
  $result.observations.install = [ordered]@{
    exitCode = $installProcess.ExitCode
    displayName = $entry.DisplayName
    displayVersion = $entry.DisplayVersion
    installLocation = $installDir
    uninstallString = $entry.UninstallString
    publisher = $entry.Publisher
    installedFiles = @(Get-ChildItem -LiteralPath $installDir -File | Select-Object -ExpandProperty Name)
  }
  $result.assertions.installSucceeded = ($installProcess.ExitCode -eq 0)
  $result.assertions.registryIdentityCorrect = (
    $entry.DisplayName -ieq $productName -and $entry.DisplayVersion -eq '0.3.0')
  $result.assertions.installedExecutableVersionCorrect = (
    $result.artifacts.installedExecutable.productVersion -eq '0.3.0')
  $result.assertions.installedExecutableAuthenticodeValid = (
    $result.artifacts.installedExecutable.signatureStatus -eq 'Valid')

  Write-ProbeStage -Stage 'application-launched'
  $launchedProcess = Start-Process -FilePath $installedExe -PassThru -WindowStyle Hidden
  Start-Sleep -Seconds 8
  $alive = -not $launchedProcess.HasExited
  $result.observations.launch = [ordered]@{
    processId = $launchedProcess.Id
    aliveAfterEightSeconds = $alive
    workingSetBytes = if ($alive) { (Get-Process -Id $launchedProcess.Id).WorkingSet64 } else { $null }
  }
  $result.assertions.applicationLaunches = $alive
  if ($alive) {
    Stop-Process -Id $launchedProcess.Id -Force
    $launchedProcess.WaitForExit()
  }
  $launchedProcess = $null

  $appDataPath = Join-Path $env:APPDATA $bundleId
  $recordingPath = Join-Path ([Environment]::GetFolderPath('MyMusic')) 'meetily-recordings'
  $result.observations.dataPaths = [ordered]@{
    appDataPath = $appDataPath
    appDataExistsAfterProbe = (Test-Path -LiteralPath $appDataPath)
    appDataFiles = if (Test-Path -LiteralPath $appDataPath) {
      @(Get-ChildItem -LiteralPath $appDataPath -Recurse -File | ForEach-Object {
          $_.FullName.Substring($appDataPath.Length).TrimStart('\')
        })
    } else { @() }
    defaultRecordingsPath = $recordingPath
    defaultRecordingsPathExists = (Test-Path -LiteralPath $recordingPath)
  }
  $result.assertions.appDataPathMatchesBundleId = (
    $appDataPath -eq (Join-Path $env:APPDATA 'com.meetily.ai'))

  Write-ProbeStage -Stage 'uninstall-started'
  $uninstallProcess = Start-Process -FilePath $uninstaller -ArgumentList @('/S') -PassThru -Wait -WindowStyle Hidden
  Start-Sleep -Seconds 2
  $result.observations.uninstall = [ordered]@{
    exitCode = $uninstallProcess.ExitCode
    registryEntryRemoved = ($null -eq (Get-MeetilyRegistryEntry))
    installDirectoryRemoved = (-not (Test-Path -LiteralPath $installDir))
  }
  $result.assertions.uninstallSucceeded = ($uninstallProcess.ExitCode -eq 0)
  $result.assertions.uninstallRemovedRegistration = $result.observations.uninstall.registryEntryRemoved
  $result.assertions.uninstallRemovedProgramFiles = $result.observations.uninstall.installDirectoryRemoved
  $result.passed = -not ($result.assertions.Values -contains $false)
  Write-ProbeStage -Stage 'probe-complete'
}
catch {
  $result.error = $_.Exception.Message
}
finally {
  if ($null -ne $launchedProcess -and -not $launchedProcess.HasExited) {
    Stop-Process -Id $launchedProcess.Id -Force -ErrorAction SilentlyContinue
  }
  $remainingEntry = Get-MeetilyRegistryEntry
  if ($null -ne $remainingEntry) {
    $remainingUninstaller = [System.IO.Path]::GetFullPath($remainingEntry.UninstallString.Trim('"'))
    if (Test-Path -LiteralPath $remainingUninstaller -PathType Leaf) {
      $null = Start-Process -FilePath $remainingUninstaller -ArgumentList @('/S') -PassThru -Wait -WindowStyle Hidden
    }
  }
  New-Item -ItemType Directory -Path (Split-Path -Parent $reportPath) -Force | Out-Null
  [System.IO.File]::WriteAllText(
    $reportPath,
    ($result | ConvertTo-Json -Depth 8),
    [System.Text.UTF8Encoding]::new($false))
  Start-Sleep -Seconds 2
  Start-Process -FilePath 'shutdown.exe' -ArgumentList @('/s', '/t', '0') -WindowStyle Hidden
}

if (-not $result.passed) { exit 1 }

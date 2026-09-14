[CmdletBinding()]
param()

$ErrorActionPreference = 'Stop'
$officialInstaller = 'C:\MeetilyInput\meetily_0.3.0_x64-setup.exe'
$localInstaller = 'C:\MeetilyAudit\meetily_0.3.0_x64-setup.exe'
$webView2Installer = 'C:\WebView2Input\MicrosoftEdgeWebView2RuntimeInstallerX64.exe'
$seedRoot = 'C:\MeetilyFixture'
$outputRoot = 'C:\MeetilyOutput'
$reportPath = Join-Path $outputRoot 'official-v030-seed-migration.json'
$expectedInstallerHash = '900C10A4EA05D991A06AB670886DE45DE08904903298A862A94FDC66F25420D9'
$expectedWebView2Hash = '82B2D8A7013E0C0EA15D48FF4742EE3778BA16BD8B7B4A47876645B3E48D4016'
$appData = Join-Path $env:APPDATA 'com.meetily.ai'
$recordings = Join-Path $env:USERPROFILE 'Music\meetily-recordings'
$meetilyConfig = Join-Path $env:APPDATA 'meetily'
$appProcess = $null
$uninstaller = $null

function Get-MeetilyRegistryEntry {
  $roots = @(
    'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\*',
    'HKLM:\Software\Microsoft\Windows\CurrentVersion\Uninstall\*',
    'HKLM:\Software\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall\*'
  )
  foreach ($root in $roots) {
    $entry = Get-ItemProperty -Path $root -ErrorAction SilentlyContinue |
      Where-Object { $_.DisplayName -eq 'meetily' } |
      Select-Object -First 1
    if ($null -ne $entry) { return $entry }
  }
  return $null
}

function Invoke-InstallerWithTimeout {
  param(
    [Parameter(Mandatory = $true)][string]$Path,
    [string[]]$Arguments = @('/S'),
    [int]$TimeoutSeconds = 120
  )
  $process = Start-Process -FilePath $Path -ArgumentList $Arguments -PassThru -WindowStyle Hidden
  if (-not $process.WaitForExit($TimeoutSeconds * 1000)) {
    & taskkill.exe /PID $process.Id /T /F 2>$null | Out-Null
    return [ordered]@{ exitCode = $null; timedOut = $true; processId = $process.Id }
  }
  return [ordered]@{ exitCode = $process.ExitCode; timedOut = $false; processId = $process.Id }
}

function Copy-DirectoryContents {
  param(
    [Parameter(Mandatory = $true)][string]$Source,
    [Parameter(Mandatory = $true)][string]$Destination
  )
  New-Item -ItemType Directory -Path $Destination -Force | Out-Null
  Get-ChildItem -LiteralPath $Source -Force | ForEach-Object {
    Copy-Item -LiteralPath $_.FullName -Destination $Destination -Recurse -Force
  }
}

function Get-FileRecord {
  param([Parameter(Mandatory = $true)][string]$Path)
  $item = Get-Item -LiteralPath $Path
  return [ordered]@{
    path = $item.FullName
    bytes = $item.Length
    sha256 = (Get-FileHash -Algorithm SHA256 -LiteralPath $item.FullName).Hash
  }
}

function Get-WebView2RegistryEntries {
  $roots = @(
    'HKCU:\Software\Microsoft\EdgeUpdate\Clients\*',
    'HKLM:\Software\Microsoft\EdgeUpdate\Clients\*',
    'HKLM:\Software\WOW6432Node\Microsoft\EdgeUpdate\Clients\*'
  )
  return @(
    foreach ($root in $roots) {
      Get-ItemProperty -Path $root -ErrorAction SilentlyContinue |
        Where-Object { $_.name -like '*WebView2*' -or $_.DisplayName -like '*WebView2*' } |
        ForEach-Object {
          [ordered]@{
            name = if ($_.name) { $_.name } else { $_.DisplayName }
            version = if ($_.pv) { $_.pv } else { $_.DisplayVersion }
            registryPath = $_.PSPath
          }
        }
    }
  )
}

function Invoke-CdpExpression {
  param(
    [Parameter(Mandatory = $true)][string]$WebSocketUrl,
    [Parameter(Mandatory = $true)][string]$Expression
  )
  $socket = [System.Net.WebSockets.ClientWebSocket]::new()
  $cancellation = [System.Threading.CancellationToken]::None
  try {
    $socket.ConnectAsync([System.Uri]::new($WebSocketUrl), $cancellation).GetAwaiter().GetResult()
    $request = [ordered]@{
      id = 1
      method = 'Runtime.evaluate'
      params = [ordered]@{
        expression = $Expression
        awaitPromise = $true
        returnByValue = $true
      }
    } | ConvertTo-Json -Depth 8 -Compress
    $requestBytes = [System.Text.Encoding]::UTF8.GetBytes($request)
    $requestSegment = [System.ArraySegment[byte]]::new($requestBytes)
    $socket.SendAsync(
      $requestSegment,
      [System.Net.WebSockets.WebSocketMessageType]::Text,
      $true,
      $cancellation).GetAwaiter().GetResult()
    $buffer = New-Object byte[] 65536
    $stream = [System.IO.MemoryStream]::new()
    do {
      $segment = [System.ArraySegment[byte]]::new($buffer)
      $received = $socket.ReceiveAsync($segment, $cancellation).GetAwaiter().GetResult()
      $stream.Write($buffer, 0, $received.Count)
    } while (-not $received.EndOfMessage)
    return [System.Text.Encoding]::UTF8.GetString($stream.ToArray()) | ConvertFrom-Json
  }
  finally {
    if ($socket.State -eq [System.Net.WebSockets.WebSocketState]::Open) {
      $socket.CloseAsync(
        [System.Net.WebSockets.WebSocketCloseStatus]::NormalClosure,
        'audit complete',
        $cancellation).GetAwaiter().GetResult()
    }
    $socket.Dispose()
  }
}

$result = [ordered]@{
  schemaVersion = 1
  phase = '15.11 / Stage 5A-3'
  generatedAt = (Get-Date).ToUniversalTime().ToString('o')
  scope = 'Official signed Meetily v0.3.0 executes its embedded SQLx migrations over a deterministic initial-schema seed in disposable Windows Sandbox'
  hostDataRead = $false
  containsRealUserData = $false
  officialTagCommit = '91b0c0985932d0797e249033601afa14f22ee3d3'
  installer = [ordered]@{}
  webView2 = [ordered]@{}
  observations = [ordered]@{}
  assertions = [ordered]@{}
  error = $null
  passed = $false
}

try {
  New-Item -ItemType Directory -Path 'C:\MeetilyAudit',$outputRoot -Force | Out-Null
  $result.webView2 = Get-FileRecord -Path $webView2Installer
  $result.webView2.signatureStatus = (Get-AuthenticodeSignature -LiteralPath $webView2Installer).Status.ToString()
  $result.assertions.webView2OfflineInstallerHashMatches = ($result.webView2.sha256 -eq $expectedWebView2Hash)
  $result.assertions.webView2OfflineInstallerSignatureValid = ($result.webView2.signatureStatus -eq 'Valid')
  if ($result.assertions.Values -contains $false) {
    throw 'The frozen Microsoft WebView2 offline prerequisite failed preflight.'
  }
  $webViewInstall = Invoke-InstallerWithTimeout -Path $webView2Installer -Arguments @('/silent','/install') -TimeoutSeconds 180
  Start-Sleep -Seconds 5
  $webViewEntries = Get-WebView2RegistryEntries
  $result.observations.webView2Install = [ordered]@{
    process = $webViewInstall
    registryEntries = $webViewEntries
  }
  $result.assertions.webView2OfflineRuntimeInstalled = (
    -not $webViewInstall.timedOut -and
    $webViewInstall.exitCode -eq 0 -and
    $webViewEntries.Count -gt 0)
  if (-not $result.assertions.webView2OfflineRuntimeInstalled) {
    throw 'Microsoft WebView2 offline runtime installation failed.'
  }

  Copy-Item -LiteralPath $officialInstaller -Destination $localInstaller -Force
  $result.installer = Get-FileRecord -Path $localInstaller
  $result.installer.signatureStatus = (Get-AuthenticodeSignature -LiteralPath $localInstaller).Status.ToString()
  $result.assertions.officialInstallerHashMatches = ($result.installer.sha256 -eq $expectedInstallerHash)
  $result.assertions.officialInstallerSignatureValid = ($result.installer.signatureStatus -eq 'Valid')
  $result.assertions.cleanRegistryBeforeInstall = ($null -eq (Get-MeetilyRegistryEntry))
  if ($result.assertions.Values -contains $false) {
    throw 'Official v0.3 bootstrap preflight failed.'
  }

  $install = Invoke-InstallerWithTimeout -Path $localInstaller -Arguments @('/S') -TimeoutSeconds 120
  $result.observations.install = $install
  if ($install.timedOut) { throw 'Official v0.3 installer exceeded 120 seconds.' }
  $entry = Get-MeetilyRegistryEntry
  if ($null -eq $entry) { throw 'Official v0.3 registry entry was not created.' }
  $installDir = [System.IO.Path]::GetFullPath($entry.InstallLocation.Trim('"'))
  $installedExe = Join-Path $installDir 'meetily.exe'
  $uninstaller = [System.IO.Path]::GetFullPath($entry.UninstallString.Trim('"'))
  Copy-Item -LiteralPath $installedExe -Destination (Join-Path $outputRoot 'official-v030-installed.exe') -Force
  $result.observations.registry = [ordered]@{
    displayVersion = $entry.DisplayVersion
    installLocation = $installDir
  }
  $result.assertions.officialV030Installed = (
    $install.exitCode -eq 0 -and
    $entry.DisplayVersion -eq '0.3.0' -and
    (Test-Path -LiteralPath $installedExe -PathType Leaf))
  if (-not $result.assertions.officialV030Installed) {
    throw 'Official v0.3 installed identity did not match expectations.'
  }

  $autoStartedProcesses = @(Get-Process -Name 'meetily' -ErrorAction SilentlyContinue)
  $result.observations.autoStartedProcessesAfterInstall = @(
    $autoStartedProcesses | ForEach-Object {
      [ordered]@{
        processId = $_.Id
        startTime = $_.StartTime.ToUniversalTime().ToString('o')
      }
    }
  )
  foreach ($process in $autoStartedProcesses) {
    & taskkill.exe /PID $process.Id /T /F 2>$null | Out-Null
  }
  if ($autoStartedProcesses.Count -gt 0) { Start-Sleep -Seconds 3 }

  Copy-DirectoryContents -Source (Join-Path $seedRoot 'app-data') -Destination $appData
  Copy-DirectoryContents -Source (Join-Path $seedRoot 'recordings') -Destination $recordings
  Copy-DirectoryContents -Source (Join-Path $seedRoot 'config\meetily') -Destination $meetilyConfig
  $database = Join-Path $appData 'meeting_minutes.sqlite'
  $result.observations.seedDatabase = Get-FileRecord -Path $database

  $env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = '--remote-debugging-port=9358'
  $appProcess = Start-Process -FilePath $installedExe -PassThru -WindowStyle Hidden
  Start-Sleep -Seconds 20
  $appProcess.Refresh()
  $alive = -not $appProcess.HasExited
  $result.observations.launch = [ordered]@{
    processId = $appProcess.Id
    aliveAfterTwentySeconds = $alive
    exitCode = if ($appProcess.HasExited) { $appProcess.ExitCode } else { $null }
  }
  $result.assertions.officialV030StayedRunningWithSeed = $alive
  if ($alive) {
    try {
      $targets = @(Invoke-RestMethod -Uri 'http://127.0.0.1:9358/json' -TimeoutSec 5)
      $pageTarget = $targets | Where-Object { $_.type -eq 'page' } | Select-Object -First 1
      $result.observations.cdpTargets = @($targets | Select-Object id,type,title,url,webSocketDebuggerUrl)
      if ($null -ne $pageTarget) {
        $expression = "(async()=>({databaseDirectory:await window.__TAURI_INTERNALS__.invoke('get_database_directory'),firstLaunch:await window.__TAURI_INTERNALS__.invoke('check_first_launch')}))()"
        $result.observations.cdpDatabaseProbe = Invoke-CdpExpression `
          -WebSocketUrl $pageTarget.webSocketDebuggerUrl -Expression $expression
      }
    }
    catch {
      $result.observations.cdpDatabaseProbeError = $_.Exception.Message
    }
  }

  if ($alive) {
    $closed = $appProcess.CloseMainWindow()
    if (-not $appProcess.WaitForExit(5000)) {
      & taskkill.exe /PID $appProcess.Id /T /F 2>$null | Out-Null
      $appProcess.WaitForExit(5000) | Out-Null
    }
  }
  $appProcess = $null
  Start-Sleep -Seconds 2

  $result.observations.profileDiscoveries = @(
    Get-ChildItem -LiteralPath $env:USERPROFILE -Recurse -Force -ErrorAction SilentlyContinue |
      Where-Object {
        $_.Name -match '(?i)meetily|meeting_minutes|com\.meetily' -or
        $_.FullName -match '(?i)com\.meetily\.ai'
      } |
      Select-Object -First 500 |
      ForEach-Object {
        [ordered]@{
          path = $_.FullName
          kind = if ($_.PSIsContainer) { 'directory' } else { 'file' }
          bytes = if ($_.PSIsContainer) { $null } else { $_.Length }
          lastWriteTimeUtc = $_.LastWriteTimeUtc.ToString('o')
        }
      }
  )

  $migratedOutput = Join-Path $outputRoot 'official-v030-migrated-app-data-strict'
  if (Test-Path -LiteralPath $migratedOutput) {
    [System.IO.Directory]::Delete($migratedOutput, $true)
  }
  Copy-DirectoryContents -Source $appData -Destination $migratedOutput
  $migratedDatabase = Join-Path $migratedOutput 'meeting_minutes.sqlite'
  $result.observations.migratedDatabase = Get-FileRecord -Path $migratedDatabase
  $result.observations.migratedAppDataFiles = @(
    Get-ChildItem -LiteralPath $migratedOutput -Recurse -File -Force |
      ForEach-Object { $_.FullName.Substring($migratedOutput.Length + 1) } |
      Sort-Object
  )
  $result.assertions.migratedDatabaseCaptured = (
    (Test-Path -LiteralPath $migratedDatabase -PathType Leaf) -and
    $result.observations.migratedDatabase.bytes -gt 0 -and
    $result.observations.migratedDatabase.sha256 -ne $result.observations.seedDatabase.sha256)
  $result.passed = -not ($result.assertions.Values -contains $false)
}
catch {
  $result.error = $_.Exception.Message
  $result.passed = $false
}
finally {
  if ($null -ne $appProcess -and -not $appProcess.HasExited) {
    & taskkill.exe /PID $appProcess.Id /T /F 2>$null | Out-Null
  }
  if ($null -ne $uninstaller -and (Test-Path -LiteralPath $uninstaller -PathType Leaf)) {
    $uninstall = Invoke-InstallerWithTimeout -Path $uninstaller -Arguments @('/S') -TimeoutSeconds 60
    $result.observations.uninstall = $uninstall
  }
  [System.IO.File]::WriteAllText(
    $reportPath,
    ($result | ConvertTo-Json -Depth 10),
    [System.Text.UTF8Encoding]::new($false))
  Start-Process -FilePath 'shutdown.exe' -ArgumentList @('/s','/t','3') -WindowStyle Hidden
}

if (-not $result.passed) { exit 1 }

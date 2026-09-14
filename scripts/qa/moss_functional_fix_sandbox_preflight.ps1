param(
    [string]$OutputPath = 'C:\evidence\vm-preflight.json',
    [string]$RunId = 'FIX-1B9C29A-20260902-01'
)

$ErrorActionPreference = 'Stop'
$ruleName = "MeetilyFunctionalFixPreflight-$RunId"
$curlPath = 'C:\Windows\System32\curl.exe'

function Get-FirewallState {
    @(
        Get-NetFirewallProfile | Sort-Object Name | ForEach-Object {
            [ordered]@{
                name = [string]$_.Name
                enabled = [bool]$_.Enabled
                default_inbound_action = [string]$_.DefaultInboundAction
                default_outbound_action = [string]$_.DefaultOutboundAction
            }
        }
    )
}

function Invoke-CurlProbe {
    param([int]$Attempts = 1)
    $last = $null
    for ($attempt = 1; $attempt -le $Attempts; $attempt++) {
        $started = Get-Date
        $previousErrorActionPreference = $ErrorActionPreference
        $ErrorActionPreference = 'Continue'
        $output = & $curlPath --silent --show-error --fail --max-time 10 'http://www.msftconnecttest.com/connecttest.txt' 2>&1
        $exitCode = $LASTEXITCODE
        $ErrorActionPreference = $previousErrorActionPreference
        $last = [ordered]@{
            attempt = $attempt
            exit_code = $exitCode
            success = $exitCode -eq 0
            elapsed_ms = [int]((Get-Date) - $started).TotalMilliseconds
            response_sha256 = if ($exitCode -eq 0) {
                $bytes = [System.Text.Encoding]::UTF8.GetBytes(($output | Out-String))
                $sha = [System.Security.Cryptography.SHA256]::Create()
                try {
                    ([System.BitConverter]::ToString($sha.ComputeHash($bytes))).Replace('-', '')
                } finally {
                    $sha.Dispose()
                }
            } else { $null }
        }
        if ($exitCode -eq 0) { break }
        Start-Sleep -Seconds 2
    }
    $last
}

$principal = [Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()
$isAdmin = $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
$os = Get-CimInstance Win32_OperatingSystem
$computer = Get-CimInstance Win32_ComputerSystem
$firewallBefore = @(Get-FirewallState)
$probeBefore = Invoke-CurlProbe -Attempts 5
$probeBlocked = $null
$probeAfter = $null
$ruleCreated = $false
$ruleRemoved = $false
$errorText = $null

try {
    Remove-NetFirewallRule -DisplayName $ruleName -ErrorAction SilentlyContinue
    New-NetFirewallRule -DisplayName $ruleName -Direction Outbound -Action Block -Program $curlPath -Profile Any | Out-Null
    $ruleCreated = $null -ne (Get-NetFirewallRule -DisplayName $ruleName -ErrorAction SilentlyContinue)
    Start-Sleep -Seconds 1
    $probeBlocked = Invoke-CurlProbe
} catch {
    $errorText = $_.Exception.Message
} finally {
    Remove-NetFirewallRule -DisplayName $ruleName -ErrorAction SilentlyContinue
    $ruleRemoved = $null -eq (Get-NetFirewallRule -DisplayName $ruleName -ErrorAction SilentlyContinue)
}

$probeAfter = Invoke-CurlProbe -Attempts 3
$firewallAfter = @(Get-FirewallState)
$status = if (
    $isAdmin -and
    $probeBefore.success -and
    $ruleCreated -and
    (-not $probeBlocked.success) -and
    $ruleRemoved -and
    $probeAfter.success
) { 'PASS' } else { 'BLOCKED' }

$result = [ordered]@{
    schema_version = 1
    run_id = $RunId
    stage = 'WINDOWS_SANDBOX_NETWORK_PREFLIGHT'
    generated_at = (Get-Date).ToString('o')
    status = $status
    is_admin = $isAdmin
    os = [ordered]@{
        caption = $os.Caption
        version = $os.Version
        build = $os.BuildNumber
        computer_model = $computer.Model
    }
    curl_path_exists = Test-Path -LiteralPath $curlPath -PathType Leaf
    probe_before = $probeBefore
    temporary_rule_created = $ruleCreated
    probe_while_blocked = $probeBlocked
    temporary_rule_removed = $ruleRemoved
    probe_after = $probeAfter
    firewall_before = $firewallBefore
    firewall_after = $firewallAfter
    error = $errorText
}

$parent = Split-Path -Parent $OutputPath
[System.IO.Directory]::CreateDirectory($parent) | Out-Null
[System.IO.File]::WriteAllText(
    $OutputPath,
    (($result | ConvertTo-Json -Depth 12) + [Environment]::NewLine),
    [System.Text.UTF8Encoding]::new($false)
)

if ($status -ne 'PASS') { exit 2 }

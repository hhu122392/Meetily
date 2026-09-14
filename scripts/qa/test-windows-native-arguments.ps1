[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)][string]$PythonPath,
    [Parameter(Mandatory = $true)][string]$OutputRoot
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$scriptRoot = Split-Path -Parent $MyInvocation.MyCommand.Path
. (Join-Path $scriptRoot 'windows-native-arguments.ps1')
$PythonPath = [System.IO.Path]::GetFullPath($PythonPath)
$OutputRoot = [System.IO.Path]::GetFullPath($OutputRoot)
if (Test-Path -LiteralPath $OutputRoot) {
    if (@(Get-ChildItem -LiteralPath $OutputRoot -Force).Count -ne 0) { throw "OutputRoot must be empty: $OutputRoot" }
} else {
    New-Item -ItemType Directory -Path $OutputRoot | Out-Null
}

$expected = @(
    '',
    'plain',
    'value with spaces',
    'C:\path with spaces\',
    'embedded"quote',
    'slashes\\before"quote',
    (([string][char]0x4E2D) + ([string][char]0x6587) + ' ' + ([string][char]0x7A7A) + ([string][char]0x683C) + '\')
)
$echoScript = Join-Path $scriptRoot 'echo-argv.py'
$stdout = Join-Path $OutputRoot 'argv.stdout.json'
$stderr = Join-Path $OutputRoot 'argv.stderr.log'
$argumentLine = (@((ConvertTo-NativeArgument $echoScript)) + @($expected | ForEach-Object { ConvertTo-NativeArgument $_ })) -join ' '
$process = Start-Process -FilePath $PythonPath -ArgumentList $argumentLine -WindowStyle Hidden -PassThru -Wait -RedirectStandardOutput $stdout -RedirectStandardError $stderr
$echoResult = Get-Content -LiteralPath $stdout -Raw -Encoding UTF8 | ConvertFrom-Json
$actual = @($echoResult.arguments)
$passed = $process.ExitCode -eq 0 -and $actual.Count -eq $expected.Count
for ($index = 0; $index -lt $expected.Count -and $passed; $index++) {
    if ([string]$actual[$index] -cne [string]$expected[$index]) { $passed = $false }
}
$result = [ordered]@{
    schema_version = 1
    shell = 'Windows PowerShell compatible Start-Process ArgumentList'
    exit_code = [int]$process.ExitCode
    expected = $expected
    actual = $actual
    stderr_bytes = [int64](Get-Item -LiteralPath $stderr).Length
    verdict = if ($passed) { 'PASS' } else { 'FAIL' }
    invocation = [ordered]@{
        powershell_version = $PSVersionTable.PSVersion.ToString()
        python_path = $PythonPath
        arguments = $expected
    }
    source_files = @(
        foreach ($path in @(
            [System.IO.Path]::GetFullPath($MyInvocation.MyCommand.Path),
            [System.IO.Path]::GetFullPath((Join-Path $scriptRoot 'windows-native-arguments.ps1')),
            [System.IO.Path]::GetFullPath($echoScript)
        )) {
            [ordered]@{
                path = $path
                bytes = [int64](Get-Item -LiteralPath $path).Length
                sha256 = (Get-FileHash -LiteralPath $path -Algorithm SHA256).Hash.ToUpperInvariant()
            }
        }
    )
    python = [ordered]@{
        path = $PythonPath
        bytes = [int64](Get-Item -LiteralPath $PythonPath).Length
        sha256 = (Get-FileHash -LiteralPath $PythonPath -Algorithm SHA256).Hash.ToUpperInvariant()
    }
}
$resultPath = Join-Path $OutputRoot 'windows-native-arguments.private.json'
[System.IO.File]::WriteAllText($resultPath, (($result | ConvertTo-Json -Depth 8) + "`n"), [System.Text.UTF8Encoding]::new($false))
$result | ConvertTo-Json -Depth 8
if (-not $passed) { exit 1 }

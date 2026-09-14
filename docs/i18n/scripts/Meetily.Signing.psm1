Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
Import-Module Microsoft.PowerShell.Utility -ErrorAction Stop

function ConvertTo-MeetilyNormalizedThumbprint {
  param([AllowNull()][string]$Thumbprint)
  if ([string]::IsNullOrWhiteSpace($Thumbprint)) { return '' }
  return ($Thumbprint -replace '[^0-9A-Fa-f]', '').ToUpperInvariant()
}

function ConvertTo-MeetilyStringArray {
  param([AllowNull()]$Value)
  if ($null -eq $Value) { return @() }
  return @($Value | ForEach-Object { [string]$_ })
}

function Read-MeetilyWindowsSigningPolicy {
  [CmdletBinding()]
  param([Parameter(Mandatory = $true)][string]$Path)

  $resolved = [IO.Path]::GetFullPath($Path)
  if (-not (Test-Path -LiteralPath $resolved -PathType Leaf)) {
    throw "Windows signing policy was not found: $resolved"
  }
  $policy = Get-Content -LiteralPath $resolved -Raw | ConvertFrom-Json
  if ([int]$policy.schemaVersion -ne 1) { throw 'Unsupported Windows signing policy schemaVersion.' }
  if ([string]$policy.authenticode.fileDigestAlgorithm -cne 'SHA256') {
    throw 'Windows signing policy must require SHA256 file digests.'
  }
  if ([string]$policy.authenticode.codeSigningEkuOid -cne '1.3.6.1.5.5.7.3.3') {
    throw 'Windows signing policy must require the Code Signing EKU.'
  }
  $subjects = @(ConvertTo-MeetilyStringArray $policy.authenticode.approvedSignerSubjects)
  if ($subjects.Count -eq 0 -or @($subjects | Where-Object { [string]::IsNullOrWhiteSpace($_) }).Count -gt 0) {
    throw 'At least one non-empty approved signer subject is required.'
  }

  $setProperties = @($policy.authenticode.signerThumbprintSets.PSObject.Properties)
  if ($setProperties.Count -eq 0) { throw 'At least one signer thumbprint set is required.' }
  foreach ($setProperty in $setProperties) {
    foreach ($thumbprint in (ConvertTo-MeetilyStringArray $setProperty.Value)) {
      $normalized = ConvertTo-MeetilyNormalizedThumbprint $thumbprint
      if ($normalized -cnotmatch '^[0-9A-F]{40}$') {
        throw "Signer thumbprint set '$($setProperty.Name)' contains a malformed SHA-1 thumbprint."
      }
    }
  }

  $roleProperties = @($policy.roles.PSObject.Properties)
  if ($roleProperties.Count -eq 0) { throw 'At least one signing role is required.' }
  foreach ($roleProperty in $roleProperties) {
    $setName = [string]$roleProperty.Value.signerThumbprintSet
    if ([string]::IsNullOrWhiteSpace($setName) -or $setProperties.Name -cnotcontains $setName) {
      throw "Signing role '$($roleProperty.Name)' references an unknown thumbprint set."
    }
  }
  $policy | Add-Member -NotePropertyName _resolvedPath -NotePropertyValue $resolved -Force
  return $policy
}

function Resolve-MeetilySignTool {
  [CmdletBinding()]
  param([string]$Path)

  if (-not [string]::IsNullOrWhiteSpace($Path)) {
    $resolved = [IO.Path]::GetFullPath($Path)
    if (-not (Test-Path -LiteralPath $resolved -PathType Leaf)) { throw "SignTool was not found: $resolved" }
    return $resolved
  }
  $command = Get-Command signtool.exe -ErrorAction SilentlyContinue | Select-Object -First 1
  if ($command) { return $command.Source }
  $roots = @(
    (Join-Path ${env:ProgramFiles(x86)} 'Windows Kits\10\bin'),
    (Join-Path $env:ProgramFiles 'Windows Kits\10\bin')
  ) | Where-Object { -not [string]::IsNullOrWhiteSpace($_) -and (Test-Path -LiteralPath $_ -PathType Container) }
  $candidates = @(
    foreach ($root in $roots) {
      Get-ChildItem -LiteralPath $root -Directory -ErrorAction SilentlyContinue |
        Sort-Object Name -Descending |
        ForEach-Object { Join-Path $_.FullName 'x64\signtool.exe' } |
        Where-Object { Test-Path -LiteralPath $_ -PathType Leaf }
    }
  )
  if ($candidates.Count -eq 0) { throw 'A Windows SDK x64 signtool.exe installation is required.' }
  return [IO.Path]::GetFullPath($candidates[0])
}

function Get-MeetilyCertificateEkuOids {
  param([AllowNull()]$Certificate)
  if ($null -eq $Certificate) { return @() }
  $oids = @()
  foreach ($extension in @($Certificate.Extensions)) {
    if ($extension.Oid.Value -eq '2.5.29.37') {
      foreach ($eku in @($extension.EnhancedKeyUsages)) { $oids += [string]$eku.Value }
    }
  }
  return @($oids | Sort-Object -Unique)
}

function Get-MeetilyAuthenticodeEvidence {
  [CmdletBinding()]
  param(
    [Parameter(Mandatory = $true)][string]$Path,
    [string]$SignToolPath
  )

  $resolved = [IO.Path]::GetFullPath($Path)
  if (-not (Test-Path -LiteralPath $resolved -PathType Leaf)) { throw "Signed file was not found: $resolved" }
  $signTool = Resolve-MeetilySignTool -Path $SignToolPath
  $signature = Get-AuthenticodeSignature -LiteralPath $resolved
  $signToolOutput = @(& $signTool verify /pa /all /tw /u 1.3.6.1.5.5.7.3.3 /q $resolved 2>&1)
  $signToolExitCode = $LASTEXITCODE
  $signer = $signature.SignerCertificate
  $timestamp = $signature.TimeStamperCertificate
  return [pscustomobject][ordered]@{
    path = $resolved
    bytes = (Get-Item -LiteralPath $resolved).Length
    sha256 = (Get-FileHash -Algorithm SHA256 -LiteralPath $resolved).Hash
    signatureStatus = $signature.Status.ToString()
    signerSubject = if ($signer) { [string]$signer.Subject } else { $null }
    signerThumbprint = if ($signer) { ConvertTo-MeetilyNormalizedThumbprint $signer.Thumbprint } else { $null }
    signerNotBeforeUtc = if ($signer) { $signer.NotBefore.ToUniversalTime().ToString('o') } else { $null }
    signerNotAfterUtc = if ($signer) { $signer.NotAfter.ToUniversalTime().ToString('o') } else { $null }
    signerEkuOids = @(Get-MeetilyCertificateEkuOids -Certificate $signer)
    timestampCertificatePresent = ($null -ne $timestamp)
    timestampSubject = if ($timestamp) { [string]$timestamp.Subject } else { $null }
    timestampThumbprint = if ($timestamp) { ConvertTo-MeetilyNormalizedThumbprint $timestamp.Thumbprint } else { $null }
    timestampNotBeforeUtc = if ($timestamp) { $timestamp.NotBefore.ToUniversalTime().ToString('o') } else { $null }
    timestampNotAfterUtc = if ($timestamp) { $timestamp.NotAfter.ToUniversalTime().ToString('o') } else { $null }
    signToolDefaultAuthenticodeExitCode = $signToolExitCode
    signToolDefaultAuthenticodePassed = ($signToolExitCode -eq 0)
    signToolOutputLineCount = $signToolOutput.Count
  }
}

function Test-MeetilySignerEvidence {
  [CmdletBinding()]
  param(
    [Parameter(Mandatory = $true)]$Evidence,
    [Parameter(Mandatory = $true)]$Policy,
    [Parameter(Mandatory = $true)][string]$Role
  )

  $roleProperty = $Policy.roles.PSObject.Properties[$Role]
  if ($null -eq $roleProperty) { throw "Unknown Windows signing role: $Role" }
  $setName = [string]$roleProperty.Value.signerThumbprintSet
  $setProperty = $Policy.authenticode.signerThumbprintSets.PSObject.Properties[$setName]
  if ($null -eq $setProperty) { throw "Signing role '$Role' references an unknown thumbprint set."
  }
  $allowedThumbprints = @(
    ConvertTo-MeetilyStringArray $setProperty.Value |
      ForEach-Object { ConvertTo-MeetilyNormalizedThumbprint $_ }
  )
  $approvedSubjects = @(ConvertTo-MeetilyStringArray $Policy.authenticode.approvedSignerSubjects)
  $requiredEku = [string]$Policy.authenticode.codeSigningEkuOid
  $actualThumbprint = ConvertTo-MeetilyNormalizedThumbprint ([string]$Evidence.signerThumbprint)
  $actualEkus = @(ConvertTo-MeetilyStringArray $Evidence.signerEkuOids)
  $failures = New-Object System.Collections.Generic.List[string]

  if ($allowedThumbprints.Count -eq 0) { $failures.Add("Signer thumbprint set '$setName' is empty.") }
  if ([string]$Evidence.signatureStatus -cne 'Valid') { $failures.Add('Authenticode status is not Valid.') }
  if ($approvedSubjects -cnotcontains [string]$Evidence.signerSubject) { $failures.Add('Signer subject is not approved.') }
  if ($allowedThumbprints -cnotcontains $actualThumbprint) { $failures.Add('Signer thumbprint is not approved for this role.') }
  if ($actualEkus -cnotcontains $requiredEku) { $failures.Add('Code Signing EKU is missing.') }
  if (-not [bool]$Evidence.timestampCertificatePresent) { $failures.Add('Timestamp certificate is missing.') }
  if (-not [bool]$Evidence.signToolDefaultAuthenticodePassed) {
    $failures.Add('SignTool Default Authenticode, timestamp, EKU, or trust-chain verification failed.')
  }

  return [pscustomobject][ordered]@{
    passed = ($failures.Count -eq 0)
    role = $Role
    signerThumbprintSet = $setName
    allowedSignerThumbprintCount = $allowedThumbprints.Count
    failures = @($failures)
    evidence = $Evidence
  }
}

function Test-MeetilySignedFile {
  [CmdletBinding()]
  param(
    [Parameter(Mandatory = $true)][string]$Path,
    [Parameter(Mandatory = $true)][string]$PolicyPath,
    [Parameter(Mandatory = $true)][string]$Role,
    [string]$ExpectedSha256,
    [string]$SignToolPath,
    [switch]$ThrowOnFailure
  )

  $policy = Read-MeetilyWindowsSigningPolicy -Path $PolicyPath
  $evidence = Get-MeetilyAuthenticodeEvidence -Path $Path -SignToolPath $SignToolPath
  $result = Test-MeetilySignerEvidence -Evidence $evidence -Policy $policy -Role $Role
  if (-not [string]::IsNullOrWhiteSpace($ExpectedSha256)) {
    $expected = $ExpectedSha256.ToUpperInvariant()
    if ($expected -cnotmatch '^[0-9A-F]{64}$') { throw 'Expected SHA-256 is malformed.' }
    if ([string]$evidence.sha256 -cne $expected) {
      $result.failures = @($result.failures) + 'SHA-256 does not match the frozen expected value.'
      $result.passed = $false
    }
  }
  if ($ThrowOnFailure -and -not $result.passed) {
    throw "Windows signing policy rejected '$([IO.Path]::GetFileName($evidence.path))' for role '$Role': $($result.failures -join ' ')"
  }
  return $result
}

Export-ModuleMember -Function @(
  'ConvertTo-MeetilyNormalizedThumbprint',
  'Read-MeetilyWindowsSigningPolicy',
  'Resolve-MeetilySignTool',
  'Get-MeetilyAuthenticodeEvidence',
  'Test-MeetilySignerEvidence',
  'Test-MeetilySignedFile'
)

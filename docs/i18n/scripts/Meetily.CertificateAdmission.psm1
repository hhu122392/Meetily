Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
Import-Module Microsoft.PowerShell.Utility -ErrorAction Stop

function ConvertTo-MeetilyAdmissionStringArray {
  param([AllowNull()]$Value)
  if ($null -eq $Value) { return @() }
  return @($Value | ForEach-Object { [string]$_ })
}

function Read-MeetilyWindowsCertificateAdmission {
  [CmdletBinding()]
  param([Parameter(Mandatory = $true)][string]$Path)

  $resolved = [IO.Path]::GetFullPath($Path)
  if (-not (Test-Path -LiteralPath $resolved -PathType Leaf)) {
    throw "Windows certificate admission record was not found: $resolved"
  }
  $record = Get-Content -LiteralPath $resolved -Raw | ConvertFrom-Json
  if ([int]$record.schemaVersion -ne 1) { throw 'Unsupported certificate admission schemaVersion.' }
  if ([string]$record.admissionId -cne 'meetily-windows-production-certificate-admission-v1') {
    throw 'Unexpected certificate admission identifier.'
  }
  if (@('PendingCandidateEvidence', 'PendingApproval', 'Approved', 'Rejected', 'Revoked') -cnotcontains [string]$record.status) {
    throw 'Unsupported certificate admission status.'
  }
  if ([int]$record.requirements.minimumRemainingValidityDays -lt 1) {
    throw 'Certificate admission must require at least one remaining validity day.'
  }
  if ([string]$record.requirements.requiredCodeSigningEkuOid -cne '1.3.6.1.5.5.7.3.3') {
    throw 'Certificate admission must require the Code Signing EKU.'
  }
  $approvals = @(ConvertTo-MeetilyAdmissionStringArray $record.requirements.requiredIndependentApprovals)
  if ($approvals.Count -lt 2 -or $approvals -cnotcontains 'releaseOwner' -or $approvals -cnotcontains 'securityReviewer') {
    throw 'Certificate admission requires independent release-owner and security-reviewer approvals.'
  }
  foreach ($requiredBoolean in @(
    'requireExactApprovedSubject', 'requireTrustedCertificateChain', 'requireDigiCertHealthcheck',
    'requireDigiCertCertificateSync', 'requireProofOfPossession',
    'requireTauriUpdaterPrivateKeyPresence', 'requireTauriUpdaterPrivateKeyPasswordPresence',
    'forbidSelfSignedCertificate', 'forbidHistoricalThumbprintReuse'
  )) {
    $property = $record.requirements.PSObject.Properties[$requiredBoolean]
    if ($null -eq $property -or -not [bool]$property.Value) {
      throw "Certificate admission requirement '$requiredBoolean' must be enabled."
    }
  }
  $record | Add-Member -NotePropertyName _resolvedPath -NotePropertyValue $resolved -Force
  return $record
}

function Get-MeetilyEnvironmentPresence {
  [CmdletBinding()]
  param([Parameter(Mandatory = $true)][string[]]$Names)

  $result = [ordered]@{}
  foreach ($name in $Names) {
    if ([string]::IsNullOrWhiteSpace($name)) { throw 'Environment variable names must be non-empty.' }
    $result[$name] = -not [string]::IsNullOrWhiteSpace([Environment]::GetEnvironmentVariable($name))
  }
  return [pscustomobject]$result
}

function Get-MeetilyTauriPublicKeyId {
  [CmdletBinding()]
  param([Parameter(Mandatory = $true)][string]$TauriConfigPath)

  $resolved = [IO.Path]::GetFullPath($TauriConfigPath)
  if (-not (Test-Path -LiteralPath $resolved -PathType Leaf)) { throw "Tauri config was not found: $resolved" }
  $config = Get-Content -LiteralPath $resolved -Raw | ConvertFrom-Json
  $encoded = [string]$config.plugins.updater.pubkey
  if ([string]::IsNullOrWhiteSpace($encoded)) { throw 'Tauri updater public key is missing.' }
  try {
    $decoded = [Text.Encoding]::UTF8.GetString([Convert]::FromBase64String($encoded))
  } catch {
    throw 'Tauri updater public key is not valid base64.'
  }
  $match = [regex]::Match($decoded, 'minisign public key:\s*([0-9A-Fa-f]{16})')
  if (-not $match.Success) { throw 'Tauri updater public key ID could not be parsed.' }
  return $match.Groups[1].Value.ToUpperInvariant()
}

function Get-MeetilyCertificatePublicEvidence {
  [CmdletBinding()]
  param([AllowNull()]$Certificate)

  if ($null -eq $Certificate) {
    return [pscustomobject][ordered]@{
      found = $false
      subject = $null
      sha1Thumbprint = $null
      serialNumber = $null
      issuer = $null
      notBeforeUtc = $null
      notAfterUtc = $null
      ekuOids = @()
      chainTrusted = $false
      chainStatus = @('CertificateNotFound')
      selfSigned = $false
    }
  }

  $ekuOids = New-Object System.Collections.Generic.List[string]
  foreach ($extension in @($Certificate.Extensions)) {
    if ($extension.Oid.Value -eq '2.5.29.37') {
      foreach ($eku in @($extension.EnhancedKeyUsages)) { $ekuOids.Add([string]$eku.Value) }
    }
  }
  $chain = New-Object Security.Cryptography.X509Certificates.X509Chain
  try {
    $chain.ChainPolicy.RevocationMode = [Security.Cryptography.X509Certificates.X509RevocationMode]::Online
    $chain.ChainPolicy.RevocationFlag = [Security.Cryptography.X509Certificates.X509RevocationFlag]::ExcludeRoot
    $chain.ChainPolicy.VerificationFlags = [Security.Cryptography.X509Certificates.X509VerificationFlags]::NoFlag
    $chainTrusted = $chain.Build($Certificate)
    $chainStatus = @($chain.ChainStatus | ForEach-Object { $_.Status.ToString() } | Sort-Object -Unique)
    if ($chainStatus.Count -eq 0) { $chainStatus = @('NoError') }
  } finally {
    $chain.Dispose()
  }
  return [pscustomobject][ordered]@{
    found = $true
    subject = [string]$Certificate.Subject
    sha1Thumbprint = ([string]$Certificate.Thumbprint -replace '[^0-9A-Fa-f]', '').ToUpperInvariant()
    serialNumber = ([string]$Certificate.SerialNumber -replace '\s', '').ToUpperInvariant()
    issuer = [string]$Certificate.Issuer
    notBeforeUtc = $Certificate.NotBefore.ToUniversalTime().ToString('o')
    notAfterUtc = $Certificate.NotAfter.ToUniversalTime().ToString('o')
    ekuOids = @($ekuOids.ToArray() | Sort-Object -Unique)
    chainTrusted = [bool]$chainTrusted
    chainStatus = @($chainStatus)
    selfSigned = ([string]$Certificate.Subject -ceq [string]$Certificate.Issuer)
  }
}

function Test-MeetilyCertificateApprovalRecord {
  [CmdletBinding()]
  param(
    [Parameter(Mandatory = $true)]$Policy,
    [Parameter(Mandatory = $true)]$Admission
  )

  $failures = New-Object System.Collections.Generic.List[string]
  if ([string]$Admission.targetPolicyId -cne [string]$Policy.policyId) { $failures.Add('Admission target policy does not match.') }
  if ([string]$Admission.targetThumbprintSet -cne 'productionActive') { $failures.Add('Admission target thumbprint set is not productionActive.') }
  if ([string]$Admission.status -cne 'Approved') { $failures.Add('Certificate admission status is not Approved.') }

  $candidateSubject = [string]$Admission.candidate.subject
  $candidateThumbprint = ([string]$Admission.candidate.sha1Thumbprint -replace '[^0-9A-Fa-f]', '').ToUpperInvariant()
  $candidateSerial = ([string]$Admission.candidate.serialNumber -replace '\s', '').ToUpperInvariant()
  if ([string]::IsNullOrWhiteSpace($candidateSubject)) { $failures.Add('Approved candidate subject is missing.') }
  if ($candidateThumbprint -cnotmatch '^[0-9A-F]{40}$') { $failures.Add('Approved candidate SHA-1 thumbprint is missing or malformed.') }
  if ([string]::IsNullOrWhiteSpace($candidateSerial)) { $failures.Add('Approved candidate serial number is missing.') }
  if ([string]::IsNullOrWhiteSpace([string]$Admission.candidate.issuer)) { $failures.Add('Approved candidate issuer is missing.') }
  $candidateNotBefore = [DateTimeOffset]::MinValue
  $candidateNotAfter = [DateTimeOffset]::MinValue
  $candidateNotBeforeValid = [DateTimeOffset]::TryParse([string]$Admission.candidate.notBeforeUtc, [ref]$candidateNotBefore)
  $candidateNotAfterValid = [DateTimeOffset]::TryParse([string]$Admission.candidate.notAfterUtc, [ref]$candidateNotAfter)
  if (-not $candidateNotBeforeValid -or -not $candidateNotAfterValid -or $candidateNotBefore -ge $candidateNotAfter) {
    $failures.Add('Approved candidate validity interval is missing or malformed.')
  }
  if (@($Policy.authenticode.approvedSignerSubjects) -cnotcontains $candidateSubject) { $failures.Add('Approved candidate subject is not allowed by signing policy.') }

  $active = @($Policy.authenticode.signerThumbprintSets.productionActive | ForEach-Object {
    ([string]$_ -replace '[^0-9A-Fa-f]', '').ToUpperInvariant()
  })
  if ($active.Count -ne 1 -or $active -cnotcontains $candidateThumbprint) {
    $failures.Add('productionActive must contain exactly the approved candidate thumbprint.')
  }
  $historical = @($Policy.authenticode.signerThumbprintSets.upstreamHistorical | ForEach-Object {
    ([string]$_ -replace '[^0-9A-Fa-f]', '').ToUpperInvariant()
  })
  if ($historical -ccontains $candidateThumbprint) { $failures.Add('Historical signer thumbprints cannot be admitted for current production signing.') }

  foreach ($approvalName in @($Admission.requirements.requiredIndependentApprovals)) {
    $approvalProperty = $Admission.approvals.PSObject.Properties[[string]$approvalName]
    $approval = if ($null -eq $approvalProperty) { $null } else { $approvalProperty.Value }
    if ($null -eq $approval -or [string]$approval.status -cne 'Approved') {
      $failures.Add("Required approval '$approvalName' is not Approved.")
      continue
    }
    $parsedApprovalTime = [DateTimeOffset]::MinValue
    if (-not [DateTimeOffset]::TryParse([string]$approval.approvedAtUtc, [ref]$parsedApprovalTime)) {
      $failures.Add("Required approval '$approvalName' has no valid approval timestamp.")
    }
    if ([string]::IsNullOrWhiteSpace([string]$approval.approvalReference)) {
      $failures.Add("Required approval '$approvalName' has no reviewable approval reference.")
    }
  }
  return [pscustomobject][ordered]@{ passed = ($failures.Count -eq 0); failures = @($failures) }
}

function Test-MeetilyCertificatePublicEvidence {
  [CmdletBinding()]
  param(
    [Parameter(Mandatory = $true)]$Evidence,
    [Parameter(Mandatory = $true)]$Policy,
    [Parameter(Mandatory = $true)]$Admission,
    [DateTimeOffset]$EvaluatedAtUtc = [DateTimeOffset]::UtcNow
  )

  $failures = New-Object System.Collections.Generic.List[string]
  if (-not [bool]$Evidence.found) {
    $failures.Add('Configured certificate was not found in the Windows certificate stores.')
    return [pscustomobject][ordered]@{ passed = $false; failures = @($failures) }
  }
  $candidateThumbprint = ([string]$Admission.candidate.sha1Thumbprint -replace '[^0-9A-Fa-f]', '').ToUpperInvariant()
  if ([string]$Evidence.subject -cne [string]$Admission.candidate.subject) { $failures.Add('Synchronized certificate subject does not match the admission record.') }
  if ([string]$Evidence.sha1Thumbprint -cne $candidateThumbprint) { $failures.Add('Synchronized certificate thumbprint does not match the admission record.') }
  if (-not [string]::IsNullOrWhiteSpace([string]$Admission.candidate.serialNumber) -and [string]$Evidence.serialNumber -cne [string]$Admission.candidate.serialNumber) {
    $failures.Add('Synchronized certificate serial number does not match the admission record.')
  }
  if (-not [string]::IsNullOrWhiteSpace([string]$Admission.candidate.issuer) -and [string]$Evidence.issuer -cne [string]$Admission.candidate.issuer) {
    $failures.Add('Synchronized certificate issuer does not match the admission record.')
  }
  if (@($Policy.authenticode.approvedSignerSubjects) -cnotcontains [string]$Evidence.subject) { $failures.Add('Synchronized certificate subject is not approved by signing policy.') }
  if (@($Evidence.ekuOids) -cnotcontains [string]$Admission.requirements.requiredCodeSigningEkuOid) { $failures.Add('Synchronized certificate does not contain the Code Signing EKU.') }
  if (-not [bool]$Evidence.chainTrusted) { $failures.Add('Synchronized certificate chain is not trusted.') }
  if ([bool]$Evidence.selfSigned) { $failures.Add('Self-signed certificates are forbidden for production admission.') }
  $notBefore = [DateTimeOffset]::MinValue
  $notAfter = [DateTimeOffset]::MinValue
  if (-not [DateTimeOffset]::TryParse([string]$Evidence.notBeforeUtc, [ref]$notBefore) -or
      -not [DateTimeOffset]::TryParse([string]$Evidence.notAfterUtc, [ref]$notAfter)) {
    $failures.Add('Synchronized certificate validity interval is malformed.')
  } else {
    $recordNotBefore = [DateTimeOffset]::MinValue
    $recordNotAfter = [DateTimeOffset]::MinValue
    if ([DateTimeOffset]::TryParse([string]$Admission.candidate.notBeforeUtc, [ref]$recordNotBefore) -and
        [DateTimeOffset]::TryParse([string]$Admission.candidate.notAfterUtc, [ref]$recordNotAfter)) {
      if ($notBefore.ToUniversalTime() -ne $recordNotBefore.ToUniversalTime() -or $notAfter.ToUniversalTime() -ne $recordNotAfter.ToUniversalTime()) {
        $failures.Add('Synchronized certificate validity interval does not match the admission record.')
      }
    }
    if ($EvaluatedAtUtc -lt $notBefore -or $EvaluatedAtUtc -gt $notAfter) { $failures.Add('Synchronized certificate is not currently valid.') }
    if ($notAfter -lt $EvaluatedAtUtc.AddDays([int]$Admission.requirements.minimumRemainingValidityDays)) {
      $failures.Add('Synchronized certificate does not meet the minimum remaining-validity requirement.')
    }
  }
  return [pscustomobject][ordered]@{ passed = ($failures.Count -eq 0); failures = @($failures) }
}

function Test-MeetilyCertificateAdmissionReadiness {
  [CmdletBinding()]
  param(
    [Parameter(Mandatory = $true)]$Evidence,
    [Parameter(Mandatory = $true)]$Policy,
    [Parameter(Mandatory = $true)]$Admission
  )

  $failures = New-Object System.Collections.Generic.List[string]
  $approval = Test-MeetilyCertificateApprovalRecord -Policy $Policy -Admission $Admission
  foreach ($failure in @($approval.failures)) { $failures.Add([string]$failure) }
  $certificate = Test-MeetilyCertificatePublicEvidence -Evidence $Evidence.certificate -Policy $Policy -Admission $Admission
  foreach ($failure in @($certificate.failures)) { $failures.Add([string]$failure) }

  $requiredEnvironmentNames = @($Policy.productionControls.requiredDigiCertEnvironmentVariables) + @($Policy.updater.privateKeyEnvironmentVariables)
  foreach ($name in $requiredEnvironmentNames) {
    $property = $Evidence.environmentPresence.PSObject.Properties[[string]$name]
    if ($null -eq $property -or -not [bool]$property.Value) { $failures.Add("Required environment entry '$name' is absent.") }
  }
  if (-not [bool]$Evidence.tools.smctlAvailable) { $failures.Add('smctl is not available on PATH.') }
  if (-not [bool]$Evidence.tools.signToolAvailable) { $failures.Add('Windows SDK SignTool is not available.') }
  if (-not [bool]$Evidence.digiCert.clientCertificateFilePresent) { $failures.Add('DigiCert client certificate file is absent.') }
  if (-not [bool]$Evidence.digiCert.healthcheckPassed) { $failures.Add('DigiCert healthcheck has not passed.') }
  if (-not [bool]$Evidence.digiCert.certificateSyncPassed) { $failures.Add('DigiCert certificate synchronization has not passed.') }
  if (-not [bool]$Evidence.configuration.configuredThumbprintMatchesAdmission) { $failures.Add('Configured DigiCert thumbprint does not match the admission record.') }
  if ([string]$Evidence.updater.publicKeyId -cne [string]$Policy.updater.publicKeyId) { $failures.Add('Tauri updater public-key ID does not match signing policy.') }
  if (-not [bool]$Evidence.proofOfPossession.passed) { $failures.Add('Disposable-artifact proof of possession has not passed.') }
  if ([string]$Evidence.proofOfPossession.signerSubject -cne [string]$Admission.candidate.subject) {
    $failures.Add('Proof-of-possession signer subject does not match the admitted certificate.')
  }
  if ([string]$Evidence.proofOfPossession.signerThumbprint -cne ([string]$Admission.candidate.sha1Thumbprint -replace '[^0-9A-Fa-f]', '').ToUpperInvariant()) {
    $failures.Add('Proof-of-possession signer does not match the admitted certificate.')
  }
  if (-not [bool]$Evidence.proofOfPossession.timestampCertificatePresent) { $failures.Add('Proof-of-possession timestamp certificate is missing.') }
  if (-not [bool]$Evidence.proofOfPossession.signToolVerificationPassed) { $failures.Add('Proof-of-possession SignTool verification has not passed.') }
  return [pscustomobject][ordered]@{
    passed = ($failures.Count -eq 0)
    approvalRecordPassed = [bool]$approval.passed
    certificateEvidencePassed = [bool]$certificate.passed
    failures = @($failures)
  }
}

Export-ModuleMember -Function @(
  'Read-MeetilyWindowsCertificateAdmission',
  'Get-MeetilyEnvironmentPresence',
  'Get-MeetilyTauriPublicKeyId',
  'Get-MeetilyCertificatePublicEvidence',
  'Test-MeetilyCertificateApprovalRecord',
  'Test-MeetilyCertificatePublicEvidence',
  'Test-MeetilyCertificateAdmissionReadiness'
)

[CmdletBinding()]
param([Parameter(Mandatory = $true)][string]$AudioPath)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$full = [System.IO.Path]::GetFullPath($AudioPath)
if (-not (Test-Path -LiteralPath $full -PathType Leaf) -or
    -not [System.IO.Path]::GetExtension($full).Equals('.wav', [System.StringComparison]::OrdinalIgnoreCase)) {
    throw 'Playback input must be an existing WAV file.'
}

$player = [System.Media.SoundPlayer]::new($full)
try {
    $player.Load()
    $player.PlaySync()
} finally {
    try { $player.Stop() } catch {}
    $player.Dispose()
}

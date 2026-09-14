[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string]$PcmF32Path,

    [Parameter(Mandatory = $true)]
    [string]$TruthPath,

    [Parameter(Mandatory = $true)]
    [string]$OutputJson
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$fullPcm = [System.IO.Path]::GetFullPath($PcmF32Path)
$fullTruth = [System.IO.Path]::GetFullPath($TruthPath)
$fullOutput = [System.IO.Path]::GetFullPath($OutputJson)
[System.IO.Directory]::CreateDirectory([System.IO.Path]::GetDirectoryName($fullOutput)) | Out-Null

$temporaryWave = [System.IO.Path]::ChangeExtension($fullOutput, '.recognizer-input.tmp.wav')
try {
    $bytes = [System.IO.File]::ReadAllBytes($fullPcm)
    if (($bytes.Length % 4) -ne 0) {
        throw 'Input must be mono f32 little-endian PCM.'
    }
    $frames = [int]($bytes.Length / 4)
    $pcm16 = [byte[]]::new($frames * 2)
    for ($index = 0; $index -lt $frames; $index++) {
        $sample = [System.BitConverter]::ToSingle($bytes, $index * 4)
        if ([Single]::IsNaN($sample) -or [Single]::IsInfinity($sample)) { $sample = 0.0 }
        $sample = [Math]::Max(-1.0, [Math]::Min(1.0, $sample))
        $integer = [int16][Math]::Round($sample * 32767.0)
        $pcm16[$index * 2] = [byte]($integer -band 0xff)
        $pcm16[$index * 2 + 1] = [byte](($integer -shr 8) -band 0xff)
    }

    $file = [System.IO.File]::Create($temporaryWave)
    $writer = [System.IO.BinaryWriter]::new($file, [System.Text.Encoding]::ASCII, $false)
    try {
        $writer.Write([System.Text.Encoding]::ASCII.GetBytes('RIFF'))
        $writer.Write([uint32](36 + $pcm16.Length))
        $writer.Write([System.Text.Encoding]::ASCII.GetBytes('WAVE'))
        $writer.Write([System.Text.Encoding]::ASCII.GetBytes('fmt '))
        $writer.Write([uint32]16)
        $writer.Write([uint16]1)
        $writer.Write([uint16]1)
        $writer.Write([uint32]48000)
        $writer.Write([uint32]96000)
        $writer.Write([uint16]2)
        $writer.Write([uint16]16)
        $writer.Write([System.Text.Encoding]::ASCII.GetBytes('data'))
        $writer.Write([uint32]$pcm16.Length)
        $writer.Write($pcm16)
    } finally {
        $writer.Dispose()
    }

    Add-Type -AssemblyName System.Speech
    $recognizerInfo = [System.Speech.Recognition.SpeechRecognitionEngine]::InstalledRecognizers() |
        Where-Object { $_.Culture.Name -eq 'zh-CN' } |
        Select-Object -First 1
    if ($null -eq $recognizerInfo) {
        throw 'No zh-CN Windows speech recognizer is installed.'
    }
    $recognizer = [System.Speech.Recognition.SpeechRecognitionEngine]::new($recognizerInfo)
    $segments = [System.Collections.Generic.List[object]]::new()
    try {
        $recognizer.LoadGrammar([System.Speech.Recognition.DictationGrammar]::new())
        $recognizer.SetInputToWaveFile($temporaryWave)
        for ($attempt = 0; $attempt -lt 100; $attempt++) {
            $result = $recognizer.Recognize([TimeSpan]::FromSeconds(30))
            if ($null -eq $result) { break }
            $segments.Add([pscustomobject]@{
                text = $result.Text
                confidence = $result.Confidence
                audio_position_seconds = $result.Audio.AudioPosition.TotalSeconds
                audio_duration_seconds = $result.Audio.Duration.TotalSeconds
            })
        }
    } finally {
        $recognizer.Dispose()
    }

    $recognizedText = ($segments | ForEach-Object { $_.text }) -join ''
    $truthText = [System.IO.File]::ReadAllText($fullTruth, [System.Text.Encoding]::UTF8).Trim()
    $normalize = {
        param([string]$Text)
        $normalized = $Text.Normalize([System.Text.NormalizationForm]::FormKC).ToLowerInvariant()
        [System.Text.RegularExpressions.Regex]::Replace($normalized, '[\s\p{P}]', '')
    }
    $normalizedTruth = & $normalize $truthText
    $normalizedRecognized = & $normalize $recognizedText
    $utf8NoBom = [System.Text.UTF8Encoding]::new($false)
    $truthNormalizedHash = [System.BitConverter]::ToString(
        [System.Security.Cryptography.SHA256]::Create().ComputeHash($utf8NoBom.GetBytes($normalizedTruth))
    ).Replace('-', '')
    $recognizedNormalizedHash = [System.BitConverter]::ToString(
        [System.Security.Cryptography.SHA256]::Create().ComputeHash($utf8NoBom.GetBytes($normalizedRecognized))
    ).Replace('-', '')
    $payload = [pscustomobject]@{
        schema_version = 1
        captured_at_utc = [DateTime]::UtcNow.ToString('o')
        evidence_scope = 'isolated Windows SAPI diagnostic only; not Meetily or MOSS product acceptance'
        recognizer_id = $recognizerInfo.Id
        recognizer_description = $recognizerInfo.Description
        recognizer_culture = $recognizerInfo.Culture.Name
        source_pcm_path = $fullPcm
        source_pcm_sha256 = (Get-FileHash -LiteralPath $fullPcm -Algorithm SHA256).Hash
        source_format = '48000Hz mono f32 little-endian'
        segment_count = $segments.Count
        segments = $segments
        recognized_text = $recognizedText
        normalized_recognized_text = $normalizedRecognized
        normalized_recognized_sha256 = $recognizedNormalizedHash
        normalized_truth_text = $normalizedTruth
        normalized_truth_sha256 = $truthNormalizedHash
        strict_normalized_match = ($normalizedRecognized -ceq $normalizedTruth)
    }
    [System.IO.File]::WriteAllText($fullOutput, ($payload | ConvertTo-Json -Depth 8), $utf8NoBom)
    $payload | ConvertTo-Json -Depth 8
} finally {
    if (Test-Path -LiteralPath $temporaryWave) {
        Remove-Item -LiteralPath $temporaryWave -Force
    }
}

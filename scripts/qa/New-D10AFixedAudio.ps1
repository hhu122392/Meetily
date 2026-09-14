[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string]$OutputPath,

    [Parameter(Mandatory = $true)]
    [string]$TruthPath,

    [ValidateRange(20, 30)]
    [int]$DurationSeconds = 24,

    [ValidateRange(1, 10)]
    [int]$LeadingSilenceSeconds = 4
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$truth = [System.Text.Encoding]::UTF8.GetString(
    [System.Convert]::FromBase64String('TU9TUyDns7vnu5/pn7PpopHnm7Tph4flj6Pku6TvvIznvJblj7fkuIPkuInkuIDkuZ3vvIzmnY7mooXlnKjok53moaXkvJrorq7lrqTjgII=')
)
$fullOutput = [System.IO.Path]::GetFullPath($OutputPath)
$fullTruth = [System.IO.Path]::GetFullPath($TruthPath)
$outputDirectory = [System.IO.Path]::GetDirectoryName($fullOutput)
[System.IO.Directory]::CreateDirectory($outputDirectory) | Out-Null
[System.IO.Directory]::CreateDirectory([System.IO.Path]::GetDirectoryName($fullTruth)) | Out-Null

$utf8NoBom = [System.Text.UTF8Encoding]::new($false)
[System.IO.File]::WriteAllText($fullTruth, $truth + "`n", $utf8NoBom)

$temporarySpeech = Join-Path $outputDirectory 'd10a-speech-only.tmp.wav'
try {
    Add-Type -AssemblyName System.Speech
    $format = [System.Speech.AudioFormat.SpeechAudioFormatInfo]::new(
        16000,
        [System.Speech.AudioFormat.AudioBitsPerSample]::Sixteen,
        [System.Speech.AudioFormat.AudioChannel]::Mono
    )
    $synthesizer = [System.Speech.Synthesis.SpeechSynthesizer]::new()
    try {
        $voice = $synthesizer.GetInstalledVoices() |
            ForEach-Object { $_.VoiceInfo } |
            Where-Object { $_.Culture.Name -eq 'zh-CN' } |
            Select-Object -First 1
        if ($null -eq $voice) {
            throw 'No zh-CN Windows speech voice is installed.'
        }
        $synthesizer.SelectVoice($voice.Name)
        $synthesizer.Rate = -1
        $synthesizer.Volume = 100
        $synthesizer.SetOutputToWaveFile($temporarySpeech, $format)
        $synthesizer.Speak($truth)
        $synthesizer.SetOutputToNull()
        $selectedVoice = $voice.Name
    } finally {
        $synthesizer.Dispose()
    }

    $stream = [System.IO.File]::OpenRead($temporarySpeech)
    $reader = [System.IO.BinaryReader]::new($stream)
    try {
        $riff = [System.Text.Encoding]::ASCII.GetString($reader.ReadBytes(4))
        [void]$reader.ReadUInt32()
        $wave = [System.Text.Encoding]::ASCII.GetString($reader.ReadBytes(4))
        if ($riff -ne 'RIFF' -or $wave -ne 'WAVE') {
            throw 'Speech synthesizer output is not a RIFF/WAVE file.'
        }
        $formatTag = 0
        $channels = 0
        $sampleRate = 0
        $bitsPerSample = 0
        [byte[]]$speechPcm = @()
        while ($stream.Position -lt $stream.Length) {
            $chunkIdBytes = $reader.ReadBytes(4)
            if ($chunkIdBytes.Length -ne 4) { break }
            $chunkId = [System.Text.Encoding]::ASCII.GetString($chunkIdBytes)
            $chunkSize = $reader.ReadUInt32()
            if ($chunkId -eq 'fmt ') {
                $formatTag = $reader.ReadUInt16()
                $channels = $reader.ReadUInt16()
                $sampleRate = $reader.ReadUInt32()
                [void]$reader.ReadUInt32()
                [void]$reader.ReadUInt16()
                $bitsPerSample = $reader.ReadUInt16()
                $remaining = [int]$chunkSize - 16
                if ($remaining -gt 0) { [void]$reader.ReadBytes($remaining) }
            } elseif ($chunkId -eq 'data') {
                $speechPcm = $reader.ReadBytes([int]$chunkSize)
            } else {
                [void]$reader.ReadBytes([int]$chunkSize)
            }
            if (($chunkSize % 2) -eq 1 -and $stream.Position -lt $stream.Length) {
                [void]$reader.ReadByte()
            }
        }
    } finally {
        $reader.Dispose()
        $stream.Dispose()
    }

    if ($formatTag -ne 1 -or $channels -ne 1 -or $sampleRate -ne 16000 -or $bitsPerSample -ne 16) {
        throw "Unexpected speech WAV format: tag=$formatTag channels=$channels sampleRate=$sampleRate bits=$bitsPerSample"
    }
    $bytesPerFrame = 2
    $targetFrames = $DurationSeconds * $sampleRate
    $leadingFrames = $LeadingSilenceSeconds * $sampleRate
    $speechFrames = [int]($speechPcm.Length / $bytesPerFrame)
    if (($leadingFrames + $speechFrames) -gt $targetFrames) {
        throw "Synthesized speech ($speechFrames frames) does not fit the fixed $DurationSeconds second fixture."
    }
    $targetPcm = [byte[]]::new($targetFrames * $bytesPerFrame)
    [System.Buffer]::BlockCopy($speechPcm, 0, $targetPcm, $leadingFrames * $bytesPerFrame, $speechPcm.Length)

    $output = [System.IO.File]::Create($fullOutput)
    $writer = [System.IO.BinaryWriter]::new($output, [System.Text.Encoding]::ASCII, $false)
    try {
        $dataLength = $targetPcm.Length
        $writer.Write([System.Text.Encoding]::ASCII.GetBytes('RIFF'))
        $writer.Write([uint32](36 + $dataLength))
        $writer.Write([System.Text.Encoding]::ASCII.GetBytes('WAVE'))
        $writer.Write([System.Text.Encoding]::ASCII.GetBytes('fmt '))
        $writer.Write([uint32]16)
        $writer.Write([uint16]1)
        $writer.Write([uint16]1)
        $writer.Write([uint32]16000)
        $writer.Write([uint32]32000)
        $writer.Write([uint16]2)
        $writer.Write([uint16]16)
        $writer.Write([System.Text.Encoding]::ASCII.GetBytes('data'))
        $writer.Write([uint32]$dataLength)
        $writer.Write($targetPcm)
    } finally {
        $writer.Dispose()
    }

    $audioHash = (Get-FileHash -LiteralPath $fullOutput -Algorithm SHA256).Hash
    $truthHash = (Get-FileHash -LiteralPath $fullTruth -Algorithm SHA256).Hash
    [pscustomobject]@{
        schema_version = 1
        generated_at_utc = [DateTime]::UtcNow.ToString('o')
        audio_path = $fullOutput
        audio_sha256 = $audioHash
        duration_seconds = $DurationSeconds
        sample_rate_hz = $sampleRate
        channels = $channels
        sample_format = 'pcm_s16le'
        bits_per_sample = $bitsPerSample
        leading_silence_seconds = $LeadingSilenceSeconds
        trailing_silence_seconds = [Math]::Round(($targetFrames - $leadingFrames - $speechFrames) / $sampleRate, 6)
        spoken_frames = $speechFrames
        truth_path = $fullTruth
        truth_file_sha256 = $truthHash
        truth_text = $truth
        voice = $selectedVoice
        voice_rate = -1
        voice_volume = 100
    } | ConvertTo-Json -Depth 5
} finally {
    if (Test-Path -LiteralPath $temporarySpeech) {
        Remove-Item -LiteralPath $temporarySpeech -Force
    }
}

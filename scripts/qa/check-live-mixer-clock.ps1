# Read-only source extraction: compile the actual ring buffer, not a rewritten mixer.
# Logging macros are the only stand-ins. This is not a native-client or ASR test.
$ErrorActionPreference = 'Stop'
$repo = (Resolve-Path (Join-Path $PSScriptRoot '../..')).Path
$pipelinePath = Join-Path $repo 'frontend/src-tauri/src/audio/pipeline.rs'
$statePath = Join-Path $repo 'frontend/src-tauri/src/audio/recording_state.rs'
$source = Get-Content -LiteralPath $pipelinePath -Raw
$stateSource = Get-Content -LiteralPath $statePath -Raw
$ring = [regex]::Match($source, '(?s)struct AudioMixerRingBuffer \{.*?(?=/// Simple audio mixer)')
$device = [regex]::Match($stateSource, '(?s)pub enum DeviceType \{.*?\r?\n\}')
if (!$ring.Success -or !$device.Success) { throw 'Production source boundaries changed; update the diagnostic explicitly.' }
$prefix = @'
use std::collections::VecDeque;
macro_rules! debug { ($($tokens:tt)*) => {{}} }
macro_rules! info { ($($tokens:tt)*) => {{}} }
macro_rules! warn { ($($tokens:tt)*) => {{}} }
macro_rules! error { ($($tokens:tt)*) => {{}} }
'@
$tests = @'
fn collect(ring: &mut AudioMixerRingBuffer, output: &mut Vec<f32>) {
    while let Some((_, system)) = ring.extract_window() { output.extend(system); }
}

#[test]
fn continuous_two_routes_must_not_add_time() {
    let mut ring = AudioMixerRingBuffer::new(48_000, true, true);
    let mut output = Vec::new();
    // Both healthy devices supply exactly 120 seconds, in ordinary 10ms callbacks.
    // Match the production loop: try extracting after each received source chunk.
    for _ in 0..12_000 {
        ring.add_samples(DeviceType::Microphone, vec![1.0; 480]);
        collect(&mut ring, &mut output);
        ring.add_samples(DeviceType::System, vec![2.0; 480]);
        collect(&mut ring, &mut output);
    }
    let supplied = 12_000 * 480;
    let artificial_zeros = output.iter().filter(|&&v| v == 0.0).count();
    eprintln!("input_samples={supplied}; output_samples={}; output_seconds={}; system_padding_samples={artificial_zeros}; remaining_mic={}; remaining_system={}",
        output.len(), output.len() as f64 / 48_000.0, ring.mic_buffer.len(), ring.system_buffer.len());
    assert!(output.len() <= supplied, "continuous equal-rate input was stretched by premature padding");
    assert_eq!(artificial_zeros, 0, "continuous system audio acquired artificial silent gaps");
}

#[test]
fn continuous_system_only_preserves_full_windows() {
    let mut ring = AudioMixerRingBuffer::new(48_000, false, true);
    let mut output = Vec::new();
    for _ in 0..12_000 {
        ring.add_samples(DeviceType::System, vec![2.0; 480]);
        collect(&mut ring, &mut output);
    }
    assert_eq!(output.len(), 12_000 * 480);
    assert!(output.iter().all(|&value| value == 2.0));
}

#[test]
fn a_healthy_route_that_arrives_later_must_not_be_replaced_with_silence() {
    let mut ring = AudioMixerRingBuffer::new(48_000, true, true);
    let mut output = Vec::new();
    for _ in 0..60 {
        ring.add_samples(DeviceType::Microphone, vec![1.0; 480]);
    }
    collect(&mut ring, &mut output);
    assert!(
        output.is_empty(),
        "a full microphone window was emitted before the healthy system route arrived"
    );

    for _ in 0..60 {
        ring.add_samples(DeviceType::System, vec![2.0; 480]);
    }
    collect(&mut ring, &mut output);
    assert_eq!(output.len(), 28_800);
    assert!(output.iter().all(|&value| value == 2.0));
}

#[test]
fn microphone_only_does_not_wait_for_an_unconfigured_system_route() {
    let mut ring = AudioMixerRingBuffer::new(48_000, true, false);
    ring.add_samples(DeviceType::Microphone, vec![1.0; 28_800]);
    let (mic, system) = ring.extract_window().expect("microphone-only window");
    assert_eq!(mic, vec![1.0; 28_800]);
    assert_eq!(system, vec![0.0; 28_800]);
}

#[test]
fn supplied_timeline_silence_keeps_dual_route_recording_moving_and_recovers() {
    let mut ring = AudioMixerRingBuffer::new(48_000, true, true);
    ring.add_samples(DeviceType::Microphone, vec![1.0; 57_600]);
    ring.add_samples(DeviceType::System, vec![0.0; 28_800]);
    let (first_mic, first_system) = ring.extract_window().expect("muted window");
    assert!(first_mic.iter().all(|&sample| sample == 1.0));
    assert!(first_system.iter().all(|&sample| sample == 0.0));

    ring.add_samples(DeviceType::System, vec![2.0; 28_800]);
    let (second_mic, second_system) = ring.extract_window().expect("recovered window");
    assert!(second_mic.iter().all(|&sample| sample == 1.0));
    assert!(second_system.iter().all(|&sample| sample == 2.0));
}

#[test]
fn stopping_drains_a_short_tail_without_rounding_it_up_to_a_full_window() {
    let mut ring = AudioMixerRingBuffer::new(48_000, true, true);
    ring.add_samples(DeviceType::Microphone, vec![1.0; 1_000]);
    ring.add_samples(DeviceType::System, vec![2.0; 900]);
    assert!(ring.extract_window().is_none());

    let (mic, system) = ring.extract_tail_window().expect("short final window");
    assert_eq!(mic.len(), 1_000);
    assert_eq!(system.len(), 1_000);
    assert!(mic.iter().all(|&sample| sample == 1.0));
    assert!(system[..900].iter().all(|&sample| sample == 2.0));
    assert!(system[900..].iter().all(|&sample| sample == 0.0));
    assert!(ring.extract_tail_window().is_none());
}

#[test]
fn a_cutover_tail_can_be_drained_before_new_epoch_audio_is_added() {
    let mut ring = AudioMixerRingBuffer::new(48_000, true, true);
    ring.add_samples(DeviceType::Microphone, vec![1.0; 480]);
    ring.add_samples(DeviceType::System, vec![2.0; 480]);
    let (old_mic, old_system) = ring.extract_tail_window().expect("old epoch tail");
    assert_eq!(old_mic, vec![1.0; 480]);
    assert_eq!(old_system, vec![2.0; 480]);

    ring.add_samples(DeviceType::Microphone, vec![3.0; 28_800]);
    ring.add_samples(DeviceType::System, vec![4.0; 28_800]);
    let (new_mic, new_system) = ring.extract_window().expect("new epoch window");
    assert!(new_mic.iter().all(|&sample| sample == 3.0));
    assert!(new_system.iter().all(|&sample| sample == 4.0));
}
'@
$tempDir = Join-Path ([IO.Path]::GetTempPath()) ('meetily-mixer-clock-' + [guid]::NewGuid().ToString('N'))
New-Item -Path $tempDir -ItemType Directory | Out-Null
$testExe = Join-Path $tempDir 'mixer-clock-test.exe'
Get-FileHash -LiteralPath $pipelinePath, $statePath -Algorithm SHA256 | Format-List
$unit = $prefix + "`n" + $device.Value + "`n" + $ring.Value + "`n" + $tests
$unit | & (Join-Path $env:USERPROFILE '.cargo/bin/rustc.exe') --edition=2021 --test --crate-name live_mixer_clock - -o $testExe
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
& $testExe --nocapture --test-threads=1
exit $LASTEXITCODE

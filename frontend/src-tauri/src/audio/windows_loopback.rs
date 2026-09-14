use anyhow::{anyhow, Context, Result};
use log::{error, info, warn};
use std::ffi::c_void;
use std::ptr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};
use windows::core::{Interface, PCWSTR};
use windows::Win32::Foundation::{
    CloseHandle, HANDLE, RPC_E_CHANGED_MODE, WAIT_FAILED, WAIT_OBJECT_0, WAIT_TIMEOUT,
};
use windows::Win32::Media::Audio::Endpoints::IAudioEndpointVolume;
use windows::Win32::Media::Audio::{
    eRender, IAudioCaptureClient, IAudioClient, IMMDeviceEnumerator, IMMEndpoint,
    MMDeviceEnumerator, AUDCLNT_BUFFERFLAGS_SILENT, AUDCLNT_SHAREMODE_SHARED,
    AUDCLNT_STREAMFLAGS_EVENTCALLBACK, AUDCLNT_STREAMFLAGS_LOOPBACK, AUDCLNT_S_BUFFER_EMPTY,
    WAVEFORMATEX, WAVEFORMATEXTENSIBLE, WAVE_FORMAT_PCM,
};
use windows::Win32::Media::KernelStreaming::{KSDATAFORMAT_SUBTYPE_PCM, WAVE_FORMAT_EXTENSIBLE};
use windows::Win32::Media::Multimedia::{KSDATAFORMAT_SUBTYPE_IEEE_FLOAT, WAVE_FORMAT_IEEE_FLOAT};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize, CLSCTX_ALL,
    COINIT_MULTITHREADED,
};
use windows::Win32::System::Performance::{QueryPerformanceCounter, QueryPerformanceFrequency};
use windows::Win32::System::Threading::{CreateEventW, WaitForSingleObject};

use super::devices::AudioDevice;
use super::pipeline::AudioCapture;
use super::recording_state::{
    AudioChunk, AudioError, DeviceType, RecordingState, SystemAudioFormat,
};

const EVENT_WAIT_MS: u32 = 50;
const STARTUP_DEADLINE: Duration = Duration::from_secs(8);

fn missing_idle_frames(written_frames: u64, seconds: f64, sample_rate: u32) -> usize {
    let expected = (seconds.max(0.0) * sample_rate as f64).round() as u64;
    let missing = expected.saturating_sub(written_frames);
    // Ignore sub-5ms packet clock jitter, rather than adding tiny gaps per callback.
    if missing <= (sample_rate / 200) as u64 {
        0
    } else {
        missing as usize
    }
}

fn fill_idle_time(capture: &AudioCapture, written: &mut u64, seconds: f64, rate: u32) {
    let mut missing = missing_idle_frames(*written, seconds, rate);
    // Bound allocations and retain the existing resampler/channel. These zeros
    // are timeline padding, not native packets or evidence of received sound.
    while missing > 0 {
        let frames = missing.min((rate / 20).max(1) as usize);
        if !capture.process_timeline_silence(frames) {
            break;
        }
        *written += frames as u64;
        missing -= frames;
    }
}

#[derive(Clone, Copy, Debug)]
enum NativeSampleFormat {
    Float32,
    Unsigned8,
    SignedPcm {
        container_bytes: usize,
        valid_bits: u16,
    },
}

#[derive(Clone, Debug)]
struct NativeStreamFormat {
    sample_rate: u32,
    channels: u16,
    bits_per_sample: u16,
    block_align: u16,
    sample_format: NativeSampleFormat,
}

impl NativeStreamFormat {
    unsafe fn from_wave_format(format: *const WAVEFORMATEX) -> Result<Self> {
        if format.is_null() {
            return Err(anyhow!("WASAPI returned a null mix format"));
        }
        let base = ptr::read_unaligned(format);
        let format_tag = base.wFormatTag;
        let channels = base.nChannels;
        let sample_rate = base.nSamplesPerSec;
        let block_align = base.nBlockAlign;
        let bits_per_sample = base.wBitsPerSample;
        let extra_size = base.cbSize;
        if sample_rate == 0 || channels == 0 || block_align == 0 {
            return Err(anyhow!(
                "WASAPI returned an invalid mix format: rate={}, channels={}, block_align={}",
                sample_rate,
                channels,
                block_align
            ));
        }
        if block_align as usize % channels as usize != 0 {
            return Err(anyhow!(
                "WASAPI mix format block alignment is not divisible by channel count"
            ));
        }

        let bytes_per_sample = block_align as usize / channels as usize;
        let (sub_format, valid_bits) = if format_tag as u32 == WAVE_FORMAT_EXTENSIBLE {
            if extra_size
                < (std::mem::size_of::<WAVEFORMATEXTENSIBLE>()
                    - std::mem::size_of::<WAVEFORMATEX>()) as u16
            {
                return Err(anyhow!("WASAPI returned a truncated extensible mix format"));
            }
            let extended = ptr::read_unaligned(format.cast::<WAVEFORMATEXTENSIBLE>());
            let valid_bits = extended.Samples.wValidBitsPerSample;
            (Some(extended.SubFormat), valid_bits)
        } else {
            (None, bits_per_sample)
        };

        let sample_format = if (format_tag as u32 == WAVE_FORMAT_IEEE_FLOAT
            || sub_format == Some(KSDATAFORMAT_SUBTYPE_IEEE_FLOAT))
            && bits_per_sample == 32
            && bytes_per_sample == 4
        {
            NativeSampleFormat::Float32
        } else if format_tag as u32 == WAVE_FORMAT_PCM
            || sub_format == Some(KSDATAFORMAT_SUBTYPE_PCM)
        {
            if bytes_per_sample == 1 && bits_per_sample == 8 {
                NativeSampleFormat::Unsigned8
            } else if (2..=4).contains(&bytes_per_sample)
                && valid_bits > 0
                && valid_bits as usize <= bytes_per_sample * 8
            {
                NativeSampleFormat::SignedPcm {
                    container_bytes: bytes_per_sample,
                    valid_bits,
                }
            } else {
                return Err(anyhow!(
                    "Unsupported WASAPI PCM mix format: container_bytes={}, bits={}, valid_bits={}",
                    bytes_per_sample,
                    bits_per_sample,
                    valid_bits
                ));
            }
        } else {
            return Err(anyhow!(
                "Unsupported WASAPI mix format: tag={}, bits={}, bytes_per_sample={}",
                format_tag,
                bits_per_sample,
                bytes_per_sample
            ));
        };

        Ok(Self {
            sample_rate,
            channels,
            bits_per_sample,
            block_align,
            sample_format,
        })
    }

    fn public_format(&self) -> SystemAudioFormat {
        SystemAudioFormat {
            sample_rate: self.sample_rate,
            channels: self.channels,
            bits_per_sample: self.bits_per_sample,
            block_align: self.block_align,
            sample_format: match self.sample_format {
                NativeSampleFormat::Float32 => "f32".to_string(),
                NativeSampleFormat::Unsigned8 => "u8_pcm".to_string(),
                NativeSampleFormat::SignedPcm { valid_bits, .. } => {
                    format!("i{valid_bits}_pcm")
                }
            },
        }
    }

    unsafe fn convert_packet(
        &self,
        data: *const u8,
        frames: u32,
        silent: bool,
    ) -> Result<Vec<f32>> {
        let sample_count = frames as usize * self.channels as usize;
        if silent {
            return Ok(vec![0.0; sample_count]);
        }
        if data.is_null() {
            return Err(anyhow!(
                "WASAPI returned a null packet without AUDCLNT_BUFFERFLAGS_SILENT"
            ));
        }
        let byte_count = frames as usize * self.block_align as usize;
        let bytes = std::slice::from_raw_parts(data, byte_count);
        let bytes_per_sample = self.block_align as usize / self.channels as usize;
        let mut output = Vec::with_capacity(sample_count);
        for frame in 0..frames as usize {
            let frame_start = frame * self.block_align as usize;
            for channel in 0..self.channels as usize {
                let offset = frame_start + channel * bytes_per_sample;
                let sample = match self.sample_format {
                    NativeSampleFormat::Float32 => f32::from_le_bytes(
                        bytes[offset..offset + 4]
                            .try_into()
                            .expect("validated f32 sample width"),
                    ),
                    NativeSampleFormat::Unsigned8 => (bytes[offset] as f32 - 128.0) / 128.0,
                    NativeSampleFormat::SignedPcm {
                        container_bytes,
                        valid_bits,
                    } => signed_pcm_to_f32(&bytes[offset..offset + container_bytes], valid_bits),
                };
                output.push(sample.clamp(-1.0, 1.0));
            }
        }
        Ok(output)
    }
}

fn signed_pcm_to_f32(bytes: &[u8], valid_bits: u16) -> f32 {
    let mut raw = match bytes.len() {
        2 => i16::from_le_bytes([bytes[0], bytes[1]]) as i32,
        3 => {
            let packed = bytes[0] as i32 | (bytes[1] as i32) << 8 | (bytes[2] as i32) << 16;
            if packed & 0x0080_0000 != 0 {
                packed | !0x00ff_ffff
            } else {
                packed
            }
        }
        4 => i32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]),
        _ => unreachable!("validated PCM container width"),
    };
    let container_bits = (bytes.len() * 8) as u16;
    if valid_bits < container_bits {
        raw >>= container_bits - valid_bits;
    }
    let scale = (1_u64 << (valid_bits - 1)) as f32;
    raw as f32 / scale
}

fn count_all_zero_frames(samples: &[f32], channels: u16) -> u32 {
    samples
        .chunks_exact(channels as usize)
        .filter(|frame| frame.iter().all(|sample| sample.abs() <= f32::EPSILON))
        .count() as u32
}

pub(crate) fn qpc_now_ns() -> Result<u64> {
    let mut counter = 0_i64;
    let mut frequency = 0_i64;
    unsafe {
        QueryPerformanceCounter(&mut counter).context("QueryPerformanceCounter failed")?;
        QueryPerformanceFrequency(&mut frequency).context("QueryPerformanceFrequency failed")?;
    }
    if counter < 0 || frequency <= 0 {
        return Err(anyhow!(
            "Invalid QPC values: counter={}, frequency={}",
            counter,
            frequency
        ));
    }
    Ok(((counter as u128 * 1_000_000_000_u128) / frequency as u128) as u64)
}

struct ComApartment {
    uninitialize: bool,
}

impl ComApartment {
    fn initialize() -> Result<Self> {
        let result = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
        if result.is_ok() {
            Ok(Self { uninitialize: true })
        } else if result == RPC_E_CHANGED_MODE {
            Ok(Self {
                uninitialize: false,
            })
        } else {
            Err(anyhow!(
                "CoInitializeEx failed with HRESULT 0x{:08X}",
                result.0 as u32
            ))
        }
    }
}

impl Drop for ComApartment {
    fn drop(&mut self) {
        if self.uninitialize {
            unsafe { CoUninitialize() };
        }
    }
}

struct WaveFormatMemory(*mut WAVEFORMATEX);

impl Drop for WaveFormatMemory {
    fn drop(&mut self) {
        unsafe { CoTaskMemFree(Some(self.0.cast::<c_void>())) };
    }
}

struct EventHandle(HANDLE);

impl Drop for EventHandle {
    fn drop(&mut self) {
        if let Err(error) = unsafe { CloseHandle(self.0) } {
            warn!("Failed to close WASAPI event handle: {error}");
        }
    }
}

pub struct WindowsLoopbackStream {
    stop_requested: Arc<AtomicBool>,
    thread: Option<JoinHandle<Result<()>>>,
}

impl WindowsLoopbackStream {
    pub async fn start(
        device: Arc<AudioDevice>,
        state: Arc<RecordingState>,
        recording_sender: Option<mpsc::UnboundedSender<AudioChunk>>,
    ) -> Result<Self> {
        let endpoint_id = device.native_id.clone().ok_or_else(|| {
            anyhow!(
                "Windows system audio selection '{}' has no native endpoint ID",
                device.name
            )
        })?;
        let stop_requested = Arc::new(AtomicBool::new(false));
        let thread_stop = stop_requested.clone();
        let thread_state = state.clone();
        let (startup_sender, startup_receiver) = oneshot::channel::<Result<(), String>>();
        let thread_name = format!("wasapi-loopback-{}", state.get_device_epoch());
        let thread = std::thread::Builder::new()
            .name(thread_name)
            .spawn(move || {
                let mut startup_sender = Some(startup_sender);
                let result = run_loopback_capture(
                    &endpoint_id,
                    device,
                    thread_state.clone(),
                    recording_sender,
                    thread_stop,
                    &mut startup_sender,
                );
                if let Err(error) = &result {
                    if let Some(sender) = startup_sender.take() {
                        let _ = sender.send(Err(error.to_string()));
                    } else if thread_state.is_recording() {
                        error!("Native WASAPI loopback stream failed after startup: {error:#}");
                        thread_state.report_route_stream_error(
                            &DeviceType::System,
                            AudioError::StreamFailed,
                        );
                    }
                }
                result
            })
            .context("Failed to create native WASAPI loopback thread")?;

        match tokio::time::timeout(STARTUP_DEADLINE, startup_receiver).await {
            Ok(Ok(Ok(()))) => Ok(Self {
                stop_requested,
                thread: Some(thread),
            }),
            Ok(Ok(Err(message))) => {
                stop_requested.store(true, Ordering::SeqCst);
                let _ = thread.join();
                Err(anyhow!(message))
            }
            Ok(Err(_)) => {
                stop_requested.store(true, Ordering::SeqCst);
                let _ = thread.join();
                Err(anyhow!(
                    "Native WASAPI loopback thread ended before reporting startup"
                ))
            }
            Err(_) => {
                stop_requested.store(true, Ordering::SeqCst);
                Err(anyhow!(
                    "Native WASAPI loopback initialization exceeded 8 seconds"
                ))
            }
        }
    }

    pub fn stop(mut self) -> Result<()> {
        self.stop_requested.store(true, Ordering::SeqCst);
        if let Some(thread) = self.thread.take() {
            match thread.join() {
                Ok(Ok(())) => Ok(()),
                Ok(Err(error)) => {
                    warn!("Native WASAPI loopback had already ended with an error: {error:#}");
                    Ok(())
                }
                Err(_) => Err(anyhow!("Native WASAPI loopback thread panicked")),
            }
        } else {
            Ok(())
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn run_loopback_capture(
    endpoint_id: &str,
    device: Arc<AudioDevice>,
    state: Arc<RecordingState>,
    recording_sender: Option<mpsc::UnboundedSender<AudioChunk>>,
    stop_requested: Arc<AtomicBool>,
    startup_sender: &mut Option<oneshot::Sender<Result<(), String>>>,
) -> Result<()> {
    let _com = ComApartment::initialize()?;
    unsafe {
        let enumerator: IMMDeviceEnumerator =
            CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)
                .context("Failed to create MMDeviceEnumerator")?;
        let endpoint_wide: Vec<u16> = endpoint_id.encode_utf16().chain(Some(0)).collect();
        let endpoint = enumerator
            .GetDevice(PCWSTR(endpoint_wide.as_ptr()))
            .with_context(|| format!("Windows system endpoint is unavailable: {endpoint_id}"))?;
        let flow = endpoint
            .cast::<IMMEndpoint>()
            .and_then(|endpoint| endpoint.GetDataFlow())
            .context("Failed to read Windows system endpoint flow")?;
        if flow != eRender {
            return Err(anyhow!(
                "Windows system endpoint is not a render endpoint: {endpoint_id}"
            ));
        }

        let audio_client: IAudioClient = endpoint
            .Activate(CLSCTX_ALL, None)
            .context("Failed to activate IAudioClient for selected Windows endpoint")?;
        let endpoint_volume: IAudioEndpointVolume = endpoint
            .Activate(CLSCTX_ALL, None)
            .context("Failed to activate endpoint volume for selected Windows endpoint")?;
        let mix_format = WaveFormatMemory(
            audio_client
                .GetMixFormat()
                .context("Failed to read selected Windows endpoint mix format")?,
        );
        let native_format = NativeStreamFormat::from_wave_format(mix_format.0)?;
        audio_client
            .Initialize(
                AUDCLNT_SHAREMODE_SHARED,
                AUDCLNT_STREAMFLAGS_EVENTCALLBACK | AUDCLNT_STREAMFLAGS_LOOPBACK,
                0,
                0,
                mix_format.0,
                None,
            )
            .context("Failed to initialize native WASAPI loopback capture")?;
        drop(mix_format);

        let event = EventHandle(
            CreateEventW(None, false, false, PCWSTR::null())
                .context("Failed to create native WASAPI callback event")?,
        );
        audio_client
            .SetEventHandle(event.0)
            .context("Failed to bind native WASAPI callback event")?;
        let capture_client: IAudioCaptureClient = audio_client
            .GetService()
            .context("Failed to obtain IAudioCaptureClient")?;
        let capture = AudioCapture::new(
            device.clone(),
            state.clone(),
            native_format.sample_rate,
            native_format.channels,
            DeviceType::System,
            recording_sender,
        );
        let bound_epoch = state.get_device_epoch();
        state.set_system_audio_format(native_format.public_format());

        audio_client
            .Start()
            .context("Failed to start native WASAPI loopback capture")?;
        let loop_result = (|| -> Result<()> {
            let started_qpc_ns = qpc_now_ns()?;
            state.mark_system_stream_started(started_qpc_ns);
            info!(
                "Native WASAPI loopback started by endpoint ID: {} ({}) rate={} channels={} bits={} block_align={} qpc_ns={}",
                endpoint_id,
                device.name,
                native_format.sample_rate,
                native_format.channels,
                native_format.bits_per_sample,
                native_format.block_align,
                started_qpc_ns
            );

            // An idle Windows render endpoint does not have to deliver packets.
            // Reaching IAudioClient::Start means the selected endpoint is ready;
            // actual stream failures are still reported by the capture thread.
            if let Some(sender) = startup_sender.take() {
                let _ = sender.send(Ok(()));
            }

            capture_loop(
                &audio_client,
                &capture_client,
                &endpoint_volume,
                &capture,
                &native_format,
                &state,
                &stop_requested,
                &event,
                bound_epoch,
            )
        })();
        if let Err(error) = audio_client.Stop() {
            warn!("Failed to stop native WASAPI loopback client: {error}");
            if loop_result.is_ok() {
                return Err(anyhow!(
                    "Failed to stop native WASAPI loopback client: {error}"
                ));
            }
        }
        loop_result
    }
}

#[allow(clippy::too_many_arguments)]
unsafe fn capture_loop(
    _audio_client: &IAudioClient,
    capture_client: &IAudioCaptureClient,
    endpoint_volume: &IAudioEndpointVolume,
    capture: &AudioCapture,
    format: &NativeStreamFormat,
    state: &Arc<RecordingState>,
    stop_requested: &AtomicBool,
    event: &EventHandle,
    bound_epoch: u64,
) -> Result<()> {
    let mut clock = state.get_active_recording_duration().unwrap_or(0.0);
    let mut active_seconds = 0.0;
    let mut written_frames = 0_u64;
    while !stop_requested.load(Ordering::SeqCst)
        && state.is_recording()
        && state.get_device_epoch() == bound_epoch
    {
        let wait_result = WaitForSingleObject(event.0, EVENT_WAIT_MS);
        if wait_result == WAIT_FAILED {
            return Err(anyhow!(
                "WaitForSingleObject failed for native WASAPI callback"
            ));
        }
        if wait_result != WAIT_OBJECT_0 && wait_result != WAIT_TIMEOUT {
            return Err(anyhow!(
                "Unexpected native WASAPI wait result: {}",
                wait_result.0
            ));
        }

        let now = state.get_active_recording_duration().unwrap_or(clock);
        let accepting = state.accepts_audio_timeline();
        // Pauses are already excluded by RecordingState; readiness failures and
        // reconnect waits must not be silently filled after the route recovers.
        if accepting {
            active_seconds += (now - clock).max(0.0);
        }
        clock = now;

        if wait_result == WAIT_OBJECT_0 {
            loop {
                let packet_frames = capture_client
                    .GetNextPacketSize()
                    .context("IAudioCaptureClient::GetNextPacketSize failed")?;
                if packet_frames == 0 {
                    break;
                }

                let mut data = ptr::null_mut();
                let mut frames = 0_u32;
                let mut flags = 0_u32;
                let mut device_position = 0_u64;
                let mut qpc_position_100ns = 0_u64;
                let get_result = capture_client.GetBuffer(
                    &mut data,
                    &mut frames,
                    &mut flags,
                    Some(&mut device_position),
                    Some(&mut qpc_position_100ns),
                );
                if let Err(error) = get_result {
                    if error.code() == AUDCLNT_S_BUFFER_EMPTY {
                        continue;
                    }
                    return Err(anyhow!("IAudioCaptureClient::GetBuffer failed: {error}"));
                }

                let silent = flags & AUDCLNT_BUFFERFLAGS_SILENT.0 as u32 != 0;
                let converted = format.convert_packet(data.cast_const(), frames, silent);
                let release_result = capture_client.ReleaseBuffer(frames);
                let samples = converted?;
                release_result.context("IAudioCaptureClient::ReleaseBuffer failed")?;

                let capture_qpc_ns = qpc_position_100ns
                    .checked_mul(100)
                    .ok_or_else(|| anyhow!("WASAPI packet QPC timestamp overflowed"))?;
                let observed_qpc_ns = qpc_now_ns()?;
                let all_zero_frames = count_all_zero_frames(&samples, format.channels);
                let Some(callback_guard) =
                    state.begin_audio_callback(bound_epoch, Some(capture_qpc_ns), frames as u64)
                else {
                    continue;
                };
                let endpoint_muted = endpoint_volume
                    .GetMute()
                    .context("Failed to read selected Windows endpoint mute state")?
                    .as_bool();
                state.record_system_callback_qpc(
                    capture_qpc_ns,
                    observed_qpc_ns,
                    frames,
                    silent,
                    all_zero_frames,
                    endpoint_muted,
                );
                if accepting {
                    let packet_age = observed_qpc_ns.saturating_sub(capture_qpc_ns) as f64 / 1e9;
                    fill_idle_time(
                        capture,
                        &mut written_frames,
                        active_seconds - packet_age,
                        format.sample_rate,
                    );
                    written_frames += frames as u64;
                }
                capture.process_audio_data_with_guard(
                    &samples,
                    Some(capture_qpc_ns),
                    callback_guard,
                );
            }
        }
        if accepting {
            // Read all queued packets first. Keep 100ms behind the clock so a
            // normal packet isn't replaced by padding while it is being delivered.
            fill_idle_time(
                capture,
                &mut written_frames,
                active_seconds - 0.1,
                format.sample_rate,
            );
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn idle_loopback_keeps_initial_middle_and_final_silence() {
        let rate = 48_000;
        let mut audio = Vec::new();
        audio.resize(missing_idle_frames(0, 2.0, rate), 0.0);
        assert_eq!(audio.len(), 96_000, "initial idle time must not disappear");
        audio.extend(vec![0.25; rate as usize]);
        audio.extend(vec![
            0.0;
            missing_idle_frames(audio.len() as u64, 5.0, rate)
        ]);
        audio.extend(vec![0.5; rate as usize]);
        audio.extend(vec![
            0.0;
            missing_idle_frames(audio.len() as u64, 8.0, rate)
        ]);
        assert_eq!(audio.len(), 384_000);
        assert!(audio[..96_000].iter().all(|x| *x == 0.0));
        assert!(audio[96_000..144_000].iter().all(|x| *x == 0.25));
        assert!(audio[144_000..240_000].iter().all(|x| *x == 0.0));
        assert!(audio[240_000..288_000].iter().all(|x| *x == 0.5));
        assert!(audio[288_000..].iter().all(|x| *x == 0.0));
    }

    #[test]
    fn idle_loopback_does_not_repeat_received_silent_packets() {
        assert_eq!(missing_idle_frames(48_000, 1.0, 48_000), 0);
        assert_eq!(missing_idle_frames(48_000, 0.9, 48_000), 0);
        assert_eq!(missing_idle_frames(48_000, 1.002, 48_000), 0);
        assert_eq!(missing_idle_frames(48_000, 1.05, 48_000), 2_400);
    }

    #[test]
    fn idle_loopback_padding_reaches_pipeline_without_faking_native_metrics() {
        let state = RecordingState::new();
        state.start_recording().unwrap();
        state.mark_routes_ready();
        let (sender, mut receiver) = mpsc::unbounded_channel();
        state.set_audio_sender(sender);
        let device = Arc::new(AudioDevice::new(
            "test render".into(),
            super::super::devices::DeviceType::Output,
        ));
        let capture = AudioCapture::new(device, state.clone(), 48_000, 2, DeviceType::System, None);
        let mut written = 0;
        fill_idle_time(&capture, &mut written, 2.0, 48_000);
        capture.process_audio_data(&vec![0.25; 96_000]);
        written += 48_000;
        fill_idle_time(&capture, &mut written, 5.0, 48_000);
        capture.process_audio_data(&vec![0.5; 96_000]);
        written += 48_000;
        fill_idle_time(&capture, &mut written, 8.0, 48_000);
        let mut audio = Vec::new();
        while let Ok(chunk) = receiver.try_recv() {
            assert_eq!(chunk.sample_rate, 48_000);
            audio.extend(chunk.data);
        }
        assert_eq!(audio.len(), 384_000);
        assert!(audio[..96_000].iter().all(|x| *x == 0.0));
        assert!(audio[96_000..144_000].iter().all(|x| *x == 0.25));
        assert!(audio[144_000..240_000].iter().all(|x| *x == 0.0));
        assert!(audio[240_000..288_000].iter().all(|x| *x == 0.5));
        assert!(audio[288_000..].iter().all(|x| *x == 0.0));
        assert_eq!(
            state.route_callback_counts(&DeviceType::System),
            (2, 192_000)
        );
        assert_eq!(state.route_audio_levels(&DeviceType::System), (0.5, 0.5));
        assert_eq!(state.route_callback_counts(&DeviceType::Microphone), (0, 0));
    }

    #[test]
    fn idle_loopback_padding_stops_for_pause_fault_and_old_epoch() {
        let state = RecordingState::new();
        state.start_recording().unwrap();
        state.mark_routes_ready();
        let (sender, mut receiver) = mpsc::unbounded_channel();
        state.set_audio_sender(sender);
        let device = Arc::new(AudioDevice::new(
            "test render".into(),
            super::super::devices::DeviceType::Output,
        ));
        let capture = AudioCapture::new(device, state.clone(), 48_000, 2, DeviceType::System, None);
        state.pause_recording().unwrap();
        assert!(!capture.process_timeline_silence(480));
        state.resume_recording().unwrap();
        assert!(capture.process_timeline_silence(480));
        let chunk = receiver.try_recv().unwrap();
        assert_eq!(chunk.data, vec![0.0; 480]);
        assert_eq!(chunk.capture_qpc_ns, None);
        state.advance_device_epoch();
        assert!(!capture.process_timeline_silence(480));
        let device = Arc::new(AudioDevice::new(
            "test render rebound".into(),
            super::super::devices::DeviceType::Output,
        ));
        let rebound = AudioCapture::new(device, state.clone(), 48_000, 2, DeviceType::System, None);
        state.mark_routes_ready();
        state.report_route_stream_error(&DeviceType::System, AudioError::StreamFailed);
        assert!(!rebound.process_timeline_silence(480));
        assert!(receiver.try_recv().is_err());
        assert_eq!(state.route_callback_counts(&DeviceType::System), (0, 0));
    }

    #[test]
    fn idle_loopback_padding_cannot_cross_a_device_cutover_watermark() {
        let state = RecordingState::new();
        state.start_recording().unwrap();
        state.mark_routes_ready();
        let (sender, mut receiver) = mpsc::unbounded_channel();
        state.set_audio_sender(sender);
        let device = Arc::new(AudioDevice::new(
            "test render".into(),
            super::super::devices::DeviceType::Output,
        ));
        let capture = AudioCapture::new(device, state.clone(), 48_000, 2, DeviceType::System, None);
        state.begin_device_cutover(1_000);
        assert!(!capture.process_timeline_silence(480));
        assert!(receiver.try_recv().is_err());
        assert_eq!(state.callback_drain_state().0, 0);
        assert_eq!(state.route_callback_counts(&DeviceType::System), (0, 0));
    }

    #[test]
    fn signed_pcm_conversion_handles_16_and_24_bit_edges() {
        assert_eq!(signed_pcm_to_f32(&[0x00, 0x80], 16), -1.0);
        assert!(signed_pcm_to_f32(&[0xff, 0x7f], 16) > 0.999);
        assert_eq!(signed_pcm_to_f32(&[0x00, 0x00, 0x80], 24), -1.0);
        assert!(signed_pcm_to_f32(&[0xff, 0xff, 0x7f], 24) > 0.999);
    }

    #[test]
    fn all_zero_frame_count_is_per_frame_not_per_sample() {
        let samples = [0.0, 0.0, 0.0, 0.25, f32::EPSILON, -f32::EPSILON];
        assert_eq!(count_all_zero_frames(&samples, 2), 2);
    }
}

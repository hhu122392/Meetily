use anyhow::Result;
use cpal::traits::{DeviceTrait, StreamTrait};
use cpal::{Device, Stream, SupportedStreamConfig};
use log::{error, info, warn};
use std::sync::Arc;
use tokio::sync::mpsc;

#[cfg(target_os = "windows")]
struct WindowsMicrophoneWatchdog {
    stop_requested: Arc<std::sync::atomic::AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

#[cfg(target_os = "windows")]
impl WindowsMicrophoneWatchdog {
    fn start(state: Arc<RecordingState>, bound_epoch: u64) -> Result<Self> {
        use std::sync::atomic::Ordering;

        let stop_requested = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let thread_stop = stop_requested.clone();
        let thread = std::thread::Builder::new()
            .name(format!("wasapi-microphone-watchdog-{bound_epoch}"))
            .spawn(move || {
                let mut monitoring_started_qpc_ns = 0_u64;
                let mut last_seen_callback_qpc_ns = 0_u64;
                let mut no_signal_reported = false;

                while !thread_stop.load(Ordering::SeqCst)
                    && state.is_recording()
                    && state.get_device_epoch() == bound_epoch
                {
                    std::thread::sleep(std::time::Duration::from_millis(50));
                    let now_qpc_ns = match super::windows_loopback::qpc_now_ns() {
                        Ok(value) => value,
                        Err(error) => {
                            error!("Microphone callback watchdog QPC read failed: {error:#}");
                            state.report_route_stream_error(
                                &DeviceType::Microphone,
                                super::recording_state::AudioError::StreamFailed,
                            );
                            break;
                        }
                    };

                    if !state.route_watchdog_enabled() {
                        monitoring_started_qpc_ns = 0;
                        last_seen_callback_qpc_ns = 0;
                        no_signal_reported = false;
                        continue;
                    }
                    if monitoring_started_qpc_ns == 0 {
                        monitoring_started_qpc_ns = now_qpc_ns;
                    }

                    let (_, observed_qpc_ns, _, _, _) = state.microphone_route_health();
                    if let Some(observed_qpc_ns) = observed_qpc_ns {
                        if observed_qpc_ns != last_seen_callback_qpc_ns {
                            last_seen_callback_qpc_ns = observed_qpc_ns;
                            no_signal_reported = false;
                        }
                    }
                    let reference_qpc_ns = observed_qpc_ns
                        .filter(|observed| *observed >= monitoring_started_qpc_ns)
                        .unwrap_or(monitoring_started_qpc_ns);
                    let gap_ns = now_qpc_ns.saturating_sub(reference_qpc_ns);
                    if gap_ns >= super::recording_state::AUDIO_CALLBACK_DEADLINE_NS
                        && !no_signal_reported
                    {
                        state.report_microphone_no_signal(gap_ns);
                        no_signal_reported = true;
                    }
                }
            })?;

        Ok(Self {
            stop_requested,
            thread: Some(thread),
        })
    }

    fn stop(mut self) -> Result<()> {
        use std::sync::atomic::Ordering;

        self.stop_requested.store(true, Ordering::SeqCst);
        if let Some(thread) = self.thread.take() {
            thread
                .join()
                .map_err(|_| anyhow::anyhow!("Microphone callback watchdog thread panicked"))?;
        }
        Ok(())
    }
}

#[cfg(target_os = "windows")]
fn capture_qpc_ns(info: &cpal::InputCallbackInfo) -> Option<u64> {
    use windows::Win32::System::Performance::{QueryPerformanceCounter, QueryPerformanceFrequency};

    let mut counter = 0_i64;
    let mut frequency = 0_i64;
    unsafe {
        QueryPerformanceCounter(&mut counter).ok()?;
        QueryPerformanceFrequency(&mut frequency).ok()?;
    }
    if counter < 0 || frequency <= 0 {
        return None;
    }
    let now_ns = (counter as u128)
        .saturating_mul(1_000_000_000)
        .checked_div(frequency as u128)? as u64;
    let timestamp = info.timestamp();
    let capture_latency_ns = timestamp
        .callback
        .duration_since(&timestamp.capture)
        .map(|duration| duration.as_nanos() as u64)
        .unwrap_or(0);
    Some(now_ns.saturating_sub(capture_latency_ns))
}

#[cfg(not(target_os = "windows"))]
fn capture_qpc_ns(_info: &cpal::InputCallbackInfo) -> Option<u64> {
    None
}

use super::capture::{get_current_backend, AudioCaptureBackend};
use super::devices::{get_device_and_config, AudioDevice};
use super::pipeline::AudioCapture;
use super::recording_state::{DeviceType, RecordingState};

#[cfg(target_os = "macos")]
use super::capture::CoreAudioCapture;

/// Stream backend implementation
pub enum StreamBackend {
    /// CPAL-based stream (ScreenCaptureKit or default)
    Cpal(Stream),
    /// Native endpoint-ID-bound WASAPI loopback (Windows system audio only).
    #[cfg(target_os = "windows")]
    WindowsLoopback(super::windows_loopback::WindowsLoopbackStream),
    /// Core Audio direct implementation (macOS only)
    #[cfg(target_os = "macos")]
    CoreAudio {
        task: Option<tokio::task::JoinHandle<()>>,
    },
}

// SAFETY: While Stream doesn't implement Send, we ensure it's only accessed
// from the same thread context by using spawn_blocking for operations that cross thread boundaries
unsafe impl Send for StreamBackend {}

struct PendingCpalStream(Stream);
// CPAL's WASAPI stream is owned by this task and is only moved, never accessed
// concurrently, while the first-callback deadline is awaited.
unsafe impl Send for PendingCpalStream {}

/// Simplified audio stream wrapper with multi-backend support
pub struct AudioStream {
    device: Arc<AudioDevice>,
    backend: StreamBackend,
    #[cfg(target_os = "windows")]
    microphone_watchdog: Option<WindowsMicrophoneWatchdog>,
}

// SAFETY: AudioStream contains StreamBackend which we've marked as Send
unsafe impl Send for AudioStream {}

impl AudioStream {
    /// Create a new audio stream for the given device
    pub async fn create(
        device: Arc<AudioDevice>,
        state: Arc<RecordingState>,
        device_type: DeviceType,
        recording_sender: Option<mpsc::UnboundedSender<super::recording_state::AudioChunk>>,
    ) -> Result<Self> {
        // Get current backend from global config
        let backend_type = get_current_backend();
        Self::create_with_backend(device, state, device_type, recording_sender, backend_type).await
    }

    /// Create a new audio stream with explicit backend selection
    pub async fn create_with_backend(
        device: Arc<AudioDevice>,
        state: Arc<RecordingState>,
        device_type: DeviceType,
        recording_sender: Option<mpsc::UnboundedSender<super::recording_state::AudioChunk>>,
        backend_type: AudioCaptureBackend,
    ) -> Result<Self> {
        info!(
            "🎵 Stream: Creating audio stream for device: {} with backend: {:?}, device_type: {:?}",
            device.name, backend_type, device_type
        );

        #[cfg(target_os = "windows")]
        if device_type == DeviceType::System {
            info!(
                "Stream: Using native WASAPI loopback for endpoint ID: {}",
                device.native_id.as_deref().unwrap_or("missing")
            );
            let native_stream = super::windows_loopback::WindowsLoopbackStream::start(
                device.clone(),
                state,
                recording_sender,
            )
            .await?;
            return Ok(Self {
                device,
                backend: StreamBackend::WindowsLoopback(native_stream),
                microphone_watchdog: None,
            });
        }

        // For system audio devices, use the selected backend
        // For microphone devices, always use CPAL
        #[cfg(target_os = "macos")]
        let use_core_audio =
            device_type == DeviceType::System && backend_type == AudioCaptureBackend::CoreAudio;

        #[cfg(not(target_os = "macos"))]
        let use_core_audio = false;

        #[cfg(target_os = "macos")]
        info!(
            "🎵 Stream: use_core_audio = {}, device_type == System: {}, backend == CoreAudio: {}",
            use_core_audio,
            device_type == DeviceType::System,
            backend_type == AudioCaptureBackend::CoreAudio
        );

        #[cfg(not(target_os = "macos"))]
        info!(
            "🎵 Stream: use_core_audio = {}, device_type == System: {}",
            use_core_audio,
            device_type == DeviceType::System
        );

        #[cfg(target_os = "macos")]
        if use_core_audio {
            info!("🎵 Stream: Using Core Audio backend (cidre) for system audio");
            return Self::create_core_audio_stream(device, state, device_type, recording_sender)
                .await;
        }

        // Default path: use CPAL
        #[cfg(target_os = "macos")]
        let backend_name = if backend_type == AudioCaptureBackend::ScreenCaptureKit {
            "ScreenCaptureKit"
        } else {
            "CPAL (default)"
        };

        #[cfg(not(target_os = "macos"))]
        let backend_name = "CPAL";

        info!(
            "🎵 Stream: Using CPAL backend ({}) for device: {}",
            backend_name, device.name
        );
        Self::create_cpal_stream(device, state, device_type, recording_sender).await
    }

    /// Create a CPAL-based stream (ScreenCaptureKit on macOS)
    async fn create_cpal_stream(
        device: Arc<AudioDevice>,
        state: Arc<RecordingState>,
        device_type: DeviceType,
        recording_sender: Option<mpsc::UnboundedSender<super::recording_state::AudioChunk>>,
    ) -> Result<Self> {
        info!("Creating CPAL stream for device: {}", device.name);

        // Get the underlying cpal device and config
        let (cpal_device, config) = get_device_and_config(&device).await?;

        info!(
            "Audio config - Sample rate: {}, Channels: {}, Format: {:?}",
            config.sample_rate().0,
            config.channels(),
            config.sample_format()
        );

        // Create audio capture processor
        let is_system_stream = device_type == DeviceType::System;
        let capture = AudioCapture::new(
            device.clone(),
            state.clone(),
            config.sample_rate().0,
            config.channels(),
            device_type.clone(),
            recording_sender,
        );

        // Build the appropriate stream based on sample format
        let require_first_callback = is_system_stream
            || cfg!(target_os = "windows") && device_type == DeviceType::Microphone;
        let first_callback = require_first_callback.then(|| Arc::new(tokio::sync::Notify::new()));
        let stream = PendingCpalStream(Self::build_stream(
            &cpal_device,
            &config,
            capture.clone(),
            first_callback.clone(),
        )?);

        #[cfg(target_os = "windows")]
        let microphone_started_qpc_ns = if device_type == DeviceType::Microphone {
            Some(super::windows_loopback::qpc_now_ns()?)
        } else {
            None
        };
        stream.0.play()?;
        #[cfg(target_os = "windows")]
        if let Some(started_qpc_ns) = microphone_started_qpc_ns {
            state.mark_microphone_stream_started(started_qpc_ns);
        }

        if let Some(first_callback) = first_callback {
            if tokio::time::timeout(
                std::time::Duration::from_nanos(super::recording_state::AUDIO_CALLBACK_DEADLINE_NS),
                first_callback.notified(),
            )
            .await
            .is_err()
            {
                let endpoint = device.native_id.as_deref().unwrap_or("unknown");
                #[cfg(target_os = "windows")]
                if device_type == DeviceType::Microphone {
                    let now_qpc_ns = super::windows_loopback::qpc_now_ns()?;
                    state.report_microphone_no_signal(
                        now_qpc_ns.saturating_sub(microphone_started_qpc_ns.unwrap_or(now_qpc_ns)),
                    );
                }
                return Err(anyhow::anyhow!(
                    "Audio endpoint produced no callback within 2 seconds: {} ({})",
                    device.name,
                    endpoint
                ));
            }

            #[cfg(target_os = "windows")]
            if device_type == DeviceType::Microphone {
                let (_, first_observed_qpc_ns, _, _, _) = state.microphone_route_health();
                let started_qpc_ns = microphone_started_qpc_ns.ok_or_else(|| {
                    anyhow::anyhow!("Windows microphone stream start QPC was not recorded")
                })?;
                let first_callback_gap_ns = first_observed_qpc_ns
                    .ok_or_else(|| {
                        anyhow::anyhow!("Windows microphone callback QPC was not recorded")
                    })?
                    .saturating_sub(started_qpc_ns);
                if first_callback_gap_ns >= super::recording_state::AUDIO_CALLBACK_DEADLINE_NS {
                    state.report_microphone_no_signal(first_callback_gap_ns);
                    return Err(anyhow::anyhow!(
                        "Microphone endpoint first callback took at least 2 seconds: {} ({})",
                        device.name,
                        device.native_id.as_deref().unwrap_or("unknown")
                    ));
                }
            }
        }
        info!("CPAL stream started for device: {}", device.name);

        #[cfg(target_os = "windows")]
        let microphone_watchdog = if device_type == DeviceType::Microphone {
            let bound_epoch = state.get_device_epoch();
            Some(WindowsMicrophoneWatchdog::start(state, bound_epoch)?)
        } else {
            None
        };

        Ok(Self {
            device,
            backend: StreamBackend::Cpal(stream.0),
            #[cfg(target_os = "windows")]
            microphone_watchdog,
        })
    }

    /// Create a Core Audio stream (macOS only)
    #[cfg(target_os = "macos")]
    async fn create_core_audio_stream(
        device: Arc<AudioDevice>,
        state: Arc<RecordingState>,
        device_type: DeviceType,
        recording_sender: Option<mpsc::UnboundedSender<super::recording_state::AudioChunk>>,
    ) -> Result<Self> {
        info!(
            "🔊 Stream: Creating Core Audio stream for device: {}",
            device.name
        );

        // Create Core Audio capture
        info!("🔊 Stream: Calling CoreAudioCapture::new()...");
        let capture_impl = CoreAudioCapture::new().map_err(|e| {
            error!("❌ Stream: CoreAudioCapture::new() failed: {}", e);
            anyhow::anyhow!("Failed to create Core Audio capture: {}", e)
        })?;

        info!("✅ Stream: CoreAudioCapture created, calling stream()...");
        let core_stream = capture_impl.stream().map_err(|e| {
            error!("❌ Stream: capture_impl.stream() failed: {}", e);
            anyhow::anyhow!("Failed to create Core Audio stream: {}", e)
        })?;

        let sample_rate = core_stream.sample_rate();
        info!(
            "✅ Stream: Core Audio stream created with sample rate: {} Hz",
            sample_rate
        );

        // Create audio capture processor for pipeline integration
        // CRITICAL: Core Audio tap is MONO (with_mono_global_tap_excluding_processes)
        let capture = AudioCapture::new(
            device.clone(),
            state.clone(),
            sample_rate,
            1, // Core Audio tap is MONO (not stereo!)
            device_type,
            recording_sender,
        );

        // Spawn task to process Core Audio stream samples
        // The stream needs to be polled continuously to produce samples
        let device_name = device.name.clone();
        info!("🔊 Stream: Spawning tokio task to poll Core Audio stream...");
        let task = tokio::spawn({
            let capture = capture.clone();
            let mut stream = core_stream;

            async move {
                use futures_util::StreamExt;

                let mut buffer = Vec::new();
                let mut frame_count = 0;
                let frames_per_chunk = 1024; // Process in chunks of 1024 samples

                info!(
                    "✅ Stream: Core Audio processing task started for {}",
                    device_name
                );

                let mut _sample_count = 0u64;
                while let Some(sample) = stream.next().await {
                    _sample_count += 1;
                    // if _sample_count % 48000 == 0 {
                    //     info!("📊 Stream: Received {} samples from Core Audio stream", _sample_count);
                    // }

                    buffer.push(sample);
                    frame_count += 1;

                    // Process when we have enough samples
                    if frame_count >= frames_per_chunk {
                        capture.process_audio_data(&buffer);
                        buffer.clear();
                        frame_count = 0;
                    }
                }

                // Process any remaining samples
                if !buffer.is_empty() {
                    capture.process_audio_data(&buffer);
                }

                info!(
                    "⚠️ Stream: Core Audio processing task ended for {}",
                    device_name
                );
            }
        });

        info!(
            "✅ Stream: Core Audio stream fully initialized for device: {}",
            device.name
        );

        Ok(Self {
            device: device.clone(),
            backend: StreamBackend::CoreAudio { task: Some(task) },
            #[cfg(target_os = "windows")]
            microphone_watchdog: None,
        })
    }

    /// Build stream based on sample format
    fn build_stream(
        device: &Device,
        config: &SupportedStreamConfig,
        capture: AudioCapture,
        first_callback: Option<Arc<tokio::sync::Notify>>,
    ) -> Result<Stream> {
        let config_copy = config.clone();

        let stream = match config.sample_format() {
            cpal::SampleFormat::F32 => {
                let capture_clone = capture.clone();
                let callback_signal = first_callback.clone();
                device.build_input_stream(
                    &config_copy.into(),
                    move |data: &[f32], info: &cpal::InputCallbackInfo| {
                        capture.process_audio_data_at_qpc(data, capture_qpc_ns(info));
                        if !data.is_empty() {
                            if let Some(signal) = callback_signal.as_ref() {
                                signal.notify_one();
                            }
                        }
                    },
                    move |err| {
                        capture_clone.handle_stream_error(err);
                    },
                    None,
                )?
            }
            cpal::SampleFormat::I16 => {
                let capture_clone = capture.clone();
                let callback_signal = first_callback.clone();
                device.build_input_stream(
                    &config_copy.into(),
                    move |data: &[i16], info: &cpal::InputCallbackInfo| {
                        let f32_data: Vec<f32> = data
                            .iter()
                            .map(|&sample| sample as f32 / i16::MAX as f32)
                            .collect();
                        capture.process_audio_data_at_qpc(&f32_data, capture_qpc_ns(info));
                        if !data.is_empty() {
                            if let Some(signal) = callback_signal.as_ref() {
                                signal.notify_one();
                            }
                        }
                    },
                    move |err| {
                        capture_clone.handle_stream_error(err);
                    },
                    None,
                )?
            }
            cpal::SampleFormat::I32 => {
                let capture_clone = capture.clone();
                let callback_signal = first_callback.clone();
                device.build_input_stream(
                    &config_copy.into(),
                    move |data: &[i32], info: &cpal::InputCallbackInfo| {
                        let f32_data: Vec<f32> = data
                            .iter()
                            .map(|&sample| sample as f32 / i32::MAX as f32)
                            .collect();
                        capture.process_audio_data_at_qpc(&f32_data, capture_qpc_ns(info));
                        if !data.is_empty() {
                            if let Some(signal) = callback_signal.as_ref() {
                                signal.notify_one();
                            }
                        }
                    },
                    move |err| {
                        capture_clone.handle_stream_error(err);
                    },
                    None,
                )?
            }
            cpal::SampleFormat::I8 => {
                let capture_clone = capture.clone();
                let callback_signal = first_callback.clone();
                device.build_input_stream(
                    &config_copy.into(),
                    move |data: &[i8], info: &cpal::InputCallbackInfo| {
                        let f32_data: Vec<f32> = data
                            .iter()
                            .map(|&sample| sample as f32 / i8::MAX as f32)
                            .collect();
                        capture.process_audio_data_at_qpc(&f32_data, capture_qpc_ns(info));
                        if !data.is_empty() {
                            if let Some(signal) = callback_signal.as_ref() {
                                signal.notify_one();
                            }
                        }
                    },
                    move |err| {
                        capture_clone.handle_stream_error(err);
                    },
                    None,
                )?
            }
            _ => {
                return Err(anyhow::anyhow!(
                    "Unsupported sample format: {:?}",
                    config.sample_format()
                ));
            }
        };

        Ok(stream)
    }

    /// Get device info
    pub fn device(&self) -> &AudioDevice {
        &self.device
    }

    /// Stop the stream
    pub fn stop(mut self) -> Result<()> {
        info!("Stopping audio stream for device: {}", self.device.name);

        #[cfg(target_os = "windows")]
        if let Some(watchdog) = self.microphone_watchdog.take() {
            watchdog.stop()?;
        }

        match self.backend {
            StreamBackend::Cpal(stream) => {
                // CRITICAL: Pause the stream first to stop callbacks immediately
                // This ensures closures stop executing before we drop the stream,
                // allowing Arc references captured in callbacks to be released
                if let Err(e) = stream.pause() {
                    warn!("Failed to pause stream before drop: {}", e);
                }
                info!("Stream paused, now dropping to release callbacks");
                drop(stream);
            }
            #[cfg(target_os = "windows")]
            StreamBackend::WindowsLoopback(stream) => {
                stream.stop()?;
            }
            #[cfg(target_os = "macos")]
            StreamBackend::CoreAudio { task } => {
                // Abort the processing task and wait briefly for cleanup
                if let Some(task_handle) = task {
                    info!("Aborting Core Audio task...");
                    task_handle.abort();
                    // Give the runtime a moment to clean up the aborted task
                    // This helps ensure Arc references in the closure are dropped
                    std::thread::sleep(std::time::Duration::from_millis(50));
                    info!("Core Audio task aborted");
                }
            }
        }

        // Explicitly drop self.device Arc reference
        drop(self.device);
        info!("Audio stream stopped and device reference dropped");
        Ok(())
    }
}

/// Audio stream manager for handling multiple streams
pub struct AudioStreamManager {
    microphone_stream: Option<AudioStream>,
    system_stream: Option<AudioStream>,
    state: Arc<RecordingState>,
}

// SAFETY: AudioStreamManager contains AudioStream which we've marked as Send
unsafe impl Send for AudioStreamManager {}

impl AudioStreamManager {
    pub fn new(state: Arc<RecordingState>) -> Self {
        Self {
            microphone_stream: None,
            system_stream: None,
            state,
        }
    }

    /// Start audio streams for the given devices
    pub async fn start_streams(
        &mut self,
        microphone_device: Option<Arc<AudioDevice>>,
        system_device: Option<Arc<AudioDevice>>,
        recording_sender: Option<mpsc::UnboundedSender<super::recording_state::AudioChunk>>,
    ) -> Result<()> {
        use super::capture::get_current_backend;
        let backend = get_current_backend();
        info!("🎙️ Starting audio streams with backend: {:?}", backend);

        // Start microphone stream
        if let Some(mic_device) = microphone_device {
            info!(
                "🎤 Creating microphone stream: {} (always uses CPAL)",
                mic_device.name
            );
            match AudioStream::create(
                mic_device.clone(),
                self.state.clone(),
                DeviceType::Microphone,
                recording_sender.clone(),
            )
            .await
            {
                Ok(stream) => {
                    self.state.set_microphone_device(mic_device);
                    self.microphone_stream = Some(stream);
                    info!("✅ Microphone stream created successfully");
                }
                Err(e) => {
                    error!("❌ Failed to create microphone stream: {}", e);
                    return Err(e);
                }
            }
        } else {
            info!("ℹ️ No microphone device specified, skipping microphone stream");
        }

        // Start system audio stream
        if let Some(sys_device) = system_device {
            info!(
                "🔊 Creating system audio stream: {} (backend: {:?})",
                sys_device.name, backend
            );
            match AudioStream::create(
                sys_device.clone(),
                self.state.clone(),
                DeviceType::System,
                recording_sender.clone(),
            )
            .await
            {
                Ok(stream) => {
                    self.state.set_system_device(sys_device);
                    self.system_stream = Some(stream);
                    info!("✅ System audio stream created with {:?} backend", backend);
                }
                Err(e) => {
                    error!("❌ Failed to create requested system audio stream: {}", e);
                    return Err(anyhow::anyhow!(
                        "Requested system audio stream failed to start: {}",
                        e
                    ));
                }
            }
        } else {
            info!("ℹ️ No system device specified, skipping system audio stream");
        }

        // Ensure at least one stream was created
        if self.microphone_stream.is_none() && self.system_stream.is_none() {
            return Err(anyhow::anyhow!("No audio streams could be created"));
        }

        Ok(())
    }

    /// Stop all audio streams
    pub fn stop_streams(&mut self) -> Result<()> {
        info!("Stopping all audio streams");

        let mut errors = Vec::new();

        // Stop microphone stream
        if let Some(mic_stream) = self.microphone_stream.take() {
            if let Err(e) = mic_stream.stop() {
                error!("Failed to stop microphone stream: {}", e);
                errors.push(e);
            }
        }

        // Stop system stream
        if let Some(sys_stream) = self.system_stream.take() {
            if let Err(e) = sys_stream.stop() {
                error!("Failed to stop system stream: {}", e);
                errors.push(e);
            }
        }

        if !errors.is_empty() {
            Err(anyhow::anyhow!("Failed to stop some streams: {:?}", errors))
        } else {
            info!("All audio streams stopped successfully");
            Ok(())
        }
    }

    /// Stop callbacks without allowing a stuck driver to block the async
    /// recording lifecycle forever. The detached blocking task still owns and
    /// eventually drops the stream; the epoch guard rejects any late callback.
    pub async fn stop_streams_with_timeout(&mut self) -> Result<()> {
        self.stop_streams_with_timeout_duration(std::time::Duration::from_secs(2))
            .await
    }

    pub(crate) async fn stop_streams_with_timeout_duration(
        &mut self,
        timeout: std::time::Duration,
    ) -> Result<()> {
        let microphone_stream = self.microphone_stream.take();
        let system_stream = self.system_stream.take();
        let stop_task = tokio::task::spawn_blocking(move || -> Result<()> {
            if let Some(stream) = microphone_stream {
                stream.stop()?;
            }
            if let Some(stream) = system_stream {
                stream.stop()?;
            }
            Ok(())
        });

        match tokio::time::timeout(timeout, stop_task).await {
            Ok(Ok(result)) => result,
            Ok(Err(error)) => Err(anyhow::anyhow!("Audio stream stop task failed: {error}")),
            Err(_) => Err(anyhow::anyhow!(
                "Audio stream callback drain exceeded its fixed deadline"
            )),
        }
    }

    /// Get stream count
    pub fn active_stream_count(&self) -> usize {
        let mut count = 0;
        if self.microphone_stream.is_some() {
            count += 1;
        }
        if self.system_stream.is_some() {
            count += 1;
        }
        count
    }

    /// Check if any streams are active
    pub fn has_active_streams(&self) -> bool {
        self.microphone_stream.is_some() || self.system_stream.is_some()
    }
}

impl Drop for AudioStreamManager {
    fn drop(&mut self) {
        if let Err(e) = self.stop_streams() {
            error!("Error stopping streams during drop: {}", e);
        }
    }
}

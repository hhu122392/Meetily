use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;
use tokio::sync::mpsc;

use super::buffer_pool::AudioBufferPool;
use super::devices::AudioDevice;

pub(crate) const AUDIO_CALLBACK_DEADLINE_NS: u64 = 2_000_000_000;

/// Device type for audio chunks
#[derive(Debug, Clone, PartialEq)]
pub enum DeviceType {
    Microphone,
    System,
}

/// Audio chunk with metadata for processing
#[derive(Debug, Clone)]
pub struct AudioChunk {
    pub data: Vec<f32>,
    pub sample_rate: u32,
    pub timestamp: f64,
    pub chunk_id: u64,
    pub device_type: DeviceType,
    pub device_epoch: u64,
    pub capture_qpc_ns: Option<u64>,
}

/// Processed audio chunk (post-VAD) for recording
#[derive(Debug, Clone)]
pub struct ProcessedAudioChunk {
    pub data: Vec<f32>,
    pub sample_rate: u32,
    pub timestamp: f64,
    pub device_type: DeviceType,
}

/// Comprehensive error types for audio system
#[derive(Debug, Clone)]
pub enum AudioError {
    DeviceDisconnected,
    StreamFailed,
    ProcessingFailed,
    TranscriptionFailed,
    ChannelClosed,
    InitializationFailed,
    ConfigurationError,
    PermissionDenied,
    BufferOverflow,
    SampleRateUnsupported,
    SystemAudioNoSignal,
    SystemAudioSilent,
    MicrophoneNoSignal,
}

impl AudioError {
    /// Check if error is recoverable (can attempt reconnection)
    pub fn is_recoverable(&self) -> bool {
        match self {
            // Device disconnect is now recoverable - we can attempt reconnection
            AudioError::DeviceDisconnected => true,
            AudioError::StreamFailed => true,
            AudioError::ProcessingFailed => true,
            AudioError::TranscriptionFailed => true,
            AudioError::ChannelClosed => false,
            AudioError::InitializationFailed => false,
            AudioError::ConfigurationError => false,
            AudioError::PermissionDenied => false,
            AudioError::BufferOverflow => true,
            AudioError::SampleRateUnsupported => false,
            AudioError::SystemAudioNoSignal => true,
            AudioError::SystemAudioSilent => true,
            AudioError::MicrophoneNoSignal => true,
        }
    }

    /// Get user-friendly error message
    pub fn user_message(&self) -> &'static str {
        match self {
            AudioError::DeviceDisconnected => "Audio device was disconnected",
            AudioError::StreamFailed => "Audio stream encountered an error",
            AudioError::ProcessingFailed => "Audio processing failed",
            AudioError::TranscriptionFailed => "Speech transcription failed",
            AudioError::ChannelClosed => "Audio channel was closed unexpectedly",
            AudioError::InitializationFailed => "Failed to initialize audio system",
            AudioError::ConfigurationError => "Audio configuration error",
            AudioError::PermissionDenied => "Microphone permission denied",
            AudioError::BufferOverflow => "Audio buffer overflow",
            AudioError::SampleRateUnsupported => "Audio sample rate not supported",
            AudioError::SystemAudioNoSignal => {
                "System audio endpoint produced no callback for 2 seconds"
            }
            AudioError::SystemAudioSilent => {
                "System audio endpoint is muted and produced only silent frames"
            }
            AudioError::MicrophoneNoSignal => {
                "Microphone endpoint produced no callback for 2 seconds"
            }
        }
    }
}

/// Native format bound to the current Windows system-audio epoch.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SystemAudioFormat {
    pub sample_rate: u32,
    pub channels: u16,
    pub bits_per_sample: u16,
    pub block_align: u16,
    pub sample_format: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AudioDeviceEpochEvidence {
    pub device_epoch: u64,
    pub microphone_name: Option<String>,
    pub microphone_native_id: Option<String>,
    pub microphone_default_role: Option<String>,
    pub system_name: Option<String>,
    pub system_native_id: Option<String>,
    pub system_default_role: Option<String>,
    pub system_format: Option<SystemAudioFormat>,
    pub microphone_stream_started_qpc_ns: Option<u64>,
    pub system_stream_started_qpc_ns: Option<u64>,
    pub microphone_last_capture_qpc_ns: Option<u64>,
    pub system_last_capture_qpc_ns: Option<u64>,
    pub microphone_last_callback_observed_qpc_ns: Option<u64>,
    pub system_last_callback_observed_qpc_ns: Option<u64>,
    pub microphone_max_callback_gap_ns: u64,
    pub system_max_callback_gap_ns: u64,
    pub microphone_callback_count: u64,
    pub microphone_frame_count: u64,
    pub microphone_sample_count: u64,
    pub system_callback_count: u64,
    pub system_frame_count: u64,
    pub system_sample_count: u64,
    pub system_silent_flag_frame_count: u64,
    pub system_all_zero_frame_count: u64,
    pub microphone_no_signal_count: u64,
    pub system_no_signal_count: u64,
    pub microphone_route_failed: bool,
    pub system_route_failed: bool,
    pub system_endpoint_muted: bool,
    pub system_audio_silent: bool,
    pub driver_mute_behavior: Option<String>,
    pub microphone_rms: f32,
    pub microphone_peak: f32,
    pub system_rms: f32,
    pub system_peak: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AudioRouteIncidentEvidence {
    pub sequence: u64,
    pub device_epoch: u64,
    pub route: String,
    pub code: String,
    pub started_qpc_ns: Option<u64>,
    pub recovered_qpc_ns: Option<u64>,
    pub callback_gap_ns: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AudioCutoverEvidence {
    pub sequence: u64,
    pub old_device_epoch: u64,
    pub new_device_epoch: Option<u64>,
    pub watermark_qpc_ns: u64,
    pub drain_deadline_qpc_ns: u64,
    pub old_epoch_last_capture_qpc_ns: Option<u64>,
    pub new_epoch_first_capture_qpc_ns: Option<u64>,
    pub late_callback_dropped_frames: u64,
    pub attribution_error_frames: u64,
    pub terminal_result: String,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AudioRouteEvidenceSnapshot {
    pub schema_version: u32,
    pub current_device_epoch: u64,
    pub microphone_stream_started_count: u64,
    pub epochs: Vec<AudioDeviceEpochEvidence>,
    pub incidents: Vec<AudioRouteIncidentEvidence>,
    pub cutovers: Vec<AudioCutoverEvidence>,
}

pub struct InFlightAudioCallback {
    state: Arc<RecordingState>,
}

impl Drop for InFlightAudioCallback {
    fn drop(&mut self) {
        self.state
            .in_flight_callback_count
            .fetch_sub(1, Ordering::SeqCst);
    }
}

/// Recording statistics
#[derive(Debug, Default)]
pub struct RecordingStats {
    pub chunks_processed: u64,
    pub total_duration: f64,
    pub last_activity: Option<Instant>,
}

/// Unified state management for audio recording
pub struct RecordingState {
    // Core recording state
    is_recording: AtomicBool,
    is_paused: AtomicBool,
    is_reconnecting: AtomicBool, // NEW: Attempting to reconnect to device
    routes_ready: AtomicBool,

    // Audio devices
    microphone_device: Mutex<Option<Arc<AudioDevice>>>,
    system_device: Mutex<Option<Arc<AudioDevice>>>,
    // Track which device is disconnected for reconnection attempts
    disconnected_device: Mutex<Option<(Arc<AudioDevice>, DeviceType)>>,

    // Audio pipeline
    audio_sender: Mutex<Option<mpsc::UnboundedSender<AudioChunk>>>,

    // Memory optimization
    buffer_pool: AudioBufferPool,

    // Error handling
    error_count: AtomicU32,
    recoverable_error_count: AtomicU32,
    last_error: Mutex<Option<AudioError>>,
    error_callback: Mutex<Option<Box<dyn Fn(&AudioError) + Send + Sync>>>,

    // Statistics
    stats: Mutex<RecordingStats>,

    // Windows system-audio attribution. Epoch zero is the initial binding;
    // every successful rebind advances it exactly once.
    device_epoch: AtomicU64,
    system_stream_started_qpc_ns: AtomicU64,
    system_last_capture_qpc_ns: AtomicU64,
    system_last_callback_observed_qpc_ns: AtomicU64,
    system_max_callback_gap_ns: AtomicU64,
    system_no_signal: AtomicBool,
    system_no_signal_count: AtomicU64,
    system_endpoint_muted: AtomicBool,
    system_audio_silent: AtomicBool,
    driver_mute_behavior: AtomicU32,
    system_muted_zero_since_qpc_ns: AtomicU64,
    system_frame_count: AtomicU64,
    system_silent_flag_frame_count: AtomicU64,
    system_all_zero_frame_count: AtomicU64,
    system_audio_format: Mutex<Option<SystemAudioFormat>>,
    closing_device_epoch: AtomicU64,
    cutover_watermark_qpc_ns: AtomicU64,
    callback_drain_deadline_qpc_ns: AtomicU64,
    in_flight_callback_count: AtomicU64,
    late_callback_dropped_frames: AtomicU64,
    attribution_error_frames: AtomicU64,
    cutover_old_last_capture_qpc_ns: AtomicU64,
    cutover_new_epoch: AtomicU64,
    cutover_new_first_capture_qpc_ns: AtomicU64,
    microphone_stream_started_count: AtomicU64,
    microphone_stream_started_qpc_ns: AtomicU64,
    microphone_last_capture_qpc_ns: AtomicU64,
    microphone_last_callback_observed_qpc_ns: AtomicU64,
    microphone_max_callback_gap_ns: AtomicU64,
    microphone_no_signal: AtomicBool,
    microphone_no_signal_count: AtomicU64,
    microphone_route_failed: AtomicBool,
    system_route_failed: AtomicBool,
    microphone_callback_count: AtomicU64,
    microphone_frame_count: AtomicU64,
    microphone_sample_count: AtomicU64,
    system_callback_count: AtomicU64,
    system_sample_count: AtomicU64,
    microphone_rms_bits: AtomicU32,
    microphone_peak_bits: AtomicU32,
    system_rms_bits: AtomicU32,
    system_peak_bits: AtomicU32,
    completed_epoch_evidence: Mutex<Vec<AudioDeviceEpochEvidence>>,
    route_incidents: Mutex<Vec<AudioRouteIncidentEvidence>>,
    cutover_evidence: Mutex<Vec<AudioCutoverEvidence>>,
    evidence_sequence: AtomicU64,
    evidence_dirty: AtomicBool,

    // Recording start time for accurate timestamps
    recording_start: Mutex<Option<Instant>>,
    // Pause time tracking
    pause_start: Mutex<Option<Instant>>,
    total_pause_duration: Mutex<std::time::Duration>,
}

impl RecordingState {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            is_recording: AtomicBool::new(false),
            is_paused: AtomicBool::new(false),
            is_reconnecting: AtomicBool::new(false),
            routes_ready: AtomicBool::new(false),
            microphone_device: Mutex::new(None),
            system_device: Mutex::new(None),
            disconnected_device: Mutex::new(None),
            audio_sender: Mutex::new(None),
            buffer_pool: AudioBufferPool::new(16, 48000), // Pool of 16 buffers with 48kHz samples capacity
            error_count: AtomicU32::new(0),
            recoverable_error_count: AtomicU32::new(0),
            last_error: Mutex::new(None),
            error_callback: Mutex::new(None),
            stats: Mutex::new(RecordingStats::default()),
            device_epoch: AtomicU64::new(0),
            system_stream_started_qpc_ns: AtomicU64::new(0),
            system_last_capture_qpc_ns: AtomicU64::new(0),
            system_last_callback_observed_qpc_ns: AtomicU64::new(0),
            system_max_callback_gap_ns: AtomicU64::new(0),
            system_no_signal: AtomicBool::new(false),
            system_no_signal_count: AtomicU64::new(0),
            system_endpoint_muted: AtomicBool::new(false),
            system_audio_silent: AtomicBool::new(false),
            driver_mute_behavior: AtomicU32::new(0),
            system_muted_zero_since_qpc_ns: AtomicU64::new(0),
            system_frame_count: AtomicU64::new(0),
            system_silent_flag_frame_count: AtomicU64::new(0),
            system_all_zero_frame_count: AtomicU64::new(0),
            system_audio_format: Mutex::new(None),
            closing_device_epoch: AtomicU64::new(u64::MAX),
            cutover_watermark_qpc_ns: AtomicU64::new(0),
            callback_drain_deadline_qpc_ns: AtomicU64::new(0),
            in_flight_callback_count: AtomicU64::new(0),
            late_callback_dropped_frames: AtomicU64::new(0),
            attribution_error_frames: AtomicU64::new(0),
            cutover_old_last_capture_qpc_ns: AtomicU64::new(0),
            cutover_new_epoch: AtomicU64::new(u64::MAX),
            cutover_new_first_capture_qpc_ns: AtomicU64::new(0),
            microphone_stream_started_count: AtomicU64::new(0),
            microphone_stream_started_qpc_ns: AtomicU64::new(0),
            microphone_last_capture_qpc_ns: AtomicU64::new(0),
            microphone_last_callback_observed_qpc_ns: AtomicU64::new(0),
            microphone_max_callback_gap_ns: AtomicU64::new(0),
            microphone_no_signal: AtomicBool::new(false),
            microphone_no_signal_count: AtomicU64::new(0),
            microphone_route_failed: AtomicBool::new(false),
            system_route_failed: AtomicBool::new(false),
            microphone_callback_count: AtomicU64::new(0),
            microphone_frame_count: AtomicU64::new(0),
            microphone_sample_count: AtomicU64::new(0),
            system_callback_count: AtomicU64::new(0),
            system_sample_count: AtomicU64::new(0),
            microphone_rms_bits: AtomicU32::new(0.0_f32.to_bits()),
            microphone_peak_bits: AtomicU32::new(0.0_f32.to_bits()),
            system_rms_bits: AtomicU32::new(0.0_f32.to_bits()),
            system_peak_bits: AtomicU32::new(0.0_f32.to_bits()),
            completed_epoch_evidence: Mutex::new(Vec::new()),
            route_incidents: Mutex::new(Vec::new()),
            cutover_evidence: Mutex::new(Vec::new()),
            evidence_sequence: AtomicU64::new(0),
            evidence_dirty: AtomicBool::new(false),
            recording_start: Mutex::new(None),
            pause_start: Mutex::new(None),
            total_pause_duration: Mutex::new(std::time::Duration::ZERO),
        })
    }

    // Recording control
    pub fn start_recording(&self) -> Result<()> {
        self.is_recording.store(true, Ordering::SeqCst);
        self.routes_ready.store(false, Ordering::SeqCst);
        *self.recording_start.lock().unwrap() = Some(Instant::now());
        self.error_count.store(0, Ordering::SeqCst);
        self.recoverable_error_count.store(0, Ordering::SeqCst);
        *self.last_error.lock().unwrap() = None;
        self.device_epoch.store(0, Ordering::SeqCst);
        self.system_stream_started_qpc_ns.store(0, Ordering::SeqCst);
        self.system_last_capture_qpc_ns.store(0, Ordering::SeqCst);
        self.system_last_callback_observed_qpc_ns
            .store(0, Ordering::SeqCst);
        self.system_max_callback_gap_ns.store(0, Ordering::SeqCst);
        self.system_no_signal.store(false, Ordering::SeqCst);
        self.system_no_signal_count.store(0, Ordering::SeqCst);
        self.system_endpoint_muted.store(false, Ordering::SeqCst);
        self.system_audio_silent.store(false, Ordering::SeqCst);
        self.driver_mute_behavior.store(0, Ordering::SeqCst);
        self.system_muted_zero_since_qpc_ns
            .store(0, Ordering::SeqCst);
        self.system_frame_count.store(0, Ordering::SeqCst);
        self.system_silent_flag_frame_count
            .store(0, Ordering::SeqCst);
        self.system_all_zero_frame_count.store(0, Ordering::SeqCst);
        *self.system_audio_format.lock().unwrap() = None;
        self.closing_device_epoch.store(u64::MAX, Ordering::SeqCst);
        self.cutover_watermark_qpc_ns.store(0, Ordering::SeqCst);
        self.callback_drain_deadline_qpc_ns
            .store(0, Ordering::SeqCst);
        self.in_flight_callback_count.store(0, Ordering::SeqCst);
        self.late_callback_dropped_frames.store(0, Ordering::SeqCst);
        self.attribution_error_frames.store(0, Ordering::SeqCst);
        self.cutover_old_last_capture_qpc_ns
            .store(0, Ordering::SeqCst);
        self.cutover_new_epoch.store(u64::MAX, Ordering::SeqCst);
        self.cutover_new_first_capture_qpc_ns
            .store(0, Ordering::SeqCst);
        self.microphone_stream_started_count
            .store(0, Ordering::SeqCst);
        self.microphone_stream_started_qpc_ns
            .store(0, Ordering::SeqCst);
        self.microphone_last_capture_qpc_ns
            .store(0, Ordering::SeqCst);
        self.microphone_last_callback_observed_qpc_ns
            .store(0, Ordering::SeqCst);
        self.microphone_max_callback_gap_ns
            .store(0, Ordering::SeqCst);
        self.microphone_no_signal.store(false, Ordering::SeqCst);
        self.microphone_no_signal_count.store(0, Ordering::SeqCst);
        self.microphone_route_failed.store(false, Ordering::SeqCst);
        self.system_route_failed.store(false, Ordering::SeqCst);
        self.microphone_callback_count.store(0, Ordering::SeqCst);
        self.microphone_frame_count.store(0, Ordering::SeqCst);
        self.microphone_sample_count.store(0, Ordering::SeqCst);
        self.system_callback_count.store(0, Ordering::SeqCst);
        self.system_sample_count.store(0, Ordering::SeqCst);
        self.microphone_rms_bits
            .store(0.0_f32.to_bits(), Ordering::SeqCst);
        self.microphone_peak_bits
            .store(0.0_f32.to_bits(), Ordering::SeqCst);
        self.system_rms_bits
            .store(0.0_f32.to_bits(), Ordering::SeqCst);
        self.system_peak_bits
            .store(0.0_f32.to_bits(), Ordering::SeqCst);
        self.completed_epoch_evidence.lock().unwrap().clear();
        self.route_incidents.lock().unwrap().clear();
        self.cutover_evidence.lock().unwrap().clear();
        self.evidence_sequence.store(0, Ordering::SeqCst);
        self.evidence_dirty.store(false, Ordering::SeqCst);
        Ok(())
    }

    pub fn stop_recording(&self) {
        self.is_recording.store(false, Ordering::SeqCst);
        self.is_paused.store(false, Ordering::SeqCst);
        self.routes_ready.store(false, Ordering::SeqCst);
        // Clear pause tracking when stopping
        *self.pause_start.lock().unwrap() = None;
        // CRITICAL: Clear audio sender to close the pipeline channel
        // This ensures the pipeline loop exits properly after processing all chunks
        *self.audio_sender.lock().unwrap() = None;
        // CRITICAL: Clear device references to release microphone/speaker
        // Without this, Arc<AudioDevice> references persist and keep the mic active
        *self.microphone_device.lock().unwrap() = None;
        *self.system_device.lock().unwrap() = None;
        *self.disconnected_device.lock().unwrap() = None;
        log::info!("Recording stopped, device references cleared");
    }

    pub fn pause_recording(&self) -> Result<()> {
        if !self.is_recording() {
            return Err(anyhow::anyhow!("Cannot pause when not recording"));
        }
        if self.is_paused() {
            return Err(anyhow::anyhow!("Recording is already paused"));
        }

        self.is_paused.store(true, Ordering::SeqCst);
        *self.pause_start.lock().unwrap() = Some(Instant::now());
        log::info!("Recording paused");
        Ok(())
    }

    pub fn resume_recording(&self) -> Result<()> {
        if !self.is_recording() {
            return Err(anyhow::anyhow!("Cannot resume when not recording"));
        }
        if !self.is_paused() {
            return Err(anyhow::anyhow!("Recording is not paused"));
        }

        // Calculate pause duration and add to total
        if let Some(pause_start) = self.pause_start.lock().unwrap().take() {
            let pause_duration = pause_start.elapsed();
            *self.total_pause_duration.lock().unwrap() += pause_duration;
            log::info!(
                "Recording resumed after pause of {:.2}s",
                pause_duration.as_secs_f64()
            );
        }

        self.is_paused.store(false, Ordering::SeqCst);
        Ok(())
    }

    pub fn is_recording(&self) -> bool {
        self.is_recording.load(Ordering::SeqCst)
    }

    pub fn is_paused(&self) -> bool {
        self.is_paused.load(Ordering::SeqCst)
    }

    pub fn is_active(&self) -> bool {
        self.is_recording()
            && !self.is_paused()
            && !self.is_reconnecting()
            && self.routes_ready.load(Ordering::SeqCst)
            && !self.system_no_signal.load(Ordering::SeqCst)
            && !self.system_audio_silent.load(Ordering::SeqCst)
            && !self.microphone_no_signal.load(Ordering::SeqCst)
            && !self.microphone_route_failed.load(Ordering::SeqCst)
            && !self.system_route_failed.load(Ordering::SeqCst)
    }

    pub(crate) fn accepts_audio_timeline(&self) -> bool {
        self.is_recording()
            && !self.is_paused()
            && !self.is_reconnecting()
            && self.routes_ready.load(Ordering::SeqCst)
            && !self.microphone_no_signal.load(Ordering::SeqCst)
            && !self.microphone_route_failed.load(Ordering::SeqCst)
            && !self.system_route_failed.load(Ordering::SeqCst)
    }

    // Reconnection state management
    pub fn start_reconnecting(&self, device: Arc<AudioDevice>, device_type: DeviceType) {
        self.is_reconnecting.store(true, Ordering::SeqCst);
        self.routes_ready.store(false, Ordering::SeqCst);
        *self.disconnected_device.lock().unwrap() = Some((device, device_type));
        log::info!("Started reconnection attempt for device");
    }

    pub fn stop_reconnecting(&self) {
        self.is_reconnecting.store(false, Ordering::SeqCst);
        self.routes_ready.store(true, Ordering::SeqCst);
        *self.disconnected_device.lock().unwrap() = None;
        log::info!("Stopped reconnection attempt");
    }

    pub fn is_reconnecting(&self) -> bool {
        self.is_reconnecting.load(Ordering::SeqCst)
    }

    pub fn mark_routes_ready(&self) {
        self.routes_ready.store(true, Ordering::SeqCst);
    }

    pub fn route_watchdog_enabled(&self) -> bool {
        self.is_recording()
            && !self.is_paused()
            && !self.is_reconnecting()
            && self.routes_ready.load(Ordering::SeqCst)
    }

    pub fn get_disconnected_device(&self) -> Option<(Arc<AudioDevice>, DeviceType)> {
        self.disconnected_device.lock().unwrap().clone()
    }

    // Device management
    pub fn set_microphone_device(&self, device: Arc<AudioDevice>) {
        *self.microphone_device.lock().unwrap() = Some(device);
    }

    pub fn set_system_device(&self, device: Arc<AudioDevice>) {
        *self.system_device.lock().unwrap() = Some(device);
    }

    pub fn clear_microphone_device(&self) {
        *self.microphone_device.lock().unwrap() = None;
        self.evidence_dirty.store(true, Ordering::SeqCst);
    }

    pub fn clear_system_device(&self) {
        *self.system_device.lock().unwrap() = None;
        self.evidence_dirty.store(true, Ordering::SeqCst);
    }

    pub fn get_microphone_device(&self) -> Option<Arc<AudioDevice>> {
        self.microphone_device.lock().unwrap().clone()
    }

    pub fn get_system_device(&self) -> Option<Arc<AudioDevice>> {
        self.system_device.lock().unwrap().clone()
    }

    pub fn get_device_epoch(&self) -> u64 {
        self.device_epoch.load(Ordering::SeqCst)
    }

    pub fn advance_device_epoch(&self) -> u64 {
        self.completed_epoch_evidence
            .lock()
            .unwrap()
            .push(self.snapshot_current_epoch());
        let next = self.device_epoch.fetch_add(1, Ordering::SeqCst) + 1;
        self.system_stream_started_qpc_ns.store(0, Ordering::SeqCst);
        self.system_last_capture_qpc_ns.store(0, Ordering::SeqCst);
        self.system_last_callback_observed_qpc_ns
            .store(0, Ordering::SeqCst);
        self.system_max_callback_gap_ns.store(0, Ordering::SeqCst);
        self.system_no_signal.store(false, Ordering::SeqCst);
        self.system_no_signal_count.store(0, Ordering::SeqCst);
        self.system_endpoint_muted.store(false, Ordering::SeqCst);
        self.system_audio_silent.store(false, Ordering::SeqCst);
        self.driver_mute_behavior.store(0, Ordering::SeqCst);
        self.system_muted_zero_since_qpc_ns
            .store(0, Ordering::SeqCst);
        self.microphone_stream_started_qpc_ns
            .store(0, Ordering::SeqCst);
        self.microphone_last_capture_qpc_ns
            .store(0, Ordering::SeqCst);
        self.microphone_last_callback_observed_qpc_ns
            .store(0, Ordering::SeqCst);
        self.microphone_max_callback_gap_ns
            .store(0, Ordering::SeqCst);
        self.microphone_no_signal.store(false, Ordering::SeqCst);
        self.microphone_no_signal_count.store(0, Ordering::SeqCst);
        self.microphone_route_failed.store(false, Ordering::SeqCst);
        self.system_route_failed.store(false, Ordering::SeqCst);
        self.microphone_callback_count.store(0, Ordering::SeqCst);
        self.microphone_frame_count.store(0, Ordering::SeqCst);
        self.microphone_sample_count.store(0, Ordering::SeqCst);
        self.system_callback_count.store(0, Ordering::SeqCst);
        self.system_sample_count.store(0, Ordering::SeqCst);
        self.microphone_rms_bits
            .store(0.0_f32.to_bits(), Ordering::SeqCst);
        self.microphone_peak_bits
            .store(0.0_f32.to_bits(), Ordering::SeqCst);
        self.system_rms_bits
            .store(0.0_f32.to_bits(), Ordering::SeqCst);
        self.system_peak_bits
            .store(0.0_f32.to_bits(), Ordering::SeqCst);
        self.system_frame_count.store(0, Ordering::SeqCst);
        self.system_silent_flag_frame_count
            .store(0, Ordering::SeqCst);
        self.system_all_zero_frame_count.store(0, Ordering::SeqCst);
        *self.system_audio_format.lock().unwrap() = None;
        self.evidence_dirty.store(true, Ordering::SeqCst);
        next
    }

    pub fn begin_device_cutover(&self, watermark_qpc_ns: u64) -> u64 {
        self.freeze_latest_cutover_metrics();
        let closing_epoch = self.get_device_epoch();
        self.closing_device_epoch
            .store(closing_epoch, Ordering::SeqCst);
        self.cutover_watermark_qpc_ns
            .store(watermark_qpc_ns, Ordering::SeqCst);
        let deadline = watermark_qpc_ns.saturating_add(AUDIO_CALLBACK_DEADLINE_NS);
        self.callback_drain_deadline_qpc_ns
            .store(deadline, Ordering::SeqCst);
        self.cutover_old_last_capture_qpc_ns
            .store(0, Ordering::SeqCst);
        self.cutover_new_epoch.store(u64::MAX, Ordering::SeqCst);
        self.cutover_new_first_capture_qpc_ns
            .store(0, Ordering::SeqCst);
        self.late_callback_dropped_frames.store(0, Ordering::SeqCst);
        self.attribution_error_frames.store(0, Ordering::SeqCst);
        let sequence = self.evidence_sequence.fetch_add(1, Ordering::SeqCst) + 1;
        self.cutover_evidence
            .lock()
            .unwrap()
            .push(AudioCutoverEvidence {
                sequence,
                old_device_epoch: closing_epoch,
                new_device_epoch: None,
                watermark_qpc_ns,
                drain_deadline_qpc_ns: deadline,
                old_epoch_last_capture_qpc_ns: None,
                new_epoch_first_capture_qpc_ns: None,
                late_callback_dropped_frames: 0,
                attribution_error_frames: 0,
                terminal_result: "in_progress".to_owned(),
                error: None,
            });
        self.evidence_dirty.store(true, Ordering::SeqCst);
        deadline
    }

    pub fn finish_device_cutover(&self, new_epoch: u64) {
        self.closing_device_epoch.store(u64::MAX, Ordering::SeqCst);
        self.cutover_new_epoch.store(new_epoch, Ordering::SeqCst);
        self.update_active_cutover("rebound", Some(new_epoch), None);
        self.evidence_dirty.store(true, Ordering::SeqCst);
    }

    pub fn abort_device_cutover(&self, reason: &str) -> u64 {
        let invalidating_epoch = self.advance_device_epoch();
        self.closing_device_epoch.store(u64::MAX, Ordering::SeqCst);
        self.cutover_new_epoch.store(u64::MAX, Ordering::SeqCst);
        self.update_active_cutover("failed", Some(invalidating_epoch), Some(reason.to_owned()));
        self.evidence_dirty.store(true, Ordering::SeqCst);
        invalidating_epoch
    }

    pub fn fail_device_cutover(&self, new_epoch: u64, reason: &str) {
        self.closing_device_epoch.store(u64::MAX, Ordering::SeqCst);
        self.cutover_new_epoch.store(new_epoch, Ordering::SeqCst);
        let mut cutovers = self.cutover_evidence.lock().unwrap();
        if let Some(cutover) = cutovers.last_mut() {
            cutover.new_device_epoch = Some(new_epoch);
            cutover.terminal_result = "failed".to_owned();
            cutover.error = Some(reason.to_owned());
        }
        self.evidence_dirty.store(true, Ordering::SeqCst);
    }

    fn update_active_cutover(
        &self,
        terminal_result: &str,
        new_device_epoch: Option<u64>,
        error: Option<String>,
    ) {
        if let Some(cutover) = self
            .cutover_evidence
            .lock()
            .unwrap()
            .iter_mut()
            .rev()
            .find(|cutover| cutover.terminal_result == "in_progress")
        {
            cutover.new_device_epoch = new_device_epoch;
            cutover.terminal_result = terminal_result.to_owned();
            cutover.error = error;
        }
    }

    fn freeze_latest_cutover_metrics(&self) {
        if let Some(cutover) = self.cutover_evidence.lock().unwrap().last_mut() {
            cutover.old_epoch_last_capture_qpc_ns =
                Self::option_nonzero(self.cutover_old_last_capture_qpc_ns.load(Ordering::SeqCst));
            cutover.new_epoch_first_capture_qpc_ns =
                Self::option_nonzero(self.cutover_new_first_capture_qpc_ns.load(Ordering::SeqCst));
            cutover.late_callback_dropped_frames =
                self.late_callback_dropped_frames.load(Ordering::SeqCst);
            cutover.attribution_error_frames = self.attribution_error_frames.load(Ordering::SeqCst);
        }
    }

    /// Join producer drain accounting without claiming a hardware callback/QPC.
    #[cfg(target_os = "windows")]
    pub(crate) fn begin_timeline_padding(
        self: &Arc<Self>,
        epoch: u64,
    ) -> Option<InFlightAudioCallback> {
        self.in_flight_callback_count.fetch_add(1, Ordering::SeqCst);
        let guard = InFlightAudioCallback {
            state: self.clone(),
        };
        (self.accepts_audio_timeline()
            && epoch == self.get_device_epoch()
            && epoch != self.closing_device_epoch.load(Ordering::SeqCst))
        .then_some(guard)
    }

    pub fn begin_audio_callback(
        self: &Arc<Self>,
        callback_epoch: u64,
        capture_qpc_ns: Option<u64>,
        frame_count: u64,
    ) -> Option<InFlightAudioCallback> {
        self.in_flight_callback_count.fetch_add(1, Ordering::SeqCst);
        let current_epoch = self.get_device_epoch();
        let closing_epoch = self.closing_device_epoch.load(Ordering::SeqCst);
        let watermark = self.cutover_watermark_qpc_ns.load(Ordering::SeqCst);
        let new_epoch = self.cutover_new_epoch.load(Ordering::SeqCst);
        let missing_cutover_qpc = capture_qpc_ns.is_none()
            && watermark != 0
            && (closing_epoch == callback_epoch || new_epoch == callback_epoch);
        if missing_cutover_qpc {
            self.attribution_error_frames
                .fetch_add(frame_count, Ordering::SeqCst);
        }
        let late_for_closing_epoch = closing_epoch == callback_epoch
            && capture_qpc_ns.is_some_and(|capture| capture > watermark);
        if callback_epoch != current_epoch || late_for_closing_epoch || missing_cutover_qpc {
            self.late_callback_dropped_frames
                .fetch_add(frame_count, Ordering::SeqCst);
            self.in_flight_callback_count.fetch_sub(1, Ordering::SeqCst);
            return None;
        }

        if closing_epoch == callback_epoch {
            if let Some(capture) = capture_qpc_ns {
                self.cutover_old_last_capture_qpc_ns
                    .fetch_max(capture, Ordering::SeqCst);
            }
        }
        if new_epoch == callback_epoch {
            if let Some(capture) = capture_qpc_ns {
                let _ = self.cutover_new_first_capture_qpc_ns.compare_exchange(
                    0,
                    capture,
                    Ordering::SeqCst,
                    Ordering::SeqCst,
                );
            }
        }

        Some(InFlightAudioCallback {
            state: self.clone(),
        })
    }

    pub fn callback_drain_state(&self) -> (u64, u64) {
        (
            self.in_flight_callback_count.load(Ordering::SeqCst),
            self.callback_drain_deadline_qpc_ns.load(Ordering::SeqCst),
        )
    }

    pub fn cutover_state(&self) -> (Option<u64>, Option<u64>, Option<u64>, u64, u64) {
        let watermark = self.cutover_watermark_qpc_ns.load(Ordering::SeqCst);
        let old_last = self.cutover_old_last_capture_qpc_ns.load(Ordering::SeqCst);
        let new_first = self.cutover_new_first_capture_qpc_ns.load(Ordering::SeqCst);
        (
            (watermark != 0).then_some(watermark),
            (old_last != 0).then_some(old_last),
            (new_first != 0).then_some(new_first),
            self.late_callback_dropped_frames.load(Ordering::SeqCst),
            self.attribution_error_frames.load(Ordering::SeqCst),
        )
    }

    fn option_nonzero(value: u64) -> Option<u64> {
        (value != 0).then_some(value)
    }

    fn snapshot_current_epoch(&self) -> AudioDeviceEpochEvidence {
        let microphone = self.get_microphone_device();
        let system = self.get_system_device();
        let (microphone_rms, microphone_peak) = self.route_audio_levels(&DeviceType::Microphone);
        let (system_rms, system_peak) = self.route_audio_levels(&DeviceType::System);
        let (_, _, driver_mute_behavior) = self.system_mute_state();
        AudioDeviceEpochEvidence {
            device_epoch: self.get_device_epoch(),
            microphone_name: microphone.as_ref().map(|device| device.name.clone()),
            microphone_native_id: microphone
                .as_ref()
                .and_then(|device| device.native_id.clone()),
            microphone_default_role: microphone
                .as_ref()
                .and_then(|device| device.default_role.clone()),
            system_name: system.as_ref().map(|device| device.name.clone()),
            system_native_id: system.as_ref().and_then(|device| device.native_id.clone()),
            system_default_role: system
                .as_ref()
                .and_then(|device| device.default_role.clone()),
            system_format: self.system_audio_format(),
            microphone_stream_started_qpc_ns: Self::option_nonzero(
                self.microphone_stream_started_qpc_ns.load(Ordering::SeqCst),
            ),
            system_stream_started_qpc_ns: self.system_stream_started_qpc_ns(),
            microphone_last_capture_qpc_ns: Self::option_nonzero(
                self.microphone_last_capture_qpc_ns.load(Ordering::SeqCst),
            ),
            system_last_capture_qpc_ns: Self::option_nonzero(
                self.system_last_capture_qpc_ns.load(Ordering::SeqCst),
            ),
            microphone_last_callback_observed_qpc_ns: Self::option_nonzero(
                self.microphone_last_callback_observed_qpc_ns
                    .load(Ordering::SeqCst),
            ),
            system_last_callback_observed_qpc_ns: Self::option_nonzero(
                self.system_last_callback_observed_qpc_ns
                    .load(Ordering::SeqCst),
            ),
            microphone_max_callback_gap_ns: self
                .microphone_max_callback_gap_ns
                .load(Ordering::SeqCst),
            system_max_callback_gap_ns: self.system_max_callback_gap_ns.load(Ordering::SeqCst),
            microphone_callback_count: self.microphone_callback_count.load(Ordering::SeqCst),
            microphone_frame_count: self.microphone_frame_count.load(Ordering::SeqCst),
            microphone_sample_count: self.microphone_sample_count.load(Ordering::SeqCst),
            system_callback_count: self.system_callback_count.load(Ordering::SeqCst),
            system_frame_count: self.system_frame_count.load(Ordering::SeqCst),
            system_sample_count: self.system_sample_count.load(Ordering::SeqCst),
            system_silent_flag_frame_count: self
                .system_silent_flag_frame_count
                .load(Ordering::SeqCst),
            system_all_zero_frame_count: self.system_all_zero_frame_count.load(Ordering::SeqCst),
            microphone_no_signal_count: self.microphone_no_signal_count.load(Ordering::SeqCst),
            system_no_signal_count: self.system_no_signal_count.load(Ordering::SeqCst),
            microphone_route_failed: self.microphone_route_failed.load(Ordering::SeqCst),
            system_route_failed: self.system_route_failed.load(Ordering::SeqCst),
            system_endpoint_muted: self.system_endpoint_muted.load(Ordering::SeqCst),
            system_audio_silent: self.system_audio_silent.load(Ordering::SeqCst),
            driver_mute_behavior: driver_mute_behavior.map(str::to_owned),
            microphone_rms,
            microphone_peak,
            system_rms,
            system_peak,
        }
    }

    pub fn audio_route_evidence_snapshot(&self) -> AudioRouteEvidenceSnapshot {
        let mut epochs = self.completed_epoch_evidence.lock().unwrap().clone();
        let current = self.snapshot_current_epoch();
        if let Some(saved) = epochs
            .iter_mut()
            .find(|saved| saved.device_epoch == current.device_epoch)
        {
            *saved = current;
        } else {
            epochs.push(current);
        }
        let mut cutovers = self.cutover_evidence.lock().unwrap().clone();
        if let Some(cutover) = cutovers.last_mut() {
            cutover.old_epoch_last_capture_qpc_ns =
                Self::option_nonzero(self.cutover_old_last_capture_qpc_ns.load(Ordering::SeqCst));
            cutover.new_epoch_first_capture_qpc_ns =
                Self::option_nonzero(self.cutover_new_first_capture_qpc_ns.load(Ordering::SeqCst));
            cutover.late_callback_dropped_frames =
                self.late_callback_dropped_frames.load(Ordering::SeqCst);
            cutover.attribution_error_frames = self.attribution_error_frames.load(Ordering::SeqCst);
        }
        AudioRouteEvidenceSnapshot {
            schema_version: 1,
            current_device_epoch: self.get_device_epoch(),
            microphone_stream_started_count: self.microphone_stream_started_count(),
            epochs,
            incidents: self.route_incidents.lock().unwrap().clone(),
            cutovers,
        }
    }

    pub fn take_audio_route_evidence_dirty(&self) -> bool {
        self.evidence_dirty.swap(false, Ordering::SeqCst)
    }

    pub fn mark_audio_route_evidence_dirty(&self) {
        self.evidence_dirty.store(true, Ordering::SeqCst);
    }

    fn open_route_incident(
        &self,
        route: &str,
        code: &str,
        started_qpc_ns: Option<u64>,
        callback_gap_ns: Option<u64>,
    ) {
        let mut incidents = self.route_incidents.lock().unwrap();
        if incidents.iter().rev().any(|incident| {
            incident.device_epoch == self.get_device_epoch()
                && incident.route == route
                && incident.code == code
                && incident.recovered_qpc_ns.is_none()
        }) {
            return;
        }
        let sequence = self.evidence_sequence.fetch_add(1, Ordering::SeqCst) + 1;
        incidents.push(AudioRouteIncidentEvidence {
            sequence,
            device_epoch: self.get_device_epoch(),
            route: route.to_owned(),
            code: code.to_owned(),
            started_qpc_ns,
            recovered_qpc_ns: None,
            callback_gap_ns,
        });
        self.evidence_dirty.store(true, Ordering::SeqCst);
    }

    /// 已保存的录音设备不可用、已退回系统默认设备时登记这次回退。
    /// 只记录事件码，不改证据结构；实际使用的设备名仍在录音元数据的 devices 里。
    pub fn record_device_fallback(&self, route: &str) {
        self.open_route_incident(route, "preferred_device_fallback", None, None);
    }

    fn recover_route_incidents(&self, route: &str, recovered_qpc_ns: u64) {
        for incident in self.route_incidents.lock().unwrap().iter_mut().rev() {
            if incident.device_epoch != self.get_device_epoch() {
                break;
            }
            if incident.route == route && incident.recovered_qpc_ns.is_none() {
                incident.recovered_qpc_ns = Some(recovered_qpc_ns);
                self.evidence_dirty.store(true, Ordering::SeqCst);
            }
        }
    }

    pub fn mark_system_stream_started(&self, qpc_ns: u64) {
        let _ = self.system_stream_started_qpc_ns.compare_exchange(
            0,
            qpc_ns,
            Ordering::SeqCst,
            Ordering::SeqCst,
        );
        self.evidence_dirty.store(true, Ordering::SeqCst);
    }

    pub fn system_stream_started_qpc_ns(&self) -> Option<u64> {
        match self.system_stream_started_qpc_ns.load(Ordering::SeqCst) {
            0 => None,
            value => Some(value),
        }
    }

    pub fn set_system_audio_format(&self, format: SystemAudioFormat) {
        *self.system_audio_format.lock().unwrap() = Some(format);
        self.evidence_dirty.store(true, Ordering::SeqCst);
    }

    pub fn system_audio_format(&self) -> Option<SystemAudioFormat> {
        self.system_audio_format.lock().unwrap().clone()
    }

    pub fn record_system_callback_qpc(
        &self,
        capture_qpc_ns: u64,
        observed_qpc_ns: u64,
        frames: u32,
        silent_flag: bool,
        all_zero_frames: u32,
        endpoint_muted: bool,
    ) {
        self.system_last_capture_qpc_ns
            .store(capture_qpc_ns, Ordering::SeqCst);
        let prior = self
            .system_last_callback_observed_qpc_ns
            .swap(observed_qpc_ns, Ordering::SeqCst);
        let reference = if prior == 0 {
            self.system_stream_started_qpc_ns.load(Ordering::SeqCst)
        } else {
            prior
        };
        if reference > 0 && observed_qpc_ns >= reference {
            self.system_max_callback_gap_ns
                .fetch_max(observed_qpc_ns - reference, Ordering::SeqCst);
        }
        let recovered_no_signal = self.system_no_signal.swap(false, Ordering::SeqCst);
        let recovered_stream_error = self.system_route_failed.swap(false, Ordering::SeqCst);
        if recovered_no_signal || recovered_stream_error {
            self.recover_route_incidents("system", observed_qpc_ns);
        }
        self.system_frame_count
            .fetch_add(frames as u64, Ordering::SeqCst);
        if silent_flag {
            self.system_silent_flag_frame_count
                .fetch_add(frames as u64, Ordering::SeqCst);
        }
        self.system_all_zero_frame_count
            .fetch_add(all_zero_frames as u64, Ordering::SeqCst);
        let was_muted = self
            .system_endpoint_muted
            .swap(endpoint_muted, Ordering::SeqCst);
        if endpoint_muted && !was_muted {
            self.driver_mute_behavior.store(0, Ordering::SeqCst);
            self.system_audio_silent.store(false, Ordering::SeqCst);
            self.system_muted_zero_since_qpc_ns
                .store(observed_qpc_ns, Ordering::SeqCst);
        }
        if endpoint_muted && frames > 0 && all_zero_frames == frames {
            if self.driver_mute_behavior.load(Ordering::SeqCst) == 1 {
                self.system_audio_silent.store(false, Ordering::SeqCst);
            } else {
                self.driver_mute_behavior.store(2, Ordering::SeqCst);
                let zero_since = self.system_muted_zero_since_qpc_ns.load(Ordering::SeqCst);
                if zero_since == 0 {
                    self.system_muted_zero_since_qpc_ns
                        .store(observed_qpc_ns, Ordering::SeqCst);
                } else if observed_qpc_ns.saturating_sub(zero_since) >= AUDIO_CALLBACK_DEADLINE_NS
                    && !self.system_audio_silent.swap(true, Ordering::SeqCst)
                {
                    self.open_route_incident(
                        "system",
                        "silent_frames_while_muted",
                        Some(zero_since),
                        Some(observed_qpc_ns.saturating_sub(zero_since)),
                    );
                    self.report_route_error(AudioError::SystemAudioSilent);
                }
            }
        } else {
            if endpoint_muted && frames > 0 {
                self.driver_mute_behavior.store(1, Ordering::SeqCst);
            }
            if self.system_audio_silent.swap(false, Ordering::SeqCst) {
                self.recover_route_incidents("system", observed_qpc_ns);
            }
            self.system_muted_zero_since_qpc_ns
                .store(0, Ordering::SeqCst);
        }
    }

    pub fn report_system_no_signal(&self, gap_ns: u64, endpoint_muted: bool) {
        self.system_max_callback_gap_ns
            .fetch_max(gap_ns, Ordering::SeqCst);
        self.system_no_signal.store(true, Ordering::SeqCst);
        self.system_rms_bits
            .store(0.0_f32.to_bits(), Ordering::SeqCst);
        self.system_peak_bits
            .store(0.0_f32.to_bits(), Ordering::SeqCst);
        self.system_no_signal_count.fetch_add(1, Ordering::SeqCst);
        let last_observed = self
            .system_last_callback_observed_qpc_ns
            .load(Ordering::SeqCst);
        let stream_started = self.system_stream_started_qpc_ns.load(Ordering::SeqCst);
        let started_qpc_ns = Self::option_nonzero(if last_observed != 0 {
            last_observed
        } else {
            stream_started
        });
        self.open_route_incident("system", "no_signal", started_qpc_ns, Some(gap_ns));
        let was_muted = self
            .system_endpoint_muted
            .swap(endpoint_muted, Ordering::SeqCst);
        if endpoint_muted && !was_muted {
            self.driver_mute_behavior.store(0, Ordering::SeqCst);
            self.system_audio_silent.store(false, Ordering::SeqCst);
            self.system_muted_zero_since_qpc_ns
                .store(0, Ordering::SeqCst);
        }
        if endpoint_muted && self.driver_mute_behavior.load(Ordering::SeqCst) == 0 {
            self.driver_mute_behavior.store(3, Ordering::SeqCst);
        }
        self.report_route_error(AudioError::SystemAudioNoSignal);
        log::error!(
            "System audio callback watchdog expired: gap_ns={} threshold_ns=2000000000",
            gap_ns
        );
    }

    fn report_route_error(&self, error: AudioError) {
        self.error_count.fetch_add(1, Ordering::SeqCst);
        self.recoverable_error_count.fetch_add(1, Ordering::SeqCst);
        *self.last_error.lock().unwrap() = Some(error.clone());
        if let Some(callback) = self.error_callback.lock().unwrap().as_ref() {
            callback(&error);
        }
    }

    pub fn system_route_health(&self) -> (Option<u64>, Option<u64>, u64, bool, u64) {
        let capture = self.system_last_capture_qpc_ns.load(Ordering::SeqCst);
        let observed = self
            .system_last_callback_observed_qpc_ns
            .load(Ordering::SeqCst);
        (
            (capture != 0).then_some(capture),
            (observed != 0).then_some(observed),
            self.system_max_callback_gap_ns.load(Ordering::SeqCst),
            self.system_no_signal.load(Ordering::SeqCst),
            self.system_no_signal_count.load(Ordering::SeqCst),
        )
    }

    pub fn system_native_frame_counts(&self) -> (u64, u64, u64) {
        (
            self.system_frame_count.load(Ordering::SeqCst),
            self.system_silent_flag_frame_count.load(Ordering::SeqCst),
            self.system_all_zero_frame_count.load(Ordering::SeqCst),
        )
    }

    pub fn system_mute_state(&self) -> (bool, bool, Option<&'static str>) {
        let behavior = match self.driver_mute_behavior.load(Ordering::SeqCst) {
            1 => Some("audible_frames"),
            2 => Some("silent_frames"),
            3 => Some("no_callback"),
            _ => None,
        };
        (
            self.system_endpoint_muted.load(Ordering::SeqCst),
            self.system_audio_silent.load(Ordering::SeqCst),
            behavior,
        )
    }

    pub fn mark_microphone_stream_started(&self, qpc_ns: u64) {
        self.microphone_stream_started_count
            .fetch_add(1, Ordering::SeqCst);
        self.microphone_stream_started_qpc_ns
            .store(qpc_ns, Ordering::SeqCst);
        self.microphone_no_signal.store(false, Ordering::SeqCst);
        self.microphone_route_failed.store(false, Ordering::SeqCst);
        self.evidence_dirty.store(true, Ordering::SeqCst);
    }

    pub fn microphone_stream_started_count(&self) -> u64 {
        self.microphone_stream_started_count.load(Ordering::SeqCst)
    }

    pub fn record_microphone_callback_qpc(
        &self,
        capture_qpc_ns: Option<u64>,
        observed_qpc_ns: u64,
    ) {
        if let Some(capture_qpc_ns) = capture_qpc_ns {
            self.microphone_last_capture_qpc_ns
                .store(capture_qpc_ns, Ordering::SeqCst);
        }
        let prior = self
            .microphone_last_callback_observed_qpc_ns
            .swap(observed_qpc_ns, Ordering::SeqCst);
        let reference = if prior == 0 {
            self.microphone_stream_started_qpc_ns.load(Ordering::SeqCst)
        } else {
            prior
        };
        if reference > 0 && observed_qpc_ns >= reference {
            self.microphone_max_callback_gap_ns
                .fetch_max(observed_qpc_ns - reference, Ordering::SeqCst);
        }
        let recovered_no_signal = self.microphone_no_signal.swap(false, Ordering::SeqCst);
        let recovered_stream_error = self.microphone_route_failed.swap(false, Ordering::SeqCst);
        if recovered_no_signal || recovered_stream_error {
            self.recover_route_incidents("microphone", observed_qpc_ns);
        }
    }

    pub fn report_microphone_no_signal(&self, gap_ns: u64) {
        self.microphone_max_callback_gap_ns
            .fetch_max(gap_ns, Ordering::SeqCst);
        self.microphone_no_signal.store(true, Ordering::SeqCst);
        self.microphone_no_signal_count
            .fetch_add(1, Ordering::SeqCst);
        let last_observed = self
            .microphone_last_callback_observed_qpc_ns
            .load(Ordering::SeqCst);
        let stream_started = self.microphone_stream_started_qpc_ns.load(Ordering::SeqCst);
        let started_qpc_ns = Self::option_nonzero(if last_observed != 0 {
            last_observed
        } else {
            stream_started
        });
        self.open_route_incident("microphone", "no_signal", started_qpc_ns, Some(gap_ns));
        self.microphone_rms_bits
            .store(0.0_f32.to_bits(), Ordering::SeqCst);
        self.microphone_peak_bits
            .store(0.0_f32.to_bits(), Ordering::SeqCst);
        self.report_route_error(AudioError::MicrophoneNoSignal);
    }

    pub fn microphone_route_health(&self) -> (Option<u64>, Option<u64>, u64, bool, u64) {
        let capture = self.microphone_last_capture_qpc_ns.load(Ordering::SeqCst);
        let observed = self
            .microphone_last_callback_observed_qpc_ns
            .load(Ordering::SeqCst);
        (
            (capture != 0).then_some(capture),
            (observed != 0).then_some(observed),
            self.microphone_max_callback_gap_ns.load(Ordering::SeqCst),
            self.microphone_no_signal.load(Ordering::SeqCst),
            self.microphone_no_signal_count.load(Ordering::SeqCst),
        )
    }

    pub fn route_failed(&self, device_type: &DeviceType) -> bool {
        match device_type {
            DeviceType::Microphone => self.microphone_route_failed.load(Ordering::SeqCst),
            DeviceType::System => self.system_route_failed.load(Ordering::SeqCst),
        }
    }

    pub fn report_route_stream_error(&self, device_type: &DeviceType, error: AudioError) {
        let route = match device_type {
            DeviceType::Microphone => "microphone",
            DeviceType::System => "system",
        };
        // Some stream-error callbacks cannot obtain QPC after the native API
        // has failed. Preserve that fact as null instead of substituting the
        // timestamp of an earlier audio callback.
        self.open_route_incident(route, &format!("{:?}", error), None, None);
        match device_type {
            DeviceType::Microphone => {
                self.microphone_route_failed.store(true, Ordering::SeqCst);
                self.microphone_rms_bits
                    .store(0.0_f32.to_bits(), Ordering::SeqCst);
                self.microphone_peak_bits
                    .store(0.0_f32.to_bits(), Ordering::SeqCst);
            }
            DeviceType::System => {
                self.system_route_failed.store(true, Ordering::SeqCst);
                self.system_rms_bits
                    .store(0.0_f32.to_bits(), Ordering::SeqCst);
                self.system_peak_bits
                    .store(0.0_f32.to_bits(), Ordering::SeqCst);
            }
        }
        self.report_error(error);
    }

    pub fn record_route_callback(&self, device_type: &DeviceType, samples: &[f32], channels: u16) {
        let sample_count = samples.len() as u64;
        let frame_count = samples
            .len()
            .checked_div(channels.max(1) as usize)
            .unwrap_or(0) as u64;
        let (rms, peak) = if samples.is_empty() {
            (0.0_f32, 0.0_f32)
        } else {
            let energy = samples
                .iter()
                .map(|sample| {
                    let value = if sample.is_finite() { *sample } else { 0.0 };
                    value * value
                })
                .sum::<f32>();
            let peak = samples
                .iter()
                .filter(|sample| sample.is_finite())
                .map(|sample| sample.abs())
                .fold(0.0_f32, f32::max);
            ((energy / samples.len() as f32).sqrt(), peak)
        };
        match device_type {
            DeviceType::Microphone => {
                self.microphone_callback_count
                    .fetch_add(1, Ordering::SeqCst);
                self.microphone_frame_count
                    .fetch_add(frame_count, Ordering::SeqCst);
                self.microphone_sample_count
                    .fetch_add(sample_count, Ordering::SeqCst);
                self.microphone_rms_bits
                    .store(rms.to_bits(), Ordering::SeqCst);
                self.microphone_peak_bits
                    .store(peak.to_bits(), Ordering::SeqCst);
            }
            DeviceType::System => {
                self.system_callback_count.fetch_add(1, Ordering::SeqCst);
                self.system_sample_count
                    .fetch_add(sample_count, Ordering::SeqCst);
                self.system_rms_bits.store(rms.to_bits(), Ordering::SeqCst);
                self.system_peak_bits
                    .store(peak.to_bits(), Ordering::SeqCst);
            }
        }
    }

    pub fn route_audio_levels(&self, device_type: &DeviceType) -> (f32, f32) {
        match device_type {
            DeviceType::Microphone => (
                f32::from_bits(self.microphone_rms_bits.load(Ordering::SeqCst)),
                f32::from_bits(self.microphone_peak_bits.load(Ordering::SeqCst)),
            ),
            DeviceType::System => (
                f32::from_bits(self.system_rms_bits.load(Ordering::SeqCst)),
                f32::from_bits(self.system_peak_bits.load(Ordering::SeqCst)),
            ),
        }
    }

    pub fn route_callback_counts(&self, device_type: &DeviceType) -> (u64, u64) {
        match device_type {
            DeviceType::Microphone => (
                self.microphone_callback_count.load(Ordering::SeqCst),
                self.microphone_sample_count.load(Ordering::SeqCst),
            ),
            DeviceType::System => (
                self.system_callback_count.load(Ordering::SeqCst),
                self.system_sample_count.load(Ordering::SeqCst),
            ),
        }
    }

    pub fn microphone_frame_count(&self) -> u64 {
        self.microphone_frame_count.load(Ordering::SeqCst)
    }

    // Audio pipeline management
    pub fn set_audio_sender(&self, sender: mpsc::UnboundedSender<AudioChunk>) {
        *self.audio_sender.lock().unwrap() = Some(sender);
    }

    pub fn send_audio_chunk(&self, chunk: AudioChunk) -> Result<()> {
        // Recoverable Windows system silence is represented by explicit zero
        // samples, so it must not stop the microphone or the recording clock.
        if !self.accepts_audio_timeline() {
            return Ok(());
        }

        if let Some(sender) = self.audio_sender.lock().unwrap().as_ref() {
            sender
                .send(chunk)
                .map_err(|_| anyhow::anyhow!("Failed to send audio chunk"))?;

            // Update statistics
            let mut stats = self.stats.lock().unwrap();
            stats.chunks_processed += 1;
            stats.last_activity = Some(Instant::now());
            Ok(())
        } else {
            // Return an error when no sender is available (pipeline not ready)
            Err(anyhow::anyhow!(
                "Audio pipeline not ready - no sender available"
            ))
        }
    }

    // Error handling
    pub fn set_error_callback<F>(&self, callback: F)
    where
        F: Fn(&AudioError) + Send + Sync + 'static,
    {
        *self.error_callback.lock().unwrap() = Some(Box::new(callback));
    }

    pub fn report_error(&self, error: AudioError) {
        let count = self.error_count.fetch_add(1, Ordering::SeqCst) + 1;

        // Track recoverable vs non-recoverable errors separately
        if error.is_recoverable() {
            let recoverable_count = self.recoverable_error_count.fetch_add(1, Ordering::SeqCst) + 1;
            log::warn!(
                "Recoverable audio error ({}): {:?}",
                recoverable_count,
                error
            );

            // Allow more recoverable errors before stopping
            if recoverable_count >= 10 {
                log::error!(
                    "Too many recoverable errors ({}), stopping recording",
                    recoverable_count
                );
                self.stop_recording();
            }
        } else {
            log::error!("Non-recoverable audio error: {:?}", error);
            // Stop immediately for non-recoverable errors
            self.stop_recording();
        }

        *self.last_error.lock().unwrap() = Some(error.clone());

        // Call error callback if set
        if let Some(callback) = self.error_callback.lock().unwrap().as_ref() {
            callback(&error);
        }

        // Fallback: stop recording after too many total errors
        if count >= 15 {
            log::error!(
                "Too many total audio errors ({}), stopping recording",
                count
            );
            self.stop_recording();
        }
    }

    pub fn get_error_count(&self) -> u32 {
        self.error_count.load(Ordering::SeqCst)
    }

    pub fn get_recoverable_error_count(&self) -> u32 {
        self.recoverable_error_count.load(Ordering::SeqCst)
    }

    pub fn get_last_error(&self) -> Option<AudioError> {
        self.last_error.lock().unwrap().clone()
    }

    pub fn has_fatal_error(&self) -> bool {
        if let Some(error) = &*self.last_error.lock().unwrap() {
            !error.is_recoverable() && self.error_count.load(Ordering::SeqCst) > 0
        } else {
            false
        }
    }

    // Statistics
    pub fn get_stats(&self) -> RecordingStats {
        self.stats.lock().unwrap().clone()
    }

    pub fn get_recording_duration(&self) -> Option<f64> {
        self.recording_start
            .lock()
            .unwrap()
            .map(|start| start.elapsed().as_secs_f64())
    }

    pub fn get_active_recording_duration(&self) -> Option<f64> {
        self.recording_start.lock().unwrap().map(|start| {
            let total_duration = start.elapsed().as_secs_f64();
            let pause_duration = self.get_total_pause_duration();
            let current_pause = if self.is_paused() {
                self.pause_start
                    .lock()
                    .unwrap()
                    .map(|p| p.elapsed().as_secs_f64())
                    .unwrap_or(0.0)
            } else {
                0.0
            };
            total_duration - pause_duration - current_pause
        })
    }

    pub fn get_total_pause_duration(&self) -> f64 {
        self.total_pause_duration.lock().unwrap().as_secs_f64()
    }

    pub fn get_current_pause_duration(&self) -> Option<f64> {
        if self.is_paused() {
            self.pause_start
                .lock()
                .unwrap()
                .map(|start| start.elapsed().as_secs_f64())
        } else {
            None
        }
    }

    // Memory management
    pub fn get_buffer_pool(&self) -> AudioBufferPool {
        self.buffer_pool.clone()
    }

    // Cleanup
    pub fn cleanup(&self) {
        self.stop_recording();
        self.stop_reconnecting();
        *self.microphone_device.lock().unwrap() = None;
        *self.system_device.lock().unwrap() = None;
        *self.disconnected_device.lock().unwrap() = None;
        *self.audio_sender.lock().unwrap() = None;
        *self.last_error.lock().unwrap() = None;
        *self.error_callback.lock().unwrap() = None;
        *self.stats.lock().unwrap() = RecordingStats::default();
        *self.recording_start.lock().unwrap() = None;
        *self.pause_start.lock().unwrap() = None;
        *self.total_pause_duration.lock().unwrap() = std::time::Duration::ZERO;
        self.error_count.store(0, Ordering::SeqCst);
        self.recoverable_error_count.store(0, Ordering::SeqCst);

        // Clear buffer pool to free memory
        self.buffer_pool.clear();
    }
}

impl Default for RecordingState {
    fn default() -> Self {
        Self {
            is_recording: AtomicBool::new(false),
            is_paused: AtomicBool::new(false),
            is_reconnecting: AtomicBool::new(false),
            routes_ready: AtomicBool::new(false),
            microphone_device: Mutex::new(None),
            system_device: Mutex::new(None),
            disconnected_device: Mutex::new(None),
            audio_sender: Mutex::new(None),
            buffer_pool: AudioBufferPool::new(16, 48000), // Pool of 16 buffers with 48kHz samples capacity
            error_count: AtomicU32::new(0),
            recoverable_error_count: AtomicU32::new(0),
            last_error: Mutex::new(None),
            error_callback: Mutex::new(None),
            stats: Mutex::new(RecordingStats::default()),
            device_epoch: AtomicU64::new(0),
            system_stream_started_qpc_ns: AtomicU64::new(0),
            system_last_capture_qpc_ns: AtomicU64::new(0),
            system_last_callback_observed_qpc_ns: AtomicU64::new(0),
            system_max_callback_gap_ns: AtomicU64::new(0),
            system_no_signal: AtomicBool::new(false),
            system_no_signal_count: AtomicU64::new(0),
            system_endpoint_muted: AtomicBool::new(false),
            system_audio_silent: AtomicBool::new(false),
            driver_mute_behavior: AtomicU32::new(0),
            system_muted_zero_since_qpc_ns: AtomicU64::new(0),
            system_frame_count: AtomicU64::new(0),
            system_silent_flag_frame_count: AtomicU64::new(0),
            system_all_zero_frame_count: AtomicU64::new(0),
            system_audio_format: Mutex::new(None),
            closing_device_epoch: AtomicU64::new(u64::MAX),
            cutover_watermark_qpc_ns: AtomicU64::new(0),
            callback_drain_deadline_qpc_ns: AtomicU64::new(0),
            in_flight_callback_count: AtomicU64::new(0),
            late_callback_dropped_frames: AtomicU64::new(0),
            attribution_error_frames: AtomicU64::new(0),
            cutover_old_last_capture_qpc_ns: AtomicU64::new(0),
            cutover_new_epoch: AtomicU64::new(u64::MAX),
            cutover_new_first_capture_qpc_ns: AtomicU64::new(0),
            microphone_stream_started_count: AtomicU64::new(0),
            microphone_stream_started_qpc_ns: AtomicU64::new(0),
            microphone_last_capture_qpc_ns: AtomicU64::new(0),
            microphone_last_callback_observed_qpc_ns: AtomicU64::new(0),
            microphone_max_callback_gap_ns: AtomicU64::new(0),
            microphone_no_signal: AtomicBool::new(false),
            microphone_no_signal_count: AtomicU64::new(0),
            microphone_route_failed: AtomicBool::new(false),
            system_route_failed: AtomicBool::new(false),
            microphone_callback_count: AtomicU64::new(0),
            microphone_frame_count: AtomicU64::new(0),
            microphone_sample_count: AtomicU64::new(0),
            system_callback_count: AtomicU64::new(0),
            system_sample_count: AtomicU64::new(0),
            microphone_rms_bits: AtomicU32::new(0.0_f32.to_bits()),
            microphone_peak_bits: AtomicU32::new(0.0_f32.to_bits()),
            system_rms_bits: AtomicU32::new(0.0_f32.to_bits()),
            system_peak_bits: AtomicU32::new(0.0_f32.to_bits()),
            completed_epoch_evidence: Mutex::new(Vec::new()),
            route_incidents: Mutex::new(Vec::new()),
            cutover_evidence: Mutex::new(Vec::new()),
            evidence_sequence: AtomicU64::new(0),
            evidence_dirty: AtomicBool::new(false),
            recording_start: Mutex::new(None),
            pause_start: Mutex::new(None),
            total_pause_duration: Mutex::new(std::time::Duration::ZERO),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn route_counters_are_independent_and_reset_on_start() {
        let state = RecordingState::new();
        state.record_route_callback(&DeviceType::Microphone, &vec![0.5; 480], 1);
        state.record_route_callback(&DeviceType::System, &vec![0.25; 960], 2);
        state.record_route_callback(&DeviceType::System, &vec![0.25; 480], 2);
        assert_eq!(
            state.route_callback_counts(&DeviceType::Microphone),
            (1, 480)
        );
        assert_eq!(state.microphone_frame_count(), 480);
        assert_eq!(state.route_callback_counts(&DeviceType::System), (2, 1440));
        assert_eq!(
            state.route_audio_levels(&DeviceType::Microphone),
            (0.5, 0.5)
        );
        assert_eq!(state.route_audio_levels(&DeviceType::System), (0.25, 0.25));

        state.start_recording().unwrap();
        assert_eq!(state.route_callback_counts(&DeviceType::Microphone), (0, 0));
        assert_eq!(state.microphone_frame_count(), 0);
        assert_eq!(state.route_callback_counts(&DeviceType::System), (0, 0));
    }

    #[test]
    fn muted_driver_branches_remain_distinct() {
        let state = RecordingState::new();
        state.start_recording().unwrap();

        state.record_system_callback_qpc(1, 10, 480, true, 480, true);
        assert_eq!(
            state.system_mute_state(),
            (true, false, Some("silent_frames"))
        );
        state.record_system_callback_qpc(2, 2_000_000_010, 480, true, 480, true);
        assert_eq!(
            state.system_mute_state(),
            (true, true, Some("silent_frames"))
        );
        assert!(!state.system_route_health().3);

        state.advance_device_epoch();
        state.record_system_callback_qpc(3, 3_000_000_000, 480, false, 0, true);
        state.record_system_callback_qpc(4, 5_500_000_000, 480, true, 480, true);
        assert_eq!(
            state.system_mute_state(),
            (true, false, Some("audible_frames"))
        );

        state.advance_device_epoch();
        state.report_system_no_signal(2_000_000_000, true);
        assert_eq!(
            state.system_mute_state(),
            (true, false, Some("no_callback"))
        );
        assert!(state.system_route_health().3);
    }

    #[test]
    fn recoverable_system_silence_keeps_pipeline_input_open() {
        let state = RecordingState::new();
        state.start_recording().unwrap();
        state.mark_routes_ready();
        let (sender, mut receiver) = mpsc::unbounded_channel();
        state.set_audio_sender(sender);
        state.record_system_callback_qpc(1, 10, 480, true, 480, true);
        state.record_system_callback_qpc(2, 2_000_000_010, 480, true, 480, true);
        assert!(state.system_mute_state().1);

        state
            .send_audio_chunk(AudioChunk {
                data: vec![0.25; 480],
                sample_rate: 48_000,
                timestamp: 0.0,
                chunk_id: 1,
                device_type: DeviceType::Microphone,
                device_epoch: 0,
                capture_qpc_ns: Some(2_000_000_020),
            })
            .unwrap();
        assert_eq!(receiver.try_recv().unwrap().data, vec![0.25; 480]);
    }

    #[test]
    fn recoverable_system_no_callback_keeps_timeline_padding_open() {
        let state = RecordingState::new();
        state.start_recording().unwrap();
        state.mark_routes_ready();
        state.report_system_no_signal(AUDIO_CALLBACK_DEADLINE_NS, true);

        assert!(state.begin_timeline_padding(0).is_some());
    }

    #[test]
    fn endpoint_epoch_resets_system_route_health() {
        let state = RecordingState::new();
        assert_eq!(state.get_device_epoch(), 0);
        assert_eq!(state.advance_device_epoch(), 1);
        assert_eq!(state.get_device_epoch(), 1);

        state.mark_system_stream_started(123);
        state.mark_system_stream_started(456);
        assert_eq!(state.system_stream_started_qpc_ns(), Some(123));
        state.record_system_callback_qpc(125, 130, 480, true, 480, false);
        state.report_system_no_signal(2_000_000_000, false);
        assert_eq!(
            state.system_route_health(),
            (Some(125), Some(130), 2_000_000_000, true, 1)
        );

        assert_eq!(state.advance_device_epoch(), 2);
        assert_eq!(state.system_stream_started_qpc_ns(), None);
        assert_eq!(state.system_route_health(), (None, None, 0, false, 0));
        assert_eq!(state.system_native_frame_counts(), (0, 0, 0));
    }

    #[test]
    fn cutover_accepts_old_frames_through_watermark_and_drops_later_frames() {
        let state = RecordingState::new();
        state.start_recording().unwrap();
        assert_eq!(state.begin_device_cutover(1_000), 2_000_001_000);
        let guard = state.begin_audio_callback(0, Some(1_000), 480);
        assert!(guard.is_some());
        assert_eq!(state.callback_drain_state().0, 1);
        drop(guard);
        assert_eq!(state.callback_drain_state().0, 0);
        assert!(state.begin_audio_callback(0, Some(1_001), 240).is_none());
        assert_eq!(
            state.cutover_state(),
            (Some(1_000), Some(1_000), None, 240, 0)
        );

        let epoch = state.advance_device_epoch();
        state.finish_device_cutover(epoch);
        let guard = state.begin_audio_callback(epoch, Some(1_100), 480);
        assert!(guard.is_some());
        drop(guard);
        assert_eq!(
            state.cutover_state(),
            (Some(1_000), Some(1_000), Some(1_100), 240, 0)
        );

        let evidence = state.audio_route_evidence_snapshot();
        assert_eq!(evidence.epochs.len(), 2);
        assert_eq!(evidence.cutovers.len(), 1);
        assert_eq!(evidence.cutovers[0].terminal_result, "rebound");
        assert_eq!(evidence.cutovers[0].old_device_epoch, 0);
        assert_eq!(evidence.cutovers[0].new_device_epoch, Some(1));
        assert_eq!(evidence.cutovers[0].late_callback_dropped_frames, 240);
        assert_eq!(
            evidence.cutovers[0].new_epoch_first_capture_qpc_ns,
            Some(1_100)
        );
    }

    #[test]
    fn route_evidence_preserves_completed_epoch_and_no_signal_interval() {
        let state = RecordingState::new();
        state.start_recording().unwrap();
        state.mark_microphone_stream_started(100);
        state.record_microphone_callback_qpc(Some(110), 120);
        state.record_route_callback(&DeviceType::Microphone, &[0.25; 480], 1);
        state.report_microphone_no_signal(2_000_000_000);
        state.record_microphone_callback_qpc(Some(130), 2_000_000_130);
        assert_eq!(state.advance_device_epoch(), 1);

        let evidence = state.audio_route_evidence_snapshot();
        assert_eq!(evidence.schema_version, 1);
        assert_eq!(evidence.current_device_epoch, 1);
        assert_eq!(evidence.epochs.len(), 2);
        assert_eq!(evidence.epochs[0].device_epoch, 0);
        assert_eq!(evidence.epochs[0].microphone_callback_count, 1);
        assert_eq!(evidence.epochs[0].microphone_frame_count, 480);
        assert_eq!(evidence.epochs[0].microphone_no_signal_count, 1);
        assert_eq!(evidence.incidents.len(), 1);
        assert_eq!(evidence.incidents[0].route, "microphone");
        assert_eq!(evidence.incidents[0].code, "no_signal");
        assert_eq!(evidence.incidents[0].started_qpc_ns, Some(120));
        assert_eq!(evidence.incidents[0].recovered_qpc_ns, Some(2_000_000_130));
    }

    #[test]
    fn preferred_device_fallback_is_recorded_once_per_route() {
        let state = RecordingState::new();
        state.start_recording().unwrap();
        state.record_device_fallback("microphone");
        state.record_device_fallback("system");
        // 同一路由重复回退只登记一条，避免证据被重复刷屏
        state.record_device_fallback("microphone");

        let evidence = state.audio_route_evidence_snapshot();
        assert_eq!(evidence.incidents.len(), 2);
        assert_eq!(evidence.incidents[0].route, "microphone");
        assert_eq!(evidence.incidents[0].code, "preferred_device_fallback");
        assert_eq!(evidence.incidents[1].route, "system");
        assert_eq!(evidence.incidents[1].code, "preferred_device_fallback");
        assert!(evidence
            .incidents
            .iter()
            .all(|incident| incident.started_qpc_ns.is_none()));
    }

    #[test]
    fn failed_cutover_has_one_explicit_terminal_result() {
        let state = RecordingState::new();
        state.start_recording().unwrap();
        state.begin_device_cutover(10_000);
        assert_eq!(state.abort_device_cutover("callback_drain_timeout"), 1);

        let evidence = state.audio_route_evidence_snapshot();
        assert_eq!(evidence.cutovers.len(), 1);
        assert_eq!(evidence.cutovers[0].terminal_result, "failed");
        assert_eq!(
            evidence.cutovers[0].error.as_deref(),
            Some("callback_drain_timeout")
        );
    }
}

// Thread-safe cloning for RecordingStats
impl Clone for RecordingStats {
    fn clone(&self) -> Self {
        Self {
            chunks_processed: self.chunks_processed,
            total_duration: self.total_duration,
            last_activity: self.last_activity,
        }
    }
}

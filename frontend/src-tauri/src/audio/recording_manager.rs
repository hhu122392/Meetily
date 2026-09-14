use anyhow::Result;
use log::{debug, error, info, warn};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::mpsc;

use super::devices::{list_audio_devices, AudioDevice};

#[cfg(target_os = "macos")]
use super::devices::get_safe_recording_devices_macos;

use super::device_monitor::{AudioDeviceMonitor, DeviceEvent, DeviceMonitorType};
#[cfg(not(target_os = "macos"))]
use super::devices::{default_input_device, default_output_device};
use super::measurement::{D11MeasurementRecorder, RealtimeAudioInputContract};
use super::pipeline::AudioPipelineManager;
use super::recording_saver::RecordingSaver;
use super::recording_state::{AudioChunk, DeviceType as RecordingDeviceType, RecordingState};
use super::stream::AudioStreamManager;
use crate::meeting_context::{MeetingContextContainer, RecordingSummaryTemplatePreference};

/// Stream manager type enumeration
pub enum StreamManagerType {
    Standard(AudioStreamManager),
}

/// Simplified recording manager that coordinates all audio components
pub struct RecordingManager {
    state: Arc<RecordingState>,
    stream_manager: AudioStreamManager,
    pipeline_manager: AudioPipelineManager,
    recording_saver: RecordingSaver,
    device_monitor: Option<AudioDeviceMonitor>,
    device_event_receiver: Option<mpsc::UnboundedReceiver<DeviceEvent>>,
    route_evidence_persist_error: Option<String>,
}

// SAFETY: RecordingManager contains types that we've marked as Send
unsafe impl Send for RecordingManager {}

impl RecordingManager {
    pub fn persist_audio_route_evidence(&mut self) -> Result<()> {
        self.persist_audio_route_evidence_inner(false)
    }

    fn persist_audio_route_evidence_inner(&mut self, force: bool) -> Result<()> {
        if !force && !self.state.take_audio_route_evidence_dirty() {
            return Ok(());
        }
        let result = self
            .recording_saver
            .set_audio_route_evidence(self.state.audio_route_evidence_snapshot());
        if result.is_err() {
            self.state.mark_audio_route_evidence_dirty();
        }
        result
    }

    fn persist_device_binding(&mut self, device_epoch: u64) -> Result<()> {
        let microphone = self.state.get_microphone_device();
        let system_audio = self.state.get_system_device();
        self.recording_saver.set_audio_devices(
            microphone.as_deref(),
            system_audio.as_deref(),
            device_epoch,
        )?;
        self.persist_audio_route_evidence()
    }

    /// 已保存的设备不可用、录音改用系统默认设备时登记这次回退。
    /// 录音器还没准备好时只标记脏位，随后仍会在状态轮询或停止时落盘。
    pub fn record_preferred_device_fallback(&mut self, route: &str) {
        self.state.record_device_fallback(route);
        let _ = self.persist_audio_route_evidence();
    }

    #[cfg(target_os = "windows")]
    async fn prepare_stream_rebind(&mut self) -> Result<u64> {
        let watermark_qpc_ns = super::windows_loopback::qpc_now_ns()?;
        let deadline_qpc_ns = self.state.begin_device_cutover(watermark_qpc_ns);
        let now_qpc_ns = match super::windows_loopback::qpc_now_ns() {
            Ok(value) => value,
            Err(error) => {
                self.state
                    .abort_device_cutover("qpc_read_failed_before_stop");
                return Err(error);
            }
        };
        let remaining_ns = deadline_qpc_ns.saturating_sub(now_qpc_ns);
        if remaining_ns == 0 {
            self.state
                .abort_device_cutover("drain_deadline_expired_before_stop");
            return Err(anyhow::anyhow!(
                "Audio callback drain deadline expired before old streams could stop"
            ));
        }
        if let Err(error) = self
            .stream_manager
            .stop_streams_with_timeout_duration(std::time::Duration::from_nanos(remaining_ns))
            .await
        {
            self.state.abort_device_cutover("stop_streams_failed");
            return Err(error);
        }

        loop {
            let (in_flight, _) = self.state.callback_drain_state();
            if in_flight == 0 {
                break;
            }
            let now_qpc_ns = match super::windows_loopback::qpc_now_ns() {
                Ok(value) => value,
                Err(error) => {
                    self.state
                        .abort_device_cutover("qpc_read_failed_during_drain");
                    return Err(error);
                }
            };
            if now_qpc_ns >= deadline_qpc_ns {
                self.state.abort_device_cutover("callback_drain_timeout");
                return Err(anyhow::anyhow!(
                    "Audio callback drain reached the 2 second QPC deadline with {} callbacks still active",
                    in_flight
                ));
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }

        let drain_completed_qpc_ns = match super::windows_loopback::qpc_now_ns() {
            Ok(value) => value,
            Err(error) => {
                self.state
                    .abort_device_cutover("qpc_read_failed_after_drain");
                return Err(error);
            }
        };
        if drain_completed_qpc_ns >= deadline_qpc_ns {
            self.state
                .abort_device_cutover("callback_drain_completed_after_deadline");
            return Err(anyhow::anyhow!(
                "Audio callback drain completed after the 2 second QPC deadline"
            ));
        }
        let epoch = self.state.advance_device_epoch();
        info!(
            "Audio device cutover drained; awaiting new route: epoch={} watermark_qpc_ns={} drain_deadline_qpc_ns={}",
            epoch, watermark_qpc_ns, deadline_qpc_ns
        );
        Ok(epoch)
    }

    #[cfg(not(target_os = "windows"))]
    async fn prepare_stream_rebind(&mut self) -> Result<u64> {
        self.stream_manager.stop_streams_with_timeout().await?;
        Ok(self.state.advance_device_epoch())
    }

    /// Create a new recording manager
    pub fn new(recordings_folder: PathBuf) -> Self {
        let state = RecordingState::new();
        let stream_manager = AudioStreamManager::new(state.clone());
        let pipeline_manager = AudioPipelineManager::new();
        let (device_monitor, device_event_receiver) = AudioDeviceMonitor::new();

        Self {
            state,
            stream_manager,
            pipeline_manager,
            recording_saver: RecordingSaver::new(recordings_folder),
            device_monitor: Some(device_monitor),
            device_event_receiver: Some(device_event_receiver),
            route_evidence_persist_error: None,
        }
    }

    pub fn set_summary_template(
        &mut self,
        preference: Option<RecordingSummaryTemplatePreference>,
    ) -> Result<()> {
        self.recording_saver.set_summary_template(preference)
    }

    pub fn set_meeting_context(&mut self, context: Option<MeetingContextContainer>) -> Result<()> {
        self.recording_saver.set_meeting_context(context)
    }

    pub fn set_preserve_transcription_gaps(&mut self, enabled: bool) {
        self.pipeline_manager.set_preserve_transcription_gaps(enabled);
    }

    pub fn set_realtime_input_contract(
        &mut self,
        contract: RealtimeAudioInputContract,
    ) -> Result<()> {
        self.recording_saver.set_realtime_input_contract(contract)
    }

    pub fn measurement_recorder(&self) -> Option<Arc<D11MeasurementRecorder>> {
        self.recording_saver.measurement_recorder()
    }

    // Remove app handle storage for now - will be passed directly when saving

    /// Start recording with specified devices
    ///
    /// # Arguments
    /// * `microphone_device` - Optional microphone device to use
    /// * `system_device` - Optional system audio device to use
    /// * `auto_save` - Whether to save audio checkpoints (true) or just transcripts/metadata (false)
    pub async fn start_recording(
        &mut self,
        microphone_device: Option<Arc<AudioDevice>>,
        system_device: Option<Arc<AudioDevice>>,
        auto_save: bool,
    ) -> Result<mpsc::UnboundedReceiver<AudioChunk>> {
        info!("Starting recording manager (auto_save: {})", auto_save);
        self.route_evidence_persist_error = None;
        let microphone_enabled = microphone_device.is_some();
        let system_enabled = system_device.is_some();

        // Set up transcription channel
        let (transcription_sender, transcription_receiver) =
            mpsc::unbounded_channel::<AudioChunk>();

        // Persist device information in the first metadata transaction. A write
        // failure must abort before recording state or audio streams start.
        self.recording_saver.set_audio_devices(
            microphone_device.as_deref(),
            system_device.as_deref(),
            self.state.get_device_epoch(),
        )?;

        // CRITICAL FIX: Create recording sender for pre-mixed audio from pipeline
        // Pipeline will mix mic + system audio professionally and send to this channel
        // Pass auto_save to control whether audio checkpoints are created
        let recording_sender = self.recording_saver.start_accumulation(auto_save)?;

        // Start recording state only after the initial metadata transaction succeeds.
        self.state.start_recording()?;

        // Get device information for adaptive mixing
        // The pipeline uses device kind (Bluetooth vs Wired) to apply adaptive buffering:
        // - Bluetooth: Larger buffers (80-200ms) to handle jitter
        // - Wired: Smaller buffers (20-50ms) for low latency
        let (mic_name, mic_kind) = if let Some(ref mic) = microphone_device {
            let device_kind =
                super::device_detection::InputDeviceKind::detect(&mic.name, 512, 48000);
            (mic.name.clone(), device_kind)
        } else {
            (
                "No Microphone".to_string(),
                super::device_detection::InputDeviceKind::Unknown,
            )
        };

        let (sys_name, sys_kind) = if let Some(ref sys) = system_device {
            let device_kind =
                super::device_detection::InputDeviceKind::detect(&sys.name, 512, 48000);
            (sys.name.clone(), device_kind)
        } else {
            (
                "No System Audio".to_string(),
                super::device_detection::InputDeviceKind::Unknown,
            )
        };

        // Start the audio processing pipeline with FFmpeg adaptive mixer
        // Pipeline will: 1) Mix mic+system audio with adaptive buffering, 2) Send mixed to recording_sender,
        // 3) Apply VAD and send speech segments to transcription
        self.pipeline_manager.start(
            self.state.clone(),
            transcription_sender,
            0,     // Ignored - using dynamic sizing internally
            48000, // 48kHz sample rate
            microphone_enabled,
            system_enabled,
            Some(recording_sender), // CRITICAL: Pass recording sender to receive pre-mixed audio
            mic_name,
            mic_kind,
            sys_name,
            sys_kind,
            self.recording_saver.measurement_recorder(),
        )?;

        // Start audio streams - they send RAW unmixed chunks to pipeline for mixing
        // Pipeline handles mixing and distribution to both recording and transcription
        if let Err(error) = self
            .stream_manager
            .start_streams(microphone_device.clone(), system_device.clone(), None)
            .await
        {
            self.state.advance_device_epoch();
            self.cleanup_without_save().await;
            return Err(error);
        }

        // Start device monitoring to detect disconnects
        if let Some(ref mut monitor) = self.device_monitor {
            if let Err(e) = monitor.start_monitoring(microphone_device, system_device) {
                error!("Failed to start required device monitoring: {}", e);
                monitor.stop_monitoring().await;
                self.state.advance_device_epoch();
                self.cleanup_without_save().await;
                return Err(anyhow::anyhow!(
                    "Required audio device monitoring failed to start: {}",
                    e
                ));
            } else {
                info!("✅ Device monitoring started");
            }
        }

        self.state.mark_routes_ready();

        info!(
            "Recording manager started successfully with {} active streams",
            self.stream_manager.active_stream_count()
        );

        Ok(transcription_receiver)
    }

    /// Start the real-time pipeline from a controlled, file-backed PCM source.
    /// This path is only reachable through the strict isolated D-11 command and
    /// deliberately starts no microphone, loopback, or device-monitor task.
    pub async fn start_controlled_realtime_input(
        &mut self,
    ) -> Result<mpsc::UnboundedReceiver<AudioChunk>> {
        self.route_evidence_persist_error = None;
        let (transcription_sender, transcription_receiver) =
            mpsc::unbounded_channel::<AudioChunk>();
        self.recording_saver
            .set_audio_devices(None, None, self.state.get_device_epoch())?;
        let recording_sender = self.recording_saver.start_accumulation(true)?;
        self.state.start_recording()?;
        self.pipeline_manager.start(
            self.state.clone(),
            transcription_sender,
            0,
            super::measurement::CONTROLLED_SAMPLE_RATE,
            true,
            false,
            Some(recording_sender),
            "D-11 controlled input".to_owned(),
            super::device_detection::InputDeviceKind::Unknown,
            "No system audio".to_owned(),
            super::device_detection::InputDeviceKind::Unknown,
            self.recording_saver.measurement_recorder(),
        )?;
        self.state.mark_routes_ready();
        Ok(transcription_receiver)
    }

    /// Start recording with default devices and auto_save setting
    ///
    /// # Arguments
    /// * `auto_save` - Whether to save audio checkpoints (true) or just transcripts/metadata (false)
    ///
    /// # Platform-Specific Behavior
    ///
    /// **macOS**: Uses smart device selection that automatically overrides
    /// Bluetooth devices to built-in wired devices for stable, consistent sample rates.
    /// This prevents Core Audio/ScreenCaptureKit from delivering variable sample rate
    /// streams that cause sync issues when mixing mic + system audio.
    ///
    /// **Windows/Linux**: Uses system default devices directly without override.
    ///
    /// # macOS Bluetooth Override Strategy
    ///
    /// - Microphone: If Bluetooth → Use built-in MacBook mic
    /// - Speaker: If Bluetooth → Use built-in MacBook speaker (for ScreenCaptureKit)
    /// - Each device is checked INDEPENDENTLY
    ///
    /// Rationale: Bluetooth devices on macOS can have variable sample rates as Core Audio
    /// and the Bluetooth stack may resample dynamically. Built-in devices provide
    /// fixed, consistent sample rates for reliable audio mixing.
    ///
    /// User still hears audio via Bluetooth (playback), but recording captures
    /// via stable wired path for best quality.
    pub async fn start_recording_with_defaults_and_auto_save(
        &mut self,
        auto_save: bool,
    ) -> Result<mpsc::UnboundedReceiver<AudioChunk>> {
        #[cfg(target_os = "macos")]
        {
            info!("🎙️ [macOS] Starting recording with smart device selection (Bluetooth override enabled)");

            // Get safe recording devices with automatic Bluetooth fallback
            // This function handles all the detection and override logic for macOS
            let (microphone_device, system_device) = get_safe_recording_devices_macos()?;

            // Wrap in Arc for sharing across threads
            let microphone_device = microphone_device.map(Arc::new);
            let system_device = system_device.map(Arc::new);

            // Ensure at least microphone is available
            if microphone_device.is_none() {
                return Err(anyhow::anyhow!(
                    "❌ No microphone device available for recording"
                ));
            }

            // Start recording with selected devices and auto_save setting
            self.start_recording(microphone_device, system_device, auto_save)
                .await
        }

        #[cfg(not(target_os = "macos"))]
        {
            info!("Starting recording with default devices");

            // Get default devices (no Bluetooth override on Windows/Linux)
            let microphone_device = match default_input_device() {
                Ok(device) => {
                    info!("Using default microphone: {}", device.name);
                    Some(Arc::new(device))
                }
                Err(e) => {
                    warn!("No default microphone available: {}", e);
                    None
                }
            };

            let system_device = match default_output_device() {
                Ok(device) => {
                    info!("Using default system audio: {}", device.name);
                    Some(Arc::new(device))
                }
                Err(e) => {
                    warn!("No default system audio available: {}", e);
                    None
                }
            };

            // Ensure at least microphone is available
            if microphone_device.is_none() {
                return Err(anyhow::anyhow!("No microphone device available"));
            }

            self.start_recording(microphone_device, system_device, auto_save)
                .await
        }
    }

    /// Stop recording streams without saving (for use when waiting for transcription)
    pub async fn stop_streams_only(&mut self) -> Result<()> {
        info!("Stopping recording streams only");

        self.route_evidence_persist_error = self
            .persist_audio_route_evidence_inner(true)
            .err()
            .map(|error| format!("{error:#}"));

        // Stop device monitoring
        if let Some(ref mut monitor) = self.device_monitor {
            monitor.stop_monitoring().await;
        }

        // Stop recording state first
        self.state.stop_recording();

        // Stop audio streams
        if let Err(e) = self.stream_manager.stop_streams_with_timeout().await {
            error!("Error stopping audio streams: {}", e);
        }

        // Stop audio pipeline
        if let Err(e) = self.pipeline_manager.stop().await {
            error!("Error stopping audio pipeline: {}", e);
        }

        debug!("Recording streams stopped successfully");
        Ok(())
    }

    /// Stop streams and force immediate pipeline flush to process all accumulated audio
    pub async fn stop_streams_and_force_flush(&mut self) -> Result<()> {
        info!("🚀 Stopping recording streams with IMMEDIATE pipeline flush");

        // Capture the last epoch before RecordingState releases its endpoint
        // references. A persistence failure is returned after the recording
        // itself has still been finalized and saved.
        self.route_evidence_persist_error = self
            .persist_audio_route_evidence_inner(true)
            .err()
            .map(|error| format!("{error:#}"));

        // CRITICAL: Stop device monitor FIRST to prevent continuous WASAPI polling on Windows
        // This fixes the slow shutdown issue where device enumeration runs for 90+ seconds
        if let Some(ref mut monitor) = self.device_monitor {
            info!("Stopping device monitor first...");
            monitor.stop_monitoring().await;
        }

        // Stop recording state first - this clears device references
        self.state.stop_recording();

        // Stop audio streams immediately
        if let Err(e) = self.stream_manager.stop_streams_with_timeout().await {
            error!("Error stopping audio streams: {}", e);
        }

        // CRITICAL: Force pipeline to flush ALL accumulated audio before stopping
        debug!("💨 Forcing pipeline to flush accumulated audio immediately");
        if let Err(e) = self.pipeline_manager.force_flush_and_stop().await {
            error!("Error during force flush: {}", e);
        }

        // CRITICAL: Full cleanup to release all Arc references and resources
        // This ensures microphone is released even if Drop is delayed
        self.state.cleanup();

        info!("✅ Recording streams stopped with immediate flush completed");
        Ok(())
    }

    /// Save recording after transcription is complete
    pub async fn save_recording_only<R: tauri::Runtime>(
        &mut self,
        app: &tauri::AppHandle<R>,
        duration_before_stop: Option<f64>,
    ) -> Result<()> {
        debug!("Saving recording with transcript chunks");

        // The shutdown path clears RecordingState before queued transcripts and
        // audio checkpoints are finalized. Prefer the duration captured just
        // before that cleanup; the state lookup remains a legacy fallback.
        let recording_duration =
            duration_before_stop.or_else(|| self.state.get_active_recording_duration());
        info!("Recording duration from state: {:?}s", recording_duration);

        // Save the recording with actual duration
        match self
            .recording_saver
            .stop_and_save(app, recording_duration)
            .await
            .map_err(anyhow::Error::msg)?
        {
            Some(file_path) => {
                info!("Recording saved successfully to: {}", file_path);
            }
            None => {
                debug!("Recording not saved (auto-save disabled or no audio data)");
            }
        }

        if let Some(error) = self.route_evidence_persist_error.take() {
            return Err(anyhow::anyhow!(
                "recording was saved, but the final audio route evidence snapshot was not persisted: {error}"
            ));
        }

        debug!("Recording save operation completed");
        Ok(())
    }

    /// Stop recording and save audio (legacy method)
    pub async fn stop_recording<R: tauri::Runtime>(
        &mut self,
        app: &tauri::AppHandle<R>,
    ) -> Result<()> {
        info!("Stopping recording manager");

        // Get recording duration BEFORE stopping (important!)
        let recording_duration = self.state.get_active_recording_duration();
        info!("Recording duration before stop: {:?}s", recording_duration);

        // Freeze the final route epoch while the bound endpoint identities are
        // still present. stop_recording() intentionally releases those Arcs.
        self.route_evidence_persist_error = self
            .persist_audio_route_evidence_inner(true)
            .err()
            .map(|error| format!("{error:#}"));

        // Stop recording state first
        self.state.stop_recording();

        // Stop audio streams
        if let Err(e) = self.stream_manager.stop_streams_with_timeout().await {
            error!("Error stopping audio streams: {}", e);
        }

        // Stop audio pipeline
        if let Err(e) = self.pipeline_manager.stop().await {
            error!("Error stopping audio pipeline: {}", e);
        }

        // Save the recording with actual duration
        match self
            .recording_saver
            .stop_and_save(app, recording_duration)
            .await
            .map_err(anyhow::Error::msg)?
        {
            Some(file_path) => {
                info!("Recording saved successfully to: {}", file_path);
            }
            None => {
                info!("Recording not saved (auto-save disabled or no audio data)");
            }
        }

        if let Some(error) = self.route_evidence_persist_error.take() {
            return Err(anyhow::anyhow!(
                "recording was saved, but the final audio route evidence snapshot was not persisted: {error}"
            ));
        }

        info!("Recording manager stopped");
        Ok(())
    }

    /// Get recording stats from the saver
    pub fn get_recording_stats(&self) -> (usize, u32) {
        self.recording_saver.get_stats()
    }

    /// Check if currently recording
    pub fn is_recording(&self) -> bool {
        self.state.is_recording()
    }

    /// Pause the current recording session
    pub fn pause_recording(&self) -> Result<()> {
        info!("Pausing recording");
        self.state.pause_recording()
    }

    /// Resume the current recording session
    pub async fn resume_recording(&mut self) -> Result<()> {
        info!("Resuming recording");
        if !self.state.is_recording() {
            return Err(anyhow::anyhow!("Cannot resume when not recording"));
        }
        if !self.state.is_paused() {
            return Err(anyhow::anyhow!("Recording is not paused"));
        }
        #[cfg(target_os = "windows")]
        {
            if self.state.is_reconnecting() {
                let (disconnected, recording_device_type) = self
                    .state
                    .get_disconnected_device()
                    .ok_or_else(|| anyhow::anyhow!("Reconnecting route identity is missing"))?;
                let monitor_device_type = match recording_device_type {
                    RecordingDeviceType::Microphone => DeviceMonitorType::Microphone,
                    RecordingDeviceType::System => DeviceMonitorType::SystemAudio,
                };
                if !self
                    .attempt_device_reconnect(&disconnected.name, monitor_device_type)
                    .await?
                {
                    return Err(anyhow::anyhow!(
                        "Audio route is unavailable; recording cannot resume"
                    ));
                }
                self.state.stop_reconnecting();
            }
            let available_devices = list_audio_devices().await?;
            if let Some(microphone) = self.state.get_microphone_device() {
                let native_id = microphone.native_id.as_deref().ok_or_else(|| {
                    anyhow::anyhow!("Active Windows microphone has no native endpoint ID")
                })?;
                if !available_devices
                    .iter()
                    .any(|device| device.native_id.as_deref() == Some(native_id))
                {
                    self.handle_device_disconnect(
                        microphone.name.clone(),
                        DeviceMonitorType::Microphone,
                    );
                    return Err(anyhow::anyhow!(
                        "Microphone endpoint is unavailable; recording cannot resume: {}",
                        native_id
                    ));
                }
            }

            if let Some(system) = self.state.get_system_device() {
                let native_id = system.native_id.as_deref().ok_or_else(|| {
                    anyhow::anyhow!("Active Windows system route has no native endpoint ID")
                })?;
                if system.default_role.is_some() {
                    let default_id = super::devices::platform::default_windows_endpoint_id(
                        super::devices::DeviceType::Output,
                    )?;
                    if default_id != native_id {
                        self.handle_device_disconnect(
                            system.name.clone(),
                            DeviceMonitorType::SystemAudio,
                        );
                        if !self
                            .attempt_device_reconnect(&system.name, DeviceMonitorType::SystemAudio)
                            .await?
                        {
                            return Err(anyhow::anyhow!(
                                "New default system endpoint is unavailable; recording cannot resume"
                            ));
                        }
                        self.state.stop_reconnecting();
                    }
                } else if !available_devices
                    .iter()
                    .any(|device| device.native_id.as_deref() == Some(native_id))
                {
                    self.handle_device_disconnect(
                        system.name.clone(),
                        DeviceMonitorType::SystemAudio,
                    );
                    return Err(anyhow::anyhow!(
                        "System audio endpoint is unavailable; recording cannot resume: {}",
                        native_id
                    ));
                }
            }
        }
        self.state.resume_recording()
    }

    /// Check if recording is currently paused
    pub fn is_paused(&self) -> bool {
        self.state.is_paused()
    }

    /// Check if recording is active (recording and not paused)
    pub fn is_active(&self) -> bool {
        self.state.is_active()
    }

    /// Get recording statistics
    pub fn get_stats(&self) -> super::recording_state::RecordingStats {
        self.state.get_stats()
    }

    /// Get recording duration
    pub fn get_recording_duration(&self) -> Option<f64> {
        self.state.get_recording_duration()
    }

    /// Get active recording duration (excluding pauses)
    pub fn get_active_recording_duration(&self) -> Option<f64> {
        self.state.get_active_recording_duration()
    }

    /// Get total pause duration
    pub fn get_total_pause_duration(&self) -> f64 {
        self.state.get_total_pause_duration()
    }

    /// Get current pause duration if paused
    pub fn get_current_pause_duration(&self) -> Option<f64> {
        self.state.get_current_pause_duration()
    }

    /// Get error information
    pub fn get_error_info(&self) -> (u32, Option<super::recording_state::AudioError>) {
        (self.state.get_error_count(), self.state.get_last_error())
    }

    /// Get active stream count
    pub fn active_stream_count(&self) -> usize {
        self.stream_manager.active_stream_count()
    }

    /// Set error callback for handling errors
    pub fn set_error_callback<F>(&self, callback: F)
    where
        F: Fn(&super::recording_state::AudioError) + Send + Sync + 'static,
    {
        self.state.set_error_callback(callback);
    }

    /// Check if there's a fatal error
    pub fn has_fatal_error(&self) -> bool {
        self.state.has_fatal_error()
    }

    /// Set the meeting name for this recording session
    pub fn set_meeting_name(&mut self, name: Option<String>) {
        self.recording_saver.set_meeting_name(name);
    }

    /// Bind transcription workers to this meeting's persistence, not global state.
    pub fn transcript_writer(&self) -> super::recording_saver::TranscriptWriter {
        self.recording_saver.transcript_writer()
    }

    /// Add a structured transcript segment to be saved later
    pub fn add_transcript_segment(&self, segment: super::recording_saver::TranscriptSegment) {
        self.recording_saver.add_transcript_segment(segment);
    }

    pub fn add_transcript_segment_with_result(
        &self,
        segment: super::recording_saver::TranscriptSegment,
    ) -> Result<()> {
        self.recording_saver
            .add_transcript_segment_with_result(segment)
    }

    /// Add a transcript chunk to be saved later (legacy method)
    pub fn add_transcript_chunk(&self, text: String) {
        self.recording_saver.add_transcript_chunk(text);
    }

    /// Get accumulated transcript segments from current recording session
    /// Used for syncing frontend state after page reload during active recording
    pub fn get_transcript_segments(&self) -> Vec<super::recording_saver::TranscriptSegment> {
        self.recording_saver.get_transcript_segments()
    }

    /// Get meeting name from current recording session
    /// Used for syncing frontend state after page reload during active recording
    pub fn get_meeting_name(&self) -> Option<String> {
        self.recording_saver.get_meeting_name()
    }

    /// Cleanup all resources without saving
    pub async fn cleanup_without_save(&mut self) {
        if self.is_recording() {
            debug!("Stopping recording without saving during cleanup");

            // Stop recording state first
            self.state.stop_recording();

            // Stop audio streams
            if let Err(e) = self.stream_manager.stop_streams_with_timeout().await {
                error!("Error stopping audio streams during cleanup: {}", e);
            }

            // Stop audio pipeline
            if let Err(e) = self.pipeline_manager.stop().await {
                error!("Error stopping audio pipeline during cleanup: {}", e);
            }
        }
        self.state.cleanup();
    }

    /// Get the meeting folder path (if available)
    /// Returns None if no meeting name was set or folder structure not initialized
    pub fn get_meeting_folder(&self) -> Option<std::path::PathBuf> {
        self.recording_saver.get_meeting_folder().map(|p| p.clone())
    }

    /// Check for device events (disconnects/reconnects)
    /// Returns Some(DeviceEvent) if an event occurred, None otherwise
    pub fn poll_device_events(&mut self) -> Option<DeviceEvent> {
        if let Some(ref mut receiver) = self.device_event_receiver {
            receiver.try_recv().ok()
        } else {
            None
        }
    }

    /// Attempt to reconnect a disconnected device
    /// Returns true if reconnection successful
    pub async fn attempt_device_reconnect(
        &mut self,
        device_name: &str,
        device_type: DeviceMonitorType,
    ) -> Result<bool> {
        info!(
            "🔄 Attempting to reconnect device: {} ({:?})",
            device_name, device_type
        );

        // List current devices
        let available_devices = list_audio_devices().await?;

        let previously_bound = match device_type {
            DeviceMonitorType::Microphone => self.state.get_microphone_device(),
            DeviceMonitorType::SystemAudio => self.state.get_system_device(),
        };
        // Keep the original endpoint identity across disconnect/reconnect. A
        // same-named endpoint is not a safe replacement.
        let device = previously_bound.as_ref().and_then(|bound| {
            if bound.default_role.is_some() && device_type == DeviceMonitorType::SystemAudio {
                return super::devices::default_output_device().ok();
            }
            available_devices
                .iter()
                .find(|candidate| {
                    #[cfg(target_os = "windows")]
                    {
                        bound.native_id.as_deref().is_some_and(|native_id| {
                            candidate.native_id.as_deref() == Some(native_id)
                        })
                    }
                    #[cfg(not(target_os = "windows"))]
                    {
                        if let Some(native_id) = bound.native_id.as_deref() {
                            candidate.native_id.as_deref() == Some(native_id)
                        } else {
                            candidate.name == device_name
                        }
                    }
                })
                .cloned()
        });

        if let Some(device) = device {
            info!("✅ Device '{}' found, recreating stream...", device_name);

            // Determine which device to reconnect based on type
            let device_arc: Arc<AudioDevice> = Arc::new(device);
            match device_type {
                DeviceMonitorType::Microphone => {
                    // Stop existing mic stream and start new one
                    // We need to keep system audio running if it exists
                    let system_device = self.state.get_system_device();

                    // Restart streams with new microphone
                    let epoch = self.prepare_stream_rebind().await?;
                    self.state.clear_microphone_device();
                    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

                    if let Err(error) = self
                        .stream_manager
                        .start_streams(Some(device_arc.clone()), system_device, None)
                        .await
                    {
                        self.state
                            .fail_device_cutover(epoch, "new_microphone_stream_start_failed");
                        let _ = self.persist_audio_route_evidence();
                        return Err(error);
                    }
                    self.state.set_microphone_device(device_arc);
                    self.state.finish_device_cutover(epoch);
                    if let Err(error) = self.persist_device_binding(epoch) {
                        self.state
                            .fail_device_cutover(epoch, "route_metadata_persist_failed");
                        return Err(error);
                    }

                    info!("✅ Microphone reconnected successfully at device_epoch={epoch}");
                    Ok(true)
                }
                DeviceMonitorType::SystemAudio => {
                    // Stop existing system audio stream and start new one
                    let microphone_device = self.state.get_microphone_device();

                    // Restart streams with new system audio
                    let epoch = self.prepare_stream_rebind().await?;
                    self.state.clear_system_device();
                    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

                    if let Err(error) = self
                        .stream_manager
                        .start_streams(microphone_device, Some(device_arc.clone()), None)
                        .await
                    {
                        self.state
                            .fail_device_cutover(epoch, "new_system_stream_start_failed");
                        let _ = self.persist_audio_route_evidence();
                        return Err(error);
                    }
                    self.state.set_system_device(device_arc);
                    self.state.finish_device_cutover(epoch);
                    if let Err(error) = self.persist_device_binding(epoch) {
                        self.state
                            .fail_device_cutover(epoch, "route_metadata_persist_failed");
                        return Err(error);
                    }

                    info!("✅ System audio reconnected successfully at device_epoch={epoch}");
                    Ok(true)
                }
            }
        } else {
            warn!("❌ Device '{}' not yet available", device_name);
            Ok(false)
        }
    }

    /// Handle a device disconnect event
    /// Pauses recording and attempts reconnection
    pub fn handle_device_disconnect(
        &mut self,
        device_name: String,
        device_type: DeviceMonitorType,
    ) {
        warn!(
            "📱 Device disconnected: {} ({:?})",
            device_name, device_type
        );

        // Mark state as reconnecting (keeps recording alive but in waiting state)
        let device = match device_type {
            DeviceMonitorType::Microphone => self.state.get_microphone_device(),
            DeviceMonitorType::SystemAudio => self.state.get_system_device(),
        };

        if let Some(device) = device {
            let recording_device_type = match device_type {
                DeviceMonitorType::Microphone => RecordingDeviceType::Microphone,
                DeviceMonitorType::SystemAudio => RecordingDeviceType::System,
            };
            self.state.start_reconnecting(device, recording_device_type);
            self.state
                .report_error(super::recording_state::AudioError::DeviceDisconnected);
        }
    }

    /// Handle a device reconnect event
    pub async fn handle_device_reconnect(
        &mut self,
        device_name: String,
        device_type: DeviceMonitorType,
    ) -> Result<()> {
        info!("📱 Device reconnected: {} ({:?})", device_name, device_type);

        // Attempt to reconnect the device
        match self
            .attempt_device_reconnect(&device_name, device_type)
            .await
        {
            Ok(true) => {
                info!("✅ Successfully reconnected device: {}", device_name);
                self.state.stop_reconnecting();
                Ok(())
            }
            Ok(false) => {
                warn!("Device reconnect attempt failed (device not yet available)");
                Err(anyhow::anyhow!("Device not available"))
            }
            Err(e) => {
                error!("Device reconnect failed: {}", e);
                Err(e)
            }
        }
    }

    /// Check if currently attempting to reconnect
    pub fn is_reconnecting(&self) -> bool {
        self.state.is_reconnecting()
    }

    /// Get reference to recording state for external access
    pub fn get_state(&self) -> &Arc<RecordingState> {
        &self.state
    }
}

impl Drop for RecordingManager {
    fn drop(&mut self) {
        // Note: Can't call async cleanup in Drop, but streams have their own Drop implementations
        self.state.cleanup();
    }
}

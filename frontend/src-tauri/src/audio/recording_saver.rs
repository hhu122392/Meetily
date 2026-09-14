use crate::meeting_context::{
    normalize_and_validate_container, MeetingContextContainer, RecordingSummaryTemplatePreference,
};
use anyhow::Result;
use log::{error, info, warn};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Emitter, Runtime};
use tokio::sync::Mutex as AsyncMutex;
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;

use super::audio_processing::create_meeting_folder;
use super::devices::AudioDevice;
use super::incremental_saver::IncrementalAudioSaver;
use super::measurement::{
    D11MeasurementRecorder, RealtimeAudioInputContract, RealtimeMeasurementReference,
};
use super::recording_state::{AudioChunk, AudioRouteEvidenceSnapshot};

/// Structured transcript segment for JSON export
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TranscriptSegment {
    pub id: String,
    pub text: String,
    pub audio_start_time: f64, // Seconds from recording start
    pub audio_end_time: f64,   // Seconds from recording start
    pub duration: f64,         // Segment duration in seconds
    pub display_time: String,  // Formatted time for display like "[02:15]"
    pub confidence: f32,
    pub sequence_id: u64,
    #[serde(default)]
    pub revision: u64,
    #[serde(default)]
    pub is_partial: bool,
}

/// Final database saves must use the completed file, not a delayed UI event.
pub fn read_completed_transcript_snapshot(folder: &std::path::Path) -> Result<Vec<TranscriptSegment>> {
    let metadata: serde_json::Value = serde_json::from_slice(&std::fs::read(folder.join("metadata.json"))?)?;
    if metadata["status"] != "completed" {
        return Err(anyhow::anyhow!("Recording has not completed successfully"));
    }
    #[derive(Deserialize)]
    struct Snapshot { segments: Vec<TranscriptSegment> }
    let snapshot: Snapshot = serde_json::from_slice(&std::fs::read(folder.join("transcripts.json"))?)?;
    let mut ids = std::collections::HashSet::new();
    if snapshot.segments.iter().any(|row| row.is_partial || !ids.insert(row.sequence_id)) {
        return Err(anyhow::anyhow!("Transcript snapshot contains unfinished or duplicate rows"));
    }
    Ok(snapshot.segments)
}

/// A worker keeps this meeting's text alive even when shutdown moves the saver.
#[derive(Clone)]
pub struct TranscriptWriter {
    folder: Option<PathBuf>,
    segments: Arc<Mutex<Vec<TranscriptSegment>>>,
    error: Arc<Mutex<Option<String>>>,
}

impl TranscriptWriter {
    pub fn write(&self, segment: TranscriptSegment) -> Result<()> {
        self.write_batch(vec![segment])
    }

    /// A tail revision and its following row belong to one snapshot.
    pub fn write_batch(&self, updates: Vec<TranscriptSegment>) -> Result<()> {
        let result = (|| {
            // ponytail: serialize this meeting's upsert and atomic replacement
            // under the existing segment lock; no second persistence queue.
            let mut segments = self
                .segments
                .lock()
                .map_err(|_| anyhow::anyhow!("Failed to lock transcript segments"))?;
            for segment in updates {
                if let Some(existing) = segments
                    .iter_mut()
                    .find(|existing| existing.sequence_id == segment.sequence_id)
                {
                    if segment.revision > existing.revision
                        || (segment.revision == 0 && existing.revision == 0)
                    {
                        *existing = segment;
                    }
                } else {
                    segments.push(segment);
                }
            }
            let folder = self
                .folder
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("Transcript meeting folder is unavailable"))?;
            RecordingSaver::write_transcript_snapshot(folder, &segments)
        })();
        if let Err(error) = &result {
            if let Ok(mut first_error) = self.error.lock() {
                first_error.get_or_insert_with(|| format!("Transcript write failed: {error:#}"));
            }
        }
        result
    }

    fn first_error(&self) -> Option<String> {
        match self.error.lock() {
            Ok(error) => error.clone(),
            Err(_) => Some("Transcript error state is unavailable".to_owned()),
        }
    }

    pub fn report_transcription_error(&self, message: &str) {
        if let Ok(mut error) = self.error.lock() {
            error.get_or_insert_with(|| format!("Transcription failed: {message}"));
        }
    }
}

/// Meeting metadata structure
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MeetingMetadata {
    pub version: String,
    pub meeting_id: Option<String>,
    pub meeting_name: Option<String>,
    pub created_at: String,
    pub completed_at: Option<String>,
    pub duration_seconds: Option<f64>,
    pub devices: DeviceInfo,
    pub audio_file: String,
    pub transcript_file: String,
    pub sample_rate: u32,
    pub status: String, // "recording", "completed", "error"
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audio_route_evidence: Option<AudioRouteEvidenceSnapshot>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary_template: Option<RecordingSummaryTemplatePreference>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub meeting_context: Option<MeetingContextContainer>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub realtime_measurement: Option<RealtimeMeasurementReference>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DeviceInfo {
    pub microphone: Option<String>,
    pub system_audio: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub microphone_native_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system_audio_native_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub microphone_default_role: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system_audio_default_role: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub microphone_device_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system_audio_device_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recording_mode: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recording_start_device_epoch: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_device_epoch: Option<u64>,
}

/// New recording saver using incremental saving strategy
pub struct RecordingSaver {
    recordings_folder: PathBuf,
    incremental_saver: Option<Arc<AsyncMutex<IncrementalAudioSaver>>>,
    meeting_folder: Option<PathBuf>,
    meeting_name: Option<String>,
    metadata: Option<MeetingMetadata>,
    pending_device_info: DeviceInfo,
    pending_summary_template: Option<RecordingSummaryTemplatePreference>,
    pending_meeting_context: Option<MeetingContextContainer>,
    pending_realtime_input_contract: Option<RealtimeAudioInputContract>,
    recording_sample_rate: u32,
    measurement_recorder: Option<Arc<D11MeasurementRecorder>>,
    transcript_segments: Arc<Mutex<Vec<TranscriptSegment>>>,
    transcript_error: Arc<Mutex<Option<String>>>,
    stop_tx: Option<oneshot::Sender<()>>,
    save_task: Option<JoinHandle<Result<(), String>>>,
    save_error: Option<String>,
}

impl RecordingSaver {
    pub fn new(recordings_folder: PathBuf) -> Self {
        Self {
            recordings_folder,
            incremental_saver: None,
            meeting_folder: None,
            meeting_name: None,
            metadata: None,
            pending_device_info: DeviceInfo::default(),
            pending_summary_template: None,
            pending_meeting_context: None,
            pending_realtime_input_contract: None,
            recording_sample_rate: 48_000,
            measurement_recorder: None,
            transcript_segments: Arc::new(Mutex::new(Vec::new())),
            transcript_error: Arc::new(Mutex::new(None)),
            stop_tx: None,
            save_task: None,
            save_error: None,
        }
    }

    /// Set the meeting name for this recording session
    pub fn set_meeting_name(&mut self, name: Option<String>) {
        self.meeting_name = name;
    }

    /// Configure the summary template selected for this recording before the
    /// meeting directory is initialized. Once recording persistence starts the
    /// value is immutable; later corrections must use the meeting-level
    /// preference API rather than silently changing the recording snapshot.
    pub fn set_summary_template(
        &mut self,
        preference: Option<RecordingSummaryTemplatePreference>,
    ) -> Result<()> {
        if self.meeting_folder.is_some() {
            return Err(anyhow::anyhow!(
                "summary template cannot be replaced after recording persistence starts"
            ));
        }
        self.pending_summary_template = preference;
        Ok(())
    }

    /// Configure the append-only meeting context container before the meeting
    /// directory is initialized. The original recording context is written in
    /// the first metadata transaction and survives every later metadata write.
    pub fn set_meeting_context(&mut self, context: Option<MeetingContextContainer>) -> Result<()> {
        if self.meeting_folder.is_some() {
            return Err(anyhow::anyhow!(
                "meeting context cannot be replaced after recording persistence starts"
            ));
        }
        self.pending_meeting_context = context
            .map(normalize_and_validate_container)
            .transpose()
            .map_err(|issues| {
                anyhow::anyhow!(
                    "invalid meeting context: {}",
                    serde_json::to_string(&issues)
                        .unwrap_or_else(|_| "validation details unavailable".to_owned())
                )
            })?;
        Ok(())
    }

    pub fn set_realtime_input_contract(
        &mut self,
        contract: RealtimeAudioInputContract,
    ) -> Result<()> {
        if self.meeting_folder.is_some() {
            return Err(anyhow::anyhow!(
                "real-time input contract cannot be replaced after recording persistence starts"
            ));
        }
        self.recording_sample_rate = contract.canonical_sample_rate_hz;
        self.pending_realtime_input_contract = Some(contract);
        Ok(())
    }

    pub fn measurement_recorder(&self) -> Option<Arc<D11MeasurementRecorder>> {
        self.measurement_recorder.clone()
    }

    /// Set device information in metadata
    pub fn set_device_info(
        &mut self,
        mic_name: Option<String>,
        sys_name: Option<String>,
    ) -> Result<()> {
        let devices = DeviceInfo {
            microphone: mic_name.clone(),
            system_audio: sys_name.clone(),
            ..self.pending_device_info.clone()
        };
        self.apply_device_info(devices)
    }

    pub fn set_audio_devices(
        &mut self,
        microphone: Option<&AudioDevice>,
        system_audio: Option<&AudioDevice>,
        device_epoch: u64,
    ) -> Result<()> {
        let recording_mode = match (microphone.is_some(), system_audio.is_some()) {
            (true, true) => "microphone_and_system",
            (true, false) => "microphone_only",
            (false, true) => "system_only",
            (false, false) => "inactive",
        };
        let devices = DeviceInfo {
            microphone: microphone.map(|device| device.name.clone()),
            system_audio: system_audio.map(|device| device.name.clone()),
            microphone_native_id: microphone.and_then(|device| device.native_id.clone()),
            system_audio_native_id: system_audio.and_then(|device| device.native_id.clone()),
            microphone_default_role: microphone.and_then(|device| device.default_role.clone()),
            system_audio_default_role: system_audio.and_then(|device| device.default_role.clone()),
            microphone_device_type: microphone.map(|_| "input".to_string()),
            system_audio_device_type: system_audio.map(|_| "output".to_string()),
            recording_mode: Some(recording_mode.to_string()),
            recording_start_device_epoch: self
                .pending_device_info
                .recording_start_device_epoch
                .or(Some(device_epoch)),
            current_device_epoch: Some(device_epoch),
        };
        self.apply_device_info(devices)
    }

    fn apply_device_info(&mut self, devices: DeviceInfo) -> Result<()> {
        let updated_metadata = if let Some(metadata) = self.metadata.as_ref() {
            let mut updated = metadata.clone();
            updated.devices = devices.clone();
            Some(updated)
        } else {
            None
        };

        if let (Some(folder), Some(metadata)) = (&self.meeting_folder, updated_metadata.as_ref()) {
            self.write_metadata(folder, &metadata)?;
        }
        if let Some(metadata) = updated_metadata {
            self.metadata = Some(metadata);
        }
        self.pending_device_info = devices;

        Ok(())
    }

    pub fn set_audio_route_evidence(&mut self, evidence: AudioRouteEvidenceSnapshot) -> Result<()> {
        let updated_metadata = if let Some(metadata) = self.metadata.as_ref() {
            let mut updated = metadata.clone();
            updated.audio_route_evidence = Some(evidence);
            Some(updated)
        } else {
            None
        };
        if let (Some(folder), Some(metadata)) = (&self.meeting_folder, updated_metadata.as_ref()) {
            self.write_metadata(folder, metadata)?;
        }
        if let Some(metadata) = updated_metadata {
            self.metadata = Some(metadata);
        }
        Ok(())
    }

    /// Add or update a structured transcript segment (upserts based on sequence_id)
    /// Also saves incrementally to disk
    pub fn add_transcript_segment(&self, segment: TranscriptSegment) {
        if let Err(error) = self.add_transcript_segment_with_result(segment) {
            warn!("Failed to persist transcript segment: {error:#}");
        }
    }

    pub fn add_transcript_segment_with_result(&self, segment: TranscriptSegment) -> Result<()> {
        self.transcript_writer().write(segment)
    }

    pub fn transcript_writer(&self) -> TranscriptWriter {
        TranscriptWriter {
            folder: self.meeting_folder.clone(),
            segments: self.transcript_segments.clone(),
            error: self.transcript_error.clone(),
        }
    }

    /// Legacy method for backward compatibility - converts text to basic segment
    pub fn add_transcript_chunk(&self, text: String) {
        let segment = TranscriptSegment {
            id: format!("seg_{}", chrono::Utc::now().timestamp_millis()),
            text,
            audio_start_time: 0.0,
            audio_end_time: 0.0,
            duration: 0.0,
            display_time: "[00:00]".to_string(),
            confidence: 1.0,
            sequence_id: 0,
            revision: 0,
            is_partial: false,
        };
        self.add_transcript_segment(segment);
    }

    /// Start accumulation with optional incremental saving
    ///
    /// # Arguments
    /// * `auto_save` - If true, creates checkpoints and enables saving. If false, audio chunks are discarded.
    pub fn start_accumulation(
        &mut self,
        auto_save: bool,
    ) -> Result<mpsc::UnboundedSender<AudioChunk>> {
        // A saver owns one meeting, including any unfinished or failed save.
        if self.meeting_folder.is_some() {
            return Err(anyhow::anyhow!(
                "recording saver already belongs to a meeting; use a new saver for a new recording"
            ));
        }
        if auto_save {
            info!("Initializing incremental audio saver for recording (auto-save ENABLED)");
        } else {
            info!(
                "Starting recording without audio saving (auto-save DISABLED - transcripts only)"
            );
        }

        // Initialize meeting folder and incremental saver ONLY if auto_save is enabled
        let meeting_name = self
            .meeting_name
            .clone()
            .ok_or_else(|| anyhow::anyhow!("meeting name is required before recording starts"))?;
        if auto_save {
            self.initialize_meeting_folder(&meeting_name, true)?;
            info!("Successfully initialized meeting folder with checkpoints");
        } else {
            // When auto_save is false, still create meeting folder for transcripts/metadata
            // but skip .checkpoints directory
            self.initialize_meeting_folder(&meeting_name, false)?;
            info!("Successfully initialized meeting folder (transcripts only)");
        }

        // Create the channel only after persistence initialization succeeds, so
        // a failed start cannot leave a half-initialized receiver behind.
        let (sender, mut receiver) = mpsc::unbounded_channel::<AudioChunk>();
        let (stop_tx, mut stop_rx) = oneshot::channel();
        let incremental_saver_arc = self.incremental_saver.clone();
        self.stop_tx = Some(stop_tx);
        self.save_task = Some(tokio::spawn(async move {
            let mut accepting = true;
            let mut first_error = None;
            loop {
                let chunk = tokio::select! {
                    biased;
                    _ = &mut stop_rx, if accepting => {
                        receiver.close();
                        accepting = false;
                        continue;
                    }
                    chunk = receiver.recv() => match chunk {
                        Some(chunk) => chunk,
                        None => break,
                    },
                };
                // Closing admission does not discard the already accepted queue.
                if auto_save {
                    let result = match &incremental_saver_arc {
                        Some(saver) => saver
                            .lock()
                            .await
                            .add_chunk(chunk)
                            .map_err(|error| format!("Failed to write audio chunk: {error}")),
                        None => Err("Incremental saver unavailable while accumulating".to_owned()),
                    };
                    if let Err(error) = result {
                        error!("{error}");
                        first_error.get_or_insert(error);
                    }
                }
            }
            first_error.map_or(Ok(()), Err)
        }));

        Ok(sender)
    }

    /// Initialize meeting folder structure and metadata
    ///
    /// # Arguments
    /// * `meeting_name` - Name of the meeting
    /// * `create_checkpoints` - Whether to create .checkpoints/ directory and IncrementalAudioSaver
    fn initialize_meeting_folder(
        &mut self,
        meeting_name: &str,
        create_checkpoints: bool,
    ) -> Result<()> {
        // The root was validated and frozen before this saver was created.
        let base_folder = self.recordings_folder.clone();
        self.initialize_meeting_folder_at(&base_folder, meeting_name, create_checkpoints)
    }

    fn initialize_meeting_folder_at(
        &mut self,
        base_folder: &PathBuf,
        meeting_name: &str,
        create_checkpoints: bool,
    ) -> Result<()> {
        // Create meeting folder structure (with or without .checkpoints/ subdirectory)
        let meeting_folder = create_meeting_folder(base_folder, meeting_name, create_checkpoints)?;

        // Create initial metadata
        let metadata = MeetingMetadata {
            version: "1.0".to_string(),
            meeting_id: None, // Will be set by backend
            meeting_name: Some(meeting_name.to_string()),
            created_at: chrono::Utc::now().to_rfc3339(),
            completed_at: None,
            duration_seconds: None,
            devices: self.pending_device_info.clone(),
            audio_file: if create_checkpoints {
                "audio.mp4".to_string()
            } else {
                "".to_string()
            },
            transcript_file: "transcripts.json".to_string(),
            sample_rate: self.recording_sample_rate,
            status: "recording".to_string(),
            audio_route_evidence: None,
            summary_template: self.pending_summary_template.clone(),
            meeting_context: self.pending_meeting_context.clone(),
            realtime_measurement: self
                .pending_realtime_input_contract
                .as_ref()
                .map(RealtimeMeasurementReference::from),
        };

        // Complete initialization as one transaction. If any step fails, the
        // newly-created meeting directory is removed and no in-memory state is
        // committed.
        let initialized_saver = (|| -> Result<(
            Option<Arc<AsyncMutex<IncrementalAudioSaver>>>,
            Option<Arc<D11MeasurementRecorder>>,
        )> {
            let incremental_saver = if create_checkpoints {
                let saver = IncrementalAudioSaver::new(
                    meeting_folder.clone(),
                    self.recording_sample_rate,
                )?;
                Some(Arc::new(AsyncMutex::new(saver)))
            } else {
                None
            };
            self.write_metadata(&meeting_folder, &metadata)?;
            let measurement_recorder = self
                .pending_realtime_input_contract
                .clone()
                .map(|contract| {
                    let recording_id = meeting_folder
                        .file_name()
                        .and_then(|value| value.to_str())
                        .unwrap_or("unknown-recording")
                        .to_owned();
                    D11MeasurementRecorder::new(meeting_folder.clone(), recording_id, contract)
                })
                .transpose()?;
            Ok((incremental_saver, measurement_recorder))
        })();

        let (incremental_saver, measurement_recorder) = match initialized_saver {
            Ok(value) => value,
            Err(error) => {
                if let Err(cleanup_error) = std::fs::remove_dir_all(&meeting_folder) {
                    return Err(anyhow::anyhow!(
                        "recording folder initialization failed: {error}; cleanup failed for {}: {cleanup_error}",
                        meeting_folder.display()
                    ));
                }
                return Err(error);
            }
        };

        if create_checkpoints {
            info!(
                "✅ Incremental audio saver initialized for meeting: {}",
                meeting_name
            );
        } else {
            info!("⚠️  Skipped incremental audio saver (auto-save disabled)");
        }

        self.incremental_saver = incremental_saver;
        if let Some(recorder) = measurement_recorder.as_ref() {
            recorder.start_load_sampler();
            recorder.record_stop_stage("recording_started", "completed", None);
        }
        self.measurement_recorder = measurement_recorder;
        self.meeting_folder = Some(meeting_folder);
        self.metadata = Some(metadata);

        Ok(())
    }

    fn complete_metadata(&mut self, recording_duration: Option<f64>) -> Result<()> {
        let folder = self
            .meeting_folder
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("meeting folder is unavailable"))?;
        let mut metadata = self
            .metadata
            .clone()
            .ok_or_else(|| anyhow::anyhow!("meeting metadata is unavailable"))?;
        metadata.status = "completed".to_owned();
        metadata.completed_at = Some(chrono::Utc::now().to_rfc3339());
        metadata.duration_seconds = recording_duration.or_else(|| {
            self.transcript_segments
                .lock()
                .ok()
                .and_then(|segments| segments.last().map(|segment| segment.audio_end_time))
        });
        self.write_metadata(folder, &metadata)?;
        self.metadata = Some(metadata);
        Ok(())
    }

    fn record_save_failure(&mut self, error: String) -> String {
        let mut message = self.save_error.get_or_insert(error).clone();
        if let (Some(folder), Some(metadata)) = (&self.meeting_folder, &self.metadata) {
            let mut failed = metadata.clone();
            failed.status = "error".to_owned();
            failed.completed_at = None;
            match self.write_metadata(folder, &failed) {
                Ok(()) => self.metadata = Some(failed),
                Err(error) => {
                    message.push_str(&format!("; failed to persist error metadata: {error}"))
                }
            }
        }
        if let Some(recorder) = &self.measurement_recorder {
            recorder.record_stop_stage("metadata_completed", "failed", Some(&message));
            recorder.record_stop_stage("final_save_finished", "failed", Some(&message));
        }
        error!("Recording save failed: {message}");
        message
    }

    /// Write metadata.json to disk (atomic write with temp file)
    fn write_metadata(&self, folder: &PathBuf, metadata: &MeetingMetadata) -> Result<()> {
        let metadata_path = folder.join("metadata.json");
        let temp_path = folder.join(".metadata.json.tmp");

        let json_string = serde_json::to_string_pretty(metadata)?;
        std::fs::write(&temp_path, json_string)?;
        std::fs::rename(&temp_path, &metadata_path)?; // Atomic

        Ok(())
    }

    /// Write transcripts.json to disk (atomic write with temp file and validation)
    fn write_transcripts_json(&self, folder: &PathBuf) -> Result<()> {
        let segments = self
            .transcript_segments
            .lock()
            .map_err(|_| anyhow::anyhow!("Failed to lock transcript segments"))?;
        Self::write_transcript_snapshot(folder, &segments)
    }

    fn write_transcript_snapshot(
        folder: &PathBuf,
        segments_clone: &[TranscriptSegment],
    ) -> Result<()> {
        info!(
            "Writing {} transcript segments to JSON",
            segments_clone.len()
        );

        let transcript_path = folder.join("transcripts.json");
        let temp_path = folder.join(".transcripts.json.tmp");

        // Create JSON structure
        let json = serde_json::json!({
            "version": "1.0",
            "segments": segments_clone,
            "last_updated": chrono::Utc::now().to_rfc3339(),
            "total_segments": segments_clone.len()
        });

        // Serialize to pretty JSON string
        let json_string = serde_json::to_string_pretty(&json).map_err(|e| {
            error!("Failed to serialize transcripts to JSON: {}", e);
            anyhow::anyhow!("JSON serialization failed: {}", e)
        })?;

        // Write to temp file with error handling
        std::fs::write(&temp_path, &json_string).map_err(|e| {
            error!(
                "Failed to write transcript temp file to {}: {}",
                temp_path.display(),
                e
            );
            anyhow::anyhow!("Failed to write temp file: {}", e)
        })?;

        // Verify temp file was written correctly
        if !temp_path.exists() {
            error!(
                "Temp transcript file does not exist after write: {}",
                temp_path.display()
            );
            return Err(anyhow::anyhow!("Temp file verification failed"));
        }

        // Atomic rename
        std::fs::rename(&temp_path, &transcript_path).map_err(|e| {
            error!(
                "Failed to rename transcript file from {} to {}: {}",
                temp_path.display(),
                transcript_path.display(),
                e
            );
            anyhow::anyhow!("Failed to rename transcript file: {}", e)
        })?;

        info!(
            "✅ Successfully wrote transcripts.json with {} segments",
            segments_clone.len()
        );
        Ok(())
    }

    // in frontend/src-tauri/src/audio/recording_saver.rs
    pub fn get_stats(&self) -> (usize, u32) {
        if let Some(ref saver) = self.incremental_saver {
            if let Ok(guard) = saver.try_lock() {
                (
                    guard.get_checkpoint_count() as usize,
                    self.recording_sample_rate,
                )
            } else {
                (0, self.recording_sample_rate)
            }
        } else {
            (0, self.recording_sample_rate)
        }
    }

    /// Stop and save using incremental saving approach
    ///
    /// # Arguments
    /// * `app` - Tauri app handle for emitting events
    /// * `recording_duration` - Actual recording duration in seconds (from RecordingState)
    pub async fn stop_and_save<R: Runtime>(
        &mut self,
        app: &AppHandle<R>,
        recording_duration: Option<f64>,
    ) -> Result<Option<String>, String> {
        info!("Stopping recording saver");
        let measurement_recorder = self.measurement_recorder.clone();
        if let Some(recorder) = measurement_recorder.as_ref() {
            recorder.record_stop_stage("final_save_started", "started", None);
        }

        if let Some(stop_tx) = self.stop_tx.take() {
            let _ = stop_tx.send(());
        }
        if let Some(recorder) = measurement_recorder.as_ref() {
            recorder.record_stop_stage("tail_wait_started", "started", None);
        }
        // Await by reference: cancellation leaves the same task owned here.
        if let Some(task) = self.save_task.as_mut() {
            let result = task
                .await
                .unwrap_or_else(|error| Err(format!("Audio writer task failed: {error}")));
            self.save_task = None;
            if let Err(error) = result {
                self.save_error.get_or_insert(error);
            }
        }
        if let Some(recorder) = measurement_recorder.as_ref() {
            let status = if self.save_error.is_some() {
                "failed"
            } else {
                "completed"
            };
            recorder.record_stop_stage("accumulation_stopped", status, self.save_error.as_deref());
            recorder.record_stop_stage("tail_wait_finished", status, self.save_error.as_deref());
        }

        // Check if incremental saver exists (indicates auto_save was enabled)
        let should_save_audio = self.incremental_saver.is_some();

        // Salvage buffered audio even after a write error, without hiding it.
        let final_audio_path = if let Some(saver_arc) = &self.incremental_saver {
            if let Some(recorder) = measurement_recorder.as_ref() {
                recorder.record_stop_stage("checkpoint_merge_started", "started", None);
            }
            let mut saver = saver_arc.lock().await;
            match saver.finalize().await {
                Ok(path) => {
                    info!("✅ Successfully finalized audio: {}", path.display());
                    if let Some(recorder) = measurement_recorder.as_ref() {
                        recorder.record_stop_stage("checkpoint_merge_finished", "completed", None);
                    }
                    path
                }
                Err(e) => {
                    error!("❌ Failed to finalize incremental saver: {}", e);
                    if let Some(recorder) = measurement_recorder.as_ref() {
                        recorder.record_stop_stage(
                            "checkpoint_merge_finished",
                            "failed",
                            Some(&e.to_string()),
                        );
                    }
                    self.save_error
                        .get_or_insert_with(|| format!("Failed to finalize audio: {e}"));
                    PathBuf::new()
                }
            }
        } else {
            info!("⚠️  Auto-save disabled - skipping audio finalization");
            if let Some(recorder) = measurement_recorder.as_ref() {
                recorder.record_stop_stage("checkpoint_merge_finished", "not_applicable", None);
            }
            PathBuf::new()
        };

        // Preserve text even if audio finalization failed.
        if let Some(recorder) = measurement_recorder.as_ref() {
            recorder.record_stop_stage("final_transcript_write_started", "started", None);
        }
        let transcript_result = self
            .meeting_folder
            .as_ref()
            .ok_or_else(|| "Meeting folder is unavailable".to_owned())
            .and_then(|folder| {
                self.write_transcripts_json(folder)
                    .map_err(|error| format!("Failed to save transcripts: {error}"))
            });
        if let Some(recorder) = measurement_recorder.as_ref() {
            recorder.record_stop_stage(
                "final_transcript_write_finished",
                if transcript_result.is_ok() {
                    "completed"
                } else {
                    "failed"
                },
                transcript_result.as_ref().err().map(String::as_str),
            );
        }
        if let Err(error) = transcript_result {
            error!("{error}");
            self.save_error.get_or_insert(error);
        }

        if let Some(error) = self.transcript_writer().first_error() {
            self.save_error.get_or_insert(error);
        }

        if let Some(error) = self.save_error.clone() {
            return Err(self.record_save_failure(error));
        }
        if let Some(recorder) = measurement_recorder.as_ref() {
            recorder.record_stop_stage("metadata_completion_started", "started", None);
        }
        if let Err(error) = self.complete_metadata(recording_duration) {
            return Err(self.record_save_failure(format!("Failed to update metadata: {error}")));
        }
        info!("✅ Metadata updated to completed status");
        if let Some(recorder) = measurement_recorder.as_ref() {
            recorder.record_stop_stage("metadata_completed", "completed", None);
        }

        if !should_save_audio {
            info!("✅ Transcripts and metadata finalized without an audio file");
            if let Some(recorder) = measurement_recorder.as_ref() {
                recorder.record_stop_stage("final_save_finished", "completed", None);
            }
            return Ok(None);
        }

        // Emit save event with audio and transcript paths
        let save_event = serde_json::json!({
            "audio_file": final_audio_path.to_string_lossy(),
            "transcript_file": self.meeting_folder.as_ref()
                .map(|f| f.join("transcripts.json").to_string_lossy().to_string()),
            "meeting_name": self.meeting_name,
            "meeting_folder": self.meeting_folder.as_ref()
                .map(|f| f.to_string_lossy().to_string())
        });

        if let Err(e) = app.emit("recording-saved", &save_event) {
            warn!("Failed to emit recording-saved event: {}", e);
            if let Some(recorder) = measurement_recorder.as_ref() {
                recorder.record_stop_stage(
                    "recording_saved_event_emitted",
                    "failed",
                    Some(&e.to_string()),
                );
            }
        } else if let Some(recorder) = measurement_recorder.as_ref() {
            recorder.record_stop_stage("recording_saved_event_emitted", "completed", None);
        }

        // Keep this meeting's segments until the saver is dropped. A repeated
        // stop must not overwrite transcripts.json with an empty collection.
        if let Some(recorder) = measurement_recorder.as_ref() {
            recorder.record_stop_stage("final_save_finished", "completed", None);
        }

        Ok(Some(final_audio_path.to_string_lossy().to_string()))
    }

    /// Get the meeting folder path (for passing to backend)
    pub fn get_meeting_folder(&self) -> Option<&PathBuf> {
        self.meeting_folder.as_ref()
    }

    /// Get accumulated transcript segments (for reload sync)
    pub fn get_transcript_segments(&self) -> Vec<TranscriptSegment> {
        if let Ok(segments) = self.transcript_segments.lock() {
            segments.clone()
        } else {
            Vec::new()
        }
    }

    /// Get meeting name (for reload sync)
    pub fn get_meeting_name(&self) -> Option<String> {
        self.meeting_name.clone()
    }
}

#[cfg(test)]
#[path = "recording_saver_shutdown_tests.rs"]
mod shutdown_tests;

#[cfg(test)]
#[path = "recording_saver_drain_tests.rs"]
mod drain_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::meeting_context::{MeetingContextContainer, MeetingContextProfile, PersonProfile};
    use chrono::{TimeZone, Utc};
    use tempfile::tempdir;

    fn context_container() -> MeetingContextContainer {
        MeetingContextContainer::from_profile(
            MeetingContextProfile {
                schema_version: 1,
                fixed_meeting_mechanism: Some("每周三 14:00".to_owned()),
                people: vec![PersonProfile {
                    person_id: "person_rayson".to_owned(),
                    display_name: "Rayson".to_owned(),
                    aliases: vec!["瑞森".to_owned()],
                    department: None,
                    role: None,
                    enabled: true,
                }],
                terms: Vec::new(),
            },
            "license_station_weekly".to_owned(),
            1,
            "a".repeat(64),
            Utc.timestamp_opt(1_700_000_000, 0).unwrap(),
        )
        .unwrap()
    }

    fn metadata(context: Option<MeetingContextContainer>) -> MeetingMetadata {
        MeetingMetadata {
            version: "1.0".to_owned(),
            meeting_id: None,
            meeting_name: Some("Test meeting".to_owned()),
            created_at: "2026-08-26T00:00:00Z".to_owned(),
            completed_at: None,
            duration_seconds: None,
            devices: DeviceInfo {
                microphone: None,
                system_audio: None,
                ..DeviceInfo::default()
            },
            audio_file: "audio.mp4".to_owned(),
            transcript_file: "transcripts.json".to_owned(),
            sample_rate: 48_000,
            status: "recording".to_owned(),
            audio_route_evidence: None,
            summary_template: None,
            meeting_context: context,
            realtime_measurement: None,
        }
    }

    #[test]
    fn old_metadata_without_context_remains_compatible() {
        let raw = r#"{
            "version":"1.0",
            "meeting_id":null,
            "meeting_name":"Legacy",
            "created_at":"2026-08-26T00:00:00Z",
            "completed_at":null,
            "duration_seconds":null,
            "devices":{"microphone":null,"system_audio":null},
            "audio_file":"audio.mp4",
            "transcript_file":"transcripts.json",
            "sample_rate":48000,
            "status":"completed"
        }"#;
        let parsed: MeetingMetadata = serde_json::from_str(raw).unwrap();
        assert!(parsed.meeting_context.is_none());
        assert!(parsed.summary_template.is_none());
    }

    #[test]
    fn metadata_rewrites_preserve_context_identity_and_hash() {
        let directory = tempdir().unwrap();
        let folder = directory.path().to_path_buf();
        let context = context_container();
        let recording_id = context.recording_context_id.clone();
        let recording_hash = context.recording_context().unwrap().context_sha256.clone();
        let saver = RecordingSaver::new(directory.path().to_path_buf());
        let mut value = metadata(Some(context));

        saver.write_metadata(&folder, &value).unwrap();
        value.devices.microphone = Some("Test microphone".to_owned());
        value.status = "completed".to_owned();
        value.duration_seconds = Some(42.5);
        saver.write_metadata(&folder, &value).unwrap();

        let stored: MeetingMetadata =
            serde_json::from_str(&std::fs::read_to_string(folder.join("metadata.json")).unwrap())
                .unwrap();
        let stored_context = stored.meeting_context.unwrap();
        assert_eq!(stored_context.recording_context_id, recording_id);
        assert_eq!(
            stored_context.recording_context().unwrap().context_sha256,
            recording_hash
        );
        assert_eq!(
            stored.devices.microphone.as_deref(),
            Some("Test microphone")
        );
        assert_eq!(stored.status, "completed");
    }

    #[test]
    fn context_cannot_be_replaced_after_persistence_starts() {
        let directory = tempdir().unwrap();
        let mut saver = RecordingSaver::new(directory.path().to_path_buf());
        saver.meeting_folder = Some(directory.path().to_path_buf());
        let error = saver
            .set_meeting_context(Some(context_container()))
            .unwrap_err();
        assert!(error
            .to_string()
            .contains("cannot be replaced after recording persistence starts"));
    }

    #[test]
    fn metadata_write_failure_is_returned_to_caller() {
        let directory = tempdir().unwrap();
        let not_a_directory = directory.path().join("file");
        std::fs::write(&not_a_directory, "not a directory").unwrap();
        let saver = RecordingSaver::new(directory.path().to_path_buf());
        assert!(saver
            .write_metadata(&not_a_directory, &metadata(None))
            .is_err());
    }

    #[test]
    fn folder_initialization_failure_leaves_no_persisted_state() {
        let directory = tempdir().unwrap();
        let base_path = directory.path().join("not-a-directory");
        std::fs::write(&base_path, "file blocks directory creation").unwrap();
        let mut saver = RecordingSaver::new(directory.path().to_path_buf());
        saver
            .set_meeting_context(Some(context_container()))
            .unwrap();

        let error = saver
            .initialize_meeting_folder_at(&base_path, "Must fail", false)
            .unwrap_err();
        assert!(!error.to_string().is_empty());
        assert!(saver.meeting_folder.is_none());
        assert!(saver.metadata.is_none());
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 1);
    }

    #[test]
    fn device_metadata_write_failure_bubbles_without_in_memory_commit() {
        let directory = tempdir().unwrap();
        let not_a_directory = directory.path().join("file");
        std::fs::write(&not_a_directory, "not a directory").unwrap();
        let mut saver = RecordingSaver::new(directory.path().to_path_buf());
        saver.meeting_folder = Some(not_a_directory);
        saver.metadata = Some(metadata(Some(context_container())));

        let error = saver
            .set_device_info(Some("New mic".to_owned()), None)
            .unwrap_err();
        assert!(!error.to_string().is_empty());
        assert_eq!(
            saver
                .metadata
                .as_ref()
                .unwrap()
                .devices
                .microphone
                .as_deref(),
            None
        );
    }

    #[test]
    fn initialization_device_update_and_completion_preserve_context() {
        let directory = tempdir().unwrap();
        let mut saver = RecordingSaver::new(directory.path().to_path_buf());
        let context = context_container();
        let recording_id = context.recording_context_id.clone();
        let recording_hash = context.recording_context().unwrap().context_sha256.clone();
        saver
            .set_meeting_context(Some(context))
            .expect("valid context");
        saver
            .set_summary_template(Some(RecordingSummaryTemplatePreference {
                schema_version: 1,
                mode: "custom".to_owned(),
                template_id: Some("license_station_weekly".to_owned()),
                template_version: Some(3),
                template_file_sha256: Some("c".repeat(64)),
                selected_at: "2026-08-26T00:00:00Z".to_owned(),
            }))
            .unwrap();
        saver
            .set_device_info(Some("Mic A".to_owned()), Some("System A".to_owned()))
            .unwrap();
        saver
            .initialize_meeting_folder_at(
                &directory.path().to_path_buf(),
                "Context lifecycle",
                false,
            )
            .unwrap();
        saver
            .set_device_info(Some("Mic B".to_owned()), Some("System B".to_owned()))
            .unwrap();
        saver.complete_metadata(Some(42.5)).unwrap();

        let stored: MeetingMetadata = serde_json::from_str(
            &std::fs::read_to_string(saver.meeting_folder.as_ref().unwrap().join("metadata.json"))
                .unwrap(),
        )
        .unwrap();
        let stored_context = stored.meeting_context.unwrap();
        assert_eq!(stored_context.recording_context_id, recording_id);
        assert_eq!(
            stored_context.recording_context().unwrap().context_sha256,
            recording_hash
        );
        assert_eq!(stored.devices.microphone.as_deref(), Some("Mic B"));
        assert_eq!(stored.devices.system_audio.as_deref(), Some("System B"));
        assert_eq!(stored.status, "completed");
        assert_eq!(stored.duration_seconds, Some(42.5));
        assert!(stored.completed_at.is_some());
        let summary_template = stored.summary_template.unwrap();
        assert_eq!(
            summary_template.template_id.as_deref(),
            Some("license_station_weekly")
        );
        assert_eq!(summary_template.template_version, Some(3));
        assert_eq!(summary_template.template_file_sha256, Some("c".repeat(64)));
    }

    #[tokio::test]
    async fn saved_folder_is_used_for_live_recording() {
        let selected_root = tempdir().unwrap();
        let default_root = tempdir().unwrap();
        let mut saver = RecordingSaver::new(selected_root.path().to_path_buf());
        saver.set_meeting_name(Some("Saved folder test".to_string()));

        let sender = saver.start_accumulation(true).unwrap();
        saver.add_transcript_segment(TranscriptSegment {
            id: "segment-1".to_string(),
            text: "directory contract".to_string(),
            audio_start_time: 0.0,
            audio_end_time: 0.5,
            duration: 0.5,
            display_time: "[00:00]".to_string(),
            confidence: 1.0,
            sequence_id: 1,
            revision: 0,
            is_partial: false,
        });
        drop(sender);

        let meeting_folder = saver.get_meeting_folder().unwrap();
        assert!(meeting_folder.starts_with(selected_root.path()));
        assert!(meeting_folder.join("metadata.json").is_file());
        assert!(meeting_folder.join("transcripts.json").is_file());
        assert!(meeting_folder.join(".checkpoints").is_dir());
        let stored: MeetingMetadata = serde_json::from_str(
            &std::fs::read_to_string(meeting_folder.join("metadata.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(stored.audio_file, "audio.mp4");
        assert!(meeting_folder
            .join(&stored.audio_file)
            .starts_with(selected_root.path()));
        assert_eq!(std::fs::read_dir(default_root.path()).unwrap().count(), 0);
    }

    #[tokio::test]
    async fn recording_folder_is_frozen_for_active_session() {
        let first_root = tempdir().unwrap();
        let second_root = tempdir().unwrap();
        let mut configured_root = first_root.path().to_path_buf();
        let mut saver = RecordingSaver::new(configured_root.clone());
        saver.set_meeting_name(Some("Frozen folder test".to_string()));
        let sender = saver.start_accumulation(false).unwrap();

        // Simulate changing the saved preference while this recording is active.
        configured_root = second_root.path().to_path_buf();
        saver.add_transcript_segment(TranscriptSegment {
            id: "segment-1".to_string(),
            text: "still in first root".to_string(),
            audio_start_time: 0.0,
            audio_end_time: 0.5,
            duration: 0.5,
            display_time: "[00:00]".to_string(),
            confidence: 1.0,
            sequence_id: 1,
            revision: 0,
            is_partial: false,
        });
        drop(sender);

        let meeting_folder = saver.get_meeting_folder().unwrap();
        assert!(meeting_folder.starts_with(first_root.path()));
        assert!(!meeting_folder.starts_with(&configured_root));
        assert!(meeting_folder.join("transcripts.json").is_file());
        assert_eq!(std::fs::read_dir(second_root.path()).unwrap().count(), 0);
    }

    #[test]
    fn invalid_context_is_rejected_before_persistence() {
        let directory = tempdir().unwrap();
        let mut saver = RecordingSaver::new(directory.path().to_path_buf());
        let mut context = context_container();
        context.contexts[0].people[0].display_name = "Tampered".to_owned();
        let error = saver.set_meeting_context(Some(context)).unwrap_err();
        assert!(error.to_string().contains("CONTEXT_HASH_MISMATCH"));
        assert!(saver.pending_meeting_context.is_none());
    }

    fn shutdown_test_chunk(id: u64, frequency: f32) -> AudioChunk {
        AudioChunk {
            data: (0..48_000)
                .map(|sample| {
                    (sample as f32 * std::f32::consts::TAU * frequency / 48_000.0).sin() * 0.3
                })
                .collect(),
            sample_rate: 48_000,
            timestamp: id as f64,
            chunk_id: id,
            device_type: super::super::recording_state::DeviceType::Microphone,
            device_epoch: 0,
            capture_qpc_ns: None,
        }
    }

    #[tokio::test]
    async fn shutdown_drains_accepted_audio_after_two_second_write_delay() {
        let directory = tempdir().unwrap();
        let mut saver = RecordingSaver::new(directory.path().to_path_buf());
        saver.set_meeting_name(Some("Delayed writer tail".to_owned()));
        let sender = saver.start_accumulation(true).unwrap();
        let writer = saver.incremental_saver.as_ref().unwrap().clone();
        let write_guard = writer.lock().await;
        sender.send(shutdown_test_chunk(0, 440.0)).unwrap();
        // On this current-thread runtime, the writer now waits on write_guard.
        tokio::task::yield_now().await;
        sender.send(shutdown_test_chunk(1, 880.0)).unwrap();
        drop(sender);

        let app = tauri::test::mock_app();
        let started = std::time::Instant::now();
        let (saved, ()) = tokio::join!(saver.stop_and_save(app.handle(), Some(2.0)), async {
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
            drop(write_guard);
        });
        let path = PathBuf::from(saved.expect("accepted audio must be drained").unwrap());
        assert!(started.elapsed() >= std::time::Duration::from_secs(2));
        let decoded = super::super::decoder::decode_audio_file(&path).unwrap();
        assert!(
            (1.95..2.10).contains(&decoded.duration_seconds),
            "expected both seconds, decoded {} seconds",
            decoded.duration_seconds
        );
        assert_eq!(decoded.channels, 1);
        let start = (1.3 * decoded.sample_rate as f64) as usize;
        let end = (1.8 * decoded.sample_rate as f64) as usize;
        let tail = &decoded.samples[start..end];
        let rms =
            (tail.iter().map(|sample| sample * sample).sum::<f32>() / tail.len() as f32).sqrt();
        assert!(rms > 0.1, "tail must contain sound, not padding silence");
        let crossings = tail
            .windows(2)
            .filter(|pair| pair[0] <= 0.0 && pair[1] > 0.0)
            .count();
        let frequency = crossings as f64 * decoded.sample_rate as f64 / tail.len() as f64;
        assert!(
            (850.0..910.0).contains(&frequency),
            "missing 880Hz tail: {frequency}"
        );
    }

    #[tokio::test]
    async fn shutdown_text_only_does_not_create_audio_or_checkpoints() {
        let directory = tempdir().unwrap();
        let mut saver = RecordingSaver::new(directory.path().to_path_buf());
        saver.set_meeting_name(Some("Text only shutdown".to_owned()));
        let sender = saver.start_accumulation(false).unwrap();
        sender.send(shutdown_test_chunk(0, 440.0)).unwrap();
        drop(sender);
        let app = tauri::test::mock_app();
        assert_eq!(
            saver.stop_and_save(app.handle(), Some(1.0)).await.unwrap(),
            None
        );
        let folder = saver.get_meeting_folder().unwrap();
        assert!(folder.join("transcripts.json").is_file());
        assert!(!folder.join("audio.mp4").exists());
        assert!(!folder.join(".checkpoints").exists());
    }

    #[test]
    fn missing_meeting_name_fails_before_channel_initialization() {
        let directory = tempdir().unwrap();
        let mut saver = RecordingSaver::new(directory.path().to_path_buf());
        let error = saver.start_accumulation(false).unwrap_err();
        assert!(error.to_string().contains("meeting name is required"));
        assert!(saver.save_task.is_none());
        assert!(saver.stop_tx.is_none());
        assert!(saver.meeting_folder.is_none());
        assert!(saver.metadata.is_none());
    }
}

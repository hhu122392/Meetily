// audio/transcription/worker.rs
//
// Parallel transcription worker pool and chunk processing logic.

use super::engine::TranscriptionEngine;
use super::provider::TranscriptionError;
use crate::audio::measurement::D11MeasurementRecorder;
use crate::audio::recording_saver::{TranscriptSegment, TranscriptWriter};
use crate::audio::AudioChunk;
use crate::meeting_context::RecognitionContext;
use log::{error, info, warn};
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use tauri::{AppHandle, Emitter, Runtime};

// Sequence counter for transcript updates
static SEQUENCE_COUNTER: AtomicU64 = AtomicU64::new(0);

fn next_sequence_id() -> u64 { SEQUENCE_COUNTER.fetch_add(1, Ordering::SeqCst) }

// Speech detection flag - reset per recording session
static SPEECH_DETECTED_EMITTED: AtomicBool = AtomicBool::new(false);

// These counters are the product's live queue truth. They are updated at the
// same points as the worker-local accounting and are exposed to the stop flow
// so finalization cannot claim success while work is still pending.
static CURRENT_CHUNKS_QUEUED: AtomicU64 = AtomicU64::new(0);
static CURRENT_CHUNKS_COMPLETED: AtomicU64 = AtomicU64::new(0);
static TRANSCRIPTION_ACTIVE: AtomicBool = AtomicBool::new(false);

struct TranscriptionActivityGuard;

impl Drop for TranscriptionActivityGuard {
    fn drop(&mut self) {
        TRANSCRIPTION_ACTIVE.store(false, Ordering::SeqCst);
    }
}

fn mark_chunks_completed(count: u64) {
    CURRENT_CHUNKS_COMPLETED.fetch_add(count, Ordering::SeqCst);
}

fn pending_chunk_count(queued: u64, completed: u64) -> usize {
    queued.saturating_sub(completed) as usize
}

pub fn current_transcription_queue_status() -> (usize, bool) {
    let queued = CURRENT_CHUNKS_QUEUED.load(Ordering::SeqCst);
    let completed = CURRENT_CHUNKS_COMPLETED.load(Ordering::SeqCst);
    (
        pending_chunk_count(queued, completed),
        TRANSCRIPTION_ACTIVE.load(Ordering::SeqCst),
    )
}

// whisper.cpp pads short input to its encoder context, so repeatedly invoking it for
// many 3-7 second VAD fragments is disproportionately expensive on CPU. When the
// worker is already behind, combine only the fragments that are already queued into
// one bounded batch. This preserves every source chunk while avoiding dozens of
// fixed-cost inference calls during shutdown. A real 9.81-second CPU batch took
// 10.15 seconds and missed the live bubble gate, so keep merged batches at or
// below nine seconds. The Whisper layer also reduces its encoder context to the
// actual bounded audio duration instead of padding every batch to 30s.
const MAX_WHISPER_BATCH_DURATION_SECONDS: f64 = 9.0;
const MAX_WHISPER_BATCH_GAP_SECONDS: f64 = 2.0;

fn try_merge_transcription_chunk(
    current: &mut AudioChunk,
    next: AudioChunk,
) -> Result<u64, AudioChunk> {
    if current.sample_rate == 0 || current.sample_rate != next.sample_rate {
        return Err(next);
    }

    // Never treat a chunk from an earlier timeline position as overlap. The
    // previous implementation could trim the entire out-of-order chunk and
    // still increment the "completed" counter, producing a false zero-loss
    // result.
    if next.timestamp < current.timestamp {
        return Err(next);
    }

    let sample_rate = current.sample_rate as f64;
    let current_end = current.timestamp + current.data.len() as f64 / sample_rate;
    let gap_seconds = next.timestamp - current_end;

    if gap_seconds > MAX_WHISPER_BATCH_GAP_SECONDS {
        return Err(next);
    }

    let silence_samples = if gap_seconds > 0.0 {
        (gap_seconds * sample_rate).round() as usize
    } else {
        0
    };
    let overlap_samples = if gap_seconds < 0.0 {
        ((-gap_seconds) * sample_rate).round() as usize
    } else {
        0
    }
    .min(next.data.len());
    let next_samples = next.data.len() - overlap_samples;
    let max_samples = (MAX_WHISPER_BATCH_DURATION_SECONDS * current.sample_rate as f64) as usize;

    if current.data.len() + silence_samples + next_samples > max_samples {
        return Err(next);
    }

    current
        .data
        .extend(std::iter::repeat(0.0).take(silence_samples));
    current
        .data
        .extend_from_slice(&next.data[overlap_samples..]);
    Ok(overlap_samples as u64)
}

/// Reset the speech detected flag for a new recording session
pub fn reset_speech_detected_flag() {
    SPEECH_DETECTED_EMITTED.store(false, Ordering::SeqCst);
    info!(
        "🔍 SPEECH_DETECTED_EMITTED reset to: {}",
        SPEECH_DETECTED_EMITTED.load(Ordering::SeqCst)
    );
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct TranscriptUpdate {
    #[serde(default)]
    pub chunk_id: u64,
    #[serde(default)]
    pub source_chunk_ids: Vec<u64>,
    pub text: String,
    pub timestamp: String, // Wall-clock time for reference (e.g., "14:30:05")
    pub source: String,
    pub sequence_id: u64,
    pub chunk_start_time: f64, // Legacy field, kept for compatibility
    pub is_partial: bool,
    #[serde(default)]
    pub revision: u64,
    pub confidence: f32,
    // NEW: Recording-relative timestamps for playback sync
    pub audio_start_time: f64, // Seconds from recording start (e.g., 125.3)
    pub audio_end_time: f64,   // Seconds from recording start (e.g., 128.6)
    pub duration: f64,         // Segment duration in seconds (e.g., 3.3)
}

// NOTE: get_transcript_history and get_recording_meeting_name functions
// have been moved to recording_commands.rs where they have access to RECORDING_MANAGER

/// Optimized parallel transcription task ensuring ZERO chunk loss
pub fn start_transcription_task<R: Runtime>(
    app: AppHandle<R>,
    transcription_receiver: tokio::sync::mpsc::UnboundedReceiver<AudioChunk>,
    recognition_context: Option<Arc<RecognitionContext>>,
    measurement_recorder: Option<Arc<D11MeasurementRecorder>>,
    transcript_writer: TranscriptWriter,
) -> tokio::task::JoinHandle<()> {
    CURRENT_CHUNKS_QUEUED.store(0, Ordering::SeqCst);
    CURRENT_CHUNKS_COMPLETED.store(0, Ordering::SeqCst);
    TRANSCRIPTION_ACTIVE.store(true, Ordering::SeqCst);
    tokio::spawn(async move {
        let _transcription_activity_guard = TranscriptionActivityGuard;
        info!("🚀 Starting optimized parallel transcription task - guaranteeing zero chunk loss");
        if let Some(context) = recognition_context.as_ref() {
            info!("Recognition context enabled: {:?}", context.diagnostics());
        } else {
            info!("Recognition context disabled for this recording");
        }

        // Initialize transcription engine (Whisper or Parakeet based on config)
        let transcription_engine = match super::engine::get_or_init_transcription_engine(&app).await
        {
            Ok(engine) => engine,
            Err(e) => {
                error!("Failed to initialize transcription engine: {}", e);
                if let Some(recorder) = measurement_recorder.as_ref() {
                    recorder
                        .record_error(format!("failed to initialize transcription engine: {e}"));
                }
                let _ = app.emit("transcription-error", serde_json::json!({
                    "error": e,
                    "userMessage": "Recording failed: Unable to initialize speech recognition. Please check your model settings.",
                    "actionable": true
                }));
                return;
            }
        };

        // Create parallel workers for faster processing while preserving ALL chunks
        const NUM_WORKERS: usize = 1; // Serial processing ensures transcripts emit in chronological order
        let (work_sender, work_receiver) = tokio::sync::mpsc::unbounded_channel::<AudioChunk>();
        let work_receiver = Arc::new(tokio::sync::Mutex::new(work_receiver));

        // Track completion: AtomicU64 for chunks queued, AtomicU64 for chunks completed
        let chunks_queued = Arc::new(AtomicU64::new(0));
        let chunks_completed = Arc::new(AtomicU64::new(0));
        let input_finished = Arc::new(AtomicBool::new(false));

        info!(
            "📊 Starting {} transcription worker{} (serial mode for ordered emission)",
            NUM_WORKERS,
            if NUM_WORKERS == 1 { "" } else { "s" }
        );

        // Spawn worker tasks
        let mut worker_handles = Vec::new();
        for worker_id in 0..NUM_WORKERS {
            let engine_clone = match &transcription_engine {
                TranscriptionEngine::SenseVoice(e) => TranscriptionEngine::SenseVoice(e.clone()),
                TranscriptionEngine::Whisper(e) => TranscriptionEngine::Whisper(e.clone()),
                TranscriptionEngine::Parakeet(e) => TranscriptionEngine::Parakeet(e.clone()),
                TranscriptionEngine::Provider(p) => TranscriptionEngine::Provider(p.clone()),
            };
            let app_clone = app.clone();
            let work_receiver_clone = work_receiver.clone();
            let chunks_completed_clone = chunks_completed.clone();
            let input_finished_clone = input_finished.clone();
            let chunks_queued_clone = chunks_queued.clone();
            let recognition_context_clone = recognition_context.clone();
            let measurement_recorder_clone = measurement_recorder.clone();
            let transcript_writer_clone = transcript_writer.clone();

            let worker_handle = tokio::spawn(async move {
                info!("👷 Worker {} started", worker_id);
                let should_coalesce_backlog =
                    matches!(&engine_clone, TranscriptionEngine::Whisper(_));
                let mut pending_chunk: Option<AudioChunk> = None;
                let mut sensevoice_worker = if let TranscriptionEngine::SenseVoice(engine) = &engine_clone {
                    match engine.new_live_session().await {
                        Ok(session) => Some(super::sensevoice_live_worker::SenseVoiceLiveWorker::new(session)),
                        Err(error) => {
                            transcript_writer_clone.report_transcription_error(&error.to_string());
                            let _ = app_clone.emit("transcription-error", serde_json::json!({
                                "error":error.to_string(), "userMessage":"SenseVoice 初始化失败，请重新开始录音。", "actionable":true
                            }));
                            None
                        }
                    }
                } else { None };

                // PRE-VALIDATE model state to avoid repeated async calls per chunk
                let initial_model_loaded = engine_clone.is_model_loaded().await;
                let current_model = engine_clone
                    .get_current_model()
                    .await
                    .unwrap_or_else(|| "unknown".to_string());

                let engine_name = engine_clone.provider_name();
                if let Some(recorder) = measurement_recorder_clone.as_ref() {
                    recorder.record_engine_identity(&engine_name, &current_model);
                }

                if initial_model_loaded {
                    info!(
                        "✅ Worker {} pre-validation: {} model '{}' is loaded and ready",
                        worker_id, engine_name, current_model
                    );
                } else {
                    warn!(
                        "⚠️ Worker {} pre-validation: {} model not loaded - chunks may be skipped",
                        worker_id, engine_name
                    );
                }

                loop {
                    // Try to get a chunk to process
                    let chunk = if let Some(chunk) = pending_chunk.take() {
                        Some(chunk)
                    } else {
                        let mut receiver = work_receiver_clone.lock().await;
                        receiver.recv().await
                    };

                    match chunk {
                        Some(mut chunk) => {
                            let first_chunk_id = chunk.chunk_id;
                            let mut last_chunk_id = chunk.chunk_id;
                            let mut represented_chunks = 1_u64;
                            let mut source_chunk_ids = vec![chunk.chunk_id];
                            if let Some(recorder) = measurement_recorder_clone.as_ref() {
                                recorder.record_dequeued(chunk.chunk_id);
                            }

                            if should_coalesce_backlog {
                                loop {
                                    let next_chunk = {
                                        let mut receiver = work_receiver_clone.lock().await;
                                        receiver.try_recv()
                                    };

                                    match next_chunk {
                                        Ok(next) => {
                                            let next_id = next.chunk_id;
                                            match try_merge_transcription_chunk(&mut chunk, next) {
                                                Ok(overlap_samples) => {
                                                    represented_chunks += 1;
                                                    last_chunk_id = next_id;
                                                    source_chunk_ids.push(next_id);
                                                    if let Some(recorder) =
                                                        measurement_recorder_clone.as_ref()
                                                    {
                                                        recorder.record_dequeued(next_id);
                                                        recorder.record_overlap(
                                                            first_chunk_id,
                                                            next_id,
                                                            overlap_samples,
                                                        );
                                                    }
                                                }
                                                Err(next) => {
                                                    pending_chunk = Some(next);
                                                    break;
                                                }
                                            }
                                        }
                                        Err(tokio::sync::mpsc::error::TryRecvError::Empty)
                                        | Err(
                                            tokio::sync::mpsc::error::TryRecvError::Disconnected,
                                        ) => {
                                            break;
                                        }
                                    }
                                }

                                if represented_chunks > 1 {
                                    info!(
                                        "📦 Worker {} coalesced queued chunks {}..{} ({} source chunks, {:.2}s audio)",
                                        worker_id,
                                        first_chunk_id,
                                        last_chunk_id,
                                        represented_chunks,
                                        chunk.data.len() as f64 / chunk.sample_rate as f64
                                    );
                                }
                            }

                            // PERFORMANCE OPTIMIZATION: Reduce logging in hot path
                            // Only log every 10th chunk per worker to reduce I/O overhead
                            let should_log_this_chunk = chunk.chunk_id % 10 == 0;

                            if should_log_this_chunk {
                                info!(
                                    "👷 Worker {} processing chunk {} with {} samples",
                                    worker_id,
                                    chunk.chunk_id,
                                    chunk.data.len()
                                );
                            }

                            // Check if model is still loaded before processing
                            if !engine_clone.is_model_loaded().await {
                                transcript_writer_clone.report_transcription_error("The transcription model was unloaded before queued audio was processed");
                                warn!("⚠️ Worker {}: Model unloaded, but continuing to preserve chunk {}", worker_id, chunk.chunk_id);
                                // Still count as completed even if we can't process
                                chunks_completed_clone
                                    .fetch_add(represented_chunks, Ordering::SeqCst);
                                mark_chunks_completed(represented_chunks);
                                if let Some(recorder) = measurement_recorder_clone.as_ref() {
                                    recorder.record_terminal_without_writeback(
                                        &source_chunk_ids,
                                        "model_unloaded",
                                    );
                                }
                                continue;
                            }

                            let chunk_timestamp = chunk.timestamp;
                            let chunk_duration = chunk.data.len() as f64 / chunk.sample_rate as f64;

                            if let Some(worker) = sensevoice_worker.as_mut() {
                                let result = worker.push(chunk, &app_clone, &transcript_writer_clone,
                                    measurement_recorder_clone.as_deref(), recognition_context_clone.as_deref(),
                                    &current_model, next_sequence_id).await;
                                if let Err(error) = result {
                                    transcript_writer_clone.report_transcription_error(&error);
                                    if let Some(recorder) = measurement_recorder_clone.as_ref() {
                                        recorder.record_inference_finished(&source_chunk_ids, Some(&error));
                                        recorder.record_final_writeback(&source_chunk_ids, Err(error.clone()));
                                    }
                                    let _ = app_clone.emit("transcription-error", serde_json::json!({
                                        "error":error,"userMessage":"部分录音转写失败，原录音已保留。", "actionable":true
                                    }));
                                }
                                if !SPEECH_DETECTED_EMITTED.load(Ordering::SeqCst) && worker.has_text() {
                                    SPEECH_DETECTED_EMITTED.store(true, Ordering::SeqCst);
                                    let _ = app_clone.emit("speech-detected", serde_json::json!({"message":"Speech activity detected"}));
                                }
                                let completed = chunks_completed_clone.fetch_add(represented_chunks, Ordering::SeqCst) + represented_chunks;
                                mark_chunks_completed(represented_chunks);
                                let queued = chunks_queued_clone.load(Ordering::SeqCst);
                                let _ = app_clone.emit("transcription-progress", serde_json::json!({
                                    "worker_id":worker_id,"chunks_completed":completed,"chunks_queued":queued,
                                    "progress_percentage":completed*100/queued.max(1)
                                }));
                                continue;
                            }

                            // Transcribe with provider-agnostic approach
                            match transcribe_chunk_with_provider(
                                &engine_clone,
                                chunk,
                                &app_clone,
                                recognition_context_clone.as_deref(),
                                &source_chunk_ids,
                                measurement_recorder_clone.as_deref(),
                                &engine_name,
                                &current_model,
                            )
                            .await
                            {
                                Ok((
                                    raw_transcript,
                                    confidence_opt,
                                    is_partial,
                                    text_before_dedup,
                                    text_after_dedup,
                                    dedup_trace_status,
                                )) => {
                                    let transcript = recognition_context_clone
                                        .as_ref()
                                        .map_or(raw_transcript.clone(), |context| {
                                            context.normalize_transcript(&raw_transcript)
                                        });
                                    if let Some(recorder) = measurement_recorder_clone.as_ref() {
                                        recorder.record_inference_finished(&source_chunk_ids, None);
                                        recorder.record_text(
                                            &source_chunk_ids,
                                            &text_before_dedup,
                                            &text_after_dedup,
                                            &transcript,
                                            dedup_trace_status,
                                        );
                                    }
                                    // Provider-aware confidence threshold
                                    let confidence_threshold = match &engine_clone {
                                        TranscriptionEngine::SenseVoice(_) => 0.0,
                                        TranscriptionEngine::Whisper(_)
                                        | TranscriptionEngine::Provider(_) => 0.3,
                                        TranscriptionEngine::Parakeet(_) => 0.0, // Parakeet has no confidence, accept all
                                    };

                                    let confidence_str = match confidence_opt {
                                        Some(c) => format!("{:.2}", c),
                                        None => "N/A".to_string(),
                                    };

                                    info!("🔍 Worker {} transcription result: text='{}', confidence={}, partial={}, threshold={:.2}",
                                          worker_id, transcript, confidence_str, is_partial, confidence_threshold);

                                    // Check confidence threshold (or accept if no confidence provided)
                                    let meets_threshold =
                                        confidence_opt.map_or(true, |c| c >= confidence_threshold);

                                    if !transcript.trim().is_empty() && meets_threshold {
                                        // PERFORMANCE: Only log transcription results, not every processing step
                                        info!("✅ Worker {} transcribed: {} (confidence: {}, partial: {})",
                                              worker_id, transcript, confidence_str, is_partial);

                                        // Emit speech-detected event for frontend UX (only on first detection per session)
                                        // This is lightweight and provides better user feedback
                                        let current_flag =
                                            SPEECH_DETECTED_EMITTED.load(Ordering::SeqCst);
                                        info!("🔍 Checking speech-detected flag: current={}, will_emit={}", current_flag, !current_flag);

                                        if !current_flag {
                                            SPEECH_DETECTED_EMITTED.store(true, Ordering::SeqCst);
                                            match app_clone.emit("speech-detected", serde_json::json!({
                                                "message": "Speech activity detected"
                                            })) {
                                                Ok(_) => info!("🎤 ✅ First speech detected - successfully emitted speech-detected event"),
                                                Err(e) => error!("🎤 ❌ Failed to emit speech-detected event: {}", e),
                                            }
                                        } else {
                                            info!("🔍 Speech already detected in this session, not re-emitting");
                                        }

                                        // Generate sequence ID and calculate timestamps FIRST
                                        let sequence_id =
                                            SEQUENCE_COUNTER.fetch_add(1, Ordering::SeqCst);
                                        let audio_start_time = chunk_timestamp; // Already in seconds from recording start
                                        let audio_end_time = chunk_timestamp + chunk_duration;

                                        let update = TranscriptUpdate {
                                            chunk_id: first_chunk_id,
                                            source_chunk_ids: source_chunk_ids.clone(),
                                            text: transcript,
                                            timestamp: format_current_timestamp(), // Wall-clock for reference
                                            source: "Audio".to_string(),
                                            sequence_id,
                                            chunk_start_time: chunk_timestamp, // Legacy compatibility
                                            is_partial,
                                            revision: 0,
                                            confidence: confidence_opt.unwrap_or(0.85), // Default for providers without confidence
                                            // NEW: Recording-relative timestamps for sync
                                            audio_start_time,
                                            audio_end_time,
                                            duration: chunk_duration,
                                        };

                                        let write_result = transcript_writer_clone
                                            .write(TranscriptSegment {
                                                id: format!("seg_{}", update.sequence_id),
                                                text: update.text.clone(),
                                                audio_start_time: update.audio_start_time,
                                                audio_end_time: update.audio_end_time,
                                                duration: update.duration,
                                                display_time: update.timestamp.clone(),
                                                confidence: update.confidence,
                                                sequence_id: update.sequence_id,
                                                revision: update.revision,
                                                is_partial: update.is_partial,
                                            })
                                            .map_err(|error| format!("{error:#}"));
                                        if let Some(recorder) = measurement_recorder_clone.as_ref() {
                                            recorder.record_final_writeback(
                                                &source_chunk_ids,
                                                write_result.clone(),
                                            );
                                        }
                                        if let Err(error) = write_result {
                                            error!(
                                                "Worker {}: Failed to save transcript: {}",
                                                worker_id, error
                                            );
                                            let _ = app_clone.emit("transcription-warning", error);
                                        } else if let Err(e) = app_clone.emit("transcript-update", &update)
                                        {
                                            error!(
                                                "Worker {}: Failed to emit transcript update: {}",
                                                worker_id, e
                                            );
                                            if let Some(recorder) =
                                                measurement_recorder_clone.as_ref()
                                            {
                                                recorder.record_error(format!(
                                                    "transcript persisted but UI notification failed: {e}"
                                                ));
                                            }
                                        }
                                        // PERFORMANCE: Removed verbose logging of every emission
                                    } else if !transcript.trim().is_empty() {
                                        // PERFORMANCE: Only log low-confidence results occasionally
                                        if should_log_this_chunk {
                                            if let Some(c) = confidence_opt {
                                                info!("Worker {} low-confidence transcription (confidence: {:.2}), skipping", worker_id, c);
                                            }
                                        }
                                        if let Some(recorder) = measurement_recorder_clone.as_ref()
                                        {
                                            recorder.record_terminal_without_writeback(
                                                &source_chunk_ids,
                                                "below_confidence_threshold",
                                            );
                                        }
                                    } else if let Some(recorder) =
                                        measurement_recorder_clone.as_ref()
                                    {
                                        recorder.record_terminal_without_writeback(
                                            &source_chunk_ids,
                                            "empty_transcript",
                                        );
                                    }
                                }
                                Err(e) => {
                                    if let Some(recorder) = measurement_recorder_clone.as_ref() {
                                        recorder.record_inference_finished(
                                            &source_chunk_ids,
                                            Some(&e.to_string()),
                                        );
                                        recorder.record_terminal_without_writeback(
                                            &source_chunk_ids,
                                            "transcription_failed",
                                        );
                                    }
                                    // Improved error handling with specific cases
                                    match e {
                                        TranscriptionError::AudioTooShort { .. } => {
                                            // Skip silently, this is expected for very short chunks
                                            info!("Worker {}: {}", worker_id, e);
                                            chunks_completed_clone
                                                .fetch_add(represented_chunks, Ordering::SeqCst);
                                            mark_chunks_completed(represented_chunks);
                                            continue;
                                        }
                                        TranscriptionError::ModelNotLoaded => {
                                            warn!(
                                                "Worker {}: Model unloaded during transcription",
                                                worker_id
                                            );
                                            chunks_completed_clone
                                                .fetch_add(represented_chunks, Ordering::SeqCst);
                                            mark_chunks_completed(represented_chunks);
                                            continue;
                                        }
                                        _ => {
                                            warn!(
                                                "Worker {}: Transcription failed: {}",
                                                worker_id, e
                                            );
                                            let _ = app_clone
                                                .emit("transcription-warning", e.to_string());
                                        }
                                    }
                                }
                            }

                            // Mark chunk as completed
                            let completed = chunks_completed_clone
                                .fetch_add(represented_chunks, Ordering::SeqCst)
                                + represented_chunks;
                            mark_chunks_completed(represented_chunks);
                            let queued = chunks_queued_clone.load(Ordering::SeqCst);

                            // PERFORMANCE: Only log progress every 5th chunk to reduce I/O overhead
                            if completed % 5 == 0 || should_log_this_chunk {
                                info!(
                                    "Worker {}: Progress {}/{} chunks ({:.1}%)",
                                    worker_id,
                                    completed,
                                    queued,
                                    (completed as f64 / queued.max(1) as f64 * 100.0)
                                );
                            }

                            // Emit progress event for frontend
                            let progress_percentage = if queued > 0 {
                                (completed as f64 / queued as f64 * 100.0) as u32
                            } else {
                                100
                            };

                            let _ = app_clone.emit("transcription-progress", serde_json::json!({
                                "worker_id": worker_id,
                                "chunks_completed": completed,
                                "chunks_queued": queued,
                                "progress_percentage": progress_percentage,
                                "message": format!("Worker {} processing... ({}/{})", worker_id, completed, queued)
                            }));
                        }
                        None => {
                            // No more chunks available
                            if input_finished_clone.load(Ordering::SeqCst) {
                                // Double-check that all queued chunks are actually completed
                                let final_queued = chunks_queued_clone.load(Ordering::SeqCst);
                                let final_completed = chunks_completed_clone.load(Ordering::SeqCst);

                                if final_completed >= final_queued {
                                    info!(
                                        "👷 Worker {} finishing - all {}/{} chunks processed",
                                        worker_id, final_completed, final_queued
                                    );
                                    break;
                                } else {
                                    warn!("👷 Worker {} detected potential chunk loss: {}/{} completed, waiting...", worker_id, final_completed, final_queued);
                                    // AGGRESSIVE POLLING: Reduced from 50ms to 5ms for faster chunk detection during shutdown
                                    tokio::time::sleep(tokio::time::Duration::from_millis(5)).await;
                                }
                            } else {
                                // AGGRESSIVE POLLING: Reduced from 10ms to 1ms for faster response during shutdown
                                tokio::time::sleep(tokio::time::Duration::from_millis(1)).await;
                            }
                        }
                    }
                }

                if let Some(worker) = sensevoice_worker.as_mut() {
                    if let Err(error) = worker.finish(&app_clone, &transcript_writer_clone,
                        measurement_recorder_clone.as_deref(), recognition_context_clone.as_deref(), next_sequence_id).await {
                        transcript_writer_clone.report_transcription_error(&error);
                        let _ = app_clone.emit("transcription-error", serde_json::json!({
                            "error":error,"userMessage":"最后一段转写未能保存，请保留原录音。", "actionable":true
                        }));
                    }
                }
                info!("👷 Worker {} completed", worker_id);
            });

            worker_handles.push(worker_handle);
        }

        // Main dispatcher: receive chunks and distribute to workers
        let mut receiver = transcription_receiver;
        while let Some(chunk) = receiver.recv().await {
            let queued = chunks_queued.fetch_add(1, Ordering::SeqCst) + 1;
            CURRENT_CHUNKS_QUEUED.fetch_add(1, Ordering::SeqCst);
            if let Some(recorder) = measurement_recorder.as_ref() {
                recorder.record_enqueued(chunk.chunk_id);
            }
            info!(
                "📥 Dispatching chunk {} to workers (total queued: {})",
                chunk.chunk_id, queued
            );

            if let Err(_) = work_sender.send(chunk) {
                error!("❌ Failed to send chunk to workers - this should not happen!");
                break;
            }
        }

        // Signal that input is finished
        input_finished.store(true, Ordering::SeqCst);
        drop(work_sender); // Close the channel to signal workers

        let total_chunks_queued = chunks_queued.load(Ordering::SeqCst);
        info!("📭 Input finished with {} total chunks queued. Waiting for all {} workers to complete...",
              total_chunks_queued, NUM_WORKERS);

        // Emit final chunk count to frontend
        let _ = app.emit("transcription-queue-complete", serde_json::json!({
            "total_chunks": total_chunks_queued,
            "message": format!("{} chunks queued for processing - waiting for completion", total_chunks_queued)
        }));

        // Wait for all workers to complete
        for (worker_id, handle) in worker_handles.into_iter().enumerate() {
            if let Err(e) = handle.await {
                transcript_writer.report_transcription_error(&format!("Transcription worker failed: {e}"));
                error!("❌ Worker {} panicked: {:?}", worker_id, e);
            } else {
                info!("✅ Worker {} completed successfully", worker_id);
            }
        }

        // Final verification with retry logic to catch any stragglers
        let mut verification_attempts = 0;
        const MAX_VERIFICATION_ATTEMPTS: u32 = 10;

        loop {
            let final_queued = chunks_queued.load(Ordering::SeqCst);
            let final_completed = chunks_completed.load(Ordering::SeqCst);

            if final_queued == final_completed {
                info!(
                    "🎉 ALL {} chunks processed successfully - ZERO chunks lost!",
                    final_completed
                );
                break;
            } else if verification_attempts < MAX_VERIFICATION_ATTEMPTS {
                verification_attempts += 1;
                warn!("⚠️ Chunk count mismatch (attempt {}): {} queued, {} completed - waiting for stragglers...",
                     verification_attempts, final_queued, final_completed);

                // Wait a bit for any remaining chunks to be processed
                tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
            } else {
                error!(
                    "❌ CRITICAL: After {} attempts, chunk loss detected: {} queued, {} completed",
                    MAX_VERIFICATION_ATTEMPTS, final_queued, final_completed
                );
                transcript_writer.report_transcription_error("Transcription queue did not finish processing all accepted audio");

                // Emit critical error event
                let _ = app.emit(
                    "transcript-chunk-loss-detected",
                    serde_json::json!({
                        "chunks_queued": final_queued,
                        "chunks_completed": final_completed,
                        "chunks_lost": final_queued - final_completed,
                        "message": "Some transcript chunks may have been lost during shutdown"
                    }),
                );
                break;
            }
        }

        info!("✅ Parallel transcription task completed - all workers finished, ready for model unload");
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pending_queue_count_never_underflows_and_reaches_zero_only_when_drained() {
        assert_eq!(pending_chunk_count(5, 2), 3);
        assert_eq!(pending_chunk_count(5, 5), 0);
        assert_eq!(pending_chunk_count(5, 6), 0);
    }
    use crate::audio::pipeline::live_segment_durations_ms;
    use crate::audio::vad::ContinuousVadProcessor;
    use crate::audio::RecordingDeviceType;
    use crate::meeting_context::{
        AttendanceStatus, MeetingContextSnapshot, MeetingContextSource, SnapshotPerson,
        SnapshotTerm,
    };
    use crate::whisper_engine::WhisperEngine;
    use chrono::Utc;
    use serde_json::json;
    use sha2::{Digest, Sha256};
    use std::path::PathBuf;
    use std::time::Instant;

    fn chunk(id: u64, timestamp: f64, samples: usize, sample_rate: u32) -> AudioChunk {
        AudioChunk {
            data: vec![id as f32 + 1.0; samples],
            sample_rate,
            timestamp,
            chunk_id: id,
            device_type: RecordingDeviceType::Microphone,
            device_epoch: 0,
            capture_qpc_ns: None,
        }
    }

    #[test]
    fn coalescing_inserts_short_silence_without_losing_source_audio() {
        let mut current = chunk(1, 0.0, 10, 10);
        let next = chunk(2, 1.5, 10, 10);

        try_merge_transcription_chunk(&mut current, next).unwrap();

        assert_eq!(current.data.len(), 25);
        assert!(current.data[10..15].iter().all(|sample| *sample == 0.0));
        assert!(current.data[15..].iter().all(|sample| *sample == 3.0));
    }

    #[test]
    fn coalescing_rejects_large_gaps_and_keeps_current_unchanged() {
        let mut current = chunk(1, 0.0, 10, 10);
        let original = current.data.clone();
        let next = chunk(2, 4.0, 10, 10);

        let returned = try_merge_transcription_chunk(&mut current, next).unwrap_err();

        assert_eq!(current.data, original);
        assert_eq!(returned.chunk_id, 2);
    }

    #[test]
    fn coalescing_trims_overlap_instead_of_duplicating_audio() {
        let mut current = chunk(1, 0.0, 10, 10);
        let next = chunk(2, 0.5, 10, 10);

        try_merge_transcription_chunk(&mut current, next).unwrap();

        assert_eq!(current.data.len(), 15);
        assert!(current.data[10..].iter().all(|sample| *sample == 3.0));
    }

    #[test]
    fn coalescing_never_discards_an_out_of_order_source_chunk() {
        let mut current = chunk(2, 10.0, 10, 10);
        let original = current.data.clone();
        let earlier = chunk(1, 4.0, 10, 10);

        let returned = try_merge_transcription_chunk(&mut current, earlier).unwrap_err();

        assert_eq!(current.data, original);
        assert_eq!(returned.chunk_id, 1);
        assert_eq!(returned.timestamp, 4.0);
    }

    #[test]
    fn coalescing_respects_whisper_context_bound() {
        let mut current = chunk(1, 0.0, 90, 10);
        let original_len = current.data.len();
        let next = chunk(2, 9.0, 20, 10);

        let returned = try_merge_transcription_chunk(&mut current, next).unwrap_err();

        assert_eq!(current.data.len(), original_len);
        assert_eq!(returned.chunk_id, 2);
    }

    #[test]
    fn coalescing_rejects_a_realistic_batch_that_would_exceed_nine_seconds() {
        let mut current = chunk(1, 0.0, 49, 10);
        let original_len = current.data.len();
        let next = chunk(2, 4.9, 49, 10);

        let returned = try_merge_transcription_chunk(&mut current, next).unwrap_err();

        assert_eq!(current.data.len(), original_len);
        assert_eq!(returned.chunk_id, 2);
    }

    #[derive(Clone)]
    struct ReplaySegment {
        chunk: AudioChunk,
        available_at_seconds: f64,
    }

    fn percentile_95(values: &[f64]) -> f64 {
        assert!(!values.is_empty());
        let mut sorted = values.to_vec();
        sorted.sort_by(f64::total_cmp);
        let rank = ((sorted.len() as f64) * 0.95).ceil() as usize;
        sorted[rank.saturating_sub(1).min(sorted.len() - 1)]
    }

    fn recognition_context_from_template(template: &serde_json::Value) -> RecognitionContext {
        let profile = &template["extensions"]["meetily_meeting_context"];
        let people = profile["people"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|person| person["enabled"].as_bool().unwrap_or(true))
            .map(|person| SnapshotPerson {
                person_id: person["person_id"].as_str().unwrap().to_owned(),
                display_name: person["display_name"].as_str().unwrap().to_owned(),
                aliases: person["aliases"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|alias| alias.as_str().unwrap().to_owned())
                    .collect(),
                department: person["department"].as_str().map(ToOwned::to_owned),
                role: person["role"].as_str().map(ToOwned::to_owned),
                attendance: AttendanceStatus::Expected,
            })
            .collect();
        let terms = profile["terms"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|term| term["enabled"].as_bool().unwrap_or(true))
            .map(|term| SnapshotTerm {
                term_id: term["term_id"].as_str().unwrap().to_owned(),
                canonical: term["canonical"].as_str().unwrap().to_owned(),
                aliases: term["aliases"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|alias| alias.as_str().unwrap().to_owned())
                    .collect(),
                category: term["category"].as_str().map(ToOwned::to_owned),
            })
            .collect();
        RecognitionContext::from_snapshot(&MeetingContextSnapshot {
            context_id: "ctx_mc_r03_replay".to_owned(),
            revision: 1,
            reason: "mc_r03_replay".to_owned(),
            captured_at: Utc::now(),
            source: MeetingContextSource {
                template_id: template["id"].as_str().unwrap().to_owned(),
                template_version: template["version"].as_u64().unwrap(),
                template_file_sha256: "a".repeat(64),
                profile_sha256: "b".repeat(64),
            },
            fixed_meeting_mechanism: profile["fixed_meeting_mechanism"]
                .as_str()
                .map(ToOwned::to_owned),
            people,
            host_person_id: None,
            terms,
            context_sha256: "c".repeat(64),
        })
    }

    fn prepare_replay_segments(samples: &[f32], sample_rate: u32) -> Vec<ReplaySegment> {
        let (first_segment_ms, subsequent_segment_ms) = live_segment_durations_ms();
        let mut vad = ContinuousVadProcessor::new(sample_rate, 1_200)
            .unwrap()
            .with_adaptive_live_segment_duration_ms(first_segment_ms, subsequent_segment_ms);
        let capture_frame_samples = sample_rate as usize / 10;
        let mut segments = Vec::new();
        let mut fed_samples = 0_usize;
        for frame in samples.chunks(capture_frame_samples) {
            fed_samples += frame.len();
            let available_at_seconds = fed_samples as f64 / sample_rate as f64;
            for segment in vad.process_audio(frame).unwrap() {
                if segment.samples.len() >= 800 {
                    let chunk_id = segments.len() as u64;
                    segments.push(ReplaySegment {
                        chunk: AudioChunk {
                            data: segment.samples,
                            sample_rate: 16_000,
                            timestamp: segment.start_timestamp_ms / 1_000.0,
                            chunk_id,
                            device_type: RecordingDeviceType::Microphone,
                            device_epoch: 0,
                            capture_qpc_ns: None,
                        },
                        available_at_seconds,
                    });
                }
            }
        }
        let available_at_seconds = samples.len() as f64 / sample_rate as f64;
        for segment in vad.flush().unwrap() {
            if segment.samples.len() >= 800 {
                let chunk_id = segments.len() as u64;
                segments.push(ReplaySegment {
                    chunk: AudioChunk {
                        data: segment.samples,
                        sample_rate: 16_000,
                        timestamp: segment.start_timestamp_ms / 1_000.0,
                        chunk_id,
                        device_type: RecordingDeviceType::Microphone,
                        device_epoch: 0,
                        capture_qpc_ns: None,
                    },
                    available_at_seconds,
                });
            }
        }
        segments
    }

    async fn run_deterministic_replay(
        engine: &WhisperEngine,
        source_segments: &[ReplaySegment],
        context: Option<&RecognitionContext>,
        prompt: Option<String>,
        label: &str,
    ) -> serde_json::Value {
        let mut next_index = 0_usize;
        let mut worker_free_at_seconds = 0_f64;
        let mut represented_source_segments = 0_usize;
        let mut inference_durations = Vec::new();
        let mut whisper_durations = Vec::new();
        let mut normalization_durations = Vec::new();
        let mut calls = Vec::new();
        let mut events = Vec::new();

        while next_index < source_segments.len() {
            let segment = &source_segments[next_index];
            let mut chunk = segment.chunk.clone();
            let mut represented = 1_usize;
            let worker_started_at = worker_free_at_seconds.max(segment.available_at_seconds);
            next_index += 1;

            while next_index < source_segments.len()
                && source_segments[next_index].available_at_seconds <= worker_started_at
            {
                let candidate = source_segments[next_index].chunk.clone();
                match try_merge_transcription_chunk(&mut chunk, candidate) {
                    Ok(_) => {
                        represented += 1;
                        next_index += 1;
                    }
                    Err(_) => break,
                }
            }

            let audio_start_time = chunk.timestamp;
            let audio_duration = chunk.data.len() as f64 / chunk.sample_rate as f64;
            let started = Instant::now();
            let (raw_text, confidence, is_partial) = engine
                .transcribe_audio_with_confidence_and_prompt(
                    chunk.data,
                    Some("zh".to_owned()),
                    prompt.clone(),
                )
                .await
                .unwrap();
            let whisper_seconds = started.elapsed().as_secs_f64();
            let normalization_started = Instant::now();
            let text = context.map_or(raw_text.clone(), |value| {
                value.normalize_transcript(&raw_text)
            });
            let normalization_seconds = normalization_started.elapsed().as_secs_f64();
            let inference_seconds = whisper_seconds + normalization_seconds;
            let worker_finished_at = worker_started_at + inference_seconds;
            worker_free_at_seconds = worker_finished_at;
            represented_source_segments += represented;
            inference_durations.push(inference_seconds);
            whisper_durations.push(whisper_seconds);
            normalization_durations.push(normalization_seconds);
            let valid = !text.trim().is_empty() && confidence >= 0.3;
            calls.push(json!({
                "audio_start_time": audio_start_time,
                "audio_duration_seconds": audio_duration,
                "represented_source_segments": represented,
                "worker_started_at_seconds": worker_started_at,
                "worker_finished_at_seconds": worker_finished_at,
                "inference_seconds": inference_seconds,
                "whisper_seconds": whisper_seconds,
                "normalization_seconds": normalization_seconds,
                "confidence": confidence,
                "is_partial": is_partial,
                "valid_output": valid,
                "text_chars": text.chars().count(),
                "text_sha256": format!("{:x}", Sha256::digest(text.as_bytes())),
            }));
            if valid {
                events.push(json!({
                    "output_at_seconds": worker_finished_at,
                    "audio_start_time": audio_start_time,
                    "audio_end_time": audio_start_time + audio_duration,
                    "text_chars": text.chars().count(),
                    "text_sha256": format!("{:x}", Sha256::digest(text.as_bytes())),
                }));
            }
            if calls.len() % 10 == 0 || next_index == source_segments.len() {
                eprintln!(
                    "MC-R03 {label}: {} calls, {}/{} source segments represented",
                    calls.len(),
                    represented_source_segments,
                    source_segments.len()
                );
            }
        }

        let first_speech_start = source_segments.first().unwrap().chunk.timestamp;
        let first_valid_latency = events
            .first()
            .and_then(|event| event["output_at_seconds"].as_f64())
            .map(|output| output - first_speech_start);
        let mut max_continuous_output_gap = 0_f64;
        for pair in events.windows(2) {
            let previous_audio_end = pair[0]["audio_end_time"].as_f64().unwrap();
            let current_audio_start = pair[1]["audio_start_time"].as_f64().unwrap();
            if current_audio_start - previous_audio_end <= MAX_WHISPER_BATCH_GAP_SECONDS {
                let gap = pair[1]["output_at_seconds"].as_f64().unwrap()
                    - pair[0]["output_at_seconds"].as_f64().unwrap();
                max_continuous_output_gap = max_continuous_output_gap.max(gap);
            }
        }
        let p95 = percentile_95(&inference_durations);
        let p95_whisper = percentile_95(&whisper_durations);
        let p95_normalization = percentile_95(&normalization_durations);
        json!({
            "source_segment_count": source_segments.len(),
            "represented_source_segment_count": represented_source_segments,
            "transcription_call_count": calls.len(),
            "valid_output_count": events.len(),
            "first_valid_text_latency_seconds": first_valid_latency,
            "max_continuous_output_gap_seconds": max_continuous_output_gap,
            "p95_inference_seconds": p95,
            "p95_whisper_seconds": p95_whisper,
            "p95_normalization_seconds": p95_normalization,
            "simulated_worker_finish_seconds": worker_free_at_seconds,
            "calls": calls,
            "events": events,
        })
    }

    /// Heavy, reproducible ten-minute real-audio gate. It uses the production
    /// VAD segmentation and serial-worker coalescing rules while reconstructing
    /// wall-clock playback from measured Whisper inference durations.
    #[tokio::test]
    #[ignore = "requires MEETILY_MC_R03_* real-audio acceptance inputs"]
    async fn mc_r03_ten_minute_real_audio_latency_gate() {
        let manifest_path = PathBuf::from(
            std::env::var("MEETILY_MC_R03_MANIFEST").expect("MEETILY_MC_R03_MANIFEST is required"),
        );
        let template_path = PathBuf::from(
            std::env::var("MEETILY_MC_R03_TEMPLATE").expect("MEETILY_MC_R03_TEMPLATE is required"),
        );
        let models_dir = PathBuf::from(
            std::env::var("MEETILY_MC_R03_MODELS_DIR")
                .expect("MEETILY_MC_R03_MODELS_DIR is required"),
        );
        let output_path = PathBuf::from(
            std::env::var("MEETILY_MC_R03_REPORT").expect("MEETILY_MC_R03_REPORT is required"),
        );
        let model_name = std::env::var("MEETILY_MC_R03_MODEL")
            .unwrap_or_else(|_| "large-v3-turbo-q5_0".to_owned());
        let mode = std::env::var("MEETILY_MC_R03_MODE")
            .unwrap_or_else(|_| "experimental-full-prompt".to_owned());

        let manifest: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&manifest_path).unwrap()).unwrap();
        assert_eq!(manifest["durationSeconds"].as_u64(), Some(600));
        assert_eq!(manifest["sampleRate"].as_u64(), Some(16_000));
        let raw_audio_path = PathBuf::from(manifest["rawAudioPath"].as_str().unwrap());
        let bytes = std::fs::read(&raw_audio_path).unwrap();
        assert_eq!(bytes.len() % 4, 0);
        let samples = bytes
            .chunks_exact(4)
            .map(|chunk| f32::from_le_bytes(chunk.try_into().unwrap()))
            .collect::<Vec<_>>();
        assert_eq!(samples.len(), 600 * 16_000);

        let template: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&template_path).unwrap()).unwrap();
        let context = recognition_context_from_template(&template);
        let prompt = context.whisper_initial_prompt.clone().unwrap();
        let source_segments = prepare_replay_segments(&samples, 16_000);
        assert!(
            !source_segments.is_empty(),
            "VAD returned no real speech segments"
        );

        let engine = WhisperEngine::new_with_models_dir(models_dir).unwrap();
        engine.discover_models().await.unwrap();
        engine.load_model(&model_name).await.unwrap();
        // Exclude one model/graph warm-up call from both measured replays.
        engine
            .transcribe_audio_with_confidence_and_prompt(
                source_segments[0].chunk.data.clone(),
                Some("zh".to_owned()),
                None,
            )
            .await
            .unwrap();

        if mode == "shipping-no-prompt" {
            // The shipping policy deliberately disables Whisper's initial prompt after
            // the experimental prompt path failed this gate. Measure Whisper and the
            // deterministic normalizer separately in the same pass so model order,
            // thermal drift, and power-state changes cannot distort the overhead ratio.
            let shipping = run_deterministic_replay(
                &engine,
                &source_segments,
                Some(&context),
                None,
                "shipping-no-prompt",
            )
            .await;
            let baseline_p95 = shipping["p95_whisper_seconds"].as_f64().unwrap();
            let shipping_p95 = shipping["p95_inference_seconds"].as_f64().unwrap();
            let p95_regression_percent = (shipping_p95 - baseline_p95) / baseline_p95 * 100.0;
            let assertions = json!({
                "zero_source_segment_loss": shipping["source_segment_count"] == shipping["represented_source_segment_count"],
                "has_valid_outputs": shipping["valid_output_count"].as_u64().unwrap_or(0) > 0,
                "first_valid_text_within_8_seconds": shipping["first_valid_text_latency_seconds"].as_f64().is_some_and(|value| value <= 8.0),
                "continuous_output_gap_within_10_seconds": shipping["max_continuous_output_gap_seconds"].as_f64().is_some_and(|value| value <= 10.0),
                "normalization_p95_regression_within_10_percent": p95_regression_percent <= 10.0,
                "live_whisper_prompt_is_disabled": !context.live_prompt_enabled,
            });
            let passed = assertions
                .as_object()
                .unwrap()
                .values()
                .all(|value| value.as_bool() == Some(true));
            let report = json!({
                "stage": "C",
                "gate": "MC-R03 ten-minute real-audio shipping-configuration latency",
                "audited_at": Utc::now(),
                "mode": mode,
                "model": model_name,
                "manifest_path": manifest_path,
                "template_path": template_path,
                "context_id": context.context_id,
                "context_sha256": context.context_sha256,
                "prompt_policy": {
                    "whisper_initial_prompt_sent": false,
                    "deterministic_name_and_term_normalization_enabled": true,
                    "reason": "The full initial-prompt experiment failed MC-R03 latency; the release configuration keeps the prompt disabled and retains deterministic alias normalization.",
                    "failed_experimental_evidence": [
                        "人员与术语预设-阶段C-MC-R03十分钟实时延迟证据-20260826.json",
                        "人员与术语预设-阶段C-MC-R03十分钟实时延迟证据-20260827.json"
                    ]
                },
                "replay_method": manifest["replayMethod"],
                "same_pass_whisper_p95_seconds": baseline_p95,
                "shipping_p95_seconds": shipping_p95,
                "p95_regression_percent": p95_regression_percent,
                "shipping": shipping,
                "assertions": assertions,
                "result": if passed { "PASS" } else { "FAIL" },
            });
            std::fs::create_dir_all(output_path.parent().unwrap()).unwrap();
            std::fs::write(
                &output_path,
                format!("{}\n", serde_json::to_string_pretty(&report).unwrap()),
            )
            .unwrap();
            assert!(passed, "MC-R03 failed; see {}", output_path.display());
            return;
        }

        let baseline =
            run_deterministic_replay(&engine, &source_segments, None, None, "baseline").await;
        let prompted = run_deterministic_replay(
            &engine,
            &source_segments,
            Some(&context),
            Some(prompt.clone()),
            "prompted",
        )
        .await;
        let baseline_p95 = baseline["p95_inference_seconds"].as_f64().unwrap();
        let prompted_p95 = prompted["p95_inference_seconds"].as_f64().unwrap();
        let p95_regression_percent = (prompted_p95 - baseline_p95) / baseline_p95 * 100.0;
        let assertions = json!({
            "baseline_zero_source_segment_loss": baseline["source_segment_count"] == baseline["represented_source_segment_count"],
            "prompted_zero_source_segment_loss": prompted["source_segment_count"] == prompted["represented_source_segment_count"],
            "prompted_has_valid_outputs": prompted["valid_output_count"].as_u64().unwrap_or(0) > 0,
            "first_valid_text_within_8_seconds": prompted["first_valid_text_latency_seconds"].as_f64().is_some_and(|value| value <= 8.0),
            "continuous_output_gap_within_10_seconds": prompted["max_continuous_output_gap_seconds"].as_f64().is_some_and(|value| value <= 10.0),
            "p95_regression_within_10_percent": p95_regression_percent <= 10.0,
        });
        let passed = assertions
            .as_object()
            .unwrap()
            .values()
            .all(|value| value.as_bool() == Some(true));
        let report = json!({
            "stage": "C",
            "gate": "MC-R03 ten-minute real-audio deterministic playback latency",
            "audited_at": Utc::now(),
            "model": model_name,
            "manifest_path": manifest_path,
            "template_path": template_path,
            "context_id": context.context_id,
            "context_sha256": context.context_sha256,
            "prompt_sha256": format!("{:x}", Sha256::digest(prompt.as_bytes())),
            "prompt_chars": prompt.chars().count(),
            "product_live_prompt_enabled_during_test": context.live_prompt_enabled,
            "replay_method": manifest["replayMethod"],
            "baseline": baseline,
            "prompted": prompted,
            "p95_regression_percent": p95_regression_percent,
            "assertions": assertions,
            "result": if passed { "PASS" } else { "FAIL" },
        });
        std::fs::create_dir_all(output_path.parent().unwrap()).unwrap();
        std::fs::write(
            &output_path,
            format!("{}\n", serde_json::to_string_pretty(&report).unwrap()),
        )
        .unwrap();
        assert!(passed, "MC-R03 failed; see {}", output_path.display());
    }
}

/// Transcribe audio chunk using the appropriate provider (Whisper, Parakeet, or trait-based).
/// The final fields expose the engine's real deduplication boundary when it has one.
async fn transcribe_chunk_with_provider<R: Runtime>(
    engine: &TranscriptionEngine,
    chunk: AudioChunk,
    app: &AppHandle<R>,
    recognition_context: Option<&RecognitionContext>,
    source_chunk_ids: &[u64],
    measurement_recorder: Option<&D11MeasurementRecorder>,
    transcription_provider: &str,
    transcription_model: &str,
) -> std::result::Result<
    (String, Option<f32>, bool, String, String, &'static str),
    TranscriptionError,
> {
    // Convert to 16kHz mono for transcription
    let transcription_data = if chunk.sample_rate != 16000 {
        crate::audio::audio_processing::resample_audio(&chunk.data, chunk.sample_rate, 16000)
    } else {
        chunk.data
    };

    // Skip VAD processing here since the pipeline already extracted speech using VAD
    let speech_samples = transcription_data;

    // Check for empty samples - improved error handling
    if speech_samples.is_empty() {
        warn!(
            "Audio chunk {} is empty, skipping transcription",
            chunk.chunk_id
        );
        return Err(TranscriptionError::AudioTooShort {
            samples: 0,
            minimum: 1600, // 100ms at 16kHz
        });
    }

    // Calculate energy for logging/monitoring only
    let energy: f32 =
        speech_samples.iter().map(|&x| x * x).sum::<f32>() / speech_samples.len() as f32;
    info!(
        "Processing speech audio chunk {} with {} samples (energy: {:.6})",
        chunk.chunk_id,
        speech_samples.len(),
        energy
    );

    // Resolve the language at the same provider dispatch boundary as before.
    // Parakeet has no language input, which is recorded explicitly as null.
    let actual_language = match engine {
        TranscriptionEngine::Parakeet(_) => None,
        TranscriptionEngine::Whisper(_) | TranscriptionEngine::Provider(_) | TranscriptionEngine::SenseVoice(_) => {
            crate::get_language_preference_internal()
        }
    };
    let language_applied_to_provider = !matches!(engine, TranscriptionEngine::Parakeet(_));
    if let Some(recorder) = measurement_recorder {
        recorder.record_inference_started(
            source_chunk_ids,
            &speech_samples,
            16_000,
            actual_language.as_deref(),
            language_applied_to_provider,
            recognition_context.map(|context| context.context_id.as_str()),
            recognition_context.map(|context| context.context_sha256.as_str()),
            transcription_provider,
            transcription_model,
        );
    }

    // Transcribe using the appropriate engine (with improved error handling)
    match engine {
        TranscriptionEngine::SenseVoice(_) => Err(TranscriptionError::EngineFailed("SenseVoice live session is unavailable".to_owned())),
        TranscriptionEngine::Whisper(whisper_engine) => {
            match whisper_engine
                .transcribe_audio_with_confidence_and_prompt_trace(
                    speech_samples,
                    actual_language,
                    recognition_context.and_then(RecognitionContext::active_whisper_prompt),
                )
                .await
            {
                Ok((text_before_dedup, text_after_dedup, confidence, is_partial)) => {
                    let cleaned_text = text_after_dedup.trim().to_string();
                    if cleaned_text.is_empty() {
                        return Ok((
                            String::new(),
                            Some(confidence),
                            is_partial,
                            text_before_dedup,
                            cleaned_text,
                            "captured_whisper_engine",
                        ));
                    }

                    info!(
                        "Whisper transcription complete for chunk {}: '{}' (confidence: {:.2}, partial: {})",
                        chunk.chunk_id, cleaned_text, confidence, is_partial
                    );

                    Ok((
                        cleaned_text.clone(),
                        Some(confidence),
                        is_partial,
                        text_before_dedup,
                        cleaned_text,
                        "captured_whisper_engine",
                    ))
                }
                Err(e) => {
                    error!(
                        "Whisper transcription failed for chunk {}: {}",
                        chunk.chunk_id, e
                    );

                    let transcription_error = TranscriptionError::EngineFailed(e.to_string());
                    let _ = app.emit(
                        "transcription-error",
                        &serde_json::json!({
                            "error": transcription_error.to_string(),
                            "userMessage": format!("Transcription failed: {}", transcription_error),
                            "actionable": false
                        }),
                    );

                    Err(transcription_error)
                }
            }
        }
        TranscriptionEngine::Parakeet(parakeet_engine) => {
            let language = crate::get_language_preference_internal();
            if let Err(transcription_error) =
                super::parakeet_provider::validate_parakeet_language(language.as_deref())
            {
                error!(
                    "Parakeet rejected incompatible language preference for chunk {}: {}",
                    chunk.chunk_id, transcription_error
                );
                let _ = app.emit(
                    "transcription-error",
                    &serde_json::json!({
                        "error": transcription_error.to_string(),
                        "userMessage": format!(
                            "Transcription failed: {}. Select automatic detection or use Whisper for Chinese/manual language selection.",
                            transcription_error
                        ),
                        "actionable": true
                    }),
                );
                return Err(transcription_error);
            }

            match parakeet_engine.transcribe_audio(speech_samples).await {
                Ok(text) => {
                    let cleaned_text = text.trim().to_string();
                    if cleaned_text.is_empty() {
                        return Ok((
                            String::new(),
                            None,
                            false,
                            cleaned_text.clone(),
                            cleaned_text,
                            "not_applicable_parakeet",
                        ));
                    }

                    info!(
                        "Parakeet transcription complete for chunk {}: '{}'",
                        chunk.chunk_id, cleaned_text
                    );

                    // Parakeet doesn't provide confidence or partial results
                    Ok((
                        cleaned_text.clone(),
                        None,
                        false,
                        cleaned_text.clone(),
                        cleaned_text,
                        "not_applicable_parakeet",
                    ))
                }
                Err(e) => {
                    error!(
                        "Parakeet transcription failed for chunk {}: {}",
                        chunk.chunk_id, e
                    );

                    let transcription_error = TranscriptionError::EngineFailed(e.to_string());
                    let _ = app.emit(
                        "transcription-error",
                        &serde_json::json!({
                            "error": transcription_error.to_string(),
                            "userMessage": format!("Transcription failed: {}", transcription_error),
                            "actionable": false
                        }),
                    );

                    Err(transcription_error)
                }
            }
        }
        TranscriptionEngine::Provider(provider) => {
            // NEW: Trait-based provider (clean, unified interface)
            match provider.transcribe(speech_samples, actual_language).await {
                Ok(result) => {
                    // MAIN-046: provider output goes through the same canonical
                    // term table as the Whisper path (substitution only).
                    let (corrected, applied) =
                        crate::transcript_term_correction::correct_terms(result.text.trim());
                    if !applied.is_empty() {
                        info!("term correction applied ({}): {:?}", provider.provider_name(), applied);
                    }
                    let cleaned_text = corrected.trim().to_string();
                    if cleaned_text.is_empty() {
                        return Ok((
                            String::new(),
                            result.confidence,
                            result.is_partial,
                            cleaned_text.clone(),
                            cleaned_text,
                            "provider_does_not_expose_dedup_boundary",
                        ));
                    }

                    let confidence_str = match result.confidence {
                        Some(c) => format!("confidence: {:.2}", c),
                        None => "no confidence".to_string(),
                    };

                    info!(
                        "{} transcription complete for chunk {}: '{}' ({}, partial: {})",
                        provider.provider_name(),
                        chunk.chunk_id,
                        cleaned_text,
                        confidence_str,
                        result.is_partial
                    );

                    Ok((
                        cleaned_text.clone(),
                        result.confidence,
                        result.is_partial,
                        cleaned_text.clone(),
                        cleaned_text,
                        "provider_does_not_expose_dedup_boundary",
                    ))
                }
                Err(e) => {
                    error!(
                        "{} transcription failed for chunk {}: {}",
                        provider.provider_name(),
                        chunk.chunk_id,
                        e
                    );

                    let _ = app.emit(
                        "transcription-error",
                        &serde_json::json!({
                            "error": e.to_string(),
                            "userMessage": format!("Transcription failed: {}", e),
                            "actionable": false
                        }),
                    );

                    Err(e)
                }
            }
        }
    }
}

/// Format current timestamp (wall-clock time)
fn format_current_timestamp() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();

    let hours = (now.as_secs() / 3600) % 24;
    let minutes = (now.as_secs() / 60) % 60;
    let seconds = now.as_secs() % 60;

    format!("{:02}:{:02}:{:02}", hours, minutes, seconds)
}

/// Format recording-relative time as [MM:SS]
#[allow(dead_code)]
fn format_recording_time(seconds: f64) -> String {
    let total_seconds = seconds.floor() as u64;
    let minutes = total_seconds / 60;
    let secs = total_seconds % 60;

    format!("[{:02}:{:02}]", minutes, secs)
}

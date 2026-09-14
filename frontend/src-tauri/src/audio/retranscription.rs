// Retranscription module - allows re-processing stored audio with different settings

use super::common::{create_transcript_segments, split_segment_at_silence, write_transcripts_json};
use super::constants::AUDIO_EXTENSIONS;
use crate::audio::decoder::decode_audio_file;
use crate::audio::transcription::provider::TranscriptionProvider;
use crate::audio::vad::get_speech_chunks_with_progress;
use crate::config::{DEFAULT_PARAKEET_MODEL, DEFAULT_WHISPER_MODEL};
use crate::meeting_context::{load_recognition_context, RecognitionContextSelection};
use crate::parakeet_engine::ParakeetEngine;
use crate::state::AppState;
use crate::storage::operation_lock::{begin_storage_operation, StorageOperationKind};
use crate::whisper_engine::WhisperEngine;
use anyhow::{anyhow, Result};
use log::{debug, error, info, warn};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tauri::{AppHandle, Emitter, Manager, Runtime};

/// Global flag to track if retranscription is in progress
static RETRANSCRIPTION_IN_PROGRESS: AtomicBool = AtomicBool::new(false);

#[derive(Debug, Clone, Serialize)]
pub struct RetranscriptionStatus {
    pub state: &'static str,
    pub result: Option<RetranscriptionResult>,
}
static RETRANSCRIPTION_RESULTS: once_cell::sync::Lazy<std::sync::Mutex<std::collections::HashMap<String, RetranscriptionStatus>>> =
    once_cell::sync::Lazy::new(|| std::sync::Mutex::new(std::collections::HashMap::new()));

#[tauri::command]
pub fn get_retranscription_status_command(meeting_id: String) -> Result<RetranscriptionStatus, String> {
    let results = RETRANSCRIPTION_RESULTS.lock().map_err(|_| "retranscription_status_unavailable")?;
    Ok(results.get(&meeting_id).cloned().unwrap_or(RetranscriptionStatus { state: "unknown", result: None }))
}

/// Global flag to signal cancellation
static RETRANSCRIPTION_CANCELLED: AtomicBool = AtomicBool::new(false);

/// RAII guard for RETRANSCRIPTION_IN_PROGRESS flag
/// Ensures flag is cleared even if retranscription panics or returns early
struct RetranscriptionGuard;

impl RetranscriptionGuard {
    /// Create guard and set flag atomically
    fn acquire() -> Result<Self, String> {
        if RETRANSCRIPTION_IN_PROGRESS
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            return Err("Retranscription already in progress".to_string());
        }
        Ok(RetranscriptionGuard)
    }
}

impl Drop for RetranscriptionGuard {
    fn drop(&mut self) {
        RETRANSCRIPTION_IN_PROGRESS.store(false, Ordering::SeqCst);
    }
}

/// VAD redemption time in milliseconds - bridges natural pauses in speech
/// Batch processing needs longer redemption (2000ms) than live pipeline (400ms)
/// because the entire file is processed at once by VAD, and 400ms fragments
/// speech at every natural sentence/topic pause (500ms-2s)
const VAD_REDEMPTION_TIME_MS: u32 = 2000;

/// Progress update emitted during retranscription
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetranscriptionProgress {
    pub meeting_id: String,
    pub stage: String, // "decoding", "transcribing", "saving"
    pub progress_percentage: u32,
    pub message: String,
}

/// Result of retranscription
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetranscriptionResult {
    pub file_sync_pending: bool,
    pub meeting_id: String,
    pub segments_count: usize,
    pub duration_seconds: f64,
    pub language: Option<String>,
    pub recognition_context_id: Option<String>,
    pub recognition_context_sha256: Option<String>,
    pub recognition_prompt_status: String,
    /// 覆盖前备份的原转写文件名（会议文件夹内）；原本没有转写时为 None
    pub backup_file: Option<String>,
}

/// 覆盖前读取的现有转写行（P0-5：重新转写前备份用）
#[derive(Debug, FromRow)]
struct ExistingTranscriptRow {
    id: String,
    transcript: String,
    timestamp: String,
    audio_start_time: Option<f64>,
    audio_end_time: Option<f64>,
    duration: Option<f64>,
    /// 以下四列目前基本为空，但备份时一并带上，保证"恢复"是无损的
    speaker: Option<String>,
    summary: Option<String>,
    action_items: Option<String>,
    key_points: Option<String>,
}

/// Error during retranscription
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetranscriptionError {
    pub meeting_id: String,
    pub error: String,
}

fn normalize_requested_language(language: Option<String>) -> Option<String> {
    language.and_then(|value| match value.as_str() {
        "" | "auto" | "auto-translate" => None,
        _ => Some(value),
    })
}

/// 去掉相邻分片之间重复的开头。
///
/// 长句被迫从中间切开时，后面的分片会多带 0.8 秒前文音频（见
/// `split_segment_at_silence` 的 `lead_in_samples`），于是同一句话会在两个分片里
/// 各出现一次。这里按"前一片的非标点尾部 == 后一片的非标点开头"把重复部分删掉。
fn dedupe_leading_overlap(previous: &str, current: &str) -> String {
    const MAX_OVERLAP_CHARS: usize = 16;
    const MIN_OVERLAP_CHARS: usize = 4;

    let previous_significant: Vec<char> =
        previous.chars().filter(|c| c.is_alphanumeric()).collect();
    let current_chars: Vec<char> = current.chars().collect();
    let current_significant: Vec<usize> = current_chars
        .iter()
        .enumerate()
        .filter(|(_, c)| c.is_alphanumeric())
        .map(|(index, _)| index)
        .collect();

    let max_check = MAX_OVERLAP_CHARS
        .min(previous_significant.len())
        .min(current_significant.len());
    if max_check < MIN_OVERLAP_CHARS {
        return current.to_string();
    }

    for length in (MIN_OVERLAP_CHARS..=max_check).rev() {
        let tail = &previous_significant[previous_significant.len() - length..];
        let head: Vec<char> = current_significant[..length]
            .iter()
            .map(|index| current_chars[*index])
            .collect();
        if tail == head.as_slice() {
            let cut_index = current_significant[length - 1];
            return current_chars[cut_index + 1..].iter().collect();
        }
    }

    current.to_string()
}

/// Check if retranscription is currently in progress
pub fn is_retranscription_in_progress() -> bool {
    RETRANSCRIPTION_IN_PROGRESS.load(Ordering::SeqCst)
}

/// Cancel ongoing retranscription
pub fn cancel_retranscription() {
    RETRANSCRIPTION_CANCELLED.store(true, Ordering::SeqCst);
}

/// Start retranscription of a meeting's audio
pub async fn start_retranscription<R: Runtime>(
    app: AppHandle<R>,
    meeting_id: String,
    meeting_folder_path: String,
    language: Option<String>,
    model: Option<String>,
    provider: Option<String>,
) -> Result<RetranscriptionResult> {
    let _storage_operation_guard = begin_storage_operation(StorageOperationKind::Retranscription)
        .map_err(|error| anyhow!(error.to_string()))?;
    // Acquire guard - ensures flag is cleared even on panic/early return
    let _guard = RetranscriptionGuard::acquire().map_err(|e| anyhow!(e))?;

    // Reset cancellation flag
    RETRANSCRIPTION_CANCELLED.store(false, Ordering::SeqCst);

    let use_parakeet = provider.as_deref() == Some("parakeet");
    // SenseVoice 模型由实时引擎共享持有，批量跑完不需要（也不应该）卸载
    let keep_sensevoice_loaded = provider.as_deref() == Some("sensevoice");
    let result = run_retranscription(
        app.clone(),
        meeting_id.clone(),
        meeting_folder_path,
        language,
        model,
        provider,
        RecognitionContextSelection::Current,
    )
    .await;

    // Unload the engine after the batch job (success, failure, or cancellation)
    if !keep_sensevoice_loaded {
        super::common::unload_engine_after_batch(use_parakeet).await;
    }

    // Guard will automatically clear flag on drop
    // No need for manual: RETRANSCRIPTION_IN_PROGRESS.store(false, Ordering::SeqCst);

    match &result {
        Ok(res) => {
            let _ = app.emit(
                "retranscription-complete",
                serde_json::json!({
                    "meeting_id": res.meeting_id,
                    "segments_count": res.segments_count,
                    "duration_seconds": res.duration_seconds,
                    "language": res.language,
                    "recognition_context_id": res.recognition_context_id,
                    "recognition_context_sha256": res.recognition_context_sha256,
                    "recognition_prompt_status": res.recognition_prompt_status,
                    "backup_file": res.backup_file,
                    "file_sync_pending": res.file_sync_pending
                }),
            );
        }
        Err(e) => {
            let _ = app.emit(
                "retranscription-error",
                RetranscriptionError {
                    meeting_id: meeting_id.clone(),
                    error: e.to_string(),
                },
            );
        }
    }

    result
}

/// Find audio file in meeting folder
/// Tries common names first, then scans for any file with an audio extension
fn find_audio_file(folder: &Path) -> Result<PathBuf> {
    let candidates = [
        "audio.mp4",
        "audio.m4a",
        "audio.wav",
        "audio.mp3",
        "audio.flac",
        "audio.ogg",
        "recording.mp4",
        "audio.mkv",
        "audio.webm",
        "audio.wma",
    ];

    for name in candidates {
        let path = folder.join(name);
        if path.exists() {
            return Ok(path);
        }
    }

    // Fallback: scan folder for any file with an audio extension
    if let Ok(entries) = std::fs::read_dir(folder) {
        for entry in entries.flatten() {
            let path = entry.path();
            if let Some(ext) = path.extension() {
                let ext = ext.to_string_lossy().to_lowercase();
                if AUDIO_EXTENSIONS.contains(&ext.as_str()) {
                    return Ok(path);
                }
            }
        }
    }

    Err(anyhow!("No audio file found in: {}", folder.display()))
}

/// Internal function to run retranscription
async fn run_retranscription<R: Runtime>(
    app: AppHandle<R>,
    meeting_id: String,
    meeting_folder_path: String,
    language: Option<String>,
    model: Option<String>,
    provider: Option<String>,
    context_selection: RecognitionContextSelection,
) -> Result<RetranscriptionResult> {
    let folder_path = PathBuf::from(&meeting_folder_path);
    let audio_path = find_audio_file(&folder_path)?;
    let recognition_context =
        load_recognition_context(&folder_path, context_selection).map_err(anyhow::Error::msg)?;
    if let Some(context) = recognition_context.as_ref() {
        info!(
            "Manual retranscription recognition context: {:?}",
            context.diagnostics()
        );
    }

    // Determine which provider to use (default to whisper)
    let use_parakeet = provider.as_deref() == Some("parakeet");
    let use_sensevoice = provider.as_deref() == Some("sensevoice");
    if use_parakeet {
        super::transcription::parakeet_provider::validate_parakeet_language(language.as_deref())
            .map_err(|error| anyhow!(error.to_string()))?;
    }
    if use_sensevoice {
        super::transcription::sensevoice_provider::validate_sensevoice_language(language.as_deref())
            .map_err(|error| anyhow!(error.to_string()))?;
    }
    let recognition_context_id = recognition_context
        .as_ref()
        .map(|context| context.context_id.clone());
    let recognition_context_sha256 = recognition_context
        .as_ref()
        .map(|context| context.context_sha256.clone());
    let recognition_prompt_status = if recognition_context.is_none() {
        "not_configured"
    } else if use_parakeet || use_sensevoice {
        "prompt_not_supported"
    } else {
        "disabled_pending_safety_gate"
    }
    .to_owned();

    info!(
        "Starting retranscription for meeting {} with language {:?}, model {:?}, provider {:?}",
        meeting_id, language, model, provider
    );

    // Emit progress: decoding
    emit_progress(&app, &meeting_id, "decoding", 5, "Decoding audio file...");

    // Check for cancellation
    if RETRANSCRIPTION_CANCELLED.load(Ordering::SeqCst) {
        return Err(anyhow!("Retranscription cancelled"));
    }

    // Decode the audio file (CPU-intensive, run in blocking task)
    let path_for_decode = audio_path.clone();
    let decoded = tokio::task::spawn_blocking(move || decode_audio_file(&path_for_decode))
        .await
        .map_err(|e| anyhow!("Decode task panicked: {}", e))??;
    let duration_seconds = decoded.duration_seconds;

    info!(
        "Decoded audio: {:.2}s, {}Hz, {} channels",
        duration_seconds, decoded.sample_rate, decoded.channels
    );

    emit_progress(
        &app,
        &meeting_id,
        "decoding",
        15,
        "Converting audio format...",
    );

    // Check for cancellation
    if RETRANSCRIPTION_CANCELLED.load(Ordering::SeqCst) {
        return Err(anyhow!("Retranscription cancelled"));
    }

    // Convert to 16kHz mono format (CPU-intensive, run in blocking task)
    let audio_samples = tokio::task::spawn_blocking(move || decoded.to_whisper_format())
        .await
        .map_err(|e| anyhow!("Resample task panicked: {}", e))?;
    info!(
        "Converted to 16kHz mono format: {} samples",
        audio_samples.len()
    );

    emit_progress(&app, &meeting_id, "vad", 20, "Detecting speech segments...");

    // Check for cancellation
    if RETRANSCRIPTION_CANCELLED.load(Ordering::SeqCst) {
        return Err(anyhow!("Retranscription cancelled"));
    }

    // Use VAD to find natural speech boundaries (same approach as live transcription)
    // IMPORTANT: Run VAD in a blocking task to avoid blocking the async runtime
    // For large files (35+ minutes), VAD processing can take several minutes
    let app_for_vad = app.clone();
    let meeting_id_for_vad = meeting_id.clone();

    let speech_segments = tokio::task::spawn_blocking(move || {
        get_speech_chunks_with_progress(
            &audio_samples,
            VAD_REDEMPTION_TIME_MS,
            |vad_progress, segments_found| {
                // Map VAD progress (0-100) to overall progress (20-25)
                let overall_progress = 20 + (vad_progress as f32 * 0.05) as u32;
                emit_progress(
                    &app_for_vad,
                    &meeting_id_for_vad,
                    "vad",
                    overall_progress,
                    &format!(
                        "Detecting speech segments... {}% ({} found)",
                        vad_progress, segments_found
                    ),
                );

                // Return false to cancel if cancellation requested
                !RETRANSCRIPTION_CANCELLED.load(Ordering::SeqCst)
            },
        )
    })
    .await
    .map_err(|e| anyhow!("VAD task panicked: {}", e))?
    .map_err(|e| anyhow!("VAD processing failed: {}", e))?;

    let total_segments = speech_segments.len();
    info!(
        "VAD detected {} speech segments (redemption_time={}ms)",
        total_segments, VAD_REDEMPTION_TIME_MS
    );

    // Diagnostic: log segment duration distribution
    if !speech_segments.is_empty() {
        let durations_ms: Vec<f64> = speech_segments
            .iter()
            .map(|s| s.end_timestamp_ms - s.start_timestamp_ms)
            .collect();
        let total_speech_ms: f64 = durations_ms.iter().sum();
        let avg_duration = total_speech_ms / durations_ms.len() as f64;
        let min_duration = durations_ms.iter().cloned().fold(f64::INFINITY, f64::min);
        let max_duration = durations_ms
            .iter()
            .cloned()
            .fold(f64::NEG_INFINITY, f64::max);
        info!(
            "VAD segment stats: avg={:.0}ms, min={:.0}ms, max={:.0}ms, total_speech={:.1}s/{:.1}s ({:.0}%)",
            avg_duration, min_duration, max_duration,
            total_speech_ms / 1000.0, duration_seconds,
            (total_speech_ms / 1000.0 / duration_seconds) * 100.0
        );
        // Log first 10 segments for detailed inspection
        for (i, seg) in speech_segments.iter().take(10).enumerate() {
            let dur = seg.end_timestamp_ms - seg.start_timestamp_ms;
            debug!(
                "  Segment {}: {:.0}ms-{:.0}ms ({:.0}ms, {} samples)",
                i,
                seg.start_timestamp_ms,
                seg.end_timestamp_ms,
                dur,
                seg.samples.len()
            );
        }
        if total_segments > 10 {
            debug!("  ... and {} more segments", total_segments - 10);
        }
    }

    if total_segments == 0 {
        warn!("No speech detected in audio");
        return Err(anyhow!("No speech detected in audio file"));
    }

    emit_progress(
        &app,
        &meeting_id,
        "transcribing",
        25,
        "Loading transcription engine...",
    );

    // Initialize the appropriate engine once (not per-segment)
    let whisper_engine = if !use_parakeet && !use_sensevoice {
        Some(get_or_init_whisper(&app, model.as_deref()).await?)
    } else {
        None
    };
    let parakeet_engine = if use_parakeet {
        Some(get_or_init_parakeet(&app, model.as_deref()).await?)
    } else {
        None
    };
    let sensevoice_provider = if use_sensevoice {
        crate::sensevoice_engine::commands::sensevoice_init()
            .await
            .map_err(|error| anyhow!("Failed to initialize SenseVoice engine: {}", error))?;
        crate::sensevoice_engine::commands::sensevoice_validate_model_ready()
            .await
            .map_err(|error| anyhow!("SenseVoice model is not ready: {}", error))?;
        let engine = crate::sensevoice_engine::commands::get_engine()
            .ok_or_else(|| anyhow!("SenseVoice engine not initialized"))?;
        info!("🎧 Retranscription is using SenseVoice (sherpa-onnx)");
        Some(
            crate::audio::transcription::sensevoice_provider::SenseVoiceProvider::new(engine),
        )
    } else {
        None
    };

    // Split very long segments at silence boundaries for better transcription quality.
    // Hard cuts at arbitrary sample positions lose words at boundaries. Instead, scan
    // for the lowest-energy window near the target split point and cut there.
    const MAX_SEGMENT_SAMPLES: usize = 25 * 16000; // 25 seconds at 16kHz
    // 被迫在连续语音中间切开时，给后面的分片补 0.8 秒前文当上下文
    const LEAD_IN_SAMPLES: usize = (16000.0 * 0.8) as usize;

    let mut processable_segments: Vec<crate::audio::vad::SpeechSegment> = Vec::new();
    for segment in &speech_segments {
        if segment.samples.len() > MAX_SEGMENT_SAMPLES {
            debug!(
                "Splitting large segment ({:.0}ms, {} samples) at silence boundaries",
                segment.end_timestamp_ms - segment.start_timestamp_ms,
                segment.samples.len()
            );

            let sub_segments =
                split_segment_at_silence(segment, MAX_SEGMENT_SAMPLES, LEAD_IN_SAMPLES);
            debug!("Split into {} sub-segments", sub_segments.len());
            processable_segments.extend(sub_segments);
        } else {
            processable_segments.push(segment.clone());
        }
    }

    let processable_count = processable_segments.len();
    info!(
        "Processing {} segments (after splitting)",
        processable_count
    );

    // Process each speech segment with progress updates. Keep this serial: the
    // Vulkan backend shares one device/model context, and concurrent states were
    // proven by the RC15 runtime test to contend indefinitely instead of reducing
    // wall-clock time.
    let mut all_transcripts: Vec<(String, f64, f64)> = Vec::new(); // (text, start_ms, end_ms)
    let mut total_confidence = 0.0f32;

    for (i, segment) in processable_segments.iter().enumerate() {
        if RETRANSCRIPTION_CANCELLED.load(Ordering::SeqCst) {
            return Err(anyhow!("Retranscription cancelled"));
        }

        let progress = 25 + ((i as f32 / processable_count as f32) * 55.0) as u32;
        let segment_duration_sec = (segment.end_timestamp_ms - segment.start_timestamp_ms) / 1000.0;
        emit_progress(
            &app,
            &meeting_id,
            "transcribing",
            progress,
            &format!(
                "Transcribing segment {} of {} ({:.1}s)...",
                i + 1,
                processable_count,
                segment_duration_sec
            ),
        );

        if segment.samples.len() < 1600 {
            debug!(
                "Skipping short segment {} with {} samples",
                i,
                segment.samples.len()
            );
            continue;
        }

        let (text, confidence) = if use_sensevoice {
            let provider = sensevoice_provider
                .as_ref()
                .ok_or_else(|| anyhow!("SenseVoice provider unavailable"))?;
            let result = provider
                .transcribe(segment.samples.clone(), language.clone())
                .await
                .map_err(|error| {
                    anyhow!("SenseVoice transcription failed on segment {}: {}", i, error)
                })?;
            (result.text, result.confidence.unwrap_or(0.9))
        } else if use_parakeet {
            let engine = parakeet_engine.as_ref().unwrap();
            let text = engine
                .transcribe_audio(segment.samples.clone())
                .await
                .map_err(|e| anyhow!("Parakeet transcription failed on segment {}: {}", i, e))?;
            (text, 0.9)
        } else {
            let engine = whisper_engine.as_ref().unwrap();
            let (text, confidence, _) = engine
                .transcribe_audio_with_confidence(segment.samples.clone(), language.clone())
                .await
                .map_err(|error| {
                    anyhow!("Whisper transcription failed on segment {}: {}", i, error)
                })?;
            (text, confidence)
        };
        // P1-12：离线/重新转写也要过术语纠正层，和实时字幕保持一致
        let (corrected, applied_rules) = crate::transcript_term_correction::correct_terms(&text);
        if !applied_rules.is_empty() {
            debug!(
                "Term correction applied on segment {}: {:?}",
                i, applied_rules
            );
        }
        let text = recognition_context
            .as_ref()
            .map_or(corrected.clone(), |context| context.normalize_transcript(&corrected));
        // 去掉"补前文音频"带来的重复开头（长句被切开时才会发生）
        let text = match all_transcripts.last() {
            Some((previous, _, _)) if !text.trim().is_empty() => {
                let deduped = dedupe_leading_overlap(previous, &text);
                if deduped.chars().count() != text.chars().count() {
                    debug!(
                        "Dropped {} duplicated lead-in characters on segment {}",
                        text.chars().count() - deduped.chars().count(),
                        i
                    );
                }
                deduped
            }
            _ => text,
        };
        if !text.trim().is_empty() {
            all_transcripts.push((text, segment.start_timestamp_ms, segment.end_timestamp_ms));
            total_confidence += confidence;
        }
    }

    let transcribed_count = all_transcripts.len();
    let avg_confidence = if transcribed_count > 0 {
        total_confidence / transcribed_count as f32
    } else {
        0.0
    };

    info!(
        "Transcription complete: {} segments transcribed out of {}, avg confidence: {:.2}",
        transcribed_count, processable_count, avg_confidence
    );

    // Check for cancellation
    if RETRANSCRIPTION_CANCELLED.load(Ordering::SeqCst) {
        return Err(anyhow!("Retranscription cancelled"));
    }

    emit_progress(&app, &meeting_id, "saving", 80, "Saving transcripts...");

    // Create transcript segments with proper timestamps from VAD
    let segments = create_transcript_segments(&all_transcripts);

    // Save to database
    let app_state = app
        .try_state::<AppState>()
        .ok_or_else(|| anyhow!("App state not available"))?;

    // Wrap delete+insert+update in a transaction to prevent data loss
    let pool = app_state.db_manager.pool();

    // P0-5：覆盖前先把现有转写（含人工校正）备份到会议文件夹，
    // 备份写不出去就中止，绝不做“先删后说”的不可逆替换。
    let _write_guard = crate::transcript_file_store::WRITE_LOCK.lock().await;
    crate::transcript_file_store::ensure_ready(pool, &meeting_id).await.map_err(|error| anyhow!(error))?;
    let existing_transcripts = crate::transcript_revision::load_current_segments(pool, &meeting_id).await?;
    let backup_file = crate::transcript_revision::write_rows_backup(
        &folder_path, &meeting_id, &existing_transcripts,
        crate::transcript_revision::BACKUP_FILE_PREFIX, "pre-retranscription-backup",
    )?;

    let mut conn = pool
        .acquire()
        .await
        .map_err(|e| anyhow!("DB error: {}", e))?;
    let mut tx = sqlx::Connection::begin(&mut *conn)
        .await
        .map_err(|e| anyhow!("Failed to start transaction: {}", e))?;

    sqlx::query("DELETE FROM transcripts WHERE meeting_id = ?")
        .bind(&meeting_id)
        .execute(&mut *tx)
        .await
        .map_err(|e| anyhow!("Failed to delete existing transcripts: {}", e))?;

    for segment in &segments {
        sqlx::query(
            "INSERT INTO transcripts (id, meeting_id, transcript, timestamp, audio_start_time, audio_end_time, duration)
             VALUES (?, ?, ?, ?, ?, ?, ?)"
        )
        .bind(&segment.id)
        .bind(&meeting_id)
        .bind(&segment.text)
        .bind(&segment.timestamp)
        .bind(segment.audio_start_time)
        .bind(segment.audio_end_time)
        .bind(segment.duration)
        .execute(&mut *tx)
        .await
        .map_err(|e| anyhow!("Failed to insert transcript: {}", e))?;
    }

    let audio_filename = audio_path.file_name().and_then(|name| name.to_str()).unwrap_or("audio.mp4");
    crate::transcript_file_store::stage(&mut tx, &meeting_id, &folder_path, serde_json::json!({
        "meeting_id": meeting_id, "retranscribed_at": chrono::Utc::now().to_rfc3339(),
        "status": "completed", "duration_seconds": duration_seconds, "audio_file": audio_filename,
        "transcript_file": "transcripts.json", "transcription_provider": provider, "transcription_model": model,
    })).await?;
    tx.commit().await.map_err(|error| anyhow!("Failed to commit transaction: {error}"))?;
    let file_sync_pending = crate::transcript_file_store::finish(pool, &meeting_id).await;

    emit_progress(
        &app,
        &meeting_id,
        "complete",
        100,
        "Retranscription complete",
    );

    Ok(RetranscriptionResult {
        file_sync_pending,
        meeting_id,
        segments_count: segments.len(),
        duration_seconds,
        language,
        recognition_context_id,
        recognition_context_sha256,
        recognition_prompt_status,
        backup_file,
    })
}

/// Emit progress event
fn emit_progress<R: Runtime>(
    app: &AppHandle<R>,
    meeting_id: &str,
    stage: &str,
    progress: u32,
    message: &str,
) {
    let _ = app.emit(
        "retranscription-progress",
        RetranscriptionProgress {
            meeting_id: meeting_id.to_string(),
            stage: stage.to_string(),
            progress_percentage: progress,
            message: message.to_string(),
        },
    );
}

/// Get or initialize the Whisper engine, auto-loading the model if needed
/// If `requested_model` is provided, ensures that specific model is loaded
async fn get_or_init_whisper<R: Runtime>(
    app: &AppHandle<R>,
    requested_model: Option<&str>,
) -> Result<Arc<WhisperEngine>> {
    use crate::whisper_engine::commands::WHISPER_ENGINE;

    let engine = {
        let guard = WHISPER_ENGINE.lock().unwrap_or_else(|e| e.into_inner());
        guard.as_ref().cloned()
    };

    match engine {
        Some(e) => {
            // Determine which model to use
            let target_model = match requested_model {
                Some(model) => model.to_string(),
                None => get_configured_whisper_model(app).await?,
            };

            // Check if the correct model is already loaded
            let current_model = e.get_current_model().await;
            let needs_load = match &current_model {
                Some(loaded) => loaded != &target_model,
                None => true,
            };

            if needs_load {
                info!(
                    "Loading Whisper model '{}' (current: {:?})",
                    target_model, current_model
                );

                // Discover available models first (populates the internal cache)
                info!("Discovering available Whisper models...");
                if let Err(discover_err) = e.discover_models().await {
                    warn!(
                        "Error during model discovery (continuing anyway): {}",
                        discover_err
                    );
                }

                match e.load_model(&target_model).await {
                    Ok(_) => {
                        info!("Whisper model '{}' loaded successfully", target_model);
                        Ok(e)
                    }
                    Err(load_err) => {
                        error!(
                            "Failed to load Whisper model '{}': {}",
                            target_model, load_err
                        );
                        Err(anyhow!(
                            "Failed to load Whisper model '{}': {}",
                            target_model,
                            load_err
                        ))
                    }
                }
            } else {
                info!("Whisper model '{}' already loaded", target_model);
                Ok(e)
            }
        }
        None => Err(anyhow!("Whisper engine not initialized")),
    }
}

/// Get the configured Whisper model name from the database
async fn get_configured_whisper_model<R: Runtime>(app: &AppHandle<R>) -> Result<String> {
    debug!("Getting configured Whisper model from database...");

    let app_state = app.try_state::<AppState>().ok_or_else(|| {
        error!("App state not available");
        anyhow!("App state not available")
    })?;

    debug!("Querying transcript_settings table...");

    // Query the transcript settings from the database - get both provider and model
    let result: Option<(String, String)> =
        sqlx::query_as("SELECT provider, model FROM transcript_settings WHERE id = '1'")
            .fetch_optional(app_state.db_manager.pool())
            .await
            .map_err(|e| {
                error!("Failed to query transcript config: {}", e);
                anyhow!("Failed to query transcript config: {}", e)
            })?;

    match result {
        Some((provider, model)) => {
            info!(
                "Found transcript config: provider={}, model={}",
                provider, model
            );

            // Check if provider is Whisper-based
            if provider == "localWhisper" || provider == "whisper" {
                Ok(model)
            } else {
                error!(
                    "Retranscription requires Whisper provider, but configured provider is: {}",
                    provider
                );
                Err(anyhow!("Retranscription requires Whisper. Current provider '{}' does not support retranscription with language selection.", provider))
            }
        }
        None => {
            // Default to configured Whisper model if no config exists
            warn!(
                "No transcript config found, using default model '{}'",
                DEFAULT_WHISPER_MODEL
            );
            Ok(DEFAULT_WHISPER_MODEL.to_string())
        }
    }
}

/// Get or initialize the Parakeet engine, auto-loading the model if needed
async fn get_or_init_parakeet<R: Runtime>(
    app: &AppHandle<R>,
    requested_model: Option<&str>,
) -> Result<Arc<ParakeetEngine>> {
    use crate::parakeet_engine::commands::PARAKEET_ENGINE;

    let engine = {
        let guard = PARAKEET_ENGINE.lock().unwrap_or_else(|e| e.into_inner());
        guard.as_ref().cloned()
    };

    match engine {
        Some(e) => {
            // Determine which model to use
            let target_model = match requested_model {
                Some(model) => model.to_string(),
                None => get_configured_parakeet_model(app).await?,
            };

            // Check if the correct model is already loaded
            let current_model = e.get_current_model().await;
            let needs_load = match &current_model {
                Some(loaded) => loaded != &target_model,
                None => true,
            };

            if needs_load {
                info!(
                    "Loading Parakeet model '{}' (current: {:?})",
                    target_model, current_model
                );

                // Discover available models first
                info!("Discovering available Parakeet models...");
                if let Err(discover_err) = e.discover_models().await {
                    warn!(
                        "Error during Parakeet model discovery (continuing anyway): {}",
                        discover_err
                    );
                }

                match e.load_model(&target_model).await {
                    Ok(_) => {
                        info!("Parakeet model '{}' loaded successfully", target_model);
                        Ok(e)
                    }
                    Err(load_err) => {
                        error!(
                            "Failed to load Parakeet model '{}': {}",
                            target_model, load_err
                        );
                        Err(anyhow!(
                            "Failed to load Parakeet model '{}': {}",
                            target_model,
                            load_err
                        ))
                    }
                }
            } else {
                info!("Parakeet model '{}' already loaded", target_model);
                Ok(e)
            }
        }
        None => Err(anyhow!("Parakeet engine not initialized")),
    }
}

/// Get the configured Parakeet model name from the database
async fn get_configured_parakeet_model<R: Runtime>(app: &AppHandle<R>) -> Result<String> {
    debug!("Getting configured Parakeet model from database...");

    let app_state = app.try_state::<AppState>().ok_or_else(|| {
        error!("App state not available");
        anyhow!("App state not available")
    })?;

    // Query the transcript settings from the database
    let result: Option<(String, String)> =
        sqlx::query_as("SELECT provider, model FROM transcript_settings WHERE id = '1'")
            .fetch_optional(app_state.db_manager.pool())
            .await
            .map_err(|e| {
                error!("Failed to query transcript config: {}", e);
                anyhow!("Failed to query transcript config: {}", e)
            })?;

    match result {
        Some((provider, model)) => {
            info!(
                "Found transcript config: provider={}, model={}",
                provider, model
            );

            if provider == "parakeet" {
                Ok(model)
            } else {
                // Default to configured Parakeet model
                warn!("Configured provider is not Parakeet, using default model");
                Ok(DEFAULT_PARAKEET_MODEL.to_string())
            }
        }
        None => {
            // Default to configured Parakeet model if no config exists
            warn!("No transcript config found, using default Parakeet model");
            Ok(DEFAULT_PARAKEET_MODEL.to_string())
        }
    }
}

/// Write or update metadata.json for retranscription (preserves existing fields, adds retranscribed_at)
fn write_retranscription_metadata(
    folder: &Path,
    meeting_id: &str,
    duration_seconds: f64,
    audio_filename: &str,
    provider: Option<&str>,
    model: Option<&str>,
) -> Result<()> {
    let metadata_path = folder.join("metadata.json");
    let temp_path = folder.join(".metadata.json.tmp");
    let now = chrono::Utc::now().to_rfc3339();

    // Try to read existing metadata and update it
    let json = if metadata_path.exists() {
        let existing = std::fs::read_to_string(&metadata_path)?;
        let mut value: serde_json::Value = serde_json::from_str(&existing)?;
        if let Some(obj) = value.as_object_mut() {
            obj.insert("retranscribed_at".to_string(), serde_json::json!(now));
            obj.insert("status".to_string(), serde_json::json!("completed"));
            obj.insert(
                "duration_seconds".to_string(),
                serde_json::json!(duration_seconds),
            );
            obj.insert("audio_file".to_string(), serde_json::json!(audio_filename));
            obj.insert(
                "transcript_file".to_string(),
                serde_json::json!("transcripts.json"),
            );
            if let Some(provider) = provider {
                obj.insert(
                    "transcription_provider".to_string(),
                    serde_json::json!(provider),
                );
            }
            if let Some(model) = model {
                obj.insert("transcription_model".to_string(), serde_json::json!(model));
            }
            obj.remove("detected_summary_language");
        }
        value
    } else {
        serde_json::json!({
            "version": "1.0",
            "meeting_id": meeting_id,
            "created_at": now,
            "completed_at": now,
            "retranscribed_at": now,
            "duration_seconds": duration_seconds,
            "audio_file": audio_filename,
            "transcript_file": "transcripts.json",
            "status": "completed",
            "source": "retranscription"
        })
    };

    let json_string = serde_json::to_string_pretty(&json)?;
    std::fs::write(&temp_path, &json_string)?;
    std::fs::rename(&temp_path, &metadata_path)?;

    info!("Wrote metadata.json to {}", metadata_path.display());
    Ok(())
}

// Tauri commands

/// Response when retranscription is started
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetranscriptionStarted {
    pub meeting_id: String,
    pub message: String,
}

// Start retranscription (Beta gated using configContext.betaFeatures)
#[tauri::command]
pub async fn start_retranscription_command<R: Runtime>(
    app: AppHandle<R>,
    meeting_id: String,
    meeting_folder_path: String,
    language: Option<String>,
    model: Option<String>,
    provider: Option<String>,
) -> Result<RetranscriptionStarted, String> {
    // Check if retranscription is already in progress (guard will be acquired in start_retranscription)
    if RETRANSCRIPTION_IN_PROGRESS.load(Ordering::SeqCst) {
        return Err("Retranscription already in progress".to_string());
    }

    if provider.as_deref() == Some("parakeet") {
        super::transcription::parakeet_provider::validate_parakeet_language(language.as_deref())
            .map_err(|error| error.to_string())?;
    }

    {
        let mut results = RETRANSCRIPTION_RESULTS.lock().map_err(|_| "retranscription_status_unavailable")?;
        if results.get(&meeting_id).is_some_and(|status| status.state == "running") {
            return Err("Retranscription already in progress".into());
        }
        if results.len() >= 32 { results.retain(|_, status| status.state == "running"); }
        results.insert(meeting_id.clone(), RetranscriptionStatus { state: "running", result: None });
    }

    // Clone values for the spawned task
    let meeting_id_clone = meeting_id.clone();

    // Spawn the retranscription in a background task
    tauri::async_runtime::spawn(async move {
        let result = start_retranscription(
            app,
            meeting_id_clone.clone(),
            meeting_folder_path,
            language,
            model,
            provider,
        )
        .await;

        if let Ok(mut outcomes) = RETRANSCRIPTION_RESULTS.lock() {
            outcomes.insert(meeting_id_clone, RetranscriptionStatus {
                state: if result.is_ok() { "completed" } else { "failed" },
                result: result.as_ref().ok().cloned(),
            });
        }
        // Errors are already emitted as events in start_retranscription
        // so we just log here for debugging
        if let Err(e) = result {
            error!("Retranscription failed: {}", e);
        }
    });

    Ok(RetranscriptionStarted {
        meeting_id,
        message: "Retranscription started".to_string(),
    })
}

/// Finalize a just-stopped live recording with the existing full-file batch
/// transcription pipeline before the meeting is opened for editing or summary
/// generation.
///
/// The frontend first persists the real-time draft. `run_retranscription` does
/// all decoding and inference before it starts the database transaction, so an
/// inference failure leaves that draft intact. On success the existing
/// delete+insert transaction atomically replaces the draft rows. Unlike manual
/// retranscription, this command intentionally keeps the model warm for the next
/// recording session.
#[tauri::command]
pub async fn finalize_recording_transcript_command<R: Runtime>(
    app: AppHandle<R>,
    meeting_id: String,
    meeting_folder_path: String,
    language: Option<String>,
    model: Option<String>,
    provider: Option<String>,
) -> Result<RetranscriptionResult, String> {
    let _guard = RetranscriptionGuard::acquire()?;
    RETRANSCRIPTION_CANCELLED.store(false, Ordering::SeqCst);

    let normalized_language = normalize_requested_language(language);
    info!(
        "Starting post-recording final transcription for meeting {} (provider={:?}, model={:?}, language={:?})",
        meeting_id, provider, model, normalized_language
    );

    match run_retranscription(
        app.clone(),
        meeting_id.clone(),
        meeting_folder_path,
        normalized_language,
        model,
        provider,
        RecognitionContextSelection::Recording,
    )
    .await
    {
        Ok(result) => {
            let _ = app.emit(
                "recording-final-transcription-complete",
                serde_json::json!({
                    "meeting_id": result.meeting_id,
                    "segments_count": result.segments_count,
                    "duration_seconds": result.duration_seconds,
                    "language": result.language,
                    "recognition_context_id": result.recognition_context_id,
                    "recognition_context_sha256": result.recognition_context_sha256,
                    "recognition_prompt_status": result.recognition_prompt_status,
                }),
            );
            info!(
                "Post-recording final transcription committed for meeting {} with {} segments; model retained",
                meeting_id, result.segments_count
            );
            Ok(result)
        }
        Err(error) => {
            let message = error.to_string();
            let _ = app.emit(
                "recording-final-transcription-error",
                RetranscriptionError {
                    meeting_id: meeting_id.clone(),
                    error: message.clone(),
                },
            );
            error!(
                "Post-recording final transcription failed for meeting {}; real-time draft retained: {}",
                meeting_id, message
            );
            Err(message)
        }
    }
}

#[tauri::command]
pub async fn cancel_retranscription_command() -> Result<(), String> {
    if !is_retranscription_in_progress() {
        return Err("No retranscription in progress".to_string());
    }
    cancel_retranscription();
    Ok(())
}

#[tauri::command]
pub async fn is_retranscription_in_progress_command() -> bool {
    is_retranscription_in_progress()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_transcript_segments_empty() {
        let transcripts: Vec<(String, f64, f64)> = vec![];
        let segments = create_transcript_segments(&transcripts);
        assert!(segments.is_empty());
    }

    #[test]
    fn dedupe_leading_overlap_removes_the_repeated_lead_in() {
        // 后一片带了前文音频，开头重复了前一片的结尾（标点不同也要能识别）
        assert_eq!(
            dedupe_leading_overlap("……普拉提斯", "普拉提斯，他也没有做过电商"),
            "，他也没有做过电商"
        );
        assert_eq!(
            dedupe_leading_overlap(
                "那么比V4 Flash的话是高了将近20分了",
                "高了将近20分了，这个提升是非常大的"
            ),
            "，这个提升是非常大的"
        );
    }

    #[test]
    fn dedupe_leading_overlap_keeps_unrelated_text() {
        assert_eq!(
            dedupe_leading_overlap("我们讨论了供应链", "他也没有做过电商"),
            "他也没有做过电商"
        );
        // 重叠太短（<4 字）不删，避免误伤正常的重复词
        assert_eq!(dedupe_leading_overlap("分数是12", "12分。一毛"), "12分。一毛");
    }

    #[test]
    fn retranscription_metadata_replaces_stale_duration_with_decoded_audio_duration() {
        let directory = tempfile::tempdir().unwrap();
        let metadata_path = directory.path().join("metadata.json");
        std::fs::write(
            &metadata_path,
            r#"{"version":"1.0","duration_seconds":20.25,"audio_file":"old.mp4","custom_field":"preserved"}"#,
        )
        .unwrap();

        write_retranscription_metadata(
            directory.path(),
            "meeting-duration-test",
            67.82,
            "audio.mp4",
            None,
            None,
        )
        .unwrap();

        let stored: serde_json::Value =
            serde_json::from_slice(&std::fs::read(metadata_path).unwrap()).unwrap();
        assert_eq!(stored["duration_seconds"], 67.82);
        assert_eq!(stored["audio_file"], "audio.mp4");
        assert_eq!(stored["custom_field"], "preserved");
        assert_eq!(stored["status"], "completed");
        assert!(stored["retranscribed_at"].is_string());
    }

    #[test]
    fn test_create_transcript_segments_single() {
        let transcripts = vec![
            ("Hello world".to_string(), 0.0, 1500.0), // 0-1.5 seconds
        ];
        let segments = create_transcript_segments(&transcripts);

        assert_eq!(segments.len(), 1);
        assert_eq!(segments[0].text, "Hello world");
        assert_eq!(segments[0].audio_start_time, Some(0.0));
        assert_eq!(segments[0].audio_end_time, Some(1.5));
        assert_eq!(segments[0].duration, Some(1.5));
    }

    #[test]
    fn test_create_transcript_segments_multiple() {
        let transcripts = vec![
            ("First segment".to_string(), 0.0, 2000.0), // 0-2 seconds
            ("Second segment".to_string(), 3000.0, 5000.0), // 3-5 seconds
            ("Third segment".to_string(), 6500.0, 8000.0), // 6.5-8 seconds
        ];
        let segments = create_transcript_segments(&transcripts);

        assert_eq!(segments.len(), 3);

        // First segment
        assert_eq!(segments[0].text, "First segment");
        assert_eq!(segments[0].audio_start_time, Some(0.0));
        assert_eq!(segments[0].audio_end_time, Some(2.0));
        assert_eq!(segments[0].duration, Some(2.0));

        // Second segment
        assert_eq!(segments[1].text, "Second segment");
        assert_eq!(segments[1].audio_start_time, Some(3.0));
        assert_eq!(segments[1].audio_end_time, Some(5.0));
        assert_eq!(segments[1].duration, Some(2.0));

        // Third segment
        assert_eq!(segments[2].text, "Third segment");
        assert_eq!(segments[2].audio_start_time, Some(6.5));
        assert_eq!(segments[2].audio_end_time, Some(8.0));
        assert_eq!(segments[2].duration, Some(1.5));
    }

    #[test]
    fn test_create_transcript_segments_trims_whitespace() {
        let transcripts = vec![("  Hello with spaces  ".to_string(), 0.0, 1000.0)];
        let segments = create_transcript_segments(&transcripts);

        assert_eq!(segments.len(), 1);
        assert_eq!(segments[0].text, "Hello with spaces");
    }

    #[test]
    fn test_create_transcript_segments_generates_unique_ids() {
        let transcripts = vec![
            ("Segment one".to_string(), 0.0, 1000.0),
            ("Segment two".to_string(), 1000.0, 2000.0),
        ];
        let segments = create_transcript_segments(&transcripts);

        assert_eq!(segments.len(), 2);
        assert_ne!(segments[0].id, segments[1].id);
        assert!(segments[0].id.starts_with("transcript-"));
        assert!(segments[1].id.starts_with("transcript-"));
    }

    #[test]
    fn test_cancellation_flag() {
        // Reset flag to known state
        RETRANSCRIPTION_CANCELLED.store(false, Ordering::SeqCst);
        RETRANSCRIPTION_IN_PROGRESS.store(false, Ordering::SeqCst);

        assert!(!is_retranscription_in_progress());

        // Test cancellation
        cancel_retranscription();
        assert!(RETRANSCRIPTION_CANCELLED.load(Ordering::SeqCst));

        // Reset for other tests
        RETRANSCRIPTION_CANCELLED.store(false, Ordering::SeqCst);
    }

    #[test]
    fn test_normalize_requested_language_keeps_explicit_chinese() {
        assert_eq!(
            normalize_requested_language(Some("zh".to_string())),
            Some("zh".to_string())
        );
    }

    #[test]
    fn test_normalize_requested_language_maps_automatic_modes_to_none() {
        assert_eq!(normalize_requested_language(Some("auto".to_string())), None);
        assert_eq!(
            normalize_requested_language(Some("auto-translate".to_string())),
            None
        );
        assert_eq!(normalize_requested_language(Some(String::new())), None);
        assert_eq!(normalize_requested_language(None), None);
    }

    #[test]
    fn test_vad_redemption_time_constant() {
        // Batch processing uses 2000ms to bridge natural pauses in full-file VAD
        assert_eq!(VAD_REDEMPTION_TIME_MS, 2000);
    }

    #[test]
    fn test_find_audio_file_common_candidates() {
        let dir = tempfile::tempdir().unwrap();

        // No audio file → error
        assert!(find_audio_file(dir.path()).is_err());

        // Create audio.mp4 — should be found first
        std::fs::write(dir.path().join("audio.mp4"), b"fake").unwrap();
        let found = find_audio_file(dir.path()).unwrap();
        assert_eq!(found.file_name().unwrap(), "audio.mp4");
    }

    #[test]
    fn test_find_audio_file_non_mp4_extensions() {
        let dir = tempfile::tempdir().unwrap();

        // Create audio.wav (imported as .wav, not .mp4)
        std::fs::write(dir.path().join("audio.wav"), b"fake").unwrap();
        let found = find_audio_file(dir.path()).unwrap();
        assert_eq!(found.file_name().unwrap(), "audio.wav");
    }

    #[test]
    fn test_find_audio_file_fallback_scan() {
        let dir = tempfile::tempdir().unwrap();

        // Create a file with an audio extension but non-standard name
        std::fs::write(dir.path().join("my_recording.flac"), b"fake").unwrap();
        // Also add a non-audio file that should be ignored
        std::fs::write(dir.path().join("notes.txt"), b"text").unwrap();

        let found = find_audio_file(dir.path()).unwrap();
        assert_eq!(found.file_name().unwrap(), "my_recording.flac");
    }

    #[test]
    fn test_find_audio_file_priority_order() {
        let dir = tempfile::tempdir().unwrap();

        // Create both audio.m4a and audio.mp4 — mp4 should win (listed first in candidates)
        std::fs::write(dir.path().join("audio.m4a"), b"fake").unwrap();
        std::fs::write(dir.path().join("audio.mp4"), b"fake").unwrap();
        let found = find_audio_file(dir.path()).unwrap();
        assert_eq!(found.file_name().unwrap(), "audio.mp4");
    }

    #[test]
    fn test_find_audio_file_empty_folder() {
        let dir = tempfile::tempdir().unwrap();
        let result = find_audio_file(dir.path());
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("No audio file found"));
    }

    #[test]
    fn test_find_audio_file_nonexistent_folder() {
        let result = find_audio_file(Path::new("/nonexistent/path/12345"));
        assert!(result.is_err());
    }

    #[test]
    fn test_audio_extensions_constant() {
        // Verify all expected formats are covered
        assert!(AUDIO_EXTENSIONS.contains(&"mp4"));
        assert!(AUDIO_EXTENSIONS.contains(&"m4a"));
        assert!(AUDIO_EXTENSIONS.contains(&"wav"));
        assert!(AUDIO_EXTENSIONS.contains(&"mp3"));
        assert!(AUDIO_EXTENSIONS.contains(&"flac"));
        assert!(AUDIO_EXTENSIONS.contains(&"ogg"));
        assert!(AUDIO_EXTENSIONS.contains(&"aac"));
        // FFmpeg-backed formats
        assert!(AUDIO_EXTENSIONS.contains(&"mkv"));
        assert!(AUDIO_EXTENSIONS.contains(&"webm"));
        assert!(AUDIO_EXTENSIONS.contains(&"wma"));
        // Non-audio formats
        assert!(!AUDIO_EXTENSIONS.contains(&"txt"));
        assert!(!AUDIO_EXTENSIONS.contains(&"pdf"));
    }

    fn transcript_text_evidence(path: &Path) -> (usize, String) {
        use sha2::{Digest, Sha256};

        let document: serde_json::Value =
            serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        let segments = document["segments"].as_array().unwrap();
        let mut digest = Sha256::new();
        for segment in segments {
            digest.update(segment["text"].as_str().unwrap_or_default().as_bytes());
            digest.update([0]);
        }
        (segments.len(), format!("{:x}", digest.finalize()))
    }

    /// Real-audio integration gate for the two production retranscription entry
    /// points. The test works on a temporary copy, never on the user's meeting.
    #[tokio::test]
    #[ignore = "requires MEETILY_MC_I03_* real meeting, audio, and model inputs"]
    async fn mc_i03_real_finalize_and_manual_context_consistency() {
        use crate::database::manager::DatabaseManager;
        use crate::whisper_engine::commands::WHISPER_ENGINE;
        use chrono::Utc;
        use serde_json::json;
        use sha2::{Digest, Sha256};
        use std::sync::Mutex as StdMutex;
        use tauri::Listener;

        let source_metadata = PathBuf::from(
            std::env::var("MEETILY_MC_I03_METADATA").expect("MEETILY_MC_I03_METADATA is required"),
        );
        let source_audio = PathBuf::from(
            std::env::var("MEETILY_MC_I03_AUDIO").expect("MEETILY_MC_I03_AUDIO is required"),
        );
        let models_dir = PathBuf::from(
            std::env::var("MEETILY_MC_I03_MODELS_DIR")
                .expect("MEETILY_MC_I03_MODELS_DIR is required"),
        );
        let output_path = PathBuf::from(
            std::env::var("MEETILY_MC_I03_REPORT").expect("MEETILY_MC_I03_REPORT is required"),
        );
        let model_name = std::env::var("MEETILY_MC_I03_MODEL")
            .unwrap_or_else(|_| "large-v3-turbo-q5_0".to_owned());

        let source_folder = source_metadata.parent().unwrap();
        let source_recording =
            load_recognition_context(source_folder, RecognitionContextSelection::Recording)
                .unwrap()
                .expect("source metadata must contain recording context");
        let source_current =
            load_recognition_context(source_folder, RecognitionContextSelection::Current)
                .unwrap()
                .expect("source metadata must contain current context");
        assert_eq!(
            source_recording.context_id, source_current.context_id,
            "MC-I03 fixture must represent the no-explicit-update case"
        );
        assert_eq!(
            source_recording.context_sha256, source_current.context_sha256,
            "MC-I03 fixture context hashes must match before transcription"
        );

        let temporary = tempfile::tempdir().unwrap();
        let meeting_folder = temporary.path().join("meeting");
        std::fs::create_dir_all(&meeting_folder).unwrap();
        std::fs::copy(&source_metadata, meeting_folder.join("metadata.json")).unwrap();
        std::fs::copy(&source_audio, meeting_folder.join("audio.wav")).unwrap();

        let db_path = temporary.path().join("mc-i03.sqlite");
        let legacy_path = temporary.path().join("mc-i03-legacy.db");
        let db_manager =
            DatabaseManager::new(db_path.to_str().unwrap(), legacy_path.to_str().unwrap())
                .await
                .unwrap();
        let meeting_id = "mc_i03_real_short_meeting".to_owned();
        let now = Utc::now().to_rfc3339();
        sqlx::query("INSERT INTO meetings (id, title, created_at, updated_at) VALUES (?, ?, ?, ?)")
            .bind(&meeting_id)
            .bind("MC-I03 real short meeting")
            .bind(&now)
            .bind(&now)
            .execute(db_manager.pool())
            .await
            .unwrap();

        let app = tauri::test::mock_builder()
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .unwrap();
        app.manage(AppState {
            db_manager: db_manager.clone(),
        });

        let finalize_event = Arc::new(StdMutex::new(None::<serde_json::Value>));
        let finalize_event_sink = finalize_event.clone();
        let _finalize_listener =
            app.handle()
                .listen("recording-final-transcription-complete", move |event| {
                    *finalize_event_sink.lock().unwrap() =
                        serde_json::from_str(event.payload()).ok();
                });
        let manual_event = Arc::new(StdMutex::new(None::<serde_json::Value>));
        let manual_event_sink = manual_event.clone();
        let _manual_listener = app
            .handle()
            .listen("retranscription-complete", move |event| {
                *manual_event_sink.lock().unwrap() = serde_json::from_str(event.payload()).ok();
            });

        let engine = Arc::new(WhisperEngine::new_with_models_dir(models_dir).unwrap());
        engine.discover_models().await.unwrap();
        engine.load_model(&model_name).await.unwrap();
        *WHISPER_ENGINE
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = Some(engine);

        let folder_string = meeting_folder.to_string_lossy().into_owned();
        let finalize = finalize_recording_transcript_command(
            app.handle().clone(),
            meeting_id.clone(),
            folder_string.clone(),
            Some("zh".to_owned()),
            Some(model_name.clone()),
            Some("whisper".to_owned()),
        )
        .await
        .unwrap();
        let finalize_text_evidence =
            transcript_text_evidence(&meeting_folder.join("transcripts.json"));

        let manual = start_retranscription(
            app.handle().clone(),
            meeting_id.clone(),
            folder_string,
            Some("zh".to_owned()),
            Some(model_name.clone()),
            Some("whisper".to_owned()),
        )
        .await
        .unwrap();
        let manual_text_evidence =
            transcript_text_evidence(&meeting_folder.join("transcripts.json"));
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;

        let finalize_event = finalize_event.lock().unwrap().clone().unwrap();
        let manual_event = manual_event.lock().unwrap().clone().unwrap();
        let final_metadata_recording =
            load_recognition_context(&meeting_folder, RecognitionContextSelection::Recording)
                .unwrap()
                .unwrap();
        let final_metadata_current =
            load_recognition_context(&meeting_folder, RecognitionContextSelection::Current)
                .unwrap()
                .unwrap();

        let source_audio_sha256 = format!(
            "{:x}",
            Sha256::digest(std::fs::read(&source_audio).unwrap())
        );
        let source_metadata_sha256 = format!(
            "{:x}",
            Sha256::digest(std::fs::read(&source_metadata).unwrap())
        );
        let assertions = json!({
            "fixture_has_no_explicit_context_update": source_recording.context_id == source_current.context_id
                && source_recording.context_sha256 == source_current.context_sha256,
            "finalize_return_uses_recording_context": finalize.recognition_context_id.as_deref() == Some(source_recording.context_id.as_str())
                && finalize.recognition_context_sha256.as_deref() == Some(source_recording.context_sha256.as_str()),
            "manual_return_uses_current_context": manual.recognition_context_id.as_deref() == Some(source_current.context_id.as_str())
                && manual.recognition_context_sha256.as_deref() == Some(source_current.context_sha256.as_str()),
            "finalize_event_matches_return": finalize_event["recognition_context_id"].as_str() == finalize.recognition_context_id.as_deref()
                && finalize_event["recognition_context_sha256"].as_str() == finalize.recognition_context_sha256.as_deref(),
            "manual_event_matches_return": manual_event["recognition_context_id"].as_str() == manual.recognition_context_id.as_deref()
                && manual_event["recognition_context_sha256"].as_str() == manual.recognition_context_sha256.as_deref(),
            "metadata_context_unchanged": final_metadata_recording.context_id == source_recording.context_id
                && final_metadata_recording.context_sha256 == source_recording.context_sha256
                && final_metadata_current.context_id == source_current.context_id
                && final_metadata_current.context_sha256 == source_current.context_sha256,
            "both_paths_produced_transcript_segments": finalize.segments_count > 0 && manual.segments_count > 0,
            "both_paths_produced_identical_text": finalize_text_evidence == manual_text_evidence,
            "prompt_policy_matches_shipping_configuration": finalize.recognition_prompt_status == "disabled_pending_safety_gate"
                && manual.recognition_prompt_status == "disabled_pending_safety_gate",
        });
        let passed = assertions
            .as_object()
            .unwrap()
            .values()
            .all(|value| value.as_bool() == Some(true));
        let report = json!({
            "stage": "C",
            "gate": "MC-I03 real finalize/manual context consistency",
            "audited_at": Utc::now(),
            "model": model_name,
            "source_metadata_path": source_metadata,
            "source_metadata_sha256": source_metadata_sha256,
            "source_audio_path": source_audio,
            "source_audio_sha256": source_audio_sha256,
            "source_context": {
                "recording_context_id": source_recording.context_id,
                "recording_context_sha256": source_recording.context_sha256,
                "current_context_id": source_current.context_id,
                "current_context_sha256": source_current.context_sha256,
            },
            "finalize": {
                "segments_count": finalize.segments_count,
                "duration_seconds": finalize.duration_seconds,
                "context_id": finalize.recognition_context_id,
                "context_sha256": finalize.recognition_context_sha256,
                "prompt_status": finalize.recognition_prompt_status,
                "transcript_text_segment_count": finalize_text_evidence.0,
                "transcript_text_sha256": finalize_text_evidence.1,
                "event_context_id": finalize_event["recognition_context_id"],
                "event_context_sha256": finalize_event["recognition_context_sha256"],
            },
            "manual": {
                "segments_count": manual.segments_count,
                "duration_seconds": manual.duration_seconds,
                "context_id": manual.recognition_context_id,
                "context_sha256": manual.recognition_context_sha256,
                "prompt_status": manual.recognition_prompt_status,
                "transcript_text_segment_count": manual_text_evidence.0,
                "transcript_text_sha256": manual_text_evidence.1,
                "event_context_id": manual_event["recognition_context_id"],
                "event_context_sha256": manual_event["recognition_context_sha256"],
            },
            "assertions": assertions,
            "result": if passed { "PASS" } else { "FAIL" },
        });
        std::fs::create_dir_all(output_path.parent().unwrap()).unwrap();
        std::fs::write(
            &output_path,
            format!("{}\n", serde_json::to_string_pretty(&report).unwrap()),
        )
        .unwrap();

        *WHISPER_ENGINE
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = None;
        db_manager.cleanup().await.unwrap();
        assert!(passed, "MC-I03 failed; see {}", output_path.display());
    }
}

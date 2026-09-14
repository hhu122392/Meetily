use super::worker::TranscriptUpdate;
use crate::audio::{
    measurement::D11MeasurementRecorder,
    recording_saver::{TranscriptSegment, TranscriptWriter},
    AudioChunk,
};
use crate::meeting_context::RecognitionContext;
use crate::sensevoice_engine::{live::SenseVoiceSession, tail::TranscriptDraft};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};
use tauri::{AppHandle, Emitter, Runtime};

struct SourceSpan {
    id: u64,
    start: f64,
    end: f64,
}

fn final_text(raw: &str, context: Option<&RecognitionContext>) -> String {
    let (text, _) = crate::transcript_term_correction::correct_terms(raw);
    context.map_or(text.clone(), |c| c.normalize_transcript(&text))
}

pub struct SenseVoiceLiveWorker {
    session: Arc<Mutex<SenseVoiceSession>>,
    row_ids: HashMap<u64, u64>,
    sources: Vec<SourceSpan>,
}

impl SenseVoiceLiveWorker {
    pub fn new(session: SenseVoiceSession) -> Self {
        Self {
            session: Arc::new(Mutex::new(session)),
            row_ids: HashMap::new(),
            sources: Vec::new(),
        }
    }

    pub fn has_text(&self) -> bool {
        !self.row_ids.is_empty()
    }

    pub async fn push<R: Runtime>(
        &mut self,
        chunk: AudioChunk,
        app: &AppHandle<R>,
        writer: &TranscriptWriter,
        recorder: Option<&D11MeasurementRecorder>,
        context: Option<&RecognitionContext>,
        model: &str,
        allocate: fn() -> u64,
    ) -> Result<(), String> {
        super::sensevoice_provider::validate_sensevoice_language(
            crate::get_language_preference_internal().as_deref(),
        )
        .map_err(|e| e.to_string())?;
        if chunk.sample_rate == 0 {
            return Err("Invalid audio sample rate".to_owned());
        }
        let samples = if chunk.sample_rate == 16_000 {
            chunk.data
        } else {
            crate::audio::audio_processing::resample_audio(&chunk.data, chunk.sample_rate, 16_000)
        };
        let ids = [chunk.chunk_id];
        self.sources.push(SourceSpan {
            id: chunk.chunk_id,
            start: chunk.timestamp,
            end: chunk.timestamp + samples.len() as f64 / 16_000.0,
        });
        if let Some(recorder) = recorder {
            // This existing capture records the incoming PCM once. Bridge reads
            // reuse that PCM and are identified separately by boundary_status.
            recorder.record_inference_started(
                &ids,
                &samples,
                16_000,
                Some("zh"),
                true,
                context.map(|c| c.context_id.as_str()),
                context.map(|c| c.context_sha256.as_str()),
                "SenseVoice",
                model,
            );
        }
        let session = self.session.clone();
        let result = tokio::task::spawn_blocking(move || {
            session
                .lock()
                .map_err(|_| "SenseVoice session lock failed".to_owned())?
                .push(&samples, chunk.timestamp, chunk.device_epoch)
                .map_err(|e| e.to_string())
        })
        .await
        .map_err(|e| format!("SenseVoice worker failed: {e}"))??;
        if let Some(recorder) = recorder {
            recorder.record_inference_finished(&ids, None);
            let raw = result.raw_texts.join("\n");
            let revised = result
                .updates
                .iter()
                .map(|r| r.text())
                .collect::<Vec<_>>()
                .join("\n");
            let final_rows = result
                .updates
                .iter()
                .map(|r| final_text(&r.text(), context))
                .collect::<Vec<_>>()
                .join("\n");
            recorder.record_text(
                &ids,
                &raw,
                &revised,
                &final_rows,
                "sensevoice_revised_rows_raw_input_capture",
            );
        }
        log::info!(
            "SenseVoice boundary statuses for chunk {}: {:?}",
            chunk.chunk_id,
            result.boundary_status
        );
        if result.updates.is_empty() {
            if let Some(recorder) = recorder {
                recorder.record_terminal_without_writeback(&ids, "empty_transcript");
            }
            self.sources.retain(|s| s.id != chunk.chunk_id);
            return Ok(());
        }
        self.persist(result.updates, app, writer, recorder, context, allocate)
    }

    pub async fn finish<R: Runtime>(
        &mut self,
        app: &AppHandle<R>,
        writer: &TranscriptWriter,
        recorder: Option<&D11MeasurementRecorder>,
        context: Option<&RecognitionContext>,
        allocate: fn() -> u64,
    ) -> Result<(), String> {
        let session = self.session.clone();
        let drafts = tokio::task::spawn_blocking(move || {
            session
                .lock()
                .map_err(|_| "SenseVoice session lock failed".to_owned())
                .map(|mut s| s.finish())
        })
        .await
        .map_err(|e| format!("SenseVoice tail finish failed: {e}"))??;
        self.persist(drafts, app, writer, recorder, context, allocate)
    }

    fn persist<R: Runtime>(
        &mut self,
        drafts: Vec<TranscriptDraft>,
        app: &AppHandle<R>,
        writer: &TranscriptWriter,
        recorder: Option<&D11MeasurementRecorder>,
        context: Option<&RecognitionContext>,
        allocate: fn() -> u64,
    ) -> Result<(), String> {
        if drafts.is_empty() {
            return Ok(());
        }
        let mut finalized_end = None::<f64>;
        let updates: Vec<_> = drafts
            .into_iter()
            .map(|draft| {
                let sequence_id = *self.row_ids.entry(draft.row_id).or_insert_with(allocate);
                let raw = draft.text();
                // Preserve the pre-existing explicit term/context substitutions;
                // boundary detection itself always uses the recognizer's raw tokens.
                let text = final_text(&raw, context);
                let source_chunk_ids: Vec<_> = self
                    .sources
                    .iter()
                    .filter(|s| s.start < draft.audio_end && s.end > draft.audio_start)
                    .map(|s| s.id)
                    .collect();
                if !draft.is_partial {
                    finalized_end =
                        Some(finalized_end.map_or(draft.audio_end, |end| end.max(draft.audio_end)));
                }
                TranscriptUpdate {
                    chunk_id: source_chunk_ids.first().copied().unwrap_or(0),
                    source_chunk_ids,
                    text,
                    timestamp: chrono::Local::now().format("%H:%M:%S").to_string(),
                    source: "Audio".to_owned(),
                    sequence_id,
                    chunk_start_time: draft.audio_start,
                    is_partial: draft.is_partial,
                    revision: draft.revision,
                    confidence: 0.85, // Legacy storage field; SenseVoice does not report confidence.
                    audio_start_time: draft.audio_start,
                    audio_end_time: draft.audio_end,
                    duration: draft.audio_end - draft.audio_start,
                }
            })
            .collect();
        let result = writer
            .write_batch(
                updates
                    .iter()
                    .map(|u| TranscriptSegment {
                        id: format!("seg_{}", u.sequence_id),
                        text: u.text.clone(),
                        audio_start_time: u.audio_start_time,
                        audio_end_time: u.audio_end_time,
                        duration: u.duration,
                        display_time: u.timestamp.clone(),
                        confidence: u.confidence,
                        sequence_id: u.sequence_id,
                        revision: u.revision,
                        is_partial: u.is_partial,
                    })
                    .collect(),
            )
            .map_err(|e| format!("{e:#}"));
        if let Some(end) = finalized_end {
            let finalized: Vec<_> = self
                .sources
                .iter()
                .filter(|s| s.end <= end + 0.0001)
                .map(|s| s.id)
                .collect();
            if let Some(recorder) = recorder {
                recorder.record_final_writeback(&finalized, result.clone());
            }
            if result.is_ok() {
                self.sources.retain(|s| s.end > end + 0.0001);
            }
        }
        result?;
        app.emit("transcript-update-batch", updates)
            .map_err(|e| format!("Transcript display update failed: {e}"))
    }
}

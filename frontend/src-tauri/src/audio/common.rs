use crate::api::TranscriptSegment;
use anyhow::Result;
use log::{debug, info};
use once_cell::sync::Lazy;
use std::path::Path;
use std::sync::Arc;
use tokio::sync::{Mutex as AsyncMutex, OwnedMutexGuard};
use uuid::Uuid;

static ENGINE_LIFECYCLE_LOCK: Lazy<Arc<AsyncMutex<()>>> =
    Lazy::new(|| Arc::new(AsyncMutex::new(())));

pub(crate) async fn acquire_engine_lifecycle_lock() -> OwnedMutexGuard<()> {
    ENGINE_LIFECYCLE_LOCK.clone().lock_owned().await
}

/// Unload the transcription engine after a batch job (import or retranscription).
/// Skips unloading if a live recording is currently in progress, since recording
/// uses the same global engine instances.
pub(crate) async fn unload_engine_after_batch(use_parakeet: bool) {
    let _engine_lifecycle_guard = acquire_engine_lifecycle_lock().await;

    if crate::audio::recording_commands::is_recording().await {
        log::info!("Skipping model unload after batch: recording in progress");
        return;
    }

    if use_parakeet {
        use crate::parakeet_engine::commands::PARAKEET_ENGINE;
        let engine = {
            let guard = PARAKEET_ENGINE.lock().unwrap_or_else(|e| e.into_inner());
            guard.as_ref().cloned()
        };
        if let Some(e) = engine {
            e.unload_model().await;
        }
    } else {
        use crate::whisper_engine::commands::WHISPER_ENGINE;
        let engine = {
            let guard = WHISPER_ENGINE.lock().unwrap_or_else(|e| e.into_inner());
            guard.as_ref().cloned()
        };
        if let Some(e) = engine {
            e.unload_model().await;
        }
    }
}

/// Create transcript segments from transcription results.
/// Each tuple is (text, start_ms, end_ms) from VAD timestamps.
pub(crate) fn create_transcript_segments(
    transcripts: &[(String, f64, f64)],
) -> Vec<TranscriptSegment> {
    transcripts
        .iter()
        .map(|(text, start_ms, end_ms)| {
            let start_seconds = start_ms / 1000.0;
            let end_seconds = end_ms / 1000.0;
            let duration = end_seconds - start_seconds;

            TranscriptSegment {
                id: format!("transcript-{}", Uuid::new_v4()),
                text: text.trim().to_string(),
                timestamp: chrono::Utc::now().to_rfc3339(),
                audio_start_time: Some(start_seconds),
                audio_end_time: Some(end_seconds),
                duration: Some(duration),
            }
        })
        .collect()
}

/// Write transcripts.json to a meeting folder (atomic write with temp file)
pub(crate) fn write_transcripts_json(folder: &Path, segments: &[TranscriptSegment]) -> Result<()> {
    let transcript_path = folder.join("transcripts.json");
    let temp_path = folder.join(".transcripts.json.tmp");

    let json = serde_json::json!({
        "version": "1.0",
        "last_updated": chrono::Utc::now().to_rfc3339(),
        "total_segments": segments.len(),
        "segments": segments.iter().enumerate().map(|(i, s)| {
            serde_json::json!({
                "id": s.id,
                "text": s.text,
                "timestamp": s.timestamp,
                "audio_start_time": s.audio_start_time,
                "audio_end_time": s.audio_end_time,
                "duration": s.duration,
                "sequence_id": i
            })
        }).collect::<Vec<_>>()
    });

    let json_string = serde_json::to_string_pretty(&json)?;
    std::fs::write(&temp_path, &json_string)?;
    std::fs::rename(&temp_path, &transcript_path)?;

    info!(
        "Wrote transcripts.json with {} segments to {}",
        segments.len(),
        transcript_path.display()
    );
    Ok(())
}

/// Returns the RMS energy of one fixed-size window.
fn window_rms(samples: &[f32], start: usize, window: usize) -> f32 {
    let slice = &samples[start..start + window];
    (slice.iter().map(|s| s * s).sum::<f32>() / window as f32).sqrt()
}

/// Finds the quietest *sustained* silence in `[search_start, search_end)`.
///
/// A single 100ms dip can sit inside a word ("普拉|提斯"), and splitting there
/// makes each half lose part of the word. So a candidate only qualifies when the
/// window before and after it are also below the threshold — i.e. a real pause.
/// Returns `(split_sample, rms)`.
fn find_sustained_silence(
    samples: &[f32],
    search_start: usize,
    search_end: usize,
    energy_window: usize,
    threshold: f32,
    step: usize,
) -> Option<(usize, f32)> {
    if search_end <= search_start || search_end > samples.len() {
        return None;
    }

    let mut idx = search_start;
    let mut best: Option<(usize, f32)> = None;
    while idx + energy_window <= search_end {
        let rms = window_rms(samples, idx, energy_window);
        if rms <= threshold {
            let before_ok = idx < energy_window
                || window_rms(samples, idx - energy_window, energy_window) <= threshold;
            let after_ok = idx + 2 * energy_window > samples.len()
                || window_rms(samples, idx + energy_window, energy_window) <= threshold;
            if before_ok && after_ok && best.map_or(true, |(_, best_rms)| rms < best_rms) {
                best = Some((idx + energy_window / 2, rms));
            }
        }
        idx += step;
    }
    best
}

/// Split a long speech segment at the lowest-energy (silence) point near the target size.
///
/// Scans for 100ms windows with minimal RMS energy within +/-3 seconds of each target
/// split point. If no clear silence is found, falls back to a 1-second overlap split
/// to avoid cutting words at boundaries.
pub(crate) fn split_segment_at_silence(
    segment: &crate::audio::vad::SpeechSegment,
    max_samples: usize,
    lead_in_samples: usize,
) -> Vec<crate::audio::vad::SpeechSegment> {
    const SAMPLE_RATE: usize = 16000;
    // 100ms window for energy measurement (1600 samples at 16kHz)
    const ENERGY_WINDOW: usize = SAMPLE_RATE / 10;
    // Search +/-3 seconds around the target split point
    const SEARCH_RADIUS: usize = SAMPLE_RATE * 3;
    // Widen to +/-8 seconds when the near range has no real pause
    const WIDE_SEARCH_RADIUS: usize = SAMPLE_RATE * 8;
    // RMS threshold below which we consider a window "silent"
    const SILENCE_RMS_THRESHOLD: f32 = 0.02;
    // Overlap to use when no silence boundary is found (1 second)
    const FALLBACK_OVERLAP: usize = SAMPLE_RATE;

    let total = segment.samples.len();
    if total <= max_samples {
        return vec![segment.clone()];
    }

    let ms_per_sample =
        (segment.end_timestamp_ms - segment.start_timestamp_ms) / segment.samples.len() as f64;
    let mut result = Vec::new();
    let mut pos = 0usize;

    while pos < total {
        let remaining = total - pos;
        if remaining <= max_samples {
            // Last chunk - take everything remaining
            let mut chunk_samples = segment.samples[pos..].to_vec();
            if !result.is_empty() && lead_in_samples > 0 {
                let lead_start = pos.saturating_sub(lead_in_samples);
                if lead_start < pos {
                    let mut prefixed = Vec::with_capacity(lead_in_samples + chunk_samples.len());
                    prefixed.extend_from_slice(&segment.samples[lead_start..pos]);
                    prefixed.extend_from_slice(&chunk_samples);
                    chunk_samples = prefixed;
                }
            }
            let chunk_start_ms = segment.start_timestamp_ms + (pos as f64 * ms_per_sample);
            let chunk_end_ms = segment.end_timestamp_ms;
            result.push(crate::audio::vad::SpeechSegment {
                samples: chunk_samples,
                start_timestamp_ms: chunk_start_ms,
                end_timestamp_ms: chunk_end_ms,
                confidence: segment.confidence,
            });
            break;
        }

        // Target split point
        let target = pos + max_samples;

        // Search for a real pause near the target first; if there is none, widen
        // the search (still bounded, so a single chunk cannot grow past ~33s).
        let min_pos = pos + SAMPLE_RATE;
        let near_start = target.saturating_sub(SEARCH_RADIUS).max(min_pos);
        let near_end = (target + SEARCH_RADIUS).min(total.saturating_sub(ENERGY_WINDOW));
        let step = SAMPLE_RATE / 100;
        let mut sustained = find_sustained_silence(
            &segment.samples,
            near_start,
            near_end,
            ENERGY_WINDOW,
            SILENCE_RMS_THRESHOLD,
            step,
        );
        if sustained.is_none() {
            let wide_start = target
                .saturating_sub(WIDE_SEARCH_RADIUS)
                .max(min_pos);
            let wide_end = (target + WIDE_SEARCH_RADIUS).min(total.saturating_sub(ENERGY_WINDOW));
            sustained = find_sustained_silence(
                &segment.samples,
                wide_start,
                wide_end,
                ENERGY_WINDOW,
                SILENCE_RMS_THRESHOLD,
                step,
            );
        }

        // Find the lowest-energy 100ms window in the near range as the fallback
        let mut best_split = target.min(total); // fallback: exact target
        let mut best_rms = f32::MAX;

        if near_start + ENERGY_WINDOW <= near_end {
            let mut idx = near_start;
            while idx + ENERGY_WINDOW <= near_end {
                let rms = window_rms(&segment.samples, idx, ENERGY_WINDOW);
                if rms < best_rms {
                    best_rms = rms;
                    best_split = idx + ENERGY_WINDOW / 2; // split at center of quiet window
                }
                // Step by 10ms (160 samples) for efficiency
                idx += step;
            }
        }

        // Prefer a sustained pause; otherwise keep the old "quietest dip" split.
        let (split_at, used_sustained_silence) = match sustained {
            Some((split, _)) => (split, true),
            None => (best_split, false),
        };
        if used_sustained_silence {
            debug!(
                "Splitting at sustained silence: sample {} (near-target RMS={:.4})",
                split_at, best_rms
            );
        } else {
            debug!(
                "No sustained silence found near target (best RMS={:.4}), splitting at quietest dip {}",
                best_rms, split_at
            );
        }

        // Determine the actual end of this chunk (shift by 1s if we had to cut
        // through speech, so the cut does not land exactly on a word boundary)
        let chunk_end = if used_sustained_silence {
            split_at
        } else {
            (split_at + FALLBACK_OVERLAP).min(total)
        };

        let chunk_samples = segment.samples[pos..chunk_end].to_vec();
        let chunk_start_ms = segment.start_timestamp_ms + (pos as f64 * ms_per_sample);
        let chunk_end_ms = segment.start_timestamp_ms + (chunk_end as f64 * ms_per_sample);

        // 被迫从连续语音中间切开时，给后面的分片补一小段前文音频当上下文，
        // 否则模型在分片开头会丢字（合并文本时由调用方去掉重复的这段）。
        let mut chunk_samples = chunk_samples;
        if !result.is_empty() && lead_in_samples > 0 {
            let lead_start = pos.saturating_sub(lead_in_samples);
            if lead_start < pos {
                let mut prefixed =
                    Vec::with_capacity(lead_in_samples + chunk_samples.len());
                prefixed.extend_from_slice(&segment.samples[lead_start..pos]);
                prefixed.extend_from_slice(&chunk_samples);
                chunk_samples = prefixed;
            }
        }

        result.push(crate::audio::vad::SpeechSegment {
            samples: chunk_samples,
            start_timestamp_ms: chunk_start_ms,
            end_timestamp_ms: chunk_end_ms,
            confidence: segment.confidence,
        });

        // Advance position to where the current chunk actually ends
        // to avoid transcribing the overlap region twice
        pos = chunk_end;
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_RATE: usize = 16_000;

    /// P: 词中间的能量低谷不能当停顿，真正的停顿才能被选中。
    #[test]
    fn sustained_silence_prefers_a_real_pause_over_a_dip_inside_a_word() {
        let mut samples = vec![0.2f32; TEST_RATE * 30];
        // 100ms dip at 5s (inside a "word")
        for sample in &mut samples[5 * TEST_RATE..5 * TEST_RATE + TEST_RATE / 10] {
            *sample = 0.001;
        }
        // real 400ms pause at 10s
        for sample in &mut samples[10 * TEST_RATE..10 * TEST_RATE + TEST_RATE * 4 / 10] {
            *sample = 0.001;
        }

        let found = find_sustained_silence(
            &samples,
            4 * TEST_RATE,
            12 * TEST_RATE,
            TEST_RATE / 10,
            0.02,
            TEST_RATE / 100,
        )
        .expect("a real pause must qualify");

        let expected = 10 * TEST_RATE + TEST_RATE / 10;
        assert!(
            found.0.abs_diff(expected) < TEST_RATE / 2,
            "split landed at sample {} (expected around {})",
            found.0,
            expected
        );
    }

    #[test]
    fn a_lone_dip_does_not_qualify_as_a_pause() {
        let mut samples = vec![0.2f32; TEST_RATE * 10];
        for sample in &mut samples[5 * TEST_RATE..5 * TEST_RATE + TEST_RATE / 10] {
            *sample = 0.001;
        }
        assert!(find_sustained_silence(
            &samples,
            TEST_RATE,
            6 * TEST_RATE,
            TEST_RATE / 10,
            0.02,
            TEST_RATE / 100,
        )
        .is_none());
    }

    #[tokio::test]
    async fn test_engine_lifecycle_lock_serializes_acquirers() {
        let guard = acquire_engine_lifecycle_lock().await;
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let (acquired_tx, mut acquired_rx) = tokio::sync::oneshot::channel();
        let waiter = tokio::spawn(async {
            started_tx.send(()).unwrap();
            let _guard = acquire_engine_lifecycle_lock().await;
            acquired_tx.send(()).unwrap();
        });

        started_rx.await.unwrap();
        assert!(acquired_rx.try_recv().is_err());
        drop(guard);

        acquired_rx.await.unwrap();
        waiter.await.unwrap();
    }
}

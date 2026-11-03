use crate::api::TranscriptSegment;
use anyhow::Result;
use log::{debug, info};
use once_cell::sync::Lazy;
use std::path::Path;
use std::sync::Arc;
use tokio::sync::{Mutex as AsyncMutex, OwnedMutexGuard};
use uuid::Uuid;

pub(crate) static BATCH_INFERENCE_LOCK: AsyncMutex<()> = AsyncMutex::const_new(());

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
        let sensevoice = crate::sensevoice_engine::commands::SENSEVOICE_ENGINE.lock().unwrap_or_else(|e| e.into_inner()).clone();
        if let Some(engine) = sensevoice { engine.unload_model().await; }
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
        .enumerate()
        .map(|(index, (text, start_ms, end_ms))| {
            let start_seconds = start_ms / 1000.0;
            let end_seconds = end_ms / 1000.0;
            let duration = end_seconds - start_seconds;

            TranscriptSegment {
                id: format!("transcript-{}", Uuid::new_v4()),
                text: trim_boundary_overlap(
                    index.checked_sub(1).map(|i| &transcripts[i]),
                    text,
                    *start_ms,
                    *end_ms,
                )
                .to_string(),
                timestamp: chrono::Utc::now().to_rfc3339(),
                audio_start_time: Some(start_seconds),
                audio_end_time: Some(end_seconds),
                duration: Some(duration),
            }
        })
        .collect()
}

/// Only remove a substantial exact suffix/prefix match in a real audio overlap.
/// Keep uncertain matches, short acknowledgements and whole repeated utterances.
fn trim_boundary_overlap<'a>(
    previous: Option<&(String, f64, f64)>,
    text: &'a str,
    start: f64,
    end: f64,
) -> &'a str {
    let text = text.trim();
    let Some((previous, previous_start, previous_end)) = previous else {
        return text;
    };
    let overlap = previous_end - start;
    if !(0.0 < overlap && overlap <= 1100.0 && start > *previous_start && end > *previous_end) {
        return text;
    }
    let previous = previous.trim_end_matches(|c: char| {
        c.is_whitespace() || matches!(c, '.' | ',' | '!' | '?' | '。' | '，' | '！' | '？')
    });
    // Bound matching to the short shared audio window; this is deliberately
    // conservative, not an estimate of token timestamps or speaking speed.
    let max_chars = ((overlap / 1000.0 * 12.0).ceil() as usize).min(16);
    let boundaries: Vec<usize> = text
        .char_indices()
        .map(|(i, _)| i)
        .take(max_chars + 1)
        .collect();
    for &cut in boundaries.iter().rev() {
        let prefix = &text[..cut];
        if prefix.chars().filter(|c| c.is_alphanumeric()).count() < 4 || !previous.ends_with(prefix)
        {
            continue;
        }
        // Protect English words even inside mixed Chinese/English sentences.
        if (prefix.ends_with(|c: char| c.is_ascii_alphanumeric())
            && text[cut..].starts_with(|c: char| c.is_ascii_alphanumeric()))
            || (prefix.starts_with(|c: char| c.is_ascii_alphanumeric())
                && previous[..previous.len() - prefix.len()]
                    .ends_with(|c: char| c.is_ascii_alphanumeric()))
            || (prefix.is_ascii() && prefix.split_whitespace().count() < 2)
        {
            continue;
        }
        let rest = text[cut..].trim_start_matches(|c: char| {
            c.is_whitespace() || matches!(c, '.' | ',' | '!' | '?' | '。' | '，' | '！' | '？')
        });
        if !rest.is_empty() {
            return rest;
        }
    }
    text
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

/// Split a long speech segment at the lowest-energy (silence) point near the target size.
///
/// Scans for 100ms windows with minimal RMS energy within +/-3 seconds of each target
/// split point. If no clear silence is found, falls back to a 1-second overlap split
/// to avoid cutting words at boundaries.
pub(crate) fn split_segment_at_silence(
    segment: &crate::audio::vad::SpeechSegment,
    max_samples: usize,
) -> Vec<crate::audio::vad::SpeechSegment> {
    const SAMPLE_RATE: usize = 16000;
    // 100ms window for energy measurement (1600 samples at 16kHz)
    const ENERGY_WINDOW: usize = SAMPLE_RATE / 10;
    // Search +/-3 seconds around the target split point
    const SEARCH_RADIUS: usize = SAMPLE_RATE * 3;
    // RMS threshold below which we consider a window "silent"
    const SILENCE_RMS_THRESHOLD: f32 = 0.02;
    // Overlap to use when no silence boundary is found (1 second)
    const FALLBACK_OVERLAP: usize = SAMPLE_RATE;

    assert!(max_samples > 0, "segment size must be positive");

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
            let chunk_samples = segment.samples[pos..].to_vec();
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

        // Search window: [target - SEARCH_RADIUS, target + SEARCH_RADIUS]
        let search_start = target.saturating_sub(SEARCH_RADIUS).max(pos + SAMPLE_RATE);
        let search_end = (target + SEARCH_RADIUS).min(total.saturating_sub(ENERGY_WINDOW));

        // Find the lowest-energy 100ms window in the search range
        let mut best_split = target.min(total); // fallback: exact target
        let mut best_rms = f32::MAX;

        if search_start + ENERGY_WINDOW <= search_end {
            let mut idx = search_start;
            while idx + ENERGY_WINDOW <= search_end {
                let window = &segment.samples[idx..idx + ENERGY_WINDOW];
                let rms = (window.iter().map(|s| s * s).sum::<f32>() / ENERGY_WINDOW as f32).sqrt();
                if rms < best_rms {
                    best_rms = rms;
                    best_split = idx + ENERGY_WINDOW / 2; // split at center of quiet window
                }
                // Step by 10ms (160 samples) for efficiency
                idx += SAMPLE_RATE / 100;
            }
        }

        // Without a genuine pause, use the target rather than a random low-energy
        // syllable. Extend the previous chunk and start the next at this boundary.
        let split_at = if best_rms <= SILENCE_RMS_THRESHOLD {
            best_split
        } else {
            target
        };
        if best_rms <= SILENCE_RMS_THRESHOLD {
            debug!(
                "Splitting at silence boundary: sample {} (RMS={:.4})",
                split_at, best_rms
            );
        } else {
            debug!(
                "No silence found near target (best RMS={:.4}), splitting with overlap at sample {}",
                best_rms, split_at
            );
        }

        // Determine the actual end of this chunk (with overlap if no silence)
        let chunk_end = if best_rms > SILENCE_RMS_THRESHOLD {
            (split_at + FALLBACK_OVERLAP).min(total)
        } else {
            split_at
        };

        let chunk_samples = segment.samples[pos..chunk_end].to_vec();
        let chunk_start_ms = segment.start_timestamp_ms + (pos as f64 * ms_per_sample);
        let chunk_end_ms = segment.start_timestamp_ms + (chunk_end as f64 * ms_per_sample);

        result.push(crate::audio::vad::SpeechSegment {
            samples: chunk_samples,
            start_timestamp_ms: chunk_start_ms,
            end_timestamp_ms: chunk_end_ms,
            confidence: segment.confidence,
        });

        if chunk_end == total {
            break;
        }
        pos = split_at;
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn boundary_overlap_preserves_audio_and_overlaps_exactly_one_second() {
        let source = crate::audio::vad::SpeechSegment {
            samples: (0..60 * 16000)
                .map(|i| 0.1 + (i % 100) as f32 / 1000.0)
                .collect(),
            start_timestamp_ms: 5000.0,
            end_timestamp_ms: 65000.0,
            confidence: 0.9,
        };
        let chunks = split_segment_at_silence(&source, 25 * 16000);
        assert_eq!(chunks.first().unwrap().start_timestamp_ms, 5000.0);
        assert_eq!(chunks.last().unwrap().end_timestamp_ms, 65000.0);
        for pair in chunks.windows(2) {
            assert_eq!(
                pair[0].end_timestamp_ms - pair[1].start_timestamp_ms,
                1000.0
            );
            assert_eq!(
                &pair[0].samples[pair[0].samples.len() - 16000..],
                &pair[1].samples[..16000]
            );
        }
        for chunk in &chunks {
            let start = ((chunk.start_timestamp_ms - 5000.0) * 16.0).round() as usize;
            assert_eq!(
                chunk.samples,
                source.samples[start..start + chunk.samples.len()]
            );
            assert!(chunk.samples.len() <= 26 * 16000);
        }
    }

    #[test]
    fn boundary_overlap_ends_without_redundant_tail() {
        let source = crate::audio::vad::SpeechSegment {
            samples: vec![0.5; 26 * 16000],
            start_timestamp_ms: 0.0,
            end_timestamp_ms: 26000.0,
            confidence: 0.9,
        };
        let chunks = split_segment_at_silence(&source, 25 * 16000);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].samples, source.samples);
    }

    #[test]
    fn boundary_overlap_real_silence_needs_no_overlap() {
        let mut source = crate::audio::vad::SpeechSegment {
            samples: vec![0.5; 40 * 16000],
            start_timestamp_ms: 0.0,
            end_timestamp_ms: 40000.0,
            confidence: 0.9,
        };
        source.samples[24 * 16000..25 * 16000].fill(0.0);
        let chunks = split_segment_at_silence(&source, 25 * 16000);
        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0].end_timestamp_ms, chunks[1].start_timestamp_ms);
        assert_eq!(
            chunks
                .iter()
                .flat_map(|c| c.samples.iter())
                .copied()
                .collect::<Vec<_>>(),
            source.samples
        );
    }

    #[test]
    fn boundary_overlap_stitches_chinese_and_english_in_saved_segments() {
        for (left, right, expected) in [
            (
                "下一步检查接口性能。",
                "检查接口性能，然后发布。",
                "然后发布。",
            ),
            (
                "We review the API gateway.",
                "API gateway, then deploy.",
                "then deploy.",
            ),
        ] {
            let transcripts = vec![
                (left.into(), 0.0, 26000.0),
                (right.into(), 25000.0, 50000.0),
            ];
            assert_eq!(create_transcript_segments(&transcripts)[1].text, expected);
            // Checkpoints store raw tuples. Resume must apply stitching only once.
            let checkpoint = serde_json::to_string(&transcripts).unwrap();
            let resumed: Vec<(String, f64, f64)> = serde_json::from_str(&checkpoint).unwrap();
            let segments = create_transcript_segments(&resumed);
            assert_eq!(segments[1].text, expected);
            let folder = tempfile::tempdir().unwrap();
            write_transcripts_json(folder.path(), &segments).unwrap();
            let saved: serde_json::Value = serde_json::from_str(
                &std::fs::read_to_string(folder.path().join("transcripts.json")).unwrap(),
            )
            .unwrap();
            assert_eq!(saved["segments"][1]["text"], expected);
        }
    }

    #[test]
    fn boundary_overlap_keeps_ambiguous_and_nonoverlapping_speech() {
        for (left, right, start) in [
            ("下一步检查接口性能", "检查接口性能，然后发布。", 26000.0),
            ("好的好的", "好的好的，继续。", 25950.0),
            ("下一步检查接口性能", "检查接口性能。", 25000.0),
            ("We test the gateway", "gateway deployment", 25000.0),
            ("a superfast API", "fast API changes", 25000.0),
            ("We test API gate", "API gateway changes", 25000.0),
            ("检查API gate", "检查API gateway后发布", 25000.0),
        ] {
            let previous = (left.into(), 0.0, 26000.0);
            assert_eq!(
                trim_boundary_overlap(Some(&previous), right, start, 50000.0),
                right
            );
        }
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

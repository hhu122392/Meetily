use anyhow::{anyhow, Result};
use log::{debug, info, warn};
use silero_rs::{VadConfig, VadSession, VadTransition};
use std::collections::VecDeque;
use std::time::Duration;

const VAD_SAMPLE_RATE: usize = 16_000;

fn absolute_vad_sample(timestamp_ms: usize) -> usize {
    timestamp_ms.saturating_mul(VAD_SAMPLE_RATE) / 1_000
}

/// Represents a complete speech segment detected by VAD
#[derive(Debug, Clone)]
pub struct SpeechSegment {
    pub samples: Vec<f32>,
    pub start_timestamp_ms: f64,
    pub end_timestamp_ms: f64,
    pub confidence: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VadTraceState {
    pub processed_samples: usize,
    pub buffered_input_samples: usize,
    pub buffered_speech_samples: usize,
    pub in_speech: bool,
}

/// Processes audio in 30ms chunks but returns complete speech segments
pub struct ContinuousVadProcessor {
    session: VadSession,
    chunk_size: usize,
    sample_rate: u32,
    buffer: Vec<f32>,
    speech_segments: VecDeque<SpeechSegment>,
    current_speech: Vec<f32>,
    in_speech: bool,
    processed_samples: usize,
    speech_start_sample: usize,
    // State tracking for smart logging
    last_logged_state: bool,
    // Optional latency bound for live transcription. Batch transcription leaves
    // this disabled so its longer-segment accuracy remains unchanged.
    max_segment_samples: Option<usize>,
    // After the first live bubble is visible, later bubbles may use a longer
    // acoustic window. This preserves startup responsiveness without forcing
    // every Mandarin phrase through the lower-accuracy first-bubble limit.
    subsequent_max_segment_samples: Option<usize>,
    emitted_live_segments: usize,
    // Once a long utterance has been split, the VAD SpeechEnd payload contains
    // audio that was already emitted. Track that state so only the remaining
    // tail is emitted when speech finally ends.
    streaming_split_active: bool,
    gap_audio: Option<super::gap_audio::GapAudio>,
}

impl ContinuousVadProcessor {
    pub fn new(input_sample_rate: u32, redemption_time_ms: u32) -> Result<Self> {
        // Silero VAD MUST use 16kHz - this is hardcoded requirement
        const VAD_SAMPLE_RATE: u32 = 16000;

        // Use STRICT settings to prevent silence from reaching Whisper
        let mut config = VadConfig::default();
        config.sample_rate = VAD_SAMPLE_RATE as usize;

        // CONTINUOUS SPEECH FIX: Tuned for capturing complete 5+ second utterances
        // Previous: 0.55/0.40 with 400ms redemption was fragmenting speech into 40ms segments
        // New: More lenient thresholds + longer redemption for continuous speech
        config.positive_speech_threshold = 0.50; // Silero default - good for continuous speech
        config.negative_speech_threshold = 0.35; // Silero default - allows natural pauses

        // CRITICAL FIX: Removed redemption_time capping to support long continuous speech
        // Previous: capped at 400ms, causing VAD to fragment 5-second speech into 40ms segments
        // New: Use full redemption_time from pipeline (2000ms) to bridge natural pauses
        config.redemption_time = Duration::from_millis(redemption_time_ms as u64);
        config.pre_speech_pad = Duration::from_millis(300); // Pre-speech padding for context
        config.post_speech_pad = Duration::from_millis(400); // Increased: more context at end

        // CRITICAL FIX: Increased min_speech_time to prevent tiny 40ms fragments
        // Previous: 100ms allowed too-short segments that Whisper rejects
        // New: 250ms ensures segments are substantial enough for Whisper (>100ms requirement)
        config.min_speech_time = Duration::from_millis(250); // Prevent tiny fragments

        debug!("Creating VAD session with: sample_rate={}Hz, redemption={}ms, min_speech={}ms, input_rate={}Hz",
               VAD_SAMPLE_RATE, redemption_time_ms, 250, input_sample_rate);

        let session = VadSession::new(config)
            .map_err(|e| anyhow!("Failed to create VAD session: {:?}", e))?;

        // VAD uses 30ms chunks at 16kHz (480 samples)
        let vad_chunk_size = (VAD_SAMPLE_RATE as f32 * 0.03) as usize; // 480 samples

        info!(
            "VAD processor created: input={}Hz, vad={}Hz, chunk_size={} samples",
            input_sample_rate, VAD_SAMPLE_RATE, vad_chunk_size
        );

        Ok(Self {
            session,
            chunk_size: vad_chunk_size,
            sample_rate: input_sample_rate, // Store input rate for resampling ratio in resample_to_16k()
            buffer: Vec::with_capacity(vad_chunk_size * 2),
            speech_segments: VecDeque::new(),
            current_speech: Vec::new(),
            in_speech: false,
            processed_samples: 0,
            speech_start_sample: 0,
            // Initialize state tracking
            last_logged_state: false,
            max_segment_samples: None,
            subsequent_max_segment_samples: None,
            emitted_live_segments: 0,
            streaming_split_active: false,
            gap_audio: None,
        })
    }

    /// Bound the amount of continuous speech buffered before emitting a segment.
    ///
    /// This is intended for live transcription. Natural VAD speech ends still
    /// emit immediately; this only prevents a speaker who does not pause from
    /// holding the UI indefinitely without any transcript update.
    pub fn with_max_segment_duration_ms(mut self, duration_ms: u32) -> Self {
        let requested_samples =
            ((duration_ms as u64 * 16_000) / 1_000).max(self.chunk_size as u64) as usize;
        self.max_segment_samples = Some(requested_samples);
        self.subsequent_max_segment_samples = Some(requested_samples);
        self
    }

    /// Use a short bound only for the first live result, then retain more
    /// acoustic context for subsequent results. Batch callers do not enable
    /// this mode.
    pub fn with_adaptive_live_segment_duration_ms(
        mut self,
        first_duration_ms: u32,
        subsequent_duration_ms: u32,
    ) -> Self {
        let samples_for = |duration_ms: u32| {
            ((duration_ms as u64 * 16_000) / 1_000).max(self.chunk_size as u64) as usize
        };
        self.max_segment_samples = Some(samples_for(first_duration_ms));
        self.subsequent_max_segment_samples = Some(samples_for(subsequent_duration_ms));
        self
    }

    /// Read-only state used by the opt-in D-11 evidence recorder. It does not
    /// participate in any VAD decision.
    pub fn trace_state(&self) -> VadTraceState {
        VadTraceState {
            processed_samples: self.processed_samples,
            buffered_input_samples: self.buffer.len(),
            buffered_speech_samples: self.current_speech.len(),
            in_speech: self.in_speech,
        }
    }

    pub fn enable_gap_preservation(&mut self) {
        self.gap_audio = Some(super::gap_audio::GapAudio::new(32 * VAD_SAMPLE_RATE, 3 * VAD_SAMPLE_RATE));
    }

    fn preserve_gap(&mut self, mut segment: SpeechSegment) -> SpeechSegment {
        if let Some(history) = &mut self.gap_audio {
            let start = (segment.start_timestamp_ms * 16.0).round() as usize;
            let start = history.prepend_gap(start, &mut segment.samples);
            segment.start_timestamp_ms = start as f64 / 16.0;
            segment.end_timestamp_ms = (start + segment.samples.len()) as f64 / 16.0;
        }
        segment
    }

    /// Process incoming audio samples and return any complete speech segments
    /// Handles resampling from input sample rate to 16kHz for VAD processing
    pub fn process_audio(&mut self, samples: &[f32]) -> Result<Vec<SpeechSegment>> {
        // Resample to 16kHz if needed
        let resampled_audio = if self.sample_rate == 16000 {
            samples.to_vec()
        } else {
            self.resample_to_16k(samples)?
        };

        self.buffer.extend_from_slice(&resampled_audio);
        let mut completed_segments = Vec::new();

        // Process complete 30ms chunks (480 samples at 16kHz)
        while self.buffer.len() >= self.chunk_size {
            let chunk: Vec<f32> = self.buffer.drain(..self.chunk_size).collect();
            if let Some(history) = &mut self.gap_audio {
                history.append(self.processed_samples, &chunk);
            }
            self.process_chunk(&chunk)?;

            // Extract any completed speech segments
            while let Some(segment) = self.speech_segments.pop_front() {
                completed_segments.push(self.preserve_gap(segment));
            }
        }

        Ok(completed_segments)
    }

    /// Improved resampling from input sample rate to 16kHz with anti-aliasing
    /// Uses linear interpolation and basic low-pass filtering for better quality
    fn resample_to_16k(&self, samples: &[f32]) -> Result<Vec<f32>> {
        if self.sample_rate == 16000 {
            return Ok(samples.to_vec());
        }

        // Calculate downsampling ratio
        let ratio = self.sample_rate as f64 / 16000.0;
        let output_len = (samples.len() as f64 / ratio) as usize;
        let mut resampled = Vec::with_capacity(output_len);

        // Apply simple low-pass filter before downsampling to reduce aliasing
        let cutoff_freq = 0.4; // Normalized frequency (0.4 * Nyquist)
        let mut filtered_samples = Vec::with_capacity(samples.len());

        // Simple moving average filter (basic low-pass)
        let filter_size =
            (self.sample_rate as f64 / (cutoff_freq * self.sample_rate as f64)) as usize;
        let filter_size = std::cmp::max(1, std::cmp::min(filter_size, 5)); // Limit filter size

        for i in 0..samples.len() {
            let start = if i >= filter_size { i - filter_size } else { 0 };
            let end = std::cmp::min(i + filter_size + 1, samples.len());
            let sum: f32 = samples[start..end].iter().sum();
            filtered_samples.push(sum / (end - start) as f32);
        }

        // Linear interpolation downsampling
        for i in 0..output_len {
            let source_pos = i as f64 * ratio;
            let source_index = source_pos as usize;
            let fraction = source_pos - source_index as f64;

            if source_index + 1 < filtered_samples.len() {
                // Linear interpolation
                let sample1 = filtered_samples[source_index];
                let sample2 = filtered_samples[source_index + 1];
                let interpolated = sample1 + (sample2 - sample1) * fraction as f32;
                resampled.push(interpolated);
            } else if source_index < filtered_samples.len() {
                resampled.push(filtered_samples[source_index]);
            }
        }

        debug!(
            "Resampled from {} samples ({}Hz) to {} samples (16kHz) with anti-aliasing",
            samples.len(),
            self.sample_rate,
            resampled.len()
        );

        Ok(resampled)
    }

    /// Flush any remaining audio and return final speech segments
    pub fn flush(&mut self) -> Result<Vec<SpeechSegment>> {
        debug!("VAD flush: in_speech={}, current_speech_len={}, buffer_len={}, speech_segments_queued={}",
              self.in_speech, self.current_speech.len(), self.buffer.len(), self.speech_segments.len());

        let mut completed_segments = Vec::new();

        // Process any remaining buffered audio
        // VAD needs a full frame, but padding must not extend the real recording.
        let real_end_sample = self.processed_samples + self.buffer.len();
        if !self.buffer.is_empty() {
            let remaining = self.buffer.clone();
            self.buffer.clear();
            if let Some(history) = &mut self.gap_audio {
                history.append(self.processed_samples, &remaining);
            }

            // Pad to chunk size if needed
            let mut padded_chunk = remaining;
            if padded_chunk.len() < self.chunk_size {
                padded_chunk.resize(self.chunk_size, 0.0);
            }

            self.process_chunk(&padded_chunk)?;
        }

        // Force end any ongoing speech
        if self.in_speech && !self.current_speech.is_empty() {
            // processed_samples and speech_start_sample always count 16kHz samples (post-resampling)
            let start_ms = (self.speech_start_sample as f64 / 16000.0) * 1000.0;
            let end_ms = (self.processed_samples as f64 / 16000.0) * 1000.0;

            debug!(
                "VAD flush: Force-ending speech - start={}ms, end={}ms, duration={}ms, samples={}",
                start_ms,
                end_ms,
                end_ms - start_ms,
                self.current_speech.len()
            );

            let segment = SpeechSegment {
                samples: self.current_speech.clone(),
                start_timestamp_ms: start_ms,
                end_timestamp_ms: end_ms,
                confidence: 0.8, // Estimated confidence for forced end
            };

            self.speech_segments.push_back(segment);
            self.current_speech.clear();
            self.in_speech = false;
        }

        // Extract all remaining segments
        while let Some(mut segment) = self.speech_segments.pop_front() {
            // Padding can also trigger a natural end or a live split above.
            let start_sample = (segment.start_timestamp_ms * 16.0).round() as usize;
            segment
                .samples
                .truncate(real_end_sample.saturating_sub(start_sample));
            segment.end_timestamp_ms = (start_sample + segment.samples.len()) as f64 / 16.0;
            if !segment.samples.is_empty() {
                completed_segments.push(self.preserve_gap(segment));
            }
        }

        Ok(completed_segments)
    }

    fn process_chunk(&mut self, chunk: &[f32]) -> Result<()> {
        // Track accumulated speech buffer size to detect memory issues
        let current_speech_size = self.current_speech.len();
        if current_speech_size > 1_000_000 {
            // More than ~62 seconds of accumulated speech at 16kHz
            warn!("VAD: Accumulated speech buffer is large: {} samples ({:.1}s) - possible memory issue",
                  current_speech_size, current_speech_size as f64 / 16000.0);
        }

        let transitions = self
            .session
            .process(chunk)
            .map_err(|e| anyhow!("VAD processing failed: {}", e))?;

        // Log transitions for debugging
        if !transitions.is_empty() {
            debug!(
                "VAD transitions at sample {}: {} transitions",
                self.processed_samples,
                transitions.len()
            );
        }

        // Handle VAD transitions
        for transition in transitions {
            match transition {
                VadTransition::SpeechStart { timestamp_ms } => {
                    // Silero can occasionally emit another SpeechStart while a
                    // latency-bounded segment is still active. Resetting our
                    // buffer here would lose the split state and later produce
                    // an overlapping tail segment.
                    if self.in_speech {
                        debug!(
                            "VAD: Ignoring duplicate SpeechStart at {}ms while speech is active",
                            timestamp_ms
                        );
                        continue;
                    }

                    // Only log if state changed
                    if !self.last_logged_state {
                        debug!("VAD: Speech started at {}ms", timestamp_ms);
                        self.last_logged_state = true;
                    }
                    self.in_speech = true;
                    // silero-rs reports an absolute timestamp since the start of
                    // the VAD session. Adding `processed_samples` again doubles
                    // the timeline and makes latency-bounded segments jump ahead
                    // of later natural speech-end segments.
                    self.speech_start_sample = absolute_vad_sample(timestamp_ms);
                    // SpeechStart arrives after padding and speech confirmation.
                    // Recover that already-buffered audio; the current chunk is
                    // appended once below, not twice.
                    let speech = self.session.get_current_speech();
                    self.current_speech =
                        speech[..speech.len().saturating_sub(chunk.len())].to_vec();
                    self.streaming_split_active = false;
                }
                VadTransition::SpeechEnd {
                    start_timestamp_ms,
                    end_timestamp_ms,
                    samples,
                } => {
                    // Only log if we were previously in speech state
                    if self.last_logged_state {
                        debug!(
                            "VAD: Speech ended at {}ms (duration: {}ms)",
                            end_timestamp_ms,
                            end_timestamp_ms - start_timestamp_ms
                        );
                        self.last_logged_state = false;
                    }
                    self.in_speech = false;

                    // After a latency-bounded split, the transition payload spans
                    // the original utterance and would duplicate already emitted
                    // text. In that case emit only our unsent tail.
                    let (speech_samples, segment_start_ms, segment_end_ms) =
                        if self.streaming_split_active {
                            // Use Silero's original samples and post-speech end.
                            // Our live buffer also contains the longer silence
                            // used to confirm SpeechEnd and is not the same tail.
                            let emitted = self
                                .speech_start_sample
                                .saturating_sub(absolute_vad_sample(start_timestamp_ms));
                            let tail = samples.get(emitted..).unwrap_or_default().to_vec();
                            let start_ms = self.speech_start_sample as f64 / 16.0;
                            let end_ms = start_ms + tail.len() as f64 / 16.0;
                            (tail, start_ms, end_ms)
                        } else {
                            let speech_samples = if !samples.is_empty() {
                                samples
                            } else {
                                self.current_speech.clone()
                            };
                            (
                                speech_samples,
                                start_timestamp_ms as f64,
                                end_timestamp_ms as f64,
                            )
                        };

                    if !speech_samples.is_empty() {
                        let segment = SpeechSegment {
                            samples: speech_samples,
                            start_timestamp_ms: segment_start_ms,
                            end_timestamp_ms: segment_end_ms,
                            confidence: 0.9, // VAD confidence
                        };

                        info!(
                            "VAD: Completed speech segment: {:.1}ms duration, {} samples",
                            segment.end_timestamp_ms - segment.start_timestamp_ms,
                            segment.samples.len()
                        );

                        self.speech_segments.push_back(segment);
                        self.emitted_live_segments += 1;
                    }

                    self.current_speech.clear();
                    self.streaming_split_active = false;
                }
            }
        }

        // Accumulate speech if we're currently in a speech state
        if self.in_speech {
            self.current_speech.extend_from_slice(chunk);
            self.emit_latency_bounded_segments();
        }

        self.processed_samples += chunk.len();
        Ok(())
    }

    fn emit_latency_bounded_segments(&mut self) {
        let Some(first_max_segment_samples) = self.max_segment_samples else {
            return;
        };

        loop {
            let max_segment_samples = if self.emitted_live_segments == 0 {
                first_max_segment_samples
            } else {
                self.subsequent_max_segment_samples
                    .unwrap_or(first_max_segment_samples)
            };
            if self.current_speech.len() < max_segment_samples {
                break;
            }
            let split_index = self.find_low_energy_split(max_segment_samples);
            let segment_samples: Vec<f32> = self.current_speech.drain(..split_index).collect();
            let segment_start_sample = self.speech_start_sample;
            let segment_end_sample = segment_start_sample + segment_samples.len();

            self.speech_segments.push_back(SpeechSegment {
                samples: segment_samples,
                start_timestamp_ms: segment_start_sample as f64 / 16.0,
                end_timestamp_ms: segment_end_sample as f64 / 16.0,
                confidence: 0.85,
            });

            self.speech_start_sample = segment_end_sample;
            self.streaming_split_active = true;
            self.emitted_live_segments += 1;
            info!(
                "VAD: Emitting latency-bounded live segment: {:.1}ms (limit {:.1}ms)",
                split_index as f64 / 16.0,
                max_segment_samples as f64 / 16.0
            );
        }
    }

    /// Prefer a quiet word boundary near the duration limit. Chinese words can
    /// be badly damaged when a fixed-size cut lands between two syllables, so
    /// search only the final 20% (clamped to 0.5-1.2 seconds) for a 20ms
    /// low-energy window. If the speaker truly does not pause, retain the hard
    /// duration bound.
    fn find_low_energy_split(&self, max_segment_samples: usize) -> usize {
        const SEARCH_BACK_MAX_SAMPLES: usize = 19_200; // 1.2s at 16kHz
        const SEARCH_BACK_MIN_SAMPLES: usize = 8_000; // 0.5s at 16kHz
        const ENERGY_WINDOW_SAMPLES: usize = 320; // 20ms
        const SEARCH_STEP_SAMPLES: usize = 160; // 10ms
        const QUIET_RMS_THRESHOLD: f32 = 0.02;

        if max_segment_samples <= ENERGY_WINDOW_SAMPLES
            || self.current_speech.len() < max_segment_samples
        {
            return max_segment_samples.min(self.current_speech.len());
        }

        // Searching half of a four-second live window allowed a quiet point at
        // two seconds to win, recreating the tiny fragments the duration cap was
        // meant to prevent. Restrict the search to the final 20% (0.5-1.2s).
        let search_back_samples =
            (max_segment_samples / 5).clamp(SEARCH_BACK_MIN_SAMPLES, SEARCH_BACK_MAX_SAMPLES);
        let search_start = max_segment_samples
            .saturating_sub(search_back_samples)
            .max(self.chunk_size);
        let search_end = max_segment_samples - ENERGY_WINDOW_SAMPLES;
        let mut best_start = max_segment_samples;
        let mut best_rms = f32::MAX;

        for start in (search_start..=search_end).step_by(SEARCH_STEP_SAMPLES) {
            let window = &self.current_speech[start..start + ENERGY_WINDOW_SAMPLES];
            let rms = (window.iter().map(|sample| sample * sample).sum::<f32>()
                / ENERGY_WINDOW_SAMPLES as f32)
                .sqrt();
            if rms < best_rms {
                best_rms = rms;
                best_start = start;
            }
        }

        if best_rms <= QUIET_RMS_THRESHOLD {
            (best_start + ENERGY_WINDOW_SAMPLES / 2).min(max_segment_samples)
        } else {
            max_segment_samples
        }
    }
}

/// Legacy function for backward compatibility - now uses the optimized approach
pub fn extract_speech_16k(samples_mono_16k: &[f32]) -> Result<Vec<f32>> {
    let mut processor = ContinuousVadProcessor::new(16000, 400)?;

    // Process all audio
    let mut all_segments = processor.process_audio(samples_mono_16k)?;
    let final_segments = processor.flush()?;
    all_segments.extend(final_segments);

    // Concatenate all speech segments
    let mut result = Vec::new();
    let num_segments = all_segments.len();
    for segment in &all_segments {
        result.extend_from_slice(&segment.samples);
    }

    // Apply balanced energy filtering for very short segments
    if result.len() < 1600 {
        // Less than 100ms at 16kHz
        let input_energy: f32 =
            samples_mono_16k.iter().map(|&x| x * x).sum::<f32>() / samples_mono_16k.len() as f32;
        let rms = input_energy.sqrt();
        let peak = samples_mono_16k
            .iter()
            .map(|&x| x.abs())
            .fold(0.0f32, f32::max);

        // BALANCED FIX: Lowered thresholds to preserve quiet speech while still filtering silence
        // Previous aggressive values (0.08/0.15) were discarding valid quiet speech
        // New values (0.03/0.08) are more balanced - catch quiet speech, reject pure silence
        if rms < 0.2 || peak < 0.20 {
            info!("-----VAD detected silence/noise (RMS: {:.6}, Peak: {:.6}), skipping to prevent hallucinations-----", rms, peak);
            return Ok(Vec::new());
        } else {
            info!(
                "VAD detected speech with sufficient energy (RMS: {:.6}, Peak: {:.6})",
                rms, peak
            );
            return Ok(samples_mono_16k.to_vec());
        }
    }

    debug!(
        "VAD: Processed {} samples, extracted {} speech samples from {} segments",
        samples_mono_16k.len(),
        result.len(),
        num_segments
    );

    Ok(result)
}

/// Simple convenience function to get speech chunks from audio
/// Uses the optimized ContinuousVadProcessor with configurable redemption time
pub fn get_speech_chunks(
    samples_mono_16k: &[f32],
    redemption_time_ms: u32,
) -> Result<Vec<SpeechSegment>> {
    get_speech_chunks_with_progress(samples_mono_16k, redemption_time_ms, |_, _| true)
}

/// Get speech chunks with progress callback and cancellation support
/// The callback receives (progress_percent, segments_found) and returns false to cancel
pub fn get_speech_chunks_with_progress<F>(
    samples_mono_16k: &[f32],
    redemption_time_ms: u32,
    mut progress_callback: F,
) -> Result<Vec<SpeechSegment>>
where
    F: FnMut(u32, usize) -> bool,
{
    let mut processor = ContinuousVadProcessor::new(16000, redemption_time_ms)?;

    let total_samples = samples_mono_16k.len();

    // For large files (>1 minute at 16kHz = 960,000 samples), process in chunks with progress logging
    const LARGE_FILE_THRESHOLD: usize = 960_000;
    const CHUNK_SIZE: usize = 160_000; // 10 seconds at 16kHz

    let mut all_segments = Vec::new();

    if total_samples > LARGE_FILE_THRESHOLD {
        info!(
            "VAD: Processing large file ({} samples = {:.1}s), will log progress...",
            total_samples,
            total_samples as f64 / 16000.0
        );

        let mut processed = 0;
        let mut last_progress = 0u32;
        let mut chunk_count = 0;
        let total_chunks = (total_samples + CHUNK_SIZE - 1) / CHUNK_SIZE;

        for chunk in samples_mono_16k.chunks(CHUNK_SIZE) {
            chunk_count += 1;

            let start_time = std::time::Instant::now();
            let segments = processor.process_audio(chunk)?;
            let elapsed = start_time.elapsed();

            // Debug log for chunk processing details
            debug!(
                "VAD: Chunk {}/{} processed in {:?}, found {} segments",
                chunk_count,
                total_chunks,
                elapsed,
                segments.len()
            );

            // Warn if chunk processing took too long (>1 second)
            if elapsed.as_secs() > 1 {
                warn!(
                    "VAD: Chunk {} took {:?} - possible performance issue",
                    chunk_count, elapsed
                );
            }

            all_segments.extend(segments);

            processed += chunk.len();
            let progress = ((processed * 100) / total_samples) as u32;

            // Call progress callback every 5%
            if progress >= last_progress + 5 {
                debug!(
                    "VAD: Progress {}% ({} segments found so far)",
                    progress,
                    all_segments.len()
                );

                // Check for cancellation
                if !progress_callback(progress, all_segments.len()) {
                    info!("VAD: Cancelled by callback at {}%", progress);
                    return Err(anyhow!("VAD processing cancelled"));
                }

                last_progress = progress;
            }
        }

        let final_segments = processor.flush()?;
        all_segments.extend(final_segments);

        info!(
            "VAD: Complete! Found {} speech segments",
            all_segments.len()
        );
    } else {
        // Small file - process all at once
        all_segments = processor.process_audio(samples_mono_16k)?;
        let final_segments = processor.flush()?;
        all_segments.extend(final_segments);
    }

    Ok(all_segments)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn acceptance_fixture_root() -> std::path::PathBuf {
        std::env::var_os("MEETILY_QA_FIXTURE_DIR")
            .map(std::path::PathBuf::from)
            .expect("Set MEETILY_QA_FIXTURE_DIR to the private audio acceptance fixture directory")
    }

    #[test]
    #[ignore = "requires private acceptance audio; see docs/releases/validation.md"]
    fn flush_excludes_padding_from_real_audio_and_timestamps() {
        let fixtures = acceptance_fixture_root();
        for case in ["A", "B", "C"] {
            let input: serde_json::Value = serde_json::from_slice(
                &std::fs::read(fixtures.join(format!("MAIN-013-{case}-20s-input.json"))).unwrap(),
            )
            .unwrap();
            let audio: Vec<f32> =
                serde_json::from_value(input["payload"]["audioData"].clone()).unwrap();
            let mut processor = ContinuousVadProcessor::new(16_000, 1_200)
                .unwrap()
                .with_adaptive_live_segment_duration_ms(3_500, 5_000);
            for frame in audio.chunks(800) {
                processor.process_audio(frame).unwrap();
            }
            let tail = processor.flush().unwrap();
            assert!(!tail.is_empty(), "{case}: must exercise final flush");
            for segment in tail {
                let start = (segment.start_timestamp_ms * 16.0).round() as usize;
                let end = (segment.end_timestamp_ms * 16.0).round() as usize;
                assert!(
                    end <= audio.len(),
                    "{case}: tail {end} exceeds real input {}",
                    audio.len()
                );
                assert_eq!(
                    segment.samples.len(),
                    end - start,
                    "{case}: timestamp/sample count"
                );
                assert!(
                    segment
                        .samples
                        .iter()
                        .zip(&audio[start..end])
                        .all(|(a, b)| a == b),
                    "{case}: tail must preserve every real sample, excluding VAD padding"
                );
            }
            assert!(
                processor.flush().unwrap().is_empty(),
                "{case}: repeated flush duplicated audio"
            );
        }
    }

    #[test]
    #[ignore = "requires private acceptance audio; see docs/releases/validation.md"]
    fn live_vad_samples_match_their_source_timestamps() {
        let fixture = acceptance_fixture_root().join("MAIN-007-客户端验收/fixed-zh.wav");
        let decoded = crate::audio::decoder::decode_audio_file(&fixture).unwrap();
        assert_eq!((decoded.sample_rate, decoded.channels), (16_000, 1));
        let audio = decoded.samples;
        let mut processor = ContinuousVadProcessor::new(16_000, 1_200)
            .unwrap()
            .with_adaptive_live_segment_duration_ms(3_500, 5_000);
        let mut segments = Vec::new();
        for frame in audio.chunks(800) {
            segments.extend(processor.process_audio(frame).unwrap());
        }
        segments.extend(processor.flush().unwrap());
        assert!(
            segments.len() >= 2,
            "Must exercise a real live split and its tail"
        );
        for (index, segment) in segments.iter().enumerate() {
            let start = (segment.start_timestamp_ms * 16.0).round() as usize;
            let end = (segment.end_timestamp_ms * 16.0).round() as usize;
            assert_eq!(
                end - start,
                segment.samples.len(),
                "segment {index} duration"
            );
            let expected = &audio[start..end];
            let max_error = segment
                .samples
                .iter()
                .zip(expected)
                .map(|(actual, source)| (actual - source).abs())
                .fold(0.0_f32, f32::max);
            assert_eq!(
                max_error, 0.0,
                "segment {index} at {}..{}ms is not its claimed source audio",
                segment.start_timestamp_ms, segment.end_timestamp_ms
            );
        }
    }

    /// Generate synthetic speech-like audio with alternating speech/silence
    fn generate_test_audio_with_speech(duration_seconds: f32, sample_rate: u32) -> Vec<f32> {
        let total_samples = (duration_seconds * sample_rate as f32) as usize;
        let mut samples = vec![0.0f32; total_samples];

        // Create speech-like patterns: bursts of sine waves with varying amplitude
        // Speech every 10 seconds for 5 seconds
        let speech_interval = 10.0; // seconds between speech starts
        let speech_duration = 5.0; // seconds of speech

        for i in 0..total_samples {
            let time = i as f32 / sample_rate as f32;
            let cycle_time = time % speech_interval;

            // Speech occurs in the first `speech_duration` seconds of each cycle
            if cycle_time < speech_duration {
                // Generate speech-like signal: multiple frequencies with amplitude modulation
                let freq1 = 200.0 + (time * 50.0).sin() * 100.0; // Varying fundamental
                let freq2 = freq1 * 2.0; // Harmonic
                let freq3 = freq1 * 3.0; // Another harmonic

                let amplitude = 0.3 + 0.1 * (time * 5.0).sin(); // Amplitude modulation
                samples[i] = amplitude
                    * (0.5 * (2.0 * std::f32::consts::PI * freq1 * time).sin()
                        + 0.3 * (2.0 * std::f32::consts::PI * freq2 * time).sin()
                        + 0.2 * (2.0 * std::f32::consts::PI * freq3 * time).sin());
            }
            // else: silence (already 0.0)
        }

        samples
    }

    #[test]
    fn test_vad_chunked_vs_single_processing() {
        // Generate 60 seconds of audio with speech patterns at 16kHz
        let audio = generate_test_audio_with_speech(60.0, 16000);
        println!(
            "Generated {} samples ({:.1}s)",
            audio.len(),
            audio.len() as f32 / 16000.0
        );

        // Process all at once (like small files)
        let segments_single = get_speech_chunks(&audio, 2000).expect("Single processing failed");
        println!("Single processing found {} segments", segments_single.len());

        // Process in chunks (like large files)
        let segments_chunked =
            get_speech_chunks_with_progress(&audio, 2000, |progress, segments| {
                println!("Chunked progress: {}%, {} segments", progress, segments);
                true // Don't cancel
            })
            .expect("Chunked processing failed");
        println!(
            "Chunked processing found {} segments",
            segments_chunked.len()
        );

        // Both should find the same number of segments (approximately)
        // Allow some variance due to chunk boundary effects
        let diff = (segments_single.len() as i32 - segments_chunked.len() as i32).abs();
        assert!(
            diff <= 1,
            "Chunked and single processing found different segment counts: {} vs {} (diff: {})",
            segments_single.len(),
            segments_chunked.len(),
            diff
        );
    }

    #[test]
    fn test_vad_large_file_progress() {
        // Generate 120 seconds (2 minutes) of audio - triggers large file threshold
        let audio = generate_test_audio_with_speech(120.0, 16000);
        let total_samples = audio.len();
        println!(
            "Generated {} samples ({:.1}s)",
            total_samples,
            total_samples as f32 / 16000.0
        );

        // This should trigger the large file path (>960,000 samples)
        assert!(
            total_samples > 960_000,
            "Audio should be large enough to trigger chunked processing"
        );

        let mut progress_updates = Vec::new();
        let segments = get_speech_chunks_with_progress(&audio, 2000, |progress, segments| {
            progress_updates.push((progress, segments));
            true // Don't cancel
        })
        .expect("Processing failed");

        println!(
            "Found {} segments with {} progress updates",
            segments.len(),
            progress_updates.len()
        );

        // The synthetic signal is not real speech, so Silero may merge it into
        // one long segment. This test is specifically for the large-file path:
        // it must still emit speech and report monotonic progress through 100%.
        assert!(!segments.is_empty(), "Expected at least one speech segment");
        assert!(
            segments.iter().all(|segment| !segment.samples.is_empty()
                && segment.end_timestamp_ms > segment.start_timestamp_ms),
            "Expected all speech segments to contain audio with positive duration"
        );

        // Should have received progress updates
        assert!(
            !progress_updates.is_empty(),
            "Expected progress updates for large file"
        );
        assert_eq!(
            progress_updates.last().map(|(progress, _)| *progress),
            Some(100),
            "Expected progress to reach 100%"
        );
        assert!(
            progress_updates
                .windows(2)
                .all(|pair| pair[0].0 < pair[1].0),
            "Expected progress updates to increase monotonically: {:?}",
            progress_updates
        );
    }

    #[test]
    fn test_vad_cancellation() {
        let audio = generate_test_audio_with_speech(120.0, 16000);

        // Cancel at 50%
        let result = get_speech_chunks_with_progress(&audio, 2000, |progress, _| {
            progress < 50 // Cancel when reaching 50%
        });

        // Should return error due to cancellation
        assert!(result.is_err(), "Expected cancellation error");
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("cancelled"),
            "Error should mention cancellation: {}",
            err_msg
        );
    }

    #[test]
    fn test_vad_continuous_processor_state_across_chunks() {
        // Test that VAD state is correctly maintained across chunk boundaries
        let mut processor =
            ContinuousVadProcessor::new(16000, 2000).expect("Failed to create processor");

        // Generate audio with a speech segment that spans a chunk boundary
        let chunk_size = 160_000; // 10 seconds
        let audio = generate_test_audio_with_speech(30.0, 16000); // 30 seconds

        // Process in 10-second chunks
        let mut all_segments = Vec::new();
        for (i, chunk) in audio.chunks(chunk_size).enumerate() {
            let segments = processor.process_audio(chunk).expect("Processing failed");
            println!(
                "Chunk {}: processed {} samples, found {} segments",
                i,
                chunk.len(),
                segments.len()
            );
            all_segments.extend(segments);
        }

        // Flush remaining
        let final_segments = processor.flush().expect("Flush failed");
        all_segments.extend(final_segments);

        println!("Total segments found: {}", all_segments.len());

        // Should find speech segments
        assert!(
            all_segments.len() >= 1,
            "Expected at least 1 speech segment"
        );
    }

    #[test]
    fn test_vad_400ms_vs_2000ms_segmentation() {
        // Demonstrates why 2000ms redemption is needed for batch processing:
        // 400ms creates excessive fragmentation, 2000ms bridges natural pauses.
        //
        // Audio pattern: 60s with 5s speech / 5s silence cycles
        // Natural pauses within speech (sentence gaps) are 500ms-1.5s
        let audio = generate_test_audio_with_speech(60.0, 16000);

        let segments_400 = get_speech_chunks(&audio, 400).expect("400ms processing failed");
        let segments_2000 = get_speech_chunks(&audio, 2000).expect("2000ms processing failed");

        println!(
            "400ms redemption: {} segments, 2000ms redemption: {} segments",
            segments_400.len(),
            segments_2000.len()
        );

        // 2000ms should produce fewer or equal segments (bridges more pauses)
        assert!(
            segments_2000.len() <= segments_400.len(),
            "2000ms redemption ({} segments) should not produce more segments than 400ms ({} segments)",
            segments_2000.len(),
            segments_400.len()
        );

        // Verify segments have reasonable durations with 2000ms
        for (i, seg) in segments_2000.iter().enumerate() {
            let duration_ms = seg.end_timestamp_ms - seg.start_timestamp_ms;
            println!("2000ms segment {}: {:.0}ms duration", i, duration_ms);
            // Each segment should be at least 250ms (min_speech_time)
            assert!(
                duration_ms >= 200.0,
                "Segment {} too short: {:.0}ms",
                i,
                duration_ms
            );
        }
    }

    #[test]
    fn test_live_latency_bound_emits_complete_four_second_chunks() {
        let mut processor = ContinuousVadProcessor::new(16_000, 400)
            .expect("Failed to create processor")
            .with_max_segment_duration_ms(4_000);

        processor.in_speech = true;
        processor.speech_start_sample = 1_600; // 100ms into the recording
        processor.current_speech = vec![0.25; 64_480]; // 4 seconds + one 30ms VAD frame
        processor.emit_latency_bounded_segments();

        let emitted = processor
            .speech_segments
            .pop_front()
            .expect("Expected a latency-bounded segment");
        assert_eq!(emitted.samples.len(), 64_000);
        assert_eq!(emitted.start_timestamp_ms, 100.0);
        assert_eq!(emitted.end_timestamp_ms, 4_100.0);
        assert_eq!(processor.current_speech.len(), 480);
        assert_eq!(processor.speech_start_sample, 65_600);
        assert!(processor.streaming_split_active);
    }

    #[test]
    fn test_adaptive_live_latency_uses_four_then_six_second_bounds() {
        let mut processor = ContinuousVadProcessor::new(16_000, 1_200)
            .expect("Failed to create processor")
            .with_adaptive_live_segment_duration_ms(4_000, 6_000);

        processor.in_speech = true;
        processor.current_speech = vec![0.25; 64_000];
        processor.emit_latency_bounded_segments();
        let first = processor
            .speech_segments
            .pop_front()
            .expect("Expected the first live segment");
        assert_eq!(first.samples.len(), 64_000);
        assert_eq!(processor.emitted_live_segments, 1);

        processor.current_speech.extend(vec![0.25; 96_000]);
        processor.emit_latency_bounded_segments();
        let second = processor
            .speech_segments
            .pop_front()
            .expect("Expected the subsequent live segment");
        assert_eq!(second.samples.len(), 96_000);
        assert_eq!(processor.emitted_live_segments, 2);
        assert!(processor.current_speech.is_empty());
    }

    #[test]
    fn test_live_split_ignores_quiet_point_far_before_duration_limit() {
        let mut processor = ContinuousVadProcessor::new(16_000, 1_200)
            .expect("Failed to create processor")
            .with_max_segment_duration_ms(4_000);

        processor.in_speech = true;
        processor.current_speech = vec![0.25; 64_000];
        // The former two-second search window selected this early pause and
        // produced a context-poor two-second fragment. It must now be ignored.
        processor.current_speech[32_000..32_320].fill(0.0);
        processor.current_speech[56_000..56_320].fill(0.0);
        processor.emit_latency_bounded_segments();

        let emitted = processor
            .speech_segments
            .pop_front()
            .expect("Expected a latency-bounded segment");
        assert_eq!(emitted.samples.len(), 56_160);
        assert!(emitted.samples.len() >= 51_200); // final 20% of four seconds
    }

    #[test]
    fn test_silero_absolute_timestamp_is_not_double_counted() {
        // silero-rs documents SpeechStart timestamps as absolute offsets from
        // the beginning of the VAD session. Ten seconds must therefore map to
        // exactly 160,000 samples regardless of how much audio is already read.
        assert_eq!(absolute_vad_sample(10_000), 160_000);
        assert_eq!(absolute_vad_sample(61_250), 980_000);
    }

    #[test]
    fn test_live_latency_bound_prefers_quiet_word_boundary() {
        let mut processor = ContinuousVadProcessor::new(16_000, 400)
            .expect("Failed to create processor")
            .with_max_segment_duration_ms(8_000);

        processor.in_speech = true;
        processor.speech_start_sample = 0;
        processor.current_speech = vec![0.25; 128_000];
        // A 20ms pause at 7 seconds should be preferred over an 8-second
        // hard cut because it is much less likely to bisect a spoken word.
        processor.current_speech[112_000..112_320].fill(0.0);
        processor.emit_latency_bounded_segments();

        let emitted = processor
            .speech_segments
            .pop_front()
            .expect("Expected a latency-bounded segment");
        assert_eq!(emitted.samples.len(), 112_160);
        assert_eq!(emitted.start_timestamp_ms, 0.0);
        assert_eq!(emitted.end_timestamp_ms, 7_010.0);
        assert_eq!(processor.current_speech.len(), 15_840);
        assert_eq!(processor.speech_start_sample, 112_160);
    }
}

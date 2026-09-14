//! Per-recording audio context and one revisable transcript tail.
use super::boundary::{join_boundary, TimedToken};
use super::recognizer::{decode_timed, TimedRecognition};
use super::tail::{TranscriptDraft, TranscriptTail};
use anyhow::{anyhow, Result};
use sherpa_onnx::OfflineRecognizer;
use std::sync::Arc;

const SAMPLE_RATE: usize = 16_000;
const CONTEXT_SAMPLES: usize = 12 * SAMPLE_RATE;
const CHUNK_SAMPLES: usize = 25 * SAMPLE_RATE;

#[derive(Clone)]
struct AudioContext {
    samples: Vec<f32>,
    start: f64,
    end: f64,
    epoch: u64,
    timed: bool,
}

pub struct LiveResult {
    pub updates: Vec<TranscriptDraft>,
    pub boundary_status: Vec<&'static str>,
    pub raw_texts: Vec<String>,
}

#[derive(Clone)]
pub struct SenseVoiceSession {
    recognizer: Arc<OfflineRecognizer>,
    previous: Option<AudioContext>,
    tail: TranscriptTail,
}

impl SenseVoiceSession {
    pub fn new(recognizer: Arc<OfflineRecognizer>) -> Self {
        Self {
            recognizer,
            previous: None,
            tail: TranscriptTail::default(),
        }
    }

    /// Call on a blocking thread; each decode is bounded even for a large input.
    pub fn push(&mut self, audio: &[f32], start: f64, epoch: u64) -> Result<LiveResult> {
        let recognizer = self.recognizer.clone();
        self.push_with(audio, start, epoch, &mut |samples, offset| {
            decode_timed(&recognizer, samples, offset)
        })
    }

    pub(crate) fn push_with<F>(
        &mut self,
        audio: &[f32],
        start: f64,
        epoch: u64,
        decode: &mut F,
    ) -> Result<LiveResult>
    where
        F: FnMut(&[f32], f64) -> Result<TimedRecognition>,
    {
        if !start.is_finite() || start < 0.0 {
            return Err(anyhow!("Invalid live audio timestamp"));
        }
        // Do not consume an unpublished tail if a later bounded decode fails.
        let mut staged = self.clone();
        let mut result = LiveResult {
            updates: Vec::new(),
            boundary_status: Vec::new(),
            raw_texts: Vec::new(),
        };
        for (index, chunk) in audio.chunks(CHUNK_SAMPLES).enumerate() {
            let next = staged.push_chunk(
                chunk,
                start + (index * CHUNK_SAMPLES) as f64 / SAMPLE_RATE as f64,
                epoch,
                decode,
            )?;
            result.updates.extend(next.updates);
            result.boundary_status.extend(next.boundary_status);
            result.raw_texts.extend(next.raw_texts);
        }
        *self = staged;
        Ok(result)
    }

    fn push_chunk<F>(
        &mut self,
        audio: &[f32],
        start: f64,
        epoch: u64,
        decode: &mut F,
    ) -> Result<LiveResult>
    where
        F: FnMut(&[f32], f64) -> Result<TimedRecognition>,
    {
        let end = start + audio.len() as f64 / SAMPLE_RATE as f64;
        let decoded = decode(audio, start)?;
        let raw_text = decoded.text.clone();
        let timed = decoded.tokens.is_some();
        let current = decoded.tokens.unwrap_or_else(|| {
            if decoded.text.is_empty() {
                Vec::new()
            } else {
                vec![TimedToken {
                    text: decoded.text,
                    time: start,
                }]
            }
        });
        let mut result = LiveResult {
            updates: Vec::new(),
            boundary_status: Vec::new(),
            raw_texts: vec![raw_text],
        };
        let continuous = self
            .previous
            .as_ref()
            .is_some_and(|p| p.epoch == epoch && (p.end - start).abs() <= 0.005);
        if !continuous || !timed || self.previous.as_ref().is_some_and(|p| !p.timed) {
            result.updates.extend(self.tail.finish());
            result
                .updates
                .extend(self.tail.accept(current, start, end, false));
            result.boundary_status.push(if !timed {
                "timing_unavailable"
            } else {
                "new_audio_span"
            });
        } else if let (Some(previous), Some(pending)) = (&self.previous, self.tail.pending()) {
            let next_count = audio.len().min(CONTEXT_SAMPLES);
            let bridge_audio = [previous.samples.as_slice(), &audio[..next_count]].concat();
            let bridge_end = start + next_count as f64 / SAMPLE_RATE as f64;
            let joined = decode(&bridge_audio, previous.start)
                .ok()
                .and_then(|r| r.tokens)
                .map(|bridge| {
                    join_boundary(
                        &pending.tokens,
                        &current,
                        &bridge,
                        previous.start,
                        bridge_end,
                        start,
                        (pending.audio_start - previous.start).abs() <= 1.0 / SAMPLE_RATE as f64,
                        next_count == audio.len(),
                    )
                });
            let (tokens, status) = match joined {
                Some(joined) => (joined.tokens, joined.status),
                None => (
                    pending.tokens.iter().chain(&current).cloned().collect(),
                    "bridge_unavailable",
                ),
            };
            result
                .updates
                .extend(self.tail.accept(tokens, start, end, status == "bridged"));
            result.boundary_status.push(status);
        } else {
            result
                .updates
                .extend(self.tail.accept(current, start, end, false));
            result.boundary_status.push("new_transcript");
        }
        let retained = audio.len().min(CONTEXT_SAMPLES);
        self.previous = Some(AudioContext {
            samples: audio[audio.len() - retained..].to_vec(),
            start: end - retained as f64 / SAMPLE_RATE as f64,
            end,
            epoch,
            timed,
        });
        Ok(result)
    }

    pub fn finish(&mut self) -> Vec<TranscriptDraft> {
        self.previous = None;
        self.tail.finish()
    }
}

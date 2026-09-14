use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::audio::common::split_segment_at_silence;
use crate::audio::decoder::decode_audio_file;
use crate::audio::vad::get_speech_chunks;
use crate::database::moss::{CandidateAlignmentInput, CandidateSegmentInput};
use crate::meeting_context::recognition::WHISPER_INITIAL_PROMPT_MAX_CHARS;
use crate::moss_alignment::{align_candidate_segments, ProductAlignment};
use crate::moss_helper::manager::ManagedTranscription;
use crate::whisper_engine::acceleration::WhisperCompiledBackend;
use crate::whisper_engine::hotword_bias::{
    validate_hotword_bias_diagnostics, WhisperHotwordBiasConfig, WhisperHotwordBiasDiagnostics,
    R10_HOTWORD_BIAS_SCHEMA_VERSION, R10_HOTWORD_BIAS_STRATEGY, R10_HOTWORD_TOKENIZATION_VARIANTS,
    R10_MAX_TOKENS_PER_SEQUENCE,
};
use crate::whisper_engine::{WhisperEngine, WhisperTimedToken, WHISPER_TIMESTAMP_TICK_MS};

pub const ALIGNMENT_METHOD_AUDIO_TOKEN: &str = "whisper_audio_token";
pub const R5_ALIGNMENT_MODEL_NAME: &str = "large-v3-turbo-q5_0";
pub const R5_TOKEN_TRACK_SCHEMA_VERSION: u8 = 1;
pub const R5_TOKEN_ALIGNMENT_PARAMETERS_SCHEMA_VERSION: u8 = 1;
pub const R8_CONTEXTUAL_TOKEN_ALIGNMENT_PARAMETERS_SCHEMA_VERSION: u8 = 2;
pub const R10_HOTWORD_TOKEN_ALIGNMENT_PARAMETERS_SCHEMA_VERSION: u8 = 3;
pub const R5_MAX_ALIGNED_PIECE_MS: i64 = 2_000;
pub const R5_MIN_GLOBAL_MATCH_COVERAGE: f64 = 0.50;
pub const R5_MIN_RAW_SEGMENT_MATCH_COVERAGE: f64 = 0.45;
pub const R5_MIN_TERM_TOKEN_PROBABILITY: f32 = 0.50;

const WHISPER_SAMPLE_RATE: usize = 16_000;
const VAD_REDEMPTION_TIME_MS: u32 = 2_000;
const MAX_SEGMENT_SAMPLES: usize = 25 * WHISPER_SAMPLE_RATE;
const MIN_SEGMENT_SAMPLES: usize = 1_600;
const MAX_ALIGNMENT_CELLS: usize = 24_000_000;
const MAX_MACHINE_CORRECTION_CHARACTERS: usize = 32;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AudioTokenAlignmentParameters {
    pub schema_version: u8,
    pub language: String,
    pub sample_rate_hz: u32,
    pub channels: u8,
    pub vad_redemption_time_ms: u32,
    pub maximum_segment_samples: usize,
    pub minimum_segment_samples: usize,
    pub timestamp_tick_ms: i64,
    pub no_timestamps: bool,
    pub token_timestamps: bool,
    pub split_on_word: bool,
    pub initial_prompt_used: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_sha256: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_sha256: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_chars: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_truncated: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hotword_bias: Option<AudioTokenHotwordBiasParameters>,
    pub synthetic_character_timing: bool,
    pub human_truth_used: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AudioTokenHotwordBiasParameters {
    pub schema_version: u8,
    pub strategy: String,
    pub binding_sha256: String,
    pub canonical_terms_sha256: String,
    pub canonical_term_count: usize,
    pub token_sequence_set_sha256: String,
    pub token_sequence_count: usize,
    pub tokenization_variants: Vec<String>,
    pub maximum_tokens_per_sequence: usize,
    pub start_token_logit_add: f32,
    pub continuation_token_logit_add: f32,
    pub completion_token_logit_add: f32,
    pub duplicate_adjustment_rule: String,
}

impl AudioTokenHotwordBiasParameters {
    fn from_diagnostics(diagnostics: &WhisperHotwordBiasDiagnostics) -> Self {
        Self {
            schema_version: diagnostics.schema_version,
            strategy: diagnostics.strategy.clone(),
            binding_sha256: diagnostics.binding_sha256.clone(),
            canonical_terms_sha256: diagnostics.canonical_terms_sha256.clone(),
            canonical_term_count: diagnostics.canonical_term_count,
            token_sequence_set_sha256: diagnostics.token_sequence_set_sha256.clone(),
            token_sequence_count: diagnostics.token_sequence_count,
            tokenization_variants: diagnostics.tokenization_variants.clone(),
            maximum_tokens_per_sequence: diagnostics.maximum_tokens_per_sequence,
            start_token_logit_add: diagnostics.start_token_logit_add,
            continuation_token_logit_add: diagnostics.continuation_token_logit_add,
            completion_token_logit_add: diagnostics.completion_token_logit_add,
            duplicate_adjustment_rule: diagnostics.duplicate_adjustment_rule.clone(),
        }
    }
}

impl AudioTokenAlignmentParameters {
    pub fn product(language: impl Into<String>) -> Self {
        Self {
            schema_version: R5_TOKEN_ALIGNMENT_PARAMETERS_SCHEMA_VERSION,
            language: language.into(),
            sample_rate_hz: WHISPER_SAMPLE_RATE as u32,
            channels: 1,
            vad_redemption_time_ms: VAD_REDEMPTION_TIME_MS,
            maximum_segment_samples: MAX_SEGMENT_SAMPLES,
            minimum_segment_samples: MIN_SEGMENT_SAMPLES,
            timestamp_tick_ms: WHISPER_TIMESTAMP_TICK_MS,
            no_timestamps: false,
            token_timestamps: true,
            split_on_word: true,
            initial_prompt_used: false,
            context_sha256: None,
            prompt_sha256: None,
            prompt_chars: None,
            prompt_truncated: None,
            hotword_bias: None,
            synthetic_character_timing: false,
            human_truth_used: false,
        }
    }

    fn contextual(
        language: impl Into<String>,
        context_sha256: impl Into<String>,
        prompt_sha256: impl Into<String>,
        prompt_chars: usize,
        prompt_truncated: bool,
    ) -> Self {
        let mut parameters = Self::product(language);
        parameters.schema_version = R8_CONTEXTUAL_TOKEN_ALIGNMENT_PARAMETERS_SCHEMA_VERSION;
        parameters.initial_prompt_used = true;
        parameters.context_sha256 = Some(context_sha256.into());
        parameters.prompt_sha256 = Some(prompt_sha256.into());
        parameters.prompt_chars = Some(prompt_chars);
        parameters.prompt_truncated = Some(prompt_truncated);
        parameters
    }

    fn hotword_biased(
        language: impl Into<String>,
        context_sha256: impl Into<String>,
        diagnostics: &WhisperHotwordBiasDiagnostics,
    ) -> Self {
        let mut parameters = Self::product(language);
        parameters.schema_version = R10_HOTWORD_TOKEN_ALIGNMENT_PARAMETERS_SCHEMA_VERSION;
        parameters.context_sha256 = Some(context_sha256.into());
        parameters.hotword_bias = Some(AudioTokenHotwordBiasParameters::from_diagnostics(
            diagnostics,
        ));
        parameters
    }
}

#[derive(Debug, Clone, Copy)]
pub struct AudioTokenContextPrompt<'a> {
    pub text: &'a str,
    pub context_sha256: &'a str,
    pub truncated: bool,
}

#[derive(Debug, Clone, Copy)]
pub struct AudioTokenHotwordBias<'a> {
    pub context_sha256: &'a str,
    pub canonical_terms: &'a [String],
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AudioToken {
    pub global_token_index: u32,
    pub chunk_index: u32,
    pub whisper_segment_index: u32,
    pub whisper_token_index: u32,
    pub start_ms: i64,
    pub end_ms: i64,
    pub text: String,
    pub probability: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AudioTokenSourceChunk {
    pub chunk_index: u32,
    pub start_ms: i64,
    pub end_ms: i64,
    pub sample_count: usize,
    pub text_sha256: String,
    pub first_global_token_index: u32,
    pub token_count: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AudioTokenTrack {
    pub schema_version: u8,
    pub audio_sha256: String,
    pub audio_duration_ms: i64,
    pub model_name: String,
    pub model_sha256: String,
    pub program_sha256: String,
    pub parameters_sha256: String,
    pub token_track_sha256: String,
    pub backend: String,
    pub parameters: AudioTokenAlignmentParameters,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hotword_bias_diagnostics: Option<WhisperHotwordBiasDiagnostics>,
    pub source_chunks: Vec<AudioTokenSourceChunk>,
    pub tokens: Vec<AudioToken>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ExpectedAudioTokenBinding<'a> {
    pub audio_sha256: &'a str,
    pub model_sha256: &'a str,
    pub program_sha256: &'a str,
    pub parameters_sha256: &'a str,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CandidateAudioTokenBoundary {
    pub segment_index: u32,
    pub first_token_index: u32,
    pub last_token_index: u32,
    pub token_track_sha256: String,
    pub confidence: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MachineTermSuggestion {
    pub segment_index: u32,
    pub start_char: usize,
    pub end_char: usize,
    pub original_text: String,
    pub replacement_text: String,
    pub term_id: String,
    pub context_sha256: String,
    pub token_track_sha256: String,
    pub model_sha256: String,
    pub first_token_index: u32,
    pub last_token_index: u32,
    pub confidence: f64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContextTerm {
    pub term_id: String,
    pub canonical: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct R5ProductAlignment {
    pub product: ProductAlignment,
    pub token_boundaries: Vec<CandidateAudioTokenBoundary>,
    pub token_aligned_segment_count: u32,
    pub token_fallback_raw_segment_count: u32,
    pub token_global_match_coverage: Option<f64>,
    pub token_track_sha256: Option<String>,
    pub token_fallback_reason: Option<String>,
    pub piece_raw_ranges: Vec<PieceRawRange>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PieceRawRange {
    pub output_segment_index: u32,
    pub raw_segment_index: u32,
    pub raw_start_char: usize,
    pub raw_end_char: usize,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum AudioTokenError {
    #[error("audio token track binding is invalid: {0}")]
    InvalidBinding(&'static str),
    #[error("audio token text alignment failed: {0}")]
    Alignment(&'static str),
}

#[derive(Debug, Clone)]
struct NormalizedChar {
    value: char,
    original_char_index: usize,
    owner_index: usize,
}

#[derive(Debug, Clone)]
struct RawTokenMatch {
    raw_char_index: usize,
    token_index: usize,
}

#[derive(Debug, Clone)]
struct ProposedPiece {
    text: String,
    raw_start_char: usize,
    raw_end_char: usize,
    start_ms: i64,
    end_ms: i64,
    first_token_index: usize,
    last_token_index: usize,
    confidence: f64,
}

pub fn sha256_file(path: &Path) -> Result<String> {
    let mut stream = fs::File::open(path)?;
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; 1024 * 1024];
    loop {
        let count = stream.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        digest.update(&buffer[..count]);
    }
    Ok(format!("{:x}", digest.finalize()))
}

pub fn sha256_json<T: Serialize>(value: &T) -> Result<String> {
    Ok(format!("{:x}", Sha256::digest(serde_json::to_vec(value)?)))
}

#[derive(Serialize)]
struct AudioTokenTrackHashPayload<'a> {
    schema_version: u8,
    source_chunks: &'a [AudioTokenSourceChunk],
    tokens: &'a [AudioToken],
}

pub fn sha256_audio_token_track(
    source_chunks: &[AudioTokenSourceChunk],
    tokens: &[AudioToken],
) -> Result<String> {
    sha256_json(&AudioTokenTrackHashPayload {
        schema_version: R5_TOKEN_TRACK_SCHEMA_VERSION,
        source_chunks,
        tokens,
    })
}

pub async fn build_audio_token_track(
    audio_path: &Path,
    models_dir: &Path,
    model_name: &str,
    language: &str,
) -> Result<AudioTokenTrack> {
    build_audio_token_track_internal(audio_path, models_dir, model_name, language, None, None).await
}

pub async fn build_audio_token_track_with_context_prompt(
    audio_path: &Path,
    models_dir: &Path,
    model_name: &str,
    language: &str,
    prompt: AudioTokenContextPrompt<'_>,
) -> Result<AudioTokenTrack> {
    build_audio_token_track_internal(
        audio_path,
        models_dir,
        model_name,
        language,
        Some(prompt),
        None,
    )
    .await
}

pub async fn build_audio_token_track_with_hotword_bias(
    audio_path: &Path,
    models_dir: &Path,
    model_name: &str,
    language: &str,
    hotword_bias: AudioTokenHotwordBias<'_>,
) -> Result<AudioTokenTrack> {
    build_audio_token_track_internal(
        audio_path,
        models_dir,
        model_name,
        language,
        None,
        Some(hotword_bias),
    )
    .await
}

async fn build_audio_token_track_internal(
    audio_path: &Path,
    models_dir: &Path,
    model_name: &str,
    language: &str,
    prompt: Option<AudioTokenContextPrompt<'_>>,
    hotword_bias: Option<AudioTokenHotwordBias<'_>>,
) -> Result<AudioTokenTrack> {
    if model_name != R5_ALIGNMENT_MODEL_NAME {
        return Err(anyhow!(
            "R5 Whisper alignment model does not match the frozen model"
        ));
    }
    if prompt.is_some() && hotword_bias.is_some() {
        return Err(anyhow!(
            "contextual Whisper prompt and hotword bias are mutually exclusive"
        ));
    }
    let (initial_prompt, prompt_sha256, prompt_chars) = match prompt {
        Some(prompt) => {
            let prompt_chars = prompt.text.chars().count();
            if prompt.text.trim().is_empty()
                || prompt.text.trim() != prompt.text
                || prompt_chars > WHISPER_INITIAL_PROMPT_MAX_CHARS
                || !is_sha256(prompt.context_sha256)
            {
                return Err(anyhow!("invalid bounded contextual Whisper prompt"));
            }
            let prompt_sha256 = sha256_text(prompt.text);
            (
                Some(prompt.text.to_owned()),
                Some(prompt_sha256),
                Some(prompt_chars),
            )
        }
        None => (None, None, None),
    };
    if let Some(hotword_bias) = hotword_bias {
        if !is_sha256(hotword_bias.context_sha256) || hotword_bias.canonical_terms.is_empty() {
            return Err(anyhow!("invalid bounded Whisper hotword bias input"));
        }
    }
    let executable = fs::canonicalize(std::env::current_exe()?)?;
    let program_sha256 = sha256_file(&executable).context("program hash")?;
    let model_path = models_dir.join(format!("ggml-{model_name}.bin"));
    if !model_path.is_file() {
        return Err(anyhow!("R5 Whisper alignment model is not installed"));
    }
    let audio_sha256 = sha256_file(audio_path).context("audio hash")?;
    let model_sha256 = sha256_file(&model_path).context("model hash")?;
    let decoded = decode_audio_file(audio_path).context("audio decode")?;
    let samples = decoded.to_whisper_format();
    let audio_duration_ms = sample_duration_ms(samples.len())?;
    let speech_segments = get_speech_chunks(&samples, VAD_REDEMPTION_TIME_MS).context("VAD")?;
    if speech_segments.is_empty() {
        return Err(anyhow!("R5 Whisper alignment found no speech"));
    }
    let mut chunks = Vec::new();
    for segment in &speech_segments {
        if segment.samples.len() > MAX_SEGMENT_SAMPLES {
            chunks.extend(split_segment_at_silence(segment, MAX_SEGMENT_SAMPLES, 0));
        } else {
            chunks.push(segment.clone());
        }
    }
    let engine = WhisperEngine::new_with_models_dir(models_dir.to_path_buf())?;
    engine.discover_models().await?;
    engine.load_model(model_name).await?;

    let mut tokens = Vec::new();
    let mut source_chunks = Vec::new();
    let mut hotword_bias_diagnostics: Option<WhisperHotwordBiasDiagnostics> = None;
    for chunk in &chunks {
        if chunk.samples.len() < MIN_SEGMENT_SAMPLES {
            continue;
        }
        let chunk_index = u32::try_from(source_chunks.len())?;
        let (chunk_start_ms, chunk_end_ms) = validated_chunk_range(
            chunk.start_timestamp_ms,
            chunk.end_timestamp_ms,
            audio_duration_ms,
        )?;
        let decode = match hotword_bias {
            Some(hotword_bias) => engine
                .transcribe_audio_with_token_timestamps_and_hotword_bias(
                    chunk.samples.clone(),
                    Some(language.to_owned()),
                    hotword_bias.canonical_terms,
                    WhisperHotwordBiasConfig::r10_frozen(),
                )
                .await
                .map(|(timed, diagnostics)| (timed, Some(diagnostics))),
            None => engine
                .transcribe_audio_with_token_timestamps(
                    chunk.samples.clone(),
                    Some(language.to_owned()),
                    initial_prompt.clone(),
                )
                .await
                .map(|timed| (timed, None)),
        };
        let (timed, chunk_hotword_bias_diagnostics) = match decode {
            Ok(value) => value,
            Err(error)
                if error.chain().any(|cause| {
                    cause.to_string() == "Whisper token alignment returned no spoken tokens"
                }) =>
            {
                continue;
            }
            Err(error) => return Err(error),
        };
        if let Some(chunk_diagnostics) = chunk_hotword_bias_diagnostics {
            match hotword_bias_diagnostics.as_mut() {
                Some(diagnostics) => diagnostics.merge_decode(&chunk_diagnostics)?,
                None => hotword_bias_diagnostics = Some(chunk_diagnostics),
            }
        }
        let first_global_token_index = u32::try_from(tokens.len())?;
        for token in timed.tokens {
            let global = offset_timed_token(
                token,
                chunk_index,
                u32::try_from(tokens.len())?,
                chunk_start_ms,
                chunk_end_ms,
                audio_duration_ms,
            )?;
            if tokens.last().is_some_and(|previous: &AudioToken| {
                global.start_ms < previous.start_ms
                    || global.global_token_index != previous.global_token_index.saturating_add(1)
            }) {
                return Err(anyhow!("R5 Whisper token track is not monotonic"));
            }
            tokens.push(global);
        }
        let token_count = u32::try_from(tokens.len())?
            .checked_sub(first_global_token_index)
            .ok_or_else(|| anyhow!("R5 token count underflow"))?;
        let first_token_position = usize::try_from(first_global_token_index)?;
        let chunk_text = tokens[first_token_position..]
            .iter()
            .map(|token| token.text.as_str())
            .collect::<String>();
        source_chunks.push(AudioTokenSourceChunk {
            chunk_index,
            start_ms: chunk_start_ms,
            end_ms: chunk_end_ms,
            sample_count: chunk.samples.len(),
            text_sha256: sha256_json(&chunk_text)?,
            first_global_token_index,
            token_count,
        });
    }
    if tokens.is_empty() {
        return Err(anyhow!("R5 Whisper alignment returned no spoken tokens"));
    }
    let parameters = match (prompt, hotword_bias) {
        (Some(prompt), None) => AudioTokenAlignmentParameters::contextual(
            language,
            prompt.context_sha256,
            prompt_sha256.ok_or_else(|| anyhow!("contextual prompt hash is missing"))?,
            prompt_chars.ok_or_else(|| anyhow!("contextual prompt length is missing"))?,
            prompt.truncated,
        ),
        (None, Some(hotword_bias)) => AudioTokenAlignmentParameters::hotword_biased(
            language,
            hotword_bias.context_sha256,
            hotword_bias_diagnostics
                .as_ref()
                .ok_or_else(|| anyhow!("Whisper hotword bias diagnostics are missing"))?,
        ),
        (None, None) => AudioTokenAlignmentParameters::product(language),
        (Some(_), Some(_)) => unreachable!("mutually exclusive input checked above"),
    };
    let parameters_sha256 = sha256_json(&parameters)?;
    let token_track_sha256 = sha256_audio_token_track(&source_chunks, &tokens)?;
    let track = AudioTokenTrack {
        schema_version: R5_TOKEN_TRACK_SCHEMA_VERSION,
        audio_sha256,
        audio_duration_ms,
        model_name: model_name.to_owned(),
        model_sha256,
        program_sha256,
        parameters_sha256,
        token_track_sha256,
        backend: WhisperCompiledBackend::current().as_str().to_owned(),
        parameters,
        hotword_bias_diagnostics,
        source_chunks,
        tokens,
    };
    validate_audio_token_track(&track, None).map_err(|error| anyhow!(error))?;
    Ok(track)
}

fn sample_duration_ms(sample_count: usize) -> Result<i64> {
    i64::try_from(
        sample_count
            .checked_mul(1_000)
            .ok_or_else(|| anyhow!("audio duration overflow"))?
            .div_ceil(WHISPER_SAMPLE_RATE),
    )
    .map_err(Into::into)
}

fn validated_chunk_range(start: f64, end: f64, audio_duration_ms: i64) -> Result<(i64, i64)> {
    if !start.is_finite() || !end.is_finite() || start < 0.0 || end <= start {
        return Err(anyhow!("invalid VAD chunk timestamps"));
    }
    let start_ms = start.round() as i64;
    let end_ms = end.round() as i64;
    if start_ms < 0 || end_ms <= start_ms || end_ms > audio_duration_ms {
        return Err(anyhow!("VAD chunk exceeds the audio"));
    }
    Ok((start_ms, end_ms))
}

fn offset_timed_token(
    token: WhisperTimedToken,
    chunk_index: u32,
    global_token_index: u32,
    chunk_start_ms: i64,
    chunk_end_ms: i64,
    audio_duration_ms: i64,
) -> Result<AudioToken> {
    let start_ms = chunk_start_ms
        .checked_add(token.start_ms)
        .ok_or_else(|| anyhow!("token start overflow"))?;
    let end_ms = chunk_start_ms
        .checked_add(token.end_ms)
        .ok_or_else(|| anyhow!("token end overflow"))?;
    if start_ms < chunk_start_ms
        || end_ms < start_ms
        || end_ms > chunk_end_ms
        || end_ms > audio_duration_ms
    {
        return Err(anyhow!("token is outside its real VAD chunk"));
    }
    Ok(AudioToken {
        global_token_index,
        chunk_index,
        whisper_segment_index: token.segment_index,
        whisper_token_index: token.token_index,
        start_ms,
        end_ms,
        text: token.text,
        probability: token.probability,
    })
}

pub fn validate_audio_token_track(
    track: &AudioTokenTrack,
    expected: Option<ExpectedAudioTokenBinding<'_>>,
) -> Result<(), AudioTokenError> {
    if track.schema_version != R5_TOKEN_TRACK_SCHEMA_VERSION
        || track.audio_duration_ms <= 0
        || track.model_name != R5_ALIGNMENT_MODEL_NAME
        || track.backend.trim().is_empty()
        || !valid_audio_token_parameters(&track.parameters)
    {
        return Err(AudioTokenError::InvalidBinding("metadata"));
    }
    for value in [
        track.audio_sha256.as_str(),
        track.model_sha256.as_str(),
        track.program_sha256.as_str(),
        track.parameters_sha256.as_str(),
        track.token_track_sha256.as_str(),
    ] {
        if !is_sha256(value) {
            return Err(AudioTokenError::InvalidBinding("sha256"));
        }
    }
    if sha256_json(&track.parameters).ok().as_deref() != Some(&track.parameters_sha256) {
        return Err(AudioTokenError::InvalidBinding("parameters_sha256"));
    }
    if sha256_audio_token_track(&track.source_chunks, &track.tokens)
        .ok()
        .as_deref()
        != Some(&track.token_track_sha256)
    {
        return Err(AudioTokenError::InvalidBinding("token_track_sha256"));
    }
    match (
        track.parameters.hotword_bias.as_ref(),
        track.hotword_bias_diagnostics.as_ref(),
    ) {
        (None, None) => {}
        (Some(parameters), Some(diagnostics))
            if validate_hotword_bias_diagnostics(diagnostics)
                && parameters
                    == &AudioTokenHotwordBiasParameters::from_diagnostics(diagnostics) => {}
        _ => return Err(AudioTokenError::InvalidBinding("hotword_bias_diagnostics")),
    }
    if let Some(expected) = expected {
        if track.audio_sha256 != expected.audio_sha256 {
            return Err(AudioTokenError::InvalidBinding("audio_sha256"));
        }
        if track.model_sha256 != expected.model_sha256 {
            return Err(AudioTokenError::InvalidBinding("model_sha256"));
        }
        if track.program_sha256 != expected.program_sha256 {
            return Err(AudioTokenError::InvalidBinding("program_sha256"));
        }
        if track.parameters_sha256 != expected.parameters_sha256 {
            return Err(AudioTokenError::InvalidBinding("parameters_sha256"));
        }
    }
    if track.tokens.is_empty() {
        return Err(AudioTokenError::InvalidBinding("empty_tokens"));
    }
    let mut previous_start = i64::MIN;
    for (position, token) in track.tokens.iter().enumerate() {
        if usize::try_from(token.global_token_index).ok() != Some(position)
            || token.text.trim().is_empty()
            || token.start_ms < 0
            || token.end_ms < token.start_ms
            || token.end_ms > track.audio_duration_ms
            || token.start_ms < previous_start
            || !token.probability.is_finite()
            || !(0.0..=1.0).contains(&token.probability)
        {
            return Err(AudioTokenError::InvalidBinding("token"));
        }
        previous_start = token.start_ms;
    }
    if track.source_chunks.is_empty() {
        return Err(AudioTokenError::InvalidBinding("empty_source_chunks"));
    }
    let mut expected_first = 0u32;
    let mut previous_chunk_end = None;
    for (position, chunk) in track.source_chunks.iter().enumerate() {
        if usize::try_from(chunk.chunk_index).ok() != Some(position)
            || chunk.start_ms < 0
            || chunk.end_ms <= chunk.start_ms
            || chunk.end_ms > track.audio_duration_ms
            || chunk.sample_count == 0
            || chunk.token_count == 0
            || chunk.first_global_token_index != expected_first
            || previous_chunk_end.is_some_and(|end| chunk.start_ms < end)
            || !is_sha256(&chunk.text_sha256)
        {
            return Err(AudioTokenError::InvalidBinding("source_chunk"));
        }
        let chunk_end_token = expected_first
            .checked_add(chunk.token_count)
            .ok_or(AudioTokenError::InvalidBinding("source_chunk"))?;
        let first = usize::try_from(expected_first)
            .map_err(|_| AudioTokenError::InvalidBinding("source_chunk"))?;
        let end = usize::try_from(chunk_end_token)
            .map_err(|_| AudioTokenError::InvalidBinding("source_chunk"))?;
        let chunk_tokens = track
            .tokens
            .get(first..end)
            .ok_or(AudioTokenError::InvalidBinding("source_chunk_count"))?;
        if chunk_tokens.iter().any(|token| {
            token.chunk_index != chunk.chunk_index
                || token.start_ms < chunk.start_ms
                || token.end_ms > chunk.end_ms
        }) || chunk_tokens.windows(2).any(|pair| {
            pair[1].whisper_segment_index < pair[0].whisper_segment_index
                || (pair[1].whisper_segment_index == pair[0].whisper_segment_index
                    && pair[1].whisper_token_index <= pair[0].whisper_token_index)
        }) {
            return Err(AudioTokenError::InvalidBinding("source_chunk_tokens"));
        }
        let chunk_text = chunk_tokens
            .iter()
            .map(|token| token.text.as_str())
            .collect::<String>();
        if sha256_json(&chunk_text).ok().as_deref() != Some(&chunk.text_sha256) {
            return Err(AudioTokenError::InvalidBinding("source_chunk_text_sha256"));
        }
        expected_first = chunk_end_token;
        previous_chunk_end = Some(chunk.end_ms);
    }
    if usize::try_from(expected_first).ok() != Some(track.tokens.len()) {
        return Err(AudioTokenError::InvalidBinding("source_chunk_count"));
    }
    Ok(())
}

fn valid_audio_token_parameters(parameters: &AudioTokenAlignmentParameters) -> bool {
    if let Some(hotword_bias) = parameters.hotword_bias.as_ref() {
        let config = WhisperHotwordBiasConfig::r10_frozen();
        let expected_variants = R10_HOTWORD_TOKENIZATION_VARIANTS
            .iter()
            .map(|value| (*value).to_owned())
            .collect::<Vec<_>>();
        return parameters.schema_version == R10_HOTWORD_TOKEN_ALIGNMENT_PARAMETERS_SCHEMA_VERSION
            && !parameters.initial_prompt_used
            && parameters.context_sha256.as_deref().is_some_and(is_sha256)
            && parameters.prompt_sha256.is_none()
            && parameters.prompt_chars.is_none()
            && parameters.prompt_truncated.is_none()
            && !parameters.synthetic_character_timing
            && !parameters.human_truth_used
            && hotword_bias.schema_version == R10_HOTWORD_BIAS_SCHEMA_VERSION
            && hotword_bias.strategy == R10_HOTWORD_BIAS_STRATEGY
            && is_sha256(&hotword_bias.binding_sha256)
            && is_sha256(&hotword_bias.canonical_terms_sha256)
            && hotword_bias.canonical_term_count > 0
            && is_sha256(&hotword_bias.token_sequence_set_sha256)
            && hotword_bias.token_sequence_count > 0
            && hotword_bias.tokenization_variants == expected_variants
            && hotword_bias.maximum_tokens_per_sequence == R10_MAX_TOKENS_PER_SEQUENCE
            && hotword_bias.start_token_logit_add == config.start_token_logit_add
            && hotword_bias.continuation_token_logit_add == config.continuation_token_logit_add
            && hotword_bias.completion_token_logit_add == config.completion_token_logit_add
            && hotword_bias.duplicate_adjustment_rule == "maximum_not_sum";
    }
    if !parameters.initial_prompt_used {
        return parameters == &AudioTokenAlignmentParameters::product(&parameters.language);
    }
    let (Some(context_sha256), Some(prompt_sha256), Some(prompt_chars), Some(prompt_truncated)) = (
        parameters.context_sha256.as_deref(),
        parameters.prompt_sha256.as_deref(),
        parameters.prompt_chars,
        parameters.prompt_truncated,
    ) else {
        return false;
    };
    if !is_sha256(context_sha256)
        || !is_sha256(prompt_sha256)
        || prompt_chars == 0
        || prompt_chars > WHISPER_INITIAL_PROMPT_MAX_CHARS
    {
        return false;
    }
    parameters
        == &AudioTokenAlignmentParameters::contextual(
            parameters.language.as_str(),
            context_sha256,
            prompt_sha256,
            prompt_chars,
            prompt_truncated,
        )
}

pub fn align_managed_transcription_with_audio_tokens(
    result: &ManagedTranscription,
    r4_snapshot: &crate::database::moss::SourceTranscriptAnchorSnapshot,
    track: Option<&AudioTokenTrack>,
) -> Result<R5ProductAlignment, AudioTokenError> {
    let raw_segments = managed_raw_segments(result)?;
    align_candidate_segments_with_audio_tokens(&raw_segments, r4_snapshot, track)
}

pub fn align_candidate_segments_with_audio_tokens(
    raw_segments: &[CandidateSegmentInput],
    r4_snapshot: &crate::database::moss::SourceTranscriptAnchorSnapshot,
    track: Option<&AudioTokenTrack>,
) -> Result<R5ProductAlignment, AudioTokenError> {
    let raw_segments = validated_raw_segments(raw_segments)?;
    let r4 = align_candidate_segments(&raw_segments, r4_snapshot)
        .map_err(|_| AudioTokenError::Alignment("invalid_moss_result"))?;
    let Some(track) = track else {
        return Ok(r4_fallback(r4, "AUDIO_TOKEN_TRACK_UNAVAILABLE"));
    };
    if let Err(error) = validate_audio_token_track(track, None) {
        return Ok(r4_fallback(
            r4,
            match error {
                AudioTokenError::InvalidBinding(_) => "AUDIO_TOKEN_BINDING_INVALID",
                AudioTokenError::Alignment(_) => "AUDIO_TOKEN_ALIGNMENT_INVALID",
            },
        ));
    }
    let (matches_by_raw, global_coverage) = match token_matches(&raw_segments, &track.tokens) {
        Ok(value) => value,
        Err(_) => {
            return Ok(verified_track_fallback(
                r4,
                track,
                0.0,
                "AUDIO_TOKEN_GLOBAL_ALIGNMENT_FAILED",
            ))
        }
    };
    if global_coverage < R5_MIN_GLOBAL_MATCH_COVERAGE {
        return Ok(verified_track_fallback(
            r4,
            track,
            global_coverage,
            "AUDIO_TOKEN_GLOBAL_COVERAGE_LOW",
        ));
    }

    let mut r4_by_raw = BTreeMap::<u32, Vec<usize>>::new();
    for (index, provenance) in r4.provenance.iter().enumerate() {
        r4_by_raw
            .entry(provenance.raw_segment_index)
            .or_default()
            .push(index);
    }
    let mut segments = Vec::new();
    let mut provenance = Vec::new();
    let mut token_boundaries = Vec::new();
    let mut piece_raw_ranges = Vec::new();
    let mut token_aligned_segment_count = 0u32;
    let mut token_fallback_raw_segment_count = 0u32;

    for (raw_index, raw) in raw_segments.iter().enumerate() {
        let proposed = propose_token_pieces(
            raw,
            &matches_by_raw[raw_index],
            &track.tokens,
            raw_normalized_len(&raw.text),
        )
        .filter(|pieces| {
            token_pieces_fit_sequence(
                pieces,
                segments
                    .last()
                    .map(|segment: &CandidateSegmentInput| segment.start_ms),
                raw_segments
                    .get(raw_index.saturating_add(1))
                    .map(|segment| segment.start_ms),
            )
        });
        if let Some(pieces) = proposed {
            for piece in pieces {
                let segment_index = u32::try_from(segments.len())
                    .map_err(|_| AudioTokenError::Alignment("segment_count"))?;
                segments.push(CandidateSegmentInput {
                    segment_index,
                    start_ms: piece.start_ms,
                    end_ms: piece.end_ms,
                    speaker_label: raw.speaker_label.clone(),
                    text: piece.text,
                });
                let source_id = format!(
                    "whisper-token-{:06}-{:06}",
                    piece.first_token_index, piece.last_token_index
                );
                provenance.push(CandidateAlignmentInput {
                    segment_index,
                    raw_segment_index: raw.segment_index,
                    raw_start_ms: raw.start_ms,
                    raw_end_ms: raw.end_ms,
                    raw_text_sha256: sha256_text(&raw.text),
                    alignment_method: ALIGNMENT_METHOD_AUDIO_TOKEN.to_owned(),
                    confidence: Some(piece.confidence),
                    source_anchor_ids: vec![source_id],
                });
                token_boundaries.push(CandidateAudioTokenBoundary {
                    segment_index,
                    first_token_index: u32::try_from(piece.first_token_index)
                        .map_err(|_| AudioTokenError::Alignment("token_index"))?,
                    last_token_index: u32::try_from(piece.last_token_index)
                        .map_err(|_| AudioTokenError::Alignment("token_index"))?,
                    token_track_sha256: track.token_track_sha256.clone(),
                    confidence: piece.confidence,
                });
                piece_raw_ranges.push(PieceRawRange {
                    output_segment_index: segment_index,
                    raw_segment_index: raw.segment_index,
                    raw_start_char: piece.raw_start_char,
                    raw_end_char: piece.raw_end_char,
                });
                token_aligned_segment_count = token_aligned_segment_count.saturating_add(1);
            }
        } else {
            token_fallback_raw_segment_count = token_fallback_raw_segment_count.saturating_add(1);
            let indices = r4_by_raw
                .get(&raw.segment_index)
                .ok_or(AudioTokenError::Alignment("r4_fallback_missing"))?;
            let mut raw_cursor = 0usize;
            for index in indices {
                let mut segment = r4.segments[*index].clone();
                let mut alignment = r4.provenance[*index].clone();
                let segment_index = u32::try_from(segments.len())
                    .map_err(|_| AudioTokenError::Alignment("segment_count"))?;
                let text_len = segment.text.chars().count();
                segment.segment_index = segment_index;
                alignment.segment_index = segment_index;
                segments.push(segment);
                provenance.push(alignment);
                piece_raw_ranges.push(PieceRawRange {
                    output_segment_index: segment_index,
                    raw_segment_index: raw.segment_index,
                    raw_start_char: raw_cursor,
                    raw_end_char: raw_cursor.saturating_add(text_len),
                });
                raw_cursor = raw_cursor.saturating_add(text_len);
            }
        }
    }

    if concatenate_text(&segments) != concatenate_text(&raw_segments)
        || segments.len() != provenance.len()
        || segments.len() != piece_raw_ranges.len()
        || !segments
            .windows(2)
            .all(|pair| pair[0].start_ms <= pair[1].start_ms)
    {
        return Ok(verified_track_fallback(
            r4,
            track,
            global_coverage,
            "AUDIO_TOKEN_INVARIANT_FALLBACK",
        ));
    }
    let aligned_segment_count = provenance
        .iter()
        .filter(|value| value.alignment_method != crate::moss_alignment::ALIGNMENT_METHOD_RAW)
        .count()
        .try_into()
        .unwrap_or(u32::MAX);
    let fallback_segment_count = provenance
        .iter()
        .filter(|value| value.alignment_method == crate::moss_alignment::ALIGNMENT_METHOD_RAW)
        .count()
        .try_into()
        .unwrap_or(u32::MAX);
    let product = ProductAlignment {
        segments,
        provenance,
        aligned_segment_count,
        fallback_segment_count,
        source_anchor_count: r4.source_anchor_count,
        source_hash_verified: r4.source_hash_verified,
        source_expected_sha256: r4.source_expected_sha256,
        source_actual_sha256: r4.source_actual_sha256,
        fallback_reason: (fallback_segment_count > 0)
            .then(|| "PARTIAL_ALIGNMENT_FALLBACK".to_owned()),
    };
    Ok(R5ProductAlignment {
        product,
        token_boundaries,
        token_aligned_segment_count,
        token_fallback_raw_segment_count,
        token_global_match_coverage: Some(global_coverage),
        token_track_sha256: Some(track.token_track_sha256.clone()),
        token_fallback_reason: (token_fallback_raw_segment_count > 0)
            .then(|| "PARTIAL_AUDIO_TOKEN_FALLBACK".to_owned()),
        piece_raw_ranges,
    })
}

fn r4_fallback(product: ProductAlignment, reason: &str) -> R5ProductAlignment {
    let mut raw_cursor_by_index = BTreeMap::<u32, usize>::new();
    let piece_raw_ranges = product
        .segments
        .iter()
        .zip(&product.provenance)
        .map(|(segment, provenance)| {
            let cursor = raw_cursor_by_index
                .entry(provenance.raw_segment_index)
                .or_insert(0);
            let start = *cursor;
            let end = start.saturating_add(segment.text.chars().count());
            *cursor = end;
            PieceRawRange {
                output_segment_index: segment.segment_index,
                raw_segment_index: provenance.raw_segment_index,
                raw_start_char: start,
                raw_end_char: end,
            }
        })
        .collect();
    let raw_count = product
        .provenance
        .iter()
        .map(|value| value.raw_segment_index)
        .collect::<BTreeSet<_>>()
        .len()
        .try_into()
        .unwrap_or(u32::MAX);
    R5ProductAlignment {
        product,
        token_boundaries: Vec::new(),
        token_aligned_segment_count: 0,
        token_fallback_raw_segment_count: raw_count,
        token_global_match_coverage: None,
        token_track_sha256: None,
        token_fallback_reason: Some(reason.to_owned()),
        piece_raw_ranges,
    }
}

fn verified_track_fallback(
    product: ProductAlignment,
    track: &AudioTokenTrack,
    global_coverage: f64,
    reason: &str,
) -> R5ProductAlignment {
    let mut outcome = r4_fallback(product, reason);
    outcome.token_global_match_coverage = Some(global_coverage.clamp(0.0, 1.0));
    outcome.token_track_sha256 = Some(track.token_track_sha256.clone());
    outcome
}

fn managed_raw_segments(
    result: &ManagedTranscription,
) -> Result<Vec<CandidateSegmentInput>, AudioTokenError> {
    if result.segments.is_empty() {
        return Err(AudioTokenError::Alignment("empty_moss_segments"));
    }
    result
        .segments
        .iter()
        .enumerate()
        .map(|(index, segment)| {
            if usize::try_from(segment.segment_index).ok() != Some(index)
                || segment.t0_ms < 0
                || segment.t1_ms <= segment.t0_ms
                || segment.speaker_id < 0
                || segment.text.trim().is_empty()
            {
                return Err(AudioTokenError::Alignment("invalid_moss_segment"));
            }
            Ok(CandidateSegmentInput {
                segment_index: segment.segment_index,
                start_ms: segment.t0_ms,
                end_ms: segment.t1_ms,
                speaker_label: format!("S{:02}", segment.speaker_id.saturating_add(1)),
                text: segment.text.clone(),
            })
        })
        .collect()
}

fn validated_raw_segments(
    segments: &[CandidateSegmentInput],
) -> Result<Vec<CandidateSegmentInput>, AudioTokenError> {
    if segments.is_empty()
        || segments.iter().enumerate().any(|(index, segment)| {
            usize::try_from(segment.segment_index).ok() != Some(index)
                || segment.start_ms < 0
                || segment.end_ms <= segment.start_ms
                || segment.text.trim().is_empty()
                || !segment
                    .speaker_label
                    .strip_prefix('S')
                    .is_some_and(|digits| {
                        digits.len() >= 2 && digits.bytes().all(|value| value.is_ascii_digit())
                    })
        })
        || segments
            .windows(2)
            .any(|pair| pair[1].start_ms < pair[0].start_ms)
    {
        return Err(AudioTokenError::Alignment("invalid_moss_segments"));
    }
    Ok(segments.to_vec())
}

fn token_matches(
    raw_segments: &[CandidateSegmentInput],
    tokens: &[AudioToken],
) -> Result<(Vec<Vec<RawTokenMatch>>, f64), AudioTokenError> {
    let mut moss = Vec::new();
    for (raw_index, segment) in raw_segments.iter().enumerate() {
        moss.extend(normalize_text(&segment.text, raw_index));
    }
    let mut whisper = Vec::new();
    for (token_index, token) in tokens.iter().enumerate() {
        whisper.extend(normalize_text(&token.text, token_index));
    }
    if moss.is_empty() || whisper.is_empty() {
        return Err(AudioTokenError::Alignment("empty_normalized_text"));
    }
    let matches = exact_matches_from_edit_alignment(&moss, &whisper)
        .ok_or(AudioTokenError::Alignment("edit_alignment"))?;
    let coverage =
        (matches.len() as f64 / moss.len() as f64).min(matches.len() as f64 / whisper.len() as f64);
    let mut by_raw = vec![Vec::new(); raw_segments.len()];
    for (moss_index, whisper_index) in matches {
        by_raw[moss[moss_index].owner_index].push(RawTokenMatch {
            raw_char_index: moss[moss_index].original_char_index,
            token_index: whisper[whisper_index].owner_index,
        });
    }
    Ok((by_raw, coverage))
}

fn propose_token_pieces(
    raw: &CandidateSegmentInput,
    matches: &[RawTokenMatch],
    tokens: &[AudioToken],
    normalized_raw_len: usize,
) -> Option<Vec<ProposedPiece>> {
    let matches = matches
        .iter()
        .filter(|matched| {
            tokens.get(matched.token_index).is_some_and(|token| {
                token.start_ms >= raw.start_ms
                    && token.start_ms < raw.end_ms
                    && token.end_ms <= raw.end_ms
            })
        })
        .collect::<Vec<_>>();
    if normalized_raw_len == 0
        || matches.len() as f64 / (normalized_raw_len as f64) < R5_MIN_RAW_SEGMENT_MATCH_COVERAGE
    {
        return None;
    }
    let raw_chars = raw.text.chars().collect::<Vec<_>>();
    let mut candidates = Vec::<(usize, i64, usize)>::new();
    let mut previous_token = None;
    for matched in &matches {
        if previous_token.is_some_and(|value| matched.token_index <= value) {
            continue;
        }
        previous_token = Some(matched.token_index);
        let token = tokens.get(matched.token_index)?;
        if matched.raw_char_index > 0
            && matched.raw_char_index < raw_chars.len()
            && safe_ascii_word_boundary(&raw_chars, matched.raw_char_index)
            && token.start_ms > raw.start_ms
            && token.start_ms < raw.end_ms
        {
            candidates.push((matched.raw_char_index, token.start_ms, matched.token_index));
        }
    }
    candidates.dedup_by(|left, right| left.0 == right.0 || left.1 == right.1);
    let mut chosen = Vec::<(usize, i64, usize)>::new();
    let mut piece_start_ms = raw.start_ms;
    let mut cursor = 0usize;
    while raw.end_ms.saturating_sub(piece_start_ms) > R5_MAX_ALIGNED_PIECE_MS {
        let upper = piece_start_ms.saturating_add(R5_MAX_ALIGNED_PIECE_MS);
        let next = candidates[cursor..]
            .iter()
            .enumerate()
            .take_while(|(_, value)| value.1 <= upper)
            .last()
            .map(|(offset, value)| (cursor + offset, *value))
            .or_else(|| {
                candidates[cursor..]
                    .iter()
                    .enumerate()
                    .find(|(_, value)| value.1 > piece_start_ms)
                    .map(|(offset, value)| (cursor + offset, *value))
            })?;
        if next.1 .1 <= piece_start_ms {
            return None;
        }
        chosen.push(next.1);
        piece_start_ms = next.1 .1;
        cursor = next.0.saturating_add(1);
    }
    if chosen.is_empty() {
        let first = matches.first()?.token_index;
        let last = matches.last()?.token_index;
        return single_token_piece(raw, first, last, tokens);
    }
    let mut char_boundaries = Vec::with_capacity(chosen.len().saturating_add(2));
    char_boundaries.push(0usize);
    char_boundaries.extend(chosen.iter().map(|value| value.0));
    char_boundaries.push(raw_chars.len());
    let mut time_boundaries = Vec::with_capacity(chosen.len().saturating_add(2));
    time_boundaries.push(raw.start_ms);
    time_boundaries.extend(chosen.iter().map(|value| value.1));
    time_boundaries.push(raw.end_ms);
    if char_boundaries.windows(2).any(|pair| pair[0] >= pair[1])
        || time_boundaries.windows(2).any(|pair| pair[0] >= pair[1])
    {
        return None;
    }
    let mut pieces = Vec::new();
    for piece_index in 0..char_boundaries.len().saturating_sub(1) {
        let raw_start_char = char_boundaries[piece_index];
        let raw_end_char = char_boundaries[piece_index + 1];
        let text = raw_chars[raw_start_char..raw_end_char]
            .iter()
            .collect::<String>();
        if text.trim().is_empty() {
            return None;
        }
        let piece_matches = matches
            .iter()
            .filter(|value| {
                value.raw_char_index >= raw_start_char && value.raw_char_index < raw_end_char
            })
            .collect::<Vec<_>>();
        let first_token_index = piece_matches.first()?.token_index;
        let last_token_index = piece_matches.last()?.token_index;
        let confidence = token_confidence(tokens, first_token_index, last_token_index)?;
        pieces.push(ProposedPiece {
            text,
            raw_start_char,
            raw_end_char,
            start_ms: time_boundaries[piece_index],
            end_ms: time_boundaries[piece_index + 1],
            first_token_index,
            last_token_index,
            confidence,
        });
    }
    Some(pieces)
}

fn safe_ascii_word_boundary(raw_chars: &[char], boundary: usize) -> bool {
    let Some(left) = boundary
        .checked_sub(1)
        .and_then(|index| raw_chars.get(index))
    else {
        return false;
    };
    let Some(right) = raw_chars.get(boundary) else {
        return false;
    };
    !(left.is_ascii_alphanumeric() && right.is_ascii_alphanumeric())
}

fn token_pieces_fit_sequence(
    pieces: &[ProposedPiece],
    previous_output_start_ms: Option<i64>,
    next_raw_start_ms: Option<i64>,
) -> bool {
    let (Some(first), Some(last)) = (pieces.first(), pieces.last()) else {
        return false;
    };
    first.start_ms >= previous_output_start_ms.unwrap_or(i64::MIN)
        && last.start_ms <= next_raw_start_ms.unwrap_or(i64::MAX)
        && pieces.iter().all(|piece| {
            piece.start_ms >= 0 && piece.end_ms > piece.start_ms && !piece.text.trim().is_empty()
        })
        && pieces
            .windows(2)
            .all(|pair| pair[0].start_ms <= pair[1].start_ms)
}

fn single_token_piece(
    raw: &CandidateSegmentInput,
    first_token_index: usize,
    last_token_index: usize,
    tokens: &[AudioToken],
) -> Option<Vec<ProposedPiece>> {
    if raw.end_ms.saturating_sub(raw.start_ms) > R5_MAX_ALIGNED_PIECE_MS {
        return None;
    }
    Some(vec![ProposedPiece {
        text: raw.text.clone(),
        raw_start_char: 0,
        raw_end_char: raw.text.chars().count(),
        start_ms: raw.start_ms,
        end_ms: raw.end_ms,
        first_token_index,
        last_token_index,
        confidence: token_confidence(tokens, first_token_index, last_token_index)?,
    }])
}

fn token_confidence(tokens: &[AudioToken], first: usize, last: usize) -> Option<f64> {
    if first > last {
        return None;
    }
    tokens
        .get(first..=last)?
        .iter()
        .map(|token| f64::from(token.probability))
        .reduce(f64::min)
}

pub fn machine_term_suggestions(
    raw_segments: &[CandidateSegmentInput],
    alignment: &R5ProductAlignment,
    track: &AudioTokenTrack,
    terms: &[ContextTerm],
    context_sha256: &str,
) -> Vec<MachineTermSuggestion> {
    if validate_audio_token_track(track, None).is_err()
        || !is_sha256(context_sha256)
        || alignment.token_track_sha256.as_deref() != Some(track.token_track_sha256.as_str())
        || !alignment
            .token_global_match_coverage
            .is_some_and(|coverage| coverage >= R5_MIN_GLOBAL_MATCH_COVERAGE)
    {
        return Vec::new();
    }
    let Ok((matches_by_raw, _)) = token_matches(raw_segments, &track.tokens) else {
        return Vec::new();
    };
    let token_chars = track
        .tokens
        .iter()
        .enumerate()
        .flat_map(|(index, token)| normalize_text(&token.text, index))
        .collect::<Vec<_>>();
    let token_string = token_chars
        .iter()
        .map(|value| value.value)
        .collect::<String>();
    let token_string_chars = token_string.chars().collect::<Vec<_>>();
    let mut suggestions = Vec::new();
    for term in terms {
        let canonical = normalized_values(&term.canonical);
        if canonical.is_empty() {
            continue;
        }
        for occurrence_start in find_subslice(&token_string_chars, &canonical) {
            let occurrence_end = occurrence_start.saturating_add(canonical.len());
            let Some(first_char) = token_chars.get(occurrence_start) else {
                continue;
            };
            let Some(last_char) = token_chars.get(occurrence_end.saturating_sub(1)) else {
                continue;
            };
            let first_token_index = first_char.owner_index;
            let last_token_index = last_char.owner_index;
            let Some(first_token) = track.tokens.get(first_token_index) else {
                continue;
            };
            let Some(last_token) = track.tokens.get(last_token_index) else {
                continue;
            };
            if first_token.chunk_index != last_token.chunk_index {
                continue;
            }
            let Some(confidence) =
                token_confidence(&track.tokens, first_token_index, last_token_index)
            else {
                continue;
            };
            if confidence < f64::from(R5_MIN_TERM_TOKEN_PROBABILITY) {
                continue;
            }
            let token_start_ms = first_token.start_ms;
            let token_end_ms = last_token.end_ms;
            let raw_candidates = raw_segments
                .iter()
                .enumerate()
                .filter(|(_, raw)| {
                    raw.start_ms <= token_start_ms
                        && token_start_ms < raw.end_ms
                        && token_end_ms <= raw.end_ms
                })
                .map(|(index, _)| index)
                .collect::<Vec<_>>();
            if raw_candidates.len() != 1 {
                continue;
            }
            let raw_index = raw_candidates[0];
            let raw = &raw_segments[raw_index];
            let Some((raw_start_char, raw_end_char)) = inferred_raw_term_range(
                &matches_by_raw[raw_index],
                first_token_index,
                last_token_index,
                raw.text.chars().count(),
            ) else {
                continue;
            };
            if raw_end_char <= raw_start_char
                || raw_end_char.saturating_sub(raw_start_char) > MAX_MACHINE_CORRECTION_CHARACTERS
            {
                continue;
            }
            let original = raw
                .text
                .chars()
                .skip(raw_start_char)
                .take(raw_end_char - raw_start_char)
                .collect::<String>();
            if original.trim().is_empty()
                || comparison_text(&original) == comparison_text(&term.canonical)
                || comparison_text(&original).contains(&comparison_text(&term.canonical))
            {
                continue;
            }
            let matching_pieces = alignment
                .piece_raw_ranges
                .iter()
                .filter(|range| {
                    usize::try_from(range.raw_segment_index).ok() == Some(raw_index)
                        && raw_start_char >= range.raw_start_char
                        && raw_end_char <= range.raw_end_char
                })
                .collect::<Vec<_>>();
            if matching_pieces.len() != 1 {
                continue;
            }
            let piece = matching_pieces[0];
            let Ok(first_token_index_u32) = u32::try_from(first_token_index) else {
                continue;
            };
            let Ok(last_token_index_u32) = u32::try_from(last_token_index) else {
                continue;
            };
            let matching_boundaries = alignment
                .token_boundaries
                .iter()
                .filter(|boundary| boundary.segment_index == piece.output_segment_index)
                .collect::<Vec<_>>();
            if matching_boundaries.len() != 1 {
                continue;
            }
            let boundary = matching_boundaries[0];
            if boundary.token_track_sha256 != track.token_track_sha256
                || first_token_index_u32 < boundary.first_token_index
                || last_token_index_u32 > boundary.last_token_index
            {
                continue;
            }
            let local_start = raw_start_char.saturating_sub(piece.raw_start_char);
            let local_end = raw_end_char.saturating_sub(piece.raw_start_char);
            suggestions.push(MachineTermSuggestion {
                segment_index: piece.output_segment_index,
                start_char: local_start,
                end_char: local_end,
                original_text: original,
                replacement_text: term.canonical.clone(),
                term_id: term.term_id.clone(),
                context_sha256: context_sha256.to_owned(),
                token_track_sha256: track.token_track_sha256.clone(),
                model_sha256: track.model_sha256.clone(),
                first_token_index: first_token_index_u32,
                last_token_index: last_token_index_u32,
                confidence,
            });
        }
    }
    suggestions = suggestions
        .iter()
        .enumerate()
        .filter(|(index, candidate)| {
            !suggestions.iter().enumerate().any(|(other_index, other)| {
                index != &other_index
                    && candidate.segment_index == other.segment_index
                    && candidate.start_char < other.end_char
                    && other.start_char < candidate.end_char
            })
        })
        .map(|(_, suggestion)| suggestion.clone())
        .collect();
    suggestions.sort_by(|left, right| {
        left.segment_index
            .cmp(&right.segment_index)
            .then_with(|| right.start_char.cmp(&left.start_char))
            .then_with(|| left.term_id.cmp(&right.term_id))
    });
    suggestions
}

pub fn machine_term_suggestions_for_managed(
    result: &ManagedTranscription,
    alignment: &R5ProductAlignment,
    track: &AudioTokenTrack,
    terms: &[ContextTerm],
    context_sha256: &str,
) -> Vec<MachineTermSuggestion> {
    let Ok(raw_segments) = managed_raw_segments(result) else {
        return Vec::new();
    };
    machine_term_suggestions(&raw_segments, alignment, track, terms, context_sha256)
}

fn inferred_raw_term_range(
    matches: &[RawTokenMatch],
    first_token: usize,
    last_token: usize,
    raw_char_count: usize,
) -> Option<(usize, usize)> {
    let direct = matches
        .iter()
        .filter(|value| value.token_index >= first_token && value.token_index <= last_token)
        .map(|value| value.raw_char_index)
        .collect::<Vec<_>>();
    let before = matches
        .iter()
        .rev()
        .find(|value| value.token_index < first_token)
        .map(|value| value.raw_char_index.saturating_add(1));
    let after = matches
        .iter()
        .find(|value| value.token_index > last_token)
        .map(|value| value.raw_char_index);
    let start = direct.iter().min().copied().or(before)?;
    let end = direct
        .iter()
        .max()
        .copied()
        .map(|value| value.saturating_add(1))
        .or(after)?;
    let anchored_start = before.map(|value| value.min(start)).unwrap_or(start);
    let anchored_end = after.map(|value| value.max(end)).unwrap_or(end);
    (anchored_start < anchored_end && anchored_end <= raw_char_count)
        .then_some((anchored_start, anchored_end))
}

fn normalize_text(text: &str, owner_index: usize) -> Vec<NormalizedChar> {
    text.chars()
        .enumerate()
        .filter(|(_, value)| value.is_alphanumeric())
        .map(|(original_char_index, value)| NormalizedChar {
            value: if value.is_ascii() {
                value.to_ascii_lowercase()
            } else {
                value
            },
            original_char_index,
            owner_index,
        })
        .collect()
}

fn normalized_values(text: &str) -> Vec<char> {
    normalize_text(text, 0)
        .into_iter()
        .map(|value| value.value)
        .collect()
}

fn comparison_text(text: &str) -> String {
    normalized_values(text).into_iter().collect()
}

fn raw_normalized_len(text: &str) -> usize {
    normalize_text(text, 0).len()
}

fn find_subslice(haystack: &[char], needle: &[char]) -> Vec<usize> {
    if needle.is_empty() || needle.len() > haystack.len() {
        return Vec::new();
    }
    haystack
        .windows(needle.len())
        .enumerate()
        .filter_map(|(index, value)| (value == needle).then_some(index))
        .collect()
}

fn exact_matches_from_edit_alignment(
    left: &[NormalizedChar],
    right: &[NormalizedChar],
) -> Option<Vec<(usize, usize)>> {
    let width = right.len().checked_add(1)?;
    let height = left.len().checked_add(1)?;
    if width.checked_mul(height)? > MAX_ALIGNMENT_CELLS {
        return None;
    }
    let mut distance = vec![0u32; width.checked_mul(height)?];
    for i in 0..height {
        distance[i * width] = u32::try_from(i).ok()?;
    }
    for j in 0..width {
        distance[j] = u32::try_from(j).ok()?;
    }
    for i in 1..height {
        for j in 1..width {
            let substitution = distance[(i - 1) * width + j - 1]
                + u32::from(left[i - 1].value != right[j - 1].value);
            let deletion = distance[(i - 1) * width + j] + 1;
            let insertion = distance[i * width + j - 1] + 1;
            distance[i * width + j] = substitution.min(deletion).min(insertion);
        }
    }
    let mut i = left.len();
    let mut j = right.len();
    let mut matches = Vec::new();
    while i > 0 || j > 0 {
        if i > 0 && j > 0 {
            let cost = u32::from(left[i - 1].value != right[j - 1].value);
            if distance[i * width + j] == distance[(i - 1) * width + j - 1] + cost {
                if cost == 0 {
                    matches.push((i - 1, j - 1));
                }
                i -= 1;
                j -= 1;
                continue;
            }
        }
        if i > 0 && distance[i * width + j] == distance[(i - 1) * width + j] + 1 {
            i -= 1;
        } else if j > 0 {
            j -= 1;
        } else {
            return None;
        }
    }
    matches.reverse();
    Some(matches)
}

fn concatenate_text(segments: &[CandidateSegmentInput]) -> String {
    segments
        .iter()
        .map(|segment| segment.text.as_str())
        .collect()
}

fn sha256_text(value: &str) -> String {
    format!("{:x}", Sha256::digest(value.as_bytes()))
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

pub fn model_path(models_dir: &Path) -> PathBuf {
    models_dir.join(format!("ggml-{R5_ALIGNMENT_MODEL_NAME}.bin"))
}

#[cfg(test)]
mod tests {
    use moss_helper::protocol::{
        CompletedMessage, CompletionStatus, NativeSessionLimits, NativeTimings,
    };

    use super::*;
    use crate::database::moss::{SourceTranscriptAnchor, SourceTranscriptAnchorSnapshot};
    use crate::moss_helper::manager::ManagedSegment;
    use crate::whisper_engine::hotword_bias::{
        hotword_bias_binding_sha256, WhisperHotwordTermDiagnostics,
        R10_HOTWORD_TOKENIZATION_VARIANTS,
    };

    fn token(index: u32, start_ms: i64, end_ms: i64, text: &str) -> AudioToken {
        AudioToken {
            global_token_index: index,
            chunk_index: 0,
            whisper_segment_index: 0,
            whisper_token_index: index,
            start_ms,
            end_ms,
            text: text.to_owned(),
            probability: 0.9,
        }
    }

    fn track(tokens: Vec<AudioToken>) -> AudioTokenTrack {
        let parameters = AudioTokenAlignmentParameters::product("zh");
        let parameters_sha256 = sha256_json(&parameters).unwrap();
        let chunk_text = tokens
            .iter()
            .map(|token| token.text.as_str())
            .collect::<String>();
        let source_chunks = vec![AudioTokenSourceChunk {
            chunk_index: 0,
            start_ms: 0,
            end_ms: 10_000,
            sample_count: 160_000,
            text_sha256: sha256_json(&chunk_text).unwrap(),
            first_global_token_index: 0,
            token_count: u32::try_from(tokens.len()).unwrap(),
        }];
        let token_track_sha256 = sha256_audio_token_track(&source_chunks, &tokens).unwrap();
        AudioTokenTrack {
            schema_version: 1,
            audio_sha256: "a".repeat(64),
            audio_duration_ms: 10_000,
            model_name: R5_ALIGNMENT_MODEL_NAME.to_owned(),
            model_sha256: "b".repeat(64),
            program_sha256: "c".repeat(64),
            parameters_sha256,
            token_track_sha256,
            backend: "cpu".to_owned(),
            parameters,
            hotword_bias_diagnostics: None,
            source_chunks,
            tokens,
        }
    }

    fn hotword_diagnostics() -> WhisperHotwordBiasDiagnostics {
        let config = WhisperHotwordBiasConfig::r10_frozen();
        let mut diagnostics = WhisperHotwordBiasDiagnostics {
            schema_version: R10_HOTWORD_BIAS_SCHEMA_VERSION,
            strategy: R10_HOTWORD_BIAS_STRATEGY.to_owned(),
            binding_sha256: String::new(),
            canonical_terms_sha256: "d".repeat(64),
            canonical_term_count: 1,
            token_sequence_set_sha256: "e".repeat(64),
            token_sequence_count: 2,
            tokenization_variants: R10_HOTWORD_TOKENIZATION_VARIANTS
                .iter()
                .map(|value| (*value).to_owned())
                .collect(),
            maximum_tokens_per_sequence: R10_MAX_TOKENS_PER_SEQUENCE,
            start_token_logit_add: config.start_token_logit_add,
            continuation_token_logit_add: config.continuation_token_logit_add,
            completion_token_logit_add: config.completion_token_logit_add,
            duplicate_adjustment_rule: "maximum_not_sum".to_owned(),
            canonical_term_plaintext_stored: false,
            raw_token_ids_stored: false,
            terms: vec![WhisperHotwordTermDiagnostics {
                canonical_index: 0,
                term_sha256: "f".repeat(64),
                token_sequence_count: 2,
                token_counts: vec![2, 3],
                token_sequence_sha256: vec!["1".repeat(64), "2".repeat(64)],
            }],
            decode_call_count: 2,
            callback_invocations: 20,
            callbacks_with_adjustments: 20,
            logit_adjustment_count: 40,
            maximum_adjustments_in_one_callback: 2,
        };
        diagnostics.binding_sha256 = hotword_bias_binding_sha256(&diagnostics).unwrap();
        diagnostics
    }

    fn managed(segments: Vec<ManagedSegment>) -> ManagedTranscription {
        let text = segments
            .iter()
            .map(|segment| segment.text.as_str())
            .collect::<String>();
        ManagedTranscription {
            request_id: "798d8c63-5ff1-40e3-9db8-0f706aeb930a".to_owned(),
            context_sha256: "e".repeat(64),
            raw_text: text.clone(),
            clean_text: text,
            segments,
            completed: CompletedMessage {
                v: moss_helper::protocol::PROTOCOL_VERSION,
                seq: 1,
                request_id: "798d8c63-5ff1-40e3-9db8-0f706aeb930a".to_owned(),
                context_sha256: "e".repeat(64),
                terminal: true,
                status: CompletionStatus::Ok,
                backend: "Vulkan0".to_owned(),
                device_description: "Intel(R) Arc(TM) Graphics".to_owned(),
                raw_text_sha256: "f".repeat(64),
                clean_text_sha256: "1".repeat(64),
                segment_count: 1,
                last_timestamp_ms: 5_000,
                native_run_elapsed_ms: 1,
                native_rtf: 0.1,
                wall_elapsed_ms: 1,
                wall_rtf: 0.1,
                native_timings: NativeTimings {
                    load_ms: 0.0,
                    mel_ms: 0.0,
                    encode_ms: 0.0,
                    decode_ms: 0.0,
                },
                was_aborted: false,
                was_truncated: false,
                native_session_limits: NativeSessionLimits {
                    effective_n_ctx: moss_helper::native::MOSS_SESSION_N_CTX,
                    effective_max_audio_ms: 1_200_000,
                    max_kv_bytes: 1_879_048_192,
                },
                language_requested: moss_helper::native::MOSS_LANGUAGE_REQUESTED.to_owned(),
                language_resolved: moss_helper::native::MOSS_LANGUAGE_RESOLVED.to_owned(),
                decode_parameters_json: moss_helper::native::MOSS_DECODE_PARAMETERS_JSON.to_owned(),
                decode_parameters_sha256: moss_helper::native::moss_decode_parameters_sha256(),
            },
            heartbeat_count: 0,
            supervisor_wall_elapsed_ms: 1,
            supervisor_wall_rtf: 0.1,
            helper_process_id: 1,
            helper_total_processes: 1,
            helper_peak_job_memory_bytes: 1,
            residual_process_count: 0,
            helper_runs: Vec::new(),
            audio_activity: None,
            language_requested: moss_helper::native::MOSS_LANGUAGE_REQUESTED.to_owned(),
            language_resolved: moss_helper::native::MOSS_LANGUAGE_RESOLVED.to_owned(),
            decode_parameters_json: moss_helper::native::MOSS_DECODE_PARAMETERS_JSON.to_owned(),
            decode_parameters_sha256: moss_helper::native::moss_decode_parameters_sha256(),
        }
    }

    fn snapshot() -> SourceTranscriptAnchorSnapshot {
        SourceTranscriptAnchorSnapshot {
            expected_sha256: "2".repeat(64),
            actual_sha256: "2".repeat(64),
            hash_verified: true,
            invalid_timed_rows: 0,
            anchors: Vec::<SourceTranscriptAnchor>::new(),
        }
    }

    #[test]
    fn token_track_rejects_each_tampered_binding_and_invalid_time() {
        let original = track(vec![token(0, 100, 200, " Google")]);
        assert!(validate_audio_token_track(
            &original,
            Some(ExpectedAudioTokenBinding {
                audio_sha256: &original.audio_sha256,
                model_sha256: &original.model_sha256,
                program_sha256: &original.program_sha256,
                parameters_sha256: &original.parameters_sha256,
            })
        )
        .is_ok());
        let mut tampered = original.clone();
        tampered.audio_sha256 = "e".repeat(64);
        assert!(validate_audio_token_track(
            &tampered,
            Some(ExpectedAudioTokenBinding {
                audio_sha256: &original.audio_sha256,
                model_sha256: &original.model_sha256,
                program_sha256: &original.program_sha256,
                parameters_sha256: &original.parameters_sha256,
            })
        )
        .is_err());
        let mut tampered = original.clone();
        tampered.model_sha256 = "e".repeat(64);
        assert!(validate_audio_token_track(
            &tampered,
            Some(ExpectedAudioTokenBinding {
                audio_sha256: &original.audio_sha256,
                model_sha256: &original.model_sha256,
                program_sha256: &original.program_sha256,
                parameters_sha256: &original.parameters_sha256,
            })
        )
        .is_err());
        let mut tampered = original.clone();
        tampered.program_sha256 = "e".repeat(64);
        assert!(validate_audio_token_track(
            &tampered,
            Some(ExpectedAudioTokenBinding {
                audio_sha256: &original.audio_sha256,
                model_sha256: &original.model_sha256,
                program_sha256: &original.program_sha256,
                parameters_sha256: &original.parameters_sha256,
            })
        )
        .is_err());
        let mut tampered = original.clone();
        tampered.parameters.language = "en".to_owned();
        assert!(validate_audio_token_track(&tampered, None).is_err());
        let mut tampered = original.clone();
        tampered.parameters_sha256 = "e".repeat(64);
        assert!(validate_audio_token_track(&tampered, None).is_err());
        let mut tampered = original.clone();
        tampered.token_track_sha256 = "e".repeat(64);
        assert!(validate_audio_token_track(&tampered, None).is_err());
        let mut tampered = original.clone();
        tampered.tokens[0].end_ms = 10_001;
        tampered.token_track_sha256 =
            sha256_audio_token_track(&tampered.source_chunks, &tampered.tokens).unwrap();
        assert!(validate_audio_token_track(&tampered, None).is_err());
        let mut tampered = original.clone();
        tampered.tokens[0].chunk_index = 1;
        tampered.token_track_sha256 =
            sha256_audio_token_track(&tampered.source_chunks, &tampered.tokens).unwrap();
        assert!(validate_audio_token_track(&tampered, None).is_err());
        let mut tampered = original.clone();
        tampered.source_chunks[0].first_global_token_index = 1;
        assert!(validate_audio_token_track(&tampered, None).is_err());
        let mut tampered = original.clone();
        tampered.source_chunks[0].text_sha256 = "e".repeat(64);
        tampered.token_track_sha256 =
            sha256_audio_token_track(&tampered.source_chunks, &tampered.tokens).unwrap();
        assert!(validate_audio_token_track(&tampered, None).is_err());
    }

    #[test]
    fn no_prompt_parameters_keep_the_r5_serialized_shape() {
        let parameters = AudioTokenAlignmentParameters::product("zh");
        assert_eq!(
            sha256_json(&parameters).unwrap(),
            "42846d0e607c7b3071cf0cf118ce0089daac1b051196e467d0728ab292ae0415"
        );
        let value = serde_json::to_value(parameters).unwrap();
        let object = value.as_object().unwrap();
        assert_eq!(
            object
                .get("schema_version")
                .and_then(|value| value.as_u64()),
            Some(u64::from(R5_TOKEN_ALIGNMENT_PARAMETERS_SCHEMA_VERSION))
        );
        assert_eq!(
            object
                .get("initial_prompt_used")
                .and_then(|value| value.as_bool()),
            Some(false)
        );
        for private_binding in [
            "context_sha256",
            "prompt_sha256",
            "prompt_chars",
            "prompt_truncated",
            "hotword_bias",
        ] {
            assert!(!object.contains_key(private_binding));
        }
    }

    #[test]
    fn contextual_parameters_bind_prompt_without_storing_plaintext() {
        let prompt_text = "专有名词：Google。";
        let context_sha256 = "d".repeat(64);
        let prompt_sha256 = sha256_text(prompt_text);
        let parameters = AudioTokenAlignmentParameters::contextual(
            "zh",
            context_sha256.clone(),
            prompt_sha256.clone(),
            prompt_text.chars().count(),
            false,
        );
        assert!(valid_audio_token_parameters(&parameters));
        let encoded = serde_json::to_string(&parameters).unwrap();
        assert!(!encoded.contains(prompt_text));
        assert!(encoded.contains(&context_sha256));
        assert!(encoded.contains(&prompt_sha256));

        let mut prompted_track = track(vec![token(0, 100, 200, " Google")]);
        prompted_track.parameters = parameters;
        prompted_track.parameters_sha256 = sha256_json(&prompted_track.parameters).unwrap();
        assert!(validate_audio_token_track(&prompted_track, None).is_ok());

        prompted_track.parameters.prompt_sha256 = Some("invalid".to_owned());
        prompted_track.parameters_sha256 = sha256_json(&prompted_track.parameters).unwrap();
        assert!(validate_audio_token_track(&prompted_track, None).is_err());
    }

    #[test]
    fn hotword_parameters_and_diagnostics_are_bound_without_plaintext() {
        let diagnostics = hotword_diagnostics();
        assert!(validate_hotword_bias_diagnostics(&diagnostics));
        let context_sha256 = "3".repeat(64);
        let parameters = AudioTokenAlignmentParameters::hotword_biased(
            "zh",
            context_sha256.clone(),
            &diagnostics,
        );
        assert!(valid_audio_token_parameters(&parameters));
        let encoded = serde_json::to_string(&parameters).unwrap();
        assert!(!encoded.contains("Google"));
        assert!(encoded.contains(&context_sha256));
        assert!(encoded.contains(&diagnostics.binding_sha256));

        let mut biased_track = track(vec![token(0, 100, 200, "测试")]);
        biased_track.parameters = parameters;
        biased_track.parameters_sha256 = sha256_json(&biased_track.parameters).unwrap();
        biased_track.hotword_bias_diagnostics = Some(diagnostics.clone());
        assert!(validate_audio_token_track(&biased_track, None).is_ok());

        let mut tampered = biased_track.clone();
        tampered
            .hotword_bias_diagnostics
            .as_mut()
            .unwrap()
            .callback_invocations = 0;
        assert!(validate_audio_token_track(&tampered, None).is_err());

        let mut tampered = biased_track;
        tampered
            .parameters
            .hotword_bias
            .as_mut()
            .unwrap()
            .canonical_term_count = 2;
        tampered.parameters_sha256 = sha256_json(&tampered.parameters).unwrap();
        assert!(validate_audio_token_track(&tampered, None).is_err());
    }

    #[test]
    fn token_offset_uses_real_chunk_offset_and_rejects_chunk_escape() {
        let native = WhisperTimedToken {
            segment_index: 1,
            token_index: 2,
            text: " PWA".to_owned(),
            start_ms: 340,
            end_ms: 520,
            probability: 0.8,
        };
        let global = offset_timed_token(native.clone(), 3, 9, 4_000, 5_000, 9_000).unwrap();
        assert_eq!((global.start_ms, global.end_ms), (4_340, 4_520));
        let invalid = offset_timed_token(native, 3, 9, 4_800, 5_000, 9_000);
        assert!(invalid.is_err());
    }

    #[test]
    fn proposed_pieces_only_use_native_token_starts_and_preserve_text() {
        let raw = CandidateSegmentInput {
            segment_index: 0,
            start_ms: 0,
            end_ms: 5_000,
            speaker_label: "S01".to_owned(),
            text: "第一段内容第二段内容第三段内容".to_owned(),
        };
        let tokens = vec![
            token(0, 100, 700, "第一段内容"),
            token(1, 1_900, 2_600, "第二段内容"),
            token(2, 3_700, 4_500, "第三段内容"),
        ];
        let (_, matches) = {
            let (mut result, coverage) = token_matches(&[raw.clone()], &tokens).unwrap();
            (coverage, result.remove(0))
        };
        let pieces =
            propose_token_pieces(&raw, &matches, &tokens, raw_normalized_len(&raw.text)).unwrap();
        assert_eq!(
            pieces
                .iter()
                .map(|piece| piece.text.as_str())
                .collect::<String>(),
            raw.text
        );
        let allowed = tokens
            .iter()
            .map(|value| value.start_ms)
            .collect::<BTreeSet<_>>();
        assert!(pieces
            .iter()
            .skip(1)
            .all(|piece| allowed.contains(&piece.start_ms)));
        assert!(pieces
            .iter()
            .all(|piece| piece.end_ms - piece.start_ms <= R5_MAX_ALIGNED_PIECE_MS));
    }

    #[test]
    fn proposed_pieces_do_not_split_an_ascii_word() {
        let raw = CandidateSegmentInput {
            segment_index: 0,
            start_ms: 0,
            end_ms: 6_000,
            speaker_label: "S01".to_owned(),
            text: "前文YouTube中间后文".to_owned(),
        };
        let tokens = vec![
            token(0, 100, 1_800, "前文"),
            token(1, 1_900, 2_100, "You"),
            token(2, 2_100, 2_300, "Tube"),
            token(3, 2_300, 3_900, "中间"),
            token(4, 4_000, 5_900, "后文"),
        ];
        let (mut matches, coverage) = token_matches(&[raw.clone()], &tokens).unwrap();
        assert_eq!(coverage, 1.0);
        let pieces = propose_token_pieces(
            &raw,
            &matches.remove(0),
            &tokens,
            raw_normalized_len(&raw.text),
        )
        .unwrap();
        assert_eq!(
            pieces
                .iter()
                .map(|piece| piece.text.as_str())
                .collect::<String>(),
            raw.text
        );
        assert!(pieces.iter().any(|piece| piece.text.contains("YouTube")));
        assert!(pieces
            .iter()
            .all(|piece| piece.end_ms - piece.start_ms <= R5_MAX_ALIGNED_PIECE_MS));
    }

    #[test]
    fn token_pieces_that_cross_the_next_overlapping_raw_start_fall_back_locally() {
        let pieces = vec![
            ProposedPiece {
                text: "前半".to_owned(),
                raw_start_char: 0,
                raw_end_char: 2,
                start_ms: 10_000,
                end_ms: 12_000,
                first_token_index: 0,
                last_token_index: 0,
                confidence: 0.9,
            },
            ProposedPiece {
                text: "后半".to_owned(),
                raw_start_char: 2,
                raw_end_char: 4,
                start_ms: 12_000,
                end_ms: 14_000,
                first_token_index: 1,
                last_token_index: 1,
                confidence: 0.9,
            },
        ];
        assert!(!token_pieces_fit_sequence(
            &pieces,
            Some(9_000),
            Some(11_500)
        ));
        assert!(token_pieces_fit_sequence(
            &pieces,
            Some(9_000),
            Some(12_000)
        ));
    }

    #[test]
    fn low_coverage_does_not_invent_a_boundary() {
        let raw = CandidateSegmentInput {
            segment_index: 0,
            start_ms: 0,
            end_ms: 5_000,
            speaker_label: "S01".to_owned(),
            text: "完全不同的一大段文字".to_owned(),
        };
        let tokens = vec![token(0, 100, 500, "Google")];
        let (mut matches_by_raw, _) = token_matches(&[raw.clone()], &tokens).unwrap();
        let matches = matches_by_raw.remove(0);
        assert!(
            propose_token_pieces(&raw, &matches, &tokens, raw_normalized_len(&raw.text)).is_none()
        );
    }

    #[test]
    fn product_alignment_preserves_outer_range_and_text_and_uses_only_real_token_boundaries() {
        let input = managed(vec![ManagedSegment {
            segment_index: 0,
            t0_ms: 0,
            t1_ms: 5_000,
            speaker_id: 0,
            text: "第一段内容第二段内容第三段内容".to_owned(),
        }]);
        let token_track = track(vec![
            token(0, 100, 700, "第一段内容"),
            token(1, 1_900, 2_600, "第二段内容"),
            token(2, 3_700, 4_500, "第三段内容"),
        ]);
        let aligned =
            align_managed_transcription_with_audio_tokens(&input, &snapshot(), Some(&token_track))
                .unwrap();

        assert_eq!(concatenate_text(&aligned.product.segments), input.raw_text);
        assert_eq!(aligned.product.segments.first().unwrap().start_ms, 0);
        assert_eq!(aligned.product.segments.last().unwrap().end_ms, 5_000);
        assert_eq!(
            aligned.token_boundaries.len(),
            aligned.product.segments.len()
        );
        let native_starts = token_track
            .tokens
            .iter()
            .map(|value| value.start_ms)
            .collect::<BTreeSet<_>>();
        assert!(aligned
            .product
            .segments
            .iter()
            .skip(1)
            .all(|segment| native_starts.contains(&segment.start_ms)));
        assert!(aligned.token_boundaries.iter().all(|boundary| {
            boundary.token_track_sha256 == token_track.token_track_sha256
                && boundary.first_token_index <= boundary.last_token_index
        }));
    }

    #[test]
    fn rejected_track_falls_back_without_changing_original_moss_text_hash() {
        let input = managed(vec![ManagedSegment {
            segment_index: 0,
            t0_ms: 0,
            t1_ms: 5_000,
            speaker_id: 0,
            text: "原始 MOSS 正文永久保留".to_owned(),
        }]);
        let original_hash = sha256_text(&input.raw_text);
        let mut invalid_track = track(vec![token(0, 100, 500, "完全不同")]);
        invalid_track.audio_sha256 = "x".repeat(64);
        let aligned = align_managed_transcription_with_audio_tokens(
            &input,
            &snapshot(),
            Some(&invalid_track),
        )
        .unwrap();

        assert_eq!(
            sha256_text(&concatenate_text(&aligned.product.segments)),
            original_hash
        );
        assert!(aligned.token_boundaries.is_empty());
        assert_eq!(aligned.token_aligned_segment_count, 0);
        assert_eq!(
            aligned.token_fallback_reason.as_deref(),
            Some("AUDIO_TOKEN_BINDING_INVALID")
        );
    }

    #[test]
    fn machine_term_suggestion_requires_verified_unique_boundary_and_confidence() {
        let input = managed(vec![ManagedSegment {
            segment_index: 0,
            t0_ms: 0,
            t1_ms: 1_800,
            speaker_id: 0,
            text: "我们使用谷歌包继续".to_owned(),
        }]);
        let token_track = track(vec![
            token(0, 100, 600, "我们使用"),
            token(1, 650, 1_000, " Google"),
            token(2, 1_050, 1_500, "包继续"),
        ]);
        let aligned =
            align_managed_transcription_with_audio_tokens(&input, &snapshot(), Some(&token_track))
                .unwrap();
        let raw = managed_raw_segments(&input).unwrap();
        let terms = vec![ContextTerm {
            term_id: "term-google".to_owned(),
            canonical: "Google".to_owned(),
        }];
        let suggestions =
            machine_term_suggestions(&raw, &aligned, &token_track, &terms, &"3".repeat(64));
        assert_eq!(suggestions.len(), 1);
        assert_eq!(suggestions[0].original_text, "谷歌");
        assert_eq!(suggestions[0].replacement_text, "Google");
        assert_eq!((suggestions[0].start_char, suggestions[0].end_char), (4, 6));
        assert_eq!(
            (
                suggestions[0].first_token_index,
                suggestions[0].last_token_index
            ),
            (1, 1)
        );

        let mut unverified_alignment = aligned.clone();
        unverified_alignment.token_global_match_coverage = Some(0.49);
        assert!(machine_term_suggestions(
            &raw,
            &unverified_alignment,
            &token_track,
            &terms,
            &"3".repeat(64),
        )
        .is_empty());

        let ambiguous_terms = vec![
            terms[0].clone(),
            ContextTerm {
                term_id: "term-google-duplicate".to_owned(),
                canonical: "google".to_owned(),
            },
        ];
        assert!(machine_term_suggestions(
            &raw,
            &aligned,
            &token_track,
            &ambiguous_terms,
            &"3".repeat(64),
        )
        .is_empty());

        let mut low_confidence_track = token_track.clone();
        low_confidence_track.tokens[1].probability = 0.49;
        low_confidence_track.token_track_sha256 = sha256_audio_token_track(
            &low_confidence_track.source_chunks,
            &low_confidence_track.tokens,
        )
        .unwrap();
        let low_confidence_alignment = align_managed_transcription_with_audio_tokens(
            &input,
            &snapshot(),
            Some(&low_confidence_track),
        )
        .unwrap();
        assert!(machine_term_suggestions(
            &raw,
            &low_confidence_alignment,
            &low_confidence_track,
            &terms,
            &"3".repeat(64),
        )
        .is_empty());
    }
}

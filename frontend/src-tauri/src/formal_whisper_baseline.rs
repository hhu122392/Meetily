//! Hidden, deterministic producer for the formal "current Meetily Whisper" baseline.
//!
//! This path deliberately reuses the same decoder, VAD, silence-aware splitting,
//! Whisper engine, and transcript normalization as manual retranscription.  It
//! writes one machine-readable file and never prints transcript text.

use crate::audio::common::split_segment_at_silence;
use crate::audio::decoder::decode_audio_file;
use crate::audio::vad::get_speech_chunks;
use crate::database::moss::{
    CandidateSegmentInput, SourceTranscriptAnchor, SourceTranscriptAnchorSnapshot,
};
use crate::meeting_context::{
    normalize_and_validate_container, MeetingContextContainer, MeetingContextSnapshot,
    RecognitionContext,
};
use crate::moss_audio_token_alignment::{
    align_candidate_segments_with_audio_tokens, build_audio_token_track,
    build_audio_token_track_with_context_prompt, build_audio_token_track_with_hotword_bias,
    machine_term_suggestions, sha256_json, validate_audio_token_track, AudioTokenContextPrompt,
    AudioTokenHotwordBias, AudioTokenTrack, ContextTerm, MachineTermSuggestion, R5ProductAlignment,
    ALIGNMENT_METHOD_AUDIO_TOKEN, R5_ALIGNMENT_MODEL_NAME,
};
use crate::parakeet_engine::ParakeetEngine;
use crate::whisper_engine::acceleration::WhisperCompiledBackend;
use crate::whisper_engine::WhisperEngine;
use anyhow::{anyhow, Context, Result};
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::Instant;

const FORMAL_FLAG: &str = "--formal-whisper-baseline";
const FORMAL_PARAKEET_FLAG: &str = "--formal-parakeet-baseline";
const FORMAL_WORD_ALIGNMENT_FLAG: &str = "--formal-whisper-token-alignment";
const FORMAL_R5_PRODUCT_ALIGNMENT_FLAG: &str = "--formal-r5-product-alignment";
const FORMAL_CONTEXT_PROMPT_PROBE_FLAG: &str = "--formal-whisper-context-prompt-probe";
const FORMAL_TERMS_PROMPT_PROBE_FLAG: &str = "--formal-whisper-terms-prompt-probe";
const FORMAL_HOTWORD_BIAS_PROBE_FLAG: &str = "--formal-whisper-hotword-probe";
const VAD_REDEMPTION_TIME_MS: u32 = 2000;
const MAX_SEGMENT_SAMPLES: usize = 25 * 16_000;
const MIN_SEGMENT_SAMPLES: usize = 1_600;
const FORMAL_THREAD_STACK_BYTES: usize = 32 * 1024 * 1024;

#[derive(Debug)]
struct CliArgs {
    run_id: String,
    audio: PathBuf,
    models_dir: PathBuf,
    model_name: String,
    language: String,
    output: PathBuf,
    moss_candidate: Option<PathBuf>,
    source_transcript: Option<PathBuf>,
    metadata: Option<PathBuf>,
    token_evidence: Option<PathBuf>,
    baseline_evidence: Option<PathBuf>,
}

#[derive(Debug, Deserialize)]
struct R5RawMossCandidate {
    global_turns: Vec<R5RawMossTurn>,
    source_binding: R5RawMossSourceBinding,
}

#[derive(Debug, Deserialize)]
struct R5RawMossSourceBinding {
    audio_sha256: String,
}

#[derive(Debug, Deserialize)]
struct R5RawMossTurn {
    global_start_ms: i64,
    global_end_ms: i64,
    speaker_label: String,
    text: String,
}

#[derive(Debug, Deserialize)]
struct R5SourceTranscriptFile {
    segments: Vec<R5SourceTranscriptSegment>,
    total_segments: usize,
}

#[derive(Debug, Deserialize)]
struct R5SourceTranscriptSegment {
    sequence_id: u32,
    audio_start_time: f64,
    audio_end_time: f64,
    text: String,
}

#[derive(Debug, Deserialize)]
struct R5MeetingMetadata {
    meeting_context: MeetingContextContainer,
}

fn canonicalize_for_evidence(path: impl AsRef<Path>) -> Result<PathBuf> {
    let canonical = fs::canonicalize(path)?;
    #[cfg(windows)]
    {
        let displayed = canonical.to_string_lossy();
        if let Some(rest) = displayed.strip_prefix(r"\\?\UNC\") {
            return Ok(PathBuf::from(format!(r"\\{rest}")));
        }
        if let Some(rest) = displayed.strip_prefix(r"\\?\") {
            return Ok(PathBuf::from(rest));
        }
    }
    Ok(canonical)
}

pub fn requested() -> bool {
    std::env::args_os().any(|item| {
        item == FORMAL_FLAG
            || item == FORMAL_PARAKEET_FLAG
            || item == FORMAL_WORD_ALIGNMENT_FLAG
            || item == FORMAL_R5_PRODUCT_ALIGNMENT_FLAG
            || item == FORMAL_CONTEXT_PROMPT_PROBE_FLAG
            || item == FORMAL_TERMS_PROMPT_PROBE_FLAG
            || item == FORMAL_HOTWORD_BIAS_PROBE_FLAG
    })
}

fn requested_flag() -> &'static str {
    if std::env::args_os().any(|item| item == FORMAL_PARAKEET_FLAG) {
        FORMAL_PARAKEET_FLAG
    } else if std::env::args_os().any(|item| item == FORMAL_R5_PRODUCT_ALIGNMENT_FLAG) {
        FORMAL_R5_PRODUCT_ALIGNMENT_FLAG
    } else if std::env::args_os().any(|item| item == FORMAL_CONTEXT_PROMPT_PROBE_FLAG) {
        FORMAL_CONTEXT_PROMPT_PROBE_FLAG
    } else if std::env::args_os().any(|item| item == FORMAL_TERMS_PROMPT_PROBE_FLAG) {
        FORMAL_TERMS_PROMPT_PROBE_FLAG
    } else if std::env::args_os().any(|item| item == FORMAL_HOTWORD_BIAS_PROBE_FLAG) {
        FORMAL_HOTWORD_BIAS_PROBE_FLAG
    } else if std::env::args_os().any(|item| item == FORMAL_WORD_ALIGNMENT_FLAG) {
        FORMAL_WORD_ALIGNMENT_FLAG
    } else {
        FORMAL_FLAG
    }
}

fn parse_cli() -> Result<CliArgs> {
    let requested_flag = requested_flag();
    let mut values = BTreeMap::<String, String>::new();
    let raw: Vec<String> = std::env::args()
        .skip(1)
        .filter(|item| {
            item != FORMAL_FLAG
                && item != FORMAL_PARAKEET_FLAG
                && item != FORMAL_WORD_ALIGNMENT_FLAG
                && item != FORMAL_R5_PRODUCT_ALIGNMENT_FLAG
                && item != FORMAL_CONTEXT_PROMPT_PROBE_FLAG
                && item != FORMAL_TERMS_PROMPT_PROBE_FLAG
                && item != FORMAL_HOTWORD_BIAS_PROBE_FLAG
        })
        .collect();
    if raw.len() % 2 != 0 {
        return Err(anyhow!("formal baseline arguments must be key/value pairs"));
    }
    for pair in raw.chunks_exact(2) {
        if !pair[0].starts_with("--") || values.insert(pair[0].clone(), pair[1].clone()).is_some() {
            return Err(anyhow!("duplicate or invalid formal baseline argument"));
        }
    }
    let mut required = vec![
        "--run-id",
        "--audio",
        "--models-dir",
        "--model-name",
        "--language",
        "--output",
    ];
    if requested_flag == FORMAL_R5_PRODUCT_ALIGNMENT_FLAG {
        required.extend(["--moss-candidate", "--source-transcript", "--metadata"]);
        if values.contains_key("--token-evidence") {
            required.push("--token-evidence");
        }
    } else if requested_flag == FORMAL_CONTEXT_PROMPT_PROBE_FLAG {
        required.push("--metadata");
    } else if requested_flag == FORMAL_TERMS_PROMPT_PROBE_FLAG
        || requested_flag == FORMAL_HOTWORD_BIAS_PROBE_FLAG
    {
        required.extend(["--metadata", "--baseline-evidence"]);
    }
    if values.len() != required.len() || required.iter().any(|name| !values.contains_key(*name)) {
        return Err(anyhow!("formal baseline argument set is not exact"));
    }
    let run_id = values.remove("--run-id").unwrap();
    if run_id.len() != 32
        || !run_id
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(anyhow!(
            "run id must be 32 lowercase hexadecimal characters"
        ));
    }
    let audio = canonicalize_for_evidence(values.remove("--audio").unwrap())?;
    let models_dir = canonicalize_for_evidence(values.remove("--models-dir").unwrap())?;
    let output = PathBuf::from(values.remove("--output").unwrap());
    if !output.is_absolute() || output.exists() {
        return Err(anyhow!("output must be a new absolute path"));
    }
    let model_name = values.remove("--model-name").unwrap();
    if model_name.is_empty()
        || !model_name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
    {
        return Err(anyhow!("model name contains unsupported characters"));
    }
    let language = values.remove("--language").unwrap();
    if language != "zh" {
        return Err(anyhow!("formal Chinese baseline requires language=zh"));
    }
    let moss_candidate = values
        .remove("--moss-candidate")
        .map(canonicalize_for_evidence)
        .transpose()?;
    let source_transcript = values
        .remove("--source-transcript")
        .map(canonicalize_for_evidence)
        .transpose()?;
    let metadata = values
        .remove("--metadata")
        .map(canonicalize_for_evidence)
        .transpose()?;
    let token_evidence = values
        .remove("--token-evidence")
        .map(canonicalize_for_evidence)
        .transpose()?;
    let baseline_evidence = values
        .remove("--baseline-evidence")
        .map(canonicalize_for_evidence)
        .transpose()?;
    Ok(CliArgs {
        run_id,
        audio,
        models_dir,
        model_name,
        language,
        output,
        moss_candidate,
        source_transcript,
        metadata,
        token_evidence,
        baseline_evidence,
    })
}

fn sha256_file(path: &Path) -> Result<String> {
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
    Ok(digest
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

fn canonical_command(args: &CliArgs, executable: &Path, formal_flag: &str) -> Vec<String> {
    let mut command = vec![
        executable.display().to_string(),
        formal_flag.to_owned(),
        "--run-id".to_owned(),
        args.run_id.clone(),
        "--audio".to_owned(),
        args.audio.display().to_string(),
        "--models-dir".to_owned(),
        args.models_dir.display().to_string(),
        "--model-name".to_owned(),
        args.model_name.clone(),
        "--language".to_owned(),
        args.language.clone(),
        "--output".to_owned(),
        args.output.display().to_string(),
    ];
    if formal_flag == FORMAL_CONTEXT_PROMPT_PROBE_FLAG {
        if let Some(metadata) = args.metadata.as_ref() {
            command.extend(["--metadata".to_owned(), metadata.display().to_string()]);
        }
    } else if formal_flag == FORMAL_TERMS_PROMPT_PROBE_FLAG
        || formal_flag == FORMAL_HOTWORD_BIAS_PROBE_FLAG
    {
        if let (Some(metadata), Some(baseline_evidence)) =
            (args.metadata.as_ref(), args.baseline_evidence.as_ref())
        {
            command.extend([
                "--metadata".to_owned(),
                metadata.display().to_string(),
                "--baseline-evidence".to_owned(),
                baseline_evidence.display().to_string(),
            ]);
        }
    } else if let (Some(moss_candidate), Some(source_transcript), Some(metadata)) = (
        args.moss_candidate.as_ref(),
        args.source_transcript.as_ref(),
        args.metadata.as_ref(),
    ) {
        command.extend([
            "--moss-candidate".to_owned(),
            moss_candidate.display().to_string(),
            "--source-transcript".to_owned(),
            source_transcript.display().to_string(),
            "--metadata".to_owned(),
            metadata.display().to_string(),
        ]);
    }
    if let Some(token_evidence) = args.token_evidence.as_ref() {
        command.extend([
            "--token-evidence".to_owned(),
            token_evidence.display().to_string(),
        ]);
    }
    command
}

fn sha256_canonical_json(value: &Value) -> Result<String> {
    let encoded = serde_json::to_vec(value)?;
    let digest = Sha256::digest(encoded);
    Ok(digest.iter().map(|byte| format!("{byte:02x}")).collect())
}

fn bounded_segment_seconds(
    start_timestamp_ms: f64,
    end_timestamp_ms: f64,
    audio_duration_seconds: f64,
) -> Result<(f64, f64)> {
    if !start_timestamp_ms.is_finite()
        || !end_timestamp_ms.is_finite()
        || !audio_duration_seconds.is_finite()
        || audio_duration_seconds <= 0.0
    {
        return Err(anyhow!("stage:invalid_segment_timestamps"));
    }
    let start = (start_timestamp_ms / 1000.0)
        .max(0.0)
        .min(audio_duration_seconds);
    // VAD timestamps are millisecond based while decoded duration can end on a
    // sub-millisecond sample boundary. Clamp that rounding difference so the
    // evidence never claims speech beyond the physical end of the audio.
    let end = (end_timestamp_ms / 1000.0)
        .max(0.0)
        .min(audio_duration_seconds);
    if end <= start {
        return Err(anyhow!("stage:invalid_segment_timestamps"));
    }
    Ok((start, end))
}

fn atomic_write_new_json(path: &Path, value: &Value, run_id: &str) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| anyhow!("output has no parent"))?;
    fs::create_dir_all(parent)?;
    let filename = path
        .file_name()
        .and_then(|item| item.to_str())
        .ok_or_else(|| anyhow!("output filename is invalid"))?;
    let partial = parent.join(format!(".{filename}.{run_id}.partial"));
    let mut stream = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&partial)?;
    let encoded = serde_json::to_vec_pretty(value)?;
    stream.write_all(&encoded)?;
    stream.write_all(b"\n")?;
    stream.sync_all()?;
    drop(stream);
    if path.exists() {
        let _ = fs::remove_file(&partial);
        return Err(anyhow!("refusing to replace formal baseline output"));
    }
    fs::rename(&partial, path)?;
    Ok(())
}

async fn produce(args: &CliArgs) -> Result<()> {
    let executable = canonicalize_for_evidence(std::env::current_exe()?)?;
    let model_path = args
        .models_dir
        .join(format!("ggml-{}.bin", args.model_name));
    if !model_path.is_file() {
        return Err(anyhow!("requested Whisper model is not installed"));
    }
    let command = canonical_command(args, &executable, FORMAL_FLAG);
    let command_sha256 = sha256_canonical_json(&json!(command))?;
    let started_at = chrono::Utc::now().to_rfc3339();
    let started = Instant::now();
    let decoded = decode_audio_file(&args.audio).context("stage:audio_decode")?;
    let duration_seconds = decoded.duration_seconds;
    let audio_samples = decoded.to_whisper_format();
    let speech_segments =
        get_speech_chunks(&audio_samples, VAD_REDEMPTION_TIME_MS).context("stage:vad")?;
    if speech_segments.is_empty() {
        return Err(anyhow!("stage:no_speech_detected"));
    }
    let mut processable_segments = Vec::new();
    for segment in &speech_segments {
        if segment.samples.len() > MAX_SEGMENT_SAMPLES {
            processable_segments.extend(split_segment_at_silence(segment, MAX_SEGMENT_SAMPLES, 0));
        } else {
            processable_segments.push(segment.clone());
        }
    }
    let engine =
        WhisperEngine::new_with_models_dir(args.models_dir.clone()).context("stage:engine_init")?;
    engine
        .discover_models()
        .await
        .context("stage:model_discovery")?;
    engine
        .load_model(&args.model_name)
        .await
        .context("stage:model_load")?;
    let mut output_segments = Vec::new();
    for segment in &processable_segments {
        if segment.samples.len() < MIN_SEGMENT_SAMPLES {
            continue;
        }
        let (text, confidence, _) = engine
            .transcribe_audio_with_confidence(segment.samples.clone(), Some(args.language.clone()))
            .await
            .context("stage:transcription")?;
        if !text.trim().is_empty() {
            if !confidence.is_finite() {
                return Err(anyhow!("stage:invalid_segment_confidence"));
            }
            let (start, end) = bounded_segment_seconds(
                segment.start_timestamp_ms,
                segment.end_timestamp_ms,
                duration_seconds,
            )?;
            output_segments.push(json!({
                "start": start,
                "end": end,
                "speaker": "",
                "text": text,
                "confidence": confidence,
            }));
        }
    }
    if output_segments.is_empty() {
        return Err(anyhow!("stage:empty_transcript"));
    }
    let payload = json!({
        "schema_version": 1,
        "role": "FORMAL_CURRENT_MEETILY_WHISPER_RAW_OUTPUT",
        "run_id": args.run_id,
        "started_at": started_at,
        "finished_at": chrono::Utc::now().to_rfc3339(),
        "elapsed_seconds": started.elapsed().as_secs_f64(),
        "command_sha256": command_sha256,
        "input_audio_sha256": sha256_file(&args.audio).context("stage:audio_hash")?,
        "input_audio_duration_seconds": duration_seconds,
        "stable_exe_sha256": sha256_file(&executable).context("stage:executable_hash")?,
        "model_name": args.model_name,
        "model_file_sha256": sha256_file(&model_path).context("stage:model_hash")?,
        "backend": WhisperCompiledBackend::current().as_str(),
        "language": args.language,
        "decode_parameters": {
            "product_path": "manual_retranscription_equivalent",
            "decoder": "Meetily audio::decoder::decode_audio_file + to_whisper_format",
            "sample_rate_hz": 16000,
            "channels": 1,
            "vad": "Meetily ContinuousVadProcessor",
            "vad_redemption_time_ms": VAD_REDEMPTION_TIME_MS,
            "maximum_segment_samples": MAX_SEGMENT_SAMPLES,
            "minimum_segment_samples": MIN_SEGMENT_SAMPLES,
            "splitter": "Meetily silence-aware split_segment_at_silence",
            "engine_method": "WhisperEngine::transcribe_audio_with_confidence",
            "formal_worker_thread_stack_bytes": FORMAL_THREAD_STACK_BYTES,
            "recognition_prompt_status": "disabled_pending_safety_gate",
            "recognition_normalization": "not_configured"
        },
        "speech_segment_count_before_split": speech_segments.len(),
        "speech_segment_count_after_split": processable_segments.len(),
        "segments": output_segments,
    });
    atomic_write_new_json(&args.output, &payload, &args.run_id).context("stage:output_write")
}

async fn produce_word_alignment(args: &CliArgs) -> Result<()> {
    let executable = canonicalize_for_evidence(std::env::current_exe()?)?;
    let command = canonical_command(args, &executable, FORMAL_WORD_ALIGNMENT_FLAG);
    let command_sha256 = sha256_canonical_json(&json!(command))?;
    let started_at = chrono::Utc::now().to_rfc3339();
    let started = Instant::now();
    let track = build_audio_token_track(
        &args.audio,
        &args.models_dir,
        &args.model_name,
        &args.language,
    )
    .await
    .context("stage:token_track")?;
    let mut payload = json!({
        "schema_version": 1,
        "role": "FORMAL_CURRENT_MEETILY_WHISPER_TOKEN_TRACK",
        "run_id": args.run_id,
        "started_at": started_at,
        "finished_at": chrono::Utc::now().to_rfc3339(),
        "elapsed_seconds": started.elapsed().as_secs_f64(),
        "command_sha256": command_sha256,
        "input_audio_sha256": track.audio_sha256,
        "input_audio_duration_ms": track.audio_duration_ms,
        "stable_exe_sha256": track.program_sha256,
        "model_name": track.model_name,
        "model_file_sha256": track.model_sha256,
        "backend": track.backend,
        "parameters_sha256": track.parameters_sha256,
        "decode_parameters": track.parameters,
        "source_chunks": track.source_chunks,
        "token_count": track.tokens.len(),
        "token_track_sha256": track.token_track_sha256,
        "tokens": track.tokens,
    });
    let payload_sha256 = sha256_json(&payload).context("stage:payload_hash")?;
    payload
        .as_object_mut()
        .ok_or_else(|| anyhow!("stage:payload_shape"))?
        .insert("payload_sha256".to_owned(), json!(payload_sha256));
    atomic_write_new_json(&args.output, &payload, &args.run_id).context("stage:output_write")
}

async fn produce_context_prompt_probe(args: &CliArgs) -> Result<()> {
    if args.model_name != R5_ALIGNMENT_MODEL_NAME {
        return Err(anyhow!("stage:context_probe_model_not_frozen"));
    }
    let metadata_path = args
        .metadata
        .as_deref()
        .ok_or_else(|| anyhow!("stage:context_probe_metadata_missing"))?;
    let executable = canonicalize_for_evidence(std::env::current_exe()?)?;
    let model_path = canonicalize_for_evidence(
        args.models_dir
            .join(format!("ggml-{}.bin", args.model_name)),
    )
    .context("stage:model_path")?;
    let command = canonical_command(args, &executable, FORMAL_CONTEXT_PROMPT_PROBE_FLAG);
    let command_sha256 = sha256_canonical_json(&json!(command))?;
    let started_at = chrono::Utc::now().to_rfc3339();
    let started = Instant::now();

    let metadata_sha256 = sha256_file(metadata_path).context("stage:metadata_hash")?;
    let metadata: R5MeetingMetadata =
        read_json_file(metadata_path).context("stage:metadata_parse")?;
    let context_snapshot = recording_context_snapshot(metadata.meeting_context)?;
    let recognition_context = RecognitionContext::from_snapshot(&context_snapshot);
    let context_diagnostics = recognition_context.diagnostics();
    if context_diagnostics.live_prompt_enabled {
        return Err(anyhow!(
            "stage:context_probe_live_prompt_must_remain_disabled"
        ));
    }
    let prompt = recognition_context
        .whisper_initial_prompt
        .as_deref()
        .ok_or_else(|| anyhow!("stage:context_probe_prompt_missing"))?;
    let prompt_sha256 = sha256_text(prompt);
    if recognition_context.prompt_sha256.as_deref() != Some(prompt_sha256.as_str())
        || context_diagnostics.prompt_chars != prompt.chars().count()
    {
        return Err(anyhow!("stage:context_probe_prompt_binding"));
    }

    let baseline = build_audio_token_track(
        &args.audio,
        &args.models_dir,
        &args.model_name,
        &args.language,
    )
    .await
    .context("stage:context_probe_baseline_track")?;
    let prompted = build_audio_token_track_with_context_prompt(
        &args.audio,
        &args.models_dir,
        &args.model_name,
        &args.language,
        AudioTokenContextPrompt {
            text: prompt,
            context_sha256: &context_snapshot.context_sha256,
            truncated: recognition_context.prompt_truncated,
        },
    )
    .await
    .context("stage:context_probe_prompted_track")?;
    if baseline.audio_sha256 != prompted.audio_sha256
        || baseline.audio_duration_ms != prompted.audio_duration_ms
        || baseline.model_sha256 != prompted.model_sha256
        || baseline.program_sha256 != prompted.program_sha256
        || baseline.parameters.initial_prompt_used
        || !prompted.parameters.initial_prompt_used
        || prompted.parameters.context_sha256.as_deref()
            != Some(context_snapshot.context_sha256.as_str())
        || prompted.parameters.prompt_sha256.as_deref() != Some(prompt_sha256.as_str())
    {
        return Err(anyhow!("stage:context_probe_track_binding"));
    }

    let mut payload = json!({
        "schema_version": 1,
        "role": "FORMAL_WHISPER_CONTEXT_PROMPT_PROBE",
        "run_id": args.run_id,
        "started_at": started_at,
        "finished_at": chrono::Utc::now().to_rfc3339(),
        "elapsed_seconds": started.elapsed().as_secs_f64(),
        "command": command,
        "command_sha256": command_sha256,
        "inputs": {
            "audio": {
                "path": args.audio.display().to_string(),
                "sha256": baseline.audio_sha256.as_str(),
                "duration_ms": baseline.audio_duration_ms,
            },
            "meeting_metadata": {
                "path": metadata_path.display().to_string(),
                "sha256": metadata_sha256,
            },
            "model": {
                "path": model_path.display().to_string(),
                "name": args.model_name,
                "sha256": baseline.model_sha256.as_str(),
            },
            "program": {
                "path": executable.display().to_string(),
                "sha256": baseline.program_sha256.as_str(),
            }
        },
        "recognition_context": context_diagnostics,
        "privacy": {
            "prompt_plaintext_stored": false,
            "prompt_plaintext_logged": false,
            "machine_transcript_tokens_stored": true
        },
        "safety": {
            "live_prompt_enabled": false,
            "live_prompt_configuration_modified": false,
            "human_truth_used": false,
            "prompt_source": "recording_start MeetingContextSnapshot"
        },
        "baseline": {
            "initial_prompt_used": false,
            "parameters_sha256": baseline.parameters_sha256.as_str(),
            "token_track_sha256": baseline.token_track_sha256.as_str(),
            "token_count": baseline.tokens.len(),
            "audio_token_track": baseline,
        },
        "prompted": {
            "initial_prompt_used": true,
            "context_sha256": context_snapshot.context_sha256,
            "prompt_sha256": prompt_sha256,
            "prompt_chars": prompt.chars().count(),
            "prompt_truncated": recognition_context.prompt_truncated,
            "parameters_sha256": prompted.parameters_sha256.as_str(),
            "token_track_sha256": prompted.token_track_sha256.as_str(),
            "token_count": prompted.tokens.len(),
            "audio_token_track": prompted,
        }
    });
    let payload_sha256 = sha256_json(&payload).context("stage:payload_hash")?;
    payload
        .as_object_mut()
        .ok_or_else(|| anyhow!("stage:payload_shape"))?
        .insert("payload_sha256".to_owned(), json!(payload_sha256));
    atomic_write_new_json(&args.output, &payload, &args.run_id).context("stage:output_write")
}

async fn produce_terms_prompt_probe(args: &CliArgs) -> Result<()> {
    if args.model_name != R5_ALIGNMENT_MODEL_NAME {
        return Err(anyhow!("stage:r9_model_not_frozen"));
    }
    let metadata_path = args
        .metadata
        .as_deref()
        .ok_or_else(|| anyhow!("stage:r9_metadata_missing"))?;
    let baseline_evidence_path = args
        .baseline_evidence
        .as_deref()
        .ok_or_else(|| anyhow!("stage:r9_baseline_evidence_missing"))?;
    let executable = canonicalize_for_evidence(std::env::current_exe()?)?;
    let model_path = canonicalize_for_evidence(
        args.models_dir
            .join(format!("ggml-{}.bin", args.model_name)),
    )
    .context("stage:r9_model_path")?;
    let command = canonical_command(args, &executable, FORMAL_TERMS_PROMPT_PROBE_FLAG);
    let command_sha256 = sha256_canonical_json(&json!(command))?;
    let started_at = chrono::Utc::now().to_rfc3339();
    let started = Instant::now();

    let audio_sha256 = sha256_file(&args.audio).context("stage:r9_audio_hash")?;
    let model_sha256 = sha256_file(&model_path).context("stage:r9_model_hash")?;
    let metadata_sha256 = sha256_file(metadata_path).context("stage:r9_metadata_hash")?;
    let metadata: R5MeetingMetadata =
        read_json_file(metadata_path).context("stage:r9_metadata_parse")?;
    let context_snapshot = recording_context_snapshot(metadata.meeting_context)?;
    let recognition_context = RecognitionContext::from_snapshot(&context_snapshot);
    let context_diagnostics = recognition_context.diagnostics();
    if context_diagnostics.live_prompt_enabled {
        return Err(anyhow!("stage:r9_live_prompt_must_remain_disabled"));
    }
    let (term_prompt, prompt_truncated) = recognition_context.term_only_whisper_prompt();
    let prompt = term_prompt
        .as_deref()
        .ok_or_else(|| anyhow!("stage:r9_term_prompt_missing"))?;
    let prompt_sha256 = sha256_text(prompt);
    let prompt_chars = prompt.chars().count();
    if recognition_context.canonical_terms.is_empty() || prompt_chars == 0 {
        return Err(anyhow!("stage:r9_term_prompt_binding"));
    }

    let baseline = load_reused_context_probe_baseline(
        baseline_evidence_path,
        &audio_sha256,
        &model_sha256,
        &context_snapshot.context_sha256,
        &args.language,
    )?;
    let prompted = build_audio_token_track_with_context_prompt(
        &args.audio,
        &args.models_dir,
        &args.model_name,
        &args.language,
        AudioTokenContextPrompt {
            text: prompt,
            context_sha256: &context_snapshot.context_sha256,
            truncated: prompt_truncated,
        },
    )
    .await
    .context("stage:r9_prompted_track")?;
    if prompted.audio_sha256 != baseline.track.audio_sha256
        || prompted.audio_duration_ms != baseline.track.audio_duration_ms
        || prompted.model_sha256 != baseline.track.model_sha256
        || !prompted.parameters.initial_prompt_used
        || prompted.parameters.context_sha256.as_deref()
            != Some(context_snapshot.context_sha256.as_str())
        || prompted.parameters.prompt_sha256.as_deref() != Some(prompt_sha256.as_str())
        || prompted.parameters.prompt_chars != Some(prompt_chars)
        || prompted.parameters.prompt_truncated != Some(prompt_truncated)
    {
        return Err(anyhow!("stage:r9_prompted_track_binding"));
    }

    let mut payload = json!({
        "schema_version": 1,
        "role": "FORMAL_WHISPER_TERMS_PROMPT_PROBE",
        "run_id": args.run_id,
        "started_at": started_at,
        "finished_at": chrono::Utc::now().to_rfc3339(),
        "elapsed_seconds": started.elapsed().as_secs_f64(),
        "command": command,
        "command_sha256": command_sha256,
        "inputs": {
            "audio": {
                "path": args.audio.display().to_string(),
                "sha256": audio_sha256,
                "duration_ms": prompted.audio_duration_ms,
            },
            "meeting_metadata": {
                "path": metadata_path.display().to_string(),
                "sha256": metadata_sha256,
            },
            "model": {
                "path": model_path.display().to_string(),
                "name": args.model_name,
                "sha256": model_sha256,
            },
            "program": {
                "path": executable.display().to_string(),
                "sha256": prompted.program_sha256.as_str(),
            },
            "reused_baseline_evidence": {
                "path": baseline_evidence_path.display().to_string(),
                "sha256": baseline.evidence_sha256,
                "payload_sha256": baseline.payload_sha256,
            }
        },
        "recognition_context": {
            "context_id": context_diagnostics.context_id,
            "context_sha256": context_diagnostics.context_sha256,
            "canonical_term_count": context_diagnostics.canonical_term_count,
            "term_alias_count": context_diagnostics.term_alias_count,
            "live_prompt_enabled": context_diagnostics.live_prompt_enabled,
        },
        "prompt": {
            "scope": "canonical_terms_only",
            "canonical_term_count": recognition_context.canonical_terms.len(),
            "sha256": prompt_sha256,
            "chars": prompt_chars,
            "truncated": prompt_truncated,
            "plaintext_stored": false,
            "names_in_input": false,
            "aliases_in_input": false,
        },
        "safety": {
            "live_prompt_configuration_modified": false,
            "human_truth_used": false,
            "mc_r02_prompt_hash_reused": false,
            "mc_r02_safety_gate_applicable_to_this_prompt": false,
            "product_connection_allowed_by_this_probe": false,
        },
        "baseline_reference": {
            "inference_repeated": false,
            "program_sha256": baseline.track.program_sha256,
            "parameters_sha256": baseline.track.parameters_sha256,
            "token_track_sha256": baseline.track.token_track_sha256,
            "token_count": baseline.track.tokens.len(),
        },
        "prompted": {
            "inference_executed": true,
            "parameters_sha256": prompted.parameters_sha256.as_str(),
            "token_track_sha256": prompted.token_track_sha256.as_str(),
            "token_count": prompted.tokens.len(),
            "audio_token_track": prompted,
        }
    });
    let payload_sha256 = sha256_json(&payload).context("stage:r9_payload_hash")?;
    payload
        .as_object_mut()
        .ok_or_else(|| anyhow!("stage:r9_payload_shape"))?
        .insert("payload_sha256".to_owned(), json!(payload_sha256));
    atomic_write_new_json(&args.output, &payload, &args.run_id).context("stage:r9_output_write")
}

async fn produce_hotword_bias_probe(args: &CliArgs) -> Result<()> {
    if args.model_name != R5_ALIGNMENT_MODEL_NAME {
        return Err(anyhow!("stage:r10_model_not_frozen"));
    }
    let metadata_path = args
        .metadata
        .as_deref()
        .ok_or_else(|| anyhow!("stage:r10_metadata_missing"))?;
    let baseline_evidence_path = args
        .baseline_evidence
        .as_deref()
        .ok_or_else(|| anyhow!("stage:r10_baseline_evidence_missing"))?;
    let executable = canonicalize_for_evidence(std::env::current_exe()?)?;
    let model_path = canonicalize_for_evidence(
        args.models_dir
            .join(format!("ggml-{}.bin", args.model_name)),
    )
    .context("stage:r10_model_path")?;
    let command = canonical_command(args, &executable, FORMAL_HOTWORD_BIAS_PROBE_FLAG);
    let command_sha256 = sha256_canonical_json(&json!(command))?;
    let started_at = chrono::Utc::now().to_rfc3339();
    let started = Instant::now();

    let audio_sha256 = sha256_file(&args.audio).context("stage:r10_audio_hash")?;
    let model_sha256 = sha256_file(&model_path).context("stage:r10_model_hash")?;
    let metadata_sha256 = sha256_file(metadata_path).context("stage:r10_metadata_hash")?;
    let metadata: R5MeetingMetadata =
        read_json_file(metadata_path).context("stage:r10_metadata_parse")?;
    let context_snapshot = recording_context_snapshot(metadata.meeting_context)?;
    let recognition_context = RecognitionContext::from_snapshot(&context_snapshot);
    let context_diagnostics = recognition_context.diagnostics();
    if context_diagnostics.live_prompt_enabled {
        return Err(anyhow!("stage:r10_live_prompt_must_remain_disabled"));
    }
    if recognition_context.canonical_terms.is_empty() {
        return Err(anyhow!("stage:r10_canonical_terms_missing"));
    }
    let expected_canonical_terms_sha256 =
        sha256_json(&recognition_context.canonical_terms).context("stage:r10_terms_hash")?;

    let baseline = load_reused_context_probe_baseline(
        baseline_evidence_path,
        &audio_sha256,
        &model_sha256,
        &context_snapshot.context_sha256,
        &args.language,
    )?;
    let biased = build_audio_token_track_with_hotword_bias(
        &args.audio,
        &args.models_dir,
        &args.model_name,
        &args.language,
        AudioTokenHotwordBias {
            context_sha256: &context_snapshot.context_sha256,
            canonical_terms: &recognition_context.canonical_terms,
        },
    )
    .await
    .context("stage:r10_hotword_biased_track")?;
    let bias_parameters = biased
        .parameters
        .hotword_bias
        .as_ref()
        .ok_or_else(|| anyhow!("stage:r10_hotword_parameters_missing"))?;
    let bias_diagnostics = biased
        .hotword_bias_diagnostics
        .as_ref()
        .ok_or_else(|| anyhow!("stage:r10_hotword_diagnostics_missing"))?;
    if biased.audio_sha256 != baseline.track.audio_sha256
        || biased.audio_duration_ms != baseline.track.audio_duration_ms
        || biased.model_sha256 != baseline.track.model_sha256
        || biased.parameters.initial_prompt_used
        || biased.parameters.prompt_sha256.is_some()
        || biased.parameters.prompt_chars.is_some()
        || biased.parameters.prompt_truncated.is_some()
        || biased.parameters.context_sha256.as_deref()
            != Some(context_snapshot.context_sha256.as_str())
        || baseline.track.hotword_bias_diagnostics.is_some()
        || bias_parameters.canonical_term_count != recognition_context.canonical_terms.len()
        || bias_diagnostics.canonical_term_count != recognition_context.canonical_terms.len()
        || bias_diagnostics.canonical_terms_sha256 != expected_canonical_terms_sha256
        || bias_parameters.binding_sha256 != bias_diagnostics.binding_sha256
    {
        return Err(anyhow!("stage:r10_hotword_track_binding"));
    }

    let bias_binding_sha256 = bias_diagnostics.binding_sha256.clone();
    let canonical_terms_sha256 = bias_diagnostics.canonical_terms_sha256.clone();
    let token_sequence_set_sha256 = bias_diagnostics.token_sequence_set_sha256.clone();
    let token_sequence_count = bias_diagnostics.token_sequence_count;
    let decode_call_count = bias_diagnostics.decode_call_count;
    let callback_invocations = bias_diagnostics.callback_invocations;
    let callbacks_with_adjustments = bias_diagnostics.callbacks_with_adjustments;
    let logit_adjustment_count = bias_diagnostics.logit_adjustment_count;
    let maximum_adjustments_in_one_callback = bias_diagnostics.maximum_adjustments_in_one_callback;
    let mut payload = json!({
        "schema_version": 1,
        "role": "FORMAL_WHISPER_HOTWORD_BIAS_PROBE",
        "run_id": args.run_id,
        "started_at": started_at,
        "finished_at": chrono::Utc::now().to_rfc3339(),
        "elapsed_seconds": started.elapsed().as_secs_f64(),
        "command": command,
        "command_sha256": command_sha256,
        "inputs": {
            "audio": {
                "path": args.audio.display().to_string(),
                "sha256": audio_sha256,
                "duration_ms": biased.audio_duration_ms,
            },
            "meeting_metadata": {
                "path": metadata_path.display().to_string(),
                "sha256": metadata_sha256,
            },
            "model": {
                "path": model_path.display().to_string(),
                "name": args.model_name,
                "sha256": model_sha256,
            },
            "program": {
                "path": executable.display().to_string(),
                "sha256": biased.program_sha256.as_str(),
            },
            "reused_baseline_evidence": {
                "path": baseline_evidence_path.display().to_string(),
                "sha256": baseline.evidence_sha256,
                "payload_sha256": baseline.payload_sha256,
            }
        },
        "recognition_context": {
            "context_id": context_diagnostics.context_id,
            "context_sha256": context_diagnostics.context_sha256,
            "canonical_term_count": context_diagnostics.canonical_term_count,
            "term_alias_count": context_diagnostics.term_alias_count,
            "live_prompt_enabled": context_diagnostics.live_prompt_enabled,
        },
        "hotword_bias": {
            "scope": "canonical_terms_only",
            "canonical_term_count": recognition_context.canonical_terms.len(),
            "canonical_terms_sha256": canonical_terms_sha256,
            "binding_sha256": bias_binding_sha256,
            "token_sequence_set_sha256": token_sequence_set_sha256,
            "token_sequence_count": token_sequence_count,
            "decode_call_count": decode_call_count,
            "callback_invocations": callback_invocations,
            "callbacks_with_adjustments": callbacks_with_adjustments,
            "logit_adjustment_count": logit_adjustment_count,
            "maximum_adjustments_in_one_callback": maximum_adjustments_in_one_callback,
            "canonical_term_plaintext_stored_as_configuration": false,
            "raw_token_ids_stored": false,
            "names_in_input": false,
            "aliases_in_input": false,
            "initial_prompt_used": false,
        },
        "privacy": {
            "hotword_plaintext_logged": false,
            "machine_transcript_tokens_stored": true,
        },
        "safety": {
            "live_prompt_configuration_modified": false,
            "human_truth_used": false,
            "product_connection_allowed_by_this_probe": false,
            "stable_executable_replacement_allowed_by_this_probe": false,
        },
        "baseline_reference": {
            "inference_repeated": false,
            "program_sha256": baseline.track.program_sha256,
            "parameters_sha256": baseline.track.parameters_sha256,
            "token_track_sha256": baseline.track.token_track_sha256,
            "token_count": baseline.track.tokens.len(),
        },
        "biased": {
            "inference_executed": true,
            "parameters_sha256": biased.parameters_sha256.as_str(),
            "token_track_sha256": biased.token_track_sha256.as_str(),
            "token_count": biased.tokens.len(),
            "audio_token_track": biased,
        }
    });
    let payload_sha256 = sha256_json(&payload).context("stage:r10_payload_hash")?;
    payload
        .as_object_mut()
        .ok_or_else(|| anyhow!("stage:r10_payload_shape"))?
        .insert("payload_sha256".to_owned(), json!(payload_sha256));
    atomic_write_new_json(&args.output, &payload, &args.run_id).context("stage:r10_output_write")
}

fn read_json_file<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T> {
    let bytes = fs::read(path)?;
    serde_json::from_slice(&bytes).map_err(Into::into)
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn sha256_text(value: &str) -> String {
    format!("{:x}", Sha256::digest(value.as_bytes()))
}

fn valid_speaker_label(value: &str) -> bool {
    value
        .strip_prefix('S')
        .is_some_and(|digits| digits.len() >= 2 && digits.bytes().all(|byte| byte.is_ascii_digit()))
}

fn r5_raw_segments(
    candidate: &R5RawMossCandidate,
    audio_sha256: &str,
) -> Result<Vec<CandidateSegmentInput>> {
    if candidate.global_turns.is_empty()
        || !is_sha256(&candidate.source_binding.audio_sha256)
        || !candidate
            .source_binding
            .audio_sha256
            .eq_ignore_ascii_case(audio_sha256)
    {
        return Err(anyhow!("stage:raw_moss_binding"));
    }
    let segments = candidate
        .global_turns
        .iter()
        .enumerate()
        .map(|(index, turn)| {
            if turn.global_start_ms < 0
                || turn.global_end_ms <= turn.global_start_ms
                || turn.text.trim().is_empty()
                || !valid_speaker_label(&turn.speaker_label)
            {
                return Err(anyhow!("stage:raw_moss_shape"));
            }
            Ok(CandidateSegmentInput {
                segment_index: u32::try_from(index)
                    .map_err(|_| anyhow!("stage:raw_moss_segment_count"))?,
                start_ms: turn.global_start_ms,
                end_ms: turn.global_end_ms,
                speaker_label: turn.speaker_label.clone(),
                text: turn.text.clone(),
            })
        })
        .collect::<Result<Vec<_>>>()?;
    if segments
        .windows(2)
        .any(|pair| pair[1].start_ms < pair[0].start_ms)
    {
        return Err(anyhow!("stage:raw_moss_order"));
    }
    Ok(segments)
}

fn seconds_to_milliseconds(value: f64) -> Result<i64> {
    let milliseconds = value * 1_000.0;
    if !value.is_finite()
        || value < 0.0
        || !milliseconds.is_finite()
        || milliseconds > i64::MAX as f64
    {
        return Err(anyhow!("stage:source_transcript_time"));
    }
    Ok(milliseconds.round() as i64)
}

fn r5_source_snapshot(
    transcript: &R5SourceTranscriptFile,
    source_sha256: &str,
) -> Result<SourceTranscriptAnchorSnapshot> {
    if !is_sha256(source_sha256)
        || transcript.segments.is_empty()
        || transcript.total_segments != transcript.segments.len()
    {
        return Err(anyhow!("stage:source_transcript_shape"));
    }
    let anchors = transcript
        .segments
        .iter()
        .enumerate()
        .map(|(index, segment)| {
            if usize::try_from(segment.sequence_id).ok() != Some(index)
                || segment.text.trim().is_empty()
            {
                return Err(anyhow!("stage:source_transcript_sequence"));
            }
            let start_ms = seconds_to_milliseconds(segment.audio_start_time)?;
            let end_ms = seconds_to_milliseconds(segment.audio_end_time)?;
            if end_ms <= start_ms {
                return Err(anyhow!("stage:source_transcript_time"));
            }
            Ok(SourceTranscriptAnchor {
                anchor_id: format!("source-transcript-{:06}", segment.sequence_id),
                start_ms,
                end_ms,
                text: segment.text.clone(),
            })
        })
        .collect::<Result<Vec<_>>>()?;
    if anchors
        .windows(2)
        .any(|pair| pair[1].start_ms < pair[0].start_ms)
    {
        return Err(anyhow!("stage:source_transcript_order"));
    }
    Ok(SourceTranscriptAnchorSnapshot {
        expected_sha256: source_sha256.to_owned(),
        actual_sha256: source_sha256.to_owned(),
        hash_verified: true,
        invalid_timed_rows: 0,
        anchors,
    })
}

fn r5_context(
    container: MeetingContextContainer,
) -> Result<(MeetingContextSnapshot, Vec<ContextTerm>)> {
    let snapshot = recording_context_snapshot(container)?;
    if snapshot.terms.is_empty() {
        return Err(anyhow!("stage:meeting_context_not_pretranscription"));
    }
    let terms = snapshot
        .terms
        .iter()
        .map(|term| ContextTerm {
            term_id: term.term_id.clone(),
            canonical: term.canonical.clone(),
        })
        .collect::<Vec<_>>();
    Ok((snapshot, terms))
}

fn recording_context_snapshot(
    container: MeetingContextContainer,
) -> Result<MeetingContextSnapshot> {
    let container = normalize_and_validate_container(container).map_err(|issues| {
        let codes = issues
            .iter()
            .map(|issue| issue.code.as_str())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>()
            .join(",");
        anyhow!("stage:meeting_context_validation:{codes}")
    })?;
    if container.recording_context_id != container.current_context_id {
        return Err(anyhow!("stage:meeting_context_not_recording_snapshot"));
    }
    let snapshot = container
        .contexts
        .iter()
        .find(|snapshot| snapshot.context_id == container.current_context_id)
        .cloned()
        .ok_or_else(|| anyhow!("stage:meeting_context_current_missing"))?;
    if snapshot.reason != "recording_start" {
        return Err(anyhow!("stage:meeting_context_not_pretranscription"));
    }
    Ok(snapshot)
}

struct R5TrackSelection {
    track: AudioTokenTrack,
    origin: Value,
    token_producer_program_path: PathBuf,
}

struct ReusedContextProbeBaseline {
    track: AudioTokenTrack,
    evidence_sha256: String,
    payload_sha256: String,
}

fn evidence_string<'a>(value: &'a Value, pointer: &str, label: &str) -> Result<&'a str> {
    value
        .pointer(pointer)
        .and_then(Value::as_str)
        .filter(|text| !text.trim().is_empty())
        .ok_or_else(|| anyhow!("stage:reused_token_evidence_{label}"))
}

fn load_reused_context_probe_baseline(
    evidence_path: &Path,
    audio_sha256: &str,
    model_sha256: &str,
    context_sha256: &str,
    language: &str,
) -> Result<ReusedContextProbeBaseline> {
    let evidence_sha256 = sha256_file(evidence_path).context("stage:r9_baseline_evidence_hash")?;
    let mut evidence: Value =
        read_json_file(evidence_path).context("stage:r9_baseline_evidence_parse")?;
    if evidence_string(&evidence, "/role", "r9_role")? != "FORMAL_WHISPER_CONTEXT_PROMPT_PROBE" {
        return Err(anyhow!("stage:r9_baseline_evidence_role"));
    }
    let payload_sha256 = evidence_string(&evidence, "/payload_sha256", "r9_payload")?.to_owned();
    if !is_sha256(&payload_sha256) {
        return Err(anyhow!("stage:r9_baseline_payload_hash_shape"));
    }
    evidence
        .as_object_mut()
        .ok_or_else(|| anyhow!("stage:r9_baseline_evidence_shape"))?
        .remove("payload_sha256")
        .ok_or_else(|| anyhow!("stage:r9_baseline_payload_missing"))?;
    let track: AudioTokenTrack = serde_json::from_value(
        evidence
            .pointer("/baseline/audio_token_track")
            .cloned()
            .ok_or_else(|| anyhow!("stage:r9_baseline_track_missing"))?,
    )
    .context("stage:r9_baseline_track_parse")?;
    let prompted_track: AudioTokenTrack = serde_json::from_value(
        evidence
            .pointer("/prompted/audio_token_track")
            .cloned()
            .ok_or_else(|| anyhow!("stage:r9_prompted_track_missing"))?,
    )
    .context("stage:r9_prompted_track_parse")?;
    // The R8 producer hashed strongly typed tracks containing f32 probabilities. Rehydrate
    // both tracks before hashing so JSON f64 parsing cannot create a false mismatch.
    *evidence
        .pointer_mut("/baseline/audio_token_track")
        .ok_or_else(|| anyhow!("stage:r9_baseline_track_missing"))? =
        serde_json::to_value(&track).context("stage:r9_baseline_track_serialize")?;
    *evidence
        .pointer_mut("/prompted/audio_token_track")
        .ok_or_else(|| anyhow!("stage:r9_prompted_track_missing"))? =
        serde_json::to_value(&prompted_track).context("stage:r9_prompted_track_serialize")?;
    if sha256_json(&evidence).context("stage:r9_baseline_payload_rehash")? != payload_sha256 {
        return Err(anyhow!("stage:r9_baseline_payload_hash"));
    }
    for (pointer, expected, label) in [
        ("/inputs/audio/sha256", audio_sha256, "audio"),
        ("/inputs/model/sha256", model_sha256, "model"),
        (
            "/recognition_context/context_sha256",
            context_sha256,
            "context",
        ),
    ] {
        if !evidence_string(&evidence, pointer, label)?.eq_ignore_ascii_case(expected) {
            return Err(anyhow!("stage:r9_baseline_{label}_binding"));
        }
    }
    validate_audio_token_track(&track, None)
        .map_err(|error| anyhow!("stage:r9_baseline_track_invalid:{error}"))?;
    if track.audio_sha256 != audio_sha256
        || track.model_sha256 != model_sha256
        || track.parameters.language != language
        || track.parameters.initial_prompt_used
        || evidence_string(&evidence, "/baseline/parameters_sha256", "r9_parameters")?
            != track.parameters_sha256
        || evidence_string(&evidence, "/baseline/token_track_sha256", "r9_token_track")?
            != track.token_track_sha256
    {
        return Err(anyhow!("stage:r9_baseline_track_binding"));
    }
    Ok(ReusedContextProbeBaseline {
        track,
        evidence_sha256,
        payload_sha256,
    })
}

fn load_reused_r5_track(
    evidence_path: &Path,
    args: &CliArgs,
    audio_sha256: &str,
    moss_candidate_sha256: &str,
    source_transcript_sha256: &str,
    metadata_sha256: &str,
) -> Result<R5TrackSelection> {
    let evidence_sha256 = sha256_file(evidence_path).context("stage:token_evidence_hash")?;
    let mut evidence: Value =
        read_json_file(evidence_path).context("stage:token_evidence_parse")?;
    if evidence_string(&evidence, "/role", "role")? != "FORMAL_MEETILY_R5_PRODUCT_ALIGNMENT"
        || evidence
            .pointer("/human_truth_used_for_alignment_or_correction")
            .and_then(Value::as_bool)
            != Some(false)
        || evidence
            .pointer("/invariants/all_pass")
            .and_then(Value::as_bool)
            != Some(true)
    {
        return Err(anyhow!("stage:token_evidence_not_formal"));
    }
    let expected_payload_sha256 =
        evidence_string(&evidence, "/payload_sha256", "payload_hash")?.to_owned();
    if !is_sha256(&expected_payload_sha256) {
        return Err(anyhow!("stage:token_evidence_payload_hash"));
    }
    evidence
        .as_object_mut()
        .ok_or_else(|| anyhow!("stage:token_evidence_shape"))?
        .remove("payload_sha256");
    let track: AudioTokenTrack = serde_json::from_value(
        evidence
            .pointer("/audio_token_track")
            .cloned()
            .ok_or_else(|| anyhow!("stage:token_evidence_track_missing"))?,
    )
    .context("stage:token_evidence_track_parse")?;
    // The producer hashed the strongly typed track, including f32 probabilities. Re-serialize
    // that type before checking the full payload so a JSON f32 -> f64 parse does not create a
    // false hash mismatch while every original bit-level value is still checked.
    evidence
        .as_object_mut()
        .ok_or_else(|| anyhow!("stage:token_evidence_shape"))?
        .insert(
            "audio_token_track".to_owned(),
            serde_json::to_value(&track).context("stage:token_evidence_track_serialize")?,
        );
    if sha256_json(&evidence).context("stage:token_evidence_payload_hash")?
        != expected_payload_sha256
    {
        return Err(anyhow!("stage:token_evidence_payload_hash"));
    }
    validate_audio_token_track(&track, None)
        .map_err(|error| anyhow!("stage:token_evidence_track_invalid:{error}"))?;

    for (pointer, expected, label) in [
        ("/inputs/audio/sha256", audio_sha256, "audio"),
        (
            "/inputs/raw_moss_candidate/sha256",
            moss_candidate_sha256,
            "raw_moss",
        ),
        (
            "/inputs/source_transcript/sha256",
            source_transcript_sha256,
            "source_transcript",
        ),
        (
            "/inputs/meeting_metadata/sha256",
            metadata_sha256,
            "metadata",
        ),
        ("/inputs/model/sha256", track.model_sha256.as_str(), "model"),
        (
            "/inputs/program/sha256",
            track.program_sha256.as_str(),
            "program",
        ),
        (
            "/inputs/parameters_sha256",
            track.parameters_sha256.as_str(),
            "parameters",
        ),
        (
            "/inputs/token_track_sha256",
            track.token_track_sha256.as_str(),
            "token_track",
        ),
    ] {
        if evidence_string(&evidence, pointer, label)? != expected {
            return Err(anyhow!("stage:token_evidence_{label}_binding"));
        }
    }
    if track.audio_sha256 != audio_sha256
        || track.model_name != args.model_name
        || track.parameters.language != args.language
        || evidence_string(&evidence, "/inputs/model/name", "model_name")? != args.model_name
    {
        return Err(anyhow!("stage:token_evidence_runtime_binding"));
    }
    let token_producer_program_path = canonicalize_for_evidence(evidence_string(
        &evidence,
        "/inputs/program/path",
        "program_path",
    )?)?;
    if sha256_file(&token_producer_program_path).context("stage:token_program_hash")?
        != track.program_sha256
    {
        return Err(anyhow!("stage:token_program_changed"));
    }
    Ok(R5TrackSelection {
        origin: json!({
            "mode": "REUSED_VERIFIED_FORMAL_TOKEN_TRACK",
            "evidence_path": evidence_path.display().to_string(),
            "evidence_sha256": evidence_sha256,
            "evidence_payload_sha256": expected_payload_sha256,
            "token_producer_program_path": token_producer_program_path.display().to_string(),
            "token_producer_program_sha256": track.program_sha256.as_str(),
            "prior_product_alignment_reused": false,
            "audio_inference_repeated": false,
        }),
        track,
        token_producer_program_path,
    })
}

fn alignment_invariants(
    raw_segments: &[CandidateSegmentInput],
    alignment: &R5ProductAlignment,
    track: &crate::moss_audio_token_alignment::AudioTokenTrack,
    suggestions: &[MachineTermSuggestion],
    context_sha256: &str,
) -> (Value, bool) {
    let output = &alignment.product.segments;
    let provenance = &alignment.product.provenance;
    let raw_text = raw_segments
        .iter()
        .map(|segment| segment.text.as_str())
        .collect::<String>();
    let output_text = output
        .iter()
        .map(|segment| segment.text.as_str())
        .collect::<String>();
    let output_indices_exact = output
        .iter()
        .enumerate()
        .all(|(index, segment)| usize::try_from(segment.segment_index).ok() == Some(index));
    let output_starts_monotonic = output
        .windows(2)
        .all(|pair| pair[0].start_ms <= pair[1].start_ms);
    let provenance_count_exact = provenance.len() == output.len();
    let provenance_valid = provenance_count_exact
        && output.iter().zip(provenance).all(|(segment, proof)| {
            let Some(raw) = usize::try_from(proof.raw_segment_index)
                .ok()
                .and_then(|index| raw_segments.get(index))
            else {
                return false;
            };
            proof.segment_index == segment.segment_index
                && proof.raw_start_ms == raw.start_ms
                && proof.raw_end_ms == raw.end_ms
                && proof.raw_text_sha256 == sha256_text(&raw.text)
                && segment.speaker_label == raw.speaker_label
        });

    let mut outer_ranges_preserved = provenance_valid;
    let mut speaker_labels_preserved = provenance_valid;
    let mut per_raw_text_preserved = provenance_valid;
    for raw in raw_segments {
        let indices = provenance
            .iter()
            .enumerate()
            .filter(|(_, proof)| proof.raw_segment_index == raw.segment_index)
            .map(|(index, _)| index)
            .collect::<Vec<_>>();
        if indices.is_empty() {
            outer_ranges_preserved = false;
            speaker_labels_preserved = false;
            per_raw_text_preserved = false;
            continue;
        }
        outer_ranges_preserved &= output[indices[0]].start_ms == raw.start_ms
            && output[*indices.last().unwrap()].end_ms == raw.end_ms
            && indices
                .windows(2)
                .all(|pair| output[pair[0]].end_ms == output[pair[1]].start_ms);
        speaker_labels_preserved &= indices
            .iter()
            .all(|index| output[*index].speaker_label == raw.speaker_label);
        per_raw_text_preserved &= indices
            .iter()
            .map(|index| output[*index].text.as_str())
            .collect::<String>()
            == raw.text;
    }

    let piece_raw_ranges_exact = alignment.piece_raw_ranges.len() == output.len()
        && alignment
            .piece_raw_ranges
            .iter()
            .enumerate()
            .all(|(index, range)| {
                usize::try_from(range.output_segment_index).ok() == Some(index)
                    && provenance
                        .get(index)
                        .is_some_and(|proof| proof.raw_segment_index == range.raw_segment_index)
                    && usize::try_from(range.raw_segment_index)
                        .ok()
                        .and_then(|raw_index| raw_segments.get(raw_index))
                        .is_some_and(|raw| {
                            range.raw_start_char < range.raw_end_char
                                && range.raw_end_char <= raw.text.chars().count()
                                && output[index].text
                                    == raw
                                        .text
                                        .chars()
                                        .skip(range.raw_start_char)
                                        .take(range.raw_end_char - range.raw_start_char)
                                        .collect::<String>()
                        })
            })
        && raw_segments.iter().all(|raw| {
            let ranges = alignment
                .piece_raw_ranges
                .iter()
                .filter(|range| range.raw_segment_index == raw.segment_index)
                .collect::<Vec<_>>();
            let mut cursor = 0usize;
            for range in ranges {
                if range.raw_start_char != cursor {
                    return false;
                }
                cursor = range.raw_end_char;
            }
            cursor == raw.text.chars().count()
        });

    let mut boundary_segments = BTreeSet::new();
    let token_boundaries_valid = alignment.token_boundaries.iter().all(|boundary| {
        let Some(segment) = usize::try_from(boundary.segment_index)
            .ok()
            .and_then(|index| output.get(index))
        else {
            return false;
        };
        let Some(proof) = usize::try_from(boundary.segment_index)
            .ok()
            .and_then(|index| provenance.get(index))
        else {
            return false;
        };
        let Some(first) = usize::try_from(boundary.first_token_index)
            .ok()
            .and_then(|index| track.tokens.get(index))
        else {
            return false;
        };
        let Some(last) = usize::try_from(boundary.last_token_index)
            .ok()
            .and_then(|index| track.tokens.get(index))
        else {
            return false;
        };
        boundary_segments.insert(boundary.segment_index)
            && boundary.first_token_index <= boundary.last_token_index
            && first.global_token_index == boundary.first_token_index
            && last.global_token_index == boundary.last_token_index
            && boundary.token_track_sha256 == track.token_track_sha256
            && proof.alignment_method == ALIGNMENT_METHOD_AUDIO_TOKEN
            && segment.start_ms <= first.start_ms
            && last.start_ms < segment.end_ms
            && boundary.confidence.is_finite()
            && (0.0..=1.0).contains(&boundary.confidence)
    }) && alignment.token_boundaries.len()
        == usize::try_from(alignment.token_aligned_segment_count).unwrap_or(usize::MAX)
        && provenance.iter().all(|proof| {
            (proof.alignment_method == ALIGNMENT_METHOD_AUDIO_TOKEN)
                == boundary_segments.contains(&proof.segment_index)
        });

    let machine_suggestions_valid = suggestions.iter().all(|suggestion| {
        let Some(segment) = usize::try_from(suggestion.segment_index)
            .ok()
            .and_then(|index| output.get(index))
        else {
            return false;
        };
        let original = segment
            .text
            .chars()
            .skip(suggestion.start_char)
            .take(suggestion.end_char.saturating_sub(suggestion.start_char))
            .collect::<String>();
        let boundary = alignment
            .token_boundaries
            .iter()
            .find(|boundary| boundary.segment_index == suggestion.segment_index);
        suggestion.start_char < suggestion.end_char
            && suggestion.end_char <= segment.text.chars().count()
            && original == suggestion.original_text
            && !suggestion.replacement_text.trim().is_empty()
            && suggestion.context_sha256 == context_sha256
            && suggestion.token_track_sha256 == track.token_track_sha256
            && suggestion.model_sha256 == track.model_sha256
            && boundary.is_some_and(|boundary| {
                suggestion.first_token_index >= boundary.first_token_index
                    && suggestion.last_token_index <= boundary.last_token_index
            })
            && suggestion.confidence.is_finite()
            && (0.0..=1.0).contains(&suggestion.confidence)
    }) && suggestions.iter().enumerate().all(|(index, left)| {
        suggestions
            .iter()
            .skip(index.saturating_add(1))
            .all(|right| {
                left.segment_index != right.segment_index
                    || left.end_char <= right.start_char
                    || right.end_char <= left.start_char
            })
    });

    let text_preserved_exactly = raw_text == output_text && per_raw_text_preserved;
    let token_track_bound =
        alignment.token_track_sha256.as_deref() == Some(track.token_track_sha256.as_str());
    let all_pass = output_indices_exact
        && output_starts_monotonic
        && provenance_count_exact
        && provenance_valid
        && outer_ranges_preserved
        && speaker_labels_preserved
        && text_preserved_exactly
        && piece_raw_ranges_exact
        && token_boundaries_valid
        && token_track_bound
        && machine_suggestions_valid;
    (
        json!({
            "output_indices_exact": output_indices_exact,
            "output_starts_monotonic": output_starts_monotonic,
            "provenance_count_exact": provenance_count_exact,
            "provenance_valid": provenance_valid,
            "outer_ranges_preserved": outer_ranges_preserved,
            "speaker_labels_preserved": speaker_labels_preserved,
            "text_preserved_exactly": text_preserved_exactly,
            "raw_concatenated_text_sha256": sha256_text(&raw_text),
            "output_concatenated_text_sha256": sha256_text(&output_text),
            "piece_raw_ranges_exact": piece_raw_ranges_exact,
            "token_boundaries_valid": token_boundaries_valid,
            "token_track_bound": token_track_bound,
            "machine_suggestions_valid": machine_suggestions_valid,
            "human_truth_used": false,
            "all_pass": all_pass,
        }),
        all_pass,
    )
}

async fn produce_r5_product_alignment(args: &CliArgs) -> Result<()> {
    if args.model_name != R5_ALIGNMENT_MODEL_NAME {
        return Err(anyhow!("stage:r5_model_not_frozen"));
    }
    let moss_candidate_path = args
        .moss_candidate
        .as_deref()
        .ok_or_else(|| anyhow!("stage:r5_moss_candidate_missing"))?;
    let source_transcript_path = args
        .source_transcript
        .as_deref()
        .ok_or_else(|| anyhow!("stage:r5_source_transcript_missing"))?;
    let metadata_path = args
        .metadata
        .as_deref()
        .ok_or_else(|| anyhow!("stage:r5_metadata_missing"))?;
    let executable = canonicalize_for_evidence(std::env::current_exe()?)?;
    let model_path = canonicalize_for_evidence(
        args.models_dir
            .join(format!("ggml-{}.bin", args.model_name)),
    )?;
    let command = canonical_command(args, &executable, FORMAL_R5_PRODUCT_ALIGNMENT_FLAG);
    let command_sha256 = sha256_canonical_json(&json!(command))?;
    let started_at = chrono::Utc::now().to_rfc3339();
    let started = Instant::now();

    // Validate every small machine input before starting the expensive full-audio inference.
    let audio_sha256 = sha256_file(&args.audio).context("stage:audio_hash")?;
    let moss_candidate_sha256 = sha256_file(moss_candidate_path).context("stage:raw_moss_hash")?;
    let source_transcript_sha256 =
        sha256_file(source_transcript_path).context("stage:source_transcript_hash")?;
    let metadata_sha256 = sha256_file(metadata_path).context("stage:metadata_hash")?;
    let raw_candidate: R5RawMossCandidate =
        read_json_file(moss_candidate_path).context("stage:raw_moss_parse")?;
    let raw_segments = r5_raw_segments(&raw_candidate, &audio_sha256)?;
    let source_transcript: R5SourceTranscriptFile =
        read_json_file(source_transcript_path).context("stage:source_transcript_parse")?;
    let source_snapshot = r5_source_snapshot(&source_transcript, &source_transcript_sha256)?;
    let metadata: R5MeetingMetadata =
        read_json_file(metadata_path).context("stage:metadata_parse")?;
    let (context_snapshot, context_terms) = r5_context(metadata.meeting_context)?;

    let track_selection = if let Some(evidence_path) = args.token_evidence.as_deref() {
        load_reused_r5_track(
            evidence_path,
            args,
            &audio_sha256,
            &moss_candidate_sha256,
            &source_transcript_sha256,
            &metadata_sha256,
        )?
    } else {
        let track = build_audio_token_track(
            &args.audio,
            &args.models_dir,
            &args.model_name,
            &args.language,
        )
        .await
        .context("stage:token_track")?;
        R5TrackSelection {
            origin: json!({
                "mode": "GENERATED_IN_THIS_RUN",
                "evidence_path": Value::Null,
                "evidence_sha256": Value::Null,
                "evidence_payload_sha256": Value::Null,
                "token_producer_program_path": executable.display().to_string(),
                "token_producer_program_sha256": track.program_sha256.as_str(),
                "prior_product_alignment_reused": false,
                "audio_inference_repeated": true,
            }),
            track,
            token_producer_program_path: executable.clone(),
        }
    };
    let R5TrackSelection {
        track,
        origin: track_origin,
        token_producer_program_path,
    } = track_selection;
    if track.audio_sha256 != audio_sha256
        || raw_segments
            .iter()
            .any(|segment| segment.end_ms > track.audio_duration_ms)
        || source_snapshot
            .anchors
            .iter()
            .any(|anchor| anchor.end_ms > track.audio_duration_ms)
    {
        return Err(anyhow!("stage:r5_audio_scope"));
    }
    let alignment =
        align_candidate_segments_with_audio_tokens(&raw_segments, &source_snapshot, Some(&track))
            .map_err(|error| anyhow!("stage:r5_product_alignment:{error}"))?;
    let suggestions = machine_term_suggestions(
        &raw_segments,
        &alignment,
        &track,
        &context_terms,
        &context_snapshot.context_sha256,
    );
    let (invariants, all_invariants_pass) = alignment_invariants(
        &raw_segments,
        &alignment,
        &track,
        &suggestions,
        &context_snapshot.context_sha256,
    );
    if !all_invariants_pass {
        return Err(anyhow!("stage:r5_alignment_invariants"));
    }

    let piece_raw_ranges = alignment
        .piece_raw_ranges
        .iter()
        .map(|range| {
            json!({
                "output_segment_index": range.output_segment_index,
                "raw_segment_index": range.raw_segment_index,
                "raw_start_char": range.raw_start_char,
                "raw_end_char": range.raw_end_char,
            })
        })
        .collect::<Vec<_>>();
    let alignment_program_sha256 =
        sha256_file(&executable).context("stage:alignment_program_hash")?;
    let mut payload = json!({
        "schema_version": 1,
        "role": "FORMAL_MEETILY_R5_PRODUCT_ALIGNMENT",
        "run_id": args.run_id,
        "started_at": started_at,
        "finished_at": chrono::Utc::now().to_rfc3339(),
        "elapsed_seconds": started.elapsed().as_secs_f64(),
        "command": command,
        "command_sha256": command_sha256,
        "inputs": {
            "audio": {
                "path": args.audio.display().to_string(),
                "sha256": audio_sha256,
                "duration_ms": track.audio_duration_ms,
            },
            "raw_moss_candidate": {
                "path": moss_candidate_path.display().to_string(),
                "sha256": moss_candidate_sha256,
                "source_audio_sha256": raw_candidate.source_binding.audio_sha256.to_ascii_lowercase(),
            },
            "source_transcript": {
                "path": source_transcript_path.display().to_string(),
                "sha256": source_transcript_sha256,
                "anchor_count": source_snapshot.anchors.len(),
                "role": "SECONDARY_MACHINE_ANCHOR_NOT_HUMAN_TRUTH",
            },
            "meeting_metadata": {
                "path": metadata_path.display().to_string(),
                "sha256": metadata_sha256,
            },
            "model": {
                "path": model_path.display().to_string(),
                "name": track.model_name.as_str(),
                "sha256": track.model_sha256.as_str(),
            },
            "program": {
                "path": token_producer_program_path.display().to_string(),
                "sha256": track.program_sha256.as_str(),
            },
            "alignment_program": {
                "path": executable.display().to_string(),
                "sha256": alignment_program_sha256,
            },
            "parameters_sha256": track.parameters_sha256.as_str(),
            "token_track_sha256": track.token_track_sha256.as_str(),
            "token_track_origin": track_origin,
        },
        "source_transcript_snapshot": source_snapshot,
        "pretranscription_context": {
            "context_id": context_snapshot.context_id,
            "revision": context_snapshot.revision,
            "reason": context_snapshot.reason,
            "captured_at": context_snapshot.captured_at,
            "source": context_snapshot.source,
            "context_sha256": context_snapshot.context_sha256,
            "terms": context_snapshot.terms,
            "machine_term_input_only": true,
        },
        "audio_token_track": track,
        "raw_segments": raw_segments,
        "product_alignment": {
            "segments": alignment.product.segments,
            "provenance": alignment.product.provenance,
            "aligned_segment_count": alignment.product.aligned_segment_count,
            "fallback_segment_count": alignment.product.fallback_segment_count,
            "source_anchor_count": alignment.product.source_anchor_count,
            "source_hash_verified": alignment.product.source_hash_verified,
            "source_expected_sha256": alignment.product.source_expected_sha256,
            "source_actual_sha256": alignment.product.source_actual_sha256,
            "fallback_reason": alignment.product.fallback_reason,
            "token_boundaries": alignment.token_boundaries,
            "token_aligned_segment_count": alignment.token_aligned_segment_count,
            "token_fallback_raw_segment_count": alignment.token_fallback_raw_segment_count,
            "token_global_match_coverage": alignment.token_global_match_coverage,
            "token_track_sha256": alignment.token_track_sha256,
            "token_fallback_reason": alignment.token_fallback_reason,
            "piece_raw_ranges": piece_raw_ranges,
        },
        "machine_term_suggestions": suggestions,
        "invariants": invariants,
        "human_truth_used_for_alignment_or_correction": false,
    });
    let payload_sha256 = sha256_json(&payload).context("stage:payload_hash")?;
    payload
        .as_object_mut()
        .ok_or_else(|| anyhow!("stage:payload_shape"))?
        .insert("payload_sha256".to_owned(), json!(payload_sha256));
    atomic_write_new_json(&args.output, &payload, &args.run_id).context("stage:output_write")
}

fn parakeet_model_files(models_dir: &Path, model_name: &str) -> Result<Value> {
    let model_dir = models_dir.join("parakeet").join(model_name);
    let filenames = [
        "encoder-model.int8.onnx",
        "decoder_joint-model.int8.onnx",
        "nemo128.onnx",
        "vocab.txt",
    ];
    let files = filenames
        .iter()
        .map(|filename| {
            let path = model_dir.join(filename);
            Ok(json!({
                "filename": filename,
                "sha256": sha256_file(&path)?,
                "bytes": fs::metadata(&path)?.len(),
            }))
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(json!({
        "model_directory": model_dir.display().to_string(),
        "files": files,
    }))
}

async fn produce_parakeet(args: &CliArgs) -> Result<()> {
    let executable = canonicalize_for_evidence(std::env::current_exe()?)?;
    let command = canonical_command(args, &executable, FORMAL_PARAKEET_FLAG);
    let command_sha256 = sha256_canonical_json(&json!(command))?;
    let started_at = chrono::Utc::now().to_rfc3339();
    let started = Instant::now();
    let decoded = decode_audio_file(&args.audio).context("stage:audio_decode")?;
    let duration_seconds = decoded.duration_seconds;
    let audio_samples = decoded.to_whisper_format();
    let speech_segments =
        get_speech_chunks(&audio_samples, VAD_REDEMPTION_TIME_MS).context("stage:vad")?;
    if speech_segments.is_empty() {
        return Err(anyhow!("stage:no_speech_detected"));
    }
    let mut processable_segments = Vec::new();
    for segment in &speech_segments {
        if segment.samples.len() > MAX_SEGMENT_SAMPLES {
            processable_segments.extend(split_segment_at_silence(segment, MAX_SEGMENT_SAMPLES, 0));
        } else {
            processable_segments.push(segment.clone());
        }
    }
    let engine = ParakeetEngine::new_with_models_root(args.models_dir.clone())
        .context("stage:engine_init")?;
    engine
        .discover_models()
        .await
        .context("stage:model_discovery")?;
    engine
        .load_model(&args.model_name)
        .await
        .context("stage:model_load")?;
    let mut output_segments = Vec::new();
    for segment in &processable_segments {
        if segment.samples.len() < MIN_SEGMENT_SAMPLES {
            continue;
        }
        let text = engine
            .transcribe_audio(segment.samples.clone())
            .await
            .context("stage:transcription")?;
        if !text.trim().is_empty() {
            let (start, end) = bounded_segment_seconds(
                segment.start_timestamp_ms,
                segment.end_timestamp_ms,
                duration_seconds,
            )?;
            output_segments.push(json!({
                "start": start,
                "end": end,
                "speaker": "",
                "text": text,
                "confidence": 0.9,
            }));
        }
    }
    if output_segments.is_empty() {
        return Err(anyhow!("stage:empty_transcript"));
    }
    let payload = json!({
        "schema_version": 1,
        "role": "FORMAL_CURRENT_MEETILY_PARAKEET_RAW_OUTPUT",
        "run_id": args.run_id,
        "started_at": started_at,
        "finished_at": chrono::Utc::now().to_rfc3339(),
        "elapsed_seconds": started.elapsed().as_secs_f64(),
        "command_sha256": command_sha256,
        "input_audio_sha256": sha256_file(&args.audio).context("stage:audio_hash")?,
        "input_audio_duration_seconds": duration_seconds,
        "stable_exe_sha256": sha256_file(&executable).context("stage:executable_hash")?,
        "model_name": args.model_name,
        "model_artifacts": parakeet_model_files(&args.models_dir, &args.model_name)
            .context("stage:model_hash")?,
        "backend": "onnxruntime_cpu",
        "language": "automatic_detection",
        "decode_parameters": {
            "product_path": "manual_retranscription_equivalent",
            "decoder": "Meetily audio::decoder::decode_audio_file + to_whisper_format",
            "sample_rate_hz": 16000,
            "channels": 1,
            "vad": "Meetily ContinuousVadProcessor",
            "vad_redemption_time_ms": VAD_REDEMPTION_TIME_MS,
            "maximum_segment_samples": MAX_SEGMENT_SAMPLES,
            "minimum_segment_samples": MIN_SEGMENT_SAMPLES,
            "splitter": "Meetily silence-aware split_segment_at_silence",
            "engine_method": "ParakeetEngine::transcribe_audio",
            "recognition_prompt_status": "not_supported",
            "recognition_normalization": "not_configured",
            "confidence": "product retranscription uses fixed 0.9 for Parakeet; this is not a model confidence score"
        },
        "speech_segment_count_before_split": speech_segments.len(),
        "speech_segment_count_after_split": processable_segments.len(),
        "segments": output_segments,
    });
    atomic_write_new_json(&args.output, &payload, &args.run_id).context("stage:output_write")
}

fn failure_stage(error: &anyhow::Error) -> String {
    error
        .chain()
        .map(|cause| cause.to_string())
        .find_map(|message| message.strip_prefix("stage:").map(str::to_owned))
        .unwrap_or_else(|| "unclassified".to_owned())
}

fn safe_failure_detail_code(error: &anyhow::Error) -> &'static str {
    for cause in error.chain() {
        let message = cause.to_string();
        for (needle, code) in [
            ("source_chunk_text_sha256", "TOKEN_SOURCE_CHUNK_TEXT_HASH"),
            ("source_chunk_tokens", "TOKEN_SOURCE_CHUNK_MEMBERSHIP"),
            ("source_chunk_count", "TOKEN_SOURCE_CHUNK_COUNT"),
            ("source_chunk", "TOKEN_SOURCE_CHUNK"),
            ("token_track_sha256", "TOKEN_TRACK_HASH"),
            ("parameters_sha256", "TOKEN_PARAMETERS_HASH"),
            ("empty_source_chunks", "TOKEN_SOURCE_CHUNKS_EMPTY"),
            ("empty_tokens", "TOKEN_TRACK_EMPTY"),
            ("invalid: token", "TOKEN_INVALID"),
            (
                "Whisper token alignment returned no spoken tokens",
                "WHISPER_CHUNK_NO_SPOKEN_TOKENS",
            ),
            (
                "Whisper timed token exceeds the supplied audio",
                "WHISPER_TOKEN_EXCEEDS_CHUNK",
            ),
            ("invalid Whisper timed token", "WHISPER_TOKEN_INVALID"),
            (
                "Whisper timed tokens are not monotonic",
                "WHISPER_CHUNK_TOKEN_ORDER",
            ),
            (
                "R5 Whisper token track is not monotonic",
                "TOKEN_TRACK_ORDER",
            ),
            (
                "token is outside its real VAD chunk",
                "TOKEN_OUTSIDE_VAD_CHUNK",
            ),
            ("invalid VAD chunk timestamps", "VAD_CHUNK_TIME_INVALID"),
            ("VAD chunk exceeds the audio", "VAD_CHUNK_EXCEEDS_AUDIO"),
            (
                "R5 Whisper alignment returned no spoken tokens",
                "TOKEN_TRACK_NO_SPOKEN_TOKENS",
            ),
        ] {
            if message.contains(needle) {
                return code;
            }
        }
    }
    "UNCLASSIFIED_SAFE_FAILURE"
}

fn write_failure_record(args: &CliArgs, error: &anyhow::Error) {
    if args.output.exists() {
        return;
    }
    let fingerprint = Sha256::digest(format!("{error:#}").as_bytes());
    let fingerprint: String = fingerprint
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    let role = match requested_flag() {
        FORMAL_R5_PRODUCT_ALIGNMENT_FLAG => "FORMAL_MEETILY_R5_PRODUCT_ALIGNMENT_FAILURE",
        FORMAL_CONTEXT_PROMPT_PROBE_FLAG => "FORMAL_WHISPER_CONTEXT_PROMPT_PROBE_FAILURE",
        FORMAL_TERMS_PROMPT_PROBE_FLAG => "FORMAL_WHISPER_TERMS_PROMPT_PROBE_FAILURE",
        FORMAL_HOTWORD_BIAS_PROBE_FLAG => "FORMAL_WHISPER_HOTWORD_BIAS_PROBE_FAILURE",
        _ => "FORMAL_CURRENT_MEETILY_WHISPER_FAILURE",
    };
    let payload = json!({
        "schema_version": 1,
        "role": role,
        "run_id": args.run_id,
        "failed_at": chrono::Utc::now().to_rfc3339(),
        "failure_stage": failure_stage(error),
        "failure_detail_code": safe_failure_detail_code(error),
        "error_fingerprint_sha256": fingerprint,
        "contains_transcript_text": false,
    });
    let _ = atomic_write_new_json(&args.output, &payload, &args.run_id);
}

pub fn run_cli() -> i32 {
    let args = match parse_cli() {
        Ok(value) => value,
        Err(_) => return 2,
    };
    let worker = match std::thread::Builder::new()
        .name("meetily-formal-whisper-baseline".to_owned())
        .stack_size(FORMAL_THREAD_STACK_BYTES)
        .spawn(move || {
            let runtime = match tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
            {
                Ok(value) => value,
                Err(error) => {
                    write_failure_record(
                        &args,
                        &anyhow!("stage:runtime_create").context(error.to_string()),
                    );
                    return 2;
                }
            };
            let result = match requested_flag() {
                FORMAL_PARAKEET_FLAG => runtime.block_on(produce_parakeet(&args)),
                FORMAL_R5_PRODUCT_ALIGNMENT_FLAG => {
                    runtime.block_on(produce_r5_product_alignment(&args))
                }
                FORMAL_CONTEXT_PROMPT_PROBE_FLAG => {
                    runtime.block_on(produce_context_prompt_probe(&args))
                }
                FORMAL_TERMS_PROMPT_PROBE_FLAG => {
                    runtime.block_on(produce_terms_prompt_probe(&args))
                }
                FORMAL_HOTWORD_BIAS_PROBE_FLAG => {
                    runtime.block_on(produce_hotword_bias_probe(&args))
                }
                FORMAL_WORD_ALIGNMENT_FLAG => runtime.block_on(produce_word_alignment(&args)),
                _ => runtime.block_on(produce(&args)),
            };
            match result {
                Ok(()) => 0,
                Err(error) => {
                    write_failure_record(&args, &error);
                    2
                }
            }
        }) {
        Ok(worker) => worker,
        Err(_) => return 2,
    };
    match worker.join() {
        Ok(exit_code) => exit_code,
        Err(_) => 2,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        bounded_segment_seconds, canonical_command, r5_raw_segments, r5_source_snapshot, CliArgs,
        R5RawMossCandidate, R5RawMossSourceBinding, R5RawMossTurn, R5SourceTranscriptFile,
        R5SourceTranscriptSegment, FORMAL_CONTEXT_PROMPT_PROBE_FLAG,
        FORMAL_HOTWORD_BIAS_PROBE_FLAG, FORMAL_TERMS_PROMPT_PROBE_FLAG,
    };
    use std::path::{Path, PathBuf};

    #[test]
    fn segment_end_is_clamped_to_exact_audio_duration() {
        let (start, end) = bounded_segment_seconds(712_338.0, 720_900.0, 720.896).unwrap();
        assert_eq!(start, 712.338);
        assert_eq!(end, 720.896);
    }

    #[test]
    fn empty_or_non_finite_segment_is_rejected() {
        assert!(bounded_segment_seconds(1000.0, 1000.0, 2.0).is_err());
        assert!(bounded_segment_seconds(f64::NAN, 1000.0, 2.0).is_err());
    }

    #[test]
    fn r5_raw_candidate_requires_the_exact_audio_binding() {
        let audio_sha256 = "a".repeat(64);
        let candidate = R5RawMossCandidate {
            global_turns: vec![R5RawMossTurn {
                global_start_ms: 100,
                global_end_ms: 900,
                speaker_label: "S01".to_owned(),
                text: "测试".to_owned(),
            }],
            source_binding: R5RawMossSourceBinding {
                audio_sha256: audio_sha256.to_ascii_uppercase(),
            },
        };
        let segments = r5_raw_segments(&candidate, &audio_sha256).unwrap();
        assert_eq!(segments.len(), 1);
        assert!(r5_raw_segments(&candidate, &"b".repeat(64)).is_err());
    }

    #[test]
    fn r5_source_snapshot_requires_exact_sequence_and_real_times() {
        let transcript = R5SourceTranscriptFile {
            total_segments: 1,
            segments: vec![R5SourceTranscriptSegment {
                sequence_id: 0,
                audio_start_time: 1.25,
                audio_end_time: 2.75,
                text: "测试".to_owned(),
            }],
        };
        let snapshot = r5_source_snapshot(&transcript, &"c".repeat(64)).unwrap();
        assert_eq!(snapshot.anchors[0].start_ms, 1_250);
        assert_eq!(snapshot.anchors[0].end_ms, 2_750);

        let invalid = R5SourceTranscriptFile {
            total_segments: 1,
            segments: vec![R5SourceTranscriptSegment {
                sequence_id: 1,
                audio_start_time: 1.25,
                audio_end_time: 2.75,
                text: "测试".to_owned(),
            }],
        };
        assert!(r5_source_snapshot(&invalid, &"c".repeat(64)).is_err());
    }

    #[test]
    fn context_probe_command_binds_metadata_without_prompt_plaintext_argument() {
        let args = CliArgs {
            run_id: "a".repeat(32),
            audio: PathBuf::from(r"D:\evidence\window.wav"),
            models_dir: PathBuf::from(r"D:\models"),
            model_name: "large-v3-turbo-q5_0".to_owned(),
            language: "zh".to_owned(),
            output: PathBuf::from(r"D:\evidence\probe.json"),
            moss_candidate: None,
            source_transcript: None,
            metadata: Some(PathBuf::from(r"D:\recording\metadata.json")),
            token_evidence: None,
            baseline_evidence: None,
        };
        let command = canonical_command(
            &args,
            Path::new(r"D:\build\meetily.exe"),
            FORMAL_CONTEXT_PROMPT_PROBE_FLAG,
        );
        assert!(command
            .windows(2)
            .any(|pair| pair == ["--metadata", r"D:\recording\metadata.json"]));
        assert!(!command.iter().any(|item| {
            item == "--initial-prompt"
                || item == "--initial-prompt-file"
                || item.contains("专有名词")
        }));
    }

    #[test]
    fn terms_probe_command_reuses_baseline_without_prompt_plaintext_argument() {
        let args = CliArgs {
            run_id: "b".repeat(32),
            audio: PathBuf::from(r"D:\evidence\window.wav"),
            models_dir: PathBuf::from(r"D:\models"),
            model_name: "large-v3-turbo-q5_0".to_owned(),
            language: "zh".to_owned(),
            output: PathBuf::from(r"D:\evidence\terms-probe.json"),
            moss_candidate: None,
            source_transcript: None,
            metadata: Some(PathBuf::from(r"D:\recording\metadata.json")),
            token_evidence: None,
            baseline_evidence: Some(PathBuf::from(r"D:\evidence\r8-baseline.json")),
        };
        let command = canonical_command(
            &args,
            Path::new(r"D:\build\meetily.exe"),
            FORMAL_TERMS_PROMPT_PROBE_FLAG,
        );
        assert!(command
            .windows(2)
            .any(|pair| pair == ["--metadata", r"D:\recording\metadata.json"]));
        assert!(command
            .windows(2)
            .any(|pair| { pair == ["--baseline-evidence", r"D:\evidence\r8-baseline.json"] }));
        assert!(!command.iter().any(|item| {
            item == "--initial-prompt"
                || item == "--initial-prompt-file"
                || item.contains("专有名词")
        }));
    }

    #[test]
    fn hotword_probe_command_reuses_baseline_without_term_plaintext_argument() {
        let args = CliArgs {
            run_id: "c".repeat(32),
            audio: PathBuf::from(r"D:\evidence\window.wav"),
            models_dir: PathBuf::from(r"D:\models"),
            model_name: "large-v3-turbo-q5_0".to_owned(),
            language: "zh".to_owned(),
            output: PathBuf::from(r"D:\evidence\hotword-probe.json"),
            moss_candidate: None,
            source_transcript: None,
            metadata: Some(PathBuf::from(r"D:\recording\metadata.json")),
            token_evidence: None,
            baseline_evidence: Some(PathBuf::from(r"D:\evidence\r8-baseline.json")),
        };
        let command = canonical_command(
            &args,
            Path::new(r"D:\build\meetily.exe"),
            FORMAL_HOTWORD_BIAS_PROBE_FLAG,
        );
        assert!(command
            .windows(2)
            .any(|pair| pair == ["--metadata", r"D:\recording\metadata.json"]));
        assert!(command
            .windows(2)
            .any(|pair| { pair == ["--baseline-evidence", r"D:\evidence\r8-baseline.json"] }));
        assert!(!command.iter().any(|item| {
            item == "--initial-prompt"
                || item == "--initial-prompt-file"
                || item == "--hotword"
                || item == "--hotword-file"
                || item.contains("专有名词")
        }));
    }
}

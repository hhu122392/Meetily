use serde::Serialize;
use serde_json::Value;
use std::env;
use std::fs;
use std::path::Path;
use std::time::Instant;
use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

const TARGET_RATE: usize = 16_000;

fn previous_result_prompt(result: &Value) -> Result<String, String> {
    let text = result["hypothesis"]
        .as_str()
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .ok_or_else(|| "Previous result must contain non-empty recognized text".to_owned())?;
    Ok(text
        .chars()
        .skip(text.chars().count().saturating_sub(256))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn previous_prompt_uses_recognized_text_not_reference_labels() {
        let result =
            serde_json::json!({"hypothesis": " 前一句的实际错稿。 ", "reference": "不能喂答案"});
        assert_eq!(
            previous_result_prompt(&result).unwrap(),
            "前一句的实际错稿。"
        );
        assert!(previous_result_prompt(&serde_json::json!({"hypothesis": " "})).is_err());
        assert!(previous_result_prompt(&serde_json::json!({"reference": "只有标准答案"})).is_err());
    }

    #[test]
    fn previous_prompt_keeps_only_a_bounded_unicode_tail() {
        let result = serde_json::json!({"hypothesis": format!("{}末句", "前".repeat(300))});
        let prompt = previous_result_prompt(&result).unwrap();
        assert_eq!(prompt.chars().count(), 256);
        assert!(prompt.ends_with("末句"));
    }

    #[test]
    fn probe_context_matches_current_short_live_context() {
        assert_eq!(streaming_audio_context(5 * TARGET_RATE), 320);
        assert_eq!(streaming_audio_context(20 * TARGET_RATE), 1152);
    }

    #[test]
    fn probe_reads_exact_ipc_samples_without_pcm16_quantization() {
        let directory = tempfile::tempdir().unwrap();
        let input = directory.path().join("input.json");
        fs::write(
            &input,
            r#"{"payload":{"audioData":[0.000013,0.125,-0.375]}}"#,
        )
        .unwrap();
        let (samples, rate) = read_probe_audio(&input).unwrap();
        assert_eq!(samples, vec![0.000013_f32, 0.125, -0.375]);
        assert_eq!(rate, TARGET_RATE);
    }
}

// ggml's Windows CPU feature probe reads the registry. The full Tauri binary
// receives Advapi32 transitively, while this focused QA example must request
// it explicitly.
#[cfg(target_os = "windows")]
#[link(name = "advapi32")]
extern "system" {}

#[derive(Serialize)]
struct SegmentResult {
    sequence_id: u64,
    start_seconds: f64,
    end_seconds: f64,
    elapsed_ms: u128,
    text: String,
}

#[derive(Serialize)]
struct ProbeResult {
    config: String,
    model: String,
    wav: String,
    monitor: String,
    playback_offset_seconds: f64,
    total_elapsed_ms: u128,
    hypothesis: String,
    previous_result: Option<String>,
    initial_prompt: Option<String>,
    segments: Vec<SegmentResult>,
}

fn read_probe_audio(path: &Path) -> Result<(Vec<f32>, usize), String> {
    let bytes = fs::read(path).map_err(|error| error.to_string())?;
    if path
        .extension()
        .is_some_and(|extension| extension == "json")
    {
        let input: Value = serde_json::from_slice(&bytes).map_err(|error| error.to_string())?;
        let samples: Vec<f32> = serde_json::from_value(input["payload"]["audioData"].clone())
            .map_err(|error| error.to_string())?;
        if samples.is_empty() || samples.iter().any(|sample| !sample.is_finite()) {
            return Err("Expected non-empty finite IPC samples".to_owned());
        }
        return Ok((samples, TARGET_RATE));
    }
    if bytes.len() < 44 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err("Expected a RIFF/WAVE file".to_owned());
    }
    let mut cursor = 12usize;
    let mut format = None;
    let mut data = None;
    while cursor + 8 <= bytes.len() {
        let id = &bytes[cursor..cursor + 4];
        let length = u32::from_le_bytes(bytes[cursor + 4..cursor + 8].try_into().unwrap()) as usize;
        let start = cursor + 8;
        let end = start.saturating_add(length).min(bytes.len());
        if id == b"fmt " && length >= 16 {
            let audio_format = u16::from_le_bytes(bytes[start..start + 2].try_into().unwrap());
            let channels = u16::from_le_bytes(bytes[start + 2..start + 4].try_into().unwrap());
            let sample_rate =
                u32::from_le_bytes(bytes[start + 4..start + 8].try_into().unwrap()) as usize;
            let bits = u16::from_le_bytes(bytes[start + 14..start + 16].try_into().unwrap());
            format = Some((audio_format, channels, sample_rate, bits));
        } else if id == b"data" {
            data = Some((start, end));
        }
        cursor = start + length + (length % 2);
    }
    let (audio_format, channels, sample_rate, bits) =
        format.ok_or_else(|| "Missing WAV fmt chunk".to_owned())?;
    if audio_format != 1 || channels != 1 || bits != 16 {
        return Err(format!(
            "Probe only supports mono PCM16 WAV; got format={audio_format}, channels={channels}, bits={bits}"
        ));
    }
    let (start, end) = data.ok_or_else(|| "Missing WAV data chunk".to_owned())?;
    let samples = bytes[start..end]
        .chunks_exact(2)
        .map(|pair| i16::from_le_bytes([pair[0], pair[1]]) as f32 / i16::MAX as f32)
        .collect();
    Ok((samples, sample_rate))
}

fn linear_resample(samples: &[f32], source_rate: usize, target_rate: usize) -> Vec<f32> {
    if source_rate == target_rate {
        return samples.to_vec();
    }
    let output_len = samples.len() * target_rate / source_rate;
    (0..output_len)
        .map(|index| {
            let source = index as f64 * source_rate as f64 / target_rate as f64;
            let left = source.floor() as usize;
            let right = (left + 1).min(samples.len().saturating_sub(1));
            let fraction = (source - left as f64) as f32;
            samples[left] * (1.0 - fraction) + samples[right] * fraction
        })
        .collect()
}

fn streaming_audio_context(sample_count: usize) -> i32 {
    let duration_seconds = sample_count as f64 / TARGET_RATE as f64;
    let padding = if duration_seconds <= 10.0 { 0.5 } else { 2.0 };
    let requested = ((duration_seconds + padding) * 50.0).ceil() as usize;
    let aligned = requested.div_ceil(64) * 64;
    aligned.clamp(192, 1500) as i32
}

fn streaming_max_tokens(sample_count: usize) -> i32 {
    let duration_seconds = sample_count as f64 / TARGET_RATE as f64;
    ((duration_seconds * 20.0).ceil() as usize + 32).clamp(64, 256) as i32
}

fn main() -> Result<(), String> {
    let args: Vec<String> = env::args().collect();
    if !(7..=8).contains(&args.len()) {
        return Err("Usage: live_decoder_probe <model.bin> <golden.wav|ipc-input.json> <monitor-or-offline-ranges.json> <playback-offset-seconds> <dynamic|fullctx|timestamps|fallback|prompt|greedy5|beam2|beam2-live|upstream> <output.json> [previous-result.json (timestamps only)]".to_owned());
    }
    let model_path = &args[1];
    let wav_path = &args[2];
    let monitor_path = &args[3];
    let playback_offset_seconds: f64 = args[4]
        .parse()
        .map_err(|_| "Invalid playback offset".to_owned())?;
    let config = args[5].as_str();
    let output_path = &args[6];
    let previous_result = args.get(7).cloned();
    let initial_prompt = previous_result
        .as_ref()
        .map(|path| {
            if config != "timestamps" {
                return Err(
                    "Previous-result comparison requires the current timestamps baseline"
                        .to_owned(),
                );
            }
            let result =
                serde_json::from_str(&fs::read_to_string(path).map_err(|error| error.to_string())?)
                    .map_err(|error| error.to_string())?;
            previous_result_prompt(&result)
        })
        .transpose()?;

    let (wav, source_rate) = read_probe_audio(Path::new(wav_path))?;
    let wav = linear_resample(&wav, source_rate, TARGET_RATE);
    let monitor: Value =
        serde_json::from_str(&fs::read_to_string(monitor_path).map_err(|error| error.to_string())?)
            .map_err(|error| error.to_string())?;
    let changes = monitor["changes"]
        .as_array()
        .ok_or_else(|| "Monitor has no changes array".to_owned())?;
    let final_change = changes
        .iter()
        .rev()
        .find(|change| change["segmentCount"].as_u64().unwrap_or(0) > 0)
        .ok_or_else(|| "Monitor has no non-empty live snapshot".to_owned())?;
    let source_segments = final_change["segments"]
        .as_array()
        .ok_or_else(|| "Final live snapshot has no segments".to_owned())?;
    if initial_prompt.is_some() && source_segments.len() != 1 {
        return Err("Previous-result comparison expects one target segment".to_owned());
    }

    let context = WhisperContext::new_with_params(
        model_path,
        WhisperContextParameters {
            use_gpu: true,
            gpu_device: 0,
            flash_attn: false,
            ..Default::default()
        },
    )
    .map_err(|error| error.to_string())?;

    let run_started = Instant::now();
    let mut results = Vec::new();
    for segment in source_segments {
        let sequence_id = segment["sequence_id"].as_u64().unwrap_or(0);
        let absolute_start = segment["audio_start_time"]
            .as_f64()
            .ok_or_else(|| "Segment start is missing".to_owned())?;
        let absolute_end = segment["audio_end_time"]
            .as_f64()
            .ok_or_else(|| "Segment end is missing".to_owned())?;
        let relative_start = absolute_start - playback_offset_seconds;
        let relative_end = absolute_end - playback_offset_seconds;
        let requested_samples = ((relative_end - relative_start) * TARGET_RATE as f64)
            .round()
            .max(1.0) as usize;
        let mut chunk = vec![0.0f32; requested_samples];
        let source_start = (relative_start.max(0.0) * TARGET_RATE as f64).round() as usize;
        let destination_start = ((-relative_start).max(0.0) * TARGET_RATE as f64).round() as usize;
        if source_start < wav.len() && destination_start < chunk.len() {
            let count = (wav.len() - source_start).min(chunk.len() - destination_start);
            chunk[destination_start..destination_start + count]
                .copy_from_slice(&wav[source_start..source_start + count]);
        }

        let sampling = match config {
            "beam2" | "beam2-live" | "upstream" => SamplingStrategy::BeamSearch {
                beam_size: 2,
                patience: 1.0,
            },
            "greedy5" => SamplingStrategy::Greedy { best_of: 5 },
            "dynamic" | "fullctx" | "timestamps" | "fallback" | "prompt" => {
                SamplingStrategy::Greedy { best_of: 1 }
            }
            _ => return Err(format!("Unknown probe config: {config}")),
        };
        let mut params = FullParams::new(sampling);
        params.set_language(Some("zh"));
        params.set_translate(false);
        params.set_no_timestamps(!matches!(config, "timestamps" | "beam2-live"));
        params.set_token_timestamps(config == "upstream");
        params.set_print_special(false);
        params.set_print_progress(false);
        params.set_print_realtime(false);
        params.set_print_timestamps(false);
        params.set_suppress_blank(true);
        params.set_suppress_non_speech_tokens(true);
        params.set_entropy_thold(2.4);
        params.set_logprob_thold(-1.0);
        params.set_no_speech_thold(0.55);
        params.set_max_len(200);
        // Official v0.4.0 Windows decoder baseline, not a product recommendation:
        // https://github.com/Zackriya-Solutions/meetily/blob/0281737d87d26352fb0adc78c8c0975f691b23d1/frontend/src-tauri/src/whisper_engine/whisper_engine.rs#L516-L584
        // No explicit audio/token cap or thread override upstream. Native defaults
        // in the linked whisper.cpp are no_context=true and max_initial_ts=1.0.
        if config != "upstream" {
            params.set_max_tokens(streaming_max_tokens(chunk.len()));
            params.set_n_threads(8);
        }
        params.set_no_context(true);
        params.set_single_segment(false);
        params.set_temperature(if config == "upstream" { 0.2 } else { 0.0 });
        params.set_temperature_inc(if matches!(config, "fallback" | "upstream") {
            0.2
        } else {
            0.0
        });
        if !matches!(config, "fullctx" | "upstream") {
            params.set_audio_ctx(streaming_audio_context(chunk.len()));
        }
        if config == "prompt" {
            params.set_initial_prompt("以下是简体中文会议的实时转写。");
        }
        if let Some(prompt) = initial_prompt.as_deref() {
            params.set_initial_prompt(prompt);
        }

        let started = Instant::now();
        let mut state = context.create_state().map_err(|error| error.to_string())?;
        state
            .full(params, &chunk)
            .map_err(|error| error.to_string())?;
        let segment_count = state.full_n_segments().map_err(|error| error.to_string())?;
        let mut text = String::new();
        for index in 0..segment_count {
            let part = state
                .full_get_segment_text_lossy(index)
                .map_err(|error| error.to_string())?;
            text.push_str(part.trim());
        }
        results.push(SegmentResult {
            sequence_id,
            start_seconds: absolute_start,
            end_seconds: absolute_end,
            elapsed_ms: started.elapsed().as_millis(),
            text,
        });
    }
    let hypothesis = results
        .iter()
        .map(|segment| segment.text.as_str())
        .collect();
    let output = ProbeResult {
        config: config.to_owned(),
        model: model_path.to_owned(),
        wav: wav_path.to_owned(),
        monitor: monitor_path.to_owned(),
        playback_offset_seconds,
        total_elapsed_ms: run_started.elapsed().as_millis(),
        hypothesis,
        previous_result,
        initial_prompt,
        segments: results,
    };
    if let Some(parent) = Path::new(output_path).parent() {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    fs::write(
        output_path,
        serde_json::to_string_pretty(&output).map_err(|error| error.to_string())?,
    )
    .map_err(|error| error.to_string())?;
    println!(
        "{}",
        serde_json::to_string_pretty(&output).map_err(|error| error.to_string())?
    );
    Ok(())
}

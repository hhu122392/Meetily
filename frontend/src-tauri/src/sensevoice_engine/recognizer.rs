use super::boundary::{token_text, valid_times, TimedToken};
use anyhow::{anyhow, Result};
use sherpa_onnx::{
    OfflineModelConfig, OfflineRecognizer, OfflineRecognizerConfig, OfflineSenseVoiceModelConfig,
};
use std::path::Path;

pub struct TimedRecognition {
    pub text: String,
    pub tokens: Option<Vec<TimedToken>>,
}

pub fn create_recognizer(model_dir: &Path) -> Result<OfflineRecognizer> {
    let model_path = model_dir.join("model.int8.onnx");
    let tokens_path = model_dir.join("tokens.txt");
    if !model_path.is_file() || !tokens_path.is_file() {
        return Err(anyhow!(
            "SenseVoice model files are missing in {}",
            model_dir.display()
        ));
    }
    let mut config = OfflineRecognizerConfig::default();
    config.model_config = OfflineModelConfig {
        sense_voice: OfflineSenseVoiceModelConfig {
            model: Some(model_path.to_string_lossy().to_string()),
            language: Some("zh".to_string()),
            use_itn: true,
        },
        tokens: Some(tokens_path.to_string_lossy().to_string()),
        num_threads: std::thread::available_parallelism()
            .map(|v| v.get())
            .unwrap_or(4)
            .min(8) as i32,
        debug: false,
        ..Default::default()
    };
    OfflineRecognizer::create(&config)
        .ok_or_else(|| anyhow!("sherpa-onnx refused to create a SenseVoice recognizer"))
}

/// A bounded decode with the existing model. Keep text even when timing is
/// absent or a runtime text normalizer no longer matches the emitted tokens.
pub fn decode_timed(
    recognizer: &OfflineRecognizer,
    audio: &[f32],
    offset: f64,
) -> Result<TimedRecognition> {
    if audio.len() > 30 * 16_000 {
        return Err(anyhow!("SenseVoice timed input exceeds 30 seconds"));
    }
    if !offset.is_finite() || offset < 0.0 {
        return Err(anyhow!("Invalid SenseVoice audio offset"));
    }
    if audio.is_empty() {
        return Ok(TimedRecognition {
            text: String::new(),
            tokens: Some(Vec::new()),
        });
    }
    let stream = recognizer.create_stream();
    stream.accept_waveform(16_000, audio);
    recognizer.decode(&stream);
    let result = stream
        .get_result()
        .ok_or_else(|| anyhow!("SenseVoice returned no recognition result"))?;
    let duration = audio.len() as f64 / 16_000.0;
    let tokens = result
        .timestamps
        .filter(|times| times.len() == result.tokens.len())
        .and_then(|times| {
            if times
                .iter()
                .any(|t| !t.is_finite() || *t < 0.0 || *t as f64 > duration)
            {
                return None;
            }
            let tokens: Vec<_> = result
                .tokens
                .into_iter()
                .zip(times)
                .map(|(text, t)| TimedToken {
                    text,
                    time: offset + t as f64,
                })
                .collect();
            (valid_times(&tokens) && token_text(&tokens) == result.text.trim()).then_some(tokens)
        });
    Ok(TimedRecognition {
        text: result.text.trim().to_owned(),
        tokens,
    })
}

// Commit name to recover the serial whisper engine processing for smaller meetings [Slower processing but dooes not fail] - "before parallel processing implementation"

use super::acceleration::{whisper_context_acceleration_for, WhisperCompiledBackend};
use super::hotword_bias::{
    WhisperHotwordBiasConfig, WhisperHotwordBiasDiagnostics, WhisperHotwordBiasState,
};
use crate::config::WHISPER_MODEL_CATALOG;
use anyhow::{anyhow, Result};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::ffi::CStr;
use std::os::raw::{c_char, c_void};
use std::path::PathBuf;
use std::sync::{Arc, Once};
use tokio::fs;
use tokio::io::AsyncWriteExt;
use tokio::sync::RwLock;
use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

const WHISPER_SAMPLE_RATE: usize = 16_000;
pub const WHISPER_TIMESTAMP_TICK_MS: i64 = 10;
const WHISPER_AUDIO_CONTEXTS_PER_SECOND: f64 = 50.0;
const WHISPER_AUDIO_CONTEXT_ALIGNMENT: usize = 64;
const WHISPER_MIN_STREAMING_AUDIO_CONTEXT: usize = 192;
const WHISPER_MAX_AUDIO_CONTEXT: usize = 1500;
// Half a second of requested padding still leaves roughly 0.9-1.4 seconds of
// effective headroom after 64-frame alignment for the short fragments VAD
// closes on a pause. It also avoids an otherwise unnecessary encoder block
// that made a real 5.42-second fragment take 8.57 seconds on CPU.
const WHISPER_STREAMING_CONTEXT_PADDING_SECONDS: f64 = 0.5;
const WHISPER_LONG_CONTEXT_PADDING_SECONDS: f64 = 2.0;
const WHISPER_SHORT_CONTEXT_MAX_SECONDS: f64 = 10.0;
// Live segments at or above this length switch to the quality decode plan
// (beam search + the model's native encoder context). Measured on six 20
// second windows of a real recording, that plan removed 3 wrong "VS"
// substitutions and produced 10 correct "V4" terms instead of 7, with no
// window worse, at RTF 0.59 -> 0.91. The same plan on a 5 second fragment
// produced identical text for RTF 3.58, so short fragments must stay greedy
// with the bounded context. Reverting either half alone was also measured and
// failed: bounded+beam5 kept all three "VS" errors, native+greedy fixed one
// window and broke another.
const WHISPER_LIVE_QUALITY_MIN_SECONDS: f64 = 12.0;
const WHISPER_LIVE_QUALITY_BEAM_SIZE: i32 = 5;
static WHISPER_LOG_CALLBACK_INSTALL: Once = Once::new();

/// whisper.cpp's default callback prints initial-prompt tokens directly to
/// stderr, bypassing both Rust logging and `FullParams` print flags. Install a
/// process-wide callback before creating any context so meeting-scoped names
/// can never escape through native debug output.
unsafe extern "C" fn privacy_safe_whisper_log(
    level: whisper_rs::whisper_rs_sys::ggml_log_level,
    text: *const c_char,
    _user_data: *mut c_void,
) {
    if text.is_null() {
        return;
    }
    if level != whisper_rs::whisper_rs_sys::ggml_log_level_GGML_LOG_LEVEL_WARN
        && level != whisper_rs::whisper_rs_sys::ggml_log_level_GGML_LOG_LEVEL_ERROR
    {
        return;
    }
    let message = unsafe { CStr::from_ptr(text) }.to_string_lossy();
    let trimmed = message.trim();
    if trimmed.contains("prompt[") || trimmed.contains("[_PREV_]") {
        log::warn!("whisper.cpp prompt diagnostic redacted");
    } else if level == whisper_rs::whisper_rs_sys::ggml_log_level_GGML_LOG_LEVEL_ERROR {
        log::error!("whisper.cpp: {}", trimmed);
    } else {
        log::warn!("whisper.cpp: {}", trimmed);
    }
}

fn install_privacy_safe_whisper_logging() {
    WHISPER_LOG_CALLBACK_INSTALL.call_once(|| unsafe {
        whisper_rs::whisper_rs_sys::whisper_log_set(
            Some(privacy_safe_whisper_log),
            std::ptr::null_mut(),
        );
    });
}

/// whisper.cpp otherwise pads every short VAD fragment to the model's full
/// 30-second (1500 frame) encoder context. On CPU that makes a 3-second phrase
/// cost almost as much as 30 seconds and prevents real-time convergence.
///
/// Keep bounded padding, align the context for the encoder graph, and cover the
/// complete input without exceeding the model maximum.
fn streaming_audio_context(sample_count: usize) -> i32 {
    let duration_seconds = sample_count as f64 / WHISPER_SAMPLE_RATE as f64;
    let padding_seconds = if duration_seconds <= WHISPER_SHORT_CONTEXT_MAX_SECONDS {
        WHISPER_STREAMING_CONTEXT_PADDING_SECONDS
    } else {
        WHISPER_LONG_CONTEXT_PADDING_SECONDS
    };
    let required_contexts =
        ((duration_seconds + padding_seconds) * WHISPER_AUDIO_CONTEXTS_PER_SECOND).ceil() as usize;
    let aligned_contexts = required_contexts.div_ceil(WHISPER_AUDIO_CONTEXT_ALIGNMENT)
        * WHISPER_AUDIO_CONTEXT_ALIGNMENT;

    aligned_contexts.clamp(
        WHISPER_MIN_STREAMING_AUDIO_CONTEXT,
        WHISPER_MAX_AUDIO_CONTEXT,
    ) as i32
}

// Decoder token ceiling for live and batch requests is left at whisper.cpp's
// default (0 = no explicit ceiling). The explicit cap this crate used to set
// (64-256 tokens) was measured on three 20 second windows: identical output
// and identical runtime in all three, so it bought nothing, and it could cut
// a legitimate long segment short. Repetition is handled by whisper.cpp's own
// entropy check plus the temperature fallback, not by the cap.

/// Decode plan for one live request: `true` selects the quality mode (beam
/// search plus the native encoder context) for segments long enough to carry
/// real content, `false` keeps the cheap greedy plan for short fragments.
fn live_decode_plan(sample_count: usize) -> (bool, i32) {
    let duration_seconds = sample_count as f64 / WHISPER_SAMPLE_RATE as f64;
    if duration_seconds >= WHISPER_LIVE_QUALITY_MIN_SECONDS {
        (true, WHISPER_MAX_AUDIO_CONTEXT as i32)
    } else {
        (false, streaming_audio_context(sample_count))
    }
}

#[derive(Debug, PartialEq, Eq)]
struct WhisperLanguageMode {
    language_code: Option<String>,
    translate_to_english: bool,
}

fn resolve_whisper_language_mode(language: Option<&str>) -> Result<WhisperLanguageMode> {
    let Some(raw_language) = language else {
        return Ok(WhisperLanguageMode {
            language_code: None,
            translate_to_english: false,
        });
    };

    let normalized = raw_language.trim().to_ascii_lowercase().replace('_', "-");

    match normalized.as_str() {
        "" | "auto" => Ok(WhisperLanguageMode {
            language_code: None,
            translate_to_english: false,
        }),
        "auto-translate" => Ok(WhisperLanguageMode {
            language_code: None,
            translate_to_english: true,
        }),
        "zh-cn" | "zh-sg" | "zh-hans" | "cmn" | "cmn-hans" => Ok(WhisperLanguageMode {
            language_code: Some("zh".to_string()),
            translate_to_english: false,
        }),
        value => {
            let base = value.split('-').next().unwrap_or_default();
            if !(2..=3).contains(&base.len()) || !base.bytes().all(|byte| byte.is_ascii_lowercase())
            {
                return Err(anyhow!(
                    "Unsupported Whisper transcription language: {}",
                    raw_language
                ));
            }

            Ok(WhisperLanguageMode {
                language_code: Some(base.to_string()),
                translate_to_english: false,
            })
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ModelStatus {
    Available,
    Missing,
    Downloading {
        progress: u8,
    },
    Error(String),
    Corrupted {
        file_size: u64,
        expected_min_size: u64,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelInfo {
    pub name: String,
    pub path: PathBuf,
    pub size_mb: u32,
    pub accuracy: String,
    pub speed: String,
    pub status: ModelStatus,
    pub description: String,
}

/// A text token whose time range comes directly from whisper.cpp.
///
/// These values are an independent machine alignment source. They are not
/// MOSS-native timestamps and must never be presented as human truth.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WhisperTimedToken {
    pub segment_index: u32,
    pub token_index: u32,
    pub text: String,
    pub start_ms: i64,
    pub end_ms: i64,
    pub probability: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WhisperTimedTranscript {
    pub text: String,
    pub tokens: Vec<WhisperTimedToken>,
}

fn validated_timed_token(
    segment_index: i32,
    token_index: i32,
    text: String,
    token_id: i32,
    end_of_text_token_id: i32,
    start_tick: i64,
    end_tick: i64,
    probability: f32,
    audio_duration_ms: i64,
) -> Result<Option<WhisperTimedToken>> {
    // whisper.cpp reserves all ids from EOT onwards for control, language and
    // timestamp tokens. They do not represent spoken text and are never valid
    // word-alignment anchors.
    if token_id >= end_of_text_token_id || text.trim().is_empty() {
        return Ok(None);
    }
    if segment_index < 0
        || token_index < 0
        || start_tick < 0
        || end_tick < start_tick
        || audio_duration_ms < 0
        || !probability.is_finite()
        || !(0.0..=1.0).contains(&probability)
    {
        return Err(anyhow!("invalid Whisper timed token"));
    }
    let start_ms = start_tick
        .checked_mul(WHISPER_TIMESTAMP_TICK_MS)
        .ok_or_else(|| anyhow!("Whisper token start overflow"))?;
    let end_ms = end_tick
        .checked_mul(WHISPER_TIMESTAMP_TICK_MS)
        .ok_or_else(|| anyhow!("Whisper token end overflow"))?;
    if end_ms > audio_duration_ms {
        return Err(anyhow!("Whisper timed token exceeds the supplied audio"));
    }
    Ok(Some(WhisperTimedToken {
        segment_index: u32::try_from(segment_index)?,
        token_index: u32::try_from(token_index)?,
        text,
        start_ms,
        end_ms,
        probability,
    }))
}

fn validated_timed_transcript(
    text: String,
    tokens: Vec<WhisperTimedToken>,
) -> Result<WhisperTimedTranscript> {
    if text.trim().is_empty() || tokens.is_empty() {
        return Err(anyhow!("Whisper token alignment returned no spoken tokens"));
    }
    if tokens
        .windows(2)
        .any(|pair| pair[1].start_ms < pair[0].start_ms)
    {
        return Err(anyhow!("Whisper timed tokens are not monotonic"));
    }
    Ok(WhisperTimedTranscript { text, tokens })
}

pub struct WhisperEngine {
    models_dir: PathBuf,
    current_context: Arc<RwLock<Option<WhisperContext>>>,
    current_model: Arc<RwLock<Option<String>>>,
    available_models: Arc<RwLock<HashMap<String, ModelInfo>>>,
    // State tracking for smart logging
    last_transcription_was_short: Arc<RwLock<bool>>,
    short_audio_warning_logged: Arc<RwLock<bool>>,
    // Performance optimization: reduce logging frequency
    transcription_count: Arc<RwLock<u64>>,
    // Download cancellation tracking
    cancel_download_flag: Arc<RwLock<Option<String>>>, // Model name being cancelled
    // Active downloads tracking to prevent concurrent downloads
    active_downloads: Arc<RwLock<HashSet<String>>>, // Set of models currently being downloaded
}

impl WhisperEngine {
    /// Detect available GPU acceleration capabilities
    fn detect_gpu_acceleration() -> bool {
        match WhisperCompiledBackend::current() {
            WhisperCompiledBackend::Metal => {
                log::info!("macOS detected - attempting to enable Metal GPU acceleration");
                true
            }
            WhisperCompiledBackend::Cuda => {
                log::info!("CUDA feature enabled - attempting GPU acceleration");
                true
            }
            WhisperCompiledBackend::Vulkan => {
                log::info!("Vulkan feature enabled - attempting GPU acceleration");
                true
            }
            WhisperCompiledBackend::HipBlas => {
                log::info!("HIP BLAS feature enabled - attempting GPU acceleration");
                true
            }
            WhisperCompiledBackend::Cpu => {
                log::info!("No GPU acceleration features detected - using CPU processing");
                false
            }
        }
    }

    /// Create a WhisperEngine with the exact directory supplied by StorageLayout.
    /// Callers must never guess or rebuild the application models path.
    pub fn new_with_models_dir(models_dir: PathBuf) -> Result<Self> {
        install_privacy_safe_whisper_logging();
        // PERFORMANCE: Suppress verbose whisper.cpp and Metal logs
        // These C library logs bypass Rust logging and clutter output
        // Set environment variables to reduce C library verbosity
        std::env::set_var("GGML_METAL_LOG_LEVEL", "1"); // 0=off, 1=error, 2=warn, 3=info
        std::env::set_var("WHISPER_LOG_LEVEL", "1"); // Reduce whisper.cpp verbosity

        log::info!(
            "WhisperEngine using models directory: {}",
            models_dir.display()
        );
        log::info!("Debug mode: {}", cfg!(debug_assertions));

        // Log acceleration capabilities
        let gpu_support = Self::detect_gpu_acceleration();
        log::info!(
            "Hardware acceleration support: {}",
            if gpu_support { "enabled" } else { "disabled" }
        );

        #[cfg(feature = "metal")]
        log::info!("Apple Metal GPU support: enabled");

        #[cfg(feature = "openblas")]
        log::info!("OpenBLAS CPU optimization: enabled");

        #[cfg(feature = "coreml")]
        log::info!("Apple CoreML support: enabled");

        #[cfg(feature = "cuda")]
        log::info!("NVIDIA CUDA support: enabled");

        #[cfg(feature = "vulkan")]
        log::info!("Vulkan GPU support: enabled");

        #[cfg(feature = "openmp")]
        log::info!("OpenMP parallel processing: enabled");

        let engine = Self {
            models_dir,
            current_context: Arc::new(RwLock::new(None)),
            current_model: Arc::new(RwLock::new(None)),
            available_models: Arc::new(RwLock::new(HashMap::new())),
            // Initialize state tracking
            last_transcription_was_short: Arc::new(RwLock::new(false)),
            short_audio_warning_logged: Arc::new(RwLock::new(false)),
            // Performance optimization: reduce logging frequency
            transcription_count: Arc::new(RwLock::new(0)),
            // Initialize cancellation tracking
            cancel_download_flag: Arc::new(RwLock::new(None)),
            // Initialize active downloads tracking
            active_downloads: Arc::new(RwLock::new(HashSet::new())),
        };

        Ok(engine)
    }

    pub async fn discover_models(&self) -> Result<Vec<ModelInfo>> {
        let models_dir = &self.models_dir;
        let mut models = Vec::new();
        // Use centralized model catalog from config.rs
        let model_configs = WHISPER_MODEL_CATALOG;

        for &(name, filename, size_mb, accuracy, speed, description) in model_configs {
            let model_path = models_dir.join(filename);
            let status = if model_path.exists() {
                // Check if file size is reasonable (at least 1MB for a valid model)
                match std::fs::metadata(&model_path) {
                    Ok(metadata) => {
                        let file_size_bytes = metadata.len();
                        let file_size_mb = file_size_bytes / (1024 * 1024);
                        let expected_min_size_mb = (size_mb as f64 * 0.9) as u64; // Allow 90% of expected size as minimum for more accurate corruption detection

                        if file_size_mb >= expected_min_size_mb && file_size_mb > 1 {
                            // File size looks good, but let's also check if it's a valid GGML file
                            match self.validate_model_file(&model_path).await {
                                Ok(_) => ModelStatus::Available,
                                Err(_) => {
                                    log::warn!("Model file {} has correct size but appears corrupted (failed validation)",
                                             filename);
                                    ModelStatus::Corrupted {
                                        file_size: file_size_bytes,
                                        expected_min_size: (expected_min_size_mb * 1024 * 1024)
                                            as u64,
                                    }
                                }
                            }
                        } else if file_size_mb > 0 {
                            // File exists but is smaller than expected
                            // Check if this model is currently being downloaded
                            let models_guard = self.available_models.read().await;
                            if let Some(existing_model) = models_guard.get(name) {
                                match &existing_model.status {
                                    ModelStatus::Downloading { progress } => {
                                        log::debug!("Model {} appears to be downloading ({} MB so far, {}% complete)",
                                                  filename, file_size_mb, progress);
                                        ModelStatus::Downloading {
                                            progress: *progress,
                                        }
                                    }
                                    _ => {
                                        log::warn!("Model file {} exists but is corrupted ({} MB, expected ~{} MB)",
                                                 filename, file_size_mb, size_mb);
                                        ModelStatus::Corrupted {
                                            file_size: file_size_bytes,
                                            expected_min_size: (expected_min_size_mb * 1024 * 1024)
                                                as u64,
                                        }
                                    }
                                }
                            } else {
                                log::warn!("Model file {} exists but is corrupted ({} MB, expected ~{} MB)",
                                         filename, file_size_mb, size_mb);
                                ModelStatus::Corrupted {
                                    file_size: file_size_bytes,
                                    expected_min_size: (expected_min_size_mb * 1024 * 1024) as u64,
                                }
                            }
                        } else {
                            ModelStatus::Missing
                        }
                    }
                    Err(_) => ModelStatus::Missing,
                }
            } else {
                ModelStatus::Missing
            };

            let model_info = ModelInfo {
                name: name.to_string(),
                path: model_path,
                size_mb: size_mb as u32,
                accuracy: accuracy.to_string(),
                speed: speed.to_string(),
                status,
                description: description.to_string(),
            };

            models.push(model_info);
        }

        // Update internal cache
        let mut available_models = self.available_models.write().await;
        available_models.clear();
        for model in &models {
            available_models.insert(model.name.clone(), model.clone());
        }

        Ok(models)
    }

    pub async fn load_model(&self, model_name: &str) -> Result<()> {
        let models = self.available_models.read().await;
        let model_info = models
            .get(model_name)
            .ok_or_else(|| anyhow!("Model {} not found", model_name))?;

        match model_info.status {
            ModelStatus::Available => {
                // FIX 5: Check if this model is already loaded
                // Clone the current model name before potentially unloading it.
                // Keeping the read guard alive while `unload_model()` requests a
                // write guard deadlocks every model switch.
                let current_model = self.current_model.read().await.clone();
                if let Some(current_model) = current_model {
                    if current_model == model_name {
                        log::info!("Model {} is already loaded, skipping reload", model_name);
                        return Ok(());
                    }

                    // FIX 5: Unload current model before loading new one
                    log::info!(
                        "Unloading current model '{}' before loading '{}'",
                        current_model,
                        model_name
                    );
                    self.unload_model().await;
                }

                log::info!("Loading model: {}", model_name);

                // PERFORMANCE OPTIMIZATION: Use comprehensive hardware profile for optimal GPU configuration
                let hardware_profile = crate::audio::HardwareProfile::detect();
                let adaptive_config = hardware_profile.get_whisper_config();
                let acceleration = whisper_context_acceleration_for(
                    WhisperCompiledBackend::current(),
                    hardware_profile.gpu_type,
                    hardware_profile.performance_tier,
                );

                let context_param = WhisperContextParameters {
                    use_gpu: acceleration.use_gpu,
                    gpu_device: acceleration.gpu_device,
                    flash_attn: acceleration.flash_attn,
                    ..Default::default()
                };

                log::info!(
                    "Whisper acceleration decision: compiled_backend={} runtime_detected_gpu={:?} use_gpu={} flash_attn={} gpu_device={}",
                    acceleration.compiled_backend.as_str(),
                    acceleration.runtime_detected_gpu,
                    acceleration.use_gpu,
                    acceleration.flash_attn,
                    acceleration.gpu_device,
                );

                // PERFORMANCE: Suppress verbose C library logs during model loading
                // This hides the excessive Metal/GGML initialization logs in release builds
                let ctx = {
                    // let _suppressor = crate::whisper_engine::StderrSuppressor::new();

                    // Load whisper context with hardware-optimized parameters
                    WhisperContext::new_with_params(
                        &model_info.path.to_string_lossy(),
                        context_param,
                    )
                    .map_err(|e| anyhow!("Failed to load model {}: {}", model_name, e))?
                    // Suppressor dropped here, stderr restored
                };

                // Update current context and model
                *self.current_context.write().await = Some(ctx);
                *self.current_model.write().await = Some(model_name.to_string());

                // Enhanced acceleration status reporting
                let acceleration_status = acceleration.status_label();

                log::info!("Successfully loaded model: {} with {} (Performance Tier: {:?}, Beam Size: {}, Threads: {:?})",
                          model_name, acceleration_status, hardware_profile.performance_tier,
                          adaptive_config.beam_size, adaptive_config.max_threads);
                Ok(())
            }
            ModelStatus::Missing => Err(anyhow!("Model {} is not downloaded", model_name)),
            ModelStatus::Downloading { .. } => {
                Err(anyhow!("Model {} is currently downloading", model_name))
            }
            ModelStatus::Error(ref err) => Err(anyhow!("Model {} has error: {}", model_name, err)),
            ModelStatus::Corrupted { .. } => Err(anyhow!(
                "Model {} is corrupted and cannot be loaded",
                model_name
            )),
        }
    }

    pub async fn unload_model(&self) -> bool {
        let mut ctx_guard = self.current_context.write().await;
        let unloaded = ctx_guard.take().is_some();
        if unloaded {
            log::info!("📉Whisper model unloaded");
        }

        let mut model_name_guard = self.current_model.write().await;
        model_name_guard.take();

        unloaded
    }

    pub async fn get_current_model(&self) -> Option<String> {
        self.current_model.read().await.clone()
    }

    pub async fn is_model_loaded(&self) -> bool {
        self.current_context.read().await.is_some()
    }

    // Shared text boundary for live and ordinary transcription.
    fn finalize_transcript_text(text: &str, language: Option<&str>) -> String {
        // Repeated words, counting, negation and quoted phrases can be real speech.
        // Text alone cannot establish that they are decoder hallucinations. Keep
        // every recognized word here; VAD and the worker's confidence gate remain
        // separate audio/model checks. The trace retains both raw and normalized text.
        let normalized = crate::transcript_normalization::normalize_transcript(text, language);
        // MAIN-046: canonical domain terms. Substitution only, never deletion.
        let (corrected, applied) = crate::transcript_term_correction::correct_terms(&normalized);
        if !applied.is_empty() {
            log::info!("term correction applied: {:?}", applied);
        }
        corrected
    }

    /// Transcribe audio with streaming support for partial results and adaptive quality
    pub async fn transcribe_audio_with_confidence(
        &self,
        audio_data: Vec<f32>,
        language: Option<String>,
    ) -> Result<(String, f32, bool)> {
        self.transcribe_audio_with_confidence_and_prompt(audio_data, language, None)
            .await
    }

    /// Live transcription with an optional meeting-scoped prompt. Callers must
    /// pass only the bounded prompt produced by RecognitionContext; this method
    /// intentionally does not construct or log vocabulary.
    pub async fn transcribe_audio_with_confidence_and_prompt(
        &self,
        audio_data: Vec<f32>,
        language: Option<String>,
        initial_prompt: Option<String>,
    ) -> Result<(String, f32, bool)> {
        let (_, text_after_dedup, confidence, is_partial) = self
            .transcribe_audio_with_confidence_and_prompt_trace(audio_data, language, initial_prompt)
            .await?;
        Ok((text_after_dedup, confidence, is_partial))
    }

    /// The D-11 controlled measurement path needs the actual text on both sides
    /// of the existing cleanup step. The ordinary API above deliberately keeps
    /// its old return type and output, so enabling this trace cannot alter a
    /// transcript or any decoding parameter.
    pub(crate) async fn transcribe_audio_with_confidence_and_prompt_trace(
        &self,
        audio_data: Vec<f32>,
        language: Option<String>,
        initial_prompt: Option<String>,
    ) -> Result<(String, String, f32, bool)> {
        let ctx_lock = self.current_context.read().await;
        let ctx = ctx_lock
            .as_ref()
            .ok_or_else(|| anyhow!("No model loaded. Please load a model first."))?;

        // Get adaptive configuration based on hardware
        let hardware_profile = crate::audio::HardwareProfile::detect();
        let adaptive_config = hardware_profile.get_whisper_config();

        // This method is the live/partial path. Long segments (>= 12 s) use the
        // measured quality plan; short VAD fragments stay greedy, because beam
        // search on a 3-5 second fragment costs several times the fragment
        // length for identical text (measured RTF 3.58 on a 5 second fragment).
        let (quality_mode, planned_audio_context) = live_decode_plan(audio_data.len());
        let sampling_strategy = if quality_mode {
            SamplingStrategy::BeamSearch {
                beam_size: WHISPER_LIVE_QUALITY_BEAM_SIZE,
                patience: 1.0,
            }
        } else {
            SamplingStrategy::Greedy { best_of: 1 }
        };
        let mut params = FullParams::new(sampling_strategy);

        // Configure with adaptive settings
        // If language is "auto" or None, use automatic language detection (pass None)
        // If language is "auto-translate", enable translation to English
        // Otherwise, use the specified language code
        let language_mode = resolve_whisper_language_mode(language.as_deref())?;
        params.set_language(language_mode.language_code.as_deref());
        params.set_translate(language_mode.translate_to_english);

        if let Some(prompt) = initial_prompt.as_deref() {
            params.set_initial_prompt(prompt);
        }

        // Keep native decoder boundaries: suppressing timestamp tokens caused
        // repeat loops on real Chinese speech. UI timing remains recording-relative.
        params.set_no_timestamps(false);
        params.set_token_timestamps(true); // Keep for any timestamp-aware features

        // PERFORMANCE: Disable ALL whisper.cpp internal printing
        // This reduces C library log spam significantly
        params.set_print_special(false); // Don't print special tokens
        params.set_print_progress(false); // Don't print progress
        params.set_print_realtime(false); // Don't print realtime info
        params.set_print_timestamps(false); // Don't print timestamps

        // Additional suppression to reduce C library verbosity
        params.set_suppress_blank(true);
        params.set_suppress_non_speech_tokens(true);
        params.set_temperature(adaptive_config.temperature);
        params.set_max_initial_ts(1.0);
        params.set_entropy_thold(2.4);
        params.set_logprob_thold(-1.0);
        // BALANCED FIX: Lowered from 0.75 to 0.55 to allow quiet speech detection
        // Previous value was too aggressive and rejected valid quiet speech
        // 0.55 is balanced - prevents hallucinations while preserving quiet speech
        params.set_no_speech_thold(0.55);
        params.set_max_len(200);
        params.set_single_segment(false);

        // The hardware detector already calculates this value. It must be applied
        // to FullParams; otherwise whisper.cpp falls back to its smaller default
        // and the logged thread count does not match the runtime behavior.
        if let Some(max_threads) = adaptive_config.max_threads {
            params.set_n_threads(max_threads as i32);
        }

        // Short fragments bound the encoder to the actual audio; long segments
        // take the native context that the measured quality plan needs.
        let audio_context = planned_audio_context;
        // 0 keeps whisper.cpp's default: no explicit decoder token ceiling.
        let max_tokens = 0;
        params.set_audio_ctx(audio_context);
        params.set_max_tokens(max_tokens);
        params.set_no_context(true);
        params.set_single_segment(false);
        params.set_token_timestamps(false);
        params.set_temperature(0.0);
        // Keep whisper.cpp's temperature fallback enabled. With
        // temperature_inc = 0.0 a decoder that fails the entropy check
        // (result_len > 32 && entropy < entropy_thold, i.e. a repetition
        // loop) is emitted as-is because there is no retry. Reproduced on a
        // real 2.0 s quiet/music fragment of a recorded meeting: the live
        // parameters emitted "太爽" x25, the same input with
        // temperature_inc = 0.2 emitted a single token group instead, while
        // every clean-speech fragment stayed byte-identical.
        params.set_temperature_inc(0.2);
        log::info!(
            "Live streaming inference ({:?}): {:.2}s audio, quality_mode={}, {} encoder contexts, {} max tokens (0 = whisper default), {} threads",
            WhisperCompiledBackend::current(),
            audio_data.len() as f64 / WHISPER_SAMPLE_RATE as f64,
            quality_mode,
            audio_context,
            max_tokens,
            adaptive_config.max_threads.unwrap_or(1)
        );

        let duration_seconds = audio_data.len() as f64 / 16000.0;
        let is_partial = duration_seconds < 15.0; // Consider chunks under 15s as partial

        // PERFORMANCE: Suppress verbose C library logs during transcription
        // This hides whisper_full_with_state debug logs and beam search details
        let (num_segments, state) = {
            // let _suppressor = crate::whisper_engine::StderrSuppressor::new();

            let mut state = ctx.create_state()?;
            state.full(params, &audio_data)?;
            let num_segments = state.full_n_segments();

            (num_segments, state)
            // Suppressor dropped here, stderr restored
        };
        let mut result = String::new();
        let mut token_probability_sum = 0.0_f32;
        let mut token_probability_count = 0_u32;

        let num_segments = num_segments?;
        for i in 0..num_segments {
            let segment_text = match state.full_get_segment_text(i) {
                Ok(text) => text,
                Err(_) => match state.full_get_segment_text_lossy(i) {
                    Ok(text) => text.replace('\u{FFFD}', ""),
                    Err(_) => continue,
                },
            };

            // Use the probabilities produced by Whisper. The old approximation
            // derived confidence from string length, so long hallucinations were
            // incorrectly reported as confidence 1.0 and always passed the live
            // worker's quality gate.
            if let Ok(token_count) = state.full_n_tokens(i) {
                for token_index in 0..token_count {
                    if let Ok(probability) = state.full_get_token_prob(i, token_index) {
                        if probability.is_finite() {
                            token_probability_sum += probability.clamp(0.0, 1.0);
                            token_probability_count += 1;
                        }
                    }
                }
            }

            let cleaned_text = segment_text.trim();
            if !cleaned_text.is_empty() {
                if !result.is_empty() {
                    result.push(' ');
                }
                result.push_str(cleaned_text);
            }
        }

        let final_result = result.trim().to_string();
        let cleaned_result = Self::finalize_transcript_text(&final_result, language.as_deref());

        let avg_confidence = if token_probability_count > 0 {
            token_probability_sum / token_probability_count as f32
        } else {
            0.0
        };

        Ok((final_result, cleaned_result, avg_confidence, is_partial))
    }

    /// Run a deterministic offline Whisper pass and retain the token times
    /// returned by whisper.cpp. This is intentionally separate from the live
    /// path: live fragments do not expose native word-level alignment and must
    /// not be reinterpreted as word-level evidence.
    pub async fn transcribe_audio_with_token_timestamps(
        &self,
        audio_data: Vec<f32>,
        language: Option<String>,
        initial_prompt: Option<String>,
    ) -> Result<WhisperTimedTranscript> {
        let (transcript, diagnostics) = self
            .transcribe_audio_with_token_timestamps_internal(
                audio_data,
                language,
                initial_prompt,
                None,
            )
            .await?;
        if diagnostics.is_some() {
            return Err(anyhow!(
                "ordinary Whisper token alignment unexpectedly used hotword bias"
            ));
        }
        Ok(transcript)
    }

    pub(crate) async fn transcribe_audio_with_token_timestamps_and_hotword_bias(
        &self,
        audio_data: Vec<f32>,
        language: Option<String>,
        canonical_terms: &[String],
        config: WhisperHotwordBiasConfig,
    ) -> Result<(WhisperTimedTranscript, WhisperHotwordBiasDiagnostics)> {
        let (transcript, diagnostics) = self
            .transcribe_audio_with_token_timestamps_internal(
                audio_data,
                language,
                None,
                Some((canonical_terms, config)),
            )
            .await?;
        let diagnostics =
            diagnostics.ok_or_else(|| anyhow!("Whisper hotword bias diagnostics are missing"))?;
        Ok((transcript, diagnostics))
    }

    async fn transcribe_audio_with_token_timestamps_internal(
        &self,
        audio_data: Vec<f32>,
        language: Option<String>,
        initial_prompt: Option<String>,
        hotword_bias: Option<(&[String], WhisperHotwordBiasConfig)>,
    ) -> Result<(
        WhisperTimedTranscript,
        Option<WhisperHotwordBiasDiagnostics>,
    )> {
        if audio_data.is_empty() {
            return Err(anyhow!("Whisper token alignment requires non-empty audio"));
        }
        if initial_prompt.is_some() && hotword_bias.is_some() {
            return Err(anyhow!(
                "Whisper initial prompt and hotword bias are mutually exclusive"
            ));
        }
        let audio_duration_ms = i64::try_from(
            audio_data
                .len()
                .checked_mul(1_000)
                .ok_or_else(|| anyhow!("Whisper token audio duration overflow"))?
                .div_ceil(WHISPER_SAMPLE_RATE),
        )?;
        let ctx_lock = self.current_context.read().await;
        let ctx = ctx_lock
            .as_ref()
            .ok_or_else(|| anyhow!("No model loaded. Please load a model first."))?;
        let end_of_text_token_id = ctx.token_eot();
        let hardware_profile = crate::audio::HardwareProfile::detect();
        let adaptive_config = hardware_profile.get_whisper_config();

        // Greedy + zero temperature makes the auxiliary track reproducible and
        // avoids beam alternatives changing token boundaries between runs.
        let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
        if let Some(max_threads) = adaptive_config.max_threads {
            params.set_n_threads(max_threads as i32);
        }
        let (language_code, should_translate) = match language.as_deref() {
            Some("auto") | None => (None, false),
            Some("auto-translate") => (None, true),
            Some(lang) => (Some(lang), false),
        };
        params.set_language(language_code);
        params.set_translate(should_translate);
        if let Some(prompt) = initial_prompt.as_deref() {
            params.set_initial_prompt(prompt);
        }

        // These two flags are the key safety boundary for R5. The returned
        // token t0/t1 values come from whisper.cpp; no character-proportional
        // or fixed-interval timestamp generation exists in this path.
        params.set_no_timestamps(false);
        params.set_token_timestamps(true);
        params.set_split_on_word(true);
        params.set_print_special(false);
        params.set_print_progress(false);
        params.set_print_realtime(false);
        params.set_print_timestamps(false);
        params.set_suppress_blank(true);
        params.set_suppress_non_speech_tokens(true);
        params.set_no_context(true);
        params.set_single_segment(false);
        params.set_temperature(0.0);
        params.set_temperature_inc(0.0);
        params.set_entropy_thold(2.4);
        params.set_logprob_thold(-1.0);
        params.set_no_speech_thold(0.55);

        let hotword_bias = hotword_bias
            .map(|(canonical_terms, config)| {
                WhisperHotwordBiasState::new(ctx, canonical_terms, config).map(Box::new)
            })
            .transpose()?;
        if let Some(bias) = hotword_bias.as_deref() {
            bias.install(&mut params);
        }

        let mut whisper_state = ctx.create_state()?;
        whisper_state.full(params, &audio_data)?;
        let hotword_bias_diagnostics = hotword_bias.as_deref().map(|bias| bias.diagnostics());
        let segment_count = whisper_state.full_n_segments()?;
        let mut text = String::new();
        let mut tokens = Vec::new();
        for segment_index in 0..segment_count {
            let segment_text = whisper_state
                .full_get_segment_text(segment_index)
                .or_else(|_| whisper_state.full_get_segment_text_lossy(segment_index))?;
            text.push_str(&segment_text);
            let token_count = whisper_state.full_n_tokens(segment_index)?;
            for token_index in 0..token_count {
                let token_text = whisper_state
                    .full_get_token_text(segment_index, token_index)
                    .or_else(|_| {
                        whisper_state.full_get_token_text_lossy(segment_index, token_index)
                    })?;
                let token_data = whisper_state.full_get_token_data(segment_index, token_index)?;
                if let Some(token) = validated_timed_token(
                    segment_index,
                    token_index,
                    token_text,
                    token_data.id,
                    end_of_text_token_id,
                    token_data.t0,
                    token_data.t1,
                    token_data.p,
                    audio_duration_ms,
                )? {
                    tokens.push(token);
                }
            }
        }
        Ok((
            validated_timed_transcript(text, tokens)?,
            hotword_bias_diagnostics,
        ))
    }

    pub async fn transcribe_audio(
        &self,
        audio_data: Vec<f32>,
        language: Option<String>,
    ) -> Result<String> {
        let ctx_lock = self.current_context.read().await;
        let ctx = ctx_lock
            .as_ref()
            .ok_or_else(|| anyhow!("No model loaded. Please load a model first."))?;

        // Get adaptive configuration based on hardware
        let hardware_profile = crate::audio::HardwareProfile::detect();
        let adaptive_config = hardware_profile.get_whisper_config();

        let sampling_strategy = if matches!(
            WhisperCompiledBackend::current(),
            WhisperCompiledBackend::Cpu
        ) {
            SamplingStrategy::Greedy { best_of: 1 }
        } else {
            SamplingStrategy::BeamSearch {
                beam_size: adaptive_config.beam_size as i32,
                patience: 1.0,
            }
        };
        let mut params = FullParams::new(sampling_strategy);

        if let Some(max_threads) = adaptive_config.max_threads {
            params.set_n_threads(max_threads as i32);
        }

        // Configure for good quality
        // If language is "auto" or None, use automatic language detection (pass None)
        // If language is "auto-translate", enable translation to English
        // Otherwise, use the specified language code
        let language_mode = resolve_whisper_language_mode(language.as_deref())?;
        params.set_language(language_mode.language_code.as_deref());
        params.set_translate(language_mode.translate_to_english);

        // Match the live decoder's native speech boundaries. This does not
        // replace the recording-relative timestamps used by the UI or storage.
        params.set_no_timestamps(false);
        params.set_token_timestamps(true); // Keep for any timestamp-aware features

        params.set_print_special(false);
        params.set_print_progress(false);
        params.set_print_realtime(false);
        params.set_print_timestamps(false);

        // BALANCED settings - good quality with reasonable speed
        params.set_suppress_blank(true);
        params.set_suppress_non_speech_tokens(true);
        params.set_temperature(0.3); // Lower than 0.4 for consistency, higher than 0.0 for quality
        params.set_max_initial_ts(1.0);
        params.set_entropy_thold(2.4);
        params.set_logprob_thold(-1.0);
        // BALANCED FIX: Lowered from 0.75 to 0.55 to allow quiet speech detection
        // Previous value was too aggressive and rejected valid quiet speech
        // 0.55 is balanced - prevents hallucinations while preserving quiet speech
        params.set_no_speech_thold(0.55);

        // Reasonable length limits
        params.set_max_len(200); // Reasonable length
        params.set_single_segment(false); // Allow multiple segments for better accuracy

        if matches!(
            WhisperCompiledBackend::current(),
            WhisperCompiledBackend::Cpu
        ) {
            let audio_context = streaming_audio_context(audio_data.len());
            // 0 keeps whisper.cpp's default: no explicit decoder token ceiling.
            let max_tokens = 0;
            params.set_audio_ctx(audio_context);
            params.set_max_tokens(max_tokens);
            params.set_no_context(true);
            params.set_single_segment(false);
            params.set_token_timestamps(false);
            params.set_temperature(0.0);
            // Same repetition-loop guard as the live path: on CPU this branch
            // used to disable whisper.cpp's temperature fallback, so a failed
            // (repeating) decode was kept. GPU runs never disabled it, which is
            // why the same audio could loop on CPU and not on GPU.
            params.set_temperature_inc(0.2);
        }

        // Note: compression_ratio_threshold would be ideal but not available in current whisper-rs
        // This would help detect repetitive outputs: params.set_compression_ratio_threshold(2.4);

        // Duration-based optimization is handled by beam search parameters
        let duration_seconds = audio_data.len() as f64 / 16000.0; // Assuming 16kHz
        let is_short_audio = duration_seconds < 1.0;

        // Smart logging based on audio duration and previous states
        let mut should_log_transcription = true;
        let mut should_log_short_warning = false;

        if is_short_audio {
            let last_was_short = *self.last_transcription_was_short.read().await;
            let warning_logged = *self.short_audio_warning_logged.read().await;

            if !warning_logged {
                should_log_short_warning = true;
                *self.short_audio_warning_logged.write().await = true;
            }

            // Only log transcription start if it's the first short audio or previous wasn't short
            should_log_transcription = !last_was_short;

            *self.last_transcription_was_short.write().await = true;
        } else {
            let last_was_short = *self.last_transcription_was_short.read().await;

            // Always log when transitioning from short to normal audio
            if last_was_short {
                log::info!("Audio duration normalized, resuming transcription");
                *self.short_audio_warning_logged.write().await = false;
            }

            *self.last_transcription_was_short.write().await = false;
        }

        if should_log_short_warning {
            log::warn!("Audio duration is short ({:.1}s < 1.0s). Consider padding the input audio with silence. Further short audio warnings will be suppressed.", duration_seconds);
        }

        // Performance optimization: reduce transcription start logging frequency
        let transcription_count = {
            let mut count = self.transcription_count.write().await;
            *count += 1;
            *count
        };

        // Only log every 10th transcription or significant audio (>10s) to reduce I/O overhead
        if should_log_transcription && (transcription_count % 10 == 0 || duration_seconds > 10.0) {
            log::info!(
                "Starting transcription #{} of {} samples ({:.1}s duration)",
                transcription_count,
                audio_data.len(),
                duration_seconds
            );
        }
        let mut state = ctx.create_state()?;
        state.full(params, &audio_data)?;

        // Extract text with improved segment handling
        let num_segments = state.full_n_segments()?;

        // Performance optimization: reduce segment completion logging
        // Only log for significant transcriptions to avoid I/O overhead
        if (should_log_transcription || num_segments > 0)
            && (num_segments > 3 || duration_seconds > 5.0)
        {
            perf_debug!(
                "Transcription #{} completed with {} segments ({:.1}s)",
                transcription_count,
                num_segments,
                duration_seconds
            );
        }
        let mut result = String::new();

        for i in 0..num_segments {
            let segment_text = match state.full_get_segment_text_lossy(i) {
                Ok(text) => text,
                Err(_) => continue,
            };

            let _start_time = state.full_get_segment_t0(i).unwrap_or(0);
            let _end_time = state.full_get_segment_t1(i).unwrap_or(0);

            // Performance optimization: remove per-segment debug logging
            // This was causing significant I/O overhead during transcription
            // Only log segments for very long audio (>30s) or when explicitly debugging
            if duration_seconds > 30.0 {
                perf_trace!(
                    "Segment {} ({:.2}s-{:.2}s): '{}'",
                    i,
                    _start_time as f64 / 100.0,
                    _end_time as f64 / 100.0,
                    segment_text
                );
            }

            // Clean and append segment text
            let cleaned_text = segment_text.trim();
            if !cleaned_text.is_empty() {
                if !result.is_empty() {
                    result.push(' ');
                }
                result.push_str(cleaned_text);
            }
        }

        let final_result = result.trim().to_string();

        let cleaned_result = Self::finalize_transcript_text(&final_result, language.as_deref());

        // Performance optimization: smart logging for transcription results
        if cleaned_result.is_empty() {
            // Only log empty results occasionally to reduce spam
            if should_log_transcription && transcription_count % 20 == 0 {
                perf_debug!(
                    "Transcription #{} result is empty - no speech detected",
                    transcription_count
                );
            }
        } else {
            if cleaned_result != final_result {
                log::info!(
                    "Normalized transcription #{}: '{}' -> '{}'",
                    transcription_count,
                    final_result,
                    cleaned_result
                );
            }
            // Reduce successful transcription logging frequency
            // Only log every 5th result or significant results (>50 chars) to reduce I/O overhead
            if transcription_count % 5 == 0 || cleaned_result.len() > 50 || duration_seconds > 10.0
            {
                log::info!(
                    "Transcription #{} result: '{}'",
                    transcription_count,
                    cleaned_result
                );
            } else {
                perf_debug!(
                    "Transcription #{} result: '{}'",
                    transcription_count,
                    cleaned_result
                );
            }
        }

        Ok(cleaned_result)
    }

    pub async fn get_models_directory(&self) -> PathBuf {
        self.models_dir.clone()
    }

    /// Validate if a model file is a valid GGML file by checking its header
    async fn validate_model_file(&self, model_path: &PathBuf) -> Result<()> {
        use tokio::io::AsyncReadExt;

        let mut file = fs::File::open(model_path)
            .await
            .map_err(|e| anyhow!("Failed to open model file: {}", e))?;

        // Read the first 8 bytes to check for GGML magic number
        let mut buffer = [0u8; 8];
        file.read_exact(&mut buffer)
            .await
            .map_err(|e| anyhow!("Failed to read model file header: {}", e))?;

        // Check for GGML magic number (various versions and endianness)
        if buffer.starts_with(b"ggml")
            || buffer.starts_with(b"GGUF")
            || buffer.starts_with(b"ggmf")
            || buffer.starts_with(b"lmgg")
            || buffer.starts_with(b"FUGU")
            || buffer.starts_with(b"fmgg")
        {
            Ok(())
        } else {
            Err(anyhow!(
                "Invalid model file: missing GGML/GGUF magic number. Found: {:?}",
                String::from_utf8_lossy(&buffer[..4])
            ))
        }
    }

    pub async fn delete_model(&self, model_name: &str) -> Result<String> {
        log::info!("Attempting to delete model: {}", model_name);

        // Get model info to find the file path
        let model_info = {
            let models = self.available_models.read().await;
            models.get(model_name).cloned()
        };

        let model_info = model_info.ok_or_else(|| anyhow!("Model '{}' not found", model_name))?;

        // Check if model is corrupted before allowing deletion
        log::info!("Model '{}' has status: {:?}", model_name, model_info.status);
        match &model_info.status {
            ModelStatus::Corrupted {
                file_size,
                expected_min_size,
            } => {
                log::info!(
                    "Deleting corrupted model '{}' (file size: {} bytes, expected min: {} bytes)",
                    model_name,
                    file_size,
                    expected_min_size
                );

                // Delete the file
                if model_info.path.exists() {
                    fs::remove_file(&model_info.path).await.map_err(|e| {
                        anyhow!(
                            "Failed to delete file '{}': {}",
                            model_info.path.display(),
                            e
                        )
                    })?;
                    log::info!(
                        "Successfully deleted corrupted file: {}",
                        model_info.path.display()
                    );
                } else {
                    log::warn!(
                        "File '{}' does not exist, nothing to delete",
                        model_info.path.display()
                    );
                }

                // Update model status to Missing
                {
                    let mut models = self.available_models.write().await;
                    if let Some(model) = models.get_mut(model_name) {
                        model.status = ModelStatus::Missing;
                    }
                }

                Ok(format!(
                    "Successfully deleted corrupted model '{}'",
                    model_name
                ))
            }
            ModelStatus::Available => {
                // Allow deletion of available models for testing/cleanup
                log::info!("Deleting available model '{}' (for cleanup)", model_name);

                if model_info.path.exists() {
                    fs::remove_file(&model_info.path).await.map_err(|e| {
                        anyhow!(
                            "Failed to delete file '{}': {}",
                            model_info.path.display(),
                            e
                        )
                    })?;
                    log::info!(
                        "Successfully deleted available model file: {}",
                        model_info.path.display()
                    );
                } else {
                    log::warn!(
                        "File '{}' does not exist, nothing to delete",
                        model_info.path.display()
                    );
                }

                // Update model status to Missing
                {
                    let mut models = self.available_models.write().await;
                    if let Some(model) = models.get_mut(model_name) {
                        model.status = ModelStatus::Missing;
                    }
                }

                Ok(format!("Successfully deleted model '{}'", model_name))
            }
            _ => Err(anyhow!(
                "Can only delete corrupted or available models. Model '{}' has status: {:?}",
                model_name,
                model_info.status
            )),
        }
    }

    pub async fn download_model(
        &self,
        model_name: &str,
        progress_callback: Option<Box<dyn Fn(u8) + Send>>,
    ) -> Result<()> {
        log::info!("Starting download for model: {}", model_name);

        // One atomic claim; every returned failure releases it so Retry works.
        if !self
            .active_downloads
            .write()
            .await
            .insert(model_name.to_string())
        {
            return Err(anyhow!(
                "Download already in progress for model: {}",
                model_name
            ));
        }
        let result = self
            .download_model_inner(model_name, progress_callback)
            .await;
        self.active_downloads.write().await.remove(model_name);
        if let Err(ref error) = result {
            if let Some(model) = self.available_models.write().await.get_mut(model_name) {
                model.status = ModelStatus::Error(error.to_string());
            }
        }
        result
    }

    async fn download_model_inner(
        &self,
        model_name: &str,
        progress_callback: Option<Box<dyn Fn(u8) + Send>>,
    ) -> Result<()> {
        // Clear any previous cancellation flag for this model
        {
            let mut cancel_flag = self.cancel_download_flag.write().await;
            *cancel_flag = None;
        }

        // Official ggerganov/whisper.cpp model URLs from Hugging Face
        let model_url = match model_name {
            // Standard f16 models
            "tiny" => "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-tiny.bin",
            "base" => "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-base.bin",
            "small" => "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-small.bin",
            "medium" => "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-medium.bin",
            "large-v3-turbo" => "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-large-v3-turbo.bin",
            "large-v3" => "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-large-v3.bin",

            // Q5_1 quantized models
            "tiny-q5_1" => "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-tiny-q5_1.bin",
            "base-q5_1" => "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-base-q5_1.bin",
            "small-q5_1" => "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-small-q5_1.bin",

            // Q5_0 quantized models
            "medium-q5_0" => "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-medium-q5_0.bin",
            "large-v3-turbo-q5_0" => "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-large-v3-turbo-q5_0.bin",
            "large-v3-q5_0" => "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-large-v3-q5_0.bin",

            _ => return Err(anyhow!("Unsupported model: {}", model_name))
        };

        log::info!("Model URL for {}: {}", model_name, model_url);

        // Generate correct filename - all models follow ggml-{model_name}.bin pattern
        let filename = format!("ggml-{}.bin", model_name);
        let file_path = self.models_dir.join(&filename);

        log::info!("Downloading to file path: {}", file_path.display());

        // Create models directory if it doesn't exist
        if !self.models_dir.exists() {
            fs::create_dir_all(&self.models_dir)
                .await
                .map_err(|e| anyhow!("Failed to create models directory: {}", e))?;
        }

        // Update model status to downloading
        {
            let mut models = self.available_models.write().await;
            if let Some(model_info) = models.get_mut(model_name) {
                model_info.status = ModelStatus::Downloading { progress: 0 };
            }
        }

        log::info!("Creating HTTP client and starting request...");
        let client = Client::new();

        log::info!("Sending GET request to: {}", model_url);
        let response = client
            .get(model_url)
            .send()
            .await
            .map_err(|e| anyhow!("Failed to start download: {}", e))?;

        log::info!("Received response with status: {}", response.status());
        if !response.status().is_success() {
            return Err(anyhow!(
                "Download failed with status: {}",
                response.status()
            ));
        }

        let total_size = response.content_length().unwrap_or(0);
        log::info!(
            "Response successful, content length: {} bytes ({:.1} MB)",
            total_size,
            total_size as f64 / (1024.0 * 1024.0)
        );

        if total_size == 0 {
            log::warn!("Content length is 0 or unknown - download may not show accurate progress");
        }

        let mut file = fs::File::create(&file_path)
            .await
            .map_err(|e| anyhow!("Failed to create file: {}", e))?;

        log::info!("File created successfully at: {}", file_path.display());

        // Stream download with real progress reporting
        log::info!("Starting streaming download...");
        log::info!(
            "Expected size: {:.1} MB",
            total_size as f64 / (1024.0 * 1024.0)
        );

        use futures_util::StreamExt;
        let mut stream = response.bytes_stream();
        let mut downloaded = 0u64;
        let mut last_progress_report = 0u8;
        let mut last_report_time = std::time::Instant::now();

        // Emit initial 0% progress immediately
        if let Some(ref callback) = progress_callback {
            callback(0);
        }

        while let Some(chunk_result) = stream.next().await {
            // Check for cancellation before processing chunk
            {
                let cancel_flag = self.cancel_download_flag.read().await;
                if cancel_flag.as_ref() == Some(&model_name.to_string()) {
                    log::info!("Download cancelled for {}", model_name);
                    return Err(anyhow!("Download cancelled by user"));
                }
            }

            let chunk = chunk_result.map_err(|e| anyhow!("Failed to read chunk: {}", e))?;

            file.write_all(&chunk)
                .await
                .map_err(|e| anyhow!("Failed to write chunk to file: {}", e))?;

            downloaded += chunk.len() as u64;

            // Calculate progress
            let progress = if total_size > 0 {
                ((downloaded as f64 / total_size as f64) * 100.0) as u8
            } else {
                0
            };

            // Report progress every 1% or every 2 seconds for better UI responsiveness
            let time_since_last_report = last_report_time.elapsed().as_secs();
            if progress >= last_progress_report + 1
                || progress == 100
                || time_since_last_report >= 2
            {
                log::info!(
                    "Download progress: {}% ({:.1} MB / {:.1} MB)",
                    progress,
                    downloaded as f64 / (1024.0 * 1024.0),
                    total_size as f64 / (1024.0 * 1024.0)
                );

                // Update progress in model info
                {
                    let mut models = self.available_models.write().await;
                    if let Some(model_info) = models.get_mut(model_name) {
                        model_info.status = ModelStatus::Downloading { progress };
                    }
                }

                // Call progress callback
                if let Some(ref callback) = progress_callback {
                    callback(progress);
                }

                last_progress_report = progress;
                last_report_time = std::time::Instant::now();
            }
        }

        log::info!("Streaming download completed: {} bytes", downloaded);

        // Ensure 100% progress is always reported
        {
            let mut models = self.available_models.write().await;
            if let Some(model_info) = models.get_mut(model_name) {
                model_info.status = ModelStatus::Downloading { progress: 100 };
            }
        }

        if let Some(ref callback) = progress_callback {
            callback(100);
        }

        file.flush()
            .await
            .map_err(|e| anyhow!("Failed to flush file: {}", e))?;

        log::info!("Download completed for model: {}", model_name);

        // Update model status to available
        {
            let mut models = self.available_models.write().await;
            if let Some(model_info) = models.get_mut(model_name) {
                model_info.status = ModelStatus::Available;
                model_info.path = file_path.clone();
            }
        }

        Ok(())
    }

    pub async fn cancel_download(&self, model_name: &str) -> Result<()> {
        log::info!("Cancelling download for model: {}", model_name);

        // Set cancellation flag to interrupt the download loop
        {
            let mut cancel_flag = self.cancel_download_flag.write().await;
            *cancel_flag = Some(model_name.to_string());
        }

        // Remove from active downloads
        {
            let mut active = self.active_downloads.write().await;
            active.remove(model_name);
        }

        // Update model status to Missing (so it can be retried)
        {
            let mut models = self.available_models.write().await;
            if let Some(model_info) = models.get_mut(model_name) {
                model_info.status = ModelStatus::Missing;
            }
        }

        // Clean up partially downloaded files
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await; // Brief delay to let download loop detect cancellation

        let filename = format!("ggml-{}.bin", model_name);
        let file_path = self.models_dir.join(&filename);
        if file_path.exists() {
            if let Err(e) = fs::remove_file(&file_path).await {
                log::warn!("Failed to clean up cancelled download file: {}", e);
            } else {
                log::info!(
                    "Cleaned up cancelled download file: {}",
                    file_path.display()
                );
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    #[ignore = "Requires the saved Chinese regression IPC input and installed small model"]
    async fn chinese_recording_preserves_speech_instead_of_decoder_repetition() {
        let input_path = std::env::var("MEETILY_CHINESE_REGRESSION_INPUT").unwrap();
        let models_dir = PathBuf::from(std::env::var("MEETILY_CHINESE_REGRESSION_MODELS").unwrap());
        let input: serde_json::Value =
            serde_json::from_slice(&std::fs::read(input_path).unwrap()).unwrap();
        let samples: Vec<f32> =
            serde_json::from_value(input["payload"]["audioData"].clone()).unwrap();
        assert_eq!(samples.len(), 20 * WHISPER_SAMPLE_RATE);
        let engine = WhisperEngine::new_with_models_dir(models_dir).unwrap();
        engine.discover_models().await.unwrap();
        engine.load_model("small").await.unwrap();
        let live = engine
            .transcribe_audio_with_confidence(samples.clone(), Some("zh".to_owned()))
            .await
            .unwrap()
            .0;
        let direct = engine
            .transcribe_audio(samples, Some("zh".to_owned()))
            .await
            .unwrap();
        eprintln!("Chinese regression live={live}; direct={direct}");
        for text in [live, direct] {
            assert!(
                text.contains("央行") && text.contains("利率"),
                "Speech disappeared: {text}"
            );
            assert!(
                !text.contains("处处处处处处"),
                "Decoder repetition replaced speech: {text}"
            );
        }
    }

    #[tokio::test]
    async fn failed_download_does_not_block_the_next_attempt() {
        let directory = tempfile::tempdir().unwrap();
        let engine = WhisperEngine::new_with_models_dir(directory.path().to_path_buf()).unwrap();
        for _ in 0..2 {
            let error = engine
                .download_model("unsupported-test-model", None)
                .await
                .unwrap_err();
            assert!(error.to_string().contains("Unsupported model"));
            assert!(engine.active_downloads.read().await.is_empty());
        }
    }

    #[test]
    fn streaming_context_covers_short_audio_with_padding_and_alignment() {
        assert_eq!(streaming_audio_context(0), 192);
        assert_eq!(streaming_audio_context(WHISPER_SAMPLE_RATE), 192);
        assert_eq!(streaming_audio_context(3 * WHISPER_SAMPLE_RATE), 192);
        assert_eq!(streaming_audio_context(7 * WHISPER_SAMPLE_RATE / 2), 256);
        assert_eq!(streaming_audio_context(5 * WHISPER_SAMPLE_RATE), 320);
        assert_eq!(streaming_audio_context(8 * WHISPER_SAMPLE_RATE), 448);
        assert_eq!(streaming_audio_context(10 * WHISPER_SAMPLE_RATE), 576);
    }

    #[test]
    fn streaming_context_never_exceeds_model_context() {
        assert_eq!(streaming_audio_context(28 * WHISPER_SAMPLE_RATE), 1500);
        assert_eq!(streaming_audio_context(60 * WHISPER_SAMPLE_RATE), 1500);
    }

    #[test]
    fn live_decode_plan_uses_quality_mode_only_for_long_segments() {
        // Short VAD-closed fragments stay greedy with the bounded context: the
        // quality plan costs RTF 3.58 there and returned identical text.
        assert_eq!(live_decode_plan(0), (false, 192));
        assert_eq!(live_decode_plan(5 * WHISPER_SAMPLE_RATE), (false, 320));
        assert_eq!(live_decode_plan(11 * WHISPER_SAMPLE_RATE), (false, 704));
        // Long segments take the plan that measured 0 wrong "VS" terms across
        // six 20 second windows.
        assert_eq!(live_decode_plan(12 * WHISPER_SAMPLE_RATE), (true, 1500));
        assert_eq!(live_decode_plan(20 * WHISPER_SAMPLE_RATE), (true, 1500));
        assert_eq!(live_decode_plan(25 * WHISPER_SAMPLE_RATE), (true, 1500));
    }

    #[test]
    fn timed_token_uses_native_ticks_without_character_interpolation() {
        let token = validated_timed_token(
            2,
            7,
            " Google".to_owned(),
            100,
            50_000,
            1_234,
            1_278,
            0.875,
            20_000,
        )
        .unwrap()
        .unwrap();
        assert_eq!(token.segment_index, 2);
        assert_eq!(token.token_index, 7);
        assert_eq!(token.text, " Google");
        assert_eq!(token.start_ms, 12_340);
        assert_eq!(token.end_ms, 12_780);
        assert_eq!(token.probability, 0.875);
    }

    #[test]
    fn timed_token_drops_control_tokens_and_rejects_invalid_machine_times() {
        assert!(validated_timed_token(
            0,
            0,
            "[_BEG_]".to_owned(),
            50_000,
            50_000,
            0,
            0,
            1.0,
            1_000,
        )
        .unwrap()
        .is_none());
        assert!(
            validated_timed_token(0, 1, " \n".to_owned(), 100, 50_000, 0, 0, 1.0, 1_000,)
                .unwrap()
                .is_none()
        );
        assert!(
            validated_timed_token(0, 0, "中文".to_owned(), 100, 50_000, -1, 10, 0.9, 1_000,)
                .is_err()
        );
        assert!(
            validated_timed_token(0, 0, "中文".to_owned(), 100, 50_000, 90, 101, 0.9, 1_000,)
                .is_err()
        );
        assert!(validated_timed_token(
            0,
            0,
            "中文".to_owned(),
            100,
            50_000,
            0,
            10,
            f32::NAN,
            1_000,
        )
        .is_err());
    }

    #[test]
    fn timed_transcript_rejects_empty_output_and_out_of_order_tokens() {
        let first = WhisperTimedToken {
            segment_index: 0,
            token_index: 0,
            text: "第一".to_owned(),
            start_ms: 100,
            end_ms: 200,
            probability: 0.9,
        };
        let second = WhisperTimedToken {
            segment_index: 0,
            token_index: 1,
            text: "第二".to_owned(),
            start_ms: 50,
            end_ms: 90,
            probability: 0.8,
        };
        assert!(validated_timed_transcript(String::new(), vec![first.clone()]).is_err());
        assert!(validated_timed_transcript("正文".to_owned(), Vec::new()).is_err());
        assert!(validated_timed_transcript("第一第二".to_owned(), vec![first, second]).is_err());
    }
}

#[cfg(test)]
mod transcript_finalization_tests {
    use super::WhisperEngine;

    #[test]
    fn preserves_repeated_negation_with_and_without_separators() {
        for text in ["不要不要", "不要不要。", "不要 不要 不要", "不不不不不"] {
            assert_eq!(
                WhisperEngine::finalize_transcript_text(text, Some("zh")),
                text
            );
        }
    }

    #[test]
    fn preserves_repeated_counting() {
        for text in [
            "一二三一二三",
            "一 二 三 一 二 三",
            "123123123123",
            "一二三一二三一二三一二三",
        ] {
            assert_eq!(
                WhisperEngine::finalize_transcript_text(text, Some("zh")),
                text
            );
        }
    }

    #[test]
    fn preserves_adjacent_phrases_in_context() {
        for text in [
            "这个问题这个问题需要再核对。",
            "会议结论会议结论会议结论，下一项行动",
            "会议摘要必须包含会议结论会议摘要必须包含会议结论会议摘要必",
        ] {
            assert_eq!(
                WhisperEngine::finalize_transcript_text(text, Some("zh")),
                text
            );
        }
    }

    #[test]
    fn preserves_repeated_endings_and_partial_phrases() {
        for text in [
            "数字或决定核心功能验收到这里结束核心功能验收到这里",
            "今天进行Midly核心功能验收今天进行Midly核",
            "今天讨论录音延迟，明天继续深入讨论录音延迟",
        ] {
            assert_eq!(
                WhisperEngine::finalize_transcript_text(text, Some("zh")),
                text
            );
        }
    }

    #[test]
    fn preserves_repeated_english_inside_chinese() {
        for text in [
            "API API API 需要 review",
            "we can we can ship today",
            "No no no 不要发布",
        ] {
            for language in [Some("zh"), Some("auto"), Some("en"), None] {
                assert_eq!(
                    WhisperEngine::finalize_transcript_text(text, language),
                    text
                );
            }
        }
    }

    #[test]
    fn preserves_frequent_words_in_a_long_utterance() {
        let text = "嗯 对 嗯 好 嗯 是 对 好 对 是 好 嗯 对 好 是 嗯 是 对 嗯 好";
        assert_eq!(
            WhisperEngine::finalize_transcript_text(text, Some("zh")),
            text
        );
    }

    #[test]
    fn preserves_spoken_phrases_previously_blacklisted_as_noise() {
        for text in [
            "thank you for watching",
            "介绍一下 applause 这个英文词",
            "um um um 然后继续",
        ] {
            assert_eq!(
                WhisperEngine::finalize_transcript_text(text, Some("zh")),
                text
            );
        }
    }

    #[test]
    fn keeps_existing_script_and_number_normalization() {
        assert_eq!(
            WhisperEngine::finalize_transcript_text(
                "會議會議使用 API API，2026年8月24日，M100。",
                Some("zh")
            ),
            "会议会议使用 API API，二〇二六年八月二十四日，M100。"
        );
        assert_eq!(
            WhisperEngine::finalize_transcript_text("會議會議 API API", Some("auto")),
            "會議會議 API API"
        );
    }

    #[test]
    fn preserves_empty_and_ordinary_text() {
        for text in [
            "",
            "今天开会讨论预算。",
            "我们用 ChatGPT review 这个 API，再用 Terra 和 Luna。",
        ] {
            assert_eq!(
                WhisperEngine::finalize_transcript_text(text, Some("zh")),
                text
            );
        }
    }
}

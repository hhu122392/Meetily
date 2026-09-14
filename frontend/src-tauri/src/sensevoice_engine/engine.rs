//! SenseVoice-Small inference engine backed by sherpa-onnx.

use super::model::{self, ModelInfo, ModelStatus};
use anyhow::{anyhow, Context, Result};
use sherpa_onnx::OfflineRecognizer;
use super::recognizer::create_recognizer;
use super::live::SenseVoiceSession;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::RwLock;

const DEFAULT_MODEL: &str = "sensevoice-small-int8";
/// Bound the legacy batch API's working window.
const MAX_SEGMENT_SECONDS: f32 = 30.0;

struct LoadedModel {
    name: String,
    recognizer: Arc<OfflineRecognizer>,
}

pub struct SenseVoiceEngine {
    models_root: PathBuf,
    loaded: RwLock<Option<LoadedModel>>,
}

impl SenseVoiceEngine {
    pub fn new_with_models_root(models_root: PathBuf) -> Result<Self> {
        std::fs::create_dir_all(&models_root)
            .with_context(|| format!("Failed to create {}", models_root.display()))?;
        Ok(Self { models_root, loaded: RwLock::new(None) })
    }

    pub fn models_root(&self) -> &Path {
        &self.models_root
    }

    pub fn models_directory(&self) -> PathBuf {
        self.models_root.join("sensevoice")
    }

    pub fn discover_models(&self) -> Result<Vec<ModelInfo>> {
        model::discover_models(&self.models_root)
    }

    pub async fn is_model_loaded(&self) -> bool {
        self.loaded.read().await.is_some()
    }

    pub async fn get_current_model(&self) -> Option<String> {
        self.loaded.read().await.as_ref().map(|loaded| loaded.name.clone())
    }

    pub async fn new_live_session(&self) -> Result<SenseVoiceSession> {
        let loaded = self.loaded.read().await;
        let recognizer = loaded.as_ref().map(|model| model.recognizer.clone())
            .ok_or_else(|| anyhow!("No SenseVoice model loaded"))?;
        Ok(SenseVoiceSession::new(recognizer))
    }

    /// Load a model whose files are already on disk.
    pub async fn load_model(&self, model_name: &str) -> Result<()> {
        let info = model::model_info(&self.models_root, model_name)?;
        if info.status != ModelStatus::Available {
            return Err(anyhow!(
                "Model {model_name} is not fully downloaded (status: {:?})",
                info.status
            ));
        }

        let dir = model::model_directory(&self.models_root, model_name);
        let recognizer = tokio::task::spawn_blocking(move || create_recognizer(&dir))
            .await
            .context("SenseVoice load task panicked")??;

        let mut guard = self.loaded.write().await;
        *guard = Some(LoadedModel {
            name: model_name.to_string(),
            recognizer: Arc::new(recognizer),
        });
        log::info!("✅ SenseVoice model '{model_name}' loaded");
        Ok(())
    }

    pub async fn unload_model(&self) -> bool {
        let mut guard = self.loaded.write().await;
        let had_model = guard.take().is_some();
        if had_model {
            log::info!("SenseVoice model unloaded");
        }
        had_model
    }

    /// Transcribe 16 kHz mono f32 audio. Returns plain text (no timestamps).
    pub async fn transcribe(&self, audio: Vec<f32>) -> Result<String> {
        if audio.is_empty() {
            return Ok(String::new());
        }
        let recognizer = {
            let guard = self.loaded.read().await;
            guard
                .as_ref()
                .map(|loaded| Arc::clone(&loaded.recognizer))
                .ok_or_else(|| anyhow!("No SenseVoice model loaded"))?
        };

        tokio::task::spawn_blocking(move || {
            let mut text = String::new();
            for window in split_windows(&audio) {
                let stream = recognizer.create_stream();
                stream.accept_waveform(16_000, window);
                recognizer.decode(&stream);
                if let Some(result) = stream.get_result() {
                    let piece = result.text.trim();
                    if !piece.is_empty() {
                        if !text.is_empty() {
                            text.push(' ');
                        }
                        text.push_str(piece);
                    }
                }
            }
            text
        })
        .await
        .context("SenseVoice transcription task panicked")
    }

    pub async fn delete_model(&self, model_name: &str) -> Result<()> {
        let mut guard = self.loaded.write().await;
        if guard.as_ref().map(|loaded| loaded.name == model_name).unwrap_or(false) {
            guard.take();
        }
        drop(guard);
        model::delete_model(&self.models_root, model_name)
    }

    pub fn default_model_name() -> &'static str {
        DEFAULT_MODEL
    }
}

/// Split long audio into SenseVoice-sized windows, never dropping samples.
fn split_windows(audio: &[f32]) -> Vec<&[f32]> {
    let window = (MAX_SEGMENT_SECONDS * 16_000.0) as usize;
    if audio.len() <= window {
        return vec![audio];
    }
    audio.chunks(window).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_audio_stays_in_one_window() {
        let audio = vec![0.0_f32; 16_000];
        assert_eq!(split_windows(&audio).len(), 1);
    }

    #[test]
    fn long_audio_is_split_without_losing_samples() {
        let audio = vec![0.0_f32; 16_000 * 65];
        let windows = split_windows(&audio);
        assert_eq!(windows.len(), 3);
        assert_eq!(windows.iter().map(|w| w.len()).sum::<usize>(), audio.len());
        assert!(windows.iter().all(|w| w.len() <= 16_000 * 30));
    }
}

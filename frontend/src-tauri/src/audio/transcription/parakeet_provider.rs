// audio/transcription/parakeet_provider.rs
//
// Parakeet transcription provider implementation.

use super::provider::{TranscriptResult, TranscriptionError, TranscriptionProvider};
use async_trait::async_trait;
use std::sync::Arc;

/// Parakeet TDT v3 performs automatic detection for its own supported
/// languages. It does not accept a manual language hint and it has no Whisper
/// style translate-to-English task. Reject incompatible preferences instead of
/// silently producing text in an unexpected language.
pub(crate) fn validate_parakeet_language(
    language: Option<&str>,
) -> std::result::Result<(), TranscriptionError> {
    match language.map(str::trim).filter(|value| !value.is_empty()) {
        None => Ok(()),
        Some(value) if value.eq_ignore_ascii_case("auto") => Ok(()),
        Some(value) => Err(TranscriptionError::UnsupportedLanguage(value.to_string())),
    }
}

/// Parakeet transcription provider (wraps ParakeetEngine)
pub struct ParakeetProvider {
    engine: Arc<crate::parakeet_engine::ParakeetEngine>,
}

impl ParakeetProvider {
    pub fn new(engine: Arc<crate::parakeet_engine::ParakeetEngine>) -> Self {
        Self { engine }
    }
}

#[async_trait]
impl TranscriptionProvider for ParakeetProvider {
    async fn transcribe(
        &self,
        audio: Vec<f32>,
        language: Option<String>,
    ) -> std::result::Result<TranscriptResult, TranscriptionError> {
        validate_parakeet_language(language.as_deref())?;

        match self.engine.transcribe_audio(audio).await {
            Ok(text) => Ok(TranscriptResult {
                text: text.trim().to_string(),
                confidence: None,  // Parakeet doesn't provide confidence scores
                is_partial: false, // Parakeet doesn't provide partial results
            }),
            Err(e) => Err(TranscriptionError::EngineFailed(e.to_string())),
        }
    }

    async fn is_model_loaded(&self) -> bool {
        self.engine.is_model_loaded().await
    }

    async fn get_current_model(&self) -> Option<String> {
        self.engine.get_current_model().await
    }

    fn provider_name(&self) -> &'static str {
        "Parakeet"
    }
}

#[cfg(test)]
mod tests {
    use super::validate_parakeet_language;
    use crate::audio::transcription::provider::TranscriptionError;

    #[test]
    fn accepts_only_automatic_detection() {
        assert!(validate_parakeet_language(None).is_ok());
        assert!(validate_parakeet_language(Some("")).is_ok());
        assert!(validate_parakeet_language(Some("auto")).is_ok());
        assert!(validate_parakeet_language(Some(" AUTO ")).is_ok());
    }

    #[test]
    fn rejects_manual_chinese_instead_of_silently_ignoring_it() {
        assert!(matches!(
            validate_parakeet_language(Some("zh")),
            Err(TranscriptionError::UnsupportedLanguage(language)) if language == "zh"
        ));
    }

    #[test]
    fn rejects_whisper_translation_mode() {
        assert!(matches!(
            validate_parakeet_language(Some("auto-translate")),
            Err(TranscriptionError::UnsupportedLanguage(language))
                if language == "auto-translate"
        ));
    }
}

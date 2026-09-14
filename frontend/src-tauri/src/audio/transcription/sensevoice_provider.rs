//! SenseVoice transcription provider (wraps `SenseVoiceEngine`).

use super::provider::{TranscriptResult, TranscriptionError, TranscriptionProvider};
use async_trait::async_trait;
use std::sync::Arc;

/// Languages the SenseVoice-Small checkpoint supports.
const SUPPORTED_LANGUAGES: [&str; 5] = ["zh", "en", "yue", "ja", "ko"];

/// Reject hints this checkpoint cannot honour instead of silently producing
/// text in another language. `auto` is accepted because the recognizer is
/// configured with a Chinese prior that also handles the mixed zh-en content
/// this integration targets.
pub(crate) fn validate_sensevoice_language(
    language: Option<&str>,
) -> std::result::Result<(), TranscriptionError> {
    match language.map(str::trim).filter(|value| !value.is_empty()) {
        None => Ok(()),
        Some(value) if value.eq_ignore_ascii_case("auto") => Ok(()),
        Some(value)
            if SUPPORTED_LANGUAGES
                .iter()
                .any(|supported| value.eq_ignore_ascii_case(supported)) =>
        {
            Ok(())
        }
        Some(value) => Err(TranscriptionError::UnsupportedLanguage(value.to_string())),
    }
}

pub struct SenseVoiceProvider {
    engine: Arc<crate::sensevoice_engine::SenseVoiceEngine>,
}

impl SenseVoiceProvider {
    pub fn new(engine: Arc<crate::sensevoice_engine::SenseVoiceEngine>) -> Self {
        Self { engine }
    }
}

#[async_trait]
impl TranscriptionProvider for SenseVoiceProvider {
    async fn transcribe(
        &self,
        audio: Vec<f32>,
        language: Option<String>,
    ) -> std::result::Result<TranscriptResult, TranscriptionError> {
        validate_sensevoice_language(language.as_deref())?;

        match self.engine.transcribe(audio).await {
            Ok(text) => Ok(TranscriptResult {
                text: text.trim().to_string(),
                confidence: None,  // SenseVoice does not expose token probabilities here
                is_partial: false, // The offline recognizer returns completed text only
            }),
            Err(error) => Err(TranscriptionError::EngineFailed(error.to_string())),
        }
    }

    async fn is_model_loaded(&self) -> bool {
        self.engine.is_model_loaded().await
    }

    async fn get_current_model(&self) -> Option<String> {
        self.engine.get_current_model().await
    }

    fn provider_name(&self) -> &'static str {
        "SenseVoice"
    }
}

#[cfg(test)]
mod tests {
    use super::validate_sensevoice_language;
    use crate::audio::transcription::provider::TranscriptionError;

    #[test]
    fn accepts_supported_languages_and_auto() {
        assert!(validate_sensevoice_language(None).is_ok());
        assert!(validate_sensevoice_language(Some("auto")).is_ok());
        assert!(validate_sensevoice_language(Some("zh")).is_ok());
        assert!(validate_sensevoice_language(Some(" EN ")).is_ok());
        assert!(validate_sensevoice_language(Some("yue")).is_ok());
    }

    #[test]
    fn rejects_whisper_translation_mode() {
        assert!(matches!(
            validate_sensevoice_language(Some("auto-translate")),
            Err(TranscriptionError::UnsupportedLanguage(language)) if language == "auto-translate"
        ));
    }

    #[test]
    fn rejects_unsupported_language() {
        assert!(matches!(
            validate_sensevoice_language(Some("de")),
            Err(TranscriptionError::UnsupportedLanguage(language)) if language == "de"
        ));
    }
}

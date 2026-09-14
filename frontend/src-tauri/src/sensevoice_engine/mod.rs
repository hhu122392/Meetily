//! SenseVoice-Small (sherpa-onnx) transcription engine.
//!
//! Added by MAIN-045 after the MAIN-044 benchmark showed SenseVoice-Small int8
//! runs at RTF 0.02 on this class of CPU (about 30x the previous whisper.cpp
//! configuration) while producing punctuation and no repetition loops. It is
//! registered as a `TranscriptionProvider`, which is the extension point the
//! transcription module recommends for new engines.

pub mod commands;
pub mod engine;
pub mod model;
pub mod boundary;
pub mod tail;
pub mod recognizer;
pub mod live;

pub use engine::SenseVoiceEngine;
pub use model::{ModelFileSpec, ModelInfo, ModelStatus, SENSEVOICE_MODELS};

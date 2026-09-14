//! Shared protocol and native boundary for the isolated MOSS transcription helper.
//!
//! The crate intentionally has no Python, llama.cpp, HTTP, or socket dependency.

pub mod native;
pub mod pcm;
pub mod protocol;

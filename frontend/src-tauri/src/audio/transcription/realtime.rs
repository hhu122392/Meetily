//! Live text is a preview. The durable saved-audio job produces the final transcript.
use super::engine::{get_or_init_transcription_engine, TranscriptionEngine};
use crate::audio::AudioChunk;
use serde_json::json;
use tauri::{AppHandle, Emitter, Manager, Runtime};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

pub fn start<R: Runtime>(
    app: AppHandle<R>,
    mut input: mpsc::UnboundedReceiver<AudioChunk>,
    folder: String,
    cancel: CancellationToken,
) -> tokio::task::JoinHandle<Result<(), String>> {
    tokio::spawn(async move {
        // Drain capture independently of inference; slow models cannot grow an unbounded preview queue.
        let (sender, mut receiver) = mpsc::channel(4);
        let drain_cancel = cancel.clone();
        let drain = tokio::spawn(async move {
            loop {
                tokio::select! {
                    biased;
                    _ = drain_cancel.cancelled() => break,
                    chunk = input.recv() => match chunk {
                        Some(chunk) => { let _ = sender.try_send(chunk); }
                        None => break,
                    }
                }
            }
        });
        let result = run(&app, &mut receiver, &folder, &cancel).await;
        cancel.cancel();
        let _ = drain.await;
        if let Err(ref error) = result {
            // This event is scoped to the capture that failed and never stops audio capture.
            let _ = app.emit("realtime-transcription-status", json!({
                "folder_path":folder, "message":format!("实时转写暂不可用，录音继续保存，会后完整转写：{error}")
            }));
        }
        result
    })
}

async fn run<R: Runtime>(
    app: &AppHandle<R>, receiver: &mut mpsc::Receiver<AudioChunk>, folder: &str,
    cancel: &CancellationToken,
) -> Result<(), String> {
    // Batch jobs may load/unload the same model. Hold their existing lock until preview finishes.
    let _guard = tokio::select! {
        biased;
        _ = cancel.cancelled() => return Ok(()),
        guard = crate::audio::common::BATCH_INFERENCE_LOCK.lock() => guard,
    };
    let config = crate::api::api::api_get_transcript_config(app.clone(), app.state(), None).await?;
    if config.as_ref().is_some_and(|c| !matches!(c.provider.as_str(), "sensevoice" | "localWhisper" | "parakeet")) {
        return Err("请先选择已下载的本地转写模型".into());
    }
    super::engine::validate_transcription_model_ready(app).await?;
    let engine = get_or_init_transcription_engine(app).await?;
    let language = crate::get_language_preference_internal();
    let mut sequence = 0_u64;
    loop {
        let chunk = tokio::select! {
            biased;
            _ = cancel.cancelled() => return Ok(()),
            chunk = receiver.recv() => match chunk { Some(chunk) => chunk, None => return Ok(()) },
        };
        let start = chunk.timestamp;
        let duration = chunk.data.len() as f64 / chunk.sample_rate as f64;
        let samples = if chunk.sample_rate == 16000 { chunk.data } else {
            crate::audio::audio_processing::resample_audio(&chunk.data, chunk.sample_rate, 16000)
        };
        let text = match &engine {
            TranscriptionEngine::Whisper(engine) => engine.transcribe_audio_with_confidence(samples, language.clone())
                .await.map(|r| r.0).map_err(|e| e.to_string()),
            TranscriptionEngine::Parakeet(engine) => engine.transcribe_audio(samples).await.map_err(|e| e.to_string()),
            TranscriptionEngine::Provider(engine) => engine.transcribe(samples, language.clone())
                .await.map(|r| r.text).map_err(|e| e.to_string()),
        };
        // An in-flight inference finishes while holding the model lock, but cannot update the next capture.
        if cancel.is_cancelled() { return Ok(()); }
        let text = text?;
        if !text.trim().is_empty() {
            app.emit("realtime-transcript", json!({
                "folder_path":folder, "text":text, "sequence_id":sequence,
                "timestamp":chrono::Local::now().format("%H:%M:%S").to_string(),
                "audio_start_time":start, "audio_end_time":start+duration, "duration":duration
            })).map_err(|e| e.to_string())?;
            sequence += 1;
        }
    }
}

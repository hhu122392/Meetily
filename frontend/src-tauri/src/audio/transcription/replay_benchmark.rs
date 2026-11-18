//! Opt-in local replay benchmark. Never opens a microphone, database or network API.
//! Uses production decoding, VAD, SenseVoice inference and batch boundary stitching.
use crate::audio::{common, decoder::decode_audio_file, vad::{get_speech_chunks_with_progress, LiveVadProcessor, SpeechSegment}};
use crate::sensevoice_engine::SenseVoiceEngine;
use anyhow::{Context, Result};
use serde_json::{json, Value};
use std::{path::{Path, PathBuf}, sync::Arc, time::{Duration, Instant}};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

const MODEL: &str = "sensevoice-small-int8";

fn decode(path: &Path) -> Result<(Vec<f32>, f64)> {
    let started = Instant::now();
    let decoded = decode_audio_file(path)?;
    let samples = decoded.to_whisper_format();
    Ok((samples, started.elapsed().as_secs_f64()))
}

async fn engine(models: &Path) -> Result<(Arc<SenseVoiceEngine>, f64)> {
    let started = Instant::now();
    let engine = Arc::new(SenseVoiceEngine::new_sensevoice(Some(models.to_owned()))?);
    engine.discover_models().await?;
    engine.load_model(MODEL).await?;
    Ok((engine, started.elapsed().as_secs_f64()))
}

fn write_report(output: &Path, name: &str, value: &Value) -> Result<()> {
    let path = output.join(format!("{name}.json"));
    let file = std::fs::OpenOptions::new().write(true).create_new(true).open(path)?;
    serde_json::to_writer_pretty(file, value)?;
    println!("完成 {name}");
    Ok(())
}

async fn batch(audio: &Path, models: &Path) -> Result<Value> {
    let started = Instant::now();
    let (samples, decode_seconds) = decode(audio)?;
    let audio_seconds = samples.len() as f64 / 16000.0;
    let vad_started = Instant::now();
    let speech = get_speech_chunks_with_progress(&samples, 2000, |_, _| true)?;
    let segments: Vec<_> = speech.iter().flat_map(|s| common::split_segment_at_silence(s, 25 * 16000)).collect();
    let vad_seconds = vad_started.elapsed().as_secs_f64();
    let (engine, model_load_seconds) = engine(models).await?;
    let mut raw = Vec::new();
    let mut records = Vec::new();
    let mut inference_seconds = 0.0;
    for segment in segments {
        if segment.samples.len() < 1600 { continue; }
        let inference_start = Instant::now();
        let text = engine.transcribe_audio_language(segment.samples, None).await?;
        let elapsed = inference_start.elapsed().as_secs_f64();
        inference_seconds += elapsed;
        records.push(json!({"start":segment.start_timestamp_ms / 1000.0, "end":segment.end_timestamp_ms / 1000.0, "text":text, "inference_seconds":elapsed}));
        if !text.trim().is_empty() { raw.push((text, segment.start_timestamp_ms, segment.end_timestamp_ms)); }
    }
    let stitched = common::create_transcript_segments(&raw);
    let text = stitched.iter().map(|s| s.text.as_str()).collect::<Vec<_>>().join("\n");
    let compute_seconds = started.elapsed().as_secs_f64();
    engine.unload_model().await;
    Ok(json!({"mode":"batch", "audio_seconds":audio_seconds, "decode_seconds":decode_seconds,
        "vad_seconds":vad_seconds, "model_load_seconds":model_load_seconds, "inference_seconds":inference_seconds,
        "compute_seconds":compute_seconds, "segments":records, "stitched_segments":stitched, "text":text,
        "excludes":"UI events, database checkpoints, output persistence, summary generation"}))
}

struct Ready { segment: SpeechSegment, ready_seconds: f64, index: usize }

async fn live(audio: &Path, models: &Path) -> Result<Value> {
    // Decode saved mixed audio once, then feed it at original speed in 50 ms windows.
    // Both paths receive identical 16 kHz mono samples. This does not test capture hardware.
    let (samples, decode_seconds) = decode(audio)?;
    let duration = samples.len() as f64 / 16000.0;
    let started = Instant::now();
    let cancel = CancellationToken::new();
    let producer_cancel = cancel.clone();
    let (sender, mut receiver) = mpsc::channel::<Ready>(4);
    let producer = tokio::task::spawn_blocking(move || -> Result<Value> {
        let mut vad = LiveVadProcessor::new(16000, 500)?;
        let mut produced = Vec::new();
        let mut vad_seconds = 0.0;
        let mut processed = 0;
        let mut index = 0;
        let mut emit = |segments: Vec<SpeechSegment>| {
            for segment in segments {
                if segment.samples.len() < 800 { continue; }
                let ready_seconds = started.elapsed().as_secs_f64();
                let start = segment.start_timestamp_ms / 1000.0;
                let end = segment.end_timestamp_ms / 1000.0;
                let accepted = sender.try_send(Ready { segment, ready_seconds, index }).is_ok();
                produced.push(json!({"index":index, "start":start, "end":end, "ready_seconds":ready_seconds, "accepted":accepted}));
                index += 1;
            }
        };
        for samples in samples.chunks(800) {
            processed += samples.len();
            let deadline = started + Duration::from_secs_f64(processed as f64 / 16000.0);
            std::thread::sleep(deadline.saturating_duration_since(Instant::now()));
            let tick = Instant::now();
            let segments = vad.process_audio(samples)?;
            vad_seconds += tick.elapsed().as_secs_f64();
            emit(segments);
        }
        let tick = Instant::now();
        emit(vad.flush()?);
        vad_seconds += tick.elapsed().as_secs_f64();
        // Match stop-recording policy: cancel preview after flushing; batch recovers tail.
        producer_cancel.cancel();
        Ok(json!({"segments":produced, "vad_seconds":vad_seconds, "stop_seconds":started.elapsed().as_secs_f64()}))
    });
    let (engine, model_load_seconds) = engine(models).await?;
    let mut records = Vec::new();
    let mut inference_seconds = 0.0;
    loop {
        let ready = tokio::select! {
            biased;
            _ = cancel.cancelled() => break,
            value = receiver.recv() => match value { Some(value) => value, None => break },
        };
        let inference_start = Instant::now();
        let text = engine.transcribe_audio_language(ready.segment.samples, None).await?;
        let elapsed = inference_start.elapsed().as_secs_f64();
        inference_seconds += elapsed;
        let completed_seconds = started.elapsed().as_secs_f64();
        let visible = !cancel.is_cancelled() && !text.trim().is_empty();
        records.push(json!({"index":ready.index, "start":ready.segment.start_timestamp_ms / 1000.0,
            "end":ready.segment.end_timestamp_ms / 1000.0, "ready_seconds":ready.ready_seconds,
            "completed_seconds":completed_seconds, "inference_seconds":elapsed, "visible":visible, "text":text}));
        if cancel.is_cancelled() { break; }
    }
    let production = producer.await??;
    let elapsed_seconds = started.elapsed().as_secs_f64();
    let text = records.iter().filter(|r| r["visible"] == true).map(|r| r["text"].as_str().unwrap()).collect::<Vec<_>>().join("\n");
    engine.unload_model().await;
    Ok(json!({"mode":"live_replay", "audio_seconds":duration, "decode_seconds":decode_seconds,
        "model_load_seconds":model_load_seconds, "inference_seconds":inference_seconds,
        "elapsed_seconds":elapsed_seconds, "production":production, "segments":records, "text":text,
        "policy":"50ms replay; live VAD 500ms silence and 8s windows; queue capacity 4; stop cancels pending preview"}))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "需要显式提供本地录音、模型和独立输出目录；按原速播放整段录音"]
async fn compare_saved_recording() -> Result<()> {
    let audio = PathBuf::from(std::env::var("MEETILY_BENCH_AUDIO").context("未设置录音路径")?);
    let models = PathBuf::from(std::env::var("MEETILY_BENCH_MODELS").context("未设置模型目录")?);
    let output = PathBuf::from(std::env::var("MEETILY_BENCH_OUTPUT").context("未设置独立输出目录")?);
    anyhow::ensure!(audio.is_file() && models.is_dir(), "本地录音或模型不存在");
    std::fs::create_dir(&output).context("输出目录必须不存在，避免覆盖既有结果")?;
    println!("开始会后转写基线");
    let first = batch(&audio, &models).await?;
    println!("会后第 1 次：{:.3} 秒", first["compute_seconds"].as_f64().unwrap());
    write_report(&output, "batch-1", &first)?;
    println!("开始原速实时回放，耗时约 {:.1} 秒", first["audio_seconds"].as_f64().unwrap());
    write_report(&output, "live", &live(&audio, &models).await?)?;
    for run in 2..=3 {
        let result = batch(&audio, &models).await?;
        println!("会后第 {run} 次：{:.3} 秒", result["compute_seconds"].as_f64().unwrap());
        write_report(&output, &format!("batch-{run}"), &result)?;
    }
    Ok(())
}

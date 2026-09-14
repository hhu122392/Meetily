//! TASK-03A: exercise the real saver with temporary audio, not source-string checks.
use super::*;
use crate::audio::incremental_saver::cleanup_checkpoints;
use crate::audio::recording_state::DeviceType;
use std::time::Duration;
use tempfile::tempdir;

fn app() -> tauri::App<tauri::test::MockRuntime> {
    tauri::test::mock_builder()
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .unwrap()
}

fn tone(id: u64, seconds: f32, frequency: f32) -> AudioChunk {
    let samples = (48_000.0 * seconds) as usize;
    AudioChunk {
        data: (0..samples)
            .map(|index| 0.25 * (std::f32::consts::TAU * frequency * index as f32 / 48_000.0).sin())
            .collect(),
        sample_rate: 48_000,
        timestamp: id as f64 * seconds as f64,
        chunk_id: id,
        device_type: DeviceType::Microphone,
        device_epoch: 0,
        capture_qpc_ns: None,
    }
}

fn decode_samples(path: &std::path::Path) -> Vec<f32> {
    let ffmpeg =
        crate::audio::ffmpeg::find_ffmpeg_path().expect("FFmpeg is required for audio tests");
    let output = std::process::Command::new(ffmpeg)
        .args(["-v", "error", "-i"])
        .arg(path)
        .args(["-f", "f32le", "-ac", "1", "-ar", "48000", "pipe:1"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "decode failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(output.stdout.len() % 4, 0);
    output
        .stdout
        .chunks_exact(4)
        .map(|bytes| f32::from_le_bytes(bytes.try_into().unwrap()))
        .collect()
}

#[tokio::test]
async fn slow_writer_preserves_all_accepted_audio_before_finalizing() {
    let directory = tempdir().unwrap();
    let app = app();
    let mut saver = RecordingSaver::new(directory.path().to_path_buf());
    saver.set_meeting_name(Some("slow-writer".to_owned()));
    let sender = saver.start_accumulation(true).unwrap();
    let writer = saver.incremental_saver.as_ref().unwrap().clone();
    let held_writer = writer.lock().await;
    sender.send(tone(0, 0.5, 440.0)).unwrap();
    sender.send(tone(1, 0.5, 880.0)).unwrap();
    drop(sender);
    // Put the accumulator ahead of finalization in the mutex's FIFO wait queue.
    tokio::task::yield_now().await;

    let (result, ()) = tokio::join!(saver.stop_and_save(app.handle(), Some(1.0)), async {
        tokio::time::sleep(Duration::from_secs(2)).await;
        drop(held_writer);
    });
    let audio = PathBuf::from(result.unwrap().unwrap());
    let samples = decode_samples(&audio);
    // AAC may add a small final padded frame. It must not remove the second tone.
    assert!(
        (48_000..=50_176).contains(&samples.len()),
        "expected both half-second chunks, decoded {} samples",
        samples.len()
    );
    let tail = &samples[43_200..47_040];
    let tail_rms =
        (tail.iter().map(|sample| sample * sample).sum::<f32>() / tail.len() as f32).sqrt();
    assert!(
        tail_rms > 0.1,
        "the final tone was lost or replaced with silence"
    );
    assert!(audio.parent().unwrap().join(".checkpoints").is_dir());
}

#[tokio::test]
async fn earlier_write_error_cannot_be_hidden_by_a_later_successful_merge() {
    let directory = tempdir().unwrap();
    let app = app();
    let mut saver = RecordingSaver::new(directory.path().to_path_buf());
    saver.set_meeting_name(Some("transient-write-error".to_owned()));
    let sender = saver.start_accumulation(true).unwrap();
    let folder = saver.meeting_folder.as_ref().unwrap().clone();
    let blocked_output = folder.join(".checkpoints/audio_chunk_000.mp4");
    std::fs::create_dir(&blocked_output).unwrap();
    let writer = saver.incremental_saver.as_ref().unwrap().clone();
    let held_writer = writer.lock().await;
    sender.send(tone(0, 30.0, 440.0)).unwrap();
    tokio::task::yield_now().await;
    drop(held_writer);
    // FIFO acquisition ensures the queued write has really attempted encoding.
    let held_writer = writer.lock().await;
    assert_eq!(held_writer.get_checkpoint_count(), 0);
    std::fs::remove_dir(&blocked_output).unwrap();
    drop(held_writer);
    drop(sender);

    let result = saver.stop_and_save(app.handle(), Some(30.0)).await;
    assert!(
        result.is_err(),
        "a failed write was silently reported as complete"
    );
    assert!(
        folder.join("audio.mp4").is_file(),
        "salvage the buffered audio when the disk recovers"
    );
    assert!(folder.join(".checkpoints").is_dir());
    let stored: MeetingMetadata =
        serde_json::from_str(&std::fs::read_to_string(folder.join("metadata.json")).unwrap())
            .unwrap();
    assert_ne!(stored.status, "completed");
    // Retrying the stop must not erase the failed writer result.
    assert!(saver.stop_and_save(app.handle(), Some(30.0)).await.is_err());
}

#[tokio::test]
async fn cleanup_without_complete_persistence_evidence_preserves_checkpoints() {
    let directory = tempdir().unwrap();
    let checkpoints = directory.path().join(".checkpoints");
    std::fs::create_dir(&checkpoints).unwrap();
    let checkpoint = checkpoints.join("audio_chunk_000.mp4");
    std::fs::write(&checkpoint, b"checkpoint evidence").unwrap();
    // File existence and the old completed status are not a database/file check.
    std::fs::write(directory.path().join("audio.mp4"), b"not validated").unwrap();
    std::fs::write(
        directory.path().join("metadata.json"),
        br#"{"status":"completed"}"#,
    )
    .unwrap();

    let result = cleanup_checkpoints(directory.path().to_string_lossy().into_owned()).await;
    assert!(result.is_err(), "unverified cleanup was allowed");
    assert_eq!(std::fs::read(checkpoint).unwrap(), b"checkpoint evidence");
}

#[tokio::test]
async fn transcript_only_mode_finishes_without_audio_or_checkpoints() {
    let directory = tempdir().unwrap();
    let app = app();
    let mut saver = RecordingSaver::new(directory.path().to_path_buf());
    saver.set_meeting_name(Some("transcript-only".to_owned()));
    let sender = saver.start_accumulation(false).unwrap();
    sender.send(tone(0, 0.1, 440.0)).unwrap();
    drop(sender);
    assert_eq!(
        saver.stop_and_save(app.handle(), Some(0.1)).await.unwrap(),
        None
    );
    let folder = saver.meeting_folder.as_ref().unwrap();
    assert!(!folder.join("audio.mp4").exists());
    assert!(!folder.join(".checkpoints").exists());
    assert!(folder.join("transcripts.json").is_file());
}

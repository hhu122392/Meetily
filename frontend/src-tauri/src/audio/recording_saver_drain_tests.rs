//! TASK-03A regression tests. Synthetic audio and isolated temporary folders only.
use super::*;
use crate::audio::decoder::decode_audio_file;
use crate::audio::recording_state::DeviceType;
use tempfile::tempdir;
use tokio::time::{sleep, Duration, Instant};

fn audio_chunk(id: u64, samples: usize, amplitude: f32) -> AudioChunk {
    AudioChunk {
        data: (0..samples)
            .map(|i| amplitude * (std::f32::consts::TAU * 880.0 * i as f32 / 48_000.0).sin())
            .collect(),
        sample_rate: 48_000,
        timestamp: id as f64 * samples as f64 / 48_000.0,
        chunk_id: id,
        device_type: DeviceType::Microphone,
        device_epoch: 0,
        capture_qpc_ns: None,
    }
}

#[tokio::test]
async fn slow_audio_write_drains_every_accepted_chunk_and_preserves_tail_signal() {
    let directory = tempdir().unwrap();
    let mut saver = RecordingSaver::new(directory.path().to_path_buf());
    saver.set_meeting_name(Some("03A slow write".to_owned()));
    let sender = saver.start_accumulation(true).unwrap();
    let incremental = saver.incremental_saver.as_ref().unwrap().clone();
    let write_gate = incremental.lock().await;
    for id in 0..30 {
        sender
            .send(audio_chunk(id, 4_800, if id >= 27 { 0.7 } else { 0.15 }))
            .unwrap();
    }
    // Keep a sender alive: shutdown must close admission rather than wait for
    // every clone to be dropped, and must still drain already accepted samples.
    let app = tauri::test::mock_builder()
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .unwrap();
    let started = Instant::now();
    let (result, ()) = tokio::join!(saver.stop_and_save(app.handle(), Some(3.0)), async move {
        sleep(Duration::from_secs(2)).await;
        drop(write_gate);
    });
    let audio_path = result.expect("queued audio must be saved").unwrap();
    assert!(started.elapsed() >= Duration::from_secs(2));
    assert!(
        sender.is_closed(),
        "late sends must not be silently accepted"
    );
    let decoded = decode_audio_file(std::path::Path::new(&audio_path)).unwrap();
    assert_eq!(decoded.sample_rate, 48_000);
    assert_eq!(decoded.channels, 1);
    assert!(
        (decoded.duration_seconds - 3.0).abs() < 0.1,
        "expected all 3 seconds, got {}",
        decoded.duration_seconds
    );
    let tail = &decoded.samples[134_400..139_200]; // 2.8s..2.9s
    let rms = (tail.iter().map(|sample| sample * sample).sum::<f32>() / tail.len() as f32).sqrt();
    assert!(
        rms > 0.3,
        "tail marker was lost or replaced by silence: {rms}"
    );
    assert!(saver
        .get_meeting_folder()
        .unwrap()
        .join(".checkpoints")
        .is_dir());
}

#[tokio::test]
async fn failed_audio_write_is_not_hidden_by_a_later_successful_finalize() {
    let directory = tempdir().unwrap();
    let mut saver = RecordingSaver::new(directory.path().to_path_buf());
    saver.set_meeting_name(Some("03A write failure".to_owned()));
    let sender = saver.start_accumulation(true).unwrap();
    let folder = saver.get_meeting_folder().unwrap().clone();
    let blocked_output = folder.join(".checkpoints/audio_chunk_000.mp4");
    std::fs::create_dir(&blocked_output).unwrap();
    sender.send(audio_chunk(0, 1_440_000, 0.2)).unwrap();
    drop(sender);
    tokio::task::yield_now().await;
    let app = tauri::test::mock_builder()
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .unwrap();
    assert!(saver.stop_and_save(app.handle(), Some(30.0)).await.is_err());
    let stored: MeetingMetadata =
        serde_json::from_str(&std::fs::read_to_string(folder.join("metadata.json")).unwrap())
            .unwrap();
    assert_eq!(stored.status, "error");
    assert!(folder.join(".checkpoints").is_dir());
    std::fs::remove_dir(&blocked_output).unwrap();
    // A new recovery operation must acknowledge/reconcile the failure. Merely
    // repeating the stop call cannot turn the failed write into full success.
    assert!(saver.stop_and_save(app.handle(), Some(30.0)).await.is_err());
}

#[tokio::test]
async fn transcripts_only_shutdown_creates_no_audio_or_checkpoints() {
    let directory = tempdir().unwrap();
    let mut saver = RecordingSaver::new(directory.path().to_path_buf());
    saver.set_meeting_name(Some("03A text only".to_owned()));
    let sender = saver.start_accumulation(false).unwrap();
    sender.send(audio_chunk(0, 4_800, 0.2)).unwrap();
    let app = tauri::test::mock_builder()
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .unwrap();
    assert_eq!(
        saver.stop_and_save(app.handle(), Some(0.1)).await.unwrap(),
        None
    );
    assert!(sender.is_closed());
    let folder = saver.get_meeting_folder().unwrap();
    assert!(!folder.join(".checkpoints").exists());
    assert!(!folder.join("audio.mp4").exists());
    assert!(folder.join("transcripts.json").is_file());
}

#[tokio::test]
async fn cancelled_stop_still_waits_for_the_same_save_task_on_retry() {
    let directory = tempdir().unwrap();
    let app = tauri::test::mock_builder()
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .unwrap();
    let mut saver = RecordingSaver::new(directory.path().to_path_buf());
    saver.set_meeting_name(Some("03A cancelled stop".to_owned()));
    let sender = saver.start_accumulation(true).unwrap();
    let incremental = saver.incremental_saver.as_ref().unwrap().clone();
    let write_gate = incremental.lock().await;
    for id in 0..2 {
        sender
            .send(audio_chunk(id, 24_000, if id == 0 { 0.2 } else { 0.7 }))
            .unwrap();
    }
    tokio::task::yield_now().await;
    drop(sender);

    // Cancel only the wait, just as the caller's file-I/O timeout does.
    let cancelled = tokio::time::timeout(
        Duration::from_millis(150),
        saver.stop_and_save(app.handle(), Some(1.0)),
    )
    .await;
    assert!(cancelled.is_err(), "precondition: first stop was cancelled");

    let started = Instant::now();
    let retried = tokio::time::timeout(Duration::from_secs(5), async {
        let (result, ()) = tokio::join!(saver.stop_and_save(app.handle(), Some(1.0)), async move {
            sleep(Duration::from_millis(500)).await;
            drop(write_gate);
        });
        result
    })
    .await
    .expect("retry must finish after the writer is released");
    assert!(started.elapsed() >= Duration::from_millis(500));
    let audio_path = retried
        .expect("retry must return the real save result")
        .unwrap();
    let decoded = decode_audio_file(std::path::Path::new(&audio_path)).unwrap();
    assert!(
        (decoded.duration_seconds - 1.0).abs() < 0.1,
        "both accepted chunks must survive cancellation, got {} seconds",
        decoded.duration_seconds
    );
    assert_eq!(decoded.sample_rate, 48_000);
    assert!(decoded.samples.len() >= 43_200);
    let tail = &decoded.samples[38_400..43_200]; // 0.8s..0.9s, before AAC padding
    let rms = (tail.iter().map(|sample| sample * sample).sum::<f32>() / tail.len() as f32).sqrt();
    assert!(rms > 0.3, "tail chunk was lost or silenced: {rms}");
}

#[tokio::test]
async fn starting_again_cannot_replace_an_unfinished_recording() {
    let directory = tempdir().unwrap();
    let mut saver = RecordingSaver::new(directory.path().to_path_buf());
    saver.set_meeting_name(Some("03A start twice".to_owned()));
    let sender = saver.start_accumulation(false).unwrap();
    let folder = saver.get_meeting_folder().unwrap().clone();
    assert!(saver.start_accumulation(false).is_err());
    assert_eq!(saver.get_meeting_folder(), Some(&folder));
    assert!(
        !sender.is_closed(),
        "the original writer must not be replaced"
    );
}

#[tokio::test]
async fn aborted_writer_is_reported_as_failure_and_preserves_transcripts() {
    let directory = tempdir().unwrap();
    let app = tauri::test::mock_builder()
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .unwrap();
    let mut saver = RecordingSaver::new(directory.path().to_path_buf());
    saver.set_meeting_name(Some("03A aborted writer".to_owned()));
    let sender = saver.start_accumulation(false).unwrap();
    saver.save_task.as_ref().unwrap().abort();
    let error = saver
        .stop_and_save(app.handle(), Some(0.1))
        .await
        .unwrap_err();
    assert!(error.contains("Audio writer task failed"), "{error}");
    assert!(sender.is_closed());
    let folder = saver.get_meeting_folder().unwrap();
    let stored: MeetingMetadata =
        serde_json::from_slice(&std::fs::read(folder.join("metadata.json")).unwrap()).unwrap();
    assert_eq!(stored.status, "error");
    assert!(folder.join("transcripts.json").is_file());
    assert!(!folder.join("audio.mp4").exists());
    assert!(saver.stop_and_save(app.handle(), Some(0.1)).await.is_err());
}

#[tokio::test]
async fn repeated_successful_stop_preserves_saved_transcripts() {
    let directory = tempdir().unwrap();
    let app = tauri::test::mock_builder()
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .unwrap();
    let mut saver = RecordingSaver::new(directory.path().to_path_buf());
    saver.set_meeting_name(Some("03A repeated stop".to_owned()));
    let sender = saver.start_accumulation(true).unwrap();
    sender.send(audio_chunk(0, 4_800, 0.2)).unwrap();
    tokio::task::yield_now().await;
    drop(sender);
    saver
        .add_transcript_segment_with_result(TranscriptSegment {
            id: "tail-text".to_owned(),
            text: "末尾文字必须保留".to_owned(),
            audio_start_time: 0.0,
            audio_end_time: 0.1,
            duration: 0.1,
            display_time: "[00:00]".to_owned(),
            confidence: 1.0,
            sequence_id: 1,
            revision: 0,
            is_partial: false,
        })
        .unwrap();
    let first_path = saver.stop_and_save(app.handle(), Some(0.1)).await.unwrap();
    let transcript_file = saver.get_meeting_folder().unwrap().join("transcripts.json");
    let first: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&transcript_file).unwrap()).unwrap();
    let second_path = saver.stop_and_save(app.handle(), Some(0.1)).await.unwrap();
    let second: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&transcript_file).unwrap()).unwrap();
    assert_eq!(first_path, second_path);
    assert_eq!(first["segments"].as_array().unwrap().len(), 1);
    assert_eq!(second["segments"], first["segments"]);
}

fn final_segment(text: &str) -> TranscriptSegment {
    TranscriptSegment {
        id: "seg_1".to_owned(),
        text: text.to_owned(),
        sequence_id: 1,
        revision: 0,
        is_partial: false,
        audio_start_time: 1.0,
        audio_end_time: 2.0,
        duration: 1.0,
        display_time: "[00:01]".to_owned(),
        confidence: 1.0,
    }
}

#[test]
fn completed_snapshot_preserves_final_rows_and_rejects_unfinished_recording() {
    let directory = tempdir().unwrap();
    let path = directory.path();
    let row = final_segment("可以可以。");
    std::fs::write(path.join("transcripts.json"), serde_json::to_vec(&serde_json::json!({"segments":[row.clone()]})).unwrap()).unwrap();
    std::fs::write(path.join("metadata.json"), r#"{"status":"recording"}"#).unwrap();
    assert!(read_completed_transcript_snapshot(path).is_err());
    std::fs::write(path.join("metadata.json"), r#"{"status":"completed"}"#).unwrap();
    assert_eq!(read_completed_transcript_snapshot(path).unwrap()[0].text,"可以可以。");
    let mut pending=row;
    pending.is_partial=true;
    std::fs::write(path.join("transcripts.json"), serde_json::to_vec(&serde_json::json!({"segments":[pending]})).unwrap()).unwrap();
    assert!(read_completed_transcript_snapshot(path).is_err());
}

#[tokio::test]
async fn boundary_revision_snapshot_rejects_stale_partial_and_preserves_repeated_rows() {
    let directory=tempdir().unwrap();
    let mut saver=RecordingSaver::new(directory.path().to_path_buf());
    saver.set_meeting_name(Some("SenseVoice revision regression".to_owned()));
    let sender=saver.start_accumulation(false).unwrap();
    let writer=saver.transcript_writer();
    let mut final_row=final_segment("可以可以。");
    final_row.revision=2;
    let mut following=final_row.clone();
    following.id="seg_2".to_owned();
    following.sequence_id=2;
    following.revision=1;
    following.is_partial=true;
    following.audio_start_time=2.0;
    following.audio_end_time=3.0;
    writer.write_batch(vec![final_row.clone(),following]).unwrap();
    let mut late=final_row;
    late.text="旧的半句话".to_owned();
    late.revision=1;
    late.is_partial=true;
    writer.write(late).unwrap();
    let path=saver.get_meeting_folder().unwrap().join("transcripts.json");
    let stored:serde_json::Value=serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    assert_eq!(stored["segments"].as_array().unwrap().len(),2);
    assert_eq!(stored["segments"][0]["text"],"可以可以。");
    assert_eq!(stored["segments"][0]["is_partial"],false);
    assert_eq!(stored["segments"][1]["text"],"可以可以。");
    assert_eq!(stored["segments"][1]["is_partial"],true);
    assert_eq!(saver.get_transcript_segments()[0].revision,2);
    drop(sender);
}

#[tokio::test]
async fn owned_transcript_writer_survives_saver_move_and_upserts_the_tail() {
    let directory = tempdir().unwrap();
    let mut saver = RecordingSaver::new(directory.path().to_path_buf());
    saver.set_meeting_name(Some("03B moved saver".to_owned()));
    let sender = saver.start_accumulation(false).unwrap();
    let writer = saver.transcript_writer();
    let folder = saver.get_meeting_folder().unwrap().clone();
    // Shutdown takes the manager out of global state. The worker's writer
    // must still refer to this same meeting rather than finding a new global.
    let mut moved_saver = Some(saver).take().unwrap();
    writer.write(final_segment("第一版")).unwrap();
    writer
        .write(final_segment("停止后识别出的最后一句"))
        .unwrap();
    drop(writer);
    drop(sender);
    let app = tauri::test::mock_builder()
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .unwrap();
    moved_saver
        .stop_and_save(app.handle(), Some(2.0))
        .await
        .unwrap();
    let stored: serde_json::Value =
        serde_json::from_slice(&std::fs::read(folder.join("transcripts.json")).unwrap()).unwrap();
    assert_eq!(stored["total_segments"], 1);
    assert_eq!(stored["segments"][0]["text"], "停止后识别出的最后一句");
    assert!(!folder.join("audio.mp4").exists());
    assert!(!folder.join(".checkpoints").exists());
}

#[tokio::test]
async fn transcript_writer_is_bound_to_its_original_meeting() {
    let directory = tempdir().unwrap();
    let mut first = RecordingSaver::new(directory.path().to_path_buf());
    first.set_meeting_name(Some("03B first".to_owned()));
    let first_sender = first.start_accumulation(false).unwrap();
    let first_writer = first.transcript_writer();
    let first_folder = first.get_meeting_folder().unwrap().clone();
    let mut second = RecordingSaver::new(directory.path().to_path_buf());
    second.set_meeting_name(Some("03B second".to_owned()));
    let second_sender = second.start_accumulation(false).unwrap();
    second
        .transcript_writer()
        .write(final_segment("第二场"))
        .unwrap();
    first_writer
        .write(final_segment("第一场晚到的末句"))
        .unwrap();
    assert_eq!(first.get_transcript_segments()[0].text, "第一场晚到的末句");
    assert_eq!(second.get_transcript_segments()[0].text, "第二场");
    let stored: serde_json::Value =
        serde_json::from_slice(&std::fs::read(first_folder.join("transcripts.json")).unwrap())
            .unwrap();
    assert_eq!(stored["segments"][0]["text"], "第一场晚到的末句");
    drop((first_sender, second_sender));
}

#[tokio::test]
async fn transcript_write_failure_is_not_hidden_by_successful_final_save() {
    let directory = tempdir().unwrap();
    let mut saver = RecordingSaver::new(directory.path().to_path_buf());
    saver.set_meeting_name(Some("03B failed text write".to_owned()));
    let sender = saver.start_accumulation(false).unwrap();
    let folder = saver.get_meeting_folder().unwrap().clone();
    let blocked = folder.join(".transcripts.json.tmp");
    std::fs::create_dir(&blocked).unwrap();
    assert!(saver
        .transcript_writer()
        .write(final_segment("写盘失败也不能假装成功"))
        .is_err());
    std::fs::remove_dir(&blocked).unwrap();
    drop(sender);
    let app = tauri::test::mock_builder()
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .unwrap();
    assert!(saver.stop_and_save(app.handle(), Some(2.0)).await.is_err());
    let stored: MeetingMetadata =
        serde_json::from_slice(&std::fs::read(folder.join("metadata.json")).unwrap()).unwrap();
    assert_eq!(stored.status, "error");
    assert!(
        folder.join("transcripts.json").is_file(),
        "retain the recovered text"
    );
}

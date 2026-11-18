use std::path::PathBuf;
use anyhow::{Result, anyhow};
use log::{info, warn, error};
use super::encode::encode_single_audio;
use super::recording_state::AudioChunk;
use serde::{Serialize, Deserialize};

use super::ffmpeg::find_ffmpeg_path;

/// Audio data without device type (we only store mixed audio)
#[derive(Clone)]
struct AudioData {
    data: Vec<f32>,
    // sample_rate: u32,
}

/// Incremental audio saver that writes checkpoints every 30 seconds
/// to minimize memory usage and enable crash recovery
pub struct IncrementalAudioSaver {
    checkpoint_buffer: Vec<AudioData>,
    checkpoint_interval_samples: usize,  // 30s at 48kHz = 1,440,000 samples
    checkpoint_count: u32,
    checkpoints_dir: PathBuf,
    meeting_folder: PathBuf,
    sample_rate: u32,
    saved_bytes: u64,
}

impl IncrementalAudioSaver {
    #[cfg(test)]
    pub(crate) fn buffered_samples(&self) -> usize {
        self.checkpoint_buffer.iter().map(|c| c.data.len()).sum()
    }
    /// Create a new incremental saver
    ///
    /// # Arguments
    /// * `meeting_folder` - Path to the meeting folder (contains .checkpoints/)
    /// * `sample_rate` - Sample rate of audio (typically 48000)
    pub fn new(meeting_folder: PathBuf, sample_rate: u32) -> Result<Self> {
        let checkpoints_dir = meeting_folder.join(".checkpoints");

        // Verify checkpoints directory exists
        if !checkpoints_dir.exists() {
            return Err(anyhow!("录音片段目录不存在：{}", checkpoints_dir.display()));
        }

        Ok(Self {
            checkpoint_buffer: Vec::new(),
            checkpoint_interval_samples: sample_rate as usize * 30, // 30 seconds
            checkpoint_count: 0,
            checkpoints_dir,
            meeting_folder,
            sample_rate,
            saved_bytes: 0,
        })
    }

    /// Add an audio chunk to the buffer
    /// Automatically saves a checkpoint when buffer reaches 30 seconds
    pub fn add_chunk(&mut self, chunk: AudioChunk) -> Result<()> {
        let audio_data = AudioData {
            data: chunk.data,
            // sample_rate: chunk.sample_rate,
        };

        self.checkpoint_buffer.push(audio_data);

        // Calculate total samples in buffer
        let total_samples: usize = self.checkpoint_buffer
            .iter()
            .map(|c| c.data.len())
            .sum();

        // Save checkpoint when buffer reaches threshold (30 seconds)
        if total_samples >= self.checkpoint_interval_samples {
            self.save_checkpoint()?;
            self.checkpoint_buffer.clear();
        }

        Ok(())
    }

    /// Save current buffer as a checkpoint file
    fn save_checkpoint(&mut self) -> Result<()> {
        std::fs::metadata(&self.checkpoints_dir)?;
        // Concatenate all chunks in buffer
        let audio_data: Vec<f32> = self.checkpoint_buffer
            .iter()
            .flat_map(|c| &c.data)
            .cloned()
            .collect();

        if audio_data.is_empty() {
            warn!("Attempted to save empty checkpoint, skipping");
            return Ok(());
        }

        // Generate checkpoint filename
        let checkpoint_path = self.checkpoints_dir
            .join(format!("audio_chunk_{:03}.mp4", self.checkpoint_count));
        let pending = self.checkpoints_dir.join(format!(".pending-{}.partial", uuid::Uuid::new_v4()));

        // Encode and save checkpoint
        let result = encode_single_audio(
            bytemuck::cast_slice(&audio_data),
            self.sample_rate,
            1,  // mono
            &pending
        ).and_then(|_| publish_audio(&pending, &checkpoint_path, false));
        if result.is_err() { let _ = std::fs::remove_file(&pending); }
        self.saved_bytes = self.saved_bytes.saturating_add(result?);

        let duration_seconds = audio_data.len() as f32 / self.sample_rate as f32;
        self.checkpoint_count += 1;

        info!("Saved checkpoint {}: {:.2}s of audio ({} samples)",
              self.checkpoint_count,
              duration_seconds,
              audio_data.len());

        Ok(())
    }

    /// Finalize the recording: save final checkpoint, merge all checkpoints, cleanup
    ///
    /// Returns the path to the final merged audio.mp4 file
    pub async fn finalize(&mut self) -> Result<PathBuf> {
        info!("Finalizing incremental recording...");

        // Save final buffer if not empty
        if !self.checkpoint_buffer.is_empty() {
            info!("Saving final checkpoint with remaining {} chunks", self.checkpoint_buffer.len());
            self.save_checkpoint()?;
            self.checkpoint_buffer.clear();
        }

        if self.checkpoint_count == 0 {
            return Err(anyhow!("没有可合并的录音片段，录音可能未成功保存"));
        }

        // Merge all checkpoints using FFmpeg concat
        let final_audio_path = self.meeting_folder.join("audio.mp4");
        let pending = self.meeting_folder.join(format!(".audio-pending-{}.partial", uuid::Uuid::new_v4()));
        let result = self.merge_checkpoints(&pending).await
            .and_then(|_| publish_audio(&pending, &final_audio_path, true));
        if result.is_err() { let _ = std::fs::remove_file(&pending); }
        result?;

        // Clean up checkpoints directory
        info!("Cleaning up {} checkpoint files", self.checkpoint_count);
        if let Err(e) = std::fs::remove_dir_all(&self.checkpoints_dir) {
            warn!("Failed to clean up checkpoints directory: {}", e);
            // Non-fatal - user can manually delete
        }

        info!("Finalized recording: {}", final_audio_path.display());

        Ok(final_audio_path)
    }

    /// Merge all checkpoint files into final audio.mp4 using FFmpeg concat
    /// Uses concat demuxer for fast merging without re-encoding
    async fn merge_checkpoints(&self, output: &PathBuf) -> Result<()> {
        info!("Merging {} checkpoints into final audio file...", self.checkpoint_count);

        // Create concat list file for FFmpeg
        let list_file = self.checkpoints_dir.join("concat_list.txt");
        let mut list_content = String::new();

        for i in 0..self.checkpoint_count {
            let checkpoint_path = self.checkpoints_dir
                .join(format!("audio_chunk_{:03}.mp4", i));

            // Verify checkpoint exists
            if !checkpoint_path.exists() {
                return Err(anyhow!("录音片段文件缺失：{}", checkpoint_path.display()));
            }

            // Use absolute path for FFmpeg (required for safe mode)
            let abs_path = checkpoint_path.canonicalize()?;
            list_content.push_str(&concat_line(&abs_path));
        }

        std::fs::write(&list_file, list_content)?;

        let ffmpeg_path = find_ffmpeg_path()
            .ok_or_else(|| anyhow!("未找到 FFmpeg，无法完成录音保存，请修复或重新安装 Meetily。"))?;
        info!("Using FFmpeg at: {:?}", ffmpeg_path);

        // Run FFmpeg concat command
        // Using concat demuxer with copy codec for fast merging (no re-encoding)
        
        let mut command = std::process::Command::new(ffmpeg_path);
        
        command.args(&[
            "-f", "concat",          // Use concat demuxer
            "-safe", "0",            // Allow absolute paths
            "-i", list_file.to_str().unwrap(),
            "-c", "copy",            // Copy codec - no re-encoding!
            "-f", "mp4",
            "-y",                    // Overwrite output file
            output.to_str().unwrap()
        ]);

        // Hide console window on Windows to prevent CMD popup during finalization
        #[cfg(target_os = "windows")]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x08000000;
            command.creation_flags(CREATE_NO_WINDOW);
        }

        let ffmpeg_output = command.output()?;

        if !ffmpeg_output.status.success() {
            let stderr = String::from_utf8_lossy(&ffmpeg_output.stderr);
            error!("FFmpeg merge failed: {}", stderr);
            return Err(anyhow!("FFmpeg 合并录音失败：{}", stderr));
        }

        // Verify output file was created
        if !output.exists() {
            return Err(anyhow!("未生成合并后的录音文件：{}", output.display()));
        }

        info!("Successfully merged {} checkpoints → {}",
              self.checkpoint_count, output.display());

        Ok(())
    }

    /// Get the meeting folder path
    pub fn get_meeting_folder(&self) -> &PathBuf {
        &self.meeting_folder
    }

    /// Get current checkpoint count
    pub fn get_checkpoint_count(&self) -> u32 {
        self.checkpoint_count
    }

    pub(crate) fn saved_bytes(&self) -> u64 { self.saved_bytes }
}

fn publish_audio(pending: &std::path::Path, target: &std::path::Path, replace: bool) -> Result<u64> {
    let size = std::fs::metadata(pending)?.len();
    if size == 0 { return Err(anyhow!("音频文件为空，未替换原文件；已保存片段保留。")); }
    if !replace && target.exists() { return Err(anyhow!("录音片段已存在，未覆盖原片段。")); }
    std::fs::OpenOptions::new().write(true).open(pending)?.sync_all()?;
    std::fs::rename(pending, target)?;
    Ok(size)
}

fn checkpoint_index(path: &std::path::Path) -> Option<u32> {
    let name = path.file_name()?.to_str()?;
    let number = name.strip_prefix("audio_chunk_")?.strip_suffix(".mp4")?;
    if number.is_empty() || !number.bytes().all(|b| b.is_ascii_digit()) { return None; }
    number.parse().ok()
}

fn concat_line(path: &std::path::Path) -> String {
    format!("file '{}'\n", path.to_string_lossy().replace('\'', "'\\''"))
}

fn ordered_checkpoints(directory: &std::path::Path) -> Result<Vec<std::fs::DirEntry>> {
    let mut files = Vec::new();
    for entry in std::fs::read_dir(directory)? {
        let entry = entry?;
        if entry.file_type()?.is_file() && checkpoint_index(&entry.path()).is_some() { files.push(entry); }
    }
    files.sort_by_key(|entry| checkpoint_index(&entry.path()));
    Ok(files)
}

/// Audio recovery status for transcript recovery feature
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AudioRecoveryStatus {
    pub status: String, // "success" | "partial" | "failed" | "none"
    pub chunk_count: u32,
    pub estimated_duration_seconds: f64,
    pub audio_file_path: Option<String>,
    pub message: String,
}

/// Recover audio from checkpoint files
/// This is called by the transcript recovery system to merge audio chunks after a crash
#[tauri::command]
pub async fn recover_audio_from_checkpoints(
    meeting_folder: String,
    _sample_rate: u32
) -> Result<AudioRecoveryStatus, String> {
    info!("Starting audio recovery for folder: {}", meeting_folder);

    let folder_path = PathBuf::from(&meeting_folder);
    let checkpoints_dir = folder_path.join(".checkpoints");

    // Check if checkpoints directory exists
    if !checkpoints_dir.exists() {
        info!("No checkpoints directory found at: {}", checkpoints_dir.display());
        return Ok(AudioRecoveryStatus {
            status: "none".to_string(),
            chunk_count: 0,
            estimated_duration_seconds: 0.0,
            audio_file_path: None,
            message: "未找到录音片段".to_string(),
        });
    }

    // Scan for checkpoint files
    let checkpoint_files = ordered_checkpoints(&checkpoints_dir).map_err(|e| e.to_string())?;

    if checkpoint_files.is_empty() {
        info!("No checkpoint files found in: {}", checkpoints_dir.display());
        return Ok(AudioRecoveryStatus {
            status: "none".to_string(),
            chunk_count: 0,
            estimated_duration_seconds: 0.0,
            audio_file_path: None,
            message: "未找到录音片段文件".to_string(),
        });
    }

    let partial = checkpoint_files.iter().enumerate().any(|(i, entry)| checkpoint_index(&entry.path()) != Some(i as u32));
    if checkpoint_files.windows(2).any(|pair| checkpoint_index(&pair[0].path()) == checkpoint_index(&pair[1].path())) {
        return Err("发现重复编号的录音片段，已保留原文件，请先核对片段。".into());
    }

    let chunk_count = checkpoint_files.len() as u32;
    let estimated_duration = (chunk_count as f64) * 30.0; // 30 seconds per chunk

    info!("Found {} checkpoint files, estimated duration: {:.2}s", chunk_count, estimated_duration);

    // Create FFmpeg concat file
    let concat_file_path = checkpoints_dir.join("concat_list.txt");
    let mut concat_content = String::new();

    for entry in &checkpoint_files {
        let path = entry.path().canonicalize()
            .map_err(|e| format!("解析录音文件路径失败：{}", e))?;
        concat_content.push_str(&concat_line(&path));
    }

    std::fs::write(&concat_file_path, concat_content)
        .map_err(|e| format!("写入录音合并清单失败：{}", e))?;

    // Run FFmpeg to merge chunks
    let output_path = folder_path.join("audio.mp4");
    let pending = folder_path.join(format!(".recovery-pending-{}.partial", uuid::Uuid::new_v4()));
    let output_path_str = output_path.to_str()
        .ok_or("录音输出路径无效")?
        .to_string();

    let ffmpeg_path = find_ffmpeg_path()
        .ok_or_else(|| "未找到 FFmpeg，无法恢复录音，请修复或重新安装 Meetily。".to_string())?;
    info!("Using FFmpeg at: {:?}", ffmpeg_path);

    let mut command = std::process::Command::new(ffmpeg_path);

    command.args(&[
        "-f", "concat",
        "-safe", "0",
        "-i", concat_file_path.to_str().unwrap(),
        "-c", "copy",
        "-f", "mp4",
        "-y", // Overwrite if exists
        pending.to_str().ok_or("录音路径无效")?
    ]);

    // Hide console window on Windows
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x08000000;
        command.creation_flags(CREATE_NO_WINDOW);
    }

    let ffmpeg_result = command.output();

    match ffmpeg_result {
        Ok(output) if output.status.success() => {
            publish_audio(&pending, &output_path, true).map_err(|e| e.to_string())?;
            // Clean up concat file
            let _ = std::fs::remove_file(concat_file_path);

            info!("Successfully recovered audio: {}", output_path_str);

            Ok(AudioRecoveryStatus {
                status: if partial { "partial" } else { "success" }.to_string(),
                chunk_count,
                estimated_duration_seconds: estimated_duration,
                audio_file_path: Some(output_path_str),
                message: if partial { format!("已恢复 {chunk_count} 个可用片段，但编号有缺口，录音可能不完整，请核对。") }
                    else { format!("已恢复 {chunk_count} 个录音片段。") },
            })
        }
        Ok(output) => {
            let _ = std::fs::remove_file(&pending);
            let error = String::from_utf8_lossy(&output.stderr);
            error!("FFmpeg recovery failed: {}", error);
            Ok(AudioRecoveryStatus {
                status: "failed".to_string(),
                chunk_count,
                estimated_duration_seconds: estimated_duration,
                audio_file_path: None,
                message: format!("FFmpeg 恢复录音失败：{}", error),
            })
        }
        Err(e) => {
            let _ = std::fs::remove_file(&pending);
            error!("Failed to run FFmpeg: {}", e);
            Ok(AudioRecoveryStatus {
                status: "failed".to_string(),
                chunk_count,
                estimated_duration_seconds: estimated_duration,
                audio_file_path: None,
                message: format!("无法启动 FFmpeg 恢复录音：{}", e),
            })
        }
    }
}

/// Clean up checkpoint files after successful recording or recovery
/// This command is called by the frontend after successful save to clean up checkpoint files
#[tauri::command]
pub async fn cleanup_checkpoints(meeting_folder: String) -> Result<(), String> {
    info!("Cleaning up checkpoints for folder: {}", meeting_folder);

    let folder_path = PathBuf::from(&meeting_folder);
    let checkpoints_dir = folder_path.join(".checkpoints");

    if checkpoints_dir.exists() {
        std::fs::remove_dir_all(&checkpoints_dir)
            .map_err(|e| format!("清理录音片段目录失败：{}", e))?;
        info!("Successfully cleaned up checkpoints directory");
    } else {
        info!("No checkpoints directory to clean up");
    }

    Ok(())
}

/// Check if a meeting folder has audio checkpoint files
/// Returns true if .checkpoints/ directory exists and contains .mp4 files
#[tauri::command]
pub async fn has_audio_checkpoints(meeting_folder: String) -> Result<bool, String> {
    let folder_path = PathBuf::from(&meeting_folder);
    let checkpoints_dir = folder_path.join(".checkpoints");

    // Check if checkpoints directory exists
    if !checkpoints_dir.exists() {
        return Ok(false);
    }

    // Scan for .mp4 checkpoint files
    let has_mp4_files = std::fs::read_dir(&checkpoints_dir)
        .map_err(|e| format!("读取录音片段目录失败：{}", e))?
        .filter_map(|entry| entry.ok())
        .any(|entry| {
            checkpoint_index(&entry.path()).is_some()
        });

    Ok(has_mp4_files)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;
    use super::super::recording_state::DeviceType;

    #[test]
    fn recovery_orders_more_than_a_thousand_chunks_and_ignores_unpublished_audio() {
        let dir = tempdir().unwrap();
        for i in 0..1002 { std::fs::write(dir.path().join(format!("audio_chunk_{i:03}.mp4")), b"x").unwrap(); }
        std::fs::write(dir.path().join(".pending-incomplete.mp4"), b"x").unwrap();
        std::fs::write(dir.path().join("other.mp4"), b"x").unwrap();
        let files = ordered_checkpoints(dir.path()).unwrap();
        assert_eq!(files.len(), 1002);
        for (i, file) in files.iter().enumerate() { assert_eq!(checkpoint_index(&file.path()), Some(i as u32)); }
    }

    #[test]
    fn failed_publication_preserves_previous_audio() {
        let dir = tempdir().unwrap();
        let target = dir.path().join("audio.mp4");
        let pending = dir.path().join("pending.mp4");
        std::fs::write(&target, b"previous audio").unwrap();
        std::fs::write(&pending, b"").unwrap();
        assert!(publish_audio(&pending, &target, true).is_err());
        assert_eq!(std::fs::read(&target).unwrap(), b"previous audio");
        std::fs::write(&pending, b"new complete audio").unwrap();
        assert!(publish_audio(&pending, &target, false).is_err());
        assert_eq!(std::fs::read(&target).unwrap(), b"previous audio");
        assert_eq!(publish_audio(&pending, &target, true).unwrap(), 18);
        assert_eq!(std::fs::read(&target).unwrap(), b"new complete audio");
        assert!(!pending.exists());
    }

    #[tokio::test]
    async fn failed_recovery_keeps_original_audio_and_source_fragments() {
        let dir = tempdir().unwrap();
        let checkpoints = dir.path().join(".checkpoints");
        std::fs::create_dir(&checkpoints).unwrap();
        let fragment = checkpoints.join("audio_chunk_000.mp4");
        std::fs::write(&fragment, b"broken audio").unwrap();
        let original = dir.path().join("audio.mp4");
        std::fs::write(&original, b"previous valid recording").unwrap();
        let result = recover_audio_from_checkpoints(dir.path().to_string_lossy().into_owned(), 48000).await.unwrap();
        assert_eq!(result.status, "failed");
        assert_eq!(std::fs::read(original).unwrap(), b"previous valid recording");
        assert_eq!(std::fs::read(fragment).unwrap(), b"broken audio");
    }

    #[tokio::test]
    async fn test_checkpoint_creation() {
        // Create temp meeting folder
        let temp_dir = tempdir().unwrap();
        let meeting_folder = temp_dir.path().join("Test_Meeting's");
        std::fs::create_dir_all(&meeting_folder).unwrap();
        std::fs::create_dir_all(meeting_folder.join(".checkpoints")).unwrap();

        let mut saver = IncrementalAudioSaver::new(
            meeting_folder.clone(),
            48000
        ).unwrap();

        // Add 60 seconds worth of audio (should create 2 checkpoints)
        for i in 0..120 {  // 120 chunks of 0.5s each
            let chunk = AudioChunk {
                data: vec![0.5f32; 24000],  // 0.5s at 48kHz
                sample_rate: 48000,
                timestamp: i as f64 * 0.5,  // timestamp in seconds
                chunk_id: i as u64,
                device_type: DeviceType::Microphone,
            };
            saver.add_chunk(chunk).unwrap();
        }

        // Verify 2 checkpoints created
        assert_eq!(saver.checkpoint_count, 2);

        // A missing ordinal must be reported as partial recovery and keep fragments.
        let checkpoints = meeting_folder.join(".checkpoints");
        std::fs::rename(checkpoints.join("audio_chunk_001.mp4"), checkpoints.join("audio_chunk_002.mp4")).unwrap();
        let recovery = recover_audio_from_checkpoints(meeting_folder.to_string_lossy().into_owned(), 48000).await.unwrap();
        assert_eq!(recovery.status, "partial");
        assert!(checkpoints.join("audio_chunk_000.mp4").exists());
        assert!(checkpoints.join("audio_chunk_002.mp4").exists());
        std::fs::rename(checkpoints.join("audio_chunk_002.mp4"), checkpoints.join("audio_chunk_001.mp4")).unwrap();

        // Finalize and verify merge
        let final_path = saver.finalize().await.unwrap();
        assert!(final_path.exists());

        // Verify checkpoints directory deleted
        assert!(!meeting_folder.join(".checkpoints").exists());
    }

    #[tokio::test]
    async fn test_empty_recording() {
        let temp_dir = tempdir().unwrap();
        let meeting_folder = temp_dir.path().join("Empty_Test");
        std::fs::create_dir_all(&meeting_folder).unwrap();
        std::fs::create_dir_all(meeting_folder.join(".checkpoints")).unwrap();

        let mut saver = IncrementalAudioSaver::new(
            meeting_folder.clone(),
            48000
        ).unwrap();

        // Try to finalize without adding any chunks
        let result = saver.finalize().await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("没有可合并的录音片段"));
    }
}

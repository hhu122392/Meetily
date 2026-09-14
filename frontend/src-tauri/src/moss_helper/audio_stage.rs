use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};

use moss_helper::pcm::{analyze_activity, PcmActivity, MAX_AUDIO_SAMPLES};
use moss_helper::protocol::AudioSpec;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const OUTPUT_RATE: u32 = 16_000;
const OUTPUT_CHANNELS: u16 = 1;
const AUDIO_NAME: &str = "audio.f32le";
const MANIFEST_NAME: &str = "manifest.json";

#[derive(Debug, thiserror::Error)]
pub enum StageError {
    #[error("audio staging input is invalid")]
    InvalidInput,
    #[error("audio staging filesystem operation failed")]
    Io,
    #[error("audio staging manifest is invalid")]
    Manifest,
    #[error("audio staging was cancelled")]
    Cancelled,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StageManifest {
    task_id: String,
    files: Vec<String>,
}

pub struct StagedAudio {
    task_id: String,
    task_dir: PathBuf,
    manifest_path: PathBuf,
    pub spec: AudioSpec,
    pub activity: PcmActivity,
}

impl StagedAudio {
    pub fn cleanup(self) -> Result<(), StageError> {
        cleanup_manifest(&self.task_id, &self.task_dir, &self.manifest_path)
    }
}

pub fn stage_interleaved(
    cache_root: &Path,
    task_id: &str,
    samples: &[f32],
    sample_rate: u32,
    channels: u16,
) -> Result<StagedAudio, StageError> {
    stage_interleaved_with_cancel(cache_root, task_id, samples, sample_rate, channels, || {
        false
    })
}

pub fn stage_interleaved_with_cancel<F>(
    cache_root: &Path,
    task_id: &str,
    samples: &[f32],
    sample_rate: u32,
    channels: u16,
    should_cancel: F,
) -> Result<StagedAudio, StageError>
where
    F: Fn() -> bool,
{
    if !moss_helper::protocol::is_uuid(task_id)
        || samples.is_empty()
        || sample_rate == 0
        || channels == 0
        || channels > 32
        || samples.len() % channels as usize != 0
    {
        return Err(StageError::InvalidInput);
    }
    for chunk in samples.chunks(4_096) {
        if should_cancel() {
            return Err(StageError::Cancelled);
        }
        if chunk.iter().any(|sample| !sample.is_finite()) {
            return Err(StageError::InvalidInput);
        }
    }
    let input_frames = samples.len() / channels as usize;
    let maximum_input_frames = (sample_rate as u64)
        .checked_mul(moss_helper::pcm::MAX_AUDIO_SECONDS)
        .ok_or(StageError::InvalidInput)?;
    if input_frames as u64 > maximum_input_frames {
        return Err(StageError::InvalidInput);
    }

    let mono = downmix(samples, channels);
    if should_cancel() {
        return Err(StageError::Cancelled);
    }
    let mut output = if sample_rate == OUTPUT_RATE {
        mono
    } else {
        crate::audio::audio_processing::resample_audio(&mono, sample_rate, OUTPUT_RATE)
    };
    if output.is_empty()
        || output.len() as u64 > MAX_AUDIO_SAMPLES
        || output.iter().any(|sample| !sample.is_finite())
    {
        return Err(StageError::InvalidInput);
    }
    for (index, sample) in output.iter_mut().enumerate() {
        if index % 4_096 == 0 && should_cancel() {
            return Err(StageError::Cancelled);
        }
        *sample = sample.clamp(-1.0, 1.0);
    }
    let activity = analyze_activity(&output).map_err(|_| StageError::InvalidInput)?;

    let task_dir = cache_root.join(task_id);
    let audio_path = task_dir.join(AUDIO_NAME);
    let audio_path_text = audio_path
        .to_str()
        .ok_or(StageError::InvalidInput)?
        .to_owned();
    let audio_temp = task_dir.join(format!(".{AUDIO_NAME}.tmp"));
    let manifest_path = task_dir.join(MANIFEST_NAME);
    let manifest_temp = task_dir.join(format!(".{MANIFEST_NAME}.tmp"));
    std::fs::create_dir_all(cache_root).map_err(|_| StageError::Io)?;
    std::fs::create_dir(&task_dir).map_err(|_| StageError::Io)?;

    let write_result = (|| {
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&audio_temp)
            .map_err(|_| StageError::Io)?;
        let mut writer = BufWriter::new(file);
        let mut digest = Sha256::new();
        for (index, sample) in output.iter().enumerate() {
            if index % 4_096 == 0 && should_cancel() {
                return Err(StageError::Cancelled);
            }
            let bytes = sample.to_le_bytes();
            writer.write_all(&bytes).map_err(|_| StageError::Io)?;
            digest.update(bytes);
        }
        writer.flush().map_err(|_| StageError::Io)?;
        writer.get_ref().sync_all().map_err(|_| StageError::Io)?;
        std::fs::rename(&audio_temp, &audio_path).map_err(|_| StageError::Io)?;

        let manifest = StageManifest {
            task_id: task_id.to_string(),
            files: vec![AUDIO_NAME.to_string()],
        };
        write_json_atomic(&manifest_temp, &manifest_path, &manifest)?;
        Ok(format!("{:X}", digest.finalize()))
    })();

    let sha256 = match write_result {
        Ok(hash) => hash,
        Err(error) => {
            let _ = std::fs::remove_file(&audio_temp);
            let _ = std::fs::remove_file(&audio_path);
            let _ = std::fs::remove_file(&manifest_temp);
            let _ = std::fs::remove_file(&manifest_path);
            let _ = std::fs::remove_dir(&task_dir);
            return Err(error);
        }
    };

    Ok(StagedAudio {
        task_id: task_id.to_string(),
        task_dir,
        manifest_path,
        spec: AudioSpec {
            path: audio_path_text,
            format: "f32le".to_string(),
            sample_rate_hz: OUTPUT_RATE,
            channels: OUTPUT_CHANNELS,
            samples: output.len() as u64,
            bytes: (output.len() as u64) * 4,
            sha256,
        },
        activity,
    })
}

fn downmix(samples: &[f32], channels: u16) -> Vec<f32> {
    let channels = channels as usize;
    if channels == 1 {
        return samples.to_vec();
    }
    samples
        .chunks_exact(channels)
        .map(|frame| frame.iter().copied().sum::<f32>() / channels as f32)
        .collect()
}

fn write_json_atomic(
    temp_path: &Path,
    destination: &Path,
    value: &StageManifest,
) -> Result<(), StageError> {
    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(temp_path)
        .map_err(|_| StageError::Io)?;
    let mut writer = BufWriter::new(file);
    serde_json::to_writer(&mut writer, value).map_err(|_| StageError::Manifest)?;
    writer.flush().map_err(|_| StageError::Io)?;
    writer.get_ref().sync_all().map_err(|_| StageError::Io)?;
    std::fs::rename(temp_path, destination).map_err(|_| StageError::Io)
}

fn cleanup_manifest(
    expected_task_id: &str,
    task_dir: &Path,
    manifest_path: &Path,
) -> Result<(), StageError> {
    let file = File::open(manifest_path).map_err(|_| StageError::Io)?;
    let manifest: StageManifest =
        serde_json::from_reader(file).map_err(|_| StageError::Manifest)?;
    if manifest.task_id != expected_task_id {
        return Err(StageError::Manifest);
    }
    for relative in &manifest.files {
        let relative_path = Path::new(relative);
        if relative_path.is_absolute()
            || relative_path.components().count() != 1
            || relative_path.file_name().and_then(|value| value.to_str()) != Some(AUDIO_NAME)
        {
            return Err(StageError::Manifest);
        }
    }
    for relative in manifest.files {
        match std::fs::remove_file(task_dir.join(relative)) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return Err(StageError::Io),
        }
    }
    std::fs::remove_file(manifest_path).map_err(|_| StageError::Io)?;
    match std::fs::remove_dir(task_dir) {
        Ok(()) => Ok(()),
        Err(_) => {
            let mut entries = std::fs::read_dir(task_dir).map_err(|_| StageError::Io)?;
            if entries.next().is_some() {
                Ok(())
            } else {
                Err(StageError::Io)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn stages_atomically_under_unicode_space_path_and_only_cleans_manifest_files() {
        let root = tempfile::tempdir().unwrap();
        let cache = root.path().join("中文 空格").join("moss tasks");
        let task_id = "798d8c63-5ff1-40e3-9db8-0f706aeb930a";
        let samples: Vec<f32> = (0..48_000)
            .flat_map(|index| {
                let sample = ((index as f32 / 100.0).sin() * 0.25).clamp(-1.0, 1.0);
                [sample, sample]
            })
            .collect();
        let staged = stage_interleaved(&cache, task_id, &samples, 48_000, 2).unwrap();
        assert!(Path::new(&staged.spec.path).exists());
        assert_eq!(staged.spec.sample_rate_hz, 16_000);
        assert_eq!(staged.spec.channels, 1);
        assert_eq!(staged.spec.bytes, staged.spec.samples * 4);
        assert!(moss_helper::protocol::is_sha256(&staged.spec.sha256));
        let staged_duration_ms = (staged.spec.samples * 1_000 + 15_999) / 16_000;
        assert_eq!(staged.activity.audio_duration_ms, staged_duration_ms);
        assert!(staged.activity.audio_duration_ms <= 1_000);
        assert!(
            1_000 - staged.activity.audio_duration_ms <= u64::from(staged.activity.frame_ms),
            "resampling must not shorten the one-second fixture by a full activity frame"
        );
        assert_eq!(staged.activity.first_active_ms, Some(0));
        let last_active_ms = staged
            .activity
            .last_active_ms
            .expect("the staged sine wave must contain active audio");
        assert!(last_active_ms <= staged.activity.audio_duration_ms);
        assert!(
            staged.activity.audio_duration_ms - last_active_ms
                <= u64::from(staged.activity.frame_ms),
            "resampling may add a sub-frame quiet tail, but must not lose a full activity frame"
        );
        assert!(!staged.task_dir.join(format!(".{AUDIO_NAME}.tmp")).exists());

        let unrelated = staged.task_dir.join("keep.txt");
        std::fs::write(&unrelated, b"keep").unwrap();
        staged.cleanup().unwrap();
        assert!(unrelated.exists());
        assert!(!cache.join(task_id).join(AUDIO_NAME).exists());
        assert!(!cache.join(task_id).join(MANIFEST_NAME).exists());
    }

    #[test]
    fn rejects_non_finite_and_misaligned_audio() {
        let root = tempfile::tempdir().unwrap();
        let id = "798d8c63-5ff1-40e3-9db8-0f706aeb930a";
        assert!(stage_interleaved(root.path(), id, &[f32::NAN], 16_000, 1).is_err());
        assert!(stage_interleaved(root.path(), id, &[0.0, 0.1, 0.2], 16_000, 2).is_err());
    }

    #[test]
    fn cancellation_is_checked_in_chunks_and_leaves_no_task_directory() {
        let root = tempfile::tempdir().unwrap();
        let id = "798d8c63-5ff1-40e3-9db8-0f706aeb930a";
        let checks = AtomicUsize::new(0);
        let result =
            stage_interleaved_with_cancel(root.path(), id, &vec![0.1; 160_000], 16_000, 1, || {
                checks.fetch_add(1, Ordering::SeqCst) >= 2
            });
        assert!(matches!(result, Err(StageError::Cancelled)));
        assert!(!root.path().join(id).exists());
    }
}

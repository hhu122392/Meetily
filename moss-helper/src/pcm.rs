use std::fs::{File, OpenOptions};
use std::path::Path;

use memmap2::{Mmap, MmapOptions};
use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::protocol::{AudioSpec, ErrorCode};

pub const REQUIRED_FORMAT: &str = "f32le";
pub const REQUIRED_SAMPLE_RATE_HZ: u32 = 16_000;
pub const REQUIRED_CHANNELS: u16 = 1;
/// R2 single-session boundary for Intel Arc Vulkan. The frozen Vulkan-16K
/// session reports a 1,283.36 second effective limit; 1,200 seconds keeps a
/// deliberate safety margin while covering the product's 10-20 minute gate.
pub const MAX_AUDIO_SECONDS: u64 = 1_200;
pub const MAX_AUDIO_SAMPLES: u64 = REQUIRED_SAMPLE_RATE_HZ as u64 * MAX_AUDIO_SECONDS;
pub const ACTIVITY_FRAME_MS: u32 = 20;
pub const ACTIVITY_THRESHOLD_DBFS: f32 = -50.0;
const ACTIVITY_FRAME_SAMPLES: usize =
    (REQUIRED_SAMPLE_RATE_HZ as usize * ACTIVITY_FRAME_MS as usize) / 1_000;
// -50 dBFS converted to mean-square amplitude: 10 ^ (-50 / 10).
const ACTIVITY_THRESHOLD_MEAN_SQUARE: f64 = 0.000_01;

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PcmActivity {
    pub frame_ms: u32,
    pub threshold_dbfs: f32,
    pub audio_duration_ms: u64,
    pub first_active_ms: Option<u64>,
    pub last_active_ms: Option<u64>,
}

#[derive(Debug)]
pub struct ValidatedPcm {
    // Keep the non-shareable file handle alive for at least as long as the
    // mapping/native call.  The mapping alone is not the ownership contract:
    // on Windows this handle is what prevents replacement or deletion.
    _file: File,
    mmap: Mmap,
    samples: usize,
}

impl ValidatedPcm {
    pub fn samples(&self) -> usize {
        self.samples
    }

    pub fn duration_seconds(&self) -> f64 {
        self.samples as f64 / REQUIRED_SAMPLE_RATE_HZ as f64
    }

    pub fn as_ptr(&self) -> *const f32 {
        self.mmap.as_ptr().cast()
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum PcmError {
    #[error("audio contract is invalid")]
    Contract,
    #[error("audio file is unavailable")]
    Unavailable,
    #[error("audio hash does not match")]
    Hash,
    #[error("audio contains a non-finite sample")]
    NonFinite,
    #[error("audio sample is outside [-1, 1]")]
    OutOfRange,
    #[error("audio mapping failed")]
    Mapping,
}

impl PcmError {
    pub fn code(&self) -> ErrorCode {
        match self {
            Self::Contract | Self::Unavailable | Self::Mapping => ErrorCode::AudioContractMismatch,
            Self::Hash => ErrorCode::AudioHashMismatch,
            Self::NonFinite => ErrorCode::AudioNonFinite,
            Self::OutOfRange => ErrorCode::AudioOutOfRange,
        }
    }
}

pub fn validate_pcm(spec: &AudioSpec) -> Result<ValidatedPcm, PcmError> {
    if spec.format != REQUIRED_FORMAT
        || spec.sample_rate_hz != REQUIRED_SAMPLE_RATE_HZ
        || spec.channels != REQUIRED_CHANNELS
        || spec.samples == 0
        || spec.samples > MAX_AUDIO_SAMPLES
        || spec.samples > i32::MAX as u64
        || !is_sha256(&spec.sha256)
    {
        return Err(PcmError::Contract);
    }
    let expected_bytes = spec.samples.checked_mul(4).ok_or(PcmError::Contract)?;
    if spec.bytes != expected_bytes {
        return Err(PcmError::Contract);
    }

    let path = Path::new(&spec.path);
    if !path.is_absolute() {
        return Err(PcmError::Contract);
    }
    let file = open_read_locked(path).map_err(|_| PcmError::Unavailable)?;
    let metadata = file.metadata().map_err(|_| PcmError::Unavailable)?;
    if !metadata.is_file() || metadata.len() != spec.bytes {
        return Err(PcmError::Contract);
    }

    // SAFETY: the file is opened read-only and held for the lifetime of the map.
    // On Windows open_read_locked also denies concurrent write/delete sharing.
    let mmap = unsafe { MmapOptions::new().map(&file) }.map_err(|_| PcmError::Mapping)?;
    let mut digest = Sha256::new();
    digest.update(&mmap);
    let actual_hash = format!("{:X}", digest.finalize());
    if !actual_hash.eq_ignore_ascii_case(&spec.sha256) {
        return Err(PcmError::Hash);
    }

    for bytes in mmap.chunks_exact(4) {
        let sample = f32::from_le_bytes(bytes.try_into().expect("four-byte chunk"));
        if !sample.is_finite() {
            return Err(PcmError::NonFinite);
        }
        if !(-1.0..=1.0).contains(&sample) {
            return Err(PcmError::OutOfRange);
        }
    }

    Ok(ValidatedPcm {
        _file: file,
        mmap,
        samples: spec.samples as usize,
    })
}

/// Measures activity on the exact 16 kHz mono samples sent to the native
/// runtime.  The result is intentionally frame-level; it is evidence about
/// audible input, not a synthesized word timestamp.
pub fn analyze_activity(samples: &[f32]) -> Result<PcmActivity, PcmError> {
    if samples.is_empty() || samples.len() as u64 > MAX_AUDIO_SAMPLES {
        return Err(PcmError::Contract);
    }
    let audio_duration_ms = samples_to_milliseconds_ceil(samples.len())?;
    let mut first_active_ms = None;
    let mut last_active_ms = None;
    for (frame_index, frame) in samples.chunks(ACTIVITY_FRAME_SAMPLES).enumerate() {
        let mut square_sum = 0.0f64;
        for sample in frame {
            if !sample.is_finite() {
                return Err(PcmError::NonFinite);
            }
            if !(-1.0..=1.0).contains(sample) {
                return Err(PcmError::OutOfRange);
            }
            square_sum += f64::from(*sample) * f64::from(*sample);
        }
        let mean_square = square_sum / frame.len() as f64;
        if mean_square >= ACTIVITY_THRESHOLD_MEAN_SQUARE {
            let frame_start_sample = frame_index
                .checked_mul(ACTIVITY_FRAME_SAMPLES)
                .ok_or(PcmError::Contract)?;
            let frame_end_sample = frame_start_sample
                .checked_add(frame.len())
                .ok_or(PcmError::Contract)?;
            let frame_start_ms = samples_to_milliseconds_floor(frame_start_sample)?;
            let frame_end_ms = samples_to_milliseconds_ceil(frame_end_sample)?;
            first_active_ms.get_or_insert(frame_start_ms);
            last_active_ms = Some(frame_end_ms);
        }
    }
    Ok(PcmActivity {
        frame_ms: ACTIVITY_FRAME_MS,
        threshold_dbfs: ACTIVITY_THRESHOLD_DBFS,
        audio_duration_ms,
        first_active_ms,
        last_active_ms,
    })
}

fn samples_to_milliseconds_floor(samples: usize) -> Result<u64, PcmError> {
    u64::try_from(samples)
        .ok()
        .and_then(|value| value.checked_mul(1_000))
        .map(|value| value / u64::from(REQUIRED_SAMPLE_RATE_HZ))
        .ok_or(PcmError::Contract)
}

fn samples_to_milliseconds_ceil(samples: usize) -> Result<u64, PcmError> {
    u64::try_from(samples)
        .ok()
        .and_then(|value| value.checked_mul(1_000))
        .and_then(|value| value.checked_add(u64::from(REQUIRED_SAMPLE_RATE_HZ) - 1))
        .map(|value| value / u64::from(REQUIRED_SAMPLE_RATE_HZ))
        .ok_or(PcmError::Contract)
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

#[cfg(windows)]
fn open_read_locked(path: &Path) -> std::io::Result<File> {
    use std::os::windows::fs::OpenOptionsExt;

    const FILE_SHARE_READ: u32 = 0x0000_0001;
    OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ)
        .open(path)
}

#[cfg(not(windows))]
fn open_read_locked(path: &Path) -> std::io::Result<File> {
    OpenOptions::new().read(true).open(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::{NamedTempFile, TempPath};

    fn fixture(samples: &[f32]) -> (TempPath, AudioSpec) {
        let mut file = NamedTempFile::new().unwrap();
        let mut bytes = Vec::new();
        for sample in samples {
            bytes.extend_from_slice(&sample.to_le_bytes());
        }
        file.write_all(&bytes).unwrap();
        file.flush().unwrap();
        let hash = format!("{:X}", Sha256::digest(&bytes));
        let path = file.into_temp_path();
        let spec = AudioSpec {
            path: path.to_str().unwrap().to_owned(),
            format: REQUIRED_FORMAT.to_string(),
            sample_rate_hz: REQUIRED_SAMPLE_RATE_HZ,
            channels: REQUIRED_CHANNELS,
            samples: samples.len() as u64,
            bytes: bytes.len() as u64,
            sha256: hash,
        };
        (path, spec)
    }

    #[test]
    fn rejects_relative_path() {
        let (_file, mut spec) = fixture(&[0.0]);
        spec.path = "audio.f32le".to_string();
        assert_eq!(validate_pcm(&spec).unwrap_err(), PcmError::Contract);
    }

    #[test]
    fn validates_well_formed_pcm() {
        let (_file, spec) = fixture(&[-1.0, -0.25, 0.0, 0.5, 1.0]);
        let pcm = validate_pcm(&spec).unwrap();
        assert_eq!(pcm.samples(), 5);
    }

    #[test]
    fn rejects_metadata_mismatch() {
        let (_file, mut spec) = fixture(&[0.0, 0.5]);
        spec.bytes += 4;
        assert_eq!(validate_pcm(&spec).unwrap_err(), PcmError::Contract);
    }

    #[test]
    fn rejects_hash_mismatch() {
        let (_file, mut spec) = fixture(&[0.0, 0.5]);
        spec.sha256 = "0".repeat(64);
        assert_eq!(validate_pcm(&spec).unwrap_err(), PcmError::Hash);
    }

    #[test]
    fn rejects_nan_and_infinity() {
        for sample in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            let (_file, spec) = fixture(&[sample]);
            assert_eq!(validate_pcm(&spec).unwrap_err(), PcmError::NonFinite);
        }
    }

    #[test]
    fn rejects_samples_outside_unit_range() {
        for sample in [-1.0001, 1.0001] {
            let (_file, spec) = fixture(&[sample]);
            assert_eq!(validate_pcm(&spec).unwrap_err(), PcmError::OutOfRange);
        }
    }

    #[test]
    fn rejects_wrong_format_rate_and_channels() {
        let (_file, mut spec) = fixture(&[0.0]);
        spec.format = "s16le".to_string();
        assert_eq!(validate_pcm(&spec).unwrap_err(), PcmError::Contract);
        spec.format = REQUIRED_FORMAT.to_string();
        spec.sample_rate_hz = 48_000;
        assert_eq!(validate_pcm(&spec).unwrap_err(), PcmError::Contract);
        spec.sample_rate_hz = REQUIRED_SAMPLE_RATE_HZ;
        spec.channels = 2;
        assert_eq!(validate_pcm(&spec).unwrap_err(), PcmError::Contract);
    }

    #[test]
    fn rejects_audio_beyond_frozen_chunk_limit() {
        let (_file, mut spec) = fixture(&[0.0]);
        spec.samples = MAX_AUDIO_SAMPLES + 1;
        spec.bytes = spec.samples * 4;
        assert_eq!(validate_pcm(&spec).unwrap_err(), PcmError::Contract);
    }

    #[test]
    fn activity_tracks_first_and_last_active_frames() {
        let mut samples = vec![0.0; ACTIVITY_FRAME_SAMPLES];
        samples.extend(vec![0.01; ACTIVITY_FRAME_SAMPLES]);
        samples.extend(vec![0.0; ACTIVITY_FRAME_SAMPLES]);
        samples.extend(vec![0.02; ACTIVITY_FRAME_SAMPLES]);
        samples.extend(vec![0.0; ACTIVITY_FRAME_SAMPLES]);

        let activity = analyze_activity(&samples).unwrap();
        assert_eq!(activity.audio_duration_ms, 100);
        assert_eq!(activity.first_active_ms, Some(20));
        assert_eq!(activity.last_active_ms, Some(80));
        assert_eq!(activity.frame_ms, 20);
        assert_eq!(activity.threshold_dbfs, -50.0);
    }

    #[test]
    fn activity_reports_all_silence_without_inventing_a_boundary() {
        let activity = analyze_activity(&vec![0.0; ACTIVITY_FRAME_SAMPLES * 2]).unwrap();
        assert_eq!(activity.audio_duration_ms, 40);
        assert_eq!(activity.first_active_ms, None);
        assert_eq!(activity.last_active_ms, None);
    }

    #[test]
    fn activity_includes_a_partial_last_frame() {
        let mut samples = vec![0.0; ACTIVITY_FRAME_SAMPLES];
        samples.extend(vec![0.01; 80]);
        let activity = analyze_activity(&samples).unwrap();
        assert_eq!(activity.audio_duration_ms, 25);
        assert_eq!(activity.first_active_ms, Some(20));
        assert_eq!(activity.last_active_ms, Some(25));
    }

    #[test]
    fn activity_rejects_non_finite_and_out_of_range_samples() {
        assert_eq!(
            analyze_activity(&[f32::NAN]).unwrap_err(),
            PcmError::NonFinite
        );
        assert_eq!(analyze_activity(&[1.01]).unwrap_err(), PcmError::OutOfRange);
    }

    #[cfg(windows)]
    #[test]
    fn validated_pcm_holds_windows_write_delete_and_rename_lock() {
        let (path, spec) = fixture(&[0.0, 0.5]);
        let source = path.to_path_buf();
        let renamed = source.with_extension("renamed");
        let pcm = validate_pcm(&spec).unwrap();

        assert!(OpenOptions::new().write(true).open(&source).is_err());
        assert!(std::fs::remove_file(&source).is_err());
        assert!(std::fs::rename(&source, &renamed).is_err());

        drop(pcm);
        std::fs::rename(&source, &renamed).unwrap();
        std::fs::rename(&renamed, &source).unwrap();
    }
}

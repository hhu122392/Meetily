#![cfg(windows)]
#![allow(dead_code)]

// Compile the exact Tauri MOSS isolation modules in a small test harness.  The
// full Meetily test image has unrelated DirectML loader requirements; this
// harness keeps the manager/Job tests executable without copying the code.
mod audio {
    pub mod audio_processing {
        pub fn resample_audio(input: &[f32], source_rate: u32, target_rate: u32) -> Vec<f32> {
            if input.is_empty() || source_rate == 0 || target_rate == 0 {
                return Vec::new();
            }
            let output_len =
                ((input.len() as u64 * target_rate as u64) / source_rate as u64) as usize;
            (0..output_len)
                .map(|index| {
                    let source = index as u64 * source_rate as u64 / target_rate as u64;
                    input[source.min(input.len().saturating_sub(1) as u64) as usize]
                })
                .collect()
        }
    }
}

#[path = "../../frontend/src-tauri/src/moss_helper/audio_stage.rs"]
mod audio_stage;
#[path = "../../frontend/src-tauri/src/moss_helper/manager.rs"]
mod manager;
#[path = "../../frontend/src-tauri/src/moss_helper/windows_job.rs"]
mod windows_job;

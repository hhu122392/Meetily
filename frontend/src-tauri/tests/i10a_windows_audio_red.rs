#![cfg(target_os = "windows")]

use std::fs;
use std::path::PathBuf;

use app_lib::audio::{AudioDevice, DeviceType};

const DEVICE_CONFIGURATION: &str = include_str!("../src/audio/devices/configuration.rs");
const DEVICE_DISCOVERY: &str = include_str!("../src/audio/devices/discovery.rs");
const WINDOWS_DEVICES: &str = include_str!("../src/audio/devices/platform/windows.rs");
const DEVICE_MONITOR: &str = include_str!("../src/audio/device_monitor.rs");
const RECORDING_COMMANDS: &str = include_str!("../src/audio/recording_commands.rs");
const RECORDING_MANAGER: &str = include_str!("../src/audio/recording_manager.rs");
const RECORDING_STATE: &str = include_str!("../src/audio/recording_state.rs");
const RECORDING_SAVER: &str = include_str!("../src/audio/recording_saver.rs");
const RECORDING_PREFERENCES: &str = include_str!("../src/audio/recording_preferences.rs");
const STREAM: &str = include_str!("../src/audio/stream.rs");
const PIPELINE: &str = include_str!("../src/audio/pipeline.rs");
const WINDOWS_LOOPBACK: &str = include_str!("../src/audio/windows_loopback.rs");

fn optional_source(relative_path: &str) -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(relative_path);
    fs::read_to_string(path).unwrap_or_default()
}

fn windows_audio_sources() -> String {
    [
        DEVICE_CONFIGURATION,
        DEVICE_DISCOVERY,
        WINDOWS_DEVICES,
        DEVICE_MONITOR,
        RECORDING_COMMANDS,
        RECORDING_MANAGER,
        RECORDING_STATE,
        RECORDING_SAVER,
        RECORDING_PREFERENCES,
        STREAM,
        PIPELINE,
        WINDOWS_LOOPBACK,
        &optional_source("src/audio/capture/windows.rs"),
    ]
    .join("\n")
}

fn missing_tokens<'a>(source: &str, required: &'a [&'a str]) -> Vec<&'a str> {
    required
        .iter()
        .copied()
        .filter(|token| !source.contains(token))
        .collect()
}

#[test]
fn output_device_transport_contains_a_stable_endpoint_id() {
    let device = AudioDevice::with_native_id(
        "Duplicate display name".to_owned(),
        DeviceType::Output,
        "{0.0.0.00000000}.stable-endpoint".to_owned(),
    );
    let serialized = serde_json::to_value(device).expect("AudioDevice must serialize");
    let endpoint_id = serialized
        .get("native_id")
        .and_then(serde_json::Value::as_str);

    assert!(
        endpoint_id.is_some_and(|value| !value.trim().is_empty()),
        "Windows output-device identity is still only name + type; serialized={serialized}"
    );
}

#[test]
fn system_loopback_uses_native_wasapi_objects_and_opens_the_selected_id() {
    let source = windows_audio_sources();
    let missing = missing_tokens(
        &source,
        &[
            "IMMDeviceEnumerator",
            "IMMDevice",
            "GetId",
            "GetDevice",
            "IAudioClient",
            "IAudioCaptureClient",
            "AUDCLNT_STREAMFLAGS_LOOPBACK",
        ],
    );

    assert!(
        missing.is_empty(),
        "Windows system capture is not an endpoint-ID-bound native WASAPI backend; missing {missing:?}"
    );
}

#[test]
fn idle_output_endpoint_is_ready_after_wasapi_start_without_waiting_for_audio() {
    let start = WINDOWS_LOOPBACK
        .find(".Start()")
        .expect("native WASAPI start call is missing");
    let capture = WINDOWS_LOOPBACK[start..]
        .find("capture_loop(")
        .map(|offset| offset + start)
        .expect("native WASAPI capture loop call is missing");
    let ready = WINDOWS_LOOPBACK[start..]
        .find("sender.send(Ok(()))")
        .map(|offset| offset + start)
        .expect("native WASAPI startup acknowledgement is missing");

    assert!(
        start < ready && ready < capture,
        "an initialized idle render endpoint must be acknowledged before waiting for audio"
    );
    assert!(
        !WINDOWS_LOOPBACK.contains("produced no callback within 2 seconds")
            && !WINDOWS_LOOPBACK.contains("first callback took at least 2 seconds"),
        "an idle render endpoint must not be treated as a failed system-audio startup"
    );
}

#[test]
fn output_selection_and_monitoring_do_not_match_or_deduplicate_by_display_name() {
    let forbidden = [
        (
            "partial output-name match",
            WINDOWS_DEVICES.contains("name.contains(base_name)"),
        ),
        (
            "missing selected output falls back to default output",
            WINDOWS_DEVICES
                .contains("No matching output device found, trying default output device"),
        ),
        (
            "device monitor checks identity by display name",
            DEVICE_MONITOR.contains("d.name == monitored.name"),
        ),
    ]
    .into_iter()
    .filter_map(|(description, present)| present.then_some(description))
    .collect::<Vec<_>>();

    assert!(
        forbidden.is_empty(),
        "display names still control Windows endpoint identity: {forbidden:?}"
    );
    assert!(
        DEVICE_DISCOVERY.contains("device.native_id.as_deref() == Some(selection)")
            && DEVICE_DISCOVERY.contains("matching.as_slice()"),
        "native endpoint IDs must be authoritative and legacy names must be ambiguity checked"
    );
}

#[test]
fn modes_that_require_a_stream_fail_closed_instead_of_silently_downgrading() {
    let source = windows_audio_sources();
    let missing = missing_tokens(
        &source,
        &[
            "RecordingMode",
            "MicrophoneAndSystem",
            "MicrophoneOnly",
            "SystemOnly",
        ],
    );
    let forbidden = [
        "Don't fail if only system audio fails",
        "Recording will continue with microphone only",
        "Non-fatal - continue without monitoring",
    ]
    .into_iter()
    .filter(|token| source.contains(token))
    .collect::<Vec<_>>();

    assert!(
        missing.is_empty() && forbidden.is_empty(),
        "recording modes do not fail closed; missing={missing:?}, forbidden={forbidden:?}"
    );
}

#[test]
fn system_callback_health_uses_start_and_capture_qpc_with_the_strict_two_second_boundary() {
    let source = windows_audio_sources();
    let missing = missing_tokens(
        &source,
        &[
            "system_stream_started_qpc_ns",
            "capture_qpc_ns",
            "AUDIO_CALLBACK_DEADLINE_NS",
            "2_000_000_000",
            "report_system_no_signal",
            "all_zero_frame_count",
            "frames as u64",
        ],
    );

    assert!(
        missing.is_empty(),
        "QPC health evidence or the all-zero/no-callback split is absent; missing {missing:?}"
    );
}

#[test]
fn endpoint_cutover_has_epoch_watermark_bounded_drain_and_per_epoch_format() {
    let source = windows_audio_sources();
    let missing = missing_tokens(
        &source,
        &[
            "device_epoch",
            "cutover_watermark_qpc_ns",
            "in_flight_callback_count",
            "callback_drain_deadline_qpc_ns",
            "late_callback_dropped_frames",
            "sample_rate",
            "channels",
            "sample_format",
            "48_000",
        ],
    );

    assert!(
        missing.is_empty(),
        "device cutover cannot yet prove ownership, bounded drain, or per-epoch conversion; missing {missing:?}"
    );
}

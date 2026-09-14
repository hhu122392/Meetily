const LIB_RS: &str = include_str!("../src/lib.rs");
const RECORDING_COMMANDS_RS: &str = include_str!("../src/audio/recording_commands.rs");
const IMPORT_RS: &str = include_str!("../src/audio/import.rs");

fn window_after<'a>(source: &'a str, marker: &str, length: usize) -> &'a str {
    let start = source
        .find(marker)
        .unwrap_or_else(|| panic!("missing source marker: {marker}"));
    let end = (start + length).min(source.len());
    &source[start..end]
}

#[test]
fn recording_command_boundary_accepts_mode_and_stable_device_identity() {
    let tauri_command = window_after(
        LIB_RS,
        "async fn start_recording_with_devices_and_meeting",
        1_600,
    );
    assert!(tauri_command.contains("recording_mode"));
    assert!(tauri_command.contains("mic_device_name"));
    assert!(tauri_command.contains("system_device_name"));

    let integration_command = window_after(
        RECORDING_COMMANDS_RS,
        "pub async fn start_recording_with_devices_meeting_and_metadata",
        1_600,
    );
    assert!(integration_command.contains("recording_mode"));
    assert!(integration_command.contains("mic_device_name"));
    assert!(integration_command.contains("system_device_name"));
    assert!(RECORDING_COMMANDS_RS.contains("resolve_audio_device"));
}

#[test]
fn import_command_rejects_an_active_recording_before_spawning_work() {
    let command = window_after(IMPORT_RS, "pub async fn start_import_audio_command", 1_400);
    let recording_check = command
        .find("recording_commands::is_recording().await")
        .expect("the command boundary must query the authoritative recording state");
    let spawn = command
        .find("tauri::async_runtime::spawn")
        .expect("the import background spawn was not found");
    assert!(
        recording_check < spawn,
        "recording must be rejected before a second transcription task can be spawned"
    );
}

#[test]
fn recording_and_import_have_an_atomic_mutual_exclusion_gate() {
    assert!(
        IMPORT_RS.contains("AUDIO_ACTIVITY_IMPORT")
            && IMPORT_RS.contains("AUDIO_ACTIVITY_RECORDING")
            && IMPORT_RS.contains("compare_exchange"),
        "recording and import need distinct states in one atomic activity gate"
    );
    assert!(
        IMPORT_RS.contains("pub struct RecordingActivityGuard")
            && RECORDING_COMMANDS_RS.contains("RecordingActivityGuard::acquire()"),
        "recording and audio import must enter through the same admission gate"
    );
    assert!(
        IMPORT_RS.contains("fn recording_and_import_are_mutually_exclusive"),
        "the operation gate needs a behavioral unit test for both conflict directions"
    );

    let command = window_after(IMPORT_RS, "pub async fn start_import_audio_command", 1_400);
    let admission = command
        .find("ImportGuard::acquire()")
        .expect("audio import must reserve the shared gate synchronously");
    let spawn = command
        .find("tauri::async_runtime::spawn")
        .expect("the import background spawn was not found");
    assert!(
        admission < spawn,
        "the import gate must be reserved before acknowledging or spawning work"
    );
}

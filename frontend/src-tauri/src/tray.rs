use tauri::{
    menu::{MenuBuilder, MenuItemBuilder, PredefinedMenuItem},
    tray::TrayIconBuilder,
    AppHandle, Emitter, Manager, Runtime,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecordingState {
    Stopped,
    Starting,
    Recording,
    Pausing,
    Paused,
    Resuming,
    Stopping,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct TrayItemSpec {
    id: &'static str,
    key: &'static str,
    enabled: bool,
}

const FIXED_TRAY_ITEMS: &[TrayItemSpec] = &[
    TrayItemSpec {
        id: "open_window",
        key: "tray.openMainWindow",
        enabled: true,
    },
    TrayItemSpec {
        id: "settings",
        key: "tray.settings",
        enabled: true,
    },
    TrayItemSpec {
        id: "check_updates",
        key: "tray.checkForUpdates",
        enabled: true,
    },
    TrayItemSpec {
        id: "quit",
        key: "tray.quit",
        enabled: true,
    },
];

pub fn create_tray<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<()> {
    // Start with default menu, will update with actual state after initialization
    // Pass can_record=true initially, will be updated by update_tray_menu immediately
    let menu = build_menu(app, RecordingState::Stopped, true)?;

    TrayIconBuilder::with_id("main-tray")
        .menu(&menu)
        .tooltip("Meetily")
        .icon(app.default_window_icon().unwrap().clone())
        .on_menu_event(|app, event| handle_menu_event(app, event.id.as_ref()))
        .build(app)?;

    // Update tray menu with actual recording state after creation
    update_tray_menu(app);

    Ok(())
}

fn handle_menu_event<R: Runtime>(app: &AppHandle<R>, item_id: &str) {
    match item_id {
        "toggle_recording" => toggle_recording_handler(app),
        "pause_recording" => pause_recording_handler(app),
        "resume_recording" => resume_recording_handler(app),
        "stop_recording" => stop_recording_handler(app),
        "open_window" => focus_main_window(app),
        "settings" => {
            focus_main_window(app);
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.eval("window.location.assign('/settings')");
            }
        }
        "check_updates" => check_updates_handler(app),
        "quit" => app.exit(0),
        _ => {}
    }
}
fn toggle_recording_handler<R: Runtime>(app: &AppHandle<R>) {
    focus_main_window(app);
    let app_clone = app.clone();
    tauri::async_runtime::spawn(async move {
        if crate::is_recording().await {
            // Immediately show stopping state
            set_tray_state(&app_clone, RecordingState::Stopping);

            log::info!("Tray toggle: Stopping recording...");

            // The recording root was frozen when the session started.
            let stop_result =
                crate::audio::recording_commands::stop_recording(app_clone.clone()).await;

            // Handle result
            match stop_result {
                Ok(_) => {
                    log::info!("Tray toggle: Recording stopped successfully");

                    // Trigger frontend post-processing via event (works from any page)
                    // (SQLite save, navigation, analytics)
                    if let Err(e) = app_clone.emit("recording-stop-complete", true) {
                        log::error!(
                            "Tray toggle: Failed to emit recording-stop-complete event: {}",
                            e
                        );
                    }
                }
                Err(e) => {
                    log::error!("Tray toggle: Failed to stop recording: {}", e);
                    // Revert tray state on error
                    update_tray_menu_async(&app_clone).await;
                }
            }
        } else {
            // Immediately show starting state
            set_tray_state(&app_clone, RecordingState::Starting);

            log::info!("Emitting start recording event from tray");
            if let Some(window) = app_clone.get_webview_window("main") {
                let _ = window
                    .eval("window.dispatchEvent(new CustomEvent('request-recording-from-tray'))");
            }
        }
    });
}

fn pause_recording_handler<R: Runtime>(app: &AppHandle<R>) {
    // Immediately show pausing state
    set_tray_state(app, RecordingState::Pausing);

    let app_clone = app.clone();
    tauri::async_runtime::spawn(async move {
        if let Err(e) = crate::audio::recording_commands::pause_recording(app_clone.clone()).await {
            log::error!("Failed to pause recording from tray: {}", e);
            // Revert to current state on error
            update_tray_menu_async(&app_clone).await;
        } else {
            log::info!("Recording paused from tray");
            // The pause_recording function will call update_tray_menu, so no need to call it here
        }
    });
}

fn resume_recording_handler<R: Runtime>(app: &AppHandle<R>) {
    // Immediately show resuming state
    set_tray_state(app, RecordingState::Resuming);

    let app_clone = app.clone();
    tauri::async_runtime::spawn(async move {
        if let Err(e) = crate::audio::recording_commands::resume_recording(app_clone.clone()).await
        {
            log::error!("Failed to resume recording from tray: {}", e);
            // Revert to current state on error
            update_tray_menu_async(&app_clone).await;
        } else {
            log::info!("Recording resumed from tray");
            // The resume_recording function will call update_tray_menu, so no need to call it here
        }
    });
}

fn stop_recording_handler<R: Runtime>(app: &AppHandle<R>) {
    // Immediately show stopping state
    set_tray_state(app, RecordingState::Stopping);

    focus_main_window(app);
    let app_clone = app.clone();
    tauri::async_runtime::spawn(async move {
        log::info!("Tray: Stopping recording...");

        // The recording root was frozen when the session started.
        let stop_result = crate::audio::recording_commands::stop_recording(app_clone.clone()).await;

        // Handle result
        match stop_result {
            Ok(_) => {
                log::info!("Tray: Recording stopped successfully");

                // Trigger frontend post-processing via event (works from any page)
                // (SQLite save, navigation, analytics)
                if let Err(e) = app_clone.emit("recording-stop-complete", true) {
                    log::error!("Tray: Failed to emit recording-stop-complete event: {}", e);
                }
            }
            Err(e) => {
                log::error!("Tray: Failed to stop recording: {}", e);
                // Revert tray state on error
                update_tray_menu_async(&app_clone).await;
            }
        }
    });
}

fn check_updates_handler<R: Runtime>(app: &AppHandle<R>) {
    focus_main_window(app);
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.eval("window.dispatchEvent(new CustomEvent('check-updates-from-tray'))");
    }
}

pub fn update_tray_menu<R: Runtime>(app: &AppHandle<R>) {
    // For sync update, spawn async task to get current state
    let app_clone = app.clone();
    tauri::async_runtime::spawn(async move {
        // Small delay to ensure recording state has been updated
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
        update_tray_menu_async(&app_clone).await;
    });
}

/// Rebuild the existing tray immediately after a native locale change.
/// Menu IDs remain stable, so changing labels cannot change command routing.
pub fn refresh_tray_for_locale<R: Runtime>(app: &AppHandle<R>) {
    let app_clone = app.clone();
    tauri::async_runtime::spawn(async move {
        update_tray_menu_async(&app_clone).await;
    });
}

pub fn set_tray_state<R: Runtime>(app: &AppHandle<R>, state: RecordingState) {
    log::info!("Tray: Setting intermediate state: {:?}", state);
    // During recording state transitions, we assume recording is allowed (we're already recording)
    if let Ok(menu) = build_menu(app, state, true) {
        if let Some(tray) = app.tray_by_id("main-tray") {
            let result = tray.set_menu(Some(menu));
            log::info!("Tray: Intermediate state menu update result: {:?}", result);
        } else {
            log::warn!("Tray: Could not find tray with id 'main-tray'");
        }
    } else {
        log::error!("Tray: Failed to build menu for intermediate state");
    }
}

async fn get_current_recording_state() -> RecordingState {
    // Check if currently recording
    let is_recording = crate::audio::recording_commands::is_recording().await;
    log::info!(
        "Tray: get_current_recording_state - is_recording: {}",
        is_recording
    );

    if !is_recording {
        log::info!("Tray: Recording state is Stopped");
        return RecordingState::Stopped;
    }

    // Check if paused
    let is_paused = crate::audio::recording_commands::is_recording_paused().await;
    log::info!("Tray: is_paused: {}", is_paused);

    if is_paused {
        log::info!("Tray: Recording state is Paused");
        RecordingState::Paused
    } else {
        log::info!("Tray: Recording state is Recording");
        RecordingState::Recording
    }
}

/// Check if recording is allowed based on onboarding status and transcription model availability
/// Returns true if:
/// - Onboarding is complete (user may prefer Whisper later), OR
/// - Parakeet transcription model is ready (downloaded)
async fn check_can_record<R: Runtime>(app: &AppHandle<R>) -> bool {
    // First check if onboarding is complete
    let onboarding_complete = match crate::onboarding::load_onboarding_status(app).await {
        Ok(status) => status.completed,
        Err(e) => {
            log::warn!(
                "Tray: Failed to load onboarding status: {}, assuming complete",
                e
            );
            true // Assume complete if we can't check (safe default)
        }
    };

    // If onboarding is complete, always allow recording
    // (user may prefer Whisper or have their own transcription setup)
    if onboarding_complete {
        return true;
    }

    // During onboarding, check if Parakeet transcription model is ready
    match crate::parakeet_engine::commands::parakeet_has_available_models().await {
        Ok(has_models) => has_models,
        Err(e) => {
            log::warn!(
                "Tray: Failed to check Parakeet models: {}, assuming not ready",
                e
            );
            false
        }
    }
}

pub async fn update_tray_menu_async<R: Runtime>(app: &AppHandle<R>) {
    log::info!("Tray: update_tray_menu_async called");
    // Get the current recording state
    let recording_state = get_current_recording_state().await;
    log::info!("Tray: Current recording state: {:?}", recording_state);

    // Determine if recording should be allowed
    // Only block recording during incomplete onboarding when no transcription model is ready
    let can_record = check_can_record(app).await;
    log::info!("Tray: can_record: {}", can_record);

    if let Ok(menu) = build_menu(app, recording_state, can_record) {
        if let Some(tray) = app.tray_by_id("main-tray") {
            let result = tray.set_menu(Some(menu));
            log::info!("Tray: Menu update result: {:?}", result);
        } else {
            log::warn!("Tray: Could not find tray with id 'main-tray'");
        }
    } else {
        log::error!("Tray: Failed to build menu");
    }
}

fn build_menu<R: Runtime>(
    app: &AppHandle<R>,
    state: RecordingState,
    can_record: bool, // True if recording is allowed (onboarding complete OR transcription model ready)
) -> tauri::Result<tauri::menu::Menu<R>> {
    let mut builder = MenuBuilder::new(app);

    let text = |key: &str| crate::i18n::translate_for_app(app, key, &[]);

    for item in state_items(state, can_record) {
        builder = builder.item(
            &MenuItemBuilder::with_id(item.id, text(item.key))
                .enabled(item.enabled)
                .build(app)?,
        );
    }

    builder = builder.item(&PredefinedMenuItem::separator(app)?);
    for (index, item) in FIXED_TRAY_ITEMS.iter().enumerate() {
        if index == FIXED_TRAY_ITEMS.len() - 1 {
            builder = builder.item(&PredefinedMenuItem::separator(app)?);
        }
        builder = builder.item(
            &MenuItemBuilder::with_id(item.id, text(item.key))
                .enabled(item.enabled)
                .build(app)?,
        );
    }
    builder.build()
}

fn state_items(state: RecordingState, can_record: bool) -> Vec<TrayItemSpec> {
    if !can_record {
        return vec![TrayItemSpec {
            id: "status_downloading_model",
            key: "tray.downloadingTranscriptionModel",
            enabled: false,
        }];
    }

    match state {
        RecordingState::Stopped => vec![TrayItemSpec {
            id: "toggle_recording",
            key: "tray.startRecording",
            enabled: true,
        }],
        RecordingState::Starting => vec![TrayItemSpec {
            id: "status_starting",
            key: "tray.startingRecording",
            enabled: false,
        }],
        RecordingState::Recording => vec![
            TrayItemSpec {
                id: "pause_recording",
                key: "tray.pauseRecording",
                enabled: true,
            },
            TrayItemSpec {
                id: "stop_recording",
                key: "tray.stopRecording",
                enabled: true,
            },
        ],
        RecordingState::Pausing => vec![
            TrayItemSpec {
                id: "status_pausing",
                key: "tray.pausing",
                enabled: false,
            },
            TrayItemSpec {
                id: "stop_recording",
                key: "tray.stopRecording",
                enabled: true,
            },
        ],
        RecordingState::Paused => vec![
            TrayItemSpec {
                id: "resume_recording",
                key: "tray.resumeRecording",
                enabled: true,
            },
            TrayItemSpec {
                id: "stop_recording",
                key: "tray.stopRecording",
                enabled: true,
            },
        ],
        RecordingState::Resuming => vec![
            TrayItemSpec {
                id: "status_resuming",
                key: "tray.resuming",
                enabled: false,
            },
            TrayItemSpec {
                id: "stop_recording",
                key: "tray.stopRecording",
                enabled: true,
            },
        ],
        RecordingState::Stopping => vec![TrayItemSpec {
            id: "status_stopping",
            key: "tray.stopping",
            enabled: false,
        }],
    }
}

pub(crate) fn focus_main_window<R: Runtime>(app: &AppHandle<R>) {
    if let Some(window) = app.get_webview_window("main") {
        if let Err(e) = window.unminimize() {
            log::error!("Failed to unminimize main window: {}", e);
        }

        if let Err(e) = window.show() {
            log::error!("Failed to show main window: {}", e);
        }

        if let Err(e) = window.set_focus() {
            log::error!("Failed to focus main window: {}", e);
        }

        if let Err(e) = window.eval("window.focus()") {
            log::error!("Failed to focus main webview: {}", e);
        }
    } else {
        log::warn!("Could not find main window");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::i18n::{translate, SupportedUiLocale};

    #[test]
    fn every_recording_state_has_a_complete_bilingual_menu_spec() {
        let states = [
            RecordingState::Stopped,
            RecordingState::Starting,
            RecordingState::Recording,
            RecordingState::Pausing,
            RecordingState::Paused,
            RecordingState::Resuming,
            RecordingState::Stopping,
        ];

        for state in states {
            let items = state_items(state, true);
            assert!(!items.is_empty(), "missing tray state: {state:?}");
            for item in items {
                let english = translate(SupportedUiLocale::En, item.key, &[]);
                let chinese = translate(SupportedUiLocale::ZhCn, item.key, &[]);
                assert_ne!(english, item.key, "missing English key: {}", item.key);
                assert_ne!(chinese, item.key, "missing Chinese key: {}", item.key);
                assert_ne!(english, chinese, "untranslated tray key: {}", item.key);
            }
        }
    }

    #[test]
    fn stable_actions_exist_in_every_applicable_state() {
        assert_eq!(
            state_items(RecordingState::Stopped, true)[0].id,
            "toggle_recording"
        );
        assert!(state_items(RecordingState::Recording, true)
            .iter()
            .any(|item| item.id == "stop_recording"));
        assert!(state_items(RecordingState::Pausing, true)
            .iter()
            .any(|item| item.id == "stop_recording"));
        assert!(state_items(RecordingState::Paused, true)
            .iter()
            .any(|item| item.id == "resume_recording"));
        assert!(state_items(RecordingState::Resuming, true)
            .iter()
            .any(|item| item.id == "stop_recording"));
        assert_eq!(
            FIXED_TRAY_ITEMS
                .iter()
                .map(|item| item.id)
                .collect::<Vec<_>>(),
            vec!["open_window", "settings", "check_updates", "quit"]
        );
    }

    #[test]
    fn downloading_state_disables_recording_without_removing_fixed_actions() {
        let items = state_items(RecordingState::Stopped, false);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].id, "status_downloading_model");
        assert!(!items[0].enabled);
        assert!(FIXED_TRAY_ITEMS.iter().all(|item| item.enabled));
    }
}

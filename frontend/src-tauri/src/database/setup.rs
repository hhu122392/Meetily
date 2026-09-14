use log::{info, warn};
use tauri::{AppHandle, Emitter, Manager};

use super::manager::DatabaseManager;
use super::moss::MossCandidateRepository;
use super::repositories::summary::SummaryProcessesRepository;
use crate::state::AppState;

/// Initialize database on app startup
/// Handles first launch detection and conditional initialization
pub async fn initialize_database_on_startup(app: &AppHandle) -> Result<(), String> {
    // Check if this is the first launch (no database exists yet)
    let is_first_launch = DatabaseManager::is_first_launch(app)
        .await
        .map_err(|e| format!("Failed to check first launch status: {}", e))?;

    if is_first_launch {
        info!("First launch detected - will notify window when ready");

        // Delay event emission to ensure window is ready and React listeners are registered
        let app_handle = app.clone();
        tauri::async_runtime::spawn(async move {
            tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
            app_handle
                .emit("first-launch-detected", ())
                .expect("Failed to emit first-launch-detected event");
            info!("Emitted first-launch-detected after delay");
        });
    } else {
        // Normal flow - initialize database immediately
        let db_manager = DatabaseManager::new_from_app_handle(app)
            .await
            .map_err(|e| format!("Failed to initialize database manager: {}", e))?;

        let recovery = SummaryProcessesRepository::recover_orphaned_generations_from_previous_run(
            db_manager.pool(),
        )
        .await
        .map_err(|e| format!("Failed to recover interrupted summary generations: {}", e))?;
        if recovery.processes_failed > 0 || recovery.histories_failed > 0 {
            warn!(
                "Recovered interrupted summary generations from the previous run: processes={}, histories={}",
                recovery.processes_failed, recovery.histories_failed
            );
        } else {
            info!("No interrupted summary generations required startup recovery");
        }

        let moss_recovery = MossCandidateRepository::recover_interrupted_runs(db_manager.pool())
            .await
            .map_err(|e| format!("Failed to recover interrupted MOSS runs: {}", e))?;
        if moss_recovery.runs_failed > 0 {
            warn!(
                "Recovered interrupted MOSS runs from the previous application session: runs={}",
                moss_recovery.runs_failed
            );
        } else {
            info!("No interrupted MOSS runs required startup recovery");
        }

        if let Err(error) =
            crate::audio::import::recover_incomplete_imports_on_startup(app, db_manager.pool())
                .await
        {
            warn!("Failed to recover interrupted audio imports: {}", error);
        }

        app.manage(AppState { db_manager });
        info!("Database initialized successfully");
    }

    Ok(())
}

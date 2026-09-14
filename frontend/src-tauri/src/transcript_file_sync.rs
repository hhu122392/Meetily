use crate::{state::AppState, transcript_file_store};
use tauri::State;

#[tauri::command]
pub async fn api_transcript_file_sync_pending(app_state: State<'_, AppState>, meeting_id: String) -> Result<bool, String> {
    transcript_file_store::pending(app_state.db_manager.pool(), &meeting_id).await.map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn api_retry_transcript_file_sync(app_state: State<'_, AppState>, meeting_id: String) -> Result<(), String> {
    let _guard = transcript_file_store::WRITE_LOCK.lock().await;
    transcript_file_store::ensure_ready(app_state.db_manager.pool(), &meeting_id).await
}

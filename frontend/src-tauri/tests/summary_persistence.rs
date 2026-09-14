// Run the production SQLite repository and its tests without loading unrelated
// native audio/model DLLs.
mod database {
    pub use app_lib::database::{manager, models};
}
#[allow(dead_code)]
#[path = "../src/database/repositories/summary.rs"]
mod summary_repository;

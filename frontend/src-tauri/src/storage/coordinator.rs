use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use thiserror::Error;

use super::migration::{
    MigrationEngine, MigrationError, MigrationProgressCheckpoint, MigrationState,
};
use super::operation_lock::{
    global_storage_operations, StorageOperationError, StorageOperationGuard,
    StorageOperationRegistry,
};

#[derive(Debug, Error)]
pub enum MigrationCoordinatorError {
    #[error("Formal storage migration is not authorized in this release stage")]
    FormalMigrationNotAuthorized,

    #[error("A storage migration task is already running: {0}")]
    TaskAlreadyRunning(String),

    #[error("No storage migration task is running")]
    TaskNotRunning,

    #[error(transparent)]
    OperationBusy(#[from] StorageOperationError),

    #[error(transparent)]
    Migration(#[from] MigrationError),
}

impl MigrationCoordinatorError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::FormalMigrationNotAuthorized => "formal_migration_not_authorized",
            Self::TaskAlreadyRunning(_) => "migration_task_already_running",
            Self::TaskNotRunning => "migration_not_running",
            Self::OperationBusy(_) => "operation_busy",
            Self::Migration(error) => error.code(),
        }
    }
}

#[derive(Debug, Clone)]
struct ActiveMigrationTask {
    migration_id: String,
    cancelled: Arc<AtomicBool>,
}

#[derive(Debug)]
struct MigrationCoordinatorInner {
    active: Mutex<Option<ActiveMigrationTask>>,
    operations: StorageOperationRegistry,
}

#[derive(Debug, Clone)]
pub struct MigrationCoordinator {
    inner: Arc<MigrationCoordinatorInner>,
    formal_authorized: bool,
}

impl MigrationCoordinator {
    pub fn production() -> Self {
        Self::new(false, global_storage_operations())
    }

    fn new(formal_authorized: bool, operations: StorageOperationRegistry) -> Self {
        Self {
            inner: Arc::new(MigrationCoordinatorInner {
                active: Mutex::new(None),
                operations,
            }),
            formal_authorized,
        }
    }

    #[cfg(test)]
    fn authorized_for_tests(operations: StorageOperationRegistry) -> Self {
        Self::new(true, operations)
    }

    pub fn formal_authorized(&self) -> bool {
        self.formal_authorized
    }

    pub fn ensure_formal_authorized(&self) -> Result<(), MigrationCoordinatorError> {
        if self.formal_authorized {
            Ok(())
        } else {
            Err(MigrationCoordinatorError::FormalMigrationNotAuthorized)
        }
    }

    pub fn active_task_id(&self) -> Option<String> {
        self.inner
            .active
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .as_ref()
            .map(|task| task.migration_id.clone())
    }

    pub fn request_cancel(&self) -> Result<String, MigrationCoordinatorError> {
        let active = self
            .inner
            .active
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let task = active
            .as_ref()
            .ok_or(MigrationCoordinatorError::TaskNotRunning)?;
        task.cancelled.store(true, Ordering::SeqCst);
        Ok(task.migration_id.clone())
    }

    pub fn run_engine<F>(
        &self,
        engine: &MigrationEngine,
        notify: F,
    ) -> Result<MigrationState, MigrationCoordinatorError>
    where
        F: FnMut(MigrationProgressCheckpoint),
    {
        let lease = self.reserve(engine.migration_id())?;
        let result = engine.run_with_cancellation_and_notifier(&lease.cancelled, notify);
        drop(lease);
        result.map_err(MigrationCoordinatorError::from)
    }

    fn reserve(&self, migration_id: &str) -> Result<MigrationTaskLease, MigrationCoordinatorError> {
        self.ensure_formal_authorized()?;
        let operation_guard = self.inner.operations.begin_migration()?;
        let cancelled = Arc::new(AtomicBool::new(false));

        let mut active = self
            .inner
            .active
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(task) = active.as_ref() {
            return Err(MigrationCoordinatorError::TaskAlreadyRunning(
                task.migration_id.clone(),
            ));
        }
        *active = Some(ActiveMigrationTask {
            migration_id: migration_id.to_owned(),
            cancelled: cancelled.clone(),
        });

        Ok(MigrationTaskLease {
            inner: self.inner.clone(),
            migration_id: migration_id.to_owned(),
            cancelled,
            _operation_guard: operation_guard,
        })
    }
}

#[derive(Debug)]
struct MigrationTaskLease {
    inner: Arc<MigrationCoordinatorInner>,
    migration_id: String,
    cancelled: Arc<AtomicBool>,
    _operation_guard: StorageOperationGuard,
}

impl Drop for MigrationTaskLease {
    fn drop(&mut self) {
        let mut active = self
            .inner
            .active
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if active.as_ref().map(|task| task.migration_id.as_str())
            == Some(self.migration_id.as_str())
        {
            *active = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::operation_lock::StorageOperationKind;

    #[test]
    fn production_coordinator_has_a_hard_formal_gate() {
        let coordinator = MigrationCoordinator::production();
        let error = coordinator.ensure_formal_authorized().unwrap_err();
        assert_eq!(error.code(), "formal_migration_not_authorized");
        assert!(coordinator.active_task_id().is_none());
    }

    #[test]
    fn one_task_can_be_cancelled_and_cleanup_releases_every_lock() {
        let operations = StorageOperationRegistry::default();
        let coordinator = MigrationCoordinator::authorized_for_tests(operations.clone());
        let lease = coordinator.reserve("s3-test").unwrap();

        assert_eq!(coordinator.active_task_id().as_deref(), Some("s3-test"));
        assert!(operations.snapshot().migration_active);
        let duplicate = coordinator.reserve("s3-duplicate").unwrap_err();
        assert_eq!(duplicate.code(), "operation_busy");

        assert_eq!(coordinator.request_cancel().unwrap(), "s3-test");
        assert!(lease.cancelled.load(Ordering::SeqCst));
        drop(lease);

        assert!(coordinator.active_task_id().is_none());
        assert!(!operations.snapshot().busy);
        assert_eq!(
            coordinator.request_cancel().unwrap_err().code(),
            "migration_not_running"
        );
    }

    #[test]
    fn active_model_operation_blocks_task_reservation() {
        let operations = StorageOperationRegistry::default();
        let _download = operations
            .begin(StorageOperationKind::ModelDownload)
            .unwrap();
        let coordinator = MigrationCoordinator::authorized_for_tests(operations);
        let error = coordinator.reserve("blocked").unwrap_err();
        assert_eq!(error.code(), "operation_busy");
    }
}

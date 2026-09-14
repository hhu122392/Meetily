use std::collections::HashMap;
use std::fmt;
use std::sync::{Arc, LazyLock, Mutex};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StorageOperationKind {
    Recording,
    AudioSaving,
    LiveTranscription,
    Retranscription,
    SummaryGeneration,
    MossEnhancement,
    ModelDownload,
    ModelLoad,
    ModelMutation,
    Migration,
}

impl StorageOperationKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Recording => "recording",
            Self::AudioSaving => "audio_saving",
            Self::LiveTranscription => "live_transcription",
            Self::Retranscription => "retranscription",
            Self::SummaryGeneration => "summary_generation",
            Self::MossEnhancement => "moss_enhancement",
            Self::ModelDownload => "model_download",
            Self::ModelLoad => "model_load",
            Self::ModelMutation => "model_mutation",
            Self::Migration => "migration",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActiveStorageOperation {
    pub operation: StorageOperationKind,
    pub count: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StorageOperationSnapshot {
    pub busy: bool,
    pub migration_active: bool,
    pub active_operations: Vec<ActiveStorageOperation>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StorageOperationError {
    pub code: &'static str,
    pub blockers: Vec<StorageOperationKind>,
}

impl StorageOperationError {
    fn busy(mut blockers: Vec<StorageOperationKind>) -> Self {
        blockers.sort_by_key(|operation| operation.as_str());
        blockers.dedup();
        Self {
            code: "operation_busy",
            blockers,
        }
    }
}

impl fmt::Display for StorageOperationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let blockers = self
            .blockers
            .iter()
            .map(|operation| operation.as_str())
            .collect::<Vec<_>>()
            .join(",");
        write!(formatter, "{}:{blockers}", self.code)
    }
}

impl std::error::Error for StorageOperationError {}

#[derive(Debug, Default)]
struct RegistryState {
    counts: HashMap<StorageOperationKind, u32>,
}

#[derive(Debug, Clone, Default)]
pub struct StorageOperationRegistry {
    state: Arc<Mutex<RegistryState>>,
}

impl StorageOperationRegistry {
    pub fn begin(
        &self,
        operation: StorageOperationKind,
    ) -> Result<StorageOperationGuard, StorageOperationError> {
        if operation == StorageOperationKind::Migration {
            return self.begin_migration();
        }

        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if state
            .counts
            .get(&StorageOperationKind::Migration)
            .copied()
            .unwrap_or_default()
            > 0
        {
            return Err(StorageOperationError::busy(vec![
                StorageOperationKind::Migration,
            ]));
        }

        let mutually_exclusive = match operation {
            StorageOperationKind::MossEnhancement => Some(StorageOperationKind::SummaryGeneration),
            StorageOperationKind::SummaryGeneration => Some(StorageOperationKind::MossEnhancement),
            _ => None,
        };
        if let Some(blocker) = mutually_exclusive {
            if state.counts.get(&blocker).copied().unwrap_or_default() > 0 {
                return Err(StorageOperationError::busy(vec![blocker]));
            }
        }

        *state.counts.entry(operation).or_default() += 1;
        Ok(StorageOperationGuard {
            registry: self.clone(),
            operation,
            active: true,
        })
    }

    pub fn begin_migration(&self) -> Result<StorageOperationGuard, StorageOperationError> {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let blockers = active_kinds(&state);
        if !blockers.is_empty() {
            return Err(StorageOperationError::busy(blockers));
        }

        state.counts.insert(StorageOperationKind::Migration, 1);
        Ok(StorageOperationGuard {
            registry: self.clone(),
            operation: StorageOperationKind::Migration,
            active: true,
        })
    }

    pub fn snapshot(&self) -> StorageOperationSnapshot {
        let state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut active_operations = state
            .counts
            .iter()
            .filter_map(|(operation, count)| {
                (*count > 0).then_some(ActiveStorageOperation {
                    operation: *operation,
                    count: *count,
                })
            })
            .collect::<Vec<_>>();
        active_operations.sort_by_key(|entry| entry.operation.as_str());
        let migration_active = active_operations
            .iter()
            .any(|entry| entry.operation == StorageOperationKind::Migration);
        StorageOperationSnapshot {
            busy: !active_operations.is_empty(),
            migration_active,
            active_operations,
        }
    }

    fn finish(&self, operation: StorageOperationKind) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(count) = state.counts.get_mut(&operation) {
            *count = count.saturating_sub(1);
            if *count == 0 {
                state.counts.remove(&operation);
            }
        }
    }
}

fn active_kinds(state: &RegistryState) -> Vec<StorageOperationKind> {
    state
        .counts
        .iter()
        .filter_map(|(operation, count)| (*count > 0).then_some(*operation))
        .collect()
}

#[derive(Debug)]
pub struct StorageOperationGuard {
    registry: StorageOperationRegistry,
    operation: StorageOperationKind,
    active: bool,
}

impl Drop for StorageOperationGuard {
    fn drop(&mut self) {
        if self.active {
            self.registry.finish(self.operation);
            self.active = false;
        }
    }
}

static GLOBAL_STORAGE_OPERATIONS: LazyLock<StorageOperationRegistry> =
    LazyLock::new(StorageOperationRegistry::default);

pub fn global_storage_operations() -> StorageOperationRegistry {
    GLOBAL_STORAGE_OPERATIONS.clone()
}

pub fn begin_storage_operation(
    operation: StorageOperationKind,
) -> Result<StorageOperationGuard, StorageOperationError> {
    GLOBAL_STORAGE_OPERATIONS.begin(operation)
}

pub fn storage_operation_snapshot() -> StorageOperationSnapshot {
    GLOBAL_STORAGE_OPERATIONS.snapshot()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migration_and_normal_operations_block_each_other() {
        let registry = StorageOperationRegistry::default();
        let recording = registry.begin(StorageOperationKind::Recording).unwrap();
        let blocked_migration = registry.begin_migration().unwrap_err();
        assert_eq!(blocked_migration.code, "operation_busy");
        assert_eq!(
            blocked_migration.blockers,
            vec![StorageOperationKind::Recording]
        );
        drop(recording);

        let migration = registry.begin_migration().unwrap();
        let blocked_summary = registry
            .begin(StorageOperationKind::SummaryGeneration)
            .unwrap_err();
        assert_eq!(blocked_summary.code, "operation_busy");
        assert_eq!(
            blocked_summary.blockers,
            vec![StorageOperationKind::Migration]
        );
        drop(migration);
        assert!(!registry.snapshot().busy);
    }

    #[test]
    fn duplicate_migration_is_rejected() {
        let registry = StorageOperationRegistry::default();
        let _migration = registry.begin_migration().unwrap();
        let error = registry.begin_migration().unwrap_err();
        assert_eq!(error.code, "operation_busy");
        assert_eq!(error.blockers, vec![StorageOperationKind::Migration]);
    }

    #[test]
    fn guard_drop_decrements_counts_without_clearing_other_guards() {
        let registry = StorageOperationRegistry::default();
        let first = registry.begin(StorageOperationKind::ModelDownload).unwrap();
        let second = registry.begin(StorageOperationKind::ModelDownload).unwrap();
        assert_eq!(registry.snapshot().active_operations[0].count, 2);
        drop(first);
        assert_eq!(registry.snapshot().active_operations[0].count, 1);
        drop(second);
        assert!(!registry.snapshot().busy);
    }

    #[test]
    fn moss_and_summary_block_each_other() {
        let registry = StorageOperationRegistry::default();

        let moss = registry
            .begin(StorageOperationKind::MossEnhancement)
            .unwrap();
        let blocked_summary = registry
            .begin(StorageOperationKind::SummaryGeneration)
            .unwrap_err();
        assert_eq!(blocked_summary.code, "operation_busy");
        assert_eq!(
            blocked_summary.blockers,
            vec![StorageOperationKind::MossEnhancement]
        );
        drop(moss);

        let summary = registry
            .begin(StorageOperationKind::SummaryGeneration)
            .unwrap();
        let blocked_moss = registry
            .begin(StorageOperationKind::MossEnhancement)
            .unwrap_err();
        assert_eq!(blocked_moss.code, "operation_busy");
        assert_eq!(
            blocked_moss.blockers,
            vec![StorageOperationKind::SummaryGeneration]
        );
        drop(summary);

        assert!(!registry.snapshot().busy);
    }
}

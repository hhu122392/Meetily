use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sysinfo::Disks;

use super::layout::{
    ensure_absolute_non_root, ensure_no_legacy_overlap, path_is_same_or_descendant,
};
use super::{StorageError, StorageLayout, StorageResult};

pub const MIGRATION_MINIMUM_FREE_BYTES: u64 = 20 * 1024 * 1024 * 1024;

pub(crate) fn available_bytes_for_path(path: &Path) -> Option<u64> {
    let disks = Disks::new_with_refreshed_list();
    available_bytes_for_path_from_disks(path, &disks)
}

pub(crate) fn available_bytes_for_path_from_disks(path: &Path, disks: &Disks) -> Option<u64> {
    let checked_directory = nearest_existing_directory(path)?;
    disks
        .list()
        .iter()
        .filter(|disk| path_is_same_or_descendant(&checked_directory, disk.mount_point()))
        .max_by_key(|disk| disk.mount_point().components().count())
        .map(|disk| disk.available_space())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StorageValidationReport {
    pub target_root: PathBuf,
    pub valid: bool,
    pub target_exists: bool,
    pub checked_directory: PathBuf,
    pub writable: bool,
    pub mount_point: Option<PathBuf>,
    pub file_system: Option<String>,
    pub removable: Option<bool>,
    pub total_bytes: Option<u64>,
    pub available_bytes: Option<u64>,
    pub required_free_bytes: u64,
    pub errors: Vec<String>,
}

pub fn validate_target_candidate(
    target_root: &Path,
    legacy_app_data_root: &Path,
    required_free_bytes: u64,
    perform_write_probe: bool,
) -> StorageValidationReport {
    let target_root = target_root.to_path_buf();
    let mut errors = Vec::new();

    let structurally_valid = match ensure_absolute_non_root(&target_root) {
        Ok(()) => true,
        Err(error) => {
            errors.push(error.to_string());
            false
        }
    };
    if structurally_valid {
        if let Err(error) = ensure_no_legacy_overlap(&target_root, legacy_app_data_root) {
            errors.push(error.to_string());
        }
        match first_link_or_reparse_point(&target_root) {
            Ok(Some(path)) => errors.push(StorageError::UnsafeReparsePoint(path).to_string()),
            Ok(None) => {}
            Err(error) => errors.push(error.to_string()),
        }
    }

    let target_exists = target_root.exists();
    if target_exists && !target_root.is_dir() {
        errors.push(format!(
            "Storage target exists but is not a directory: {}",
            target_root.display()
        ));
    }

    let checked_directory = nearest_existing_directory(&target_root).unwrap_or_else(|| {
        errors.push(format!(
            "No existing parent directory can be checked for: {}",
            target_root.display()
        ));
        target_root.clone()
    });

    let mut writable = false;
    if errors.is_empty() && checked_directory.is_dir() {
        if perform_write_probe {
            match write_probe(&checked_directory) {
                Ok(()) => writable = true,
                Err(error) => errors.push(error.to_string()),
            }
        } else {
            writable = !fs::metadata(&checked_directory)
                .map(|metadata| metadata.permissions().readonly())
                .unwrap_or(true);
            if !writable {
                errors.push(format!(
                    "Storage directory is read-only: {}",
                    checked_directory.display()
                ));
            }
        }
    }

    let disks = Disks::new_with_refreshed_list();
    let disk = disks
        .list()
        .iter()
        .filter(|disk| path_is_same_or_descendant(&checked_directory, disk.mount_point()))
        .max_by_key(|disk| disk.mount_point().components().count());

    let (mount_point, file_system, removable, total_bytes, available_bytes) = if let Some(disk) =
        disk
    {
        let file_system = disk.file_system().to_string_lossy().to_string();
        let available = disk.available_space();
        let removable = disk.is_removable();

        #[cfg(target_os = "windows")]
        {
            if !file_system.eq_ignore_ascii_case("NTFS") {
                errors.push(format!(
                    "Windows storage target must be on NTFS; detected {file_system}"
                ));
            }
            if removable {
                errors.push("Storage target must be on a fixed drive, not removable media".into());
            }
        }

        if available < required_free_bytes {
            errors.push(format!(
                "Storage target has {available} available bytes; {required_free_bytes} bytes are required"
            ));
        }

        (
            Some(disk.mount_point().to_path_buf()),
            Some(file_system),
            Some(removable),
            Some(disk.total_space()),
            Some(available),
        )
    } else {
        errors.push(format!(
            "Unable to resolve a mounted disk for {}",
            checked_directory.display()
        ));
        (None, None, None, None, None)
    };

    StorageValidationReport {
        target_root,
        valid: errors.is_empty() && writable,
        target_exists,
        checked_directory,
        writable,
        mount_point,
        file_system,
        removable,
        total_bytes,
        available_bytes,
        required_free_bytes,
        errors,
    }
}

pub fn validate_active_layout(layout: &StorageLayout) -> StorageResult<()> {
    let root = layout.active_storage_root();
    if !root.is_dir() {
        return Err(StorageError::Validation(format!(
            "Migrated storage root does not exist or is not a directory: {}",
            root.display()
        )));
    }
    if !layout.models_root().is_dir() {
        return Err(StorageError::Validation(format!(
            "Migrated models directory is missing: {}",
            layout.models_root().display()
        )));
    }

    let report = validate_target_candidate(root, layout.legacy_app_data_root(), 0, true);
    if !report.valid {
        return Err(StorageError::Validation(report.errors.join("; ")));
    }
    Ok(())
}

fn nearest_existing_directory(path: &Path) -> Option<PathBuf> {
    let mut cursor = Some(path);
    while let Some(candidate) = cursor {
        if candidate.is_dir() {
            return Some(candidate.to_path_buf());
        }
        cursor = candidate.parent();
    }
    None
}

pub(crate) fn first_link_or_reparse_point(path: &Path) -> StorageResult<Option<PathBuf>> {
    for ancestor in path.ancestors() {
        let metadata = match fs::symlink_metadata(ancestor) {
            Ok(metadata) => metadata,
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => continue,
            Err(source) => {
                return Err(StorageError::Io {
                    operation: "inspect",
                    path: ancestor.to_path_buf(),
                    source,
                })
            }
        };

        #[cfg(target_os = "windows")]
        {
            use std::os::windows::fs::MetadataExt;
            const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0400;
            if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
                return Ok(Some(ancestor.to_path_buf()));
            }
        }

        #[cfg(not(target_os = "windows"))]
        if metadata.file_type().is_symlink() {
            return Ok(Some(ancestor.to_path_buf()));
        }
    }

    Ok(None)
}

fn write_probe(directory: &Path) -> StorageResult<()> {
    let probe_path = directory.join(format!(
        ".meetily-storage-write-probe-{}.tmp",
        uuid::Uuid::new_v4().simple()
    ));
    let content = uuid::Uuid::new_v4().to_string();

    let result = (|| -> StorageResult<()> {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&probe_path)
            .map_err(|source| StorageError::Io {
                operation: "create write probe in",
                path: probe_path.clone(),
                source,
            })?;
        file.write_all(content.as_bytes())
            .map_err(|source| StorageError::Io {
                operation: "write probe",
                path: probe_path.clone(),
                source,
            })?;
        file.sync_all().map_err(|source| StorageError::Io {
            operation: "flush write probe",
            path: probe_path.clone(),
            source,
        })?;
        drop(file);

        let mut read_back = String::new();
        fs::File::open(&probe_path)
            .and_then(|mut file| file.read_to_string(&mut read_back))
            .map_err(|source| StorageError::Io {
                operation: "read write probe",
                path: probe_path.clone(),
                source,
            })?;
        if read_back != content {
            return Err(StorageError::Validation(format!(
                "Write probe content mismatch at {}",
                probe_path.display()
            )));
        }
        Ok(())
    })();

    if probe_path.exists() {
        if let Err(source) = fs::remove_file(&probe_path) {
            return Err(StorageError::Io {
                operation: "remove write probe",
                path: probe_path,
                source,
            });
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writable_directory_passes_and_probe_is_removed() {
        let legacy = tempfile::tempdir().unwrap();
        let target_parent = tempfile::tempdir().unwrap();
        let target = target_parent.path().join("中文 目录");
        let report = validate_target_candidate(&target, legacy.path(), 0, true);

        assert!(report.valid, "{:?}", report.errors);
        assert!(!report.target_exists);
        assert!(report.writable);
        let leftovers = fs::read_dir(target_parent.path())
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".meetily-storage-write-probe-")
            })
            .count();
        assert_eq!(leftovers, 0);
    }

    #[test]
    fn target_inside_legacy_data_is_rejected() {
        let legacy = tempfile::tempdir().unwrap();
        let target = legacy.path().join("models-moved");
        let report = validate_target_candidate(&target, legacy.path(), 0, false);

        assert!(!report.valid);
        assert!(report
            .errors
            .iter()
            .any(|error| error.contains("legacy application-data")));
    }

    #[test]
    fn target_containing_legacy_data_is_rejected() {
        let target = tempfile::tempdir().unwrap();
        let legacy = target.path().join("com.meetily.ai");
        let report = validate_target_candidate(target.path(), &legacy, 0, false);

        assert!(!report.valid);
        assert!(report
            .errors
            .iter()
            .any(|error| error.contains("cannot contain the legacy")));
    }

    #[test]
    fn filesystem_root_is_rejected() {
        let temporary_directory = std::env::temp_dir();
        let root = temporary_directory
            .ancestors()
            .last()
            .expect("temporary directory must have a filesystem root");
        let legacy = std::env::temp_dir().join("legacy-root-test");
        let report = validate_target_candidate(root, &legacy, 0, false);
        assert!(!report.valid);
        assert!(report.errors.iter().any(|error| error.contains("root")));
    }

    #[test]
    #[ignore = "requires MEETILY_S1_TARGET_ROOT and MEETILY_S1_LEGACY_APP_DATA on the closed-app Windows fixture"]
    fn s1_real_target_candidate_is_validated_without_creating_it() {
        let target = PathBuf::from(
            std::env::var("MEETILY_S1_TARGET_ROOT")
                .expect("MEETILY_S1_TARGET_ROOT must point to the planned target"),
        );
        let legacy = PathBuf::from(
            std::env::var("MEETILY_S1_LEGACY_APP_DATA")
                .expect("MEETILY_S1_LEGACY_APP_DATA must point to the legacy app-data root"),
        );
        assert!(
            !target.exists(),
            "S1 must not create the formal target root"
        );

        let report =
            validate_target_candidate(&target, &legacy, MIGRATION_MINIMUM_FREE_BYTES, true);

        assert!(report.valid, "{:?}", report.errors);
        assert!(!report.target_exists);
        assert!(report.writable);
        assert_eq!(report.file_system.as_deref(), Some("NTFS"));
        assert_eq!(report.removable, Some(false));
        assert!(
            report.available_bytes.unwrap_or_default() >= MIGRATION_MINIMUM_FREE_BYTES,
            "actual free space fell below the S1 migration gate"
        );
        assert!(report.errors.is_empty());
        assert!(
            !target.exists(),
            "validation must not create the formal target root"
        );
    }
}

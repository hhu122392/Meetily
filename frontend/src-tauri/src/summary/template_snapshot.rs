use crate::meeting_context::{load_summary_meeting_context, SummaryMeetingContext};
use crate::summary::source_binding::SummarySourceBinding;
use crate::summary::templates::{Template, TemplateOrigin, TemplateV2};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

pub const SNAPSHOT_DIRECTORY: &str = "summary-template-snapshots";
const MAX_SNAPSHOT_BYTES: u64 = 2 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SnapshotResolutionSource {
    MeetingOverride,
    GlobalDefault,
    BuiltinFallback,
    HistoricalSnapshot,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SnapshotTemplateOrigin {
    Builtin,
    Bundled,
    Custom,
    Snapshot,
}

impl From<TemplateOrigin> for SnapshotTemplateOrigin {
    fn from(value: TemplateOrigin) -> Self {
        match value {
            TemplateOrigin::Builtin => Self::Builtin,
            TemplateOrigin::Bundled => Self::Bundled,
            TemplateOrigin::Custom => Self::Custom,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SnapshotTemplateRef {
    pub id: String,
    pub version: u64,
    pub origin: SnapshotTemplateOrigin,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SnapshotHashes {
    pub file_sha256: String,
    pub semantic_sha256: String,
    pub legacy_cache_fingerprint: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SnapshotModelContext {
    pub provider: String,
    pub name: String,
    pub configuration_fingerprint: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SnapshotGenerationContext {
    pub summary_language: Option<String>,
    pub custom_prompt_sha256: Option<String>,
    pub model: SnapshotModelContext,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary_meeting_context: Option<SummaryMeetingContext>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary_context_sha256: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary_source_binding: Option<SummarySourceBinding>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SummaryTemplateSnapshot {
    pub snapshot_schema_version: u8,
    pub generation_id: String,
    pub meeting_id: String,
    pub captured_at: DateTime<Utc>,
    pub resolution_source: SnapshotResolutionSource,
    pub template_ref: SnapshotTemplateRef,
    pub template: TemplateV2,
    pub hashes: SnapshotHashes,
    pub generation_context: SnapshotGenerationContext,
}

#[derive(Debug, Clone)]
pub struct ResolvedGenerationTemplate {
    pub template: TemplateV2,
    pub runtime_template: Template,
    pub origin: SnapshotTemplateOrigin,
    pub resolution_source: SnapshotResolutionSource,
    pub file_sha256: String,
    pub semantic_sha256: String,
}

impl ResolvedGenerationTemplate {
    pub fn from_snapshot(snapshot: &SummaryTemplateSnapshot) -> Self {
        Self {
            template: snapshot.template.clone(),
            runtime_template: snapshot.template.to_runtime_template(),
            origin: SnapshotTemplateOrigin::Snapshot,
            resolution_source: SnapshotResolutionSource::HistoricalSnapshot,
            file_sha256: snapshot.hashes.file_sha256.clone(),
            semantic_sha256: snapshot.hashes.semantic_sha256.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResolvedTemplateSummary {
    pub id: String,
    pub version: u64,
    pub file_sha256: String,
    pub semantic_sha256: String,
    pub resolution_source: SnapshotResolutionSource,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GenerationSnapshotLink {
    pub generation_id: String,
    pub snapshot_path_relative: String,
    pub resolved_template: ResolvedTemplateSummary,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub meeting_context_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub meeting_context_sha256: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary_context_sha256: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary_source_binding: Option<SummarySourceBinding>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SnapshotListItem {
    pub generation_id: String,
    pub captured_at: DateTime<Utc>,
    pub template_id: String,
    pub template_name: String,
    pub template_version: u64,
    pub file_sha256: String,
    pub resolution_source: SnapshotResolutionSource,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SnapshotFileState {
    Available,
    Corrupt,
}

#[derive(Debug, Clone)]
pub struct SnapshotFileInspection {
    pub generation_id: String,
    pub byte_size: u64,
    pub modified_at: DateTime<Utc>,
    pub state: SnapshotFileState,
    pub snapshot: Option<SummaryTemplateSnapshot>,
}

pub fn sha256_text(value: &str) -> String {
    sha256_bytes(value.as_bytes())
}

pub fn validate_generation_id(value: &str) -> bool {
    let mut bytes = value.bytes();
    let Some(first) = bytes.next() else {
        return false;
    };
    value.len() <= 120
        && first.is_ascii_alphanumeric()
        && bytes.all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
}

pub fn capture_snapshot(
    meeting_folder: &Path,
    generation_id: &str,
    meeting_id: &str,
    resolved: &ResolvedGenerationTemplate,
    context: SnapshotGenerationContext,
) -> Result<(SummaryTemplateSnapshot, PathBuf, String), String> {
    if !validate_generation_id(generation_id) {
        return Err("generation id failed validation".to_owned());
    }
    if meeting_id.trim().is_empty() || meeting_id.len() > 200 {
        return Err("meeting id failed snapshot validation".to_owned());
    }
    validate_resolved_template(resolved)?;
    let snapshot = SummaryTemplateSnapshot {
        snapshot_schema_version: if context.summary_source_binding.is_some() {
            3
        } else {
            2
        },
        generation_id: generation_id.to_owned(),
        meeting_id: meeting_id.to_owned(),
        captured_at: Utc::now(),
        resolution_source: resolved.resolution_source,
        template_ref: SnapshotTemplateRef {
            id: resolved.template.id.clone(),
            version: resolved.template.version,
            origin: resolved.origin,
        },
        template: resolved.template.clone(),
        hashes: SnapshotHashes {
            file_sha256: resolved.file_sha256.clone(),
            semantic_sha256: resolved.semantic_sha256.clone(),
            legacy_cache_fingerprint: None,
        },
        generation_context: context,
    };
    validate_snapshot(&snapshot, Some(meeting_id), Some(generation_id))?;

    let directory = safe_snapshot_directory(meeting_folder, true)?;
    let final_path = directory.join(format!("{generation_id}.json"));
    if final_path.exists() {
        return Err("snapshot generation id already exists".to_owned());
    }
    let temporary_path = directory.join(format!(".{generation_id}.{}.tmp", uuid::Uuid::new_v4()));
    let bytes = serde_json::to_vec_pretty(&snapshot)
        .map_err(|error| format!("snapshot serialization failed: {error}"))?;
    if bytes.len() as u64 > MAX_SNAPSHOT_BYTES {
        return Err("snapshot exceeds the maximum permitted size".to_owned());
    }
    let write_result = (|| -> Result<(), String> {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary_path)
            .map_err(|error| format!("snapshot temporary file could not be created: {error}"))?;
        file.write_all(&bytes)
            .map_err(|error| format!("snapshot could not be written: {error}"))?;
        file.sync_all()
            .map_err(|error| format!("snapshot could not be flushed: {error}"))?;
        // Windows does not permit replacing/renaming a file while this handle is
        // still open with the default share flags.
        drop(file);
        fs::rename(&temporary_path, &final_path)
            .map_err(|error| format!("snapshot could not be committed atomically: {error}"))?;
        Ok(())
    })();
    if write_result.is_err() {
        let _ = fs::remove_file(&temporary_path);
    }
    write_result?;

    Ok((
        snapshot,
        final_path,
        format!("{SNAPSHOT_DIRECTORY}/{generation_id}.json"),
    ))
}

pub fn read_snapshot(
    meeting_folder: &Path,
    meeting_id: &str,
    generation_id: &str,
) -> Result<SummaryTemplateSnapshot, String> {
    if !validate_generation_id(generation_id) {
        return Err("generation id failed validation".to_owned());
    }
    let directory = safe_snapshot_directory(meeting_folder, false)?;
    let path = directory.join(format!("{generation_id}.json"));
    reject_reparse_point(&path)?;
    let metadata = fs::metadata(&path)
        .map_err(|error| format!("historical snapshot could not be opened: {error}"))?;
    if !metadata.is_file() || metadata.len() > MAX_SNAPSHOT_BYTES {
        return Err("historical snapshot is not a bounded regular file".to_owned());
    }
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    OpenOptions::new()
        .read(true)
        .open(&path)
        .and_then(|mut file| file.read_to_end(&mut bytes))
        .map_err(|error| format!("historical snapshot could not be read: {error}"))?;
    let snapshot: SummaryTemplateSnapshot = serde_json::from_slice(&bytes)
        .map_err(|error| format!("historical snapshot JSON is invalid: {error}"))?;
    validate_snapshot(&snapshot, Some(meeting_id), Some(generation_id))?;
    Ok(snapshot)
}

/// Resolves immutable meeting facts for a generation. Historical retries use
/// their stored facts; current metadata is consulted only for a new generation.
pub fn resolve_summary_context_for_generation(
    meeting_folder: &Path,
    meeting_id: &str,
    historical_generation_id: Option<&str>,
) -> Result<Option<SummaryMeetingContext>, String> {
    match historical_generation_id {
        Some(generation_id) => Ok(read_snapshot(meeting_folder, meeting_id, generation_id)?
            .generation_context
            .summary_meeting_context),
        None => load_summary_meeting_context(meeting_folder),
    }
}

pub fn list_snapshots(
    meeting_folder: &Path,
    meeting_id: &str,
) -> Result<Vec<SnapshotListItem>, String> {
    let directory = meeting_folder.join(SNAPSHOT_DIRECTORY);
    if !directory.exists() {
        return Ok(Vec::new());
    }
    let directory = safe_snapshot_directory(meeting_folder, false)?;
    let mut snapshots = Vec::new();
    for entry in fs::read_dir(&directory)
        .map_err(|error| format!("snapshot directory could not be listed: {error}"))?
    {
        let entry = entry.map_err(|error| format!("snapshot directory entry failed: {error}"))?;
        let path = entry.path();
        if path.extension().and_then(|value| value.to_str()) != Some("json") {
            continue;
        }
        let Some(generation_id) = path.file_stem().and_then(|value| value.to_str()) else {
            continue;
        };
        match read_snapshot(meeting_folder, meeting_id, generation_id) {
            Ok(snapshot) => snapshots.push(SnapshotListItem {
                generation_id: snapshot.generation_id,
                captured_at: snapshot.captured_at,
                template_id: snapshot.template_ref.id,
                template_name: snapshot.template.name,
                template_version: snapshot.template_ref.version,
                file_sha256: snapshot.hashes.file_sha256,
                resolution_source: snapshot.resolution_source,
            }),
            Err(error) => {
                tracing::warn!(path = %path.display(), detail = %error, "invalid summary template snapshot was ignored")
            }
        }
    }
    snapshots.sort_by(|left, right| right.captured_at.cmp(&left.captured_at));
    Ok(snapshots)
}

pub fn count_snapshots_for_template(
    meeting_folder: &Path,
    meeting_id: &str,
    template_id: &str,
) -> Result<usize, String> {
    Ok(list_snapshots(meeting_folder, meeting_id)?
        .into_iter()
        .filter(|snapshot| snapshot.template_id == template_id)
        .count())
}

pub fn inspect_snapshot_files(
    meeting_folder: &Path,
    meeting_id: &str,
) -> Result<Vec<SnapshotFileInspection>, String> {
    let directory = meeting_folder.join(SNAPSHOT_DIRECTORY);
    if !directory.exists() {
        return Ok(Vec::new());
    }
    let directory = safe_snapshot_directory(meeting_folder, false)?;
    let mut files = Vec::new();
    for entry in fs::read_dir(&directory)
        .map_err(|error| format!("snapshot directory could not be inspected: {error}"))?
    {
        let entry = entry.map_err(|error| format!("snapshot directory entry failed: {error}"))?;
        let path = entry.path();
        if path.extension().and_then(|value| value.to_str()) != Some("json") {
            continue;
        }
        reject_reparse_point(&path)?;
        let Some(generation_id) = path.file_stem().and_then(|value| value.to_str()) else {
            continue;
        };
        // Unknown file names are left untouched. They are visible in logs for a
        // manual audit but never become cleanup targets.
        if !validate_generation_id(generation_id) {
            tracing::warn!(file_name = ?path.file_name(), "invalid snapshot file name was ignored");
            continue;
        }
        let metadata = fs::metadata(&path)
            .map_err(|error| format!("snapshot metadata could not be read: {error}"))?;
        if !metadata.is_file() {
            continue;
        }
        let modified_at = metadata
            .modified()
            .map(DateTime::<Utc>::from)
            .unwrap_or_else(|_| Utc::now());
        let snapshot = read_snapshot(meeting_folder, meeting_id, generation_id).ok();
        files.push(SnapshotFileInspection {
            generation_id: generation_id.to_owned(),
            byte_size: metadata.len(),
            modified_at,
            state: if snapshot.is_some() {
                SnapshotFileState::Available
            } else {
                SnapshotFileState::Corrupt
            },
            snapshot,
        });
    }
    files.sort_by(|left, right| right.modified_at.cmp(&left.modified_at));
    Ok(files)
}

pub fn quarantine_snapshot(
    meeting_folder: &Path,
    generation_id: &str,
    cleanup_batch_id: &str,
) -> Result<(PathBuf, PathBuf), String> {
    if !validate_generation_id(generation_id) || !validate_generation_id(cleanup_batch_id) {
        return Err("snapshot cleanup identity failed validation".to_owned());
    }
    let directory = safe_snapshot_directory(meeting_folder, false)?;
    let source = directory.join(format!("{generation_id}.json"));
    reject_reparse_point(&source)?;
    let quarantine_root = directory.join(".quarantine");
    fs::create_dir_all(&quarantine_root)
        .map_err(|error| format!("snapshot quarantine could not be created: {error}"))?;
    reject_reparse_point(&quarantine_root)?;
    let canonical_root = fs::canonicalize(&quarantine_root)
        .map_err(|error| format!("snapshot quarantine could not be resolved: {error}"))?;
    if !canonical_root.starts_with(&directory) {
        return Err("snapshot quarantine escapes the snapshot directory".to_owned());
    }
    let batch = canonical_root.join(cleanup_batch_id);
    fs::create_dir_all(&batch)
        .map_err(|error| format!("snapshot quarantine batch could not be created: {error}"))?;
    reject_reparse_point(&batch)?;
    let canonical_batch = fs::canonicalize(&batch)
        .map_err(|error| format!("snapshot quarantine batch could not be resolved: {error}"))?;
    if !canonical_batch.starts_with(&canonical_root) {
        return Err("snapshot quarantine batch escapes its root".to_owned());
    }
    let destination = canonical_batch.join(format!("{generation_id}.json"));
    if destination.exists() {
        return Err("snapshot cleanup destination already exists".to_owned());
    }
    fs::rename(&source, &destination)
        .map_err(|error| format!("snapshot could not be moved into quarantine: {error}"))?;
    Ok((source, destination))
}

pub fn restore_quarantined_snapshot(source: &Path, quarantined: &Path) -> Result<(), String> {
    if !quarantined.is_file() || source.exists() {
        return Err("snapshot quarantine rollback precondition failed".to_owned());
    }
    fs::rename(quarantined, source)
        .map_err(|error| format!("snapshot quarantine rollback failed: {error}"))
}

pub fn remove_snapshot_after_preflight_failure(path: &Path) {
    if let Err(error) = fs::remove_file(path) {
        tracing::warn!(path = %path.display(), detail = %error, "orphan snapshot rollback failed");
    }
}

fn validate_resolved_template(resolved: &ResolvedGenerationTemplate) -> Result<(), String> {
    if resolved.template.id.is_empty()
        || resolved.template.version == 0
        || !is_sha256(&resolved.file_sha256)
        || !is_sha256(&resolved.semantic_sha256)
    {
        return Err("resolved template failed snapshot integrity validation".to_owned());
    }
    if semantic_sha256(&resolved.template)? != resolved.semantic_sha256 {
        return Err("resolved template semantic hash does not match its content".to_owned());
    }
    Ok(())
}

fn validate_snapshot(
    snapshot: &SummaryTemplateSnapshot,
    expected_meeting_id: Option<&str>,
    expected_generation_id: Option<&str>,
) -> Result<(), String> {
    if !matches!(snapshot.snapshot_schema_version, 1 | 2 | 3)
        || !validate_generation_id(&snapshot.generation_id)
        || snapshot.meeting_id.is_empty()
        || snapshot.meeting_id.len() > 200
        || snapshot.template_ref.id != snapshot.template.id
        || snapshot.template_ref.version != snapshot.template.version
        || snapshot.template.version == 0
        || !is_sha256(&snapshot.hashes.file_sha256)
        || !is_sha256(&snapshot.hashes.semantic_sha256)
    {
        return Err("snapshot failed integrity validation".to_owned());
    }
    if semantic_sha256(&snapshot.template)? != snapshot.hashes.semantic_sha256 {
        return Err("snapshot template content does not match its semantic hash".to_owned());
    }
    if expected_meeting_id.is_some_and(|value| value != snapshot.meeting_id)
        || expected_generation_id.is_some_and(|value| value != snapshot.generation_id)
    {
        return Err(
            "snapshot identity does not match its requested meeting or generation".to_owned(),
        );
    }
    if snapshot
        .generation_context
        .custom_prompt_sha256
        .as_deref()
        .is_some_and(|value| !is_sha256(value))
    {
        return Err("snapshot prompt fingerprint is invalid".to_owned());
    }
    match (
        snapshot.generation_context.summary_meeting_context.as_ref(),
        snapshot
            .generation_context
            .summary_context_sha256
            .as_deref(),
    ) {
        (Some(context), Some(context_sha256))
            if is_sha256(&context.context_sha256)
                && is_sha256(context_sha256)
                && context.sha256() == context_sha256 => {}
        (None, None) => {}
        _ => return Err("snapshot meeting context fingerprint is invalid".to_owned()),
    }
    if let Some(binding) = snapshot.generation_context.summary_source_binding.as_ref() {
        binding
            .validate_lineage()
            .map_err(|_| "snapshot summary source binding is invalid".to_owned())?;
        if binding.meeting_id != snapshot.meeting_id
            || binding.template.template_id != snapshot.template_ref.id
            || binding.template.template_version != snapshot.template_ref.version
            || binding.template.template_file_sha256 != snapshot.hashes.file_sha256
            || binding.template.template_semantic_sha256 != snapshot.hashes.semantic_sha256
        {
            return Err("snapshot summary source binding does not match its template".to_owned());
        }
    } else if snapshot.snapshot_schema_version >= 3 {
        return Err("snapshot summary source binding is missing".to_owned());
    }
    Ok(())
}

fn safe_snapshot_directory(meeting_folder: &Path, create: bool) -> Result<PathBuf, String> {
    if !meeting_folder.is_dir() {
        return Err("meeting folder is unavailable".to_owned());
    }
    reject_reparse_point(meeting_folder)?;
    let canonical_meeting = fs::canonicalize(meeting_folder)
        .map_err(|error| format!("meeting folder could not be resolved: {error}"))?;
    let directory = canonical_meeting.join(SNAPSHOT_DIRECTORY);
    if create {
        fs::create_dir_all(&directory)
            .map_err(|error| format!("snapshot directory could not be created: {error}"))?;
    }
    if !directory.is_dir() {
        return Err("snapshot directory is unavailable".to_owned());
    }
    reject_reparse_point(&directory)?;
    let canonical_directory = fs::canonicalize(&directory)
        .map_err(|error| format!("snapshot directory could not be resolved: {error}"))?;
    if !canonical_directory.starts_with(&canonical_meeting) {
        return Err("snapshot directory escapes the meeting folder".to_owned());
    }
    Ok(canonical_directory)
}

fn reject_reparse_point(path: &Path) -> Result<(), String> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| format!("snapshot path metadata could not be read: {error}"))?;
    if metadata.file_type().is_symlink() {
        return Err("symbolic links are not permitted in snapshot paths".to_owned());
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
        if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return Err("Windows reparse points are not permitted in snapshot paths".to_owned());
        }
    }
    Ok(())
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn sha256_bytes(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn semantic_sha256(template: &TemplateV2) -> Result<String, String> {
    let value = serde_json::to_value(template)
        .map_err(|error| format!("template could not be normalized for hashing: {error}"))?;
    let canonical = canonicalize_json(&value);
    let bytes = serde_json::to_vec(&canonical)
        .map_err(|error| format!("canonical template could not be hashed: {error}"))?;
    Ok(sha256_bytes(&bytes))
}

fn canonicalize_json(value: &Value) -> Value {
    match value {
        Value::Array(items) => Value::Array(items.iter().map(canonicalize_json).collect()),
        Value::Object(object) => {
            let mut keys: Vec<_> = object.keys().collect();
            keys.sort();
            let mut canonical = serde_json::Map::new();
            for key in keys {
                canonical.insert(key.clone(), canonicalize_json(&object[key]));
            }
            Value::Object(canonical)
        }
        _ => value.clone(),
    }
}

pub fn snapshot_link_json(link: &GenerationSnapshotLink) -> Value {
    serde_json::to_value(link).unwrap_or(Value::Null)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::meeting_context::{
        AttendanceStatus, MeetingContextContainer, MeetingContextProfile, PersonProfile,
    };
    use crate::summary::templates::{
        EmptyBehavior, TemplateFormat, TemplateSectionV2, TemplateSource, TemplateSourceType,
    };
    use serde_json::Map;
    use tempfile::tempdir;

    fn resolved() -> ResolvedGenerationTemplate {
        let at = DateTime::parse_from_rfc3339("2026-08-23T00:00:00+00:00").unwrap();
        let template = TemplateV2 {
            schema_version: 2,
            id: "customer_review".to_owned(),
            name: "Customer Review".to_owned(),
            description: "Review".to_owned(),
            version: 3,
            locale: Some("en".to_owned()),
            tags: Vec::new(),
            source: TemplateSource {
                source_type: TemplateSourceType::Manual,
                original_file_name: None,
                original_file_sha256: None,
                imported_at: None,
                copied_from_template_id: None,
            },
            created_at: at,
            updated_at: at,
            sections: vec![TemplateSectionV2 {
                id: "summary".to_owned(),
                title: "Summary".to_owned(),
                instruction: "Summarize".to_owned(),
                format: TemplateFormat::Paragraph,
                item_format: None,
                example_item_format: None,
                required: true,
                empty_behavior: EmptyBehavior::ShowNotMentioned,
            }],
            extensions: Map::new(),
        };
        let semantic_sha256 = semantic_sha256(&template).unwrap();
        ResolvedGenerationTemplate {
            runtime_template: template.to_runtime_template(),
            template,
            origin: SnapshotTemplateOrigin::Custom,
            resolution_source: SnapshotResolutionSource::MeetingOverride,
            file_sha256: "a".repeat(64),
            semantic_sha256,
        }
    }

    fn context() -> SnapshotGenerationContext {
        SnapshotGenerationContext {
            summary_language: Some("zh-CN".to_owned()),
            custom_prompt_sha256: Some(sha256_text("private prompt")),
            model: SnapshotModelContext {
                provider: "ollama".to_owned(),
                name: "qwen".to_owned(),
                configuration_fingerprint: None,
            },
            summary_meeting_context: None,
            summary_context_sha256: None,
            summary_source_binding: None,
        }
    }

    fn write_meeting_metadata(folder: &Path, container: &MeetingContextContainer) {
        fs::write(
            folder.join("metadata.json"),
            serde_json::to_vec_pretty(&serde_json::json!({
                "meeting_name": "Context snapshot test",
                "created_at": "2026-08-27T01:00:00Z",
                "meeting_context": container,
            }))
            .unwrap(),
        )
        .unwrap();
    }

    #[test]
    fn snapshot_round_trip_is_immutable_and_does_not_store_prompt_text() {
        let temporary = tempdir().unwrap();
        let (snapshot, path, relative) = capture_snapshot(
            temporary.path(),
            "gen_123",
            "meeting-123",
            &resolved(),
            context(),
        )
        .unwrap();
        assert_eq!(relative, "summary-template-snapshots/gen_123.json");
        assert!(path.is_file());
        let raw = fs::read_to_string(&path).unwrap();
        assert!(!raw.contains("private prompt"));
        assert!(capture_snapshot(
            temporary.path(),
            "gen_123",
            "meeting-123",
            &resolved(),
            context(),
        )
        .is_err());
        assert_eq!(
            read_snapshot(temporary.path(), "meeting-123", "gen_123").unwrap(),
            snapshot
        );
    }

    #[test]
    fn generation_id_rejects_path_traversal() {
        for invalid in ["", "../escape", "a/b", ".hidden", "a.json"] {
            assert!(!validate_generation_id(invalid));
        }
        assert!(validate_generation_id("gen_01-abc"));
    }

    #[test]
    fn list_and_count_ignore_non_json_files() {
        let temporary = tempdir().unwrap();
        capture_snapshot(
            temporary.path(),
            "gen_1",
            "meeting-1",
            &resolved(),
            context(),
        )
        .unwrap();
        fs::write(
            temporary.path().join(SNAPSHOT_DIRECTORY).join("note.txt"),
            "ignored",
        )
        .unwrap();
        assert_eq!(
            list_snapshots(temporary.path(), "meeting-1").unwrap().len(),
            1
        );
        assert_eq!(
            count_snapshots_for_template(temporary.path(), "meeting-1", "customer_review").unwrap(),
            1
        );
    }

    #[test]
    fn edited_snapshot_content_is_rejected_by_semantic_hash() {
        let temporary = tempdir().unwrap();
        let (_, path, _) = capture_snapshot(
            temporary.path(),
            "gen_tamper",
            "meeting-1",
            &resolved(),
            context(),
        )
        .unwrap();
        let mut value: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        value["template"]["name"] = Value::String("Tampered".to_owned());
        fs::write(&path, serde_json::to_vec_pretty(&value).unwrap()).unwrap();
        assert!(read_snapshot(temporary.path(), "meeting-1", "gen_tamper")
            .unwrap_err()
            .contains("semantic hash"));
    }

    #[test]
    fn oversized_snapshot_is_rejected_without_leaving_partial_files() {
        let temporary = tempdir().unwrap();
        let mut oversized = resolved();
        oversized.template.description = "x".repeat(MAX_SNAPSHOT_BYTES as usize);
        oversized.runtime_template = oversized.template.to_runtime_template();
        oversized.semantic_sha256 = semantic_sha256(&oversized.template).unwrap();

        let error = capture_snapshot(
            temporary.path(),
            "gen_oversized",
            "meeting-1",
            &oversized,
            context(),
        )
        .unwrap_err();
        assert!(error.contains("maximum permitted size"));
        let directory = temporary.path().join(SNAPSHOT_DIRECTORY);
        assert!(directory.is_dir());
        assert_eq!(fs::read_dir(directory).unwrap().count(), 0);
    }

    #[test]
    fn unavailable_snapshot_directory_fails_closed() {
        let temporary = tempdir().unwrap();
        let collision = temporary.path().join(SNAPSHOT_DIRECTORY);
        fs::write(&collision, "not a directory").unwrap();
        let error = capture_snapshot(
            temporary.path(),
            "gen_collision",
            "meeting-1",
            &resolved(),
            context(),
        )
        .unwrap_err();
        assert!(error.contains("snapshot directory"));
        assert_eq!(fs::read_to_string(collision).unwrap(), "not a directory");
    }

    #[test]
    fn snapshot_cannot_be_replayed_for_a_different_meeting() {
        let temporary = tempdir().unwrap();
        capture_snapshot(
            temporary.path(),
            "gen_identity",
            "meeting-original",
            &resolved(),
            context(),
        )
        .unwrap();
        assert!(
            read_snapshot(temporary.path(), "meeting-other", "gen_identity")
                .unwrap_err()
                .contains("identity")
        );
    }

    #[test]
    fn mc_i04_historical_retry_keeps_original_context_after_current_context_changes() {
        let temporary = tempdir().unwrap();
        let mut container = MeetingContextContainer::from_profile(
            MeetingContextProfile {
                schema_version: 1,
                fixed_meeting_mechanism: None,
                people: vec![PersonProfile {
                    person_id: "person_owner".to_owned(),
                    display_name: "Alice".to_owned(),
                    aliases: vec![],
                    department: None,
                    role: None,
                    enabled: true,
                }],
                terms: vec![],
            },
            "customer_review".to_owned(),
            3,
            "a".repeat(64),
            Utc::now(),
        )
        .unwrap();
        write_meeting_metadata(temporary.path(), &container);
        let original_context =
            resolve_summary_context_for_generation(temporary.path(), "meeting-1", None)
                .unwrap()
                .unwrap();
        let original_context_sha256 = original_context.sha256();
        let mut generation_context = context();
        generation_context.summary_meeting_context = Some(original_context.clone());
        generation_context.summary_context_sha256 = Some(original_context_sha256.clone());
        capture_snapshot(
            temporary.path(),
            "gen_original_context",
            "meeting-1",
            &resolved(),
            generation_context,
        )
        .unwrap();

        let mut revision = container.current_context().unwrap().clone();
        revision.people[0].display_name = "Bob".to_owned();
        revision.people[0].attendance = AttendanceStatus::Attending;
        container
            .append_revision(revision, "manual_update", Utc::now())
            .unwrap();
        write_meeting_metadata(temporary.path(), &container);

        let current = resolve_summary_context_for_generation(temporary.path(), "meeting-1", None)
            .unwrap()
            .unwrap();
        let historical = resolve_summary_context_for_generation(
            temporary.path(),
            "meeting-1",
            Some("gen_original_context"),
        )
        .unwrap()
        .unwrap();

        assert_ne!(current.context_id, historical.context_id);
        assert_ne!(current.sha256(), historical.sha256());
        assert_eq!(historical.context_id, original_context.context_id);
        assert_eq!(historical.sha256(), original_context_sha256);
        assert_eq!(
            historical.recognition_dictionary.people[0].display_name,
            "Alice"
        );
        assert_eq!(current.recognition_dictionary.people[0].display_name, "Bob");
        assert!(historical.verified_meeting_facts.attending.is_empty());
        assert_eq!(
            current.verified_meeting_facts.attending[0].display_name,
            "Bob"
        );
    }

    #[test]
    fn snapshot_rejects_tampered_structured_meeting_facts() {
        let temporary = tempdir().unwrap();
        let container = MeetingContextContainer::from_profile(
            MeetingContextProfile::default(),
            "customer_review".to_owned(),
            3,
            "a".repeat(64),
            Utc::now(),
        )
        .unwrap();
        write_meeting_metadata(temporary.path(), &container);
        let summary_context = load_summary_meeting_context(temporary.path())
            .unwrap()
            .unwrap();
        let mut generation_context = context();
        generation_context.summary_context_sha256 = Some(summary_context.sha256());
        generation_context.summary_meeting_context = Some(summary_context);
        let (_, path, _) = capture_snapshot(
            temporary.path(),
            "gen_context_tamper",
            "meeting-1",
            &resolved(),
            generation_context,
        )
        .unwrap();
        let mut value: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        value["generation_context"]["summary_meeting_context"]["verified_meeting_facts"]
            ["meeting_name"] = Value::String("Tampered name".to_owned());
        fs::write(&path, serde_json::to_vec_pretty(&value).unwrap()).unwrap();

        assert!(
            read_snapshot(temporary.path(), "meeting-1", "gen_context_tamper")
                .unwrap_err()
                .contains("fingerprint")
        );
    }
}

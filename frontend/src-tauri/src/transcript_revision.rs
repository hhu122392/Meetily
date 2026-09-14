//! 「增强（重新转写）」的可见、可退支持。
//!
//! 背景：重新转写覆盖前会把旧转写写到会议文件夹的
//! `transcripts-before-retranscription-<时间戳>.json`，但过去没人读它 ——
//! 界面上看不到改了什么，也没法回退。本模块补上三件事：
//!   1. 列出这场会议已有的备份；
//!   2. 计算「改前 → 改后」的逐段差异（含字级差异区间）；
//!   3. 按备份恢复转写（数据库 + transcripts.json + metadata.json）。

use crate::transcript_file_store;
use crate::state::AppState;
use anyhow::{anyhow, Context, Result};
use log::info;
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use std::path::{Path, PathBuf};
use tauri::State;

/// 重新转写备份文件名的前缀/后缀（由 `audio/retranscription.rs` 写入）
pub const BACKUP_FILE_PREFIX: &str = "transcripts-before-retranscription-";
/// 恢复前自动留下的备份前缀（保证"恢复"这一步本身也能再回退）
pub const RESTORE_BACKUP_FILE_PREFIX: &str = "transcripts-before-restore-";
/// "按术语核对"（纯文本纠正）前自动留下的备份前缀
pub const CORRECTION_BACKUP_FILE_PREFIX: &str = "transcripts-before-correction-";
/// "AI 校对"（LLM 逐段找错）写回前自动留下的备份前缀
pub const PROOFREAD_BACKUP_FILE_PREFIX: &str = "transcripts-before-proofread-";
pub const BACKUP_FILE_SUFFIX: &str = ".json";

/// 字级差异计算的长度上限（超长文本退化为"整段替换"，避免 O(n*m) 爆炸）
const MAX_DIFF_CHARS: usize = 3000;
/// 规则层一条片段最多拆成几处"逐条候选"；超过就整段给一条
const MAX_GRANULAR_RULE_EDITS: usize = 5;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TranscriptBackupInfo {
    pub file: String,
    pub total_segments: usize,
    /// 备份文件自身记录的时间戳（旧格式可能没有）
    pub created_at: Option<String>,
    /// 文件系统的最后修改时间（用于排序，不依赖 JSON 内容）
    pub saved_at: Option<String>,
    pub transcription_provider: Option<String>,
    pub transcription_model: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TranscriptBackupsResponse {
    /// 这场会议"上次用的"转写引擎/模型（读 metadata.json，重新转写会覆盖它）
    pub last_used_provider: Option<String>,
    pub last_used_model: Option<String>,
    /// 录音时长（秒），用于估算换模型要跑多久
    pub audio_duration_seconds: Option<f64>,
    pub backups: Vec<TranscriptBackupInfo>,
}

/// 备份文件里的单段（字段与 transcripts.json / 备份 JSON 对齐）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackupSegment {
    pub id: Option<String>,
    pub text: String,
    pub timestamp: Option<String>,
    pub audio_start_time: Option<f64>,
    pub audio_end_time: Option<f64>,
    pub duration: Option<f64>,
    /// 以下四列目前基本为空，但老备份里可能没有，按 NULL 处理
    pub speaker: Option<String>,
    pub summary: Option<String>,
    pub action_items: Option<String>,
    pub key_points: Option<String>,
}

#[derive(Debug, Clone, FromRow)]
pub struct StoredTranscriptRow {
    pub id: String,
    pub transcript: String,
    pub timestamp: String,
    pub audio_start_time: Option<f64>,
    pub audio_end_time: Option<f64>,
    pub duration: Option<f64>,
    pub speaker: Option<String>,
    pub summary: Option<String>,
    pub action_items: Option<String>,
    pub key_points: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TranscriptDiffKind {
    Unchanged,
    Changed,
    Added,
    Removed,
}

/// 一段文本里的差异区间（旧文本 / 新文本各一段）
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct TextSpanDifference {
    pub before_start: usize,
    pub before_length: usize,
    pub after_start: usize,
    pub after_length: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TranscriptDiffItem {
    pub kind: TranscriptDiffKind,
    pub before_id: Option<String>,
    pub after_id: Option<String>,
    pub before_text: Option<String>,
    pub after_text: Option<String>,
    pub audio_start_time: Option<f64>,
    pub audio_end_time: Option<f64>,
    pub differences: Vec<TextSpanDifference>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TranscriptDiffSummary {
    pub unchanged: usize,
    pub changed: usize,
    pub added: usize,
    pub removed: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TranscriptDiffResponse {
    pub backup_file: String,
    pub backup_created_at: Option<String>,
    pub before_segments: usize,
    pub after_segments: usize,
    pub summary: TranscriptDiffSummary,
    pub items: Vec<TranscriptDiffItem>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TranscriptRestoreResponse {
    pub file_sync_pending: bool,
    pub backup_file: String,
    pub restored_segments: usize,
    pub previous_segments: usize,
    pub restored_provider: Option<String>,
    pub restored_model: Option<String>,
    /// 恢复前把"当前版本"另存的备份文件名（可再恢复回来）
    pub pre_restore_backup: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SuggestedEdit {
    pub segment_id: String,
    pub segment_index: usize,
    pub audio_start_time: Option<f64>,
    pub original: String,
    pub suggested: String,
    pub start_char: usize,
    pub end_char: usize,
    /// rules / context / homophone / term / typo
    pub reason: String,
    pub confidence: String,
    pub segment_text: String,
    pub proposed_text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TextCorrectionResponse {
    pub file_sync_pending: bool,
    /// true = 已经写回数据库；false = 只是预览（dry_run）
    pub applied: bool,
    pub total_segments: usize,
    pub changed_segments: usize,
    /// 命中的规则词典 id（去重）
    pub applied_rules: Vec<String>,
    /// 上下文里生效的参会人/术语数量，便于判断"是不是没配对术语"
    pub context_person_count: usize,
    pub context_term_count: usize,
    pub backup_file: Option<String>,
    pub summary: TranscriptDiffSummary,
    pub items: Vec<TranscriptDiffItem>,
    /// 规则/术语层给出的"原词 → 新词"逐条改动（前端统一成候选清单，勾选后才写回）
    pub edits: Vec<SuggestedEdit>,
}

fn join_folder(meeting_folder_path: &str) -> Result<PathBuf> {
    let path = PathBuf::from(meeting_folder_path);
    if !path.is_dir() {
        return Err(anyhow!(
            "Meeting folder does not exist: {}",
            path.display()
        ));
    }
    Ok(path)
}

fn is_backup_file(name: &str) -> bool {
    (name.starts_with(BACKUP_FILE_PREFIX)
        || name.starts_with(RESTORE_BACKUP_FILE_PREFIX)
        || name.starts_with(CORRECTION_BACKUP_FILE_PREFIX)
        || name.starts_with(PROOFREAD_BACKUP_FILE_PREFIX))
        && name.ends_with(BACKUP_FILE_SUFFIX)
}

fn read_json_value(path: &Path) -> Result<serde_json::Value> {
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("Failed to read {}", path.display()))?;
    serde_json::from_str(&raw).with_context(|| format!("Invalid JSON in {}", path.display()))
}

/// 解析备份文件（容忍缺字段：老备份只有 id/text/timestamp/时间戳，没有 speaker 等）
pub fn parse_backup_segments(value: &serde_json::Value) -> Vec<BackupSegment> {
    let Some(segments) = value.get("segments").and_then(|v| v.as_array()) else {
        return Vec::new();
    };

    segments
        .iter()
        .filter_map(|item| {
            let text = item
                .get("text")
                .and_then(|v| v.as_str())
                .map(str::to_string)?;
            Some(BackupSegment {
                id: item.get("id").and_then(|v| v.as_str()).map(str::to_string),
                text,
                timestamp: item
                    .get("timestamp")
                    .and_then(|v| v.as_str())
                    .map(str::to_string),
                audio_start_time: item.get("audio_start_time").and_then(|v| v.as_f64()),
                audio_end_time: item.get("audio_end_time").and_then(|v| v.as_f64()),
                duration: item.get("duration").and_then(|v| v.as_f64()),
                speaker: item.get("speaker").and_then(|v| v.as_str()).map(str::to_string),
                summary: item.get("summary").and_then(|v| v.as_str()).map(str::to_string),
                action_items: item
                    .get("action_items")
                    .and_then(|v| v.as_str())
                    .map(str::to_string),
                key_points: item
                    .get("key_points")
                    .and_then(|v| v.as_str())
                    .map(str::to_string),
            })
        })
        .collect()
}

fn normalize_text(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join("")
}

/// 计算两段文本的字级差异区间（LCS 回溯；超长退化为整段替换）
pub fn char_diff_spans(before: &str, after: &str) -> Vec<TextSpanDifference> {
    if before == after {
        return Vec::new();
    }

    let before_chars: Vec<char> = before.chars().collect();
    let after_chars: Vec<char> = after.chars().collect();
    let n = before_chars.len();
    let m = after_chars.len();

    if n == 0 || m == 0 || n > MAX_DIFF_CHARS || m > MAX_DIFF_CHARS || n.saturating_mul(m) > 4_000_000
    {
        return vec![TextSpanDifference {
            before_start: 0,
            before_length: n,
            after_start: 0,
            after_length: m,
        }];
    }

    // LCS 长度表
    let mut table = vec![vec![0u32; m + 1]; n + 1];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            table[i][j] = if before_chars[i] == after_chars[j] {
                table[i + 1][j + 1] + 1
            } else {
                table[i + 1][j].max(table[i][j + 1])
            };
        }
    }

    // 回溯，收集不匹配的连续区间
    let mut spans: Vec<TextSpanDifference> = Vec::new();
    let (mut i, mut j) = (0usize, 0usize);
    let mut current: Option<TextSpanDifference> = None;

    while i < n && j < m {
        if before_chars[i] == after_chars[j] {
            if let Some(span) = current.take() {
                spans.push(span);
            }
            i += 1;
            j += 1;
        } else if table[i + 1][j] >= table[i][j + 1] {
            let span = current.get_or_insert(TextSpanDifference {
                before_start: i,
                before_length: 0,
                after_start: j,
                after_length: 0,
            });
            span.before_length += 1;
            i += 1;
        } else {
            let span = current.get_or_insert(TextSpanDifference {
                before_start: i,
                before_length: 0,
                after_start: j,
                after_length: 0,
            });
            span.after_length += 1;
            j += 1;
        }
    }
    if i < n {
        let span = current.get_or_insert(TextSpanDifference {
            before_start: i,
            before_length: 0,
            after_start: j,
            after_length: 0,
        });
        span.before_length += n - i;
    }
    if j < m {
        let span = current.get_or_insert(TextSpanDifference {
            before_start: i,
            before_length: 0,
            after_start: j,
            after_length: 0,
        });
        span.after_length += m - j;
    }
    if let Some(span) = current.take() {
        spans.push(span);
    }

    spans
}

/// 按字符（不是字节）截取文本片段：`char_diff_spans` 的偏移也是按字符算的
fn slice_chars(text: &str, start: usize, length: usize) -> String {
    text.chars().skip(start).take(length).collect()
}

fn segments_overlap(
    before: Option<(f64, f64)>,
    after: Option<(f64, f64)>,
) -> bool {
    match (before, after) {
        (Some((b_start, b_end)), Some((a_start, a_end))) => {
            let overlap = b_end.min(a_end) - b_start.max(a_start);
            let tolerance = (b_end - b_start).max(a_end - a_start).max(1.0) * 0.2;
            overlap > 0.0 || (b_start - a_start).abs() <= tolerance
        }
        _ => true,
    }
}

/// 把「改前」「改后」两组片段按时间对齐，产出逐段差异
pub fn build_diff_items(
    before: &[BackupSegment],
    after: &[StoredTranscriptRow],
) -> Vec<TranscriptDiffItem> {
    let mut sorted_before: Vec<(usize, &BackupSegment)> = before.iter().enumerate().collect();
    sorted_before.sort_by(|a, b| {
        a.1.audio_start_time
            .unwrap_or(0.0)
            .partial_cmp(&b.1.audio_start_time.unwrap_or(0.0))
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    let mut sorted_after: Vec<(usize, &StoredTranscriptRow)> = after.iter().enumerate().collect();
    sorted_after.sort_by(|a, b| {
        a.1.audio_start_time
            .unwrap_or(0.0)
            .partial_cmp(&b.1.audio_start_time.unwrap_or(0.0))
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    let mut used_after = vec![false; sorted_after.len()];
    let mut items: Vec<TranscriptDiffItem> = Vec::new();

    for (_, old) in &sorted_before {
        let old_range = match (old.audio_start_time, old.audio_end_time) {
            (Some(start), Some(end)) => Some((start, end)),
            _ => None,
        };

        // 找最近的未匹配新片段
        let mut best: Option<usize> = None;
        let mut best_distance = f64::MAX;
        for (candidate_index, (_, new)) in sorted_after.iter().enumerate() {
            if used_after[candidate_index] {
                continue;
            }
            let new_range = match (new.audio_start_time, new.audio_end_time) {
                (Some(start), Some(end)) => Some((start, end)),
                _ => None,
            };
            if !segments_overlap(old_range, new_range) {
                continue;
            }
            let distance = ((old.audio_start_time.unwrap_or(0.0))
                - (new.audio_start_time.unwrap_or(0.0)))
            .abs();
            if distance < best_distance {
                best_distance = distance;
                best = Some(candidate_index);
            }
        }

        match best {
            Some(candidate_index) => {
                used_after[candidate_index] = true;
                let new = sorted_after[candidate_index].1;
                let same_text = normalize_text(&old.text) == normalize_text(&new.transcript);
                items.push(TranscriptDiffItem {
                    kind: if same_text {
                        TranscriptDiffKind::Unchanged
                    } else {
                        TranscriptDiffKind::Changed
                    },
                    before_id: old.id.clone(),
                    after_id: Some(new.id.clone()),
                    before_text: Some(old.text.clone()),
                    after_text: Some(new.transcript.clone()),
                    audio_start_time: new
                        .audio_start_time
                        .or(old.audio_start_time),
                    audio_end_time: new.audio_end_time.or(old.audio_end_time),
                    differences: if same_text {
                        Vec::new()
                    } else {
                        char_diff_spans(&old.text, &new.transcript)
                    },
                });
            }
            None => items.push(TranscriptDiffItem {
                kind: TranscriptDiffKind::Removed,
                before_id: old.id.clone(),
                after_id: None,
                before_text: Some(old.text.clone()),
                after_text: None,
                audio_start_time: old.audio_start_time,
                audio_end_time: old.audio_end_time,
                differences: Vec::new(),
            }),
        }
    }

    for (candidate_index, (_, new)) in sorted_after.iter().enumerate() {
        if used_after[candidate_index] {
            continue;
        }
        items.push(TranscriptDiffItem {
            kind: TranscriptDiffKind::Added,
            before_id: None,
            after_id: Some(new.id.clone()),
            before_text: None,
            after_text: Some(new.transcript.clone()),
            audio_start_time: new.audio_start_time,
            audio_end_time: new.audio_end_time,
            differences: Vec::new(),
        });
    }

    items.sort_by(|a, b| {
        a.audio_start_time
            .unwrap_or(0.0)
            .partial_cmp(&b.audio_start_time.unwrap_or(0.0))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    items
}

pub fn summarize(items: &[TranscriptDiffItem]) -> TranscriptDiffSummary {
    let mut summary = TranscriptDiffSummary::default();
    for item in items {
        match item.kind {
            TranscriptDiffKind::Unchanged => summary.unchanged += 1,
            TranscriptDiffKind::Changed => summary.changed += 1,
            TranscriptDiffKind::Added => summary.added += 1,
            TranscriptDiffKind::Removed => summary.removed += 1,
        }
    }
    summary
}

fn read_metadata(folder: &Path) -> Option<serde_json::Value> {
    read_json_value(&folder.join("metadata.json")).ok()
}

fn metadata_string(metadata: &Option<serde_json::Value>, key: &str) -> Option<String> {
    metadata
        .as_ref()
        .and_then(|value| value.get(key))
        .and_then(|value| value.as_str())
        .map(str::to_string)
}

fn list_backups(folder: &Path) -> Result<Vec<TranscriptBackupInfo>> {
    let mut backups: Vec<TranscriptBackupInfo> = Vec::new();
    for entry in std::fs::read_dir(folder)? {
        let entry = entry?;
        let file_name = entry.file_name().to_string_lossy().to_string();
        if !entry.file_type()?.is_file() || !is_backup_file(&file_name) {
            continue;
        }
        let path = entry.path();
        let saved_at = entry
            .metadata()
            .ok()
            .and_then(|meta| meta.modified().ok())
            .map(|time| chrono::DateTime::<chrono::Utc>::from(time).to_rfc3339());

        let parsed = read_json_value(&path).ok();
        let total_segments = parsed
            .as_ref()
            .and_then(|value| value.get("total_segments"))
            .and_then(|value| value.as_u64())
            .map(|value| value as usize)
            .unwrap_or_else(|| parsed.as_ref().map(parse_backup_segments).unwrap_or_default().len());

        backups.push(TranscriptBackupInfo {
            file: file_name,
            total_segments,
            created_at: parsed
                .as_ref()
                .and_then(|value| value.get("created_at"))
                .and_then(|value| value.as_str())
                .map(str::to_string),
            saved_at,
            transcription_provider: parsed.as_ref().and_then(|value| transcript_file_store::backup_source(value).0),
            transcription_model: parsed.as_ref().and_then(|value| transcript_file_store::backup_source(value).1),
        });
    }

    backups.sort_by(|a, b| b.saved_at.cmp(&a.saved_at));
    Ok(backups)
}

fn resolve_backup_path(folder: &Path, backup_file: &str) -> Result<PathBuf> {
    if !is_backup_file(backup_file) || backup_file.contains('/') || backup_file.contains('\\') {
        return Err(anyhow!("Invalid backup file name: {}", backup_file));
    }
    let path = folder.join(backup_file);
    if !path.is_file() {
        return Err(anyhow!("Backup file not found: {}", backup_file));
    }
    Ok(path)
}

pub(crate) async fn load_current_segments(
    pool: &sqlx::SqlitePool,
    meeting_id: &str,
) -> Result<Vec<StoredTranscriptRow>> {
    let rows = sqlx::query_as::<_, StoredTranscriptRow>(
        "SELECT id, transcript, timestamp, audio_start_time, audio_end_time, duration,
                speaker, summary, action_items, key_points
         FROM transcripts WHERE meeting_id = ?
         ORDER BY COALESCE(audio_start_time, 0.0), timestamp",
    )
    .bind(meeting_id)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

/// 列出备份 + 这场会议"上次用的"引擎/模型
#[tauri::command]
pub async fn api_list_transcript_backups(
    meeting_folder_path: String,
) -> Result<TranscriptBackupsResponse, String> {
    let folder = join_folder(&meeting_folder_path).map_err(|error| error.to_string())?;
    let metadata = read_metadata(&folder);
    let backups = list_backups(&folder).map_err(|error| error.to_string())?;
    Ok(TranscriptBackupsResponse {
        last_used_provider: metadata_string(&metadata, "transcription_provider"),
        last_used_model: metadata_string(&metadata, "transcription_model"),
        audio_duration_seconds: metadata
            .as_ref()
            .and_then(|value| value.get("duration_seconds"))
            .and_then(|value| value.as_f64()),
        backups,
    })
}

/// 计算「备份（改前）→ 当前转写（改后）」的逐段差异
#[tauri::command]
pub async fn api_get_transcript_revision_diff(
    app_state: State<'_, AppState>,
    meeting_id: String,
    meeting_folder_path: String,
    backup_file: String,
) -> Result<TranscriptDiffResponse, String> {
    let folder = join_folder(&meeting_folder_path).map_err(|error| error.to_string())?;
    let backup_path =
        resolve_backup_path(&folder, &backup_file).map_err(|error| error.to_string())?;
    let backup_value = read_json_value(&backup_path).map_err(|error| error.to_string())?;
    let before = parse_backup_segments(&backup_value);
    let after = load_current_segments(app_state.db_manager.pool(), &meeting_id)
        .await
        .map_err(|error| error.to_string())?;

    let items = build_diff_items(&before, &after);
    let summary = summarize(&items);

    Ok(TranscriptDiffResponse {
        backup_file,
        backup_created_at: backup_value
            .get("created_at")
            .and_then(|value| value.as_str())
            .map(str::to_string),
        before_segments: before.len(),
        after_segments: after.len(),
        summary,
        items,
    })
}

/// 按备份恢复转写（数据库 + transcripts.json + metadata.json）
#[tauri::command]
pub async fn api_restore_transcript_revision(
    app_state: State<'_, AppState>,
    meeting_id: String,
    meeting_folder_path: String,
    backup_file: String,
) -> Result<TranscriptRestoreResponse, String> {
    let folder = join_folder(&meeting_folder_path).map_err(|error| error.to_string())?;
    let backup_path =
        resolve_backup_path(&folder, &backup_file).map_err(|error| error.to_string())?;
    let backup_value = read_json_value(&backup_path).map_err(|error| error.to_string())?;
    let segments = parse_backup_segments(&backup_value);
    if segments.is_empty() {
        return Err("Backup file has no segments to restore".to_string());
    }

    let pool = app_state.db_manager.pool();
    let _write_guard = transcript_file_store::WRITE_LOCK.lock().await;
    transcript_file_store::ensure_ready(pool, &meeting_id).await?;
    let previous = load_current_segments(pool, &meeting_id)
        .await
        .map_err(|error| error.to_string())?;

    // 恢复本身也要可回退：先把"当前版本"另存一份备份（写不出去就中止）
    let pre_restore_backup =
        write_current_backup(&folder, &meeting_id, &previous).map_err(|error| error.to_string())?;

    let mut conn = pool.acquire().await.map_err(|error| error.to_string())?;
    let mut tx = sqlx::Connection::begin(&mut *conn)
        .await
        .map_err(|error| error.to_string())?;

    sqlx::query("DELETE FROM transcripts WHERE meeting_id = ?")
        .bind(&meeting_id)
        .execute(&mut *tx)
        .await
        .map_err(|error| error.to_string())?;

    for (index, segment) in segments.iter().enumerate() {
        let id = segment
            .id
            .clone()
            .unwrap_or_else(|| format!("transcript-restored-{}-{}", meeting_id, index));
        let start = segment.audio_start_time.unwrap_or(0.0);
        let end = segment.audio_end_time.unwrap_or(start);
        let duration = segment.duration.unwrap_or((end - start).max(0.0));
        let timestamp = segment
            .timestamp
            .clone()
            .unwrap_or_else(|| chrono::Utc::now().to_rfc3339());

        sqlx::query(
            "INSERT INTO transcripts (id, meeting_id, transcript, timestamp, audio_start_time, audio_end_time, duration,
                                      speaker, summary, action_items, key_points)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&id)
        .bind(&meeting_id)
        .bind(&segment.text)
        .bind(&timestamp)
        .bind(segment.audio_start_time)
        .bind(segment.audio_end_time)
        .bind(Some(duration))
        .bind(segment.speaker.clone())
        .bind(segment.summary.clone())
        .bind(segment.action_items.clone())
        .bind(segment.key_points.clone())
        .execute(&mut *tx)
        .await
        .map_err(|error| error.to_string())?;
    }

    let (restored_provider, restored_model) = transcript_file_store::backup_source(&backup_value);
    transcript_file_store::stage(&mut tx, &meeting_id, &folder, serde_json::json!({
        "restored_at": chrono::Utc::now().to_rfc3339(), "restored_from": backup_file,
        "transcription_provider": restored_provider, "transcription_model": restored_model,
    })).await.map_err(|error| error.to_string())?;
    tx.commit().await.map_err(|error| error.to_string())?;
    let file_sync_pending = transcript_file_store::finish(pool, &meeting_id).await;

    info!(
        "Restored {} transcript segments for meeting {} from {}",
        segments.len(),
        meeting_id,
        backup_file
    );

    Ok(TranscriptRestoreResponse {
        file_sync_pending,
        backup_file,
        restored_segments: segments.len(),
        previous_segments: previous.len(),
        restored_provider,
        restored_model,
        pre_restore_backup: pre_restore_backup.clone(),
    })
}

/// 把当前转写另存为 `<prefix><时间戳>.json`（恢复/文本纠正前调用，保证动作可回退）
pub(crate) fn write_rows_backup(
    folder: &Path,
    meeting_id: &str,
    rows: &[StoredTranscriptRow],
    file_prefix: &str,
    kind: &str,
) -> Result<Option<String>> {
    if rows.is_empty() {
        return Ok(None);
    }

    let metadata = transcript_file_store::read_metadata(folder)?;
    let json = serde_json::json!({
        "version": "1.1",
        "transcription_provider": metadata.get("transcription_provider"),
        "transcription_model": metadata.get("transcription_model"),
        "kind": kind,
        "meeting_id": meeting_id,
        "created_at": chrono::Utc::now().to_rfc3339(),
        "total_segments": rows.len(),
        "segments": rows.iter().enumerate().map(|(index, row)| serde_json::json!({
            "id": row.id,
            "text": row.transcript,
            "timestamp": row.timestamp,
            "audio_start_time": row.audio_start_time,
            "audio_end_time": row.audio_end_time,
            "duration": row.duration,
            "speaker": row.speaker,
            "summary": row.summary,
            "action_items": row.action_items,
            "key_points": row.key_points,
            "sequence_id": index,
        })).collect::<Vec<_>>()
    });

    let file_name = transcript_file_store::write_new_backup(folder, file_prefix, &json)?;
    Ok(Some(file_name))
}

fn write_current_backup(
    folder: &Path,
    meeting_id: &str,
    rows: &[StoredTranscriptRow],
) -> Result<Option<String>> {
    write_rows_backup(
        folder,
        meeting_id,
        rows,
        RESTORE_BACKUP_FILE_PREFIX,
        "pre-restore-backup",
    )
}

/// 「按术语核对」：只跑文本层（规则词典 + 会议上下文的人名/术语归一化），
/// **不碰音频、不重新识别**，所以是秒级的。`dry_run=true` 只返回改动清单。
#[tauri::command]
pub async fn api_apply_meeting_text_corrections(
    app_state: State<'_, AppState>,
    meeting_id: String,
    meeting_folder_path: String,
    dry_run: bool,
) -> Result<TextCorrectionResponse, String> {
    use crate::meeting_context::{load_recognition_context, RecognitionContextSelection};
    use std::collections::BTreeSet;

    let folder = join_folder(&meeting_folder_path).map_err(|error| error.to_string())?;
    let context = load_recognition_context(&folder, RecognitionContextSelection::Current)
        .map_err(|error| error.to_string())?;

    let pool = app_state.db_manager.pool();
    let _write_guard = transcript_file_store::WRITE_LOCK.lock().await;
    transcript_file_store::ensure_ready(pool, &meeting_id).await?;
    let rows = load_current_segments(pool, &meeting_id)
        .await
        .map_err(|error| error.to_string())?;
    if rows.is_empty() {
        return Err("这场会议还没有转写内容".to_string());
    }

    let mut items: Vec<TranscriptDiffItem> = Vec::new();
    let mut rule_ids: BTreeSet<String> = BTreeSet::new();
    let mut corrected: Vec<(String, String)> = Vec::new();
    // 逐条"原词 → 新词"候选：前端和 AI 校对的候选放进同一个清单，勾选后一次写回
    let mut edits: Vec<SuggestedEdit> = Vec::new();

    for (segment_index, row) in rows.iter().enumerate() {
        let (after_rules, rules) = crate::transcript_term_correction::correct_terms(&row.transcript);
        let rules_changed = after_rules != row.transcript;
        for rule in rules {
            rule_ids.insert(rule.to_string());
        }
        let normalized = match context.as_ref() {
            Some(context) => context.normalize_transcript(&after_rules),
            None => after_rules,
        };
        let changed = normalized != row.transcript;
        let spans = if changed {
            char_diff_spans(&row.transcript, &normalized)
        } else {
            Vec::new()
        };
        if changed {
            corrected.push((row.id.clone(), normalized.clone()));
            // 来源标签：规则词典命中 → rules；只有上下文（人名/术语）归一 → context
            let reason = if rules_changed { "rules" } else { "context" };
            let granular = !spans.is_empty()
                && spans.len() <= MAX_GRANULAR_RULE_EDITS
                && spans
                    .iter()
                    .all(|span| span.before_length > 0 && span.after_length > 0);
            if granular {
                for span in &spans {
                    let original = slice_chars(&row.transcript, span.before_start, span.before_length);
                    let suggested = slice_chars(&normalized, span.after_start, span.after_length);
                    if original.is_empty() || suggested.is_empty() || original == suggested {
                        continue;
                    }
                    edits.push(SuggestedEdit {
                        segment_id: row.id.clone(),
                        segment_index,
                        audio_start_time: row.audio_start_time,
                        proposed_text: crate::transcript_text_edit::apply_text_edits(
                            &row.transcript,
                            &[crate::transcript_text_edit::TextEdit {
                                expected_text: &row.transcript, original: &original, suggested: &suggested,
                                start_char: span.before_start, end_char: span.before_start + span.before_length,
                            }],
                        ).map_err(|error| error.to_string())?,
                        start_char: span.before_start,
                        end_char: span.before_start + span.before_length,
                        original,
                        suggested,
                        reason: reason.to_string(),
                        confidence: "high".to_string(),
                        segment_text: row.transcript.clone(),
                    });
                }
            } else {
                // 插字/删字或改动太碎时整段给一条，避免清单被拆成几十条
                edits.push(SuggestedEdit {
                    segment_id: row.id.clone(),
                    segment_index,
                    audio_start_time: row.audio_start_time,
                    original: row.transcript.clone(),
                    suggested: normalized.clone(),
                    start_char: 0,
                    end_char: row.transcript.chars().count(),
                    reason: reason.to_string(),
                    confidence: "high".to_string(),
                    segment_text: row.transcript.clone(),
                    proposed_text: normalized.clone(),
                });
            }
        }
        items.push(TranscriptDiffItem {
            kind: if changed {
                TranscriptDiffKind::Changed
            } else {
                TranscriptDiffKind::Unchanged
            },
            before_id: Some(row.id.clone()),
            after_id: Some(row.id.clone()),
            before_text: Some(row.transcript.clone()),
            after_text: Some(normalized.clone()),
            audio_start_time: row.audio_start_time,
            audio_end_time: row.audio_end_time,
            differences: spans,
        });
    }

    let summary = summarize(&items);
    let applied_rules: Vec<String> = rule_ids.into_iter().collect();
    let (context_person_count, context_term_count) = match context.as_ref() {
        Some(context) => {
            let diagnostics = context.diagnostics();
            (diagnostics.canonical_name_count, diagnostics.canonical_term_count)
        }
        None => (0, 0),
    };

    if dry_run || summary.changed == 0 {
        return Ok(TextCorrectionResponse {
            file_sync_pending: false,
            applied: false,
            total_segments: rows.len(),
            changed_segments: summary.changed,
            applied_rules,
            context_person_count,
            context_term_count,
            backup_file: None,
            summary,
            items,
            edits,
        });
    }

    // 先留备份（写不出去就中止，绝不静默改文本）
    let backup_file = write_rows_backup(
        &folder,
        &meeting_id,
        &rows,
        CORRECTION_BACKUP_FILE_PREFIX,
        "pre-correction-backup",
    )
    .map_err(|error| error.to_string())?;

    let mut conn = pool.acquire().await.map_err(|error| error.to_string())?;
    let mut tx = sqlx::Connection::begin(&mut *conn)
        .await
        .map_err(|error| error.to_string())?;
    for (id, new_text) in &corrected {
        let original = rows.iter().find(|row| &row.id == id).ok_or_else(|| "proofread_source_changed".to_string())?;
        let updated = sqlx::query("UPDATE transcripts SET transcript = ? WHERE id = ? AND meeting_id = ? AND transcript = ?")
            .bind(new_text)
            .bind(id)
            .bind(&meeting_id)
            .bind(&original.transcript)
            .execute(&mut *tx)
            .await
            .map_err(|error| error.to_string())?;
        if updated.rows_affected() != 1 { return Err("proofread_source_changed".into()); }
    }
    transcript_file_store::stage(&mut tx, &meeting_id, &folder, serde_json::json!({})).await.map_err(|error| error.to_string())?;
    tx.commit().await.map_err(|error| error.to_string())?;
    let file_sync_pending = transcript_file_store::finish(pool, &meeting_id).await;

    info!(
        "Applied text corrections to {} segment(s) of meeting {} (rules: {:?})",
        summary.changed, meeting_id, applied_rules
    );

    Ok(TextCorrectionResponse {
        file_sync_pending,
        applied: true,
        total_segments: rows.len(),
        changed_segments: summary.changed,
        applied_rules,
        context_person_count,
        context_term_count,
        backup_file,
        summary,
        items,
        edits,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn backup(text: &str, start: f64, end: f64) -> BackupSegment {
        BackupSegment {
            id: Some(format!("old-{}", start)),
            text: text.to_string(),
            timestamp: Some("00:00:01".to_string()),
            audio_start_time: Some(start),
            audio_end_time: Some(end),
            duration: Some(end - start),
            speaker: None,
            summary: None,
            action_items: None,
            key_points: None,
        }
    }

    fn stored(text: &str, start: f64, end: f64) -> StoredTranscriptRow {
        StoredTranscriptRow {
            id: format!("new-{}", start),
            transcript: text.to_string(),
            timestamp: "00:00:01".to_string(),
            audio_start_time: Some(start),
            audio_end_time: Some(end),
            duration: Some(end - start),
            speaker: None,
            summary: None,
            action_items: None,
            key_points: None,
        }
    }

    #[test]
    fn char_diff_reports_replacement_span() {
        let spans = char_diff_spans("洞学里的人", "洞穴里的人");
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].before_start, 1);
        assert_eq!(spans[0].before_length, 1);
        assert_eq!(spans[0].after_start, 1);
        assert_eq!(spans[0].after_length, 1);
    }

    #[test]
    fn char_diff_returns_nothing_for_identical_text() {
        assert!(char_diff_spans("完全一样的一句话", "完全一样的一句话").is_empty());
    }

    #[test]
    fn char_diff_handles_insertions_and_deletions() {
        let inserted = char_diff_spans("你好世界", "你好，世界");
        assert_eq!(inserted.len(), 1);
        assert_eq!(inserted[0].before_length, 0);
        assert_eq!(inserted[0].after_length, 1);

        let deleted = char_diff_spans("你好，世界", "你好世界");
        assert_eq!(deleted.len(), 1);
        assert_eq!(deleted[0].before_length, 1);
        assert_eq!(deleted[0].after_length, 0);
    }

    #[test]
    fn diff_marks_unchanged_changed_and_boundary_shifts() {
        let before = vec![
            backup("第一段内容", 0.0, 10.0),
            backup("第二段内容", 10.0, 20.0),
        ];
        let after = vec![
            stored("第一段内容", 0.0, 10.0),
            stored("第二段改过的内容", 10.0, 20.0),
        ];
        let items = build_diff_items(&before, &after);
        let summary = summarize(&items);
        assert_eq!(summary.unchanged, 1);
        assert_eq!(summary.changed, 1);
        assert_eq!(summary.added, 0);
        assert_eq!(summary.removed, 0);
        let changed = items
            .iter()
            .find(|item| item.kind == TranscriptDiffKind::Changed)
            .expect("changed item");
        assert!(!changed.differences.is_empty());
    }

    #[test]
    fn diff_reports_added_and_removed_segments() {
        let before = vec![backup("被删掉的一段", 0.0, 5.0), backup("保留的一段", 5.0, 10.0)];
        let after = vec![stored("保留的一段", 5.0, 10.0), stored("新加的一段", 10.0, 15.0)];
        let items = build_diff_items(&before, &after);
        let summary = summarize(&items);
        assert_eq!(summary.unchanged, 1);
        assert_eq!(summary.added, 1);
        assert_eq!(summary.removed, 1);
    }

    #[test]
    fn parse_backup_segments_tolerates_missing_fields() {
        let value = serde_json::json!({
            "version": "1.0",
            "transcription_provider": "sensevoice",
            "total_segments": 2,
            "segments": [
                { "id": "a", "text": "第一段", "timestamp": "00:00:01", "audio_start_time": 1.0, "audio_end_time": 2.0, "duration": 1.0 },
                { "text": "只有文本的旧段" }
            ]
        });
        let segments = parse_backup_segments(&value);
        assert_eq!(segments.len(), 2);
        assert_eq!(segments[1].text, "只有文本的旧段");
        assert!(segments[1].audio_start_time.is_none());
        assert!(segments[1].summary.is_none());
    }

    #[test]
    fn backup_file_name_validation_rejects_paths() {
        let folder = std::env::temp_dir();
        assert!(resolve_backup_path(&folder, "../evil.json").is_err());
        assert!(resolve_backup_path(&folder, "transcripts.json").is_err());
    }
}

/**
 * 转写「增强」的可见/可退支持（前端侧薄封装）。
 *
 * 后端命令：`api_list_transcript_backups` / `api_get_transcript_revision_diff` /
 * `api_restore_transcript_revision`（见 `src-tauri/src/transcript_revision.rs`）。
 * 备份文件由重新转写写入会议文件夹：`transcripts-before-retranscription-<时间戳>.json`；
 * 恢复前还会自动写一份 `transcripts-before-restore-<时间戳>.json`，保证恢复本身可再回退。
 */

import { invoke } from '@tauri-apps/api/core';

export type TranscriptDiffKind = 'unchanged' | 'changed' | 'added' | 'removed';

export interface TextSpanDifference {
  before_start: number;
  before_length: number;
  after_start: number;
  after_length: number;
}

export interface TranscriptDiffItem {
  kind: TranscriptDiffKind;
  before_id: string | null;
  after_id: string | null;
  before_text: string | null;
  after_text: string | null;
  audio_start_time: number | null;
  audio_end_time: number | null;
  differences: TextSpanDifference[];
}

export interface TranscriptDiffSummary {
  unchanged: number;
  changed: number;
  added: number;
  removed: number;
}

export interface TranscriptDiffResponse {
  backup_file: string;
  backup_created_at: string | null;
  before_segments: number;
  after_segments: number;
  summary: TranscriptDiffSummary;
  items: TranscriptDiffItem[];
}

export interface TranscriptBackupInfo {
  file: string;
  total_segments: number;
  created_at: string | null;
  saved_at: string | null;
  transcription_provider: string | null;
  transcription_model: string | null;
}

export interface TranscriptBackupsResponse {
  last_used_provider: string | null;
  last_used_model: string | null;
  audio_duration_seconds: number | null;
  backups: TranscriptBackupInfo[];
}

export interface TranscriptRestoreResponse {
  file_sync_pending: boolean;
  backup_file: string;
  restored_segments: number;
  previous_segments: number;
  restored_provider: string | null;
  restored_model: string | null;
  pre_restore_backup: string | null;
}

export interface TextCorrectionResponse {
  file_sync_pending: boolean;
  /** true = 已经写回数据库；false = 只是预览（dry_run） */
  applied: boolean;
  total_segments: number;
  changed_segments: number;
  applied_rules: string[];
  context_person_count: number;
  context_term_count: number;
  backup_file: string | null;
  summary: TranscriptDiffSummary;
  items: TranscriptDiffItem[];
  /** 规则/术语层给出的逐条改动，结构与 AI 校对候选一致（同一份清单里展示） */
  edits: ProofreadCandidate[];
}

export function listTranscriptBackups(meetingFolderPath: string) {
  return invoke<TranscriptBackupsResponse>('api_list_transcript_backups', {
    meetingFolderPath,
  });
}

export function getTranscriptRevisionDiff(
  meetingId: string,
  meetingFolderPath: string,
  backupFile: string,
) {
  return invoke<TranscriptDiffResponse>('api_get_transcript_revision_diff', {
    meetingId,
    meetingFolderPath,
    backupFile,
  });
}

export function restoreTranscriptRevision(
  meetingId: string,
  meetingFolderPath: string,
  backupFile: string,
) {
  return invoke<TranscriptRestoreResponse>('api_restore_transcript_revision', {
    meetingId,
    meetingFolderPath,
    backupFile,
  }).then(result => {
    if (typeof window !== 'undefined') window.dispatchEvent(new Event('transcript-files-changed'));
    return result;
  });
}

/**
 * 「按术语核对」：只跑文本层（规则词典 + 会议上下文的人名/术语），不重新识别音频。
 * dryRun=true 只返回改动清单，不写库。
 */
export function applyMeetingTextCorrections(
  meetingId: string,
  meetingFolderPath: string,
  dryRun = false,
) {
  return invoke<TextCorrectionResponse>('api_apply_meeting_text_corrections', {
    meetingId,
    meetingFolderPath,
    dryRun,
  }).then(result => {
    if (typeof window !== 'undefined') window.dispatchEvent(new Event('transcript-files-changed'));
    return result;
  });
}

export interface ProofreadCandidate {
  segment_id: string;
  segment_index: number;
  audio_start_time: number | null;
  original: string;
  suggested: string;
  start_char: number;
  end_char: number;
  reason: string;
  confidence: string;
  segment_text: string;
  proposed_text: string;
}

export interface ProofreadResponse {
  provider: string;
  model: string;
  total_segments: number;
  reviewed_segments: number;
  truncated: boolean;
  candidates: ProofreadCandidate[];
  elapsed_ms: number;
  warnings: string[];
  /** 提示词版本（诊断用） */
  prompt_version: string;
  /** 最终仍没给出结论的片段编号 */
  missing_segments: number[];
  /** 会议文件夹里的诊断文件名（含模型原始输出） */
  diagnostics_file: string | null;
  /** 本次实际用的模型（跟随摘要模型 / 指定供应商） */
  target: ProofreadTarget;
  start_index: number;
  next_start_index: number | null;
  source_hash: string;
}

/** 「文字纠错」用哪个模型：跟随摘要模型（默认），或指定某个已配置的供应商 */
export type ProofreadTarget =
  | { kind: 'summary' }
  | { kind: 'provider'; provider: string; model: string };

/** 下拉里的一项：某台机器上可用的校对模型 */
export interface ProofreadModelOption {
  target: ProofreadTarget;
  provider: string;
  model: string;
  /** 本地内置模型：能力有限，界面要提示 */
  is_local: boolean;
}

export interface ProofreadApplyResponse {
  file_sync_pending: boolean;
  applied_edits: number;
  skipped_edits: number;
  updated_segments: number;
  backup_file: string | null;
}

export interface ProofreadEditPayload {
  segment_id: string;
  original: string;
  suggested: string;
  expected_text: string;
  start_char: number;
  end_char: number;
}

/**
 * 「文字纠错」清单里的一条候选：规则层 / 本地模型 / API 模型三种来源合并展示，
 * 所以额外带一个来源标记（同一处改动只保留最先出现的那条）。
 */
export type CorrectionCandidate = ProofreadCandidate & {
  sourceKind?: 'rules' | 'local' | 'api';
};

/**
 * AI 校对：用「文字纠错用哪个模型」这个偏好选定的模型逐段找同音字/术语错，
 * 只返回候选（不写库）。不传 target 时用存下来的偏好（默认跟随摘要模型）。
 */
export function reviewTranscriptWithLLM(
  meetingId: string,
  meetingFolderPath: string,
  target?: ProofreadTarget,
  startIndex = 0,
  expectedSourceHash?: string,
) {
  return invoke<ProofreadResponse>('api_review_transcript_with_llm', {
    meetingId,
    meetingFolderPath,
    target: target ?? null,
    startIndex,
    expectedSourceHash: expectedSourceHash ?? null,
  });
}

/** 把用户勾选的候选写回（后端会先备份当前转写） */
export function applyTranscriptProofreadEdits(
  meetingId: string,
  meetingFolderPath: string,
  edits: ProofreadEditPayload[],
) {
  return invoke<ProofreadApplyResponse>('api_apply_transcript_proofread_edits', {
    meetingId,
    meetingFolderPath,
    edits,
  }).then(result => {
    if (typeof window !== 'undefined') window.dispatchEvent(new Event('transcript-files-changed'));
    return result;
  });
}

/** 列出这台机器上能用来跑文字纠错的模型（摘要模型 + 各个已配置好的 API 供应商） */
export function listProofreadModels() {
  let knownModels: Record<string, string> = {};
  try {
    const stored: unknown = JSON.parse(localStorage.getItem('providerModelMap') || '{}');
    if (stored && typeof stored === 'object' && !Array.isArray(stored)) {
      knownModels = Object.fromEntries(Object.entries(stored).filter((entry): entry is [string, string] => typeof entry[1] === 'string'));
    }
  } catch { /* A corrupt model cache must not hide configured providers. */ }
  return invoke<ProofreadModelOption[]>('api_list_proofread_models', { knownModels });
}

/** 读「文字纠错用哪个模型」的偏好（默认 = 跟随摘要模型） */
export function getProofreadTarget() {
  return invoke<ProofreadTarget>('api_get_proofread_target');
}

/** 存「文字纠错用哪个模型」的偏好 */
export function setProofreadTarget(target: ProofreadTarget) {
  return invoke<void>('api_set_proofread_target', { target });
}

/**
 * 本机实测的"模型耗时倍率"（相对音频时长）。
 * 只收录真正测过的：2026-09-11 实测 Whisper large-v3-turbo-q5_0 在 29 秒音频上跑了约 7 分钟 ≈ 14×。
 * 没实测过的模型不要瞎填 —— 界面会显示"该模型在本机没有实测耗时"。
 */
export const MEASURED_MODEL_REALTIME_FACTORS: Record<string, number> = {
  'whisper:large-v3-turbo-q5_0': 14,
};

export function estimateEnhancementMinutes(
  provider: string | undefined,
  model: string | undefined,
  audioDurationSeconds: number | null | undefined,
): { factor: number; minutes: number } | null {
  if (!provider || !model || !audioDurationSeconds || audioDurationSeconds <= 0) return null;
  const factor = MEASURED_MODEL_REALTIME_FACTORS[`${provider}:${model}`];
  if (!factor) return null;
  return {
    factor,
    minutes: Math.max(1, Math.round((audioDurationSeconds * factor) / 60)),
  };
}

import { HelpHint } from '@/components/ui/help-hint';
import { TranscriptFileSyncNotice } from './TranscriptFileSyncNotice';
import React, { useState, useEffect, useRef, useMemo } from 'react';
import { RefreshCw, Globe, Loader2, AlertCircle, AlertTriangle, Wand2, X, Cpu, Sparkles, History } from 'lucide-react';
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '../ui/dialog';
import { Button } from '../ui/button';
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '../ui/select';
import { invoke } from '@tauri-apps/api/core';
import { listen, UnlistenFn } from '@tauri-apps/api/event';
import { toast } from 'sonner';
import { useConfig } from '@/contexts/ConfigContext';
import { LANGUAGES } from '@/constants/languages';
import { useTranscriptionModels, ModelOption } from '@/hooks/useTranscriptionModels';
import Analytics from '@/lib/analytics';
import { useTranslation } from 'react-i18next';
import { formatLanguageName, i18n as appI18n } from '@/i18n';
import type { SupportedUiLocale } from '@/i18n/types';
import { Tabs, TabsContent, TabsList, TabsTrigger } from '../ui/tabs';
import { MossReviewWorkspace } from '@/features/moss/components/MossReviewWorkspace';
import { TranscriptRevisionPanel } from './TranscriptRevisionPanel';
import { TranscriptCorrectionPanel } from './TranscriptCorrectionPanel';
import { transcriptCandidateKey } from '@/lib/transcript-edit-selection';
import {
  applyMeetingTextCorrections,
  applyTranscriptProofreadEdits,
  estimateEnhancementMinutes,
  getTranscriptRevisionDiff,
  listTranscriptBackups,
  reviewTranscriptWithLLM,
  restoreTranscriptRevision,
  listProofreadModels,
  getProofreadTarget,
  setProofreadTarget,
  type ProofreadCandidate,
  type ProofreadResponse,
  type CorrectionCandidate,
  type ProofreadModelOption,
  type ProofreadTarget,
  type TranscriptBackupInfo,
  type TranscriptBackupsResponse,
  type TranscriptDiffResponse,
  type TextCorrectionResponse,
} from '@/lib/transcript-revision';

interface RetranscribeDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  meetingId: string;
  meetingFolderPath: string | null;
  standardRetranscriptionEnabled?: boolean;
  onComplete?: () => void;
  /** 该会议当前已有的转写片段数（用于覆盖警告与强制确认，P0-5） */
  existingTranscriptCount?: number;
}

type RetranscriptionProgressStage = 'decoding' | 'vad' | 'transcribing' | 'saving' | 'complete' | 'unknown';

interface RetranscriptionProgress {
  meeting_id: string;
  stage: RetranscriptionProgressStage;
  progress_percentage: number;
  message: string;
}

interface RetranscriptionResult {
  meeting_id: string;
  segments_count: number;
  duration_seconds: number;
  language: string | null;
  /** 覆盖前备份的原转写文件名（会议文件夹内）；原本没有转写时为 null */
  backup_file?: string | null;
  file_sync_pending?: boolean;
}

interface RetranscriptionError {
  meeting_id: string;
  error: string;
}

type RetranscriptionErrorCode = 'meetingFolderUnavailable' | 'startRetranscriptionFailed' | 'retranscriptionFailed';
type RetranscriptionMode = 'standard' | 'moss';
const MOSS_TRANSLATION_NAMESPACE = 'moss' as const;

export function RetranscribeDialog({
  open,
  onOpenChange,
  meetingId,
  meetingFolderPath,
  standardRetranscriptionEnabled = true,
  onComplete,
  existingTranscriptCount = 0,
}: RetranscribeDialogProps) {
  const { t, i18n } = useTranslation(['transcription', 'moss']);
  const { selectedLanguage, transcriptModelConfig, betaFeatures } = useConfig();
  const [isProcessing, setIsProcessing] = useState(false);
  const [progress, setProgress] = useState<RetranscriptionProgress | null>(null);
  const [error, setError] = useState<RetranscriptionErrorCode | null>(null);
  const [selectedLang, setSelectedLang] = useState(selectedLanguage || 'auto');
  const [dialogMode, setDialogMode] = useState<RetranscriptionMode>(
    standardRetranscriptionEnabled ? 'standard' : 'moss',
  );
  const [mossBusy, setMossBusy] = useState(false);
  const [overwriteAcknowledged, setOverwriteAcknowledged] = useState(false);
  // P0：跑完先给"改了什么"，让用户决定保留还是回退
  const [enhancementPhase, setEnhancementPhase] = useState<'setup' | 'review' | 'correct'>(
    'setup',
  );
  const [backups, setBackups] = useState<TranscriptBackupsResponse | null>(null);
  const [revisionDiff, setRevisionDiff] = useState<TranscriptDiffResponse | null>(null);
  const [isLoadingDiff, setIsLoadingDiff] = useState(false);
  const [isRestoring, setIsRestoring] = useState(false);
  const [isCorrecting, setIsCorrecting] = useState(false);
  const [selectedBackupFile, setSelectedBackupFile] = useState<string>('');
  /** 规则/术语层的候选（dry_run，不写库） */
  const [correctionResult, setCorrectionResult] = useState<TextCorrectionResponse | null>(null);
  /**
   * AI 深查的每一次结果（手动触发，不写库）。
   * 用数组是因为本地小模型漏改时，用户会点"用 API 模型再查一遍"，两次候选要合并展示。
   */
  const [proofreadRuns, setProofreadRuns] = useState<ProofreadResponse[]>([]);
  const [isProofreading, setIsProofreading] = useState(false);
  const [isApplyingProofread, setIsApplyingProofread] = useState(false);
  /** 「文字纠错」用哪个模型（跟摘要模型解耦；默认跟随摘要模型） */
  const [isSavingProofreadModel, setIsSavingProofreadModel] = useState(false);
  const proofreadModelSaveRef = useRef(false);
  const [proofreadTarget, setProofreadTargetState] = useState<ProofreadTarget>({ kind: 'summary' });
  /** 这台机器上可选的校对模型：摘要模型 + 各已配置的 API 供应商 */
  const [proofreadModelOptions, setProofreadModelOptions] = useState<ProofreadModelOption[]>([]);
  const completionHandledRef = useRef(false);
  /**
   * 写回文字纠错后，父组件的刷新（onComplete）会把对话框卸载掉 ——
   * 真机上表现为"应用成功、对比面板一次都没出现"。
   * 所以改成：对话框还开着的时候不刷新，等它关了再刷（见下面的 effect）。
   */
  const pendingCommitRefreshRef = useRef(false);
  const mossEnabled = betaFeatures.moss_post_meeting_enhancement;
  const requiresOverwriteAcknowledgment = existingTranscriptCount > 0;
  const uiLocale: SupportedUiLocale = i18n.resolvedLanguage === 'zh-CN' ? 'zh-CN' : 'en';
  const getLanguageName = (code: string, fallback: string) => {
    if (code === 'auto') return t('labels.autoDetectOriginalLanguage');
    if (code === 'auto-translate') return t('labels.autoDetectTranslateToEnglish');
    return formatLanguageName(code, uiLocale) || fallback;
  };

  // Use centralized model fetching hook
  const {
    availableModels,
    selectedModelKey,
    setSelectedModelKey,
    loadingModels,
    fetchModels,
    resetSelection,
  } = useTranscriptionModels(transcriptModelConfig);

  // Stable refs for callbacks to avoid listener re-registration
  const onCompleteRef = useRef(onComplete);
  const onOpenChangeRef = useRef(onOpenChange);
  useEffect(() => { onCompleteRef.current = onComplete; }, [onComplete]);
  useEffect(() => { onOpenChangeRef.current = onOpenChange; }, [onOpenChange]);

  // Track previous open state to only reset on closed→open transition
  const prevOpenRef = useRef(false);

  // Helper to get selected model details (memoized)
  const selectedModelDetails = useMemo((): ModelOption | undefined => {
    if (!selectedModelKey) return undefined;
    const colonIndex = selectedModelKey.indexOf(':');
    if (colonIndex === -1) return undefined;
    const provider = selectedModelKey.slice(0, colonIndex);
    const name = selectedModelKey.slice(colonIndex + 1);
    return availableModels.find(m => m.provider === provider && m.name === name);
  }, [selectedModelKey, availableModels]);
  const isParakeetModel = selectedModelDetails?.provider === 'parakeet';
  const normalizeModelValue = (value?: string | null) => (value ?? '').trim().toLowerCase();
  /** 选的模型和这场会议上次用的一样吗？（仅用于说明，不承诺输出不变） */
  const sameModelAsLastRun = Boolean(
    selectedModelDetails?.name &&
      backups?.last_used_model &&
      normalizeModelValue(backups.last_used_model) ===
        normalizeModelValue(selectedModelDetails.name) &&
      (!backups.last_used_provider ||
        normalizeModelValue(backups.last_used_provider) ===
          normalizeModelValue(selectedModelDetails.provider)),
  );
  const estimatedEnhancement = estimateEnhancementMinutes(
    selectedModelDetails?.provider,
    selectedModelDetails?.name,
    backups?.audio_duration_seconds ?? null,
  );
  /**
   * 一行说清"这次跑会怎样"：同模型及预计耗时。
   * 原来这两件事各占一个色块/三行小字，是弹窗显得乱的主因之一。
   */
  const modelHintLine = [
    sameModelAsLastRun ? t('enhancementReview.sameModelShort') : null,
    estimatedEnhancement
      ? t('enhancementReview.estimateShort', {
          minutes: estimatedEnhancement.minutes,
          factor: estimatedEnhancement.factor,
        })
      : t('enhancementReview.estimateUnknownShort'),
  ]
    .filter(Boolean)
    .join(' · ');

  /** 备份下拉用小标签（时间 + 段数），完整文件名放 tooltip，避免长文件名撑爆控件 */
  const formatBackupLabel = (backup: TranscriptBackupInfo, isLatest: boolean) => {
    const raw = backup.saved_at ?? backup.created_at ?? null;
    let stamp = '';
    if (raw) {
      const date = new Date(raw);
      if (!Number.isNaN(date.getTime())) {
        stamp = `${String(date.getMonth() + 1).padStart(2, '0')}-${String(date.getDate()).padStart(2, '0')} ${String(date.getHours()).padStart(2, '0')}:${String(date.getMinutes()).padStart(2, '0')}`;
      }
    }
    const base = t('enhancementReview.backupLabel', {
      date: stamp || '—',
      count: backup.total_segments,
    });
    return isLatest ? `${t('enhancementReview.backupLabelLatest')} · ${base}` : base;
  };

  /**
   * 规则层（秒级）和 AI 深查的候选合成一份清单 —— 左右都是"原词 → 新词"，
   * 可以一起勾选、一次写回、一次备份。
   * 去重按"同片段 + 同原词 + 同新词"逐条抵消，AI 不会把规则层说过的改动再报一遍。
   */
  const { candidates: correctionCandidates } = useMemo(() => {
    const ruleCandidates: CorrectionCandidate[] = (correctionResult?.edits ?? []).map(
      (candidate) => ({ ...candidate, sourceKind: 'rules' }),
    );
    const aiCandidates: CorrectionCandidate[] = proofreadRuns.flatMap((run) =>
      run.candidates.map((candidate) => ({
        ...candidate,
        sourceKind: run.provider === 'builtin-ai' ? ('local' as const) : ('api' as const),
      })),
    );
    if (aiCandidates.length === 0) {
      return { candidates: ruleCandidates };
    }
    const identity = (candidate: ProofreadCandidate) =>
      transcriptCandidateKey(candidate);
    const budget = new Map<string, number>();
    ruleCandidates.forEach((candidate) => {
      const key = identity(candidate);
      budget.set(key, (budget.get(key) ?? 0) + 1);
    });
    const added: ProofreadCandidate[] = [];
    aiCandidates.forEach((candidate) => {
      const key = identity(candidate);
      const remaining = budget.get(key) ?? 0;
      if (remaining > 0) {
        budget.set(key, remaining - 1);
        return;
      }
      added.push(candidate);
    });
    return {
      candidates: [...ruleCandidates, ...added].sort((a, b) => a.segment_index - b.segment_index),
    };
  }, [correctionResult, proofreadRuns]);

  /** 读取这场会议的增强备份 + 上次用的模型（供"改了什么/能不能回退"用） */
  const refreshBackups = React.useCallback(async () => {
    if (!meetingFolderPath) {
      setBackups(null);
      return;
    }
    try {
      const response = await listTranscriptBackups(meetingFolderPath);
      setBackups(response);
      setSelectedBackupFile((current) => current || response.backups[0]?.file || '');
    } catch (loadError) {
      console.warn('Failed to load transcript backups:', loadError);
      setBackups(null);
    }
  }, [meetingFolderPath]);

  /** 拉某一版备份与当前转写的差异（backupFile 为空时用最新一版） */
  const loadRevisionDiff = React.useCallback(
    async (backupFile?: string | null) => {
      if (!meetingFolderPath) return null;
      setIsLoadingDiff(true);
      try {
        let target = backupFile && backupFile.length > 0 ? backupFile : null;
        if (!target) {
          const latest = await listTranscriptBackups(meetingFolderPath);
          setBackups(latest);
          target = latest.backups[0]?.file ?? null;
        }
        if (!target) {
          setRevisionDiff(null);
          return null;
        }
        const diff = await getTranscriptRevisionDiff(meetingId, meetingFolderPath, target);
        setRevisionDiff(diff);
        return diff;
      } catch (diffError) {
        console.warn('Failed to load transcript revision diff:', diffError);
        setRevisionDiff(null);
        toast.error(t('enhancementReview.loadFailed'));
        return null;
      } finally {
        setIsLoadingDiff(false);
      }
    },
    [meetingFolderPath, meetingId, t],
  );

  /** 跑完（事件或兜底轮询）统一走这里：先给对比，再让用户决定 */
  const handleEnhancementFinished = React.useCallback(
    async (backupFile: string | null) => {
      if (completionHandledRef.current) return;
      completionHandledRef.current = true;
      setIsProcessing(false);
      setProgress(null);
      pendingCommitRefreshRef.current = true;
      window.dispatchEvent(new Event('transcript-files-changed'));
      const diff = await loadRevisionDiff(backupFile);
      if (diff) {
        setEnhancementPhase('review');
      } else {
        // 没有可对比的备份（比如本来就没有旧转写）→ 保持原行为：直接收工
        onCompleteRef.current?.();
        onOpenChangeRef.current(false);
      }
    },
    [loadRevisionDiff],
  );

  useEffect(() => {
    if (isParakeetModel && selectedLang !== 'auto') {
      setSelectedLang('auto');
    }
  }, [isParakeetModel, selectedLang]);

  // Reset state only when dialog transitions from closed to open
  // This prevents re-initialization when config changes while dialog is already open
  useEffect(() => {
    const wasOpen = prevOpenRef.current;
    prevOpenRef.current = open;

    if (open && !wasOpen) {
      resetSelection();
      setIsProcessing(false);
      setProgress(null);
      setError(null);
      setSelectedLang(selectedLanguage || 'auto');
      setDialogMode(standardRetranscriptionEnabled ? 'standard' : 'moss');
      setMossBusy(false);
      setOverwriteAcknowledged(false);
      setEnhancementPhase('setup');
      setRevisionDiff(null);
      setIsRestoring(false);
      setIsCorrecting(false);
      setCorrectionResult(null);
      setProofreadRuns([]);
      setIsProofreading(false);
      setIsApplyingProofread(false);
      setSelectedBackupFile('');
      completionHandledRef.current = false;
      void refreshBackups();

      // Fetch available models using centralized hook
      fetchModels();
    }
  }, [
    open,
    selectedLanguage,
    transcriptModelConfig,
    fetchModels,
    standardRetranscriptionEnabled,
    refreshBackups,
  ]);

  useEffect(() => {
    if (!mossEnabled && dialogMode === 'moss') setDialogMode('standard');
    if (!standardRetranscriptionEnabled && mossEnabled && dialogMode === 'standard') setDialogMode('moss');
  }, [dialogMode, mossEnabled, standardRetranscriptionEnabled]);

  /**
   * 关掉对话框之后再去刷新父组件数据。
   * 之前是在写回成功后立刻 onComplete()，父组件刷新会把对话框卸载，
   * 用户看不到"改了什么"的对比面板（真机实测三次都这样）。
   */
  useEffect(() => {
    if (open || !pendingCommitRefreshRef.current) return;
    pendingCommitRefreshRef.current = false;
    void onCompleteRef.current?.();
  }, [open]);

  useEffect(() => {
    if (mossBusy && dialogMode === 'standard' && !isProcessing) setDialogMode('moss');
  }, [dialogMode, isProcessing, mossBusy]);

  // Listen for retranscription events
  useEffect(() => {
    if (!open || dialogMode !== 'standard') return;

    const unlisteners: UnlistenFn[] = [];
    const cleanedUpRef = { current: false };

    const setupListeners = async () => {
      // Progress events
      const unlistenProgress = await listen<RetranscriptionProgress>(
        'retranscription-progress',
        (event) => {
          if (event.payload.meeting_id === meetingId) {
            const knownStages: RetranscriptionProgressStage[] = ['decoding', 'vad', 'transcribing', 'saving', 'complete'];
            setProgress({
              ...event.payload,
              stage: knownStages.includes(event.payload.stage)
                ? event.payload.stage
                : 'unknown',
            });
          }
        }
      );
      if (cleanedUpRef.current) {
        unlistenProgress();
        return;
      }
      unlisteners.push(unlistenProgress);

      // Completion event
      const unlistenComplete = await listen<RetranscriptionResult>(
        'retranscription-complete',
        async (event) => {
          if (event.payload.meeting_id === meetingId) {
            void Analytics.track('enhance_transcript_completed', {
              success: 'true',
              duration_seconds: event.payload.duration_seconds.toString(),
              segments_count: event.payload.segments_count.toString()
            }).catch(error => console.warn('Retranscription analytics failed', error));

            const backupFile = event.payload.backup_file ?? null;
            window.dispatchEvent(new Event('transcript-files-changed'));
            if (event.payload.file_sync_pending) toast.warning(appI18n.t('transcription:fileSync.pending'));
            else toast.success(
              appI18n.t('transcription:messages.retranscriptionComplete', {
                count: event.payload.segments_count,
              }),
              backupFile
                ? {
                    description: appI18n.t('transcription:messages.retranscriptionBackupSaved', {
                      file: backupFile,
                    }),
                  }
                : undefined,
            );
            pendingCommitRefreshRef.current = true;
            // P0：不再立刻关窗，先把"改了什么"摆出来，让用户决定保留还是回退
            void handleEnhancementFinished(backupFile);
          }
        }
      );
      if (cleanedUpRef.current) {
        unlistenComplete();
        unlisteners.forEach(u => u());
        return;
      }
      unlisteners.push(unlistenComplete);

      // Error event
      const unlistenError = await listen<RetranscriptionError>(
        'retranscription-error',
        async (event) => {
          if (event.payload.meeting_id === meetingId) {
            void Analytics.trackError('enhance_transcript_failed', event.payload.error).catch(error => console.warn('Retranscription analytics failed', error));

            setIsProcessing(false);
            console.error('Retranscription processing failed:', event.payload.error);
            setError('retranscriptionFailed');
          }
        }
      );
      if (cleanedUpRef.current) {
        unlistenError();
        unlisteners.forEach(u => u());
        return;
      }
      unlisteners.push(unlistenError);
    };

    setupListeners();

    return () => {
      cleanedUpRef.current = true;
      unlisteners.forEach((unlisten) => unlisten());
    };
  }, [dialogMode, open, meetingId, handleEnhancementFinished]);

  /**
   * P0-5：事件可能丢（真机上出现过"后端早跑完、对话框卡在 52% 不关"），
   * 所以跑的过程中每 2 秒问一次后端"还在跑吗"，读取该会议的真实结果；失败不能算完成。
   */
  useEffect(() => {
    if (!open || dialogMode !== 'standard' || !isProcessing) return;

    const timer = window.setInterval(async () => {
      try {
        const status = await invoke<{ state: string; result: RetranscriptionResult | null }>('get_retranscription_status_command', { meetingId });
        if (status.state === 'completed' && status.result) {
          void handleEnhancementFinished(status.result.backup_file ?? null);
        } else if (status.state === 'failed') {
          setIsProcessing(false);
          setError('retranscriptionFailed');
        }
      } catch (pollError) {
        console.warn('Failed to poll retranscription state:', pollError);
      }
    }, 2000);

    return () => window.clearInterval(timer);
  }, [open, dialogMode, isProcessing, meetingId, handleEnhancementFinished]);

  const handleStartRetranscription = async () => {
    if (!meetingFolderPath) {
      setError('meetingFolderUnavailable');
      return;
    }

    completionHandledRef.current = false;
    setIsProcessing(true);
    setError(null);
    setProgress(null);

    try {
      const languageToSend = isParakeetModel ? null : selectedLang === 'auto' ? null : selectedLang;
      await Analytics.track('enhance_transcript_started', {
        language: isParakeetModel ? 'auto' : (selectedLang === 'auto' ? 'auto' : selectedLang),
        model_provider: selectedModelDetails?.provider || '',
        model_name: selectedModelDetails?.name || ''
      });

      await invoke('start_retranscription_command', {
        meetingId,
        meetingFolderPath,
        language: languageToSend,
        model: selectedModelDetails?.name || null,
        provider: selectedModelDetails?.provider || null,
      });
    } catch (err: any) {
      setIsProcessing(false);
      const errorMsg = typeof err === 'string' ? err : (err?.message || String(err));
      console.error('Retranscription start failed:', errorMsg);
      setError('startRetranscriptionFailed');

      await Analytics.trackError('enhance_transcript_failed', errorMsg);
    }
  };

  const handleCancel = async () => {
    if (isProcessing) {
      try {
        await invoke('cancel_retranscription_command');
        setIsProcessing(false);
        setProgress(null);
        setError(null);
        completionHandledRef.current = true;
        toast.info(t('messages.retranscriptionCancelled'));
        onOpenChange(false);
      } catch (err) {
        console.error('Failed to cancel retranscription:', err);
        toast.error(t('errors.cancelRetranscriptionFailed'));
        return;
      }
    } else {
      onOpenChange(false);
    }
  };

  /** 保留新版：新转写已经在库里了，直接收工 */
  const handleKeepNew = () => {
    setEnhancementPhase('setup');
    setRevisionDiff(null);
    onCompleteRef.current?.();
    onOpenChangeRef.current(false);
  };

  /** 恢复增强前：按备份写回 DB + transcripts.json + metadata.json（恢复前会自动再备份当前版本） */
  const handleRestore = async () => {
    if (!meetingFolderPath || !revisionDiff || !revisionDiff.backup_file) return;
    setIsRestoring(true);
    try {
      const result = await restoreTranscriptRevision(
        meetingId,
        meetingFolderPath,
        revisionDiff.backup_file,
      );
      void Analytics.track('enhance_transcript_restored', {
        segments: String(result.restored_segments),
      }).catch(error => console.warn('Restore analytics failed', error));
      if (result.file_sync_pending) toast.warning(t('fileSync.pending'));
      else toast.success(
        t('enhancementReview.restored', { count: result.restored_segments }),
        result.pre_restore_backup
          ? { description: t('enhancementReview.restoreBackupHint', { file: result.pre_restore_backup }) }
          : undefined,
      );
      setEnhancementPhase('setup');
      setRevisionDiff(null);
      onCompleteRef.current?.();
      await refreshBackups();
      onOpenChangeRef.current(false);
    } catch (restoreError) {
      console.error('Failed to restore transcript revision:', restoreError);
      toast.error(t(restoreError === 'transcript_files_pending' ? 'fileSync.pending' : 'enhancementReview.restoreFailed'));
    } finally {
      setIsRestoring(false);
    }
  };

  /**
   * 「文字纠错」第一步：只跑文本层（术语规则 + 会议上下文的人名/别名）拿候选，
   * `dryRun=true` 所以不写库、不重新识别音频，秒级返回。
   */
  const handleTermCorrection = async () => {
    if (!meetingFolderPath) return;
    setIsCorrecting(true);
    setCorrectionResult(null);
    setProofreadRuns([]);
    try {
      // Do not expose AI actions until the saved model preference is loaded.
      const [result] = await Promise.all([
        applyMeetingTextCorrections(meetingId, meetingFolderPath, true),
        refreshProofreadModels(),
      ]);
      setCorrectionResult(result);
      setEnhancementPhase('correct');
    } catch (correctionError) {
      console.error('Failed to collect rule-based text corrections:', correctionError);
      toast.error(t('enhancementReview.correctionFailed'));
    } finally {
      setIsCorrecting(false);
    }
  };

  /**
   * 「文字纠错」第二步（可选）：用选定的模型逐段深查，只出候选，不写库。
   * 多次跑的候选合并展示（不覆盖），所以"本地跑一遍 + API 再跑一遍"也能对比。
   */
  const handleProofread = async () => {
    if (!meetingFolderPath) return;
    setIsProofreading(true);
    try {
      const previousRun = proofreadRuns.at(-1);
      const continueRun = previousRun && previousRun.next_start_index !== null &&
        JSON.stringify(previousRun.target) === JSON.stringify(proofreadTarget);
      const result = await reviewTranscriptWithLLM(
        meetingId, meetingFolderPath, proofreadTarget,
        continueRun ? previousRun.next_start_index ?? 0 : 0,
        continueRun ? previousRun.source_hash : undefined,
      );
      setProofreadRuns((previous) => {
        if (previous.length === 0) return [result];
        // 同一处改动只留一条（后面的重复候选不再追加），避免清单里出现两份一样的
        const seen = new Set(
          previous.flatMap((run) =>
            run.candidates.map(transcriptCandidateKey),
          ),
        );
        const extra = result.candidates.filter(
          (candidate) =>
            !seen.has(transcriptCandidateKey(candidate)),
        );
        return [...previous, { ...result, candidates: extra }];
      });
      setEnhancementPhase('correct');
    } catch (proofreadError) {
      console.error('Failed to review transcript with LLM:', proofreadError);
      toast.error(t(proofreadError === 'proofread_source_changed' ? 'proofread.sourceChanged' : proofreadError === 'proofread_model_unavailable' ? 'proofread.modelUnavailable' : 'proofread.failed'));
    } finally {
      setIsProofreading(false);
    }
  };

  /**
   * 换「文字纠错」用的模型（记在偏好里，下次开对话框还是它）。
   * 这是把校对模型和摘要模型解耦的那一步：摘要继续用本地，校对可以换成任意已配置的 API。
   */
  const handleProofreadTargetChange = async (target: ProofreadTarget, label: string) => {
    if (proofreadModelSaveRef.current) return;
    proofreadModelSaveRef.current = true;
    setIsSavingProofreadModel(true);
    const previous = proofreadTarget;
    setProofreadTargetState(target);
    try {
      await setProofreadTarget(target);
      toast.success(t('correction.modelPickerSaved', { model: label }));
    } catch (preferenceError) {
      console.warn('Failed to save proofread model preference:', preferenceError);
      setProofreadTargetState(previous);
      toast.error(t('correction.modelPickerSaveFailed'));
    } finally {
      proofreadModelSaveRef.current = false;
      setIsSavingProofreadModel(false);
    }
  };

  /** 打开面板时读一次"可选模型 + 当前选择" */
  const refreshProofreadModels = React.useCallback(async () => {
    try {
      const [options, target] = await Promise.all([listProofreadModels(), getProofreadTarget()]);
      setProofreadModelOptions(options);
      setProofreadTargetState(target);
    } catch (loadError) {
      console.warn('Failed to load proofread model options:', loadError);
      throw loadError;
    }
  }, []);

  /**
   * 写回用户勾选的候选（规则层 + AI 层同一份清单，后端写回前自动备份）。
   * 写回后立刻把"改前 → 改后"摆出来：确认没问题再关，不满意一键恢复。
   */
  const handleApplyProofread = async (selected: ProofreadCandidate[]) => {
    if (!meetingFolderPath || selected.length === 0) return;
    setIsApplyingProofread(true);
    try {
      const response = await applyTranscriptProofreadEdits(
        meetingId,
        meetingFolderPath,
        selected.map((candidate) => ({
          segment_id: candidate.segment_id,
          original: candidate.original,
          suggested: candidate.suggested,
          expected_text: candidate.segment_text,
          start_char: candidate.start_char,
          end_char: candidate.end_char,
        })),
      );
      if (response.file_sync_pending) toast.warning(t('fileSync.pending'));
      else toast.success(
        t('proofread.applied', {
          applied: response.applied_edits,
          skipped: response.skipped_edits,
        }),
        response.backup_file
          ? { description: t('enhancementReview.restoreBackupHint', { file: response.backup_file }) }
          : undefined,
      );
      void Analytics.track('transcript_proofread_applied', {
        applied: String(response.applied_edits),
        skipped: String(response.skipped_edits),
      }).catch(error => console.warn('Proofread analytics failed', error));
      // 这里不走 loadRevisionDiff()：它内部有"备份为空就静默返回 null"的分支，
      // 真机上出现过"应用成功、对比没自动弹出来"却查不到原因的情况。
      // 直接调用命令 + 把失败原因显示出来，宁可难看也不要沉默。
      let diff: TranscriptDiffResponse | null = null;
      try {
        if (meetingFolderPath && response.backup_file) {
          diff = await getTranscriptRevisionDiff(
            meetingId,
            meetingFolderPath,
            response.backup_file,
          );
          setRevisionDiff(diff);
        } else {
          console.warn('Skip revision diff after apply', {
            hasFolder: Boolean(meetingFolderPath),
            backupFile: response.backup_file,
          });
        }
      } catch (diffError) {
        console.warn('Failed to load revision diff after apply:', diffError);
        // 只显示翻译好的文案：原始后端错误不进用户可见的界面（i18n 契约要求），
        // 需要细节时看 console 或会议文件夹里的 proofread-diagnostics-*.jsonl。
        toast.error(t('enhancementReview.loadFailed'), {
          description: t('enhancementReview.loadFailedHint'),
        });
      }
      setCorrectionResult(null);
      setProofreadRuns([]);
      if (diff) {
        // 先摆对比；父组件刷新推迟到对话框关闭时（否则对话框会被卸载）
        pendingCommitRefreshRef.current = true;
        setEnhancementPhase('review');
      } else {
        pendingCommitRefreshRef.current = true;
        setEnhancementPhase('setup');
      }
    } catch (applyError) {
      console.error('Failed to apply proofread edits:', applyError);
      const errorKeys = {
        proofread_source_changed: 'proofread.sourceChanged',
        proofread_invalid_range: 'proofread.invalidRange',
        proofread_overlapping_edits: 'proofread.conflictingEdits',
        transcript_files_pending: 'fileSync.pending',
      } as const;
      const errorKey = typeof applyError === 'string' && applyError in errorKeys
        ? errorKeys[applyError as keyof typeof errorKeys]
        : 'proofread.applyFailed';
      toast.error(t(errorKey));
    } finally {
      setIsApplyingProofread(false);
    }
  };

  const standardBusy = isProcessing || isProofreading || isApplyingProofread || isRestoring || isCorrecting || isSavingProofreadModel;

  // Prevent discarding an in-flight write or review.
  const handleOpenChange = (newOpen: boolean) => {
    if (!newOpen && dialogMode === 'standard' && standardBusy) {
      return;
    }
    onOpenChange(newOpen);
  };

  const handleEscapeKeyDown = (event: KeyboardEvent) => {
    if (dialogMode === 'standard' && standardBusy) {
      event.preventDefault();
    }
  };

  const handleInteractOutside = (event: Event) => {
    if (dialogMode === 'standard' && standardBusy) {
      event.preventDefault();
    }
  };

  const standardContent = (
    <>
        <DialogHeader>
          <DialogTitle className="flex items-center gap-2">
            {isProcessing ? (
              <>
                <Loader2 className="h-5 w-5 animate-spin text-blue-600" />
                {t('status.retranscribing')}
              </>
            ) : error ? (
              <>
                <AlertCircle className="h-5 w-5 text-red-600" />
                {t('titles.retranscriptionFailed')}
              </>
            ) : (
              <>
                <RefreshCw className="h-5 w-5 text-blue-600" />
                {t('titles.retranscribeMeeting')}
                <HelpHint text={t('descriptions.retranscriptionSetup')} />
              </>
            )}
          </DialogTitle>
          <DialogDescription className={!isProcessing && !error ? 'sr-only' : undefined}>
            {isProcessing
              ? t(`status.retranscriptionStages.${progress?.stage ?? 'unknown'}`)
              : error
                ? t('descriptions.retranscriptionError')
                : t('descriptions.retranscriptionSetup')}
          </DialogDescription>
        </DialogHeader>

        <div className="space-y-4 py-4">
          {!isProcessing && !error && (
            !isParakeetModel ? (
              <div className="space-y-3">
                <div className="flex items-center gap-2">
                  <Globe className="h-4 w-4 text-muted-foreground" />
                  <span className="text-sm font-medium">{t('labels.language')}</span>
                  <HelpHint text={t(isParakeetModel ? 'descriptions.parakeetTdtV3LanguageLimitations' : 'descriptions.selectLanguageForAccuracy')} />
                </div>
                <Select value={selectedLang} onValueChange={setSelectedLang}>
                  <SelectTrigger className="w-full">
                    <SelectValue placeholder={t('placeholders.selectLanguage')} />
                  </SelectTrigger>
                  <SelectContent className="max-h-60">
                    {LANGUAGES.map((lang) => (
                      <SelectItem key={lang.code} value={lang.code}>
                        {getLanguageName(lang.code, lang.name)}
                      </SelectItem>
                    ))}
                  </SelectContent>
                </Select>
              </div>
            ) : (
              <div className="space-y-3">
                <div className="flex items-center gap-2">
                  <Globe className="h-4 w-4 text-muted-foreground" />
                  <span className="text-sm font-medium">{t('labels.language')}</span>
                  <HelpHint text={t(isParakeetModel ? 'descriptions.parakeetTdtV3LanguageLimitations' : 'descriptions.selectLanguageForAccuracy')} />
                </div>
              </div>
            )
          )}

          {!isProcessing && !error && availableModels.length > 0 && (
            <div className="space-y-3">
              <div className="flex items-center gap-2">
                <Cpu className="h-4 w-4 text-muted-foreground" />
                <span className="text-sm font-medium">{t('labels.model')}</span>
                <HelpHint text={modelHintLine} />
              </div>
              <Select value={selectedModelKey} onValueChange={setSelectedModelKey} disabled={loadingModels}>
                <SelectTrigger className="w-full">
                  <SelectValue placeholder={loadingModels ? t('placeholders.loadingModels') : t('placeholders.selectModel')} />
                </SelectTrigger>
                <SelectContent>
                  {availableModels.map((model) => (
                    <SelectItem key={`${model.provider}:${model.name}`} value={`${model.provider}:${model.name}`}>
                      {model.displayName}
                      {Number.isFinite(model.size_mb) && model.size_mb > 0
                        ? ` ${t('labels.modelSizeValue', { size: Math.round(model.size_mb) })}`
                        : ''}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
            </div>
          )}

          {/* 文字纠错：规则层先跑（秒级、不写库），AI 深查在候选面板里按需触发 */}
          {!isProcessing && !error && (
            <div className="space-y-2 border-t pt-3">
              <div className="flex items-center justify-between gap-3">
                <Button
                  type="button"
                  size="sm"
                  variant="outline"
                  className="h-8 shrink-0 text-xs"
                  title={t('correction.hint')}
                  aria-label={t('correction.action')}
                  disabled={isCorrecting || !meetingFolderPath}
                  onClick={() => void handleTermCorrection()}
                >
                  {isCorrecting ? (
                    <Loader2 className="mr-1.5 h-3.5 w-3.5 animate-spin" aria-hidden="true" />
                  ) : (
                    <Wand2 className="mr-1.5 h-3.5 w-3.5" aria-hidden="true" />
                  )}
                  {isCorrecting ? t('correction.running') : t('correction.action')}
                </Button>
                <span className="truncate text-xs text-gray-400">{t('correction.shortHint')}</span>
              </div>
              {/* 历史备份：选一版 → 右侧图标按钮看这版和当前的差异；图标按钮样式对齐应用里其他图标按钮 */}
              {backups && backups.backups.length > 0 && (
                <div className="flex items-center gap-2">
                  <span className="shrink-0 text-xs text-gray-500">
                    {t('enhancementReview.backupSelectorLabel')}
                  </span>
                  <Select value={selectedBackupFile} onValueChange={setSelectedBackupFile}>
                    <SelectTrigger className="h-7 flex-1 text-xs" title={selectedBackupFile}>
                      <SelectValue placeholder={t('enhancementReview.noBackup')} />
                    </SelectTrigger>
                    <SelectContent>
                      {backups.backups.map((backup, index) => (
                        <SelectItem
                          key={backup.file}
                          value={backup.file}
                          className="text-xs"
                          title={backup.file}
                        >
                          {formatBackupLabel(backup, index === 0)}
                        </SelectItem>
                      ))}
                    </SelectContent>
                  </Select>
                  <Button
                    type="button"
                    variant="ghost"
                    size="icon"
                    className="h-8 w-8 shrink-0 rounded-full text-gray-600 hover:bg-gray-100 hover:text-gray-900"
                    title={t('enhancementReview.viewDiffTooltip')}
                    aria-label={t('enhancementReview.viewLastDiff')}
                    disabled={isLoadingDiff || !selectedBackupFile}
                    onClick={() => {
                      // 选看历史改动时也要切到 review，否则只把 diff 拉回来但不显示（之前的 bug）
                      void loadRevisionDiff(selectedBackupFile).then((diff) => {
                        if (diff) setEnhancementPhase('review');
                      });
                    }}
                  >
                    {isLoadingDiff ? (
                      <Loader2 className="h-4 w-4 animate-spin" aria-hidden="true" />
                    ) : (
                      <History className="h-4 w-4" aria-hidden="true" />
                    )}
                  </Button>
                </div>
              )}
            </div>
          )}

          {/* 覆盖确认：整个弹窗只保留这一处 amber，并且说清"能回退" */}
          {!isProcessing && !error && requiresOverwriteAcknowledgment && (
            <label className="flex items-start gap-2 rounded-md border border-amber-200 bg-amber-50/60 p-2 text-xs text-amber-900">
              <input
                type="checkbox"
                className="mt-0.5 h-4 w-4 shrink-0 accent-amber-600"
                checked={overwriteAcknowledged}
                onChange={(event) => setOverwriteAcknowledged(event.target.checked)}
              />
              <span>
                <span className="font-medium">{t('labels.retranscribeOverwriteConfirm')}</span>
                <span className="mt-0.5 block text-amber-800">
                  {t('descriptions.retranscribeOverwriteShort', { count: existingTranscriptCount })}
                </span>
              </span>
            </label>
          )}
          {isProcessing && progress && (
            <div className="space-y-2">
              <div className="relative">
                <div className="w-full bg-gray-200 rounded-full h-3">
                  <div
                    className="bg-blue-600 h-3 rounded-full transition-all duration-300 ease-out"
                    style={{ width: `${Math.min(progress.progress_percentage, 100)}%` }}
                  />
                </div>
                <div className="flex justify-between text-xs text-gray-600 mt-1">
                  <span>{t(`status.retranscriptionStages.${progress.stage}`)}</span>
                  <span>{Math.round(progress.progress_percentage)}%</span>
                </div>
              </div>
              <p className="text-sm text-muted-foreground text-center">
                {t(`status.retranscriptionStages.${progress.stage}`)}
              </p>
            </div>
          )}

          {error && (
            <div className="bg-red-50 border border-red-200 rounded-lg p-3">
              <p className="text-sm text-red-800">{t(`errors.${error}`)}</p>
            </div>
          )}
        </div>

        <DialogFooter>
          {!isProcessing && !error && enhancementPhase === 'setup' && (
            <>
              <Button variant="outline" onClick={() => onOpenChange(false)}>
                {t('actions.cancel')}
              </Button>
              <Button
                onClick={handleStartRetranscription}
                className="bg-blue-600 hover:bg-blue-700"
                disabled={!meetingFolderPath || (requiresOverwriteAcknowledgment && !overwriteAcknowledged)}
              >
                <RefreshCw className="h-4 w-4 mr-2" />
                {t('actions.startRetranscription')}
              </Button>
            </>
          )}
          {isProcessing && (
            <Button variant="outline" onClick={handleCancel}>
              <X className="h-4 w-4 mr-2" />
              {t('actions.cancel')}
            </Button>
          )}
          {error && (
            <>
              <Button variant="outline" onClick={() => onOpenChange(false)}>
                {t('actions.close')}
              </Button>
              <Button
                onClick={() => {
                  setError(null);
                  setProgress(null);
                }}
                variant="outline"
              >
                {t('actions.tryAgain')}
              </Button>
            </>
          )}
        </DialogFooter>
    </>
  );

  /** P0：跑完的"改前 → 改后"审核面板（保留新版 / 恢复增强前） */
  const reviewContent =
    revisionDiff && enhancementPhase === 'review' ? (
      <>
        <DialogHeader>
          <DialogTitle className="flex items-center gap-2">
            <Sparkles className="h-5 w-5 text-blue-600" aria-hidden="true" />
            {t('enhancementReview.title')}
          </DialogTitle>
          <DialogDescription>
            {t('enhancementReview.subtitle', {
              before: revisionDiff.before_segments,
              after: revisionDiff.after_segments,
              changed: revisionDiff.summary.changed,
              added: revisionDiff.summary.added,
              removed: revisionDiff.summary.removed,
            })}
          </DialogDescription>
        </DialogHeader>
        <TranscriptRevisionPanel
          diff={revisionDiff}
          isRestoring={isRestoring}
          canRestore={Boolean(revisionDiff.backup_file)}
          onRestore={() => void handleRestore()}
          onKeep={handleKeepNew}
        />
      </>
    ) : null;


  /** 文字纠错候选：规则层 + AI 深查同一份清单 */
  const correctionContent =
    correctionResult && enhancementPhase === 'correct' ? (
      <>
        <DialogHeader>
          <DialogTitle className="flex items-center gap-2">
            <Wand2 className="h-5 w-5 text-blue-600" aria-hidden="true" />
            {t('correction.title')}
            <HelpHint text={t('correction.hint')} />
          </DialogTitle>
          <DialogDescription className="sr-only">{t('correction.hint')}</DialogDescription>
        </DialogHeader>
        <TranscriptCorrectionPanel
          candidates={correctionCandidates}
          ruleCandidateCount={correctionResult.edits.length}
          appliedRules={correctionResult.applied_rules}
          contextPersonCount={correctionResult.context_person_count}
          contextTermCount={correctionResult.context_term_count}
          aiRuns={proofreadRuns}
          isRunningAi={isProofreading}
          isApplying={isApplyingProofread}
          isSavingModel={isSavingProofreadModel}
          modelOptions={proofreadModelOptions}
          selectedTarget={proofreadTarget}
          onRunAi={() => void handleProofread()}
          onTargetChange={(target) =>
            void handleProofreadTargetChange(
              target,
              target.kind === 'summary'
                ? proofreadModelOptions.find((option) => option.target.kind === 'summary')?.model ?? ''
                : target.model,
            )
          }
          onApply={(selected) => void handleApplyProofread(selected)}
          onCancel={() => {
            setCorrectionResult(null);
            setProofreadRuns([]);
            setEnhancementPhase('setup');
          }}
        />
      </>
    ) : null;

  const standardReviewOrProofread = reviewContent ?? correctionContent ?? standardContent;

  const mossContent = (
    <>
      <DialogHeader>
        <DialogTitle id="moss-review-heading" className="flex items-center gap-2">
          <Sparkles className="h-5 w-5 text-purple-600" aria-hidden="true" />
          {t('workspace.title', { ns: MOSS_TRANSLATION_NAMESPACE })}
        </DialogTitle>
        <DialogDescription>{t('workspace.description', { ns: MOSS_TRANSLATION_NAMESPACE })}</DialogDescription>
      </DialogHeader>
      <MossReviewWorkspace
        meetingId={meetingId}
        enabled={mossEnabled}
        active={open && mossEnabled}
        onBusyChange={setMossBusy}
        onTranscriptChanged={async () => { await onCompleteRef.current?.(); }}
      />
    </>
  );

  return (
    <Dialog open={open} onOpenChange={handleOpenChange}>
      <DialogContent
        className={
          dialogMode === 'moss' && mossEnabled
            ? 'max-h-[92vh] max-w-[min(96vw,1120px)] overflow-y-auto'
            : enhancementPhase === 'review' || enhancementPhase === 'correct'
              ? 'max-h-[92vh] overflow-y-auto sm:max-w-[720px]'
              : 'sm:max-w-[500px]'
        }
        onEscapeKeyDown={handleEscapeKeyDown}
        onInteractOutside={handleInteractOutside}
      >
        <TranscriptFileSyncNotice meetingId={meetingId} revision={enhancementPhase} />
        {mossEnabled && standardRetranscriptionEnabled ? (
          <Tabs
            value={dialogMode}
            onValueChange={(value) => {
              const next = value as RetranscriptionMode;
              if ((isProcessing && next === 'moss') || (mossBusy && next === 'standard')) return;
              setDialogMode(next);
            }}
          >
            <TabsList className="grid h-auto w-full grid-cols-2">
              <TabsTrigger value="standard" disabled={mossBusy} className="whitespace-normal">
                {t('mode.standard', { ns: MOSS_TRANSLATION_NAMESPACE })}
              </TabsTrigger>
              <TabsTrigger value="moss" disabled={isProcessing} className="whitespace-normal">
                {t('mode.moss', { ns: MOSS_TRANSLATION_NAMESPACE })}
              </TabsTrigger>
            </TabsList>
            <TabsContent value="standard" forceMount className="mt-4 data-[state=inactive]:hidden">
              {standardReviewOrProofread}
            </TabsContent>
            <TabsContent value="moss" forceMount className="mt-4 data-[state=inactive]:hidden">
              {mossContent}
            </TabsContent>
          </Tabs>
        ) : mossEnabled ? mossContent : standardReviewOrProofread}
      </DialogContent>
    </Dialog>
  );
}

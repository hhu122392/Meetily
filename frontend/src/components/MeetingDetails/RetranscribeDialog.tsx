import React, { useState, useEffect, useRef, useMemo } from 'react';
import { RefreshCw, Globe, Loader2, AlertCircle, Cpu } from 'lucide-react';
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
import { listen } from '@tauri-apps/api/event';
import { toast } from 'sonner';
import { useConfig } from '@/contexts/ConfigContext';
import { LANGUAGES } from '@/constants/languages';
import { normalizeTranscriptionLanguage } from '@/lib/transcription-language';
import { useTranscriptionModels, ModelOption } from '@/hooks/useTranscriptionModels';
import Analytics from '@/lib/analytics';
import { TranscriptCorrectionDialog } from './TranscriptCorrectionDialog';
import { t, useUiLanguage } from "@/i18n";

interface RetranscribeDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  meetingId: string;
  meetingFolderPath: string | null;
  onComplete?: () => void;
}

interface RetranscriptionResult {
  meeting_id: string;
  segments_count: number;
  duration_seconds: number;
  language: string | null;
}

export function RetranscribeDialog({
  open,
  onOpenChange,
  meetingId,
  meetingFolderPath,
  onComplete,
}: RetranscribeDialogProps) {
  useUiLanguage();
  const { selectedLanguage, transcriptModelConfig } = useConfig();
  const [isProcessing, setIsProcessing] = useState(false);
  const [showCorrection, setShowCorrection] = useState(false);
  useEffect(() => { if (!open) setShowCorrection(false); }, [open]);
  const [error, setError] = useState<string | null>(null);
  const [selectedLang, setSelectedLang] = useState(selectedLanguage || 'auto');

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
  useEffect(() => { onCompleteRef.current = onComplete; }, [onComplete]);

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
  useEffect(() => {
    if (selectedModelDetails?.provider === 'sensevoice' && !['auto', 'zh', 'en', 'yue', 'ja', 'ko'].includes(selectedLang)) setSelectedLang('auto');
    if (selectedModelDetails?.provider && selectedModelDetails.provider !== 'sensevoice' && selectedLang === 'yue') setSelectedLang('auto');
  }, [selectedModelDetails?.provider, selectedLang]);

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
      setError(null);
      setSelectedLang(normalizeTranscriptionLanguage(selectedModelDetails?.provider, selectedLanguage));

      // Fetch available models using centralized hook
      fetchModels();
    }
  }, [open, selectedLanguage, transcriptModelConfig, fetchModels]);

  // Refresh the detail view even after the submission dialog has closed.
  useEffect(() => {
    let disposed = false;
    let stop: (() => void) | undefined;
    void listen<RetranscriptionResult>('retranscription-complete', ({ payload }) => {
      if (!disposed && payload.meeting_id === meetingId) onCompleteRef.current?.();
    }).then(fn => { if (disposed) fn(); else stop = fn; }).catch(console.error);
    return () => { disposed = true; stop?.(); };
  }, [meetingId]);

  const submitting = useRef(false);
  const handleStartRetranscription = async () => {
    if (submitting.current) return;
    if (!meetingFolderPath || !selectedModelDetails) {
      setError('请选择可用模型，并确认会议录音目录存在');
      return;
    }

    submitting.current = true;
    setIsProcessing(true);
    setError(null);

    try {
      const languageToSend = isParakeetModel ? null : selectedLang === 'auto' ? null : selectedLang;
      void Analytics.track('enhance_transcript_started', {
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
      toast.success('已加入后台转写队列，可继续其他操作');
      onOpenChange(false);
    } catch (err: any) {
      setIsProcessing(false);
      const errorMsg = typeof err === 'string' ? err : (err?.message || String(err));
      setError(errorMsg);

      void Analytics.trackError('enhance_transcript_failed', errorMsg);
    } finally {
      submitting.current = false;
      setIsProcessing(false);
    }
  };

  // Keep the dialog open only while the queue submission is being saved.
  const handleOpenChange = (newOpen: boolean) => {
    if (!newOpen && isProcessing) {
      return;
    }
    onOpenChange(newOpen);
  };

  const handleEscapeKeyDown = (event: KeyboardEvent) => {
    if (isProcessing) {
      event.preventDefault();
    }
  };

  const handleInteractOutside = (event: Event) => {
    if (isProcessing) {
      event.preventDefault();
    }
  };

  if (open && showCorrection && meetingFolderPath) return <TranscriptCorrectionDialog key={meetingId} meetingId={meetingId} folder={meetingFolderPath} onClose={() => { setShowCorrection(false); onOpenChange(false); }} onComplete={onComplete} />;

  return (
    <Dialog open={open} onOpenChange={handleOpenChange}>
      <DialogContent
        className="sm:max-w-[450px]"
        onEscapeKeyDown={handleEscapeKeyDown}
        onInteractOutside={handleInteractOutside}
      >
        <DialogHeader>
          <DialogTitle className="flex items-center gap-2">
            {isProcessing ? (
              <>
                <Loader2 className="h-5 w-5 animate-spin text-blue-600" /> 正在加入队列… </>
            ) : error ? (
              <>
                <AlertCircle className="h-5 w-5 text-red-600" /> 加入队列失败 </>
            ) : (
              <>
                <RefreshCw className="h-5 w-5 text-blue-600" /> {t("Retranscribe Meeting")} </>
            )}
          </DialogTitle>
          <DialogDescription>
            {isProcessing
              ? '正在保存转写任务…'
              : error
                ? '任务尚未加入队列，请检查后重试'
                : '录音会在后台依次转写，提交后可离开此页面。成功后替换原转写，现有纪要保留。'}
          </DialogDescription>
        </DialogHeader>

        {!isProcessing && <div className="rounded-xl border bg-gray-50 p-3"><Button variant="outline" className="rounded-full" disabled={!meetingFolderPath} onClick={() => setShowCorrection(true)}>转写文字纠错</Button><p className="mt-2 text-xs text-gray-500">检查现有文字中的术语、同音字和错别字，无需重新识别音频。可选 AI 校对，确认后写回并保留备份。</p></div>}
        <div className="space-y-4 py-4">
          {!isProcessing && !loadingModels && availableModels.length === 0 && (
            <p className="text-sm text-amber-700">没有可用的本地转写模型，请先在设置中下载模型。</p>
          )}
          {!isProcessing && !error && (
            !isParakeetModel ? (
              <div className="space-y-3">
                <div className="flex items-center gap-2">
                  <Globe className="h-4 w-4 text-muted-foreground" />
                  <span className="text-sm font-medium">{t("Language")}</span>
                </div>
                <Select value={selectedLang} onValueChange={setSelectedLang}>
                  <SelectTrigger className="w-full">
                    <SelectValue placeholder={t("Select language")} />
                  </SelectTrigger>
                  <SelectContent className="max-h-60">
                    {LANGUAGES.filter(lang => selectedModelDetails?.provider === 'sensevoice' ? ['auto', 'zh', 'en', 'yue', 'ja', 'ko'].includes(lang.code) : lang.code !== 'yue').map((lang) => (
                      <SelectItem key={lang.code} value={lang.code}>
                        {lang.name}
                      </SelectItem>
                    ))}
                  </SelectContent>
                </Select>
                <p className="text-xs text-muted-foreground"> {t("Select a specific language to improve accuracy, or use auto-detect")} </p>
              </div>
            ) : (
              <div className="space-y-3">
                <div className="flex items-center gap-2">
                  <Globe className="h-4 w-4 text-muted-foreground" />
                  <span className="text-sm font-medium">{t("Language")}</span>
                </div>
                <p className="text-xs text-muted-foreground"> {t("Language selection isn't supported for Parakeet. It always uses automatic detection.")} </p>
              </div>
            )
          )}

          {!isProcessing && !error && availableModels.length > 0 && (
            <div className="space-y-3">
              <div className="flex items-center gap-2">
                <Cpu className="h-4 w-4 text-muted-foreground" />
                <span className="text-sm font-medium">{t("Model")}</span>
              </div>
              <Select value={selectedModelKey} onValueChange={setSelectedModelKey} disabled={loadingModels}>
                <SelectTrigger className="w-full">
                  <SelectValue placeholder={loadingModels ? t("Loading models...") : t("Select model")} />
                </SelectTrigger>
                <SelectContent>
                  {availableModels.map((model) => (
                    <SelectItem key={`${model.provider}:${model.name}`} value={`${model.provider}:${model.name}`}>
                      {model.displayName} ({Math.round(model.size_mb)} MB)
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
              <p className="text-xs text-muted-foreground"> {t("Choose a transcription model")} </p>
            </div>
          )}

          {error && (
            <div className="bg-red-50 border border-red-200 rounded-lg p-3">
              <p className="text-sm text-red-800">{error}</p>
            </div>
          )}
        </div>

        <DialogFooter>
          {!isProcessing && !error && (
            <>
              <Button variant="outline" onClick={() => onOpenChange(false)}> {t("Cancel")} </Button>
              <Button
                onClick={handleStartRetranscription}
                className="bg-blue-600 hover:bg-blue-700"
                disabled={!meetingFolderPath || loadingModels || !selectedModelDetails}
              >
                <RefreshCw className="h-4 w-4 mr-2" /> 加入后台队列 </Button>
            </>
          )}
          {isProcessing && <Button disabled><Loader2 className="h-4 w-4 mr-2 animate-spin" />正在加入队列…</Button>}
          {error && (
            <>
              <Button variant="outline" onClick={() => onOpenChange(false)}> {t("Close")} </Button>
              <Button
                onClick={() => {
                  setError(null);
                }}
                variant="outline"
              > {t("Try Again")} </Button>
            </>
          )}
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

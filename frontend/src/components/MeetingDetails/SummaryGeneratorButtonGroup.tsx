"use client";

import { ModelConfig, ModelSettingsModal } from '@/components/ModelSettingsModal';
import {
  Dialog,
  DialogContent,
  DialogTrigger,
  DialogTitle,
  DialogDescription,
} from "@/components/ui/dialog"
import { VisuallyHidden } from "@/components/ui/visually-hidden"
import { Button } from '@/components/ui/button';
import { ButtonGroup } from '@/components/ui/button-group';
import {
  AlertTriangle,
  Sparkles,
  Loader2,
  Square,
  History,
  RefreshCw,
  Save,
  FolderOpen,
  SlidersHorizontal,
} from 'lucide-react';
import Analytics from '@/lib/analytics';
import { invoke } from '@tauri-apps/api/core';
import { toast } from 'sonner';
import { useState, useEffect, useRef, ReactNode } from 'react';
import { isOllamaNotInstalledError } from '@/lib/utils';
import { BuiltInModelInfo } from '@/lib/builtin-ai';
import { useTranslation } from 'react-i18next';
import { MeetingTemplateSelector } from './MeetingTemplateSelector';
import type {
  MeetingTemplateIssue,
  MeetingTemplateMode,
  MeetingTemplateStorage,
  TemplateApiError,
  TemplateListItem,
} from '@/types/summary-template';
import type { SummaryRegenerationMode } from '@/hooks/meeting-details/useSummaryGeneration';
import { SummaryGenerationHistoryDialog } from './SummaryGenerationHistoryDialog';

interface SummaryGeneratorButtonGroupProps {
  meetingId: string;
  languageSlot?: ReactNode;
  modelConfig: ModelConfig;
  setModelConfig: (config: ModelConfig | ((prev: ModelConfig) => ModelConfig)) => void;
  onSaveModelConfig: (config?: ModelConfig) => Promise<void>;
  onGenerateSummary: (customPrompt: string) => Promise<void>;
  onRegenerateSummary: (
    mode?: SummaryRegenerationMode,
    historicalGenerationId?: string,
    customPrompt?: string,
    templateIdOverride?: string,
  ) => Promise<void>;
  onStopGeneration: () => void;
  customPrompt: string;
  summaryStatus: 'idle' | 'processing' | 'summarizing' | 'regenerating' | 'completed' | 'needs_review' | 'error';
  availableTemplates: TemplateListItem[];
  selectedTemplate: string;
  selectedTemplateName: string;
  templatePreferenceMode: MeetingTemplateMode | null;
  templateStorage: MeetingTemplateStorage | null;
  templateIssue: MeetingTemplateIssue | null;
  templateError: TemplateApiError | null;
  isTemplateLoading: boolean;
  isTemplateSaving: boolean;
  onTemplateSelect: (templateId: string, templateName: string) => void;
  onUseGlobalDefault: () => void;
  onTemplateRetry: () => void;
  hasTranscripts?: boolean;
  hasSummary?: boolean;
  isSummaryDirty?: boolean;
  onSaveSummaryChanges?: () => Promise<void>;
  onManualSummaryRestored?: (summary: Record<string, unknown>) => void;
  isModelConfigLoading?: boolean;
  onOpenModelSettings?: (openFn: () => void) => void;
  /** 主按钮那条（底部操作条）用外部入口打开模型设置，避免同一份弹窗渲染两次 */
  onRequestModelSettings?: () => void;
  onOpenFolder?: () => Promise<void> | void;
  /**
   * PRO 版式拆成两个位置：
   * - `toolbar`：右上角图标簇（会议模板 / 摘要历史 / AI 模型 / 录音文件夹）
   * - `primary`：底部悬浮操作条里的主按钮（生成 / 重新生成 / 停止）
   */
  layout?: 'toolbar' | 'primary';
}

export function SummaryGeneratorButtonGroup({
  meetingId,
  modelConfig,
  setModelConfig,
  onSaveModelConfig,
  onGenerateSummary,
  onRegenerateSummary,
  onStopGeneration,
  customPrompt,
  summaryStatus,
  availableTemplates,
  selectedTemplate,
  selectedTemplateName,
  templatePreferenceMode,
  templateStorage,
  templateIssue,
  templateError,
  isTemplateLoading,
  isTemplateSaving,
  onTemplateSelect,
  onUseGlobalDefault,
  onTemplateRetry,
  hasTranscripts = true,
  hasSummary = false,
  isSummaryDirty = false,
  onSaveSummaryChanges,
  onManualSummaryRestored,
  isModelConfigLoading = false,
  onOpenModelSettings,
  onRequestModelSettings,
  onOpenFolder,
  layout = 'primary',
  languageSlot
}: SummaryGeneratorButtonGroupProps) {
  const { t } = useTranslation(['summary', 'meetings']);
  const [isCheckingModels, setIsCheckingModels] = useState(false);
  const [settingsDialogOpen, setSettingsDialogOpen] = useState(false);
  const [regenerationDialogOpen, setRegenerationDialogOpen] = useState(false);
  const [unsavedDialogOpen, setUnsavedDialogOpen] = useState(false);
  const [savingUnsavedChanges, setSavingUnsavedChanges] = useState(false);
  const [pendingRegeneration, setPendingRegeneration] = useState<
    { kind: 'choose' } | { kind: 'historical'; generationId: string } | null
  >(null);

  const continueGeneration = (mode?: SummaryRegenerationMode) => {
    if (hasSummary) {
      // P0-4: 重新生成必须把用户填写的补充背景一起带上，否则上下文会被丢掉
      void onRegenerateSummary(mode ?? 'historical', undefined, customPrompt);
    } else {
      void onGenerateSummary(customPrompt);
    }
  };

  // 弹窗由顶部图标簇那一份持有；主按钮那份只请求外部打开，避免渲染两份模型设置弹窗
  const ownsModelSettingsDialog = layout === 'toolbar' || !onRequestModelSettings;
  const requestModelSettings = () => {
    if (onRequestModelSettings) {
      onRequestModelSettings();
      return;
    }
    setSettingsDialogOpen(true);
  };

  // Expose the function to open the modal via callback registration
  useEffect(() => {
    if (layout !== 'toolbar') return;
    if (!onOpenModelSettings) return;
    // Register our open dialog function with the parent by calling the callback
    onOpenModelSettings(() => {
      console.log('📱 Opening model settings dialog via callback');
      setSettingsDialogOpen(true);
    });
  }, [layout, onOpenModelSettings]);

  if (!hasTranscripts) {
    return null;
  }

  const checkBuiltInAIModelsAndGenerate = async (mode?: SummaryRegenerationMode) => {
    setIsCheckingModels(true);
    try {
      const selectedModel = modelConfig.model;

      // Check if specific model is configured
      if (!selectedModel) {
        toast.error(t('errors.noBuiltInAIModelSelected'), {
          description: t('descriptions.pleaseSelectAModelInSettings'),
          duration: 5000,
        });
        requestModelSettings();
        return;
      }

      // Check model readiness (with filesystem refresh)
      const isReady = await invoke<boolean>('builtin_ai_is_model_ready', {
        modelName: selectedModel,
        refresh: true,
      });

      if (isReady) {
        // Model is available, proceed with generation
        continueGeneration(mode);
        return;
      }

      // Model not ready - check detailed status
      const modelInfo = await invoke<BuiltInModelInfo | null>('builtin_ai_get_model_info', {
        modelName: selectedModel,
      });

      if (!modelInfo) {
        toast.error(t('errors.modelNotFound'), {
          description: t('errors.couldNotFindInformationForModelValue', { selectedModel }),
          duration: 5000,
        });
        requestModelSettings();
        return;
      }

      // Handle different model states
      const status = modelInfo.status;

      if (status.type === 'downloading') {
        toast.info(t('messages.modelDownloadInProgress'), {
          description: t('descriptions.valueIsDownloading', { selectedModel, progress: status.progress }),
          duration: 5000,
        });
        return;
      }

      if (status.type === 'not_downloaded') {
        toast.error(t('errors.modelNotDownloaded'), {
          description: t('status.valueNeedsToBeDownloadedBeforeUseOpeningModelSettings', { selectedModel }),
          duration: 5000,
        });
        requestModelSettings();
        return;
      }

      if (status.type === 'corrupted') {
        toast.error(t('errors.modelFileCorrupted'), {
          description: t('descriptions.valueFileIsCorruptedPleaseDeleteAndReDownload', { selectedModel }),
          duration: 7000,
        });
        requestModelSettings();
        return;
      }

      if (status.type === 'error') {
        toast.error(t('errors.modelError'), {
          description: t('errors.genericDetails'),
          duration: 5000,
        });
        requestModelSettings();
        return;
      }

      // Fallback
      toast.error(t('errors.modelNotAvailable'), {
        description: t('descriptions.theSelectedModelIsNotReadyForUse'),
        duration: 5000,
      });
      requestModelSettings();

    } catch (error) {
      console.error('Error checking built-in AI models:', error);
      toast.error(t('errors.failedToCheckModelStatus'), {
        description: t('errors.genericDetails'),
        duration: 5000,
      });
    } finally {
      setIsCheckingModels(false);
    }
  };

  const checkOllamaModelsAndGenerate = async (mode?: SummaryRegenerationMode) => {
    // The resolved template preference is part of the generation input.
    // Never start with a preference that is still loading or being saved.
    if (isTemplateLoading || isTemplateSaving || templateIssue || templateError) return;

    // Handle built-in AI provider
    if (modelConfig.provider === 'builtin-ai') {
      await checkBuiltInAIModelsAndGenerate(mode);
      return;
    }

    // Only check for Ollama provider
    if (modelConfig.provider !== 'ollama') {
      continueGeneration(mode);
      return;
    }

    setIsCheckingModels(true);
    try {
      const endpoint = modelConfig.ollamaEndpoint || null;
      const models = await invoke('get_ollama_models', { endpoint }) as any[];

      if (!models || models.length === 0) {
        // No models available, show message and open settings
        toast.error(
          t('errors.noOllamaModelsFoundPleaseDownloadGemma22bFromModel'),
          { duration: 5000 }
        );
        requestModelSettings();
        return;
      }

      // Models are available, proceed with generation
      continueGeneration(mode);
    } catch (error) {
      console.error('Error checking Ollama models:', error);
      const errorMessage = error instanceof Error ? error.message : String(error);

      if (isOllamaNotInstalledError(errorMessage)) {
        // Ollama is not installed - show specific message with download link
        toast.error(
          t('errors.ollamaIsNotInstalled'),
          {
            description: t('descriptions.pleaseDownloadAndInstallOllamaToUseLocalModels'),
            duration: 7000,
            action: {
              label: t('actions.download'),
              onClick: () => invoke('open_external_url', { url: 'https://ollama.com/download' })
            }
          }
        );
      } else {
        // Other error - generic message
        toast.error(
          t('errors.failedToCheckOllamaModelsPleaseCheckIfOllamaIs'),
          { duration: 5000 }
        );
      }
      requestModelSettings();
    } finally {
      setIsCheckingModels(false);
    }
  };

  const isGenerating = summaryStatus === 'processing' || summaryStatus === 'summarizing' || summaryStatus === 'regenerating';

  const chooseRegenerationMode = async (mode: SummaryRegenerationMode) => {
    setRegenerationDialogOpen(false);
    await checkOllamaModelsAndGenerate(mode);
  };

  const requestRegenerationChoice = () => {
    if (isSummaryDirty) {
      setPendingRegeneration({ kind: 'choose' });
      setUnsavedDialogOpen(true);
      return;
    }
    setRegenerationDialogOpen(true);
  };

  const requestHistoricalRetry = async (generationId: string) => {
    if (isSummaryDirty) {
      setPendingRegeneration({ kind: 'historical', generationId });
      setUnsavedDialogOpen(true);
      return;
    }
    await onRegenerateSummary('historical', generationId);
  };

  const cancelUnsavedRegeneration = () => {
    setUnsavedDialogOpen(false);
    setPendingRegeneration(null);
  };

  const saveAndContinueRegeneration = async () => {
    if (!onSaveSummaryChanges || !pendingRegeneration) return;
    const next = pendingRegeneration;
    setSavingUnsavedChanges(true);
    try {
      await onSaveSummaryChanges();
      setUnsavedDialogOpen(false);
      setPendingRegeneration(null);
      if (next.kind === 'choose') {
        setRegenerationDialogOpen(true);
      } else {
        await onRegenerateSummary('historical', next.generationId);
      }
    } finally {
      setSavingUnsavedChanges(false);
    }
  };

  // 模型设置弹窗：toolbar 那份是唯一持有者，主按钮那份通过外部入口打开
  const modelSettingsDialog = (
    <Dialog open={settingsDialogOpen} onOpenChange={setSettingsDialogOpen}>
      <DialogTrigger asChild>
        {layout === 'toolbar' ? (
          <Button
            variant="ghost"
            size="icon"
            className="h-8 w-8 rounded-full text-gray-600 hover:bg-gray-100 hover:text-gray-900"
            title={t('accessibility.aiModelSettings')}
            aria-label={t('accessibility.aiModelSettings')}
          >
            <SlidersHorizontal />
          </Button>
        ) : (
          <Button
            variant="outline"
            size="sm"
            title={t('accessibility.summarySettings')}
            aria-label={t('accessibility.summarySettings')}
          >
            <SlidersHorizontal />
            <span className="hidden lg:inline">{t('labels.aiModel')}</span>
          </Button>
        )}
      </DialogTrigger>
      <DialogContent aria-describedby={undefined}>
        <VisuallyHidden>
          <DialogTitle>{t('labels.modelSettings')}</DialogTitle>
        </VisuallyHidden>
        <ModelSettingsModal
          onSave={async (config) => {
            await onSaveModelConfig(config);
            setSettingsDialogOpen(false);
          }}
          modelConfig={modelConfig}
          setModelConfig={setModelConfig}
          skipInitialFetch={true}
          layout="dialog"
        />
      </DialogContent>
    </Dialog>
  );

  return (
    <>
    {layout === 'toolbar' ? (
      // PRO 版式：右上角只留图标，文字信息放进 tooltip / aria-label
      <div className="flex items-center gap-0.5">
        <MeetingTemplateSelector
          templates={availableTemplates}
          selectedTemplateId={selectedTemplate}
          selectedTemplateName={selectedTemplateName}
          preferenceMode={templatePreferenceMode}
          storage={templateStorage}
          issue={templateIssue}
          error={templateError}
          isLoading={isTemplateLoading}
          isSaving={isTemplateSaving}
          disabled={isGenerating}
          trigger="icon"
          onSelect={onTemplateSelect}
          onUseGlobalDefault={onUseGlobalDefault}
          onRetry={onTemplateRetry}
        />

        <SummaryGenerationHistoryDialog
          meetingId={meetingId}
          disabled={isGenerating}
          trigger="icon"
          onRetryGeneration={requestHistoricalRetry}
          onManualRevisionRestored={onManualSummaryRestored}
        />

        {onOpenFolder && (
          <Button
            variant="ghost"
            size="icon"
            className="h-8 w-8 rounded-full text-gray-600 hover:bg-gray-100 hover:text-gray-900"
            onClick={() => {
              Analytics.trackButtonClick('open_recording_folder', 'meeting_details');
              void onOpenFolder();
            }}
            title={t('meetings:actions.openRecordingFolder')}
            aria-label={t('meetings:actions.openRecordingFolder')}
          >
            <FolderOpen />
          </Button>
        )}

        {modelSettingsDialog}
      </div>
    ) : (
    <ButtonGroup>
      {/* Generate Summary or Stop button */}
      {isGenerating ? (
        <Button
          variant="outline"
          size="sm"
          className="border-red-200 bg-gradient-to-r from-red-50 to-orange-50 px-3 text-sm hover:from-red-100 hover:to-orange-100"
          onClick={() => {
            Analytics.trackButtonClick('stop_summary_generation', 'meeting_details');
            onStopGeneration();
          }}
          title={t('actions.stopSummaryGeneration')}
          aria-label={t('actions.stopSummaryGeneration')}
        >
          <Square size={16} fill="currentColor" />
          <span>{t('actions.stop')}</span>
        </Button>
      ) : (
        <Button
          variant="outline"
          size="sm"
          className="border-blue-200 bg-gradient-to-r from-blue-50 to-purple-50 px-3 text-sm hover:from-blue-100 hover:to-purple-100"
          onClick={() => {
            Analytics.trackButtonClick('generate_summary', 'meeting_details');
            if (hasSummary) {
              requestRegenerationChoice();
            } else {
              void checkOllamaModelsAndGenerate();
            }
          }}
          disabled={isCheckingModels || isModelConfigLoading || isTemplateLoading || isTemplateSaving || Boolean(templateIssue || templateError)}
          title={
            isModelConfigLoading
              ? t('status.loadingModelConfiguration')
              : isTemplateLoading
                ? t('templatePreference.loading')
                : isTemplateSaving
                  ? t('templatePreference.saving')
                  : templateIssue
                    ? t('templatePreference.unavailableTitle')
                    : templateError
                      ? t('templatePreference.loadFailed')
              : isCheckingModels
                ? t('status.checkingModels')
                : hasSummary ? t('actions.regenerateAISummary') : t('accessibility.generateAISummary')
          }
        >
          {isCheckingModels || isModelConfigLoading || isTemplateLoading || isTemplateSaving ? (
            <>
              <Loader2 className="animate-spin" size={16} />
              <span>{t('status.processing')}</span>
            </>
          ) : (
            <>
              <Sparkles size={16} />
              <span>{hasSummary ? t('actions.regenerateSummary') : t('labels.generateSummary')}</span>
            </>
          )}
        </Button>
      )}

      {languageSlot}
      {ownsModelSettingsDialog && modelSettingsDialog}
    </ButtonGroup>
    )}
    {layout === 'primary' && (
      <>
    <Dialog open={unsavedDialogOpen} onOpenChange={(open) => {
      if (!open && !savingUnsavedChanges) cancelUnsavedRegeneration();
    }}>
      <DialogContent className="sm:max-w-lg">
        <DialogTitle className="flex items-center gap-2">
          <AlertTriangle className="h-5 w-5 text-amber-600" aria-hidden="true" />
          {t('regeneration.unsavedTitle')}
        </DialogTitle>
        <DialogDescription>{t('regeneration.unsavedDescription')}</DialogDescription>
        <div className="flex flex-col-reverse gap-2 pt-2 sm:flex-row sm:justify-end">
          <Button
            type="button"
            variant="outline"
            disabled={savingUnsavedChanges}
            onClick={cancelUnsavedRegeneration}
          >
            {t('regeneration.cancelAndKeepEditing')}
          </Button>
          <Button
            type="button"
            disabled={savingUnsavedChanges || !onSaveSummaryChanges}
            onClick={() => void saveAndContinueRegeneration()}
          >
            {savingUnsavedChanges
              ? <Loader2 className="mr-2 h-4 w-4 animate-spin" aria-hidden="true" />
              : <Save className="mr-2 h-4 w-4" aria-hidden="true" />}
            {savingUnsavedChanges
              ? t('regeneration.savingChanges')
              : t('regeneration.saveAndContinue')}
          </Button>
        </div>
      </DialogContent>
    </Dialog>
    <Dialog open={regenerationDialogOpen} onOpenChange={setRegenerationDialogOpen}>
      <DialogContent className="sm:max-w-lg">
        <DialogTitle>{t('regeneration.chooseTitle')}</DialogTitle>
        <DialogDescription>{t('regeneration.chooseDescription')}</DialogDescription>
        <div className="grid gap-3 pt-2">
          <button
            type="button"
            className="flex items-start gap-3 rounded-lg border border-blue-200 bg-blue-50 p-4 text-left hover:bg-blue-100 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-blue-500"
            onClick={() => void chooseRegenerationMode('historical')}
          >
            <History className="mt-0.5 h-5 w-5 shrink-0 text-blue-700" aria-hidden="true" />
            <span>
              <span className="block font-medium text-gray-900">{t('regeneration.historicalTitle')}</span>
              <span className="mt-1 block text-sm text-gray-600">{t('regeneration.historicalDescription')}</span>
            </span>
          </button>
          <button
            type="button"
            className="flex items-start gap-3 rounded-lg border border-gray-200 p-4 text-left hover:bg-gray-50 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-blue-500"
            onClick={() => void chooseRegenerationMode('latest')}
          >
            <RefreshCw className="mt-0.5 h-5 w-5 shrink-0 text-gray-700" aria-hidden="true" />
            <span>
              <span className="block font-medium text-gray-900">{t('regeneration.latestTitle')}</span>
              <span className="mt-1 block text-sm text-gray-600">{t('regeneration.latestDescription')}</span>
            </span>
          </button>
        </div>
      </DialogContent>
    </Dialog>
      </>
    )}
    </>
  );
}

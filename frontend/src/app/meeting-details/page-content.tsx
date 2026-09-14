"use client";
import { useState, useEffect, useRef, useCallback } from 'react';
import { motion } from 'framer-motion';
import { Summary, SummaryResponse } from '@/types';
import { useSidebar } from '@/components/Sidebar/SidebarProvider';
import Analytics from '@/lib/analytics';
import { invoke } from '@tauri-apps/api/core';
import { toast } from 'sonner';
import { TranscriptPanel } from '@/components/MeetingDetails/TranscriptPanel';
import { SummaryPanel } from '@/components/MeetingDetails/SummaryPanel';
import { ModelConfig } from '@/components/ModelSettingsModal';
import { Sparkles, ScrollText, Columns2 } from 'lucide-react';

type MeetingDetailView = 'summary' | 'transcript' | 'split';

const MEETING_DETAIL_VIEW_STORAGE_KEY = 'meetily.meetingDetails.view';

const MEETING_DETAIL_VIEW_OPTIONS: Array<{
  value: MeetingDetailView;
  labelKey: 'summary:views.aiSummary' | 'summary:views.transcript' | 'summary:views.split';
  Icon: typeof Sparkles;
}> = [
  { value: 'summary', labelKey: 'summary:views.aiSummary', Icon: Sparkles },
  { value: 'transcript', labelKey: 'summary:views.transcript', Icon: ScrollText },
  { value: 'split', labelKey: 'summary:views.split', Icon: Columns2 },
];

// Custom hooks
import { useMeetingData } from '@/hooks/meeting-details/useMeetingData';
import { useSummaryGeneration } from '@/hooks/meeting-details/useSummaryGeneration';
import { useTemplates } from '@/hooks/meeting-details/useTemplates';
import { useCopyOperations } from '@/hooks/meeting-details/useCopyOperations';
import { useMeetingOperations } from '@/hooks/meeting-details/useMeetingOperations';
import { useConfig } from '@/contexts/ConfigContext';
import { useTranslation } from 'react-i18next';

export default function PageContent({
  meeting,
  summaryData,
  shouldAutoGenerate = false,
  isFinalizingTranscript = false,
  onAutoGenerateComplete,
  onRefetchTranscripts,
  // Pagination props for efficient transcript loading
  segments,
  hasMore,
  isLoadingMore,
  totalCount,
  loadedCount,
  onLoadMore,
}: {
  meeting: any;
  summaryData: Summary | null;
  shouldAutoGenerate?: boolean;
  isFinalizingTranscript?: boolean;
  onAutoGenerateComplete?: () => void;
  onRefetchTranscripts?: () => Promise<void>;
  // Pagination props
  segments?: any[];
  hasMore?: boolean;
  isLoadingMore?: boolean;
  totalCount?: number;
  loadedCount?: number;
  onLoadMore?: () => void;
}) {
  const { t } = useTranslation(['meetings', 'summary', 'common']);
  console.log('📄 PAGE CONTENT: Initializing with data:', {
    meetingId: meeting.id,
    summaryDataKeys: summaryData ? Object.keys(summaryData) : null,
    transcriptsCount: meeting.transcripts?.length
  });

  // State
  const [customPrompt, setCustomPrompt] = useState<string>('');
  // P0-4: 摘要"补充背景"按会议持久化草稿，切走会议/重启应用不丢
  const promptDraftKey = meeting?.id ? `meetily.summaryContextDraft.${meeting.id}` : null;

  useEffect(() => {
    if (!promptDraftKey) return;
    try {
      const saved = window.localStorage.getItem(promptDraftKey);
      if (saved) setCustomPrompt(saved);
    } catch (error) {
      console.warn('Failed to restore summary context draft:', error);
    }
  }, [promptDraftKey]);

  const handlePromptChange = useCallback((value: string) => {
    setCustomPrompt(value);
    if (!promptDraftKey) return;
    try {
      window.localStorage.setItem(promptDraftKey, value);
    } catch (error) {
      console.warn('Failed to persist summary context draft:', error);
    }
  }, [promptDraftKey]);
  const [isRecording] = useState(false);
  const [summaryResponse] = useState<SummaryResponse | null>(null);

  // Ref to store the modal open function from SummaryGeneratorButtonGroup
  const openModelSettingsRef = useRef<(() => void) | null>(null);
  const autoGenerationStartedForMeetingRef = useRef<string | null>(null);
  const lockedContentRef = useRef<HTMLDivElement | null>(null);

  // Sidebar context
  const { serverAddress } = useSidebar();

  // Get model config from ConfigContext
  const { modelConfig, setModelConfig } = useConfig();

  // Custom hooks
  const meetingData = useMeetingData({ meeting, summaryData });
  const templates = useTemplates(meeting.id);
  /**
   * 会议详情视图：AI 摘要 / 转写 / 并排。
   * - 「并排」就是旧版的双视图（左转写 + 右摘要），用户反馈这个结构好用，所以保留并做成默认；
   * - 单视图则是 PRO 版式的整页阅读；
   * - 用户切过之后记住选择（localStorage），下次打开会议沿用。
   * 标签顺序仍按官方 PRO：AI 摘要 在左、转写 在右。
   */
  const [activeView, setActiveView] = useState<MeetingDetailView>(() => {
    try {
      const stored = window.localStorage.getItem(MEETING_DETAIL_VIEW_STORAGE_KEY);
      if (stored === 'summary' || stored === 'transcript' || stored === 'split') {
        return stored;
      }
    } catch (error) {
      console.warn('Failed to read meeting detail view preference:', error);
    }
    return 'split';
  });
  useEffect(() => {
    try {
      window.localStorage.setItem(MEETING_DETAIL_VIEW_STORAGE_KEY, activeView);
    } catch (error) {
      console.warn('Failed to persist meeting detail view preference:', error);
    }
  }, [activeView]);
  const handleRefetchTranscriptsAndSummary = useCallback(async () => {
    await onRefetchTranscripts?.();
    await meetingData.refreshSummaryFromCurrentEvidence();
  }, [onRefetchTranscripts, meetingData.refreshSummaryFromCurrentEvidence]);

  // Callback to register the modal open function
  const handleRegisterModalOpen = (openFn: () => void) => {
    console.log('📝 Registering modal open function in PageContent');
    openModelSettingsRef.current = openFn;
  };

  // Callback to trigger modal open (called from error handler)
  const handleOpenModelSettings = () => {
    console.log('🔔 Opening model settings from PageContent');
    if (openModelSettingsRef.current) {
      openModelSettingsRef.current();
    } else {
      console.warn('⚠️ Modal open function not yet registered');
    }
  };

  // Save model config to backend database and sync via event
  const handleSaveModelConfig = async (config?: ModelConfig) => {
    if (!config) return;
    try {
      await invoke('api_save_model_config', {
        provider: config.provider,
        model: config.model,
        whisperModel: config.whisperModel,
        apiKey: config.apiKey ?? null,
        ollamaEndpoint: config.ollamaEndpoint ?? null,
      });

      // Emit event so ConfigContext and other listeners stay in sync
      const { emit } = await import('@tauri-apps/api/event');
      await emit('model-config-updated', config);

      toast.success(t('meetings:messages.modelSettingsSavedSuccessfully'));
    } catch (error) {
      console.error('Failed to save model config:', error);
      toast.error(t('meetings:errors.failedToSaveModelSettings'));
    }
  };

  const summaryGeneration = useSummaryGeneration({
    meeting,
    transcripts: meetingData.transcripts,
    modelConfig: modelConfig,
    isModelConfigLoading: false, // ConfigContext loads on mount
    selectedTemplate: templates.selectedTemplate,
    setAiSummary: meetingData.setAiSummary,
    onOpenModelSettings: handleOpenModelSettings,
  });

  const copyOperations = useCopyOperations({
    meeting,
    transcripts: meetingData.transcripts,
    meetingTitle: meetingData.meetingTitle,
    aiSummary: meetingData.aiSummary,
    blockNoteSummaryRef: meetingData.blockNoteSummaryRef,
  });

  const meetingOperations = useMeetingOperations({
    meeting,
  });

  // Track page view
  useEffect(() => {
    Analytics.trackPageView('meeting_details');
  }, []);

  useEffect(() => {
    const content = lockedContentRef.current;
    if (!content) return;
    if (isFinalizingTranscript) content.setAttribute('inert', '');
    else content.removeAttribute('inert');
    return () => content.removeAttribute('inert');
  }, [isFinalizingTranscript]);

  // Auto-generate summary when flag is set
  useEffect(() => {
    let cancelled = false;

    const autoGenerate = async () => {
      if (
        shouldAutoGenerate
        && meetingData.transcripts.length > 0
        && !templates.isLoading
        && !templates.isSaving
        && !templates.error
        && !templates.issue
        && autoGenerationStartedForMeetingRef.current !== meeting.id
        && !cancelled
      ) {
        // Claim this meeting before the first await so React effect re-runs cannot
        // start a second generation while model/language preflight is in progress.
        autoGenerationStartedForMeetingRef.current = meeting.id;
        console.log(`🤖 Auto-generating summary with ${modelConfig.provider}/${modelConfig.model}...`);
        await summaryGeneration.handleGenerateSummary('');

        // Notify parent that auto-generation is complete (only if not cancelled)
        if (onAutoGenerateComplete && !cancelled) {
          onAutoGenerateComplete();
        }
      }
    };

    autoGenerate();

    // Cleanup: cancel if component unmounts or meeting changes
    return () => {
      cancelled = true;
    };
  }, [
    shouldAutoGenerate,
    meeting.id,
    meetingData.transcripts.length,
    templates.isLoading,
    templates.isSaving,
    templates.error,
    templates.issue,
    modelConfig.provider,
    modelConfig.model,
    onAutoGenerateComplete,
    summaryGeneration.handleGenerateSummary,
  ]);

  return (
    <motion.div
      initial={{ opacity: 0, y: 20 }}
      animate={{ opacity: 1, y: 0 }}
      transition={{ duration: 0.3, ease: 'easeOut' }}
      className="flex flex-col h-screen bg-gray-50"
    >
      {isFinalizingTranscript && (
        <div
          className="border-b border-blue-200 bg-blue-50 px-5 py-3 text-sm text-blue-900"
          role="status"
          aria-live="polite"
        >
          <div className="font-medium">{t('common:status.finalizingTranscription')}</div>
          <div className="mt-1 text-blue-700">
            {t('common:status.finalizingTranscriptionDetail')}
          </div>
        </div>
      )}
      <div
        ref={lockedContentRef}
        className={`relative flex flex-1 overflow-hidden ${isFinalizingTranscript ? 'pointer-events-none select-none opacity-60' : ''}`}
        aria-busy={isFinalizingTranscript}
      >
        {/* PRO 版式：顶部居中分段切换（AI 摘要 / 转写 / 并排），悬浮在正文之上 */}
        {/*
          并排模式下：切换条居中在右侧摘要栏里（不压左边转写栏），
          同时右边留出 12rem 的「避让区」给右上角图标簇 —— 否则窄窗口下两者会贴在一起（用户反馈挤占）。
        */}
        <div
          className={`pointer-events-none absolute left-0 right-0 top-3 z-40 flex justify-center pl-4 ${
            activeView === 'split'
              ? 'pr-4 md:left-1/4 md:pr-48 lg:left-1/3'
              : 'pr-4'
          }`}
        >
          <div
            role="tablist"
            aria-label={t('summary:views.switchLabel')}
            className="pointer-events-auto flex items-center gap-1 rounded-full border border-gray-200 bg-gray-100/90 p-1 shadow-sm backdrop-blur"
          >
            {MEETING_DETAIL_VIEW_OPTIONS.map(({ value, labelKey, Icon }) => (
              <button
                key={value}
                type="button"
                role="tab"
                aria-selected={activeView === value}
                onClick={() => setActiveView(value)}
                className={`flex items-center gap-1.5 rounded-full px-3 py-1.5 text-sm font-medium transition-colors ${
                  activeView === value
                    ? 'bg-white text-gray-900 shadow-sm'
                    : 'text-gray-500 hover:text-gray-700'
                }`}
              >
                <Icon className="h-4 w-4" aria-hidden="true" />
                {t(labelKey)}
              </button>
            ))}
          </div>
        </div>

        <TranscriptPanel
          fullWidth={activeView === 'transcript'}
          hidden={activeView === 'summary'}
          transcripts={meetingData.transcripts}
          customPrompt={customPrompt}
          onPromptChange={handlePromptChange}
          onCopyTranscript={copyOperations.handleCopyTranscript}
          onOpenMeetingFolder={meetingOperations.handleOpenMeetingFolder}
          isRecording={isRecording}
          disableAutoScroll={true}
          // Pagination props for efficient loading
          usePagination={true}
          segments={segments}
          hasMore={hasMore}
          isLoadingMore={isLoadingMore}
          totalCount={totalCount}
          loadedCount={loadedCount}
          onLoadMore={onLoadMore}
          // Retranscription props
          meetingId={meeting.id}
          meetingFolderPath={meeting.folder_path}
          onRefetchTranscripts={handleRefetchTranscriptsAndSummary}
        />
        <SummaryPanel
          hidden={activeView === 'transcript'}
          meeting={meeting}
          meetingTitle={meetingData.meetingTitle}
          onTitleChange={meetingData.handleTitleChange}
          isEditingTitle={meetingData.isEditingTitle}
          onStartEditTitle={() => meetingData.setIsEditingTitle(true)}
          onFinishEditTitle={() => { void meetingData.handleFinishTitleEditing(); }}
          isTitleDirty={meetingData.isTitleDirty}
          summaryRef={meetingData.blockNoteSummaryRef}
          isSaving={meetingData.isSaving}
          onSaveAll={meetingData.saveAllChanges}
          onCopySummary={copyOperations.handleCopySummary}
          onOpenFolder={meetingOperations.handleOpenMeetingFolder}
          aiSummary={meetingData.aiSummary}
          summaryStatus={summaryGeneration.summaryStatus}
          generationStartedAt={summaryGeneration.summaryStartedAt}
          transcripts={meetingData.transcripts}
          modelConfig={modelConfig}
          setModelConfig={setModelConfig}
          onSaveModelConfig={handleSaveModelConfig}
          onGenerateSummary={summaryGeneration.handleGenerateSummary}
          onStopGeneration={summaryGeneration.handleStopGeneration}
          customPrompt={customPrompt}
          summaryResponse={summaryResponse}
          onSaveSummary={meetingData.handleSaveSummary}
          onSummaryChange={meetingData.handleSummaryChange}
          onDirtyChange={meetingData.setIsSummaryDirty}
          summaryError={summaryGeneration.summaryError}
          onRegenerateSummary={summaryGeneration.handleRegenerateSummary}
          getSummaryStatusMessage={summaryGeneration.getSummaryStatusMessage}
          availableTemplates={templates.availableTemplates}
          selectedTemplate={templates.selectedTemplate}
          selectedTemplateName={templates.selectedTemplateName}
          templatePreferenceMode={templates.preference?.mode ?? null}
          templateStorage={templates.storage}
          templateIssue={templates.issue}
          templateError={templates.error}
          isTemplateLoading={templates.isLoading}
          isTemplateSaving={templates.isSaving}
          onTemplateSelect={templates.handleTemplateSelection}
          onUseGlobalDefault={templates.handleUseGlobalDefault}
          onTemplateRetry={templates.handleRetry}
          isModelConfigLoading={false}
          onOpenModelSettings={handleRegisterModalOpen}
          onRequestModelSettings={handleOpenModelSettings}
        />
      </div>
    </motion.div>
  );
}

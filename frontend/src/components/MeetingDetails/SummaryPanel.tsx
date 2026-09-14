"use client";

import { HelpHint } from '@/components/ui/help-hint';
import { Summary, SummaryResponse, Transcript } from '@/types';
import { EditableTitle } from '@/components/EditableTitle';
import { BlockNoteSummaryView, BlockNoteSummaryViewRef } from '@/components/AISummary/BlockNoteSummaryView';
import { EmptyStateSummary } from '@/components/EmptyStateSummary';
import { ModelConfig } from '@/components/ModelSettingsModal';
import { SummaryGeneratorButtonGroup } from './SummaryGeneratorButtonGroup';
import { SummaryUpdaterButtonGroup } from './SummaryUpdaterButtonGroup';
import { SummaryEvidencePanel } from './SummaryEvidencePanel';
import Analytics from '@/lib/analytics';
import { useEffect, useMemo, useRef, useState, RefObject } from 'react';
import { toast } from 'sonner';
import { Languages, ChevronDown } from 'lucide-react';
import { Button } from '@/components/ui/button';
import { Popover, PopoverTrigger, PopoverContent } from '@/components/ui/popover';
import { LanguagePickerPopover } from '@/components/LanguagePickerPopover';
import { useRecentLanguages } from '@/hooks/useRecentLanguages';
import { localizedLabelForCode } from '@/lib/summary-languages';
import { useTranslation } from 'react-i18next';
import {
  readMeetingSummaryLanguage,
  saveMeetingSummaryLanguage,
  SummaryLanguageStorage,
} from '@/lib/summary-language-preferences';
import type {
  MeetingTemplateIssue,
  MeetingTemplateMode,
  MeetingTemplateStorage,
  TemplateApiError,
  TemplateListItem,
} from '@/types/summary-template';
import type { SummaryRegenerationMode } from '@/hooks/meeting-details/useSummaryGeneration';
import { readSummaryFreshness, readSummarySourceBinding } from '@/lib/summary-source';
import type { SummaryStaleReason, SummaryFieldTrace } from '@/types/summary-source';
import { formatDateTime } from '@/i18n/formatters';
import type { SupportedUiLocale } from '@/i18n/types';

const factWarningTranslationKeys = {
  missing_meeting_context: 'factValidation.missingMeetingContext',
  meeting_date_mismatch: 'factValidation.meetingDateMismatch',
  attendance_conflict: 'factValidation.attendanceConflict',
  absence_conflict: 'factValidation.absenceConflict',
  host_conflict: 'factValidation.hostConflict',
  person_name_conflict: 'factValidation.personNameConflict',
  missing_transcript_evidence: 'factValidation.missingTranscriptEvidence',
  unsupported_transcript_term: 'factValidation.unsupportedTranscriptTerm',
  unsupported_year: 'factValidation.unsupportedYear',
  unsupported_acronym_expansion: 'factValidation.unsupportedAcronymExpansion',
  unsupported_organization: 'factValidation.unsupportedOrganization',
  unsupported_status_claim: 'factValidation.unsupportedStatusClaim',
  unsupported_security_claim: 'factValidation.unsupportedSecurityClaim',
  untraceable_action_owner: 'factValidation.untraceableActionOwner',
  untraceable_action_time: 'factValidation.untraceableActionTime',
  untraceable_action_dependency: 'factValidation.untraceableActionDependency',
  manual_high_risk_fields_unverified: 'factValidation.manualHighRiskFieldsUnverified',
  summary_validation_unavailable: 'factValidation.validationUnavailable',
  summary_markdown_unavailable: 'factValidation.summaryMarkdownUnavailable',
} as const;

const EMPTY_FIELD_TRACES: SummaryFieldTrace[] = [];

const summaryStaleReasonTranslationKeys = {
  transcript_version_changed: 'sourceFreshness.transcriptVersionChanged',
  transcript_content_changed: 'sourceFreshness.transcriptContentChanged',
  speaker_bindings_changed: 'sourceFreshness.speakerBindingsChanged',
  template_changed: 'sourceFreshness.templateChanged',
  source_binding_unavailable: 'sourceFreshness.sourceBindingUnavailable',
} as const satisfies Record<SummaryStaleReason, string>;

interface SummaryPanelProps {
  meeting: {
    id: string;
    title: string;
    created_at: string;
  };
  meetingTitle: string;
  onTitleChange: (title: string) => void;
  isEditingTitle: boolean;
  onStartEditTitle: () => void;
  onFinishEditTitle: () => void;
  isTitleDirty: boolean;
  summaryRef: RefObject<BlockNoteSummaryViewRef>;
  isSaving: boolean;
  onSaveAll: () => Promise<void>;
  onCopySummary: () => Promise<void>;
  onOpenFolder: () => Promise<void>;
  aiSummary: Summary | null;
  summaryStatus: 'idle' | 'processing' | 'summarizing' | 'regenerating' | 'completed' | 'needs_review' | 'error';
  generationStartedAt?: number | null;
  transcripts: Transcript[];
  modelConfig: ModelConfig;
  setModelConfig: (config: ModelConfig | ((prev: ModelConfig) => ModelConfig)) => void;
  onSaveModelConfig: (config?: ModelConfig) => Promise<void>;
  onGenerateSummary: (customPrompt: string) => Promise<void>;
  onStopGeneration: () => void;
  customPrompt: string;
  summaryResponse: SummaryResponse | null;
  onSaveSummary: (summary: Summary | { markdown?: string; summary_json?: any[] }) => Promise<void>;
  onSummaryChange: (summary: Summary) => void;
  onDirtyChange: (isDirty: boolean) => void;
  summaryError: string | null;
  onRegenerateSummary: (
    mode?: SummaryRegenerationMode,
    historicalGenerationId?: string,
    customPrompt?: string,
    templateIdOverride?: string,
  ) => Promise<void>;
  getSummaryStatusMessage: (status: 'idle' | 'processing' | 'summarizing' | 'regenerating' | 'completed' | 'needs_review' | 'error') => string;
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
  isModelConfigLoading?: boolean;
  onOpenModelSettings?: (openFn: () => void) => void;
  /** 底部操作条上的主按钮要打开模型设置时，走这个外部入口（弹窗由顶部图标簇持有） */
  onRequestModelSettings?: () => void;
  /** PRO 版式里两个视图靠 CSS 隐藏切换，不走卸载，免得丢掉编辑器和滚动状态 */
  hidden?: boolean;
}

export function SummaryPanel({
  meeting,
  meetingTitle,
  onTitleChange,
  isEditingTitle,
  onStartEditTitle,
  onFinishEditTitle,
  isTitleDirty,
  summaryRef,
  isSaving,
  onSaveAll,
  onCopySummary,
  onOpenFolder,
  aiSummary,
  summaryStatus,
  generationStartedAt,
  transcripts,
  modelConfig,
  setModelConfig,
  onSaveModelConfig,
  onGenerateSummary,
  onStopGeneration,
  customPrompt,
  summaryResponse,
  onSaveSummary,
  onSummaryChange,
  onDirtyChange,
  summaryError,
  onRegenerateSummary,
  getSummaryStatusMessage,
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
  isModelConfigLoading = false,
  onOpenModelSettings,
  onRequestModelSettings,
  hidden = false
}: SummaryPanelProps) {
  const { t, i18n } = useTranslation('summary');
  const [summaryLang, setSummaryLang] = useState<string | null>(null);
  const [summaryLangStorage, setSummaryLangStorage] = useState<SummaryLanguageStorage>('metadata');
  const [langPickerOpen, setLangPickerOpen] = useState(false);
  const languageLoadVersionRef = useRef(0);
  const activeMeetingIdRef = useRef(meeting.id);
  const languageSaveVersionRef = useRef(0);
  const languageSaveLoopRunningRef = useRef(false);
  const latestLanguageSaveRequestRef = useRef<{
    version: number;
    meetingId: string;
    language: string | null;
    rollback: {
      language: string | null;
      storage: SummaryLanguageStorage;
    };
  } | null>(null);
  activeMeetingIdRef.current = meeting.id;
  const { addRecent } = useRecentLanguages();
  // 保存按钮只在有未保存改动时出现，所以这里得跟着编辑器的 dirty 事件走
  const [isSummaryDirtyLocal, setIsSummaryDirtyLocal] = useState(false);
  const handleSummaryDirtyChange = (dirty: boolean) => {
    setIsSummaryDirtyLocal(dirty);
    onDirtyChange(dirty);
  };

  const effectiveLangLabel = summaryLang
    ? localizedLabelForCode(summaryLang, i18n.resolvedLanguage || i18n.language)
    : t('labels.auto');
  const isLocalFallbackLanguage = summaryLangStorage === 'local_fallback';
  const autoSubtitle = isLocalFallbackLanguage
    ? t('languagePicker.savedOnDevice')
    : t('languagePicker.usesDominantTranscriptLanguage');

  useEffect(() => {
    let cancelled = false;
    const loadVersion = languageLoadVersionRef.current + 1;
    languageLoadVersionRef.current = loadVersion;

    const loadSummaryLanguage = async () => {
      try {
        const stored = await readMeetingSummaryLanguage(meeting.id);
        if (!cancelled && languageLoadVersionRef.current === loadVersion) {
          setSummaryLang(stored.language);
          setSummaryLangStorage(stored.storage);
        }
      } catch (err) {
        console.error('Failed to load summary language:', err);
        toast.warning(t('errors.couldNotLoadSavedSummaryLanguage'), {
          description: t('descriptions.usingAutoUntilMeetingMetadataCanBeRead'),
        });
        if (!cancelled && languageLoadVersionRef.current === loadVersion) setSummaryLang(null);
      }
    };

    loadSummaryLanguage();

    return () => {
      cancelled = true;
    };
  }, [meeting.id]);

  const persistLatestLanguageSelection = async () => {
    if (languageSaveLoopRunningRef.current) return;
    languageSaveLoopRunningRef.current = true;

    try {
      while (true) {
        const request = latestLanguageSaveRequestRef.current;
        if (!request) return;

        try {
          const saved = await saveMeetingSummaryLanguage(request.meetingId, request.language);
          const latest = latestLanguageSaveRequestRef.current;
          if (
            latest?.version === request.version &&
            activeMeetingIdRef.current === request.meetingId
          ) {
            setSummaryLang(saved.language);
            setSummaryLangStorage(saved.storage);
            if (saved.storage === 'local_fallback') {
              toast.info(t('messages.summaryLanguageSavedOnThisDevice'), {
                description: t('descriptions.thisMeetingHasNoRecordingFolderSoThePreferenceCannot'),
              });
            }
            if (request.language) {
              addRecent(request.language);
            }
            return;
          }

          if (latest?.version === request.version) return;
        } catch (err) {
          const latest = latestLanguageSaveRequestRef.current;
          if (
            latest?.version === request.version &&
            activeMeetingIdRef.current === request.meetingId
          ) {
            console.error('Failed to persist summary language:', err);
            toast.error(t('errors.failedToSaveSummaryLanguage'));
            setSummaryLang(request.rollback.language);
            setSummaryLangStorage(request.rollback.storage);
            return;
          }

          console.warn('Ignoring failed stale summary language save:', err);
          if (latest?.version === request.version) return;
        }
      }
    } finally {
      languageSaveLoopRunningRef.current = false;
    }
  };

  const handleLangChange = (code: string | null) => {
    const previous = summaryLang;
    const previousStorage = summaryLangStorage;
    const nextStored = code;
    languageLoadVersionRef.current += 1;
    latestLanguageSaveRequestRef.current = {
      version: languageSaveVersionRef.current + 1,
      meetingId: meeting.id,
      language: nextStored,
      rollback: {
        language: previous,
        storage: previousStorage,
      },
    };
    languageSaveVersionRef.current += 1;
    setSummaryLang(nextStored);
    setLangPickerOpen(false);
    void persistLatestLanguageSelection();
  };

  const isSummaryLoading = summaryStatus === 'processing' || summaryStatus === 'summarizing' || summaryStatus === 'regenerating';
  const factValidation = (aiSummary as any)?.factValidation as {
    status?: string;
    warnings?: Array<{ code?: string; messageKey?: string }>;
    fieldTraces?: SummaryFieldTrace[];
  } | undefined;
  const factNeedsReview = factValidation
    ? factValidation.status === 'needs_review'
    : summaryStatus === 'needs_review';
  const factWarnings = factValidation?.warnings ?? [];
  const recordedSourceBinding = readSummarySourceBinding(aiSummary);
  const persistedFreshness = readSummaryFreshness(aiSummary);
  const selectedTemplateRecord = availableTemplates.find(
    (template) => template.id === selectedTemplate,
  );
  const selectedTemplateChanged = Boolean(
    recordedSourceBinding &&
    selectedTemplateRecord &&
    (recordedSourceBinding.template.templateId !== selectedTemplateRecord.id ||
      recordedSourceBinding.template.templateVersion !== selectedTemplateRecord.version),
  );
  const sourceStaleReasons = new Set<SummaryStaleReason>(persistedFreshness?.reasons ?? []);
  if (selectedTemplateChanged) sourceStaleReasons.add('template_changed');
  if (!recordedSourceBinding && !persistedFreshness) {
    sourceStaleReasons.add('source_binding_unavailable');
  }
  const sourceFreshnessUnavailable =
    persistedFreshness?.status === 'unavailable' ||
    sourceStaleReasons.has('source_binding_unavailable');
  const summarySourceNeedsReview =
    persistedFreshness?.status === 'stale' ||
    sourceFreshnessUnavailable ||
    selectedTemplateChanged;
  const displayedSummaryStatus = isSummaryLoading || summaryStatus === 'error'
    ? summaryStatus
    : factValidation?.status === 'needs_review'
      ? 'needs_review'
      : summaryStatus === 'needs_review'
        ? 'completed'
        : summaryStatus;

  // P1-8：整篇小节几乎都是「未提及」时，多半是模板和这场会议的内容不匹配。
  // 这里只做"解释 + 一键换模板"的兜底，不改模型的生成逻辑。
  const emptySummaryNotice = useMemo(() => {
    if (!aiSummary || isSummaryLoading || summaryStatus === 'error') return null;
    const record = aiSummary as unknown as {
      markdown?: unknown;
      english_cache?: { markdown?: unknown };
    };
    // 顶层 markdown 有时是精简版（用「本节未注明」这类短写法），
    // english_cache.markdown 才是完整版，所以取更长的那份来判断。
    const candidates = [record.markdown, record.english_cache?.markdown].filter(
      (value): value is string => typeof value === 'string' && value.trim().length > 0,
    );
    if (!candidates.length) return null;
    const markdown = candidates.reduce((longest, value) =>
      value.length > longest.length ? value : longest,
    );
    const placeholders = (
      markdown.match(/未提及|未注明|未记录|not mentioned|none noted|not specified/gi) ?? []
    ).length;
    const boldSections = (markdown.match(/^\*\*[^*\n]+\*\*\s*$/gm) ?? []).length;
    const headingSections = (markdown.match(/^#{1,4}\s+\S/gm) ?? []).length;
    const sections = Math.max(boldSections, headingSections, 1);
    if (placeholders < 3 || placeholders * 2 < sections) return null;
    return { placeholders, sections };
  }, [aiSummary, isSummaryLoading, summaryStatus]);

  const standardTemplate = useMemo(
    () =>
      availableTemplates.find((template) => template.id === 'standard_meeting') ??
      availableTemplates.find((template) => /标准会议纪要|standard meeting/i.test(template.name)),
    [availableTemplates],
  );

  const handleSwitchToStandardTemplate = () => {
    if (!standardTemplate) return;
    Analytics.trackButtonClick('switch_template_from_empty_summary', 'meeting_details');
    onTemplateSelect(standardTemplate.id, standardTemplate.name);
    void onRegenerateSummary('latest', undefined, customPrompt, standardTemplate.id);
  };

  // UX：原来三条警告各占一个大黄块，正文被挤掉一大截。
  // 现在合并成"一行提示 + 展开看明细"，默认不占正文篇幅。
  const reviewNoticeItems = useMemo(() => {
    const items: Array<{
      key: string;
      title: string;
      description: string;
      bullets: string[];
    }> = [];

    if (summarySourceNeedsReview) {
      items.push({
        key: 'source-freshness',
        title: t(
          sourceFreshnessUnavailable
            ? 'sourceFreshness.unavailableTitle'
            : 'sourceFreshness.staleTitle',
        ),
        description: t(
          sourceFreshnessUnavailable
            ? 'sourceFreshness.unavailableDescription'
            : 'sourceFreshness.staleDescription',
        ),
        bullets: Array.from(sourceStaleReasons).map((reason) =>
          t(summaryStaleReasonTranslationKeys[reason]),
        ),
      });
    }

    if (factNeedsReview) {
      items.push({
        key: 'fact-validation',
        title: t('factValidation.needsReviewTitle'),
        description: t('factValidation.needsReviewDescription'),
        bullets: factWarnings.map((warning) =>
          t(
            factWarningTranslationKeys[
              warning.code as keyof typeof factWarningTranslationKeys
            ] ?? 'factValidation.genericWarning',
          ),
        ),
      });
    }

    if (emptySummaryNotice) {
      items.push({
        key: 'empty-summary',
        title: t('emptySummary.title'),
        description: t('emptySummary.description', {
          template: selectedTemplateName,
          count: emptySummaryNotice.placeholders,
        }),
        bullets: [],
      });
    }

    return items;
  }, [
    emptySummaryNotice,
    factNeedsReview,
    factWarnings,
    selectedTemplateName,
    sourceFreshnessUnavailable,
    sourceStaleReasons,
    summarySourceNeedsReview,
    t,
  ]);

  // P1-11：生成过程只有一句"正在生成 AI 摘要…"，用户不知道要等多久。
  // 这里显示真实已用时间 + 按转写长度估算的耗时（不伪造精确百分比）。
  const [generationElapsedSeconds, setGenerationElapsedSeconds] = useState(0);
  useEffect(() => {
    if (!isSummaryLoading || generationStartedAt === null) {
      setGenerationElapsedSeconds(0);
      return;
    }
    const startedAt = generationStartedAt ?? Date.now();
    const updateElapsed = () => {
      setGenerationElapsedSeconds(Math.max(0, Math.floor((Date.now() - startedAt) / 1000)));
    };
    updateElapsed();
    const timer = setInterval(updateElapsed, 1000);
    return () => clearInterval(timer);
  }, [isSummaryLoading, generationStartedAt]);

  const estimatedGenerationSeconds = useMemo(() => {
    const transcriptChars = transcripts.reduce(
      (total, transcript) => total + (transcript.text?.length ?? 0),
      0,
    );
    // 实测：约 450 字的转写用本地 qwen3.5:4b 需要 50–65 秒，所以这里给的是"至少"的估计
    const estimate = 45 + Math.round(transcriptChars / 1000) * 10;
    return Math.min(240, Math.max(45, estimate));
  }, [transcripts]);

  const languageSlot = (
    <Popover open={langPickerOpen} onOpenChange={setLangPickerOpen}>
      <PopoverTrigger asChild>
        <Button
          variant="outline"
          size="sm"
          title={isLocalFallbackLanguage
            ? t('accessibility.languageLocalFallback', { language: effectiveLangLabel })
            : t('accessibility.language', { language: effectiveLangLabel })}
          aria-label={t('accessibility.setSummaryLanguage')}
        >
          <Languages size={18} />
          <span className="hidden lg:inline">{effectiveLangLabel}</span>
          <ChevronDown size={14} className="text-gray-400" />
        </Button>
      </PopoverTrigger>
      <PopoverContent
        align="end"
        className="w-auto p-0 border-0 shadow-none bg-transparent"
      >
        <LanguagePickerPopover
          value={summaryLang}
          onChange={handleLangChange}
          onClose={() => setLangPickerOpen(false)}
          autoSubtitle={autoSubtitle}
        />
      </PopoverContent>
    </Popover>
  );

  // PRO 版式：标题右边显示会议的时间，正文里那一行 7 个入口全部收走
  const uiLocale: SupportedUiLocale = i18n.resolvedLanguage === 'zh-CN' ? 'zh-CN' : 'en';
  const meetingTimeLabel = useMemo(() => {
    if (!meeting.created_at) return null;
    const value = new Date(meeting.created_at);
    if (Number.isNaN(value.getTime())) return null;
    return {
      date: formatDateTime(value, uiLocale, { year: 'numeric', month: 'short', day: 'numeric' }),
      time: formatDateTime(value, uiLocale, { hour: '2-digit', minute: '2-digit' }),
    };
  }, [meeting.created_at, uiLocale]);
  const hasTranscripts = transcripts.length > 0;
  const showSaveButton = isTitleDirty || isSummaryDirtyLocal;

  const summaryGeneratorProps = {
    meetingId: meeting.id,
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
    hasTranscripts,
    isModelConfigLoading,
    onManualSummaryRestored: (summary: Record<string, unknown>) => onSummaryChange(summary as Summary),
  } as const;

  return (
    <div
      className={
        hidden
          ? 'hidden'
          : 'relative flex-1 min-w-0 flex flex-col bg-white overflow-hidden'
      }
    >
      {/* PRO 版式：入口全部收进右上角图标簇，正文顶部只剩标题和时间 */}
      {hasTranscripts && (
        <div
          className="absolute right-4 top-3 z-30 flex items-center gap-0.5 rounded-full border border-gray-200 bg-white/95 px-1 py-1 shadow-sm backdrop-blur"
          role="group"
          aria-label={t('accessibility.summarySettings')}
        >
          <SummaryGeneratorButtonGroup
            {...summaryGeneratorProps}
            layout="toolbar"
            onOpenModelSettings={onOpenModelSettings}
            onOpenFolder={onOpenFolder}
            isSummaryDirty={showSaveButton}
            onSaveSummaryChanges={onSaveAll}
          />
        </div>
      )}

      {isSummaryLoading ? (
        <div className="flex flex-col h-full">
          {/* 生成中：主按钮已经变「停止」在底部操作条里，这里只留进度 */}
          <div className="flex items-center justify-center flex-1">
            <div className="text-center">
              <div className="inline-block animate-spin rounded-full h-12 w-12 border-t-2 border-b-2 border-blue-500 mb-4"></div>
              <p className="text-gray-600">{t('status.generatingAISummary')}</p>
              {/* P1-11：真实已用时间 + 估算耗时，避免用户不知道要等多久 */}
              <p className="mt-2 text-sm text-gray-500">
                {generationStartedAt !== null && <>{t('progress.elapsed', { seconds: generationElapsedSeconds })} ·{' '}</>}
                {t('progress.estimate', { seconds: estimatedGenerationSeconds })}
              </p>
              <div className="mx-auto mt-3 h-1.5 w-64 overflow-hidden rounded-full bg-gray-200">
                <div
                  className="h-full bg-blue-500 transition-all duration-1000 ease-out"
                  style={{
                    width: `${Math.min(
                      95,
                      Math.round((generationElapsedSeconds / estimatedGenerationSeconds) * 100),
                    )}%`,
                  }}
                />
              </div>
            </div>
          </div>
        </div>
      ) : !aiSummary ? (
        <div className="mx-auto flex h-full w-full max-w-3xl flex-col overflow-y-auto pb-28">
          {/* 标题 + 会议时间（Pro 版式） */}
          <div className="flex items-start justify-between gap-6 px-6 pt-16">
            <EditableTitle
              title={meetingTitle}
              isEditing={isEditingTitle}
              onStartEditing={onStartEditTitle}
              onFinishEditing={onFinishEditTitle}
              onChange={onTitleChange}
              editLabel={t('accessibility.editMeetingTitle')}
            />
            {meetingTimeLabel && (
              <div className="shrink-0 pt-1 text-right text-sm leading-tight text-gray-500">
                <div className="font-medium text-gray-600">{meetingTimeLabel.date}</div>
                <div>{meetingTimeLabel.time}</div>
              </div>
            )}
          </div>
          {/* Empty state message —— 生成入口统一在底部操作条，避免同一屏两个同样的按钮 */}
          <div className="min-h-0 flex-1">
            <EmptyStateSummary
              onGenerate={() => onGenerateSummary(customPrompt)}
              hasModel={modelConfig.provider !== null && modelConfig.model !== null}
              isGenerating={isSummaryLoading}
              isDisabled={isTemplateLoading || isTemplateSaving || Boolean(templateIssue || templateError)}
              showAction={false}
            />
          </div>
        </div>
      ) : transcripts?.length > 0 && (
        <div className="flex-1 overflow-y-auto min-h-0">
          {summaryResponse && (
            <div className="fixed bottom-0 left-0 right-0 bg-white shadow-lg p-4 max-h-1/3 overflow-y-auto">
              <h3 className="text-lg font-semibold mb-2">{t('labels.meetingSummary')}</h3>
              <div className="grid grid-cols-2 gap-4">
                <div className="bg-white p-4 rounded-lg shadow-sm">
                  <h4 className="font-medium mb-1">{t('labels.keyPoints')}</h4>
                  <ul className="list-disc pl-4">
                    {summaryResponse.summary.key_points.blocks.map((block, i) => (
                      <li key={i} className="text-sm">{block.content}</li>
                    ))}
                  </ul>
                </div>
                <div className="bg-white p-4 rounded-lg shadow-sm mt-4">
                  <h4 className="font-medium mb-1">{t('labels.actionItems')}</h4>
                  <ul className="list-disc pl-4">
                    {summaryResponse.summary.action_items.blocks.map((block, i) => (
                      <li key={i} className="text-sm">{block.content}</li>
                    ))}
                  </ul>
                </div>
                <div className="bg-white p-4 rounded-lg shadow-sm mt-4">
                  <h4 className="font-medium mb-1">{t('labels.decisions')}</h4>
                  <ul className="list-disc pl-4">
                    {summaryResponse.summary.decisions.blocks.map((block, i) => (
                      <li key={i} className="text-sm">{block.content}</li>
                    ))}
                  </ul>
                </div>
                <div className="bg-white p-4 rounded-lg shadow-sm mt-4">
                  <h4 className="font-medium mb-1">{t('labels.mainTopics')}</h4>
                  <ul className="list-disc pl-4">
                    {summaryResponse.summary.main_topics.blocks.map((block, i) => (
                      <li key={i} className="text-sm">{block.content}</li>
                    ))}
                  </ul>
                </div>
              </div>
              {summaryResponse.raw_summary ? (
                <div className="mt-4">
                  <h4 className="font-medium mb-1">{t('labels.fullSummary')}</h4>
                  <p className="text-sm whitespace-pre-wrap">{summaryResponse.raw_summary}</p>
                </div>
              ) : null}
            </div>
          )}
          <div className="mx-auto w-full max-w-3xl px-6 pt-16 pb-28">
            {/* 标题 + 会议时间（Pro 版式：正文顶部只剩这两样） */}
            <div className="mb-6 flex items-start justify-between gap-6">
              <EditableTitle
                title={meetingTitle}
                isEditing={isEditingTitle}
                onStartEditing={onStartEditTitle}
                onFinishEditing={onFinishEditTitle}
                onChange={onTitleChange}
                editLabel={t('accessibility.editMeetingTitle')}
              />
              {meetingTimeLabel && (
                <div className="shrink-0 pt-1 text-right text-sm leading-tight text-gray-500">
                  <div className="font-medium text-gray-600">{meetingTimeLabel.date}</div>
                  <div>{meetingTimeLabel.time}</div>
                </div>
              )}
            </div>
            {reviewNoticeItems.length > 0 && (
              <div className="mb-3 flex flex-wrap items-center gap-1 text-xs text-gray-600">
                <span>{t('reviewNotice.summary', { count: reviewNoticeItems.length })}</span>
                <HelpHint label={t('reviewNotice.summary', { count: reviewNoticeItems.length })}>
                  <div className="space-y-3">
                    {reviewNoticeItems.map(item => <div key={item.key}>
                      <p className="font-medium text-gray-800">{item.title}</p>
                      <p className="mt-1">{item.description}</p>
                      {item.bullets.length > 0 && <ul className="mt-1 list-disc space-y-1 pl-4">
                        {item.bullets.map((bullet, index) => <li key={`${item.key}-${index}`}>{bullet}</li>)}
                      </ul>}
                    </div>)}
                  </div>
                </HelpHint>
                {reviewNoticeItems.some(item => item.key === 'empty-summary') && standardTemplate && (
                  <Button variant="outline" size="sm" onClick={handleSwitchToStandardTemplate}>
                    {t('emptySummary.switchAction', { template: standardTemplate.name })}
                  </Button>
                )}
              </div>
            )}
            <SummaryEvidencePanel
              traces={factValidation?.fieldTraces ?? EMPTY_FIELD_TRACES}
              transcripts={transcripts}
              disabled={isSummaryDirtyLocal || summarySourceNeedsReview}
            />
            <BlockNoteSummaryView
              ref={summaryRef}
              summaryData={aiSummary}
              onSave={onSaveSummary}
              onSummaryChange={onSummaryChange}
              onDirtyChange={handleSummaryDirtyChange}
              status={summaryStatus}
              error={summaryError}
              onRegenerateSummary={() => {
                Analytics.trackButtonClick('regenerate_summary', 'meeting_details');
                onRegenerateSummary(undefined, undefined, customPrompt);
              }}
              meeting={{
                id: meeting.id,
                title: meetingTitle,
                created_at: meeting.created_at
              }}
            />
          </div>
          {/* 需核对的状态已经在上面那条紧凑提示里了，这里不再重复一条横幅 */}
          {displayedSummaryStatus !== 'idle' &&
            !(displayedSummaryStatus === 'needs_review' && reviewNoticeItems.length > 0) && (
            <div className={`mt-3 text-sm ${displayedSummaryStatus === 'error' ? 'text-red-700' : 'text-gray-600'}`}>
              <p className="text-sm font-medium">{getSummaryStatusMessage(displayedSummaryStatus)}</p>
            </div>
          )}
        </div>
      )}

      {/* PRO 版式：底部居中悬浮操作条 —— 复制 / 保存（仅 dirty）/ 生成·停止 / 摘要语言 */}
      {(hasTranscripts || Boolean(aiSummary)) && (
        <div className="pointer-events-none absolute inset-x-0 bottom-6 z-20 flex justify-center px-4">
          <div className="pointer-events-auto flex items-center gap-1 rounded-full border border-gray-200 bg-white/95 p-1.5 shadow-lg backdrop-blur">
            <SummaryUpdaterButtonGroup
              isSaving={isSaving}
              isDirty={showSaveButton}
              onSave={onSaveAll}
              onCopy={onCopySummary}
              onFind={() => {
                // TODO: Implement find in summary functionality
                console.log('Find in summary clicked');
              }}
              hasSummary={!!aiSummary}
            />

            <span className="mx-0.5 h-5 w-px bg-gray-200" aria-hidden="true" />

            <SummaryGeneratorButtonGroup
              {...summaryGeneratorProps}
              layout="primary"
              hasSummary={!!aiSummary}
              isSummaryDirty={showSaveButton}
              onSaveSummaryChanges={onSaveAll}
              onRequestModelSettings={onRequestModelSettings}
              languageSlot={languageSlot}
            />
          </div>
        </div>
      )}
    </div>
  );
}

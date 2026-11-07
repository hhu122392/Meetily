"use client";

import { MeetingSummary, Summary, Transcript } from '@/types';
import { BlockNoteSummaryView, BlockNoteSummaryViewRef } from '@/components/AISummary/BlockNoteSummaryView';
import { EmptyStateSummary } from '@/components/EmptyStateSummary';
import { ModelConfig } from '@/components/ModelSettingsModal';
import { SummaryGeneratorButtonGroup } from './SummaryGeneratorButtonGroup';
import { SummaryUpdaterButtonGroup } from './SummaryUpdaterButtonGroup';
import Analytics from '@/lib/analytics';
import { useEffect, useRef, useState, RefObject } from 'react';
import { toast } from 'sonner';
import { Languages, ChevronDown } from 'lucide-react';
import { Button } from '@/components/ui/button';
import { Popover, PopoverTrigger, PopoverContent } from '@/components/ui/popover';
import { LanguagePickerPopover } from '@/components/LanguagePickerPopover';
import { useRecentLanguages } from '@/hooks/useRecentLanguages';
import { labelForCode } from '@/lib/summary-languages';
import {
  readMeetingSummaryLanguage,
  saveMeetingSummaryLanguage,
  SummaryLanguageStorage,
} from '@/lib/summary-language-preferences';
import { hasVisibleSummaryContent } from '@/lib/summary-content';
import { legacySummaryText } from '@/lib/lark';
import { LarkSendButton } from './LarkSendButton';
import { SummaryReviewActions } from './SummaryReviewActions';
import { t, useUiLanguage } from "@/i18n";

interface SummaryPanelProps {
  meeting: {
    id: string;
    title: string;
    created_at: string;
  };
  meetingTitle: string;
  isSummaryDirty: boolean;
  summaryRef: RefObject<BlockNoteSummaryViewRef>;
  isSaving: boolean;
  onSaveAll: () => Promise<void>;
  onCopySummary: () => Promise<void>;
  aiSummary: MeetingSummary | null;
  summaryStatus: 'idle' | 'processing' | 'summarizing' | 'regenerating' | 'completed' | 'error';
  transcripts: Transcript[];
  modelConfig: ModelConfig;
  setModelConfig: (config: ModelConfig | ((prev: ModelConfig) => ModelConfig)) => void;
  onSaveModelConfig: (config?: ModelConfig) => Promise<void>;
  onGenerateSummary: (customPrompt: string) => Promise<void>;
  onStopGeneration: () => void;
  customPrompt: string;
  onSaveSummary: (summary: MeetingSummary) => Promise<void>;
  onSummaryChange: (summary: Summary) => void;
  onDirtyChange: (isDirty: boolean) => void;
  summaryError: string | null;
  onRegenerateSummary: () => Promise<void>;
  getSummaryStatusMessage: (status: 'idle' | 'processing' | 'summarizing' | 'regenerating' | 'completed' | 'error') => string;
  availableTemplates: Array<{ id: string, name: string, description: string }>;
  selectedTemplate: string;
  onTemplateSelect: (templateId: string, templateName: string) => void;
  isModelConfigLoading?: boolean;
  onOpenModelSettings?: (openFn: () => void) => void;
}

export function SummaryPanel({
  meeting,
  meetingTitle,
  isSummaryDirty,
  summaryRef,
  isSaving,
  onSaveAll,
  onCopySummary,
  aiSummary,
  summaryStatus,
  transcripts,
  modelConfig,
  setModelConfig,
  onSaveModelConfig,
  onGenerateSummary,
  onStopGeneration,
  customPrompt,
  onSaveSummary,
  onSummaryChange,
  onDirtyChange,
  summaryError,
  onRegenerateSummary,
  getSummaryStatusMessage,
  availableTemplates,
  selectedTemplate,
  onTemplateSelect,
  isModelConfigLoading = false,
  onOpenModelSettings,
}: SummaryPanelProps) {
  useUiLanguage();
  const [summaryLang, setSummaryLang] = useState<string | null>(null);
  const [summaryLangStorage, setSummaryLangStorage] = useState<SummaryLanguageStorage>('metadata');
  const [langPickerOpen, setLangPickerOpen] = useState(false);
  const [reviewToolbarTarget, setReviewToolbarTarget] = useState<HTMLSpanElement | null>(null);
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

  const effectiveLangLabel = summaryLang ? labelForCode(summaryLang) : 'Auto';
  const isLocalFallbackLanguage = summaryLangStorage === 'local_fallback';
  const autoSubtitle = isLocalFallbackLanguage
    ? 'Saved on this device for folderless meetings'
    : 'Uses dominant transcript language';

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
        toast.warning('Could not load saved summary language', {
          description: 'Using Auto until meeting metadata can be read.',
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
              toast.info('Summary language saved on this device', {
                description: 'This meeting has no recording folder, so the preference cannot be written to meeting metadata.',
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
            toast.error('Failed to save summary language');
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
  const hasSummary = hasVisibleSummaryContent(aiSummary);

  const languageSlot = (
    <Popover open={langPickerOpen} onOpenChange={setLangPickerOpen}>
      <PopoverTrigger asChild>
        <Button
          variant="ghost"
          size="sm"
          className="h-8 rounded-full px-2 text-gray-600 hover:text-gray-900"
          title={`${t('Summary Language')}: ${t(effectiveLangLabel)}`}
          aria-label={t("Set summary language")}
        >
          <Languages size={18} />
          <span className="hidden @[40rem]:inline">{t(effectiveLangLabel)}</span>
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
          autoSubtitle={t(autoSubtitle)}
        />
      </PopoverContent>
    </Popover>
  );

  return (
    <div className="relative flex-1 min-w-0 flex flex-col bg-white overflow-hidden h-full w-full @container">
      {(transcripts.length > 0 || hasSummary) && <div className="flex shrink-0 justify-end px-4 py-3">
        <div className="flex max-w-full flex-wrap items-center justify-end gap-0.5 rounded-2xl border border-gray-200 bg-white/95 p-1 shadow-sm" role="group" aria-label="纪要工具">
          <span ref={setReviewToolbarTarget} className="contents" />
          <SummaryGeneratorButtonGroup layout="toolbar" modelConfig={modelConfig} setModelConfig={setModelConfig}
              onSaveModelConfig={onSaveModelConfig} onGenerateSummary={onGenerateSummary}
              onStopGeneration={onStopGeneration} customPrompt={customPrompt} summaryStatus={summaryStatus}
              availableTemplates={availableTemplates} selectedTemplate={selectedTemplate} onTemplateSelect={onTemplateSelect}
              hasTranscripts={transcripts.length > 0} hasSummary={hasSummary} isModelConfigLoading={isModelConfigLoading} onOpenModelSettings={onOpenModelSettings} />
          {hasSummary && !isSummaryLoading && (<LarkSendButton compact
                key={meeting.id}
                meetingId={meeting.id}
                title={meetingTitle}
                createdAt={meeting.created_at}
                disabled={isSaving}
                getSummary={async () => {
                  if (!aiSummary) return '';
                  if ('markdown' in aiSummary || 'summary_json' in aiSummary) {
                    if (!summaryRef.current) throw new Error('Editor not ready');
                    return summaryRef.current.getMarkdown({ strict: true });
                  }
                  return legacySummaryText(aiSummary);
                }}
              />)}
        </div>
      </div>}
      {(transcripts.length > 0 || hasSummary) && <div className="pointer-events-none absolute inset-x-0 bottom-6 z-20 flex justify-center px-3">
        <div className="pointer-events-auto flex max-w-full items-center gap-1 rounded-full border border-gray-200 bg-white/95 p-1.5 shadow-lg backdrop-blur" role="group" aria-label="纪要操作">
          {hasSummary && !isSummaryLoading && <>
            <SummaryUpdaterButtonGroup isSaving={isSaving} isDirty={isSummaryDirty} onSave={onSaveAll} onCopy={onCopySummary} />
            <span className="mx-0.5 h-5 w-px shrink-0 bg-gray-200" aria-hidden="true" />
          </>}
          <SummaryGeneratorButtonGroup layout="primary" modelConfig={modelConfig} setModelConfig={setModelConfig}
              onSaveModelConfig={onSaveModelConfig} onGenerateSummary={onGenerateSummary}
              onStopGeneration={onStopGeneration} customPrompt={customPrompt} summaryStatus={summaryStatus}
              availableTemplates={availableTemplates} selectedTemplate={selectedTemplate} onTemplateSelect={onTemplateSelect}
              hasTranscripts={transcripts.length > 0} hasSummary={hasSummary} isModelConfigLoading={isModelConfigLoading} languageSlot={languageSlot} />
        </div>
      </div>}

      {hasSummary && !isSummaryLoading && <SummaryReviewActions key={`review-${meeting.id}`}
        meetingId={meeting.id} disabled={isSaving} toolbarTarget={reviewToolbarTarget} saveEdits={async () => {
          if (!summaryRef.current) throw new Error('纪要编辑器尚未加载，请稍后重试。');
          if (summaryRef.current.isDirty) await summaryRef.current.saveSummary();
        }} />}
      {isSummaryLoading ? (
        <div className="flex items-center justify-center flex-1">
          <div className="text-center">
            <div className="inline-block animate-spin rounded-full h-12 w-12 border-t-2 border-b-2 border-blue-500 mb-4"></div>
            <p className="text-gray-600">{t("Generating AI Summary...")}</p>
          </div>
        </div>
      ) : !hasSummary ? (
        <EmptyStateSummary
          onGenerate={() => onGenerateSummary(customPrompt)}
          hasModel={modelConfig.provider !== null && modelConfig.model !== null}
          isGenerating={isSummaryLoading}
          error={summaryError}
        />
      ) : (
        <div className="flex-1 overflow-y-auto overflow-x-auto min-h-0">
          <div className="mx-auto w-full max-w-3xl px-4 pt-4 pb-28 sm:px-6">
            <BlockNoteSummaryView
              key={meeting.id}
              ref={summaryRef}
              summaryData={aiSummary}
              onSave={onSaveSummary}
              onSummaryChange={onSummaryChange}
              onDirtyChange={onDirtyChange}
              status={summaryStatus}
              error={summaryError}
              onRegenerateSummary={() => {
                Analytics.trackButtonClick('regenerate_summary', 'meeting_details');
                onRegenerateSummary();
              }}
              meeting={{
                id: meeting.id,
                title: meetingTitle,
                created_at: meeting.created_at
              }}
            />
          </div>
          {summaryStatus !== 'idle' && (
            <div className={`mt-4 p-4 rounded-lg ${summaryStatus === 'error' ? 'bg-red-100 text-red-700' :
              summaryStatus === 'completed' ? 'bg-green-100 text-green-700' :
                'bg-blue-100 text-blue-700'
              }`}>
              <p className="text-sm font-medium">{getSummaryStatusMessage(summaryStatus)}</p>
            </div>
          )}
        </div>
      )}
    </div>
  );
}

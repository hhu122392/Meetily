import { useState, useCallback, useRef, useEffect } from 'react';
import { Transcript, Summary } from '@/types';
import { ModelConfig } from '@/components/ModelSettingsModal';
import { CurrentMeeting, useSidebar } from '@/components/Sidebar/SidebarProvider';
import { invoke as invokeTauri } from '@tauri-apps/api/core';
import { toast } from 'sonner';
import Analytics from '@/lib/analytics';
import { isOllamaNotInstalledError } from '@/lib/utils';
import { BuiltInModelInfo } from '@/lib/builtin-ai';
import { useTranslation } from 'react-i18next';
import type { TFunction } from 'i18next';
import type { SummarySourceBinding } from '@/types/summary-source';
import { observeExistingSummaryTask, type NativeSummarySnapshot } from '@/lib/summary-task-resume';
import {
  detectAndCacheSummaryLanguage,
  readMeetingSummaryLanguage,
  readCachedDetectedSummaryLanguage,
} from '@/lib/summary-language-preferences';

async function resolveSummaryLanguage(
  meetingId: string,
  transcriptTexts: string[],
  t: TFunction<'summary'>,
): Promise<string | null> {
  try {
    const perMeeting = await readMeetingSummaryLanguage(meetingId);
    if (perMeeting.language) return perMeeting.language;
  } catch (err) {
    console.warn('Failed to load meeting summary language:', err);
    toast.warning(t('errors.couldNotLoadSavedSummaryLanguage'), {
      description: t('descriptions.usingAutoForThisGeneration'),
    });
  }

  try {
    const cachedDetected = await readCachedDetectedSummaryLanguage(meetingId);
    if (cachedDetected) return cachedDetected;
  } catch (err) {
    console.warn('Failed to load cached detected summary language:', err);
  }

  try {
    const detection = await detectAndCacheSummaryLanguage(meetingId, transcriptTexts);
    if (detection.reason === 'tie') {
      toast.warning(t('labels.bilingualTranscriptDetected'), {
        description: t('descriptions.pickASummaryLanguageManuallyIfAutoChoosesTheWrong'),
      });
    }
    return detection.language;
  } catch (err) {
    console.warn('Failed to detect transcript summary language:', err);
    return null;
  }
}

type SummaryStatus = 'idle' | 'processing' | 'summarizing' | 'regenerating' | 'completed' | 'needs_review' | 'error';

function completionStatusForSummary(data: any): SummaryStatus {
  return data?.factValidation?.status === 'needs_review' ? 'needs_review' : 'completed';
}

function isSummarySourceBindingFailure(error: unknown): boolean {
  return (
    typeof error === 'object' &&
    error !== null &&
    'code' in error &&
    error.code === 'SUMMARY_SOURCE_BINDING_FAILED'
  );
}

interface UseSummaryGenerationProps {
  meeting: any;
  transcripts: Transcript[];
  modelConfig: ModelConfig;
  isModelConfigLoading: boolean;
  selectedTemplate: string;
  setAiSummary: (summary: Summary | null) => void;
  onOpenModelSettings?: () => void;
}

interface ProcessTranscriptResponseV2 {
  message: string;
  processId?: string;
  process_id?: string;
  generationId: string;
  resolvedTemplate: {
    id: string;
    version: number;
    fileSha256: string;
    semanticSha256: string;
    resolutionSource: 'meeting_override' | 'global_default' | 'builtin_fallback' | 'historical_snapshot';
  };
  snapshotPathRelative: string;
  sourceBinding: SummarySourceBinding;
}

interface BeginSummaryMeasurementResponse {
  generationId: string;
  timedEndpointId: 'click_to_page_display_complete';
}

const monotonicNowNs = (): number => Math.round(performance.now() * 1_000_000);

const waitForPagePaint = (): Promise<void> => {
  if (typeof requestAnimationFrame !== 'function') {
    return Promise.resolve();
  }
  return new Promise(resolve => {
    requestAnimationFrame(() => requestAnimationFrame(() => resolve()));
  });
};

interface SummaryTemplateSnapshotListItem {
  generationId: string;
  capturedAt: string;
  templateId: string;
  templateName: string;
  templateVersion: number;
  fileSha256: string;
  resolutionSource: 'meeting_override' | 'global_default' | 'builtin_fallback' | 'historical_snapshot';
}

export type SummaryRegenerationMode = 'historical' | 'latest';

export function useSummaryGeneration({
  meeting,
  transcripts,
  modelConfig,
  isModelConfigLoading,
  selectedTemplate,
  setAiSummary,
  onOpenModelSettings,
}: UseSummaryGenerationProps) {
  const { t } = useTranslation('summary');
  const [summaryStatus, setSummaryStatus] = useState<SummaryStatus>('idle');
  const [summaryError, setSummaryError] = useState<string | null>(null);
  const [summaryStartedAt, setSummaryStartedAt] = useState<number | null>(null);
  const generationRequestInFlightRef = useRef(false);
  const activeMeasurementGenerationIdRef = useRef<string | null>(null);
  const stopResumedTaskRef = useRef<() => void>(() => {});

  useEffect(() => {
    setSummaryStatus('idle');
    setSummaryError(null);
    setSummaryStartedAt(null);
    const stop = observeExistingSummaryTask({
      read: () => invokeTauri<NativeSummarySnapshot>('api_get_summary', { meetingId: meeting.id }),
      isSuperseded: () => generationRequestInFlightRef.current,
      onUpdate: update => {
        setSummaryStatus(update.status);
        setSummaryError(update.error);
        setSummaryStartedAt(update.startedAt ?? null);
        if (update.data) setAiSummary(update.data as Summary);
      },
    });
    stopResumedTaskRef.current = stop;
    return stop;
  }, [meeting.id, setAiSummary]);

  const { startSummaryPolling, stopSummaryPolling } = useSidebar();

  const finishSummaryMeasurement = useCallback(async (
    generationId: string | undefined,
    outcome: 'failed' | 'cancelled' | 'save_failed',
    reason: string,
  ) => {
    if (!generationId) return;
    try {
      await invokeTauri('api_finish_summary_measurement', {
        meetingId: meeting.id,
        generationId,
        outcome,
        reason,
      });
    } catch (error) {
      console.warn('Failed to close D-12 summary measurement:', error);
    } finally {
      if (activeMeasurementGenerationIdRef.current === generationId) {
        activeMeasurementGenerationIdRef.current = null;
      }
    }
  }, [meeting.id]);

  const beginSummaryMeasurement = useCallback(async (): Promise<string | undefined> => {
    const clickAt = new Date().toISOString();
    const clickMonotonicNs = monotonicNowNs();
    try {
      const response = await invokeTauri<BeginSummaryMeasurementResponse>(
        'api_begin_summary_measurement',
        {
          meetingId: meeting.id,
          clickAt,
          clickMonotonicNs,
        },
      );
      if (response.timedEndpointId !== 'click_to_page_display_complete') {
        throw new Error('unexpected-summary-timed-endpoint');
      }
      activeMeasurementGenerationIdRef.current = response.generationId;
      return response.generationId;
    } catch (error) {
      // Observability must not make an otherwise valid summary unusable.  A run
      // without this record is mechanically excluded from D-12 evidence.
      console.warn('D-12 summary measurement could not start:', error);
      return undefined;
    }
  }, [meeting.id]);

  const recordWaitForTranscription = useCallback(async (
    generationId: string | undefined,
    monotonicStartNs: number,
    monotonicEndNs: number,
  ): Promise<boolean> => {
    if (!generationId) return true;
    try {
      await invokeTauri('api_record_summary_frontend_stage', {
        meetingId: meeting.id,
        generationId,
        stage: 'wait_for_transcription',
        monotonicStartNs,
        monotonicEndNs,
      });
      return true;
    } catch (error) {
      console.warn('Failed to record wait_for_transcription:', error);
      await finishSummaryMeasurement(
        generationId,
        'failed',
        'wait_for_transcription_measurement_failed',
      );
      return false;
    }
  }, [meeting.id, finishSummaryMeasurement]);

  // Helper to get status message
  const getSummaryStatusMessage = useCallback((status: SummaryStatus) => {
    switch (status) {
      case 'processing':
        return t('status.processingTranscript');
      case 'summarizing':
        return t('status.generating');
      case 'regenerating':
        return t('status.regenerating');
      case 'completed':
        return t('status.completed');
      case 'needs_review':
        return t('status.needsReview');
      case 'error':
        return t('errors.errorGeneratingSummary');
      default:
        return '';
    }
  }, [t]);

  // Unified summary processing logic
  const processSummary = useCallback(async ({
    transcriptText,
    transcriptTexts,
    customPrompt = '',
    isRegeneration = false,
    historicalGenerationId,
    measurementGenerationId,
    templateIdOverride,
  }: {
    transcriptText: string;
    transcriptTexts?: string[];
    customPrompt?: string;
    isRegeneration?: boolean;
    historicalGenerationId?: string;
    measurementGenerationId?: string;
    /** P1-8：允许调用方显式指定模板（用于"模板不匹配 → 一键换标准模板重生"） */
    templateIdOverride?: string;
  }) => {
    setSummaryStatus(isRegeneration ? 'regenerating' : 'processing');
    setSummaryError(null);
    setSummaryStartedAt(Date.now());

    try {
      if (!transcriptText.trim()) {
        throw new Error('no-transcript-text');
      }

      const effectiveTemplateId = templateIdOverride ?? selectedTemplate;
      console.log('Processing transcript with template:', effectiveTemplateId);

      // Calculate time since recording
      const timeSinceRecording = (Date.now() - new Date(meeting.created_at).getTime()) / 60000; // minutes

      // Track summary generation started
      await Analytics.trackSummaryGenerationStarted(
        modelConfig.provider,
        modelConfig.model,
        transcriptText.length,
        timeSinceRecording
      );

      // Track custom prompt usage if present
      if (customPrompt.trim().length > 0) {
        await Analytics.trackCustomPromptUsed(customPrompt.trim().length);
      }

      // Show toast notification for generation start
      toast.info(isRegeneration ? t('status.regenerating') : t('status.generating'), {
        description: t('status.usingModel', { provider: modelConfig.provider, model: modelConfig.model }),
        duration: 3000,
      });

      // Resolve explicit metadata override first; Auto detects the transcript language.
      const summaryLanguage = await resolveSummaryLanguage(
        meeting.id,
        transcriptTexts?.length ? transcriptTexts : [transcriptText],
        t,
      );

      // Process transcript and get process_id
      const result = await invokeTauri<ProcessTranscriptResponseV2>('api_process_transcript', {
        text: transcriptText,
        model: modelConfig.provider,
        modelName: modelConfig.model,
        meetingId: meeting.id,
        chunkSize: 40000,
        overlap: 1000,
        customPrompt: customPrompt,
        templateId: effectiveTemplateId,
        historicalGenerationId: historicalGenerationId ?? null,
        summaryLanguage,
        measurementGenerationId: measurementGenerationId ?? null,
      });

      const process_id = result.processId ?? result.process_id;
      if (!process_id) {
        throw new Error('summary-process-id-missing');
      }
      console.log('Process ID:', process_id, 'Generation ID:', result.generationId);
      if (measurementGenerationId && result.generationId !== measurementGenerationId) {
        throw new Error('summary-measurement-generation-mismatch');
      }

      // Start global polling via context
      startSummaryPolling(meeting.id, process_id, async (pollingResult) => {
        console.log('Summary status:', pollingResult);

        // Handle cancellation
        if (pollingResult.status === 'cancelled') {
          console.log('Summary generation was cancelled');

          // Reload summary from database (backend has already restored from backup)
          try {
            const existingSummary = await invokeTauri('api_get_summary', {
              meetingId: meeting.id
            }) as any;

            if (existingSummary?.data) {
              console.log('Restored previous summary after cancellation');
              setAiSummary(existingSummary.data);
              setSummaryStatus(completionStatusForSummary(existingSummary.data));
            } else {
              setSummaryStatus('idle');
            }
          } catch (error) {
            console.error('Failed to reload summary after cancellation:', error);
            setSummaryStatus('idle');
          }

          setSummaryError(null);
          await finishSummaryMeasurement(
            measurementGenerationId,
            'cancelled',
            'backend_reported_cancelled',
          );
          return;
        }

        // Handle errors
        if (pollingResult.status === 'error' || pollingResult.status === 'failed') {
          console.error('Backend returned error:', pollingResult.error);
          const errorMessage = pollingResult.error || `Summary ${isRegeneration ? 'regeneration' : 'generation'} failed`;

          // If this was a regeneration, try to restore previous summary from database
          if (isRegeneration) {
            try {
              const existingSummary = await invokeTauri('api_get_summary', {
                meetingId: meeting.id
              }) as any;

              if (existingSummary?.data) {
                console.log('Restored previous summary after regeneration failure');
                setAiSummary(existingSummary.data);
                setSummaryStatus(completionStatusForSummary(existingSummary.data));
                setSummaryError(null);

                // Show error toast with restoration message
                toast.error(t('errors.failedToRegenerateSummary'), {
                  description: t('descriptions.valueYourPreviousSummaryHasBeenRestored', {
                    errorMessage: t('errors.genericDetails'),
                  }),
                });

                await Analytics.trackSummaryGenerationCompleted(
                  modelConfig.provider,
                  modelConfig.model,
                  false,
                  undefined,
                  errorMessage
                );
                await finishSummaryMeasurement(
                  measurementGenerationId,
                  'failed',
                  'backend_reported_failed',
                );
                return;
              }
            } catch (error) {
              console.error('Failed to reload summary after error:', error);
            }
          }

          // Continue with normal error handling if not regeneration or reload failed
          setSummaryError('generationFailed');
          setSummaryStatus('error');

          // Check if this is a "model is required" error
          const isModelRequiredError = errorMessage === 'model_unavailable' ||
            errorMessage === 'model_authentication';

          // Show error toast
          const userDescription = errorMessage === 'model_connection'
            ? t('descriptions.unableToConnectToLLMService')
            : t('errors.genericDetails');
          toast.error(isRegeneration ? t('errors.regenerateFailed') : t('errors.generateFailed'), {
            description: userDescription,
          });

          // Auto-open model settings modal if model is missing
          if (isModelRequiredError && onOpenModelSettings) {
            console.log('🔧 Model required error detected, opening model settings...');
            onOpenModelSettings();
          }

          await Analytics.trackSummaryGenerationCompleted(
            modelConfig.provider,
            modelConfig.model,
            false,
            undefined,
            errorMessage
          );
          await finishSummaryMeasurement(
            measurementGenerationId,
            'failed',
            'backend_reported_failed',
          );
          return;
        }

        // Handle successful completion
        if (pollingResult.status === 'completed' && pollingResult.data) {
          console.log('Summary generation completed:', pollingResult.data);

          // Check if backend returned markdown format (new flow)
          if (pollingResult.data.markdown) {
            console.log('Received markdown format from backend');
            const pageDisplayStageStartNs = monotonicNowNs();
            setAiSummary({
              ...pollingResult.data,
              markdown: pollingResult.data.markdown,
              factValidation: pollingResult.data.factValidation,
            } as any);
            const completionStatus = completionStatusForSummary(pollingResult.data);
            setSummaryStatus(completionStatus);

            await waitForPagePaint();
            const pageDisplayCompleteMonotonicNs = monotonicNowNs();
            if (measurementGenerationId) {
              try {
                await invokeTauri('api_record_summary_page_completion', {
                  meetingId: meeting.id,
                  generationId: measurementGenerationId,
                  pageDisplayedBody: pollingResult.data.markdown,
                  pageDisplayCompleteAt: new Date().toISOString(),
                  stageStartMonotonicNs: pageDisplayStageStartNs,
                  stageEndMonotonicNs: pageDisplayCompleteMonotonicNs,
                });
                if (activeMeasurementGenerationIdRef.current === measurementGenerationId) {
                  activeMeasurementGenerationIdRef.current = null;
                }
              } catch (error) {
                console.warn('Failed to record summary page_display_complete:', error);
                await finishSummaryMeasurement(
                  measurementGenerationId,
                  'failed',
                  'page_display_completion_failed',
                );
              }
            }

            if (completionStatus === 'needs_review') {
              toast.warning(t('messages.summaryGeneratedNeedsReview'), {
                description: t('descriptions.summaryFactWarningsNeedReview'),
                duration: 6000,
              });
            } else {
              toast.success(t('messages.summaryGeneratedSuccessfully'), {
                description: t('descriptions.yourMeetingSummaryIsReady'),
                duration: 4000,
              });
            }

            await Analytics.trackSummaryGenerationCompleted(
              modelConfig.provider,
              modelConfig.model,
              true
            );
            return;
          }

          // Legacy format handling
          const summarySections = Object.entries(pollingResult.data).filter(([key]) => key !== 'MeetingName');
          const allEmpty = summarySections.every(([, section]) => !(section as any).blocks || (section as any).blocks.length === 0);

          if (allEmpty) {
            console.error('Summary completed but all sections empty');
            setSummaryError('emptyContent');
            setSummaryStatus('error');

            await Analytics.trackSummaryGenerationCompleted(
              modelConfig.provider,
              modelConfig.model,
              false,
              undefined,
              'Empty summary generated'
            );
            await finishSummaryMeasurement(
              measurementGenerationId,
              'failed',
              'legacy_summary_empty',
            );
            return;
          }

          // Remove MeetingName from data before formatting
          const { MeetingName, ...summaryData } = pollingResult.data;

          // Format legacy summary data
          const formattedSummary: Summary = {};
          const sectionKeys = pollingResult.data._section_order || Object.keys(summaryData);

          for (const key of sectionKeys) {
            try {
              const section = summaryData[key];
              if (section && typeof section === 'object' && 'title' in section && 'blocks' in section) {
                const typedSection = section as { title?: string; blocks?: any[] };

                if (Array.isArray(typedSection.blocks)) {
                  formattedSummary[key] = {
                    title: typedSection.title || key,
                    blocks: typedSection.blocks.map((block: any) => ({
                      ...block,
                      color: 'default',
                      content: block?.content?.trim() || ''
                    }))
                  };
                } else {
                  formattedSummary[key] = {
                    title: typedSection.title || key,
                    blocks: []
                  };
                }
              }
            } catch (error) {
              console.warn(`Error processing section ${key}:`, error);
            }
          }

          setAiSummary(formattedSummary);
          setSummaryStatus(completionStatusForSummary(pollingResult.data));

          // Show success toast
          toast.success(t('messages.summaryGeneratedSuccessfully'), {
            description: t('descriptions.yourMeetingSummaryIsReady'),
            duration: 4000,
          });

          await Analytics.trackSummaryGenerationCompleted(
            modelConfig.provider,
            modelConfig.model,
            true
          );

          await finishSummaryMeasurement(
            measurementGenerationId,
            'failed',
            'legacy_summary_has_no_hashable_markdown_body',
          );

        }
      });
    } catch (error) {
      console.error(`Failed to ${isRegeneration ? 'regenerate' : 'generate'} summary:`, error);
      const errorMessage = error instanceof Error ? error.message : 'unknown-error';
      setSummaryError('generationFailed');
      setSummaryStatus('error');
      // Note: We don't clear the summary here because the backend has already restored from backup

      toast.error(isRegeneration ? t('errors.regenerateFailed') : t('errors.generateFailed'), {
        description: isSummarySourceBindingFailure(error)
          ? t('errors.sourceBindingFailed')
          : t('errors.genericDetails'),
      });

      await Analytics.trackSummaryGenerationCompleted(
        modelConfig.provider,
        modelConfig.model,
        false,
        undefined,
        errorMessage
      );
      await finishSummaryMeasurement(
        measurementGenerationId,
        'failed',
        'frontend_or_preflight_failed',
      );
    }
  }, [
    meeting.id,
    meeting.created_at,
    modelConfig,
    selectedTemplate,
    startSummaryPolling,
    setAiSummary,
    finishSummaryMeasurement,
    t,
  ]);

  // Helper function to fetch ALL transcripts for summary generation
  const fetchAllTranscripts = useCallback(async (meetingId: string): Promise<Transcript[]> => {
    try {
      console.log('📊 Fetching all transcripts for meeting:', meetingId);

      // First, get total count by fetching first page
      const firstPage = await invokeTauri('api_get_meeting_transcripts', {
        meetingId,
        limit: 1,
        offset: 0,
      }) as { transcripts: Transcript[]; total_count: number; has_more: boolean };

      const totalCount = firstPage.total_count;
      console.log(`📊 Total transcripts in database: ${totalCount}`);

      if (totalCount === 0) {
        return [];
      }

      // Fetch all transcripts in one call
      const allData = await invokeTauri('api_get_meeting_transcripts', {
        meetingId,
        limit: totalCount,
        offset: 0,
      }) as { transcripts: Transcript[]; total_count: number; has_more: boolean };

      console.log(`✅ Fetched ${allData.transcripts.length} transcripts from database`);
      return allData.transcripts;
    } catch (error) {
      console.error('❌ Error fetching all transcripts:', error);
      toast.error(t('errors.failedToFetchTranscriptsForSummaryGeneration'));
      return [];
    }
  }, [t]);

  const buildSummaryTranscriptPayload = useCallback((allTranscripts: Transcript[]) => {
    const formatTime = (seconds: number | undefined, fallbackTimestamp: string): string => {
      if (seconds === undefined) {
        return fallbackTimestamp;
      }
      const totalSecs = Math.floor(seconds);
      const mins = Math.floor(totalSecs / 60);
      const secs = totalSecs % 60;
      return `[${mins.toString().padStart(2, '0')}:${secs.toString().padStart(2, '0')}]`;
    };

    return {
      transcriptText: allTranscripts
        .map(t => `${formatTime(t.audio_start_time, t.timestamp)} ${t.text}`)
        .join('\n'),
      transcriptTexts: allTranscripts.map(t => t.text),
    };
  }, []);

  // Public API: Generate summary from transcripts
  const handleGenerateSummary = useCallback(async (customPrompt: string = '') => {
    if (generationRequestInFlightRef.current) return;
    stopResumedTaskRef.current();
    generationRequestInFlightRef.current = true;
    let measurementGenerationId = await beginSummaryMeasurement();
    let handedToSummaryPipeline = false;
    try {
    // Check if model config is still loading
    if (isModelConfigLoading) {
      console.log('⏳ Model configuration is still loading, please wait...');
      toast.info(t('messages.loadingModelConfigurationPleaseWait'));
      return;
    }

    // CHANGE: Fetch ALL transcripts from database, not from pagination state
    console.log('📊 Fetching all transcripts for summary generation...');
    const waitForTranscriptionStartNs = monotonicNowNs();
    const allTranscripts = await fetchAllTranscripts(meeting.id);
    const waitForTranscriptionEndNs = monotonicNowNs();
    const waitMeasurementRecorded = await recordWaitForTranscription(
      measurementGenerationId,
      waitForTranscriptionStartNs,
      waitForTranscriptionEndNs,
    );
    if (!waitMeasurementRecorded) measurementGenerationId = undefined;

    if (!allTranscripts.length) {
      console.log('No transcripts available for summary');
      toast.error(t('errors.noTranscriptsAvailableForSummary'));
      return;
    }

    console.log(`✅ Proceeding with ${allTranscripts.length} transcripts`);

    console.log('🚀 Starting summary generation with config:', {
      provider: modelConfig.provider,
      model: modelConfig.model,
      template: selectedTemplate
    });

    // Check if Ollama provider has models available
    if (modelConfig.provider === 'ollama') {
      try {
        const endpoint = modelConfig.ollamaEndpoint || null;
        const models = await invokeTauri('get_ollama_models', { endpoint }) as any[];

        if (!models || models.length === 0) {
          toast.error(
            t('errors.noOllamaModelsFoundPleaseDownloadGemma31bFromModel'),
            { duration: 5000 }
          );
          return;
        }
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
                onClick: () => invokeTauri('open_external_url', { url: 'https://ollama.com/download' })
              }
            }
          );
        } else {
          // Other error - generic message
          toast.error(
            t('errors.failedToCheckOllamaModelsPleaseEnsureOllamaIsRunning'),
            { duration: 5000 }
          );
        }
        return;
      }
    }

    // Check if built-in AI provider has models available
    if (modelConfig.provider === 'builtin-ai') {
      try {
        const selectedModel = modelConfig.model;

        if (!selectedModel) {
          toast.error(t('errors.noBuiltInAIModelSelected'), {
            description: t('descriptions.pleaseSelectAModelInSettings'),
            duration: 5000,
          });
          if (onOpenModelSettings) {
            onOpenModelSettings();
          }
          return;
        }

        // Check model readiness with filesystem refresh
        const isReady = await invokeTauri<boolean>('builtin_ai_is_model_ready', {
          modelName: selectedModel,
          refresh: true,
        });

        if (!isReady) {
          // Get detailed model status
          const modelInfo = await invokeTauri<BuiltInModelInfo | null>('builtin_ai_get_model_info', {
            modelName: selectedModel,
          });

          if (modelInfo) {
            const status = modelInfo.status;

            if (status.type === 'downloading') {
              toast.info(t('messages.modelDownloadInProgress'), {
                description: t('descriptions.valueIsDownloading', { selectedModel, progress: status.progress }),
                duration: 5000,
              });
              return;
            }

            if (status.type === 'not_downloaded') {
              toast.error(t('errors.builtInAIModelNotDownloaded'), {
                description: t('descriptions.valueNeedsToBeDownloadedPleaseDownloadItInModel', { selectedModel }),
                duration: 7000,
              });
              if (onOpenModelSettings) {
                onOpenModelSettings();
              }
              return;
            }

            if (status.type === 'corrupted' || status.type === 'error') {
              toast.error(t('errors.builtInAIModelNotAvailable'), {
                description: status.type === 'corrupted'
                  ? t('descriptions.valueFileIsCorruptedPleaseDeleteAndReDownload', { selectedModel })
                  : t('errors.genericDetails'),
                duration: 7000,
              });
              if (onOpenModelSettings) {
                onOpenModelSettings();
              }
              return;
            }
          }

          // Fallback if we couldn't get model info
          toast.error(t('errors.builtInAIModelNotReady'), {
            description: t('descriptions.pleaseEnsureTheModelIsDownloadedInSettings'),
            duration: 5000,
          });
          if (onOpenModelSettings) {
            onOpenModelSettings();
          }
          return;
        }

        // Model is ready, continue to backend call
      } catch (error) {
        console.error('Error validating built-in AI model:', error);
        toast.error(t('errors.failedToValidateBuiltInAIModel'), {
          description: t('errors.genericDetails'),
          duration: 5000,
        });
        return;
      }
    }

    const summaryPayload = buildSummaryTranscriptPayload(allTranscripts);

    handedToSummaryPipeline = true;
    await processSummary({
      ...summaryPayload,
      customPrompt,
      measurementGenerationId,
    });
    } finally {
      if (!handedToSummaryPipeline) {
        await finishSummaryMeasurement(
          measurementGenerationId,
          'failed',
          'frontend_preflight_did_not_start_summary',
        );
      }
      generationRequestInFlightRef.current = false;
    }
  }, [meeting.id, fetchAllTranscripts, buildSummaryTranscriptPayload, processSummary, modelConfig, isModelConfigLoading, selectedTemplate, onOpenModelSettings, t, beginSummaryMeasurement, recordWaitForTranscription, finishSummaryMeasurement]);

  // Public API: Regenerate summary from the current saved transcript
  const handleRegenerateSummary = useCallback(async (
    mode: SummaryRegenerationMode = 'historical',
    requestedHistoricalGenerationId?: string,
    customPrompt: string = '',
    templateIdOverride?: string,
  ) => {
    if (generationRequestInFlightRef.current) return;
    stopResumedTaskRef.current();
    generationRequestInFlightRef.current = true;
    let measurementGenerationId = await beginSummaryMeasurement();
    let handedToSummaryPipeline = false;
    try {
    const waitForTranscriptionStartNs = monotonicNowNs();
    const allTranscripts = await fetchAllTranscripts(meeting.id);
    const waitForTranscriptionEndNs = monotonicNowNs();
    const waitMeasurementRecorded = await recordWaitForTranscription(
      measurementGenerationId,
      waitForTranscriptionStartNs,
      waitForTranscriptionEndNs,
    );
    if (!waitMeasurementRecorded) measurementGenerationId = undefined;

    if (!allTranscripts.length) {
      console.error('No transcripts available for regeneration');
      toast.error(t('errors.noTranscriptsAvailableForSummaryRegeneration'));
      return;
    }

    let historicalGenerationId: string | undefined = requestedHistoricalGenerationId;
    if (mode === 'historical') {
      if (!historicalGenerationId) {
        try {
          const snapshots = await invokeTauri<SummaryTemplateSnapshotListItem[]>(
            'api_list_meeting_template_snapshots',
            { request: { meetingId: meeting.id } },
          );
          historicalGenerationId = snapshots[0]?.generationId;
          if (!historicalGenerationId) {
            toast.info(t('regeneration.noHistoricalSnapshotTitle'), {
              description: t('regeneration.noHistoricalSnapshotDescription'),
            });
          }
        } catch (error) {
          console.error('Failed to load historical template snapshots:', error);
          toast.error(t('regeneration.snapshotLoadFailedTitle'), {
            description: t('regeneration.snapshotLoadFailedDescription'),
          });
          return;
        }
      }
    }

    handedToSummaryPipeline = true;
    await processSummary({
      ...buildSummaryTranscriptPayload(allTranscripts),
      isRegeneration: true,
      historicalGenerationId,
      measurementGenerationId,
      customPrompt,
      templateIdOverride,
    });
    } finally {
      if (!handedToSummaryPipeline) {
        await finishSummaryMeasurement(
          measurementGenerationId,
          'failed',
          'frontend_preflight_did_not_start_summary',
        );
      }
      generationRequestInFlightRef.current = false;
    }
  }, [meeting.id, fetchAllTranscripts, buildSummaryTranscriptPayload, processSummary, t, beginSummaryMeasurement, recordWaitForTranscription, finishSummaryMeasurement]);

  // Public API: Stop ongoing summary generation
  const handleStopGeneration = useCallback(async () => {
    console.log('Stopping summary generation for meeting:', meeting.id);
    stopResumedTaskRef.current();

    try {
      // Call backend to cancel the summary generation
      await invokeTauri('api_cancel_summary', {
        meetingId: meeting.id
      });
      console.log('✓ Backend cancellation request sent for meeting:', meeting.id);
      await finishSummaryMeasurement(
        activeMeasurementGenerationIdRef.current ?? undefined,
        'cancelled',
        'user_requested_cancel',
      );
    } catch (error) {
      console.error('Failed to cancel summary generation:', error);
      await finishSummaryMeasurement(
        activeMeasurementGenerationIdRef.current ?? undefined,
        'failed',
        'cancel_request_failed',
      );
      // Continue with frontend cleanup even if backend call fails
    }

    // Stop polling
    stopSummaryPolling(meeting.id);

    // Reset status to idle
    setSummaryStatus('idle');
    setSummaryError(null);

    // Show toast notification
    toast.info(t('messages.summaryGenerationStopped'), {
      description: t('descriptions.youCanGenerateANewSummaryAnytime'),
      duration: 3000,
    });
  }, [meeting.id, stopSummaryPolling, t, finishSummaryMeasurement]);

  return {
    summaryStatus,
    summaryError,
    summaryStartedAt,
    handleGenerateSummary,
    handleRegenerateSummary,
    handleStopGeneration,
    getSummaryStatusMessage,
  };
}

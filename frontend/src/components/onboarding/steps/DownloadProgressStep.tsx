import React, { useEffect, useState, useRef } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { Mic, Sparkles, Check, Loader2 } from 'lucide-react';
import { Button } from '@/components/ui/button';
import { OnboardingContainer } from '../OnboardingContainer';
import { useOnboarding } from '@/contexts/OnboardingContext';
import { toast } from 'sonner';
import { HelpHint } from '@/components/ui/help-hint';
import { getSummaryModelSizeMb } from '@/lib/onboarding-summary-model';
import { useTranslation } from 'react-i18next';
import { formatNumber, formatPercent, resolveUiLocale } from '@/i18n';

import { SENSEVOICE_BYTES } from '@/lib/sensevoice';

type DownloadStatus = 'waiting' | 'downloading' | 'completed' | 'error';

interface DownloadState {
  status: DownloadStatus;
  progress: number;
  downloadedMb: number;
  totalMb: number;
  speedMbps: number;
  error?: string;
}

export function DownloadProgressStep() {
  const { t, i18n } = useTranslation('onboarding');
  const locale = resolveUiLocale(i18n.resolvedLanguage || i18n.language);
  const {
    goNext,
    selectedSummaryModel,
    recommendedSummaryModel,
    sensevoice,
    summaryModelDownloaded,
    setSummaryModelDownloaded,
    startBackgroundDownloads,
    completeOnboarding,
  } = useOnboarding();

  const [isMac, setIsMac] = useState(false);

  const sensevoiceDownloaded = sensevoice.state.status === 'available';
  const transcriptionState: DownloadState = {
    status: sensevoiceDownloaded ? 'completed' : sensevoice.state.status === 'error' ? 'error' : sensevoice.state.status === 'downloading' ? 'downloading' : 'waiting',
    progress: sensevoiceDownloaded ? 100 : Math.min(99, sensevoice.state.downloaded_bytes / Math.max(1, sensevoice.state.total_bytes) * 100),
    downloadedMb: sensevoice.state.downloaded_bytes / 1024 / 1024,
    totalMb: SENSEVOICE_BYTES / 1024 / 1024,
    speedMbps: 0,
    error: sensevoice.state.status === 'error' ? 'download failed' : undefined,
  };

  const [summaryState, setSummaryState] = useState<DownloadState>({
    status: summaryModelDownloaded ? 'completed' : 'waiting',
    progress: summaryModelDownloaded ? 100 : 0,
    downloadedMb: 0,
    totalMb: 0,
    speedMbps: 0,
  });

  const [isCompleting, setIsCompleting] = useState(false);
  const transcriptionDownloadStartedRef = useRef(false);
  const summaryDownloadStartedRef = useRef(false);
  const retryingSummaryRef = useRef(false);

  const handleRetryDownload = sensevoice.download;

  // Retry summary download handler
  const handleRetrySummaryDownload = async () => {
    // Prevent multiple simultaneous retries
    if (retryingSummaryRef.current) {
      console.log('[DownloadProgressStep] Summary retry already in progress, ignoring');
      return;
    }

    console.log('[DownloadProgressStep] Retrying summary model download');
    retryingSummaryRef.current = true;

    // Reset error state
    setSummaryState((prev) => ({
      ...prev,
      status: 'downloading',
      error: undefined,
      progress: 0,
      downloadedMb: 0,
      totalMb: getSummaryModelSizeMb(selectedSummaryModel || recommendedSummaryModel),
      speedMbps: 0,
    }));

    try {
      // Call download command directly (no retry command exists for built-in AI)
      const modelName = selectedSummaryModel;
      if (!modelName) {
        throw new Error('Summary model recommendation is not ready yet');
      }
      await invoke('builtin_ai_download_model', { modelName });
    } catch (error) {
      console.error('[DownloadProgressStep] Summary retry failed:', error);
      setSummaryState((prev) => ({
        ...prev,
        status: 'error',
        error: error instanceof Error ? error.message : 'Retry failed',
      }));

      toast.error(t('errors.summaryModelDownloadRetryFailed'), {
        description: t('descriptions.pleaseCheckYourConnectionAndTryAgain'),
      });
    } finally {
      // Allow retry again after 2 seconds
      setTimeout(() => {
        retryingSummaryRef.current = false;
      }, 2000);
    }
  };

  // Detect platform on mount
  useEffect(() => {
    const checkPlatform = async () => {
      try {
        const { platform } = await import('@tauri-apps/plugin-os');
        setIsMac(platform() === 'macos');
      } catch (e) {
        setIsMac(navigator.userAgent.includes('Mac'));
      }
    };

    checkPlatform();
  }, []);

  // Wait for disk/download status, then download the default transcription engine first.
  useEffect(() => {
    if (transcriptionDownloadStartedRef.current || sensevoice.state.status === 'checking') return;
    transcriptionDownloadStartedRef.current = true;
    if (sensevoice.state.status === 'missing' || sensevoice.state.status === 'partial') void sensevoice.download();
  }, [sensevoice.state.status, sensevoice.download]);

  // Start the selected summary model only after the backend recommendation is known.
  useEffect(() => {
    if (summaryDownloadStartedRef.current) return;
    if (!selectedSummaryModel || !sensevoiceDownloaded) return;
    summaryDownloadStartedRef.current = true;

    startSummaryDownload();
  }, [selectedSummaryModel, sensevoiceDownloaded]);

  // Listen to Summary Model download progress (always downloading for builtin-ai)
  useEffect(() => {
    const unlisten = listen<{
      model: string;
      progress: number;
      downloaded_mb?: number;
      total_mb?: number;
      speed_mbps?: number;
      status: string;
      error?: string;
    }>('builtin-ai-download-progress', (event) => {
      const { model, progress, downloaded_mb, total_mb, speed_mbps, status, error } = event.payload;
      if (selectedSummaryModel && model === selectedSummaryModel) {
        setSummaryState((prev) => ({
          ...prev,
          status: status === 'completed'
            ? 'completed'
            : status === 'error'
            ? 'error'
            : 'downloading',
          progress,
          downloadedMb: downloaded_mb ?? prev.downloadedMb,
          totalMb: (total_mb ?? prev.totalMb) || getSummaryModelSizeMb(model),
          speedMbps: speed_mbps ?? prev.speedMbps,
          error: status === 'error' ? error : undefined,
        }));

        if (status === 'completed' || progress >= 100) {
          setSummaryModelDownloaded(true);
        }
      }
    });

    return () => {
      unlisten.then((fn) => fn());
    };
  }, [selectedSummaryModel]);

  useEffect(() => {
    const modelForSize = selectedSummaryModel || recommendedSummaryModel;
    if (!modelForSize) return;

    setSummaryState((prev) => ({
      ...prev,
      status: summaryModelDownloaded
        ? 'completed'
        : prev.status === 'completed'
        ? 'waiting'
        : prev.status,
      progress: summaryModelDownloaded
        ? 100
        : prev.status === 'completed'
        ? 0
        : prev.progress,
      totalMb: prev.totalMb || getSummaryModelSizeMb(modelForSize),
    }));
  }, [selectedSummaryModel, recommendedSummaryModel, summaryModelDownloaded]);

  const startSummaryDownload = async () => {
    if (!summaryModelDownloaded && selectedSummaryModel) {
      try {
        setSummaryState((prev) => ({
          ...prev,
          status: 'downloading',
          totalMb: getSummaryModelSizeMb(selectedSummaryModel),
        }));
        await startBackgroundDownloads({
          includeSummary: true,
          summaryModel: selectedSummaryModel,
        });
      } catch (error) {
        console.error('Failed to start summary model download:', error);
        setSummaryState((prev) => ({ ...prev, status: 'error', error: String(error) }));
      }
    }
  };

  const handleContinue = async () => {
    const actual = await sensevoice.refresh();
    if (actual?.status !== 'available') {
      toast.error(t('errors.transcriptionEngineRequired'));
      return;
    }

    // Check if downloads are complete for toast notification
    const downloadsComplete = transcriptionState.status === 'completed' &&
      summaryState.status === 'completed';

    // Show toast if downloads still in progress
    if (!downloadsComplete) {
      toast.info(t('messages.downloadsWillContinueInTheBackground'), {
        description: t('descriptions.youCanStartUsingTheAppRecordingWillBeAvailable'),
        duration: 5000,
      });
    }

    if (isMac) {
      // macOS: Go to Permissions step (will complete after permissions granted)
      goNext();
    } else {
      // Non-macOS: Complete onboarding immediately (downloads continue in background)
      setIsCompleting(true);
      try {
        await completeOnboarding();

        // Small delay to ensure state is saved before reload
        await new Promise(resolve => setTimeout(resolve, 100));

        window.location.reload();
      } catch (error) {
        console.error('Failed to complete onboarding:', error);
        toast.error(t('errors.failedToCompleteSetup'), {
          description: t('descriptions.pleaseTryAgain'),
        });
        setIsCompleting(false);
      }
    }
  };

  const formatModelSize = (sizeMb: number) => {
    if (sizeMb <= 0) return '';
    const useGiB = sizeMb >= 1024;
    const value = useGiB ? sizeMb / 1024 : sizeMb;
    return t('progress.approximateSize', {
      size: formatNumber(value, locale, {
        minimumFractionDigits: useGiB ? 1 : 0,
        maximumFractionDigits: 1,
      }),
      unit: useGiB ? 'GiB' : 'MiB',
    });
  };

  const renderDownloadCard = (
    kind: 'transcription' | 'summary',
    title: string,
    icon: React.ReactNode,
    state: DownloadState,
    modelSize: string,
    sizeUnit = 'MB'
  ) => (
    <div className="bg-white rounded-xl border border-gray-200 p-5">
      <div className="flex items-center justify-between mb-4">
        <div className="flex items-center gap-3">
          <div className="w-10 h-10 rounded-full bg-gray-100 flex items-center justify-center">
            {icon}
          </div>
          <div>
            <h3 className="font-medium text-gray-900">{title}</h3>
            <p className="text-sm text-gray-500">{modelSize}</p>
          </div>
        </div>
        <div>
          {state.status === 'waiting' && (
            <span className="text-sm text-gray-500">{t('labels.waiting')}</span>
          )}
          {state.status === 'downloading' && (
            <Loader2 className="w-5 h-5 text-gray-700 animate-spin" />
          )}
          {state.status === 'completed' && (
            <div className="w-6 h-6 rounded-full bg-green-100 flex items-center justify-center">
              <Check className="w-4 h-4 text-green-600" />
            </div>
          )}
          {state.status === 'error' && (
            <span className="text-sm text-red-500">{t('labels.failed')}</span>
          )}
        </div>
      </div>

      {/* Progress Bar */}
      {(state.status === 'downloading' || state.status === 'completed') && (
        <div className="space-y-2">
          <div className="w-full h-2 bg-gray-200 rounded-full overflow-hidden">
            <div
              role="progressbar"
              aria-label={title}
              aria-valuemin={0}
              aria-valuemax={100}
              aria-valuenow={Math.round(state.progress)}
              className="h-full bg-gradient-to-r from-gray-700 to-gray-900 rounded-full transition-all duration-300"
              style={{ width: `${state.progress}%` }}
            />
          </div>
          <div className="flex items-center justify-between text-sm">
            <span className="text-gray-600">
              {t('progress.downloaded', {
                downloaded: formatNumber(state.downloadedMb, locale, {
                  minimumFractionDigits: 1,
                  maximumFractionDigits: 1,
                }),
                total: formatNumber(state.totalMb, locale, {
                  minimumFractionDigits: 1,
                  maximumFractionDigits: 1,
                }),
                unit: sizeUnit,
              })}
            </span>
            <div className="flex items-center gap-2">
              {state.speedMbps > 0 && (
                <span className="text-gray-500">
                  {t('progress.speed', {
                    speed: formatNumber(state.speedMbps, locale, {
                      minimumFractionDigits: 1,
                      maximumFractionDigits: 1,
                    }),
                    unit: sizeUnit,
                  })}
                </span>
              )}
              <span className="font-semibold text-gray-900">
                {formatPercent(state.progress / 100, locale, {
                  maximumFractionDigits: 0,
                })}
              </span>
            </div>
          </div>
        </div>
      )}

      {state.status === 'error' && state.error && (
        <div className="mt-2 p-3 bg-red-50 border border-red-200 rounded-md">
          <p className="text-sm text-red-600 font-medium">
            {t('actions.downloadError')}
          </p>
          <p className="text-xs text-red-500 mt-1">
            {t('descriptions.pleaseCheckYourConnectionAndTryAgain')}
          </p>
          {(kind === 'transcription' || kind === 'summary') && (
            <button
              onClick={kind === 'transcription' ? handleRetryDownload : handleRetrySummaryDownload}
              className="mt-3 w-full h-9 px-4 bg-gray-900 hover:bg-gray-800 text-white text-sm font-medium rounded-md transition-colors flex items-center justify-center gap-2"
            >
              <svg className="w-4 h-4" fill="none" viewBox="0 0 24 24" stroke="currentColor">
                <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2}
                      d="M4 4v5h.582m15.356 2A8.001 8.001 0 004.582 9m0 0H9m11 11v-5h-.581m0 0a8.003 8.003 0 01-15.357-2m15.357 2H15" />
              </svg>
              {t('actions.tryAgain')}
            </button>
          )}
        </div>
      )}
    </div>
  );

  return (
    <OnboardingContainer
      title={t('accessibility.gettingThingsReady')}
      description={t('descriptions.youCanStartUsingMeetilyAfterDownloadingTheTranscriptionEngine')}
      step={3}
      totalSteps={isMac ? 4 : 3}
    >
      <div className="flex flex-col items-center space-y-6">
        {/* Download Cards */}
        <div className="w-full max-w-lg space-y-4">
          {renderDownloadCard(
            'transcription',
            'SenseVoice Small (int8)',
            <Mic className="w-5 h-5 text-gray-600" />,
            transcriptionState,
            formatModelSize(SENSEVOICE_BYTES / 1024 / 1024),
            'MiB'
          )}

          {renderDownloadCard(
            'summary',
            t('labels.summaryEngine'),
            <Sparkles className="w-5 h-5 text-gray-600" />,
            summaryState,
            formatModelSize(getSummaryModelSizeMb(selectedSummaryModel || recommendedSummaryModel)),
            'MiB'
          )}
        </div>

        {sensevoiceDownloaded && !summaryModelDownloaded && (
          <div className="flex items-center gap-1 text-xs text-gray-500">
            {t('labels.youCanContinueWhileThisFinishes')}
            <HelpHint>{t('actions.downloadWillContinueInTheBackground')}</HelpHint>
          </div>
        )}

        {/* Continue Button */}
        <div className="w-full max-w-xs">
          <Button
            onClick={handleContinue}
            disabled={!sensevoiceDownloaded || isCompleting}
            className="w-full h-11 bg-gray-900 hover:bg-gray-800 text-white disabled:opacity-50 disabled:cursor-not-allowed"
          >
            {(isCompleting || !sensevoiceDownloaded) ? (
              <Loader2 className="w-4 h-4 mr-2 animate-spin" />
            ) : (
              t('actions.continue')
            )}
          </Button>
        </div>
      </div>
    </OnboardingContainer>
  );
}

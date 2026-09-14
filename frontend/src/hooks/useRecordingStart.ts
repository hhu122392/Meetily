import { useCallback, useEffect, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { toast } from 'sonner';
import { useTranslation } from 'react-i18next';
import { useTranscripts } from '@/contexts/TranscriptContext';
import { useSidebar } from '@/components/Sidebar/SidebarProvider';
import { useConfig } from '@/contexts/ConfigContext';
import { RecordingStatus, useRecordingState } from '@/contexts/RecordingStateContext';
import { recordingService } from '@/services/recordingService';
import Analytics from '@/lib/analytics';
import { showRecordingNotification } from '@/lib/recordingNotification';
import { consumeAutoStartRecordingRequest } from '@/lib/recording-auto-start';
import type { PreparedRecordingMetadata } from '@/types/summary-template';
import { needsMultilingualSetup } from '@/lib/transcription-setup';
import { WhisperAPI } from '@/lib/whisper';

interface UseRecordingStartReturn {
  handleRecordingStart: () => Promise<void>;
  isAutoStarting: boolean;
}

type RecordingStartSource = 'home_page' | 'sidebar_auto' | 'sidebar_direct';

/** Manual and both sidebar entry points share this one preparation path. */
export function useRecordingStart(
  isRecording: boolean,
  setIsRecording: (value: boolean) => void,
  showModal?: (name: 'modelSelector', message?: string) => void,
  prepareRecordingMetadata?: () => Promise<PreparedRecordingMetadata>,
): UseRecordingStartReturn {
  const { t } = useTranslation(['recording', 'transcription']);
  const [isAutoStarting, setIsAutoStarting] = useState(false);
  const { clearTranscripts, setMeetingTitle } = useTranscripts();
  const { setIsMeetingActive } = useSidebar();
  const { selectedDevices, transcriptModelConfig, selectedLanguage, setSelectedLanguage } = useConfig();
  const { setStatus } = useRecordingState();

  const generateMeetingTitle = useCallback(() => {
    const now = new Date();
    const timestamp = [
      String(now.getDate()).padStart(2, '0'),
      String(now.getMonth() + 1).padStart(2, '0'),
      String(now.getFullYear()).slice(-2),
      String(now.getHours()).padStart(2, '0'),
      String(now.getMinutes()).padStart(2, '0'),
      String(now.getSeconds()).padStart(2, '0'),
    ].join('_');
    return t('labels.generatedMeetingTitle', { timestamp });
  }, [t]);

  const prepareAndStart = useCallback(async (source: RecordingStartSource): Promise<boolean> => {
    if (needsMultilingualSetup(transcriptModelConfig.provider, transcriptModelConfig.model)) {
      showModal?.('modelSelector', t('transcription:setup.required'));
      toast.info(t('transcription:setup.required'));
      setStatus(RecordingStatus.IDLE);
      return false;
    }
    if (transcriptModelConfig.provider === 'sensevoice') {
      // The SenseVoice engine loads its own model from the shared models root;
      // the Whisper catalogue does not describe it.
      try {
        await invoke('sensevoice_validate_model_ready');
      } catch (error) {
        console.warn('SenseVoice model is not ready', error);
        toast.error(t('errors.transcriptionModelNotReady'), {
          description: t('descriptions.pleaseDownloadATranscriptionModelBeforeRecording'),
          duration: 5000,
        });
        setStatus(RecordingStatus.IDLE);
        return false;
      }
    } else {
      await WhisperAPI.init();
      const models = await WhisperAPI.getAvailableModels();
      const model = models.find(item => item.name === transcriptModelConfig.model);
      if (model?.status !== 'Available') {
        const isDownloading = typeof model?.status === 'object' && 'Downloading' in model.status;
        if (isDownloading) {
          toast.info(t('messages.modelDownloadInProgress'), {
            description: t('descriptions.pleaseWaitForTheTranscriptionModelToFinishDownloadingBefore'),
            duration: 5000,
          });
          Analytics.trackButtonClick('start_recording_blocked_downloading', source);
        } else {
          toast.error(t('errors.transcriptionModelNotReady'), {
            description: t('descriptions.pleaseDownloadATranscriptionModelBeforeRecording'),
            duration: 5000,
          });
          showModal?.('modelSelector', t('errors.transcriptionModelNotReady'));
          Analytics.trackButtonClick('start_recording_blocked_missing', source);
        }
        setStatus(RecordingStatus.IDLE);
        return false;
      }
    }

    const meetingName = generateMeetingTitle();
    // Await the saved source language before any of the recording entry points.
    await setSelectedLanguage(selectedLanguage);
    const metadata = prepareRecordingMetadata
      ? await prepareRecordingMetadata()
      : { templateSelection: null, meetingContextDraft: null };
    setMeetingTitle(meetingName);
    setStatus(RecordingStatus.STARTING, t('status.initializingRecording'));
    await recordingService.startRecordingWithDevices(
      selectedDevices?.micDevice || null,
      selectedDevices?.systemDevice || null,
      meetingName,
      selectedDevices?.recordingMode || 'microphone_and_system',
      metadata,
    );

    setIsRecording(true);
    clearTranscripts();
    setIsMeetingActive(true);
    Analytics.trackButtonClick('start_recording', source);
    try {
      await showRecordingNotification();
    } catch (notificationError) {
      console.warn('Recording started but the notification failed:', notificationError);
    }
    return true;
  }, [
    clearTranscripts,
    generateMeetingTitle,
    prepareRecordingMetadata,
    selectedDevices,
    selectedLanguage,
    setSelectedLanguage,
    transcriptModelConfig,
    setIsMeetingActive,
    setIsRecording,
    setMeetingTitle,
    setStatus,
    showModal,
    t,
  ]);

  const handleRecordingStart = useCallback(async () => {
    try {
      await prepareAndStart('home_page');
    } catch (error) {
      console.error('Failed to start recording:', error);
      setStatus(RecordingStatus.ERROR, t('errors.failedToStartRecording'));
      setIsRecording(false);
      Analytics.trackButtonClick('start_recording_error', 'home_page');
      throw error;
    }
  }, [prepareAndStart, setIsRecording, setStatus, t]);

  useEffect(() => {
    const checkAutoStartRecording = async () => {
      let shouldAutoStart = false;
      try {
        const appSessionId = await invoke<string>('get_app_session_id');
        shouldAutoStart = consumeAutoStartRecordingRequest(sessionStorage, appSessionId);
      } catch (error) {
        sessionStorage.removeItem('autoStartRecording');
        console.error('Failed to validate auto-start recording request:', error);
      }
      if (!shouldAutoStart || isRecording || isAutoStarting) return;

      setIsAutoStarting(true);
      try {
        await prepareAndStart('sidebar_auto');
      } catch (error) {
        console.error('Failed to auto-start recording:', error);
        setStatus(RecordingStatus.ERROR, t('errors.failedToAutoStartRecording'));
        toast.error(t('errors.failedToStartRecordingCheckConsoleForDetails'));
        Analytics.trackButtonClick('start_recording_error', 'sidebar_auto');
      } finally {
        setIsAutoStarting(false);
      }
    };
    void checkAutoStartRecording();
  }, [isAutoStarting, isRecording, prepareAndStart, setStatus, t]);

  useEffect(() => {
    const handleDirectStart = async () => {
      if (isRecording || isAutoStarting) return;
      setIsAutoStarting(true);
      try {
        await prepareAndStart('sidebar_direct');
      } catch (error) {
        console.error('Failed to start recording from sidebar:', error);
        setStatus(RecordingStatus.ERROR, t('errors.failedToStartRecordingFromSidebar'));
        toast.error(t('errors.failedToStartRecordingCheckConsoleForDetails'));
        Analytics.trackButtonClick('start_recording_error', 'sidebar_direct');
      } finally {
        setIsAutoStarting(false);
      }
    };
    window.addEventListener('start-recording-from-sidebar', handleDirectStart);
    return () => window.removeEventListener('start-recording-from-sidebar', handleDirectStart);
  }, [isAutoStarting, isRecording, prepareAndStart, setStatus, t]);

  return { handleRecordingStart, isAutoStarting };
}

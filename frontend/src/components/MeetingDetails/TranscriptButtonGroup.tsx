"use client";

import { useState, useCallback } from 'react';
import { Button } from '@/components/ui/button';
import { ButtonGroup } from '@/components/ui/button-group';
import { Copy, FolderOpen, RefreshCw } from 'lucide-react';
import Analytics from '@/lib/analytics';
import { RetranscribeDialog } from './RetranscribeDialog';
import { useConfig } from '@/contexts/ConfigContext';
import { useTranslation } from 'react-i18next';


interface TranscriptButtonGroupProps {
  transcriptCount: number;
  onCopyTranscript: () => void;
  onOpenMeetingFolder: () => Promise<void>;
  meetingId?: string;
  meetingFolderPath?: string | null;
  onRefetchTranscripts?: () => Promise<void>;
  /** 转写单独成页时按钮靠右，跟摘要页右上角的图标簇呼应 */
  align?: 'center' | 'end';
}


export function TranscriptButtonGroup({
  transcriptCount,
  onCopyTranscript,
  onOpenMeetingFolder,
  meetingId,
  meetingFolderPath,
  onRefetchTranscripts,
  align = 'center',
}: TranscriptButtonGroupProps) {
  const { t } = useTranslation('meetings');
  const { betaFeatures } = useConfig();
  const [showRetranscribeDialog, setShowRetranscribeDialog] = useState(false);

  const handleRetranscribeComplete = useCallback(async () => {
    // Refetch transcripts to show the updated data
    if (onRefetchTranscripts) {
      await onRefetchTranscripts();
    }
  }, [onRefetchTranscripts]);

  return (
    <div className={`flex w-full items-center gap-2 ${align === 'end' ? 'justify-end' : 'justify-center'}`}>
      <ButtonGroup>
        <Button
          variant="outline"
          size="sm"
          onClick={() => {
            Analytics.trackButtonClick('copy_transcript', 'meeting_details');
            onCopyTranscript();
          }}
          disabled={transcriptCount === 0}
          title={transcriptCount === 0 ? t('errors.noTranscriptAvailable') : t('accessibilityActions.copyTranscript')}
          aria-label={transcriptCount === 0 ? t('errors.noTranscriptAvailable') : t('accessibilityActions.copyTranscript')}
        >
          <Copy />
          <span className="hidden lg:inline">{t('actions.copy')}</span>
        </Button>

        <Button
          size="sm"
          variant="outline"
          className="xl:px-4"
          onClick={() => {
            Analytics.trackButtonClick('open_recording_folder', 'meeting_details');
            onOpenMeetingFolder();
          }}
          title={t('actions.openRecordingFolder')}
          aria-label={t('actions.openRecordingFolder')}
        >
          <FolderOpen className="xl:mr-2" size={18} />
          <span className="hidden lg:inline">{t('labels.recording')}</span>
        </Button>

        {meetingId && (
          (betaFeatures.importAndRetranscribe && Boolean(meetingFolderPath))
          || betaFeatures.moss_post_meeting_enhancement
        ) && (
          <Button
            size="sm"
            variant="outline"
            className="bg-gradient-to-r from-blue-50 to-purple-50 hover:from-blue-100 hover:to-purple-100 border-blue-200 xl:px-4"
            onClick={() => {
              Analytics.trackButtonClick('enhance_transcript', 'meeting_details');
              setShowRetranscribeDialog(true);
            }}
            title={t('accessibilityActions.enhanceRecordedAudio')}
            aria-label={t('accessibilityActions.enhanceRecordedAudio')}
          >
            <RefreshCw className="xl:mr-2" size={18} />
            <span className="hidden lg:inline">{t('actions.enhanceTranscript')}</span>
          </Button>
        )}
      </ButtonGroup>

      {meetingId && (
        (betaFeatures.importAndRetranscribe && Boolean(meetingFolderPath))
        || betaFeatures.moss_post_meeting_enhancement
      ) && (
        <RetranscribeDialog
          open={showRetranscribeDialog}
          onOpenChange={setShowRetranscribeDialog}
          meetingId={meetingId}
          meetingFolderPath={meetingFolderPath ?? null}
          standardRetranscriptionEnabled={betaFeatures.importAndRetranscribe && Boolean(meetingFolderPath)}
          onComplete={handleRetranscribeComplete}
          existingTranscriptCount={transcriptCount}
        />
      )}
    </div>
  );
}
